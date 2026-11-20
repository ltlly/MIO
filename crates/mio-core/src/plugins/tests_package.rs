use super::*;

thread_local! {
static ENTRY_METADATA_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
const { std::cell::RefCell::new(None) };
}

pub(crate) fn after_entry_metadata() {
    if let Some(hook) = ENTRY_METADATA_HOOK.with_borrow_mut(Option::take) {
        hook();
    }
}

#[cfg(unix)]
#[test]
fn executable_magic_is_read_from_the_same_verified_file() {
    use std::{
        ffi::CString,
        io::Write,
        os::unix::{ffi::OsStrExt, fs::OpenOptionsExt, fs::PermissionsExt},
        sync::mpsc::channel,
    };

    let directory = tempfile::tempdir().unwrap();
    let entry = directory.path().join("worker");
    let original = directory.path().join("original-worker");
    fs::write(&entry, [0xcf, 0xfa, 0xed, 0xfe]).unwrap();
    fs::set_permissions(&entry, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
directory.path().join("manifest.json"),
br#"{"id":"lab.entry","version":"1","api_version":1,"engine":"native","entry":"worker","permissions":[],"contributions":["suite_panel"]}"#,
)
.unwrap();
    let package_path = directory.path().to_path_buf();
    let (replaced, checkpoint) = channel();
    let (finished, outcome) = channel();
    let reader = std::thread::spawn(move || {
        ENTRY_METADATA_HOOK.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || {
                fs::rename(&entry, &original).unwrap();
                let path = CString::new(entry.as_os_str().as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o700) }, 0);
                let mut fifo = fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&entry)
                    .unwrap();
                fifo.write_all(b"BAD!").unwrap();
                replaced.send(fifo).unwrap();
            }));
        });
        let checked = VerifiedPackage::read(&package_path);
        let _ = finished.send(checked);
    });
    // Keep the substitute FIFO readable so the old code fails deterministically, never hangs.
    let fifo = checkpoint.recv_timeout(Duration::from_secs(1)).unwrap();
    let checked = outcome.recv_timeout(Duration::from_secs(1));
    reader.join().unwrap();
    drop(fifo);
    let package = checked
        .unwrap()
        .expect("entry bytes must come from the same regular file whose metadata was verified");
    assert_eq!(package.executable_size, 4);
    assert!(
        package
            .unchanged()
            .unwrap_err()
            .to_string()
            .contains("入口")
    );
}

#[test]
fn manifest_read_has_a_hard_byte_limit_and_rejects_non_files() {
    let directory = tempfile::tempdir().unwrap();
    let manifest = directory.path().join("manifest.json");
    let bytes = vec![b'x'; MAX_MANIFEST_BYTES];
    fs::write(&manifest, &bytes).unwrap();
    assert_eq!(read_manifest(&manifest).unwrap(), bytes);
    fs::write(&manifest, vec![b'x'; MAX_MANIFEST_BYTES + 1]).unwrap();
    assert!(
        read_manifest(&manifest)
            .unwrap_err()
            .to_string()
            .contains("16 KiB")
    );
    assert!(
        read_manifest(directory.path())
            .unwrap_err()
            .to_string()
            .contains("普通文件")
    );
    #[cfg(unix)]
    {
        let device = directory.path().join("device.json");
        std::os::unix::fs::symlink("/dev/null", &device).unwrap();
        assert!(
            read_manifest(&device)
                .unwrap_err()
                .to_string()
                .contains("普通文件")
        );
    }
}

#[cfg(unix)]
#[test]
fn manifest_fifo_never_blocks_discovery_or_trust_recheck() {
    use std::{
        ffi::CString,
        io::Write,
        os::unix::{ffi::OsStrExt, fs::OpenOptionsExt, fs::PermissionsExt},
        sync::mpsc::{RecvTimeoutError, channel},
    };

    let directory = tempfile::tempdir().unwrap();
    let entry = directory.path().join("worker");
    fs::write(&entry, [0xcf, 0xfa, 0xed, 0xfe]).unwrap();
    fs::set_permissions(&entry, fs::Permissions::from_mode(0o700)).unwrap();
    let manifest = directory.path().join("manifest.json");
    fs::write(
&manifest,
br#"{"id":"lab.fifo","version":"1","api_version":1,"engine":"native","entry":"worker","permissions":[],"contributions":["suite_panel"]}"#,
)
.unwrap();
    let package = VerifiedPackage::read(directory.path()).unwrap();
    fs::remove_file(&manifest).unwrap();
    let fifo = CString::new(manifest.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);

    let mut outcomes = Vec::new();
    for recheck in [false, true] {
        let package = package.clone();
        let (started, reader_started) = channel();
        let (done, result) = channel();
        let reader = std::thread::spawn(move || {
            let _ = started.send(());
            let checked = if recheck {
                package.unchanged()
            } else {
                VerifiedPackage::read(&package.directory).map(|_| ())
            };
            let _ = done.send(checked.map_err(|error| error.to_string()));
        });
        reader_started.recv().unwrap();
        let mut checked = result.recv_timeout(Duration::from_secs(1));
        let without_writer = checked.is_ok();
        if matches!(checked, Err(RecvTimeoutError::Timeout)) {
            // A writer only opens once the old blocking reader is present; close it to deliver EOF.
            loop {
                match fs::OpenOptions::new()
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&manifest)
                {
                    Ok(mut writer) => {
                        let _ = writer.write_all(b"{}");
                        drop(writer);
                        checked = result.recv_timeout(Duration::from_secs(1));
                        break;
                    }
                    Err(error) if error.raw_os_error() == Some(libc::ENXIO) => {
                        checked = result.recv_timeout(Duration::from_millis(1));
                        if !matches!(checked, Err(RecvTimeoutError::Timeout)) {
                            break;
                        }
                    }
                    Err(error) => panic!("cannot release the FIFO reader: {error}"),
                }
            }
        }
        reader.join().unwrap();
        outcomes.push((recheck, without_writer, checked));
    }
    for (recheck, without_writer, checked) in outcomes {
        assert!(
            without_writer,
            "manifest recheck={recheck} waited for a FIFO writer; cancellation cannot recover this read"
        );
        assert!(checked.unwrap().unwrap_err().contains("普通文件"));
    }
}

fn test_sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn build_test_tar_gz(entries: &[(&str, u8, u32, &[u8])]) -> Vec<u8> {
    use flate2::{Compression, write::GzEncoder};
    use std::io::Write;

    let mut tar = Vec::new();
    for (name, typeflag, mode, data) in entries {
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
        header[156] = *typeflag;
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

#[test]
fn single_file_tar_gz_and_mio_plugin_inspect_verify_and_cleanup() {
    let manifest_bytes = br#"{"id":"lab.archive","version":"0.2.0","api_version":1,"engine":"native","entry":"bin/worker","permissions":["traffic.read"],"contributions":["suite_panel"]}"#;
    let entry_bytes = [0xcf, 0xfa, 0xed, 0xfe, 0x01, 0x02];
    let sha_json = serde_json::json!({
        "id": "lab.archive",
        "version": "0.2.0",
        "engine": "native",
        "entry": "bin/worker",
        "manifest_sha256": test_sha256_hex(manifest_bytes),
        "entry_sha256": test_sha256_hex(&entry_bytes),
        "entry_bytes": entry_bytes.len(),
    })
    .to_string();
    let archive_bytes = build_test_tar_gz(&[
        ("manifest.json", b'0', 0o644, manifest_bytes),
        ("bin/worker", b'0', 0o755, &entry_bytes),
        ("sha256-manifest.json", b'0', 0o644, sha_json.as_bytes()),
    ]);

    let tmp = tempfile::tempdir().unwrap();
    for ext in ["tar.gz", "tgz", "mio-plugin"] {
        let archive_path = tmp.path().join(format!("plugin.{ext}"));
        fs::write(&archive_path, &archive_bytes).unwrap();
        let pkg = VerifiedPackage::read(&archive_path).unwrap();
        assert_eq!(pkg.manifest.id, "lab.archive");
        assert_eq!(pkg.directory, archive_path.canonicalize().unwrap());
        assert_ne!(pkg.work_dir(), pkg.directory.as_path());
        assert!(pkg.executable.starts_with(pkg.work_dir()));
        assert!(pkg.executable.is_file());
        pkg.unchanged().unwrap();
        assert!(pkg.poll_source_fingerprint().is_some());

        let unpacked_dir = pkg.work_dir().to_path_buf();
        assert!(unpacked_dir.exists());

        fs::write(&archive_path, b"mutated").unwrap();
        let err = pkg.unchanged().unwrap_err().to_string();
        assert!(err.contains("归档"), "unexpected error: {err}");

        drop(pkg);
        assert!(
            !unpacked_dir.exists(),
            "temporary unpacked directory must be cleaned up when VerifiedPackage drops"
        );
    }
}

#[test]
fn single_file_archive_rejects_tampered_sha256_traversal_symlinks_and_non_gzip() {
    let tmp = tempfile::tempdir().unwrap();
    let manifest_bytes = br#"{"id":"lab.bad","version":"1","api_version":1,"engine":"native","entry":"worker","permissions":[],"contributions":["suite_panel"]}"#;
    let entry_bytes = [0xcf, 0xfa, 0xed, 0xfe];

    let bad_sha = serde_json::json!({
        "id": "lab.bad",
        "version": "1",
        "engine": "native",
        "entry": "worker",
        "manifest_sha256": test_sha256_hex(manifest_bytes),
        "entry_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
        "entry_bytes": entry_bytes.len(),
    })
    .to_string();
    let bad_sha_path = tmp.path().join("bad-sha.tar.gz");
    fs::write(
        &bad_sha_path,
        build_test_tar_gz(&[
            ("manifest.json", b'0', 0o644, manifest_bytes),
            ("worker", b'0', 0o755, &entry_bytes),
            ("sha256-manifest.json", b'0', 0o644, bad_sha.as_bytes()),
        ]),
    )
    .unwrap();
    let err = VerifiedPackage::read(&bad_sha_path).unwrap_err().to_string();
    assert!(err.contains("sha256-manifest.json"), "unexpected: {err}");

    let trav_path = tmp.path().join("traversal.tar.gz");
    fs::write(
        &trav_path,
        build_test_tar_gz(&[
            ("../escape", b'0', 0o644, b"x"),
            ("manifest.json", b'0', 0o644, manifest_bytes),
            ("worker", b'0', 0o755, &entry_bytes),
        ]),
    )
    .unwrap();
    let err = VerifiedPackage::read(&trav_path).unwrap_err().to_string();
    assert!(err.contains("穿越"), "unexpected: {err}");

    let sym_path = tmp.path().join("symlink.tar.gz");
    fs::write(
        &sym_path,
        build_test_tar_gz(&[
            ("worker", b'2', 0o777, b""),
            ("manifest.json", b'0', 0o644, manifest_bytes),
        ]),
    )
    .unwrap();
    let err = VerifiedPackage::read(&sym_path).unwrap_err().to_string();
    assert!(err.contains("软链接"), "unexpected: {err}");

    let non_gz_path = tmp.path().join("raw-bin.mio-plugin");
    fs::write(&non_gz_path, b"MZ\x90\x00not-gzip").unwrap();
    let err = VerifiedPackage::read(&non_gz_path).unwrap_err().to_string();
    assert!(err.contains("gzip"), "unexpected: {err}");
}
