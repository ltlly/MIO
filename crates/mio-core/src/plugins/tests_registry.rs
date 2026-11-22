use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use mio_plugin_api::protocol::Permission;

use super::{
    NativeTrust, VerifiedPackage,
    registry::{InstalledPluginEntry, PluginRegistryStore},
};

#[cfg(windows)]
const NATIVE_MAGIC: [u8; 4] = [b'M', b'Z', 0, 0];
#[cfg(not(windows))]
const NATIVE_MAGIC: [u8; 4] = [0xcf, 0xfa, 0xed, 0xfe];

fn build_tar_gz(entries: &[(&str, u32, &[u8])]) -> Vec<u8> {
    use std::io::Write;

    use flate2::{Compression, write::GzEncoder};

    let mut tar = Vec::new();
    for (name, mode, data) in entries {
        let mut header = [0u8; 512];
        let name_bytes = name.as_bytes();
        header[..name_bytes.len()].copy_from_slice(name_bytes);
        let write_oct = |dst: &mut [u8], val: u64, width: usize| {
            let s = format!("{val:0width$o}", width = width);
            dst[..width].copy_from_slice(s.as_bytes());
        };
        write_oct(&mut header[100..108], u64::from(*mode), 7);
        write_oct(&mut header[108..116], 0, 7);
        write_oct(&mut header[116..124], 0, 7);
        write_oct(&mut header[124..136], data.len() as u64, 11);
        write_oct(&mut header[136..148], 0, 11);
        header[148..156].fill(b' ');
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let chk: u32 = header.iter().map(|&b| u32::from(b)).sum();
        let chk_s = format!("{chk:06o}\0 ");
        header[148..156].copy_from_slice(chk_s.as_bytes());
        tar.extend_from_slice(&header);
        tar.extend_from_slice(data);
        let rem = data.len() % 512;
        if rem != 0 {
            tar.resize(tar.len() + (512 - rem), 0);
        }
    }
    tar.resize(tar.len() + 1024, 0);
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar).unwrap();
    gz.finish().unwrap()
}

fn write_native_dir(dir: &std::path::Path, id: &str, version: &str) {
    fs::create_dir_all(dir).unwrap();
    let worker = dir.join("worker");
    fs::write(&worker, NATIVE_MAGIC).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&worker, fs::Permissions::from_mode(0o755)).unwrap();
    let manifest = serde_json::json!({
        "id": id,
        "version": version,
        "api_version": 1,
        "engine": "native",
        "entry": "worker",
        "permissions": ["traffic.read"],
        "contributions": ["suite_panel"],
    });
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

#[test]
fn registry_stages_directory_and_tar_gz_into_packages_dir_and_survives_source_delete() {
    let temp = tempfile::tempdir().unwrap();
    let plugins_root = temp.path().join("user-plugins");
    let store = PluginRegistryStore::new(Some(plugins_root.clone()));

    let src_dir = temp.path().join("src-dir");
    write_native_dir(&src_dir, "lab.dir", "1.0.0");
    let dir_pkg = VerifiedPackage::read(&src_dir).unwrap();
    let (staged_dir_pkg, dir_staging) = store.stage_install_package(&dir_pkg).unwrap().unwrap();
    assert_eq!(dir_staging.package_dir, "lab.dir");
    store
        .upsert(InstalledPluginEntry {
            id: "lab.dir".into(),
            version: "1.0.0".into(),
            engine: "native".into(),
            package_dir: dir_staging.package_dir.clone(),
            original_source: Some(dir_pkg.directory.clone()),
            grants: vec![Permission::TrafficRead],
            enabled: true,
            auto_reload: true,
        })
        .unwrap();
    dir_staging.commit();
    assert!(NativeTrust::confirmed(&staged_dir_pkg, [Permission::TrafficRead]).is_ok());

    let arch_manifest = br#"{"id":"lab.arch","version":"0.2.0","api_version":1,"engine":"native","entry":"bin/worker","permissions":["traffic.read"],"contributions":["suite_panel"]}"#;
    let arch_path = temp.path().join("plugin.tar.gz");
    fs::write(
        &arch_path,
        build_tar_gz(&[
            ("manifest.json", 0o644, arch_manifest),
            ("bin/worker", 0o755, &NATIVE_MAGIC),
        ]),
    )
    .unwrap();
    let arch_pkg = VerifiedPackage::read(&arch_path).unwrap();
    let (staged_arch_pkg, arch_staging) = store.stage_install_package(&arch_pkg).unwrap().unwrap();
    store
        .upsert(InstalledPluginEntry {
            id: "lab.arch".into(),
            version: "0.2.0".into(),
            engine: "native".into(),
            package_dir: arch_staging.package_dir.clone(),
            original_source: Some(arch_pkg.directory.clone()),
            grants: vec![Permission::TrafficRead],
            enabled: true,
            auto_reload: false,
        })
        .unwrap();
    arch_staging.commit();

    // Removing both original source inputs leaves persisted packages usable.
    fs::remove_dir_all(&src_dir).unwrap();
    fs::remove_file(&arch_path).unwrap();
    drop(dir_pkg);
    drop(arch_pkg);

    let entries = store.list().unwrap();
    assert_eq!(entries.len(), 2);
    let stored_arch_path = store.package_path(&entries[1]).unwrap();
    let reopened = VerifiedPackage::read(&stored_arch_path).unwrap();
    assert_eq!(reopened.manifest.id, "lab.arch");
    assert_eq!(reopened.directory, staged_arch_pkg.directory);
}

#[cfg(unix)]
#[test]
fn registry_rejects_symlinks_in_source_directory() {
    let temp = tempfile::tempdir().unwrap();
    let store = PluginRegistryStore::new(Some(temp.path().join("user-plugins")));
    let src_dir = temp.path().join("sym-src");
    write_native_dir(&src_dir, "lab.sym", "1.0.0");
    let pkg = VerifiedPackage::read(&src_dir).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", src_dir.join("extra-link")).unwrap();
    let err = store
        .stage_install_package(&pkg)
        .unwrap_err()
        .to_string();
    assert!(err.contains("符号链接"), "unexpected error: {err}");
}

#[test]
fn registry_mutations_and_staging_rollback_work() {
    let temp = tempfile::tempdir().unwrap();
    let plugins_root = temp.path().join("user-plugins");
    let store = PluginRegistryStore::new(Some(plugins_root));

    let src1 = temp.path().join("src1");
    write_native_dir(&src1, "lab.one", "1.0.0");
    let pkg1 = VerifiedPackage::read(&src1).unwrap();
    let (_staged1, staging1) = store.stage_install_package(&pkg1).unwrap().unwrap();
    store
        .upsert(InstalledPluginEntry {
            id: "lab.one".into(),
            version: "1.0.0".into(),
            engine: "native".into(),
            package_dir: staging1.package_dir.clone(),
            original_source: Some(src1.clone()),
            grants: vec![Permission::TrafficRead],
            enabled: true,
            auto_reload: true,
        })
        .unwrap();
    staging1.commit();

    // Failed fresh install rolls back newly created directory.
    let src2 = temp.path().join("src2");
    write_native_dir(&src2, "lab.two", "1.0.0");
    let pkg2 = VerifiedPackage::read(&src2).unwrap();
    let (_, staging2) = store.stage_install_package(&pkg2).unwrap().unwrap();
    let pkg2_dir = staging2.final_dir.clone();
    assert!(pkg2_dir.is_dir());
    staging2.rollback();
    assert!(!pkg2_dir.exists());

    // Re-install src2 + src3 to test reorder keeping disabled slots, set_enabled, set_auto_reload, remove, and upgrade rollback.
    let (_, staging2) = store.stage_install_package(&pkg2).unwrap().unwrap();
    store
        .upsert(InstalledPluginEntry {
            id: "lab.two".into(),
            version: "1.0.0".into(),
            engine: "native".into(),
            package_dir: staging2.package_dir.clone(),
            original_source: Some(src2.clone()),
            grants: vec![Permission::TrafficRead],
            enabled: false,
            auto_reload: false,
        })
        .unwrap();
    staging2.commit();

    let src3 = temp.path().join("src3");
    write_native_dir(&src3, "lab.three", "1.0.0");
    let pkg3 = VerifiedPackage::read(&src3).unwrap();
    let (_, staging3) = store.stage_install_package(&pkg3).unwrap().unwrap();
    store
        .upsert(InstalledPluginEntry {
            id: "lab.three".into(),
            version: "1.0.0".into(),
            engine: "native".into(),
            package_dir: staging3.package_dir.clone(),
            original_source: Some(src3.clone()),
            grants: vec![Permission::TrafficRead],
            enabled: true,
            auto_reload: true,
        })
        .unwrap();
    staging3.commit();

    store
        .reorder_by_ids(&["lab.three".into(), "lab.one".into()])
        .unwrap();
    let list = store.list().unwrap();
    let ids: Vec<_> = list.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["lab.three", "lab.two", "lab.one"]);

    assert!(store.set_enabled("lab.two", true).unwrap());
    assert!(store.set_auto_reload("lab.two", true).unwrap());
    let two = store.get("lab.two").unwrap().unwrap();
    assert!(two.enabled && two.auto_reload);

    // Upgrade rollback restores previous v1.0.0 package.
    let src1_v2 = temp.path().join("src1-v2");
    write_native_dir(&src1_v2, "lab.one", "2.0.0");
    let pkg1_v2 = VerifiedPackage::read(&src1_v2).unwrap();
    let (staged_v2, staging_v2) = store.stage_install_package(&pkg1_v2).unwrap().unwrap();
    assert_eq!(staged_v2.manifest.version, "2.0.0");
    let final_one = staging_v2.final_dir.clone();
    staging_v2.rollback();
    let restored_one = VerifiedPackage::read(&final_one).unwrap();
    assert_eq!(restored_one.manifest.version, "1.0.0");

    assert!(store.remove("lab.two").unwrap());
    assert!(!store.remove("lab.two").unwrap());
    assert!(store.get("lab.two").unwrap().is_none());
    assert!(!pkg2_dir.exists());
}

#[test]
fn registry_allows_listing_and_uninstalling_legacy_wasm_entries_alongside_native() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("user-plugins");
    let store = PluginRegistryStore::new(Some(root.clone()));

    let src = temp.path().join("native-src");
    write_native_dir(&src, "lab.keep", "1.0.0");
    let pkg = VerifiedPackage::read(&src).unwrap();
    let (_, staging) = store.stage_install_package(&pkg).unwrap().unwrap();
    store
        .upsert(InstalledPluginEntry {
            id: "lab.keep".into(),
            version: "1.0.0".into(),
            engine: "native".into(),
            package_dir: staging.package_dir.clone(),
            original_source: None,
            grants: vec![Permission::TrafficRead],
            enabled: true,
            auto_reload: false,
        })
        .unwrap();
    staging.commit();

    let legacy_pkg_dir = root.join("packages").join("lab.legacy_wasm");
    fs::create_dir_all(&legacy_pkg_dir).unwrap();
    let installed_path = root.join("installed.json");
    let mut file_json: serde_json::Value =
        serde_json::from_slice(&fs::read(&installed_path).unwrap()).unwrap();
    file_json["plugins"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": "lab.legacy_wasm",
            "version": "0.1.0",
            "engine": "wasm",
            "package_dir": "lab.legacy_wasm",
            "original_source": null,
            "grants": [],
            "enabled": true,
            "auto_reload": false
        }));
    fs::write(&installed_path, serde_json::to_vec(&file_json).unwrap()).unwrap();

    let entries = store.list().unwrap();
    assert_eq!(entries.len(), 2);
    assert!(store.remove("lab.legacy_wasm").unwrap());
    assert!(!legacy_pkg_dir.exists());
    let remaining = store.list().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, "lab.keep");
}
