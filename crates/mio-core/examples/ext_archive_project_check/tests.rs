use super::*;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{PermissionsExt, symlink};
use tokio::sync::mpsc;

const PRIVATE_MARKER: &str = "synthetic-never-log-this";

// A tiny schema-228 fixture, not a claim about any real External product version.
fn synthetic_source(count: usize, empty_prefix: usize, message: &[u8]) -> Vec<u8> {
    assert!(empty_prefix <= count);
    fn record(bytes: &mut Vec<u8>, value: &[u8]) -> i64 {
        if !bytes.len().is_multiple_of(2) {
            bytes.push(0);
        }
        let address = bytes.len() as i64;
        bytes.extend_from_slice(value);
        address
    }

    fn array(bytes: &mut Vec<u8>, width: usize, payload: &[u8]) -> i64 {
        let mut value = Vec::new();
        value.extend_from_slice(&((8 + payload.len().max(1)) as i32).to_be_bytes());
        value.extend_from_slice(&((payload.len() / width) as i32).to_be_bytes());
        value.extend_from_slice(payload);
        if payload.is_empty() {
            value.push(0);
        }
        record(bytes, &value)
    }

    fn object(bytes: &mut Vec<u8>, subtype: u8, fields: Vec<(u8, Vec<u8>)>) -> i64 {
        let mut value = vec![0, 0, subtype, fields.len() as u8];
        let mut offset = 4 + 3 * fields.len();
        for (id, field) in &fields {
            value.push(*id);
            value.extend_from_slice(&(offset as i16).to_be_bytes());
            offset += field.len();
        }
        for (_, field) in fields {
            value.extend(field);
        }
        record(bytes, &value)
    }

    let mut bytes = vec![0; 72];
    bytes[..4].copy_from_slice(&[0x66, 0x85, 0x82, 0x80]);
    bytes[4..8].copy_from_slice(&1_i32.to_be_bytes());
    bytes[8..12].copy_from_slice(&0x8052_7974_u32.to_be_bytes());
    bytes[12..14].copy_from_slice(&228_u16.to_be_bytes());
    bytes[14..16].copy_from_slice(&228_u16.to_be_bytes());
    bytes[48..56].copy_from_slice(&i64::MAX.to_be_bytes());
    let message = array(&mut bytes, 1, message);
    let tab = object(&mut bytes, 2, vec![(4, message.to_be_bytes().to_vec())]);
    let mut references = Vec::with_capacity(count * 8);
    if empty_prefix > 0 {
        let empty = array(&mut bytes, 1, &[]);
        let empty_tab = object(&mut bytes, 2, vec![(4, empty.to_be_bytes().to_vec())]);
        references.extend_from_slice(&empty_tab.to_be_bytes().repeat(empty_prefix));
    }
    references.extend_from_slice(&tab.to_be_bytes().repeat(count - empty_prefix));
    let references = array(&mut bytes, 8, &references);
    let tabs = object(
        &mut bytes,
        0,
        vec![
            (0, (count as i32).to_be_bytes().to_vec()),
            (1, references.to_be_bytes().to_vec()),
        ],
    );
    let repeater = object(&mut bytes, 2, vec![(1, tabs.to_be_bytes().to_vec())]);
    let root = object(&mut bytes, 7, vec![(2, repeater.to_be_bytes().to_vec())]);
    let length = bytes.len() as i64;
    bytes[56..64].copy_from_slice(&length.to_be_bytes());
    bytes[64..72].copy_from_slice(&root.to_be_bytes());
    bytes
}

fn paths(directory: &Path) -> Paths {
    Paths {
        source: directory.join("source.archive"),
        converted: directory.join("converted.mio"),
        saved: directory.join("saved.mio"),
    }
}

fn parse(paths: &Paths) -> Result<Paths> {
    arguments(
        [&paths.source, &paths.converted, &paths.saved]
            .map(|path| path.as_os_str().to_owned())
            .into_iter(),
    )
}

#[test]
fn real_command_chain_pages_samples_saves_and_reopens_without_logging_traffic() {
    let directory = tempfile::tempdir().unwrap();
    let paths = paths(directory.path());
    let mut message = format!(
        "POST /{PRIVATE_MARKER} HTTP/1.1\r\nHost: fixture.invalid\r\nCookie: {PRIVATE_MARKER}\r\nContent-Length: {}\r\n\r\n",
        8195,
    ).into_bytes();
    message.extend(vec![b'X'; 8192]);
    message.extend_from_slice(&[0, 255, 90]);
    let original = synthetic_source(PAGE + 1, 0, &message);
    fs::write(&paths.source, &original).unwrap();
    fs::set_permissions(&paths.source, fs::Permissions::from_mode(0o400)).unwrap();
    let paths = parse(&paths).unwrap();
    let mut output = Vec::new();
    check(&paths, &mut output, Duration::from_secs(10)).unwrap();
    let output = String::from_utf8(output).unwrap();
    assert_eq!(fs::read(&paths.source).unwrap(), original);
    assert_eq!(
        fs::metadata(&paths.source).unwrap().permissions().mode() & 0o777,
        0o400
    );
    assert!(paths.converted.is_file());
    assert!(paths.saved.is_file());
    for stage in [
        "inspect",
        "convert",
        "open",
        "report_pages_samples",
        "save",
        "save_as",
        "new_project",
        "reopen",
        "recheck",
        "shutdown_join",
    ] {
        assert!(
            output.contains(&format!("phase={stage} wall_ms=")),
            "missing {stage}"
        );
    }
    assert!(output.contains("indexed_items=201 messages=201 proxy_flows=0 repeater_tabs=201"));
    assert!(output.contains("runtime_join=confirmed"));
    assert!(output.contains("source_metadata=unchanged"));
    assert!(output.contains("budget_exceeded=false"));
    assert_eq!(output.matches("file_present=true").count(), 2);
    assert!(output.contains("verification=passed"));
    assert!(output.contains("performance_parity=unverified"));
    assert!(!output.contains(PRIVATE_MARKER));
    assert!(!output.contains("fixture.invalid"));
    assert!(!output.contains("Cookie:"));
}

#[test]
fn empty_leading_messages_do_not_consume_the_nonempty_raw_sample_budget() {
    let directory = tempfile::tempdir().unwrap();
    let paths = paths(directory.path());
    let mut message = format!("GET /{PRIVATE_MARKER} HTTP/1.1\r\n\r\n").into_bytes();
    message.extend_from_slice(&[0, 255, 90]);
    let original = synthetic_source(9, 8, &message);
    assert!(
        original.len() < 1024,
        "the regression fixture must stay tiny"
    );
    fs::write(&paths.source, &original).unwrap();
    fs::set_permissions(&paths.source, fs::Permissions::from_mode(0o400)).unwrap();
    let paths = parse(&paths).unwrap();
    let mut output = Vec::new();
    check(&paths, &mut output, Duration::from_secs(10)).unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("indexed_items=9 messages=1 proxy_flows=0 repeater_tabs=9"));
    assert!(output.contains("verification=passed"));
    assert!(output.contains("source_metadata=unchanged"));
    assert!(!output.contains(PRIVATE_MARKER));
    assert_eq!(fs::read(&paths.source).unwrap(), original);
}

#[test]
fn invalid_source_still_joins_and_reports_absent_outputs() {
    let directory = tempfile::tempdir().unwrap();
    let paths = paths(directory.path());
    fs::write(&paths.source, PRIVATE_MARKER).unwrap();
    let paths = parse(&paths).unwrap();
    let mut output = Vec::new();
    let error = check(&paths, &mut output, Duration::from_secs(10)).unwrap_err();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("runtime_join=confirmed"));
    assert_eq!(output.matches("file_present=false").count(), 2);
    assert!(!output.contains("verification=passed"));
    assert!(!output.contains(PRIVATE_MARKER));
    assert!(!format!("{error:#}").contains(PRIVATE_MARKER));
    assert_eq!(fs::read(&paths.source).unwrap(), PRIVATE_MARKER.as_bytes());
    assert!(!paths.converted.exists());
    assert!(!paths.saved.exists());
}

#[test]
fn arguments_reject_missing_extra_alias_and_non_file_sources() {
    for count in [0, 1, 2, 4] {
        assert!(arguments((0..count).map(|_| OsString::from("unused"))).is_err());
    }
    let directory = tempfile::tempdir().unwrap();
    let mut paths = paths(directory.path());
    assert!(parse(&paths).is_err());
    fs::create_dir(&paths.source).unwrap();
    assert!(parse(&paths).is_err());
    paths.source = directory.path().join("regular.archive");
    fs::write(&paths.source, PRIVATE_MARKER).unwrap();
    let accepted = parse(&paths).unwrap();
    assert!(accepted.source.is_absolute());
    assert!(!accepted.converted.exists());
    assert!(!accepted.saved.exists());
    let linked_source = directory.path().join("linked-source.archive");
    symlink(&paths.source, &linked_source).unwrap();
    paths.source = linked_source;
    assert!(parse(&paths).is_err());
    assert!(Source::open(&paths.source).is_err());
    paths.source = accepted.source;
    paths.saved = paths.converted.clone();
    assert!(parse(&paths).is_err());
    let alias = directory.path().join("alias");
    symlink(directory.path(), &alias).unwrap();
    paths.saved = alias.join("converted.mio");
    assert!(parse(&paths).is_err());
    paths.saved = directory.path().join("saved.archive");
    assert!(parse(&paths).is_err());
    paths.saved = directory.path().join("missing-parent/saved.mio");
    assert!(parse(&paths).is_err());
}

#[test]
fn arguments_reject_existing_targets_sidecars_and_dangling_links_without_modifying_them() {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let directory = tempfile::tempdir().unwrap();
        let paths = paths(directory.path());
        fs::write(&paths.source, PRIVATE_MARKER).unwrap();
        let existing = directory.path().join(format!("converted.mio{suffix}"));
        fs::write(&existing, PRIVATE_MARKER).unwrap();
        assert!(parse(&paths).is_err());
        assert_eq!(fs::read(&existing).unwrap(), PRIVATE_MARKER.as_bytes());
        assert_eq!(fs::read(&paths.source).unwrap(), PRIVATE_MARKER.as_bytes());
        assert!(!paths.saved.exists());
    }
    let directory = tempfile::tempdir().unwrap();
    let paths = paths(directory.path());
    fs::write(&paths.source, PRIVATE_MARKER).unwrap();
    let missing = directory.path().join("unrelated-missing");
    symlink(&missing, &paths.converted).unwrap();
    assert!(parse(&paths).is_err());
    assert_eq!(fs::read_link(&paths.converted).unwrap(), missing);
    assert!(!missing.exists());
}

#[test]
fn source_descriptor_is_nonblocking_and_detects_modification_and_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.archive");
    fs::write(&path, b"synthetic").unwrap();
    let source = Source::open(&path).unwrap();
    // Query the live descriptor flags instead of risking a hanging FIFO regression test.
    let flags = unsafe { libc::fcntl(source.file.as_raw_fd(), libc::F_GETFL) };
    assert_ne!(flags, -1);
    assert_ne!(flags & libc::O_NONBLOCK, 0);
    source.unchanged(&path).unwrap();
    fs::write(&path, b"synthetic modified").unwrap();
    assert!(source.unchanged(&path).is_err());
    let source = Source::open(&path).unwrap();
    fs::rename(&path, directory.path().join("retained-original.archive")).unwrap();
    fs::write(&path, b"synthetic modified").unwrap();
    assert!(source.unchanged(&path).is_err());
}

#[tokio::test]
async fn failed_publication_reports_but_never_deletes_the_acknowledged_created_path() {
    let directory = tempfile::tempdir().unwrap();
    let published = directory.path().join("published.mio");
    fs::write(&published, PRIVATE_MARKER).unwrap();
    let (commands, _receiver) = mpsc::channel(1);
    let (events, event_receiver) = mpsc::channel(1);
    let mut client = Client {
        commands,
        events: event_receiver,
        shutdown: CancellationToken::new(),
        ipc_socket_path: None,
    };
    let project = ProjectKey {
        id: Uuid::new_v4(),
        generation: 1,
    };
    let job_id = Uuid::new_v4();
    events
        .send(Event::ExtJob {
            project,
            job_id,
            outcome: ExtJobOutcome::Failed {
                message: PRIVATE_MARKER.into(),
                created_path: Some(published.clone()),
            },
        })
        .await
        .unwrap();
    let mut output = Vec::new();
    let error = job(&mut client, project, job_id, &mut output)
        .await
        .unwrap_err();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("publication=created_but_failed"));
    assert!(output.contains(published.to_str().unwrap()));
    assert!(!output.contains(PRIVATE_MARKER));
    assert!(!format!("{error:#}").contains(PRIVATE_MARKER));
    assert_eq!(fs::read(&published).unwrap(), PRIVATE_MARKER.as_bytes());
}

#[test]
fn disk_inventory_includes_wal_shm_and_journal_and_does_not_claim_absent_files_exist() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("inventory.mio");
    let mut output = Vec::new();
    assert!(!disk_usage(&mut output, "absent", &path).unwrap());
    for (suffix, size) in [("", 3), ("-wal", 5), ("-shm", 7), ("-journal", 11)] {
        fs::write(
            directory.path().join(format!("inventory.mio{suffix}")),
            vec![0; size],
        )
        .unwrap();
    }
    assert!(disk_usage(&mut output, "present", &path).unwrap());
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains(
        "file_present=false file_bytes=0 wal_bytes=0 shm_bytes=0 journal_bytes=0 total_bytes=0"
    ));
    assert!(output.contains(
        "file_present=true file_bytes=3 wal_bytes=5 shm_bytes=7 journal_bytes=11 total_bytes=26"
    ));
}
