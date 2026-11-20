//! MIO Plugin Authoring CLI (`scaffold`, `check`, `pack`).

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    ffi::OsString,
    fmt::Write as _,
    fs,
    io::{self, Write},
    path::{Component, Path, PathBuf},
    process::ExitCode,
};

use anyhow::{Context, Result, bail, ensure};
use flate2::{Compression, write::GzEncoder};
use mio_core::{
    plugins::{PluginManifest, VerifiedPackage, wasm::build_envelope_wasm_module},
    protocol::Permission,
};
use serde::{Deserialize, Serialize};

const SHA256_INIT: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];
const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineKind {
    Wasm,
    Native,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackSummary {
    pub id: String,
    pub version: String,
    pub engine: String,
    pub entry: String,
    pub manifest_sha256: String,
    pub entry_sha256: String,
    pub entry_bytes: u64,
}

pub fn sha256_hex(input: &[u8]) -> String {
    let mut state = SHA256_INIT;
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity((input.len() + 9).div_ceil(64) * 64);
    padded.extend_from_slice(input);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for block in padded.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (idx, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[idx] = u32::from_be_bytes(*word);
        }
        for idx in 16..64 {
            let s0 =
                w[idx - 15].rotate_right(7) ^ w[idx - 15].rotate_right(18) ^ (w[idx - 15] >> 3);
            let s1 = w[idx - 2].rotate_right(17) ^ w[idx - 2].rotate_right(19) ^ (w[idx - 2] >> 10);
            w[idx] = w[idx - 16]
                .wrapping_add(s0)
                .wrapping_add(w[idx - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for idx in 0..64 {
            let sum1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(sum1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[idx])
                .wrapping_add(w[idx]);
            let sum0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(sum0.wrapping_add(maj));
        }
        for (slot, delta) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(delta);
        }
    }
    let mut hex = String::with_capacity(64);
    for word in state {
        for byte in word.to_be_bytes() {
            let _ = write!(hex, "{byte:02x}");
        }
    }
    hex
}

fn validate_plugin_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-')),
        "插件 id 无效: {id}"
    );
    Ok(())
}

pub fn scaffold(dir: &Path, plugin_id: &str, engine: EngineKind) -> Result<PluginManifest> {
    validate_plugin_id(plugin_id)?;
    fs::create_dir_all(dir.join("src")).context("无法创建插件脚手架目录")?;
    let (engine_str, entry_name) = match engine {
        EngineKind::Wasm => ("wasm", "plugin.wasm"),
        EngineKind::Native => ("native", "worker"),
    };
    let manifest = PluginManifest {
        id: plugin_id.to_owned(),
        version: "0.1.0".into(),
        api_version: 1,
        engine: engine_str.into(),
        entry: PathBuf::from(entry_name),
        permissions: vec![Permission::TrafficRead, Permission::Storage],
        contributions: vec!["suite_panel".into(), "passive_check".into()],
    };
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    match engine {
        EngineKind::Wasm => {
            fs::write(
                dir.join("Cargo.toml"),
                format!(
                    "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[profile.release]\npanic = \"abort\"\nlto = true\n",
                    plugin_id.replace('.', "-")
                ),
            )?;
            fs::write(
                dir.join("src/lib.rs"),
                format!(
                    "//! MIO WASM plugin `{plugin_id}` (`wasm32-unknown-unknown`, zero-WASI).\n#![no_std]\n\n#[cfg(not(test))]\n#[panic_handler]\nfn panic(_: &core::panic::PanicInfo) -> ! {{\n    core::arch::wasm32::unreachable()\n}}\n\n#[link(wasm_import_module = \"mio_host\")]\nunsafe extern \"C\" {{\n    fn host_call(ptr: *mut u8, req_len: usize, out_cap: usize) -> i32;\n}}\n\n#[unsafe(no_mangle)]\npub extern \"C\" fn mio_alloc(_len: usize) -> *mut u8 {{\n    0x4000 as *mut u8\n}}\n\n#[unsafe(no_mangle)]\npub extern \"C\" fn mio_dispatch(_ptr: *const u8, _len: usize) -> u64 {{\n    let _ = host_call as usize;\n    0\n}}\n"
                ),
            )?;
            fs::write(dir.join(entry_name), build_envelope_wasm_module())?;
        }
        EngineKind::Native => {
            fs::write(
                dir.join("Cargo.toml"),
                format!(
                    "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nmio-plugin-sdk = \"0.1.0\" # Or: mio-plugin-sdk = {{ path = \"/path/to/MIO/crates/mio-plugin-sdk\" }}\n",
                    plugin_id.replace('.', "-")
                ),
            )?;
            fs::write(
                dir.join("src/main.rs"),
                format!(
                    "//! MIO Native Worker plugin `{plugin_id}` built with `mio-plugin-sdk`.\nuse std::io;\nuse mio_plugin_sdk::{{HostClient, MioPlugin, protocol::*, run_stdio, ui}};\n\nstruct Plugin;\n\nimpl MioPlugin for Plugin {{\n    fn id(&self) -> &str {{\n        \"{plugin_id}\"\n    }}\n\n    fn on_passive_check(\n        &mut self,\n        _ctx: &mut dyn HostClient,\n        call: PassiveCheckCall,\n    ) -> Result<PassiveCheckResult, String> {{\n        Ok(PassiveCheckResult {{\n            findings: vec![],\n            log_line: format!(\"checked {{}}\", call.url),\n        }})\n    }}\n}}\n\nfn main() -> io::Result<()> {{\n    run_stdio(Plugin)\n}}\n"
                ),
            )?;
            let worker = dir.join(entry_name);
            fs::write(&worker, [0xcf, 0xfa, 0xed, 0xfe])?;
            #[cfg(unix)]
            fs::set_permissions(&worker, fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(manifest)
}

fn check_no_symlinks_under_entry(base_dir: &Path, rel_entry: &Path) -> Result<()> {
    ensure!(
        !rel_entry.as_os_str().is_empty()
            && rel_entry
                .components()
                .all(|c| matches!(c, Component::Normal(_))),
        "插件入口必须是包内相对路径"
    );
    let mut cursor = base_dir.to_path_buf();
    for comp in rel_entry.components() {
        cursor.push(comp);
        let meta = fs::symlink_metadata(&cursor).context("无法读取插件入口元数据")?;
        ensure!(
            !meta.file_type().is_symlink(),
            "拒绝通过软链接暴露插件入口: {}",
            cursor.display()
        );
    }
    Ok(())
}

pub fn check_package(dir: &Path) -> Result<VerifiedPackage> {
    let meta = fs::symlink_metadata(dir).context("插件包目录不存在")?;
    ensure!(!meta.file_type().is_symlink(), "插件包根目录不能是软链接");
    let manifest_meta =
        fs::symlink_metadata(dir.join("manifest.json")).context("缺少 manifest.json")?;
    ensure!(
        !manifest_meta.file_type().is_symlink(),
        "manifest.json 不能是软链接"
    );
    let pkg = VerifiedPackage::read(dir)?;
    check_no_symlinks_under_entry(&pkg.directory, &pkg.manifest.entry)?;
    let sha_manifest_path = pkg.directory.join("sha256-manifest.json");
    if let Ok(sha_meta) = fs::symlink_metadata(&sha_manifest_path) {
        ensure!(
            sha_meta.is_file() && !sha_meta.file_type().is_symlink(),
            "sha256-manifest.json 必须是普通文件"
        );
        let expected: PackSummary = serde_json::from_slice(&fs::read(&sha_manifest_path)?)
            .context("解析 sha256-manifest.json 失败")?;
        let actual_manifest_sha = sha256_hex(&fs::read(pkg.directory.join("manifest.json"))?);
        let actual_entry_bytes = fs::read(&pkg.executable)?;
        let actual_entry_sha = sha256_hex(&actual_entry_bytes);
        ensure!(
            expected.id == pkg.manifest.id
                && expected.version == pkg.manifest.version
                && expected.engine == pkg.manifest.engine
                && expected.entry == pkg.manifest.entry.to_string_lossy()
                && expected.manifest_sha256 == actual_manifest_sha
                && expected.entry_sha256 == actual_entry_sha
                && expected.entry_bytes == actual_entry_bytes.len() as u64,
            "sha256-manifest.json 完整性校验不匹配"
        );
    }
    Ok(pkg)
}

fn append_tar_entry(tar: &mut Vec<u8>, rel_path: &str, mode: u32, data: &[u8]) -> Result<()> {
    ensure!(
        !rel_path.is_empty() && rel_path.len() <= 100 && !rel_path.starts_with('/'),
        "归档内条目路径超长或非法: {rel_path}"
    );
    let mut header = [0u8; 512];
    header[..rel_path.len()].copy_from_slice(rel_path.as_bytes());
    write_tar_octal(&mut header[100..108], u64::from(mode), 7)?;
    write_tar_octal(&mut header[108..116], 0, 7)?;
    write_tar_octal(&mut header[116..124], 0, 7)?;
    write_tar_octal(&mut header[124..136], data.len() as u64, 11)?;
    write_tar_octal(&mut header[136..148], 0, 11)?;
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum: u32 = header.iter().map(|&b| u32::from(b)).sum();
    let chk = format!("{checksum:06o}\0 ");
    header[148..156].copy_from_slice(chk.as_bytes());
    tar.extend_from_slice(&header);
    tar.extend_from_slice(data);
    let rem = data.len() % 512;
    if rem != 0 {
        tar.resize(tar.len() + (512 - rem), 0);
    }
    Ok(())
}

fn write_tar_octal(field: &mut [u8], value: u64, digits: usize) -> Result<()> {
    let formatted = format!("{value:0width$o}", width = digits);
    let bytes = formatted.as_bytes();
    ensure!(bytes.len() <= digits, "tar 归档头部八进制字段超限");
    field[..bytes.len()].copy_from_slice(bytes);
    Ok(())
}

pub fn pack_package(src_dir: &Path, dest_dir: &Path) -> Result<PackSummary> {
    let pkg = check_package(src_dir)?;
    let manifest_bytes = fs::read(pkg.directory.join("manifest.json"))?;
    let entry_bytes = fs::read(&pkg.executable)?;
    check_package(src_dir)?;
    let summary = PackSummary {
        id: pkg.manifest.id.clone(),
        version: pkg.manifest.version.clone(),
        engine: pkg.manifest.engine.clone(),
        entry: pkg.manifest.entry.to_string_lossy().into_owned(),
        manifest_sha256: sha256_hex(&manifest_bytes),
        entry_sha256: sha256_hex(&entry_bytes),
        entry_bytes: entry_bytes.len() as u64,
    };
    let summary_bytes = serde_json::to_vec_pretty(&summary)?;
    let dest_name = dest_dir.to_string_lossy();
    if dest_name.ends_with(".tar.gz") || dest_name.ends_with(".tgz") {
        if let Some(parent) = dest_dir.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).context("无法创建归档父目录")?;
        }
        let mut tar = Vec::new();
        append_tar_entry(&mut tar, "manifest.json", 0o644, &manifest_bytes)?;
        let entry_rel = pkg.manifest.entry.to_string_lossy().replace('\\', "/");
        let entry_mode = if pkg.manifest.engine == "native" {
            0o755
        } else {
            0o644
        };
        append_tar_entry(&mut tar, &entry_rel, entry_mode, &entry_bytes)?;
        append_tar_entry(&mut tar, "sha256-manifest.json", 0o644, &summary_bytes)?;
        tar.resize(tar.len() + 1024, 0);
        let file = fs::File::create(dest_dir).context("无法创建 .tar.gz 归档文件")?;
        let mut gz = GzEncoder::new(file, Compression::default());
        gz.write_all(&tar)?;
        gz.finish()?;
        return Ok(summary);
    }

    fs::create_dir_all(dest_dir).context("无法创建打包输出目录")?;
    let canon_dest = dest_dir.canonicalize().context("打包输出目录无效")?;
    ensure!(
        canon_dest != pkg.directory,
        "打包目标目录不能与源插件目录相同"
    );

    let dest_manifest = canon_dest.join("manifest.json");
    let dest_entry = canon_dest.join(&pkg.manifest.entry);
    if let Some(parent) = dest_entry.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&dest_manifest, &manifest_bytes)?;
    fs::write(&dest_entry, &entry_bytes)?;
    #[cfg(unix)]
    if pkg.manifest.engine == "native" {
        fs::set_permissions(&dest_entry, fs::Permissions::from_mode(0o755))?;
    }
    fs::write(canon_dest.join("sha256-manifest.json"), &summary_bytes)?;
    check_package(&canon_dest)?;
    Ok(summary)
}

pub fn run_cli<I: IntoIterator<Item = OsString>>(args: I, out: &mut impl Write) -> Result<()> {
    let args: Vec<String> = args
        .into_iter()
        .map(|s| {
            s.into_string()
                .map_err(|_| anyhow::anyhow!("参数必须是有效 UTF-8"))
        })
        .collect::<Result<_>>()?;
    match args.first().map(String::as_str) {
        Some("scaffold") => {
            ensure!(
                (3..=4).contains(&args.len()),
                "用法: scaffold <dir> <plugin_id> [--wasm|--native]"
            );
            let engine = match args.get(3).map(String::as_str) {
                None | Some("--wasm") => EngineKind::Wasm,
                Some("--native") => EngineKind::Native,
                Some(flag) => bail!("未知引擎参数: {flag}（支持 --wasm 或 --native）"),
            };
            let manifest = scaffold(Path::new(&args[1]), &args[2], engine)?;
            writeln!(
                out,
                "scaffolded {} ({}) -> {}",
                manifest.id, manifest.engine, args[1]
            )?;
        }
        Some("check") => {
            ensure!(args.len() == 2, "用法: check <dir>");
            let pkg = check_package(Path::new(&args[1]))?;
            writeln!(
                out,
                "checked {} {} (engine={}, contributions={})",
                pkg.manifest.id,
                pkg.manifest.version,
                pkg.manifest.engine,
                pkg.manifest.contributions.join(",")
            )?;
        }
        Some("pack") => {
            ensure!(args.len() == 3, "用法: pack <src_dir> <dest_dir>");
            let summary = pack_package(Path::new(&args[1]), Path::new(&args[2]))?;
            writeln!(out, "{}", serde_json::to_string_pretty(&summary)?)?;
        }
        _ => bail!(
            "用法: mio_plugin_cli <scaffold <dir> <id> [--wasm|--native] | check <dir> | pack <src> <dest>>"
        ),
    }
    Ok(())
}

fn main() -> ExitCode {
    match run_cli(std::env::args_os().skip(1), &mut io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("mio_plugin_cli error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_check_and_pack_wasm_and_native_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let wasm_src = tmp.path().join("src-wasm");
        let wasm_out = tmp.path().join("dist-wasm");
        let mut buf = Vec::new();
        run_cli(
            [
                "scaffold",
                wasm_src.to_str().unwrap(),
                "lab.cli.wasm",
                "--wasm",
            ]
            .map(OsString::from),
            &mut buf,
        )
        .unwrap();
        assert!(wasm_src.join("src/lib.rs").is_file());

        buf.clear();
        run_cli(
            ["check", wasm_src.to_str().unwrap()].map(OsString::from),
            &mut buf,
        )
        .unwrap();
        assert!(String::from_utf8_lossy(&buf).contains("checked lab.cli.wasm"));

        buf.clear();
        run_cli(
            [
                "pack",
                wasm_src.to_str().unwrap(),
                wasm_out.to_str().unwrap(),
            ]
            .map(OsString::from),
            &mut buf,
        )
        .unwrap();
        let summary: PackSummary = serde_json::from_slice(&buf).unwrap();
        assert_eq!(summary.id, "lab.cli.wasm");
        assert_eq!(summary.engine, "wasm");
        assert_eq!(summary.manifest_sha256.len(), 64);
        assert_eq!(summary.entry_sha256.len(), 64);
        assert!(wasm_out.join("sha256-manifest.json").is_file());
        check_package(&wasm_out).unwrap();
        let wasm_tgz = tmp.path().join("lab.cli.wasm.tar.gz");
        pack_package(&wasm_src, &wasm_tgz).unwrap();
        let tgz_bytes = fs::read(&wasm_tgz).unwrap();
        assert_eq!(
            &tgz_bytes[..2],
            &[0x1f, 0x8b],
            "must write real gzip archive"
        );

        // Tampering entry after pack fails sha256-manifest.json verification.
        fs::write(wasm_out.join("plugin.wasm"), build_envelope_wasm_module()).unwrap();
        let mut bad_summary = summary.clone();
        bad_summary.entry_sha256 = "0".repeat(64);
        fs::write(
            wasm_out.join("sha256-manifest.json"),
            serde_json::to_vec_pretty(&bad_summary).unwrap(),
        )
        .unwrap();
        assert!(check_package(&wasm_out).is_err());

        let native_src = tmp.path().join("src-native");
        scaffold(&native_src, "lab.cli.native", EngineKind::Native).unwrap();
        assert_eq!(
            check_package(&native_src).unwrap().manifest.engine,
            "native"
        );
    }

    #[test]
    fn check_rejects_path_traversal_symlink_escape_and_invalid_wasm() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bad-escape");
        scaffold(&dir, "lab.escape", EngineKind::Wasm).unwrap();

        // 1. Relative `..` path traversal rejected
        let outside = tmp.path().join("outside.wasm");
        fs::write(&outside, build_envelope_wasm_module()).unwrap();
        fs::write(
            dir.join("manifest.json"),
            br#"{"id":"lab.escape","version":"0.1.0","api_version":1,"engine":"wasm","entry":"../outside.wasm","permissions":[],"contributions":["suite_panel"]}"#,
        )
        .unwrap();
        assert!(check_package(&dir).is_err());

        // 2. Symlink escape inside package directory rejected
        #[cfg(unix)]
        {
            fs::remove_file(dir.join("plugin.wasm")).unwrap();
            std::os::unix::fs::symlink(&outside, dir.join("plugin.wasm")).unwrap();
            fs::write(
            dir.join("manifest.json"),
            br#"{"id":"lab.escape","version":"0.1.0","api_version":1,"engine":"wasm","entry":"plugin.wasm","permissions":[],"contributions":["suite_panel"]}"#,
        )
        .unwrap();
            assert!(check_package(&dir).is_err());
        }

        // 3. Corrupted WASM binary rejected
        let _ = fs::remove_file(dir.join("plugin.wasm"));
        fs::write(dir.join("plugin.wasm"), b"NOT_WASM").unwrap();
        assert!(check_package(&dir).is_err());
    }
}
