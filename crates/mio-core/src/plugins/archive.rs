use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use anyhow::{Context, Result, bail, ensure};
use flate2::read::GzDecoder;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_UNPACKED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 1024;
const MAX_PAX_RECORD_BYTES: usize = 64 * 1024;
const MAX_LONGNAME_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ArchiveFingerprint {
    pub(crate) size: u64,
    pub(crate) modified: SystemTime,
}

#[derive(Deserialize)]
struct ArchiveShaManifest {
    id: String,
    version: String,
    engine: String,
    entry: String,
    manifest_sha256: String,
    entry_sha256: String,
    entry_bytes: u64,
}

#[derive(Deserialize)]
struct PartialManifestMeta {
    id: String,
    version: String,
    engine: String,
    entry: PathBuf,
}

pub(crate) fn unpack_plugin_archive(
    archive_path: &Path,
) -> Result<(Arc<tempfile::TempDir>, PathBuf, ArchiveFingerprint)> {
    #[cfg(not(unix))]
    if let Ok(meta) = fs::symlink_metadata(archive_path) {
        ensure!(meta.is_file(), "插件归档路径必须是普通文件");
    }
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NONBLOCK);
    }
    let mut file = opts.open(archive_path).context("无法打开单文件插件归档")?;
    let meta = file.metadata().context("无法读取插件归档元数据")?;
    ensure!(meta.is_file(), "插件归档路径必须是普通文件");
    let archive_size = meta.len();
    let archive_modified = meta.modified()?;
    ensure!(
        archive_size <= MAX_ARCHIVE_BYTES,
        "单文件插件归档超过 128 MiB 限额"
    );

    let mut magic = [0u8; 2];
    file.read_exact(&mut magic)
        .context("插件归档过短，无法识别格式")?;
    ensure!(
        magic == [0x1f, 0x8b],
        "仅支持 gzip tar 单文件插件归档（.tar.gz / .tgz / .mio-plugin）"
    );
    file.seek(SeekFrom::Start(0))?;

    let tempdir = tempfile::Builder::new()
        .prefix(".mio-plugin-unpacked-")
        .tempdir()
        .context("无法创建插件临时解包目录")?;
    let work_dir = tempdir
        .path()
        .canonicalize()
        .context("插件临时解包目录无效")?;

    let gz = GzDecoder::new(file);
    let mut reader = gz.take(MAX_UNPACKED_BYTES + 1);
    let mut total_unpacked = 0u64;
    let mut entry_count = 0usize;
    let mut header_count = 0usize;
    let mut seen_files = BTreeSet::new();
    let mut next_long_name: Option<String> = None;
    let mut next_pax_path: Option<String> = None;
    let mut header = [0u8; 512];

    while read_tar_header(&mut reader, &mut header)? {
        if header.iter().all(|&b| b == 0) {
            break;
        }
        verify_tar_checksum(&header)?;
        header_count += 1;
        ensure!(
            header_count <= MAX_ARCHIVE_ENTRIES * 2,
            "插件归档头部记录数超过上限"
        );
        let typeflag = header[156];
        let size = parse_tar_octal(&header[124..136])?;
        let mode = parse_tar_octal(&header[100..108])? as u32;
        let pad = (512 - (size % 512)) % 512;

        match typeflag {
            b'x' => {
                ensure!(
                    next_pax_path.is_none() && next_long_name.is_none(),
                    "插件归档包含连续未关联目标条目的扩展头"
                );
                ensure!(size <= MAX_PAX_RECORD_BYTES as u64, "PAX 扩展头超过 64 KiB 限额");
                let data = read_exact_bounded(&mut reader, size as usize, &mut total_unpacked)?;
                discard_padding(&mut reader, pad, &mut total_unpacked)?;
                next_pax_path = parse_pax_path(&data)?;
                continue;
            }
            b'g' => {
                ensure!(size <= MAX_PAX_RECORD_BYTES as u64, "PAX 全局头超过 64 KiB 限额");
                let data = read_exact_bounded(&mut reader, size as usize, &mut total_unpacked)?;
                discard_padding(&mut reader, pad, &mut total_unpacked)?;
                ensure!(
                    parse_pax_path(&data)?.is_none(),
                    "PAX 全局头禁止覆盖条目的路径"
                );
                continue;
            }
            b'L' => {
                ensure!(
                    next_pax_path.is_none() && next_long_name.is_none(),
                    "插件归档包含连续未关联目标条目的扩展头"
                );
                ensure!(size <= MAX_LONGNAME_BYTES as u64, "GNU 长路径名超过 4 KiB 限额");
                let data = read_exact_bounded(&mut reader, size as usize, &mut total_unpacked)?;
                discard_padding(&mut reader, pad, &mut total_unpacked)?;
                let trimmed = match data.iter().position(|&b| b == 0) {
                    Some(end) => &data[..end],
                    None => &data[..],
                };
                next_long_name = Some(
                    std::str::from_utf8(trimmed)
                        .context("归档长路径不是有效 UTF-8")?
                        .to_owned(),
                );
                continue;
            }
            b'K' => {
                bail!("插件归档禁止包含长链接目标条目");
            }
            b'1' | b'2' => {
                bail!("插件归档禁止包含软链接或硬链接");
            }
            b'3' | b'4' | b'6' => {
                bail!("插件归档禁止包含特殊设备文件或 FIFO");
            }
            b'5' | b'0' | b'\0' => {}
            other => {
                bail!("插件归档包含不支持的 tar 条目类型: 0x{other:02x}");
            }
        }

        entry_count += 1;
        ensure!(
            entry_count <= MAX_ARCHIVE_ENTRIES,
            "插件归档条目数超过 {MAX_ARCHIVE_ENTRIES} 上限"
        );

        let pax_path = next_pax_path.take();
        let long_name = next_long_name.take();
        let raw_name = pax_path
            .or(long_name)
            .unwrap_or_else(|| parse_ustar_path(&header));
        let is_dir = typeflag == b'5' || raw_name.ends_with('/');
        let rel_path = sanitize_archive_rel_path(&raw_name, is_dir)?;

        if is_dir {
            ensure!(size == 0, "目录条目不能包含文件载荷");
            if let Some(rel) = rel_path {
                fs::create_dir_all(work_dir.join(rel))?;
            }
            continue;
        }

        let Some(rel) = rel_path else {
            bail!("插件归档包含无效的空文件名条目");
        };
        ensure!(
            seen_files.insert(rel.clone()),
            "插件归档包含重复文件路径: {}",
            rel.display()
        );

        let dest_path = work_dir.join(&rel);
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&dest_path)
            .with_context(|| format!("无法写出解包文件 {}", rel.display()))?;

        copy_exact_bounded(&mut reader, &mut out_file, size, &mut total_unpacked)?;
        discard_padding(&mut reader, pad, &mut total_unpacked)?;
        out_file.flush()?;
        drop(out_file);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file_mode = if mode & 0o111 != 0 { 0o755 } else { 0o644 };
            fs::set_permissions(&dest_path, fs::Permissions::from_mode(file_mode))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
    }

    ensure!(
        next_pax_path.is_none() && next_long_name.is_none(),
        "插件归档包含未关联目标条目的扩展头"
    );
    verify_optional_sha256_manifest(&work_dir)?;

    Ok((
        Arc::new(tempdir),
        work_dir,
        ArchiveFingerprint {
            size: archive_size,
            modified: archive_modified,
        },
    ))
}

fn read_bounded_plain_file(path: &Path, max_bytes: u64, label: &str) -> Result<Vec<u8>> {
    #[cfg(not(unix))]
    {
        let meta = fs::symlink_metadata(path).with_context(|| format!("读取{label}失败"))?;
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "{label}必须是普通文件"
        );
    }
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = opts.open(path).with_context(|| format!("打开{label}失败"))?;
    let meta = file.metadata()?;
    ensure!(meta.is_file(), "{label}必须是普通文件");
    ensure!(meta.len() <= max_bytes, "{label}超过允许大小上限");
    let mut bytes = Vec::new();
    file.take(max_bytes + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= max_bytes, "{label}在读取时超过大小上限");
    Ok(bytes)
}

pub(super) fn verify_optional_sha256_manifest(work_dir: &Path) -> Result<()> {
    let sha_path = work_dir.join("sha256-manifest.json");
    match fs::symlink_metadata(&sha_path) {
        Ok(meta) => {
            ensure!(
                meta.is_file() && !meta.file_type().is_symlink(),
                "sha256-manifest.json 必须是普通文件"
            );
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).context("检查 sha256-manifest.json 失败"),
    }
    let sha_bytes = read_bounded_plain_file(
        &sha_path,
        super::MAX_MANIFEST_BYTES as u64,
        "sha256-manifest.json",
    )?;
    let expected: ArchiveShaManifest =
        serde_json::from_slice(&sha_bytes).context("解析 sha256-manifest.json 失败")?;

    let manifest_path = work_dir.join("manifest.json");
    let manifest_bytes =
        super::package::read_manifest(&manifest_path).context("归档缺少 manifest.json")?;
    let partial: PartialManifestMeta =
        serde_json::from_slice(&manifest_bytes).context("解析归档内 manifest.json 失败")?;
    let entry_raw = partial.entry.to_string_lossy();
    ensure!(
        !partial.entry.as_os_str().is_empty()
            && !entry_raw.contains('\0')
            && !entry_raw.contains('\\')
            && partial
                .entry
                .components()
                .all(|c| matches!(c, Component::Normal(_))),
        "单文件插件入口必须是包内相对路径，禁止路径穿越"
    );
    let canon_work_dir = work_dir.canonicalize().context("无法解析插件工作目录")?;
    let entry_path = super::package::resolve_entry_path(work_dir, &partial.entry, &partial.engine);
    let canon_entry = entry_path.canonicalize().context("归档缺少插件入口文件")?;
    ensure!(
        canon_entry.starts_with(&canon_work_dir),
        "插件入口不能指向包外"
    );
    let entry_bytes =
        read_bounded_plain_file(&entry_path, MAX_UNPACKED_BYTES, "插件入口文件")?;

    ensure!(
        expected.id == partial.id
            && expected.version == partial.version
            && expected.engine == partial.engine
            && expected.entry == partial.entry.to_string_lossy()
            && expected.manifest_sha256 == sha256_hex(&manifest_bytes)
            && expected.entry_sha256 == sha256_hex(&entry_bytes)
            && expected.entry_bytes == entry_bytes.len() as u64,
        "单文件插件 sha256-manifest.json 完整性校验不匹配"
    );
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn read_tar_header(reader: &mut impl Read, buf: &mut [u8; 512]) -> Result<bool> {
    let mut read = 0usize;
    while read < buf.len() {
        let n = reader.read(&mut buf[read..]).context("读取 tar 归档失败")?;
        if n == 0 {
            if read == 0 {
                return Ok(false);
            }
            bail!("tar 归档头部被截断");
        }
        read += n;
    }
    Ok(true)
}

fn verify_tar_checksum(header: &[u8; 512]) -> Result<()> {
    let expected = parse_tar_octal(&header[148..156])?;
    let unsigned_sum: u64 = header
        .iter()
        .enumerate()
        .map(|(idx, &b)| if (148..156).contains(&idx) { u64::from(b' ') } else { u64::from(b) })
        .sum();
    let signed_sum: i64 = header
        .iter()
        .enumerate()
        .map(|(idx, &b)| {
            if (148..156).contains(&idx) {
                i64::from(b' ')
            } else {
                i64::from(b as i8)
            }
        })
        .sum();
    ensure!(
        expected == unsigned_sum || expected as i64 == signed_sum,
        "tar 归档头部校验和无效"
    );
    Ok(())
}

fn parse_tar_octal(field: &[u8]) -> Result<u64> {
    if field.first().is_some_and(|b| *b & 0x80 != 0) && field.len() <= 12 {
        let mut acc = 0u128;
        for (idx, &b) in field.iter().enumerate() {
            let byte = if idx == 0 { b & 0x7f } else { b };
            acc = (acc << 8) | u128::from(byte);
        }
        return u64::try_from(acc).context("tar 二进制数值字段溢出");
    }
    let s = std::str::from_utf8(field)
        .context("tar 头部数字字段非 UTF-8")?
        .trim_matches(|c: char| c == '\0' || c.is_ascii_whitespace());
    if s.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(s, 8).with_context(|| format!("tar 头部八进制字段无效: {s}"))
}

fn parse_ustar_path(header: &[u8; 512]) -> String {
    let take_cstr = |slice: &[u8]| -> String {
        let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
        String::from_utf8_lossy(&slice[..end]).into_owned()
    };
    let name = take_cstr(&header[0..100]);
    let magic = &header[257..263];
    if magic.starts_with(b"ustar") {
        let prefix = take_cstr(&header[345..500]);
        if !prefix.is_empty() {
            return format!("{prefix}/{name}");
        }
    }
    name
}

fn parse_pax_path(mut data: &[u8]) -> Result<Option<String>> {
    let mut found = None;
    while !data.is_empty() {
        let Some(space_idx) = data.iter().position(|&b| b == b' ') else {
            bail!("PAX 扩展头记录缺少长度分隔符");
        };
        let len_str = std::str::from_utf8(&data[..space_idx]).context("PAX 长度字段非 UTF-8")?;
        let record_len: usize = len_str.parse().context("PAX 长度字段无效")?;
        ensure!(
            record_len > space_idx + 1 && record_len <= data.len(),
            "PAX 扩展头记录长度越界"
        );
        let record = &data[space_idx + 1..record_len];
        let content = record.strip_suffix(b"\n").unwrap_or(record);
        let text = std::str::from_utf8(content).context("PAX 扩展头非 UTF-8")?;
        if let Some((key, val)) = text.split_once('=') {
            if key == "linkpath" {
                bail!("PAX 扩展头禁止包含链接目标属性");
            }
            if key == "path" {
                found = Some(val.to_owned());
            }
        }
        data = &data[record_len..];
    }
    Ok(found)
}

fn sanitize_archive_rel_path(raw: &str, is_dir: bool) -> Result<Option<PathBuf>> {
    ensure!(!raw.contains('\0'), "归档路径包含 NUL 字节");
    ensure!(!raw.contains('\\'), "归档路径禁止包含反斜杠: {raw}");
    ensure!(
        !raw.starts_with('/'),
        "插件归档禁止包含绝对路径: {raw}"
    );
    let trimmed = raw.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "." {
        return Ok(None);
    }
    let mut clean = PathBuf::new();
    for comp in Path::new(trimmed).components() {
        match comp {
            Component::CurDir => {}
            Component::Normal(part) => {
                let s = part.to_string_lossy();
                ensure!(
                    !s.contains(':'),
                    "插件归档禁止包含盘符路径前缀: {raw}"
                );
                clean.push(part);
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("插件归档包含非法目录穿越或绝对路径: {raw}");
            }
        }
    }
    if clean.as_os_str().is_empty() {
        if is_dir {
            return Ok(None);
        }
        bail!("插件归档包含无效的空文件路径: {raw}");
    }
    Ok(Some(clean))
}

fn read_exact_bounded(
    reader: &mut impl Read,
    len: usize,
    total_unpacked: &mut u64,
) -> Result<Vec<u8>> {
    *total_unpacked = total_unpacked
        .checked_add(len as u64)
        .context("归档解压总大小溢出")?;
    ensure!(
        *total_unpacked <= MAX_UNPACKED_BYTES,
        "插件归档解压后超过 256 MiB 限额"
    );
    let mut buf = vec![0u8; len];
    reader
        .read_exact(&mut buf)
        .context("tar 归档扩展记录被截断")?;
    Ok(buf)
}

fn copy_exact_bounded(
    reader: &mut impl Read,
    out: &mut impl Write,
    size: u64,
    total_unpacked: &mut u64,
) -> Result<()> {
    *total_unpacked = total_unpacked
        .checked_add(size)
        .context("归档解压总大小溢出")?;
    ensure!(
        *total_unpacked <= MAX_UNPACKED_BYTES,
        "插件归档解压后超过 256 MiB 限额"
    );
    let mut buf = [0u8; 16 * 1024];
    let mut rem = size;
    while rem > 0 {
        let want = (rem as usize).min(buf.len());
        let n = reader.read(&mut buf[..want]).context("读取归档条目失败")?;
        ensure!(n > 0, "tar 归档文件条目被意外截断");
        out.write_all(&buf[..n])?;
        rem -= n as u64;
    }
    Ok(())
}

fn discard_padding(reader: &mut impl Read, pad: u64, total_unpacked: &mut u64) -> Result<()> {
    if pad == 0 {
        return Ok(());
    }
    *total_unpacked = total_unpacked
        .checked_add(pad)
        .context("归档解压总大小溢出")?;
    ensure!(
        *total_unpacked <= MAX_UNPACKED_BYTES,
        "插件归档解压后超过 256 MiB 限额"
    );
    let mut buf = [0u8; 512];
    reader
        .read_exact(&mut buf[..pad as usize])
        .context("tar 归档块填充被截断")?;
    Ok(())
}
