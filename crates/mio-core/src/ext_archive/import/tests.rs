use super::*;
use crate::{ExtImportPhase, ExtJobOutcome, store::ProjectStore};

#[path = "tests/cleanup.rs"]
mod cleanup;
#[path = "tests/conversion.rs"]
mod conversion;
#[cfg(unix)]
#[path = "tests/sidecars.rs"]
mod sidecars;
#[path = "tests/snapshots.rs"]
mod snapshots;

pub(crate) fn fixture(directory: &Path, length: usize) -> (PathBuf, Vec<u8>) {
    let mut bytes = vec![0; length.max(72)];
    bytes[..4].copy_from_slice(&[0x66, 0x85, 0x82, 0x80]);
    bytes[4..8].copy_from_slice(&1_i32.to_be_bytes());
    bytes[8..12].copy_from_slice(&0x8052_7974_u32.to_be_bytes());
    bytes[12..14].copy_from_slice(&228_u16.to_be_bytes());
    bytes[14..16].copy_from_slice(&228_u16.to_be_bytes());
    bytes[48..56].copy_from_slice(&i64::MAX.to_be_bytes());
    let length = bytes.len() as i64;
    bytes[56..64].copy_from_slice(&length.to_be_bytes());
    for (index, byte) in bytes[72..].iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }
    let path = directory.join("synthetic.archive");
    fs::write(&path, &bytes).unwrap();
    (path, bytes)
}

fn inspected(path: PathBuf) -> Inspection {
    inspect(path, &CancellationToken::new(), &mut |_, _, _| {}).unwrap()
}

pub(crate) fn inspected_with_snapshot(path: PathBuf) -> Inspection {
    let mut inspection = inspected(path.clone());
    inspection.snapshot =
        Some(PrivateDirectory::new(parent(&path), ".mio-ext-source-").unwrap());
    inspection
}

fn directory_names(path: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn inspect_does_not_write_to_source_directory_and_detects_later_source_writes() {
    let directory = tempfile::tempdir().unwrap();
    let (path, bytes) = fixture(directory.path(), 1024);
    let before = Identity::of(&fs::metadata(&path).unwrap()).unwrap();
    let inspection = inspected(path.clone());
    assert!(inspection.snapshot.is_none());
    assert_eq!(directory_names(directory.path()), vec!["synthetic.archive"]);
    assert_eq!(Identity::of(&fs::metadata(&path).unwrap()).unwrap(), before);
    let mut changed = bytes;
    changed.push(7);
    fs::write(&path, &changed).unwrap();
    assert!(inspection.check_source().is_err());
    inspection.release().unwrap();
    assert_eq!(fs::read(path).unwrap(), changed);
}

#[cfg(unix)]
#[test]
fn release_refuses_to_delete_a_replacement_directory() {
    let directory = tempfile::tempdir().unwrap();
    let (path, _) = fixture(directory.path(), 72);
    let inspection = inspected_with_snapshot(path);
    let snapshot = inspection.snapshot.as_ref().unwrap().path().to_owned();
    let held = directory.path().join("held-snapshot");
    fs::rename(&snapshot, &held).unwrap();
    fs::create_dir(&snapshot).unwrap();
    fs::write(snapshot.join("other-data"), b"keep this replacement").unwrap();

    let result = inspection.release();
    assert!(result.is_err(), "cleanup must reject a replaced directory");
    assert_eq!(
        fs::read(snapshot.join("other-data")).unwrap(),
        b"keep this replacement"
    );
    assert!(held.is_dir());
    let mut cleanup = result.unwrap_err().downcast::<CleanupFailure>().unwrap();
    assert!(!cleanup.is_empty());
    assert!(cleanup.retry().is_err());
    let replacement = directory.path().join("kept-replacement");
    fs::rename(&snapshot, &replacement).unwrap();
    fs::rename(&held, &snapshot).unwrap();
    cleanup.retry().unwrap();
    assert!(cleanup.is_empty());
    assert!(!snapshot.exists());
    assert_eq!(
        fs::read(replacement.join("other-data")).unwrap(),
        b"keep this replacement"
    );
}

#[cfg(unix)]
#[test]
fn release_does_not_report_success_when_owned_directory_was_only_renamed() {
    let directory = tempfile::tempdir().unwrap();
    let (path, _) = fixture(directory.path(), 72);
    let inspection = inspected_with_snapshot(path);
    let snapshot = inspection.snapshot.as_ref().unwrap().path().to_owned();
    let held = directory.path().join("held-snapshot");
    fs::rename(&snapshot, &held).unwrap();
    let result = inspection.release();
    assert!(
        result.is_err(),
        "an absent path does not prove its owned inode was removed"
    );
    assert!(held.is_dir());
    let mut cleanup = result.unwrap_err().downcast::<CleanupFailure>().unwrap();
    assert!(cleanup.retry().is_err());
    fs::rename(&held, &snapshot).unwrap();
    cleanup.retry().unwrap();
    assert!(cleanup.is_empty());
    assert!(!snapshot.exists());
}

#[cfg(unix)]
#[test]
fn an_already_removed_owned_directory_does_not_delete_a_new_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let (path, bytes) = fixture(directory.path(), 72);
    let inspection = inspected_with_snapshot(path.clone());
    let snapshot = inspection.snapshot.as_ref().unwrap().path().to_owned();
    fs::remove_dir_all(&snapshot).unwrap();
    let identity = inspection
        .snapshot
        .as_ref()
        .unwrap()
        .directory
        .as_ref()
        .unwrap();
    assert!(identity.removed().unwrap());
    assert!(
        identity.clone().removed().unwrap(),
        "the deletion event remains latched"
    );
    fs::create_dir(&snapshot).unwrap();
    fs::write(snapshot.join("other-data"), b"keep this replacement").unwrap();
    inspection
        .release()
        .expect("the owned directory really was removed");
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert_eq!(
        fs::read(snapshot.join("other-data")).unwrap(),
        b"keep this replacement"
    );
}
