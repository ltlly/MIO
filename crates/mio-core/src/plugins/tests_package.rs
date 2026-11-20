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
