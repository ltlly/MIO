use std::fs::OpenOptions;

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
    assert_eq!(directory_names(directory.path()), vec!["synthetic.archive"]);
    assert_eq!(Identity::of(&fs::metadata(&path).unwrap()).unwrap(), before);
    let mut changed = bytes;
    changed.push(7);
    fs::write(&path, &changed).unwrap();
    assert!(inspection.check_source().is_err());
    inspection.release();
    assert_eq!(fs::read(path).unwrap(), changed);
}
