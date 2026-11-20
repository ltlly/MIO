use super::*;
#[cfg(unix)]
use std::{
    ffi::CString,
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    sync::mpsc::{RecvTimeoutError, channel},
    time::Duration,
};
use std::{fs, io::Write};

thread_local! {
    static BEFORE_OPEN_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static AFTER_METADATA_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

pub(super) fn before_payload_open() {
    if let Some(hook) = BEFORE_OPEN_HOOK.with_borrow_mut(Option::take) {
        hook();
    }
}

pub(super) fn after_payload_metadata() {
    if let Some(hook) = AFTER_METADATA_HOOK.with_borrow_mut(Option::take) {
        hook();
    }
}

struct Fixture {
    path: PathBuf,
}

impl Fixture {
    fn new(bytes: &[u8]) -> Self {
        let directory = std::env::temp_dir().join(format!("mio-payloads-{}", Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let fixture = Self {
            path: directory.join("payloads.txt"),
        };
        fs::write(&fixture.path, bytes).unwrap();
        fixture
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.path.parent().unwrap());
    }
}

#[cfg(unix)]
fn make_fifo(path: &std::path::Path) {
    let path = CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
}

#[test]
fn payload_file_keeps_binary_duplicates_delimiters_and_empty_values() {
    let bytes = b"a\r\n\r\n\xff\0\na\n";
    let fixture = Fixture::new(bytes);
    assert_eq!(
        read_payload_file(&fixture.path).unwrap(),
        vec![b"a".to_vec(), vec![], vec![255, 0], b"a".to_vec()]
    );
    assert_eq!(fs::read(&fixture.path).unwrap(), bytes);
    fs::write(&fixture.path, b"\n").unwrap();
    assert_eq!(
        read_payload_file(&fixture.path).unwrap(),
        vec![Vec::<u8>::new()]
    );
    fs::write(&fixture.path, b"").unwrap();
    assert!(read_payload_file(&fixture.path).unwrap().is_empty());
    fs::write(&fixture.path, b"a\r").unwrap();
    assert_eq!(
        read_payload_file(&fixture.path).unwrap(),
        vec![b"a\r".to_vec()]
    );
}

#[test]
fn payload_file_rejects_directories_and_missing_files() {
    let fixture = Fixture::new(b"x");
    assert_eq!(
        read_payload_file(fixture.path.parent().unwrap()).unwrap_err(),
        "请选择不超过 32 MiB 的普通 payload 文件；列表未改变。"
    );
    fs::remove_file(&fixture.path).unwrap();
    assert!(read_payload_file(&fixture.path).is_err());
}

#[cfg(unix)]
#[test]
fn payload_file_fifo_never_waits_for_a_writer() {
    for replace_before_open in [false, true] {
        let fixture = Fixture::new(b"x\n");
        if !replace_before_open {
            fs::remove_file(&fixture.path).unwrap();
            make_fifo(&fixture.path);
        }
        let path = fixture.path.clone();
        let (started, starting) = channel();
        let (opened, metadata_read) = channel();
        let (done, outcome) = channel();
        let reader = std::thread::spawn(move || {
            if replace_before_open {
                let replacement = path.clone();
                BEFORE_OPEN_HOOK.with_borrow_mut(|hook| {
                    *hook = Some(Box::new(move || {
                        fs::remove_file(&replacement).unwrap();
                        make_fifo(&replacement);
                    }));
                });
            }
            AFTER_METADATA_HOOK.with_borrow_mut(|hook| {
                *hook = Some(Box::new(move || {
                    let _ = opened.send(());
                }));
            });
            let _ = started.send(());
            let checked = read_payload_file(&path);
            let _ = done.send(checked);
        });
        let started = starting.recv_timeout(Duration::from_secs(1));
        let mut checked = outcome.recv_timeout(Duration::from_secs(1));
        let without_writer = checked.is_ok();
        let mut released = true;
        if matches!(checked, Err(RecvTimeoutError::Timeout)) {
            // Keep the rescue writer open until the reader owns its fd, then deliver EOF.
            let writer = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&fixture.path);
            released = writer.is_ok() && metadata_read.recv_timeout(Duration::from_secs(1)).is_ok();
            drop(writer);
            checked = outcome.recv_timeout(Duration::from_secs(1));
        }
        reader.join().unwrap();
        assert!(started.is_ok());
        assert!(
            released,
            "the FIFO reader must be released before asserting"
        );
        assert!(
            without_writer,
            "payload replacement={replace_before_open} waited for a FIFO writer"
        );
        assert_eq!(
            checked.unwrap().unwrap_err(),
            "请选择不超过 32 MiB 的普通 payload 文件；列表未改变。"
        );
    }
}

#[test]
fn payload_file_rechecks_grown_length_on_the_opened_descriptor() {
    let fixture = Fixture::new(b"x");
    let growing = fixture.path.clone();
    BEFORE_OPEN_HOOK.with_borrow_mut(|hook| {
        *hook = Some(Box::new(move || {
            fs::OpenOptions::new()
                .write(true)
                .open(growing)
                .unwrap()
                .set_len(MAX_PAYLOAD_BYTES as u64 + 1)
                .unwrap();
        }));
    });
    let shrinking = fixture.path.clone();
    AFTER_METADATA_HOOK.with_borrow_mut(|hook| {
        // A later truncation cannot excuse accepting the oversized fd metadata.
        *hook = Some(Box::new(move || {
            fs::OpenOptions::new()
                .write(true)
                .open(shrinking)
                .unwrap()
                .set_len(1)
                .unwrap();
        }));
    });
    assert!(
        read_payload_file(&fixture.path)
            .unwrap_err()
            .contains("32 MiB")
    );
}

#[test]
fn payload_file_rejects_initially_oversized_sparse_files() {
    let fixture = Fixture::new(b"x");
    fs::OpenOptions::new()
        .write(true)
        .open(&fixture.path)
        .unwrap()
        .set_len(MAX_PAYLOAD_BYTES as u64 + 1)
        .unwrap();
    assert!(
        read_payload_file(&fixture.path)
            .unwrap_err()
            .contains("32 MiB")
    );
}

#[test]
fn payload_file_enforces_byte_limit_when_the_open_file_grows() {
    for length in [MAX_PAYLOAD_BYTES, MAX_PAYLOAD_BYTES + 1] {
        let fixture = Fixture::new(b"\xff");
        let growing = fixture.path.clone();
        AFTER_METADATA_HOOK.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move || {
                // Sparse growth exercises the real limit without allocating a fixture buffer.
                fs::OpenOptions::new()
                    .write(true)
                    .open(growing)
                    .unwrap()
                    .set_len(length as u64)
                    .unwrap();
            }));
        });
        let result = read_payload_file(&fixture.path);
        if length == MAX_PAYLOAD_BYTES {
            let values = result.unwrap();
            assert_eq!(values.len(), 1);
            assert_eq!(values[0].len(), MAX_PAYLOAD_BYTES);
            assert_eq!(values[0][0], 255);
            assert_eq!(values[0][MAX_PAYLOAD_BYTES - 1], 0);
        } else {
            assert!(result.unwrap_err().contains("未截断或加载任何条目"));
        }
    }
}

#[cfg(unix)]
#[test]
fn payload_file_reads_the_checked_descriptor_after_path_replacement() {
    let fixture = Fixture::new(b"original\n\0\xff");
    let replacement = fixture.path.clone();
    AFTER_METADATA_HOOK.with_borrow_mut(|hook| {
        *hook = Some(Box::new(move || {
            fs::rename(&replacement, replacement.with_extension("opened")).unwrap();
            fs::write(replacement, b"replacement").unwrap();
        }));
    });
    assert_eq!(
        read_payload_file(&fixture.path).unwrap(),
        vec![b"original".to_vec(), vec![0, 255]]
    );
    assert_eq!(fs::read(&fixture.path).unwrap(), b"replacement");
}

#[test]
fn payload_file_counts_empty_lines_without_truncating_at_the_item_limit() {
    let fixture = Fixture::new(&vec![b'\n'; MAX_PAYLOADS]);
    let values = read_payload_file(&fixture.path).unwrap();
    assert_eq!(values.len(), MAX_PAYLOADS);
    assert!(values.iter().all(Vec::is_empty));
    fs::OpenOptions::new()
        .append(true)
        .open(&fixture.path)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    assert!(
        read_payload_file(&fixture.path)
            .unwrap_err()
            .contains("100000")
    );
}

#[test]
fn list_delimiters_keep_binary_duplicates_and_explicit_empty_values() {
    assert_eq!(
        payload_lines(b"a\r\n\r\n\xff\0\na\n").unwrap(),
        vec![b"a".to_vec(), vec![], vec![255, 0], b"a".to_vec()]
    );
    assert_eq!(payload_lines(b"\n").unwrap(), vec![Vec::<u8>::new()]);
    assert!(payload_lines(b"").unwrap().is_empty());
    assert_eq!(payload_lines(b"a\r").unwrap(), vec![b"a\r".to_vec()]);
}

#[test]
fn all_position_append_checks_aggregate_limits_before_any_replication() {
    let mut spec = super::super::tests::attack().spec;
    spec.attack_type = AttackType::Pitchfork;
    spec.positions.resize(
        MAX_PAYLOAD_SETS,
        mio_core::intruder::Position { start: 0, end: 0 },
    );
    spec.payload_sets = vec![Vec::new(); MAX_PAYLOAD_SETS];
    assert_eq!(
        payload_append_capacity(&spec, None, 1, MAX_PAYLOAD_BYTES / MAX_PAYLOAD_SETS),
        Ok(MAX_PAYLOAD_SETS)
    );
    assert!(
        payload_append_capacity(&spec, None, 1, MAX_PAYLOAD_BYTES / MAX_PAYLOAD_SETS + 1).is_err()
    );
    assert_eq!(
        payload_append_capacity(&spec, Some(19), 1, MAX_PAYLOAD_BYTES),
        Ok(MAX_PAYLOAD_SETS)
    );
    assert!(payload_append_capacity(&spec, Some(20), 1, 0).is_err());
    assert_eq!(
        payload_append_capacity(&spec, None, MAX_PAYLOADS / MAX_PAYLOAD_SETS, 0),
        Ok(MAX_PAYLOAD_SETS)
    );
    assert!(payload_append_capacity(&spec, None, MAX_PAYLOADS / MAX_PAYLOAD_SETS + 1, 0).is_err());
    assert!(payload_append_capacity(&spec, None, usize::MAX, 0).is_err());
    assert!(payload_append_capacity(&spec, None, 1, usize::MAX).is_err());
    spec.positions
        .resize(256, mio_core::intruder::Position { start: 0, end: 0 });
    assert!(
        payload_append_capacity(&spec, None, 1, 0)
            .unwrap_err()
            .contains("20 个")
    );
    assert!(spec.payload_sets.iter().all(Vec::is_empty));
}

#[test]
fn append_budget_includes_existing_values_and_retained_extra_sets() {
    let mut spec = super::super::tests::attack().spec;
    spec.payload_sets.push(vec![vec![0, 255]]);
    assert_eq!(payload_append_capacity(&spec, None, 1, 0), Ok(2));
    assert!(payload_append_capacity(&spec, Some(0), 1, MAX_PAYLOAD_BYTES - 4).is_err());
    assert_eq!(
        payload_append_capacity(&spec, Some(0), 1, MAX_PAYLOAD_BYTES - 5),
        Ok(2)
    );
    assert!(!payload_sets_identical(&spec));
    spec.payload_sets[1] = spec.payload_sets[0].clone();
    assert!(payload_sets_identical(&spec));
    spec.payload_sets[1][1].reverse();
    assert!(!payload_sets_identical(&spec));
}
