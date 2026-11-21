use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use tempfile::NamedTempFile;

use crate::fs_atomic::{
    acquire_sidecar_lock, atomic_write_bytes, ensure_owned_parent_dir, read_bounded_owned_file,
    validate_parent_dir,
};

pub fn write_export_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    fs::write(path, bytes)
}

pub fn read_file_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    fs::read(path)
}

pub fn read_file_to_string(path: &Path) -> std::io::Result<String> {
    fs::read_to_string(path)
}

pub fn file_exists_or_accessible(path: &Path) -> bool {
    !matches!(fs::metadata(path), Err(err) if err.kind() == ErrorKind::NotFound)
}

pub enum RegularFileReadError {
    Open(std::io::Error),
    Metadata(std::io::Error),
    InvalidOrTooLarge,
    TooLargeAfterRead,
    Read(std::io::Error),
}

pub fn read_regular_file_bounded_with_hook(
    path: &Path,
    max_bytes: u64,
    allow_empty: bool,
    on_after_metadata: impl FnOnce(),
) -> std::result::Result<Vec<u8>, RegularFileReadError> {
    #[cfg(not(unix))]
    if let Ok(meta) = fs::symlink_metadata(path)
        && (!meta.is_file() || (!allow_empty && meta.len() == 0) || meta.len() > max_bytes)
    {
        return Err(RegularFileReadError::InvalidOrTooLarge);
    }
    // Nonblocking open lets the same fd reject a replacement FIFO before reading.
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NONBLOCK);
    }
    let file = opts.open(path).map_err(RegularFileReadError::Open)?;
    let metadata = file.metadata().map_err(RegularFileReadError::Metadata)?;
    on_after_metadata();
    if !metadata.is_file() || (!allow_empty && metadata.len() == 0) || metadata.len() > max_bytes {
        return Err(RegularFileReadError::InvalidOrTooLarge);
    }
    let mut bytes = Vec::new();
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(RegularFileReadError::Read)?;
    if !allow_empty && bytes.is_empty() {
        return Err(RegularFileReadError::InvalidOrTooLarge);
    }
    if bytes.len() as u64 > max_bytes {
        return Err(RegularFileReadError::TooLargeAfterRead);
    }
    Ok(bytes)
}

pub fn read_regular_file_bounded(
    path: &Path,
    max_bytes: u64,
    allow_empty: bool,
    label: &str,
) -> std::result::Result<Vec<u8>, String> {
    read_regular_file_bounded_with_hook(path, max_bytes, allow_empty, || {}).map_err(|err| match err
    {
        RegularFileReadError::Open(e) => format!("无法读取{label}：{e}"),
        RegularFileReadError::Metadata(e) => format!("无法读取{label}信息：{e}"),
        RegularFileReadError::InvalidOrTooLarge | RegularFileReadError::TooLargeAfterRead => {
            if allow_empty {
                format!("{label}必须是大小不超过 {max_bytes} 字节的普通文件。")
            } else {
                format!("{label}必须是非空且不超过 {max_bytes} 字节的普通文件。")
            }
        }
        RegularFileReadError::Read(e) => format!("无法完整读取{label}：{e}"),
    })
}

pub fn read_utf8_rule_lines(
    path: &Path,
    max_bytes: u64,
    max_line_bytes: usize,
) -> std::result::Result<Vec<String>, String> {
    let meta = fs::metadata(path).map_err(|err| format!("读取文件元数据失败：{err}"))?;
    if !meta.is_file() {
        return Err("所选路径不是普通文件。".into());
    }
    if meta.len() > max_bytes {
        return Err("规则文件超过 256 KiB 上限，未导入。".into());
    }
    let text = fs::read_to_string(path).map_err(|err| format!("读取 UTF-8 规则文件失败：{err}"))?;
    let entries: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty() && line.len() <= max_line_bytes && !line.chars().any(char::is_control)
        })
        .map(str::to_owned)
        .collect();
    if entries.is_empty() {
        return Err("文件中未找到有效的 URL 前缀规则。".into());
    }
    Ok(entries)
}

pub fn read_owned_file_bounded(
    path: &Path,
    max_bytes: u64,
    label: &str,
) -> Result<Option<Vec<u8>>> {
    read_bounded_owned_file(path, max_bytes, true, label)
}

pub fn locked_cas_update_owned_file<F>(
    path: &Path,
    lock_name: &str,
    max_bytes: u64,
    label: &str,
    transform: F,
) -> Result<Vec<u8>>
where
    F: FnOnce(Option<Vec<u8>>) -> Result<Vec<u8>>,
{
    let (parent, dir) = ensure_owned_parent_dir(path, label)?;
    let _lock = acquire_sidecar_lock(&parent, lock_name, label)?;
    let before = read_bounded_owned_file(path, max_bytes, true, label)?;
    let next_bytes = transform(before.clone())?;
    ensure!(
        next_bytes.len() as u64 <= max_bytes,
        "{label}超过有界写入限额"
    );
    validate_parent_dir(path, label)?;
    ensure!(
        read_bounded_owned_file(path, max_bytes, false, label)? == before,
        "{label}已发生变化，请重新加载"
    );
    atomic_write_bytes(&parent, path, &next_bytes, &dir, label)?;
    Ok(next_bytes)
}

pub fn read_bounded_optional_file(
    path: &Path,
    max_bytes: u64,
    label: &str,
) -> Result<Option<Vec<u8>>> {
    match fs::metadata(path) {
        Ok(meta) => {
            ensure!(meta.is_file(), "{label}路径不是普通文件");
            ensure!(meta.len() <= max_bytes, "{label}文件超过大小上限");
        }
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    }
    let bytes = fs::read(path).with_context(|| format!("读取{label}失败：{}", path.display()))?;
    ensure!(bytes.len() as u64 <= max_bytes, "{label}文件超过大小上限");
    Ok(Some(bytes))
}

pub fn atomic_write_bounded_file(
    path: &Path,
    bytes: &[u8],
    max_bytes: u64,
    label: &str,
) -> Result<()> {
    ensure!(bytes.len() as u64 <= max_bytes, "{label}文件超过大小上限");
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .with_context(|| format!("{label}路径缺少父目录"))?;
    fs::create_dir_all(parent).with_context(|| format!("创建{label}目录失败"))?;
    let mut temp = NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|err| err.error)?;
    Ok(())
}

pub fn write_response_preview_file(
    preview_dir: &Path,
    ext: &str,
    body_bytes: &[u8],
) -> Result<PathBuf> {
    fs::create_dir_all(preview_dir)?;
    let file_path = preview_dir.join(format!(
        "mio-response-preview-{}.{ext}",
        crate::Uuid::new_v4()
    ));
    fs::write(&file_path, body_bytes)?;
    Ok(fs::canonicalize(&file_path).unwrap_or(file_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_files_atomic_cas_and_bounded_readers_work() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let owned_path = root.join("owned.json");

        assert_eq!(
            read_owned_file_bounded(&owned_path, 1024, "测试库").unwrap(),
            None
        );

        let written =
            locked_cas_update_owned_file(&owned_path, "owned.lock", 1024, "测试库", |before| {
                assert!(before.is_none());
                Ok(br#"{"v":1}"#.to_vec())
            })
            .unwrap();
        assert_eq!(written, br#"{"v":1}"#);
        assert_eq!(
            read_owned_file_bounded(&owned_path, 1024, "测试库")
                .unwrap()
                .as_deref(),
            Some(br#"{"v":1}"#.as_slice())
        );

        let script_path = root.join("sub").join("scripts.json");
        atomic_write_bounded_file(&script_path, b"[1,2]", 1024, "脚本库").unwrap();
        assert_eq!(
            read_bounded_optional_file(&script_path, 1024, "脚本库")
                .unwrap()
                .as_deref(),
            Some(b"[1,2]".as_slice())
        );

        let rules_path = root.join("rules.txt");
        write_export_file(&rules_path, b" https://example.invalid \n\nhttp://127.0.0.1/\n").unwrap();
        assert_eq!(
            read_utf8_rule_lines(&rules_path, 1024, 256).unwrap(),
            vec![
                "https://example.invalid".to_owned(),
                "http://127.0.0.1/".to_owned()
            ]
        );
        assert!(file_exists_or_accessible(&rules_path));
        assert!(!file_exists_or_accessible(&root.join("missing")));

        let preview =
            write_response_preview_file(&root.join("previews"), "html", b"<p>ok</p>").unwrap();
        assert_eq!(read_file_to_string(&preview).unwrap(), "<p>ok</p>");
    }
}
