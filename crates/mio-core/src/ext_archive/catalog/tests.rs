use super::*;
use std::{fs, path::PathBuf};

mod collections;

fn object(bytes: &mut [u8], at: usize, subtype: u8, fields: &[(u8, Vec<u8>)]) {
    bytes[at..at + 4].copy_from_slice(&[0, 0, subtype, fields.len() as u8]);
    let mut offset = 4 + fields.len() * 3;
    for (index, (id, value)) in fields.iter().enumerate() {
        let descriptor = at + 4 + index * 3;
        bytes[descriptor] = *id;
        bytes[descriptor + 1..descriptor + 3].copy_from_slice(&(offset as i16).to_be_bytes());
        bytes[at + offset..at + offset + value.len()].copy_from_slice(value);
        offset += value.len();
    }
}

fn int(value: i32) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}

fn reference(value: i64) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}

fn array(bytes: &mut [u8], at: usize, values: &[i64]) {
    bytes[at..at + 4].copy_from_slice(&((8 + (values.len() * 8).max(1)) as i32).to_be_bytes());
    bytes[at + 4..at + 8].copy_from_slice(&(values.len() as i32).to_be_bytes());
    for (index, value) in values.iter().enumerate() {
        let start = at + 8 + index * 8;
        bytes[start..start + 8].copy_from_slice(&value.to_be_bytes());
    }
}

fn fixture_bytes() -> Vec<u8> {
    let mut bytes = vec![0; 8192];
    bytes[..4].copy_from_slice(&[0x66, 0x85, 0x82, 0x80]);
    bytes[4..8].copy_from_slice(&1_i32.to_be_bytes());
    bytes[8..12].copy_from_slice(&0x8052_7974_u32.to_be_bytes());
    bytes[12..14].copy_from_slice(&228_u16.to_be_bytes());
    bytes[14..16].copy_from_slice(&228_u16.to_be_bytes());
    bytes[48..56].copy_from_slice(&i64::MAX.to_be_bytes());
    bytes[56..64].copy_from_slice(&8192_i64.to_be_bytes());
    bytes[64..72].copy_from_slice(&72_i64.to_be_bytes());
    object(
        &mut bytes,
        72,
        7,
        &[
            (99, reference(-1)),
            (2, reference(384)),
            (1, reference(256)),
        ],
    );
    object(
        &mut bytes,
        256,
        0,
        &[
            (2, reference(768)),
            (0, reference(512)),
            (90, reference(-1)),
            (3, reference(896)),
            (1, reference(640)),
        ],
    );
    object(
        &mut bytes,
        384,
        2,
        &[
            (1, reference(3072)),
            (99, reference(-1)),
            (4, reference(3200)),
        ],
    );
    for index in 0..4 {
        let collection = 512 + index * 128;
        let direct = 1024 + index * 128;
        let backing = 1536 + index * 128;
        let chunk = 2048 + index * 256;
        object(
            &mut bytes,
            collection,
            0,
            &[
                (0, int(3)),
                (1, int(2)),
                (2, reference(direct as i64)),
                (3, int(1)),
            ],
        );
        object(
            &mut bytes,
            direct,
            0,
            &[(0, int(2)), (1, reference(backing as i64))],
        );
        array(
            &mut bytes,
            backing,
            &[chunk as i64, (chunk + 128) as i64, -1],
        );
        array(&mut bytes, chunk, &[-1, 4096]);
        array(&mut bytes, chunk + 128, &[0, 4096]);
    }
    object(&mut bytes, 3072, 0, &[(0, int(2)), (1, reference(3328))]);
    object(&mut bytes, 3200, 0, &[(0, int(0)), (1, reference(3456))]);
    array(&mut bytes, 3328, &[4096, 0, -1, 8192]);
    array(&mut bytes, 3456, &[]);
    // Item targets are deliberately not readable compact objects or array headers.
    bytes[4096..4128].fill(0xff);
    bytes
}

fn fixture(bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("synthetic.archive");
    fs::write(&path, bytes).unwrap();
    (directory, path)
}

fn value<T>(observation: &Observation<T>) -> &T {
    match observation {
        Observation::Value(value) => value,
        _ => panic!("expected a metadata value"),
    }
}

#[test]
fn schema_229_active_tab_reads_only_the_verified_f0_integer_without_following_tabs() {
    for selected in [0, 1, -1, i32::MIN, i32::MAX] {
        let mut bytes = fixture_bytes();
        bytes[12..14].copy_from_slice(&229_u16.to_be_bytes());
        bytes[14..16].copy_from_slice(&229_u16.to_be_bytes());
        object(
            &mut bytes,
            384,
            2,
            &[(0, int(selected)), (1, reference(3072)), (2, int(77))],
        );
        let (_directory, path) = fixture(&bytes);
        let catalog = inspect(&path, MAX_BYTE_BUDGET).unwrap();
        assert_eq!(catalog.repeater_active_tab, Observation::Value(selected));
        assert_eq!(catalog.collections.len(), 5);
        assert_eq!(value(&catalog.collections[0].state).logical_len, 2);
        assert_eq!(
            value(&value(&catalog.collections[0].state).sample),
            &[Slot::Present, Slot::Null]
        );
        assert!(catalog.bytes_read < 1024);
        if selected == 0 {
            let mut unknown_field = bytes.clone();
            unknown_field[384 + 4] = 99;
            let (_directory, path) = fixture(&unknown_field);
            let baseline = inspect(&path, MAX_BYTE_BUDGET).unwrap();
            assert_eq!(baseline.repeater_active_tab, Observation::Missing);
            assert_eq!(
                catalog.bytes_read,
                baseline.bytes_read + 4,
                "the verified scalar adds exactly one i32 read"
            );
        }
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn schema_229_active_tab_preserves_missing_malformed_unsupported_and_budget_states() {
    for case in [
        "missing",
        "short",
        "reference-width",
        "tag",
        "schema228",
        "budget",
    ] {
        let mut bytes = fixture_bytes();
        if case != "schema228" {
            bytes[12..14].copy_from_slice(&229_u16.to_be_bytes());
            bytes[14..16].copy_from_slice(&229_u16.to_be_bytes());
        }
        let mut fields = vec![(1, reference(3072)), (2, int(1))];
        if case != "missing" {
            fields.insert(
                0,
                (
                    0,
                    match case {
                        "short" => vec![0],
                        "reference-width" => reference(0),
                        _ => int(1),
                    },
                ),
            );
        }
        object(&mut bytes, 384, if case == "tag" { 7 } else { 2 }, &fields);
        let (_directory, path) = fixture(&bytes);
        let budget = if case == "budget" {
            HEADER_READ_BYTES
        } else {
            MAX_BYTE_BUDGET
        };
        let catalog = inspect(&path, budget).unwrap();
        match case {
            "missing" => assert_eq!(catalog.repeater_active_tab, Observation::Missing),
            "short" | "reference-width" => {
                assert!(
                    matches!(&catalog.repeater_active_tab, Observation::Failed(error) if error.kind == io::ErrorKind::InvalidData),
                    "{case}"
                );
                assert!(catalog.has_errors());
            }
            _ => assert!(
                matches!(catalog.repeater_active_tab, Observation::Unavailable { .. }),
                "{case}"
            ),
        }
        assert!(catalog.bytes_read <= budget);
        if case == "budget" {
            assert_eq!(catalog.bytes_read, HEADER_READ_BYTES);
        }
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

fn root_f1_229_bytes(length: i32) -> Vec<u8> {
    let mut bytes = fixture_bytes();
    bytes[12..14].copy_from_slice(&229_u16.to_be_bytes());
    bytes[14..16].copy_from_slice(&229_u16.to_be_bytes());
    for index in 0..4 {
        let logical = if index < 2 { length } else { 0 };
        let direct = 1024 + index * 128;
        let backing = 1536 + index * 128;
        object(
            &mut bytes,
            512 + index * 128,
            0,
            &[
                (0, int(logical)),
                (1, int(200)),
                (2, reference(direct as i64)),
                (3, int(0)),
            ],
        );
        object(
            &mut bytes,
            direct,
            0,
            &[
                (0, int(i32::from(logical > 0))),
                (1, reference(backing as i64)),
            ],
        );
        array(&mut bytes, backing, &[4096, -1]);
    }
    let mut slots = vec![-1; 200];
    slots[..3].copy_from_slice(&[7000, 7001, 7000]);
    array(&mut bytes, 4096, &slots);
    bytes[7000..7040].fill(0xff);
    bytes
}

#[test]
fn schema_229_root_f1_catalogs_empty_added_and_deleted_history_slots_without_items() {
    for length in [0, 3, 2] {
        let bytes = root_f1_229_bytes(length);
        let (_directory, path) = fixture(&bytes);
        let catalog = inspect(&path, MAX_BYTE_BUDGET).unwrap();
        let entries: Vec<_> = catalog
            .collections
            .iter()
            .filter(|entry| entry.path.starts_with("root.f1."))
            .collect();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            ["root.f1.f0", "root.f1.f1", "root.f1.f2", "root.f1.f3"]
        );
        for (index, entry) in entries.iter().enumerate() {
            let directory = value(&entry.state);
            let expected = if index < 2 { length as u64 } else { 0 };
            assert_eq!(directory.logical_len, expected);
            assert!(matches!(
                directory.layout,
                Layout::Chunked {
                    chunk_width: 200,
                    leading_offset: 0,
                    ..
                }
            ));
            assert_eq!(
                value(&directory.sample),
                &vec![Slot::Present; expected as usize]
            );
            let mut source = Container::open(&path).unwrap();
            assert_eq!(
                directory.source.page(&mut source, 0, 8).unwrap(),
                [Some(7000), Some(7001), Some(7000)][..expected as usize]
            );
        }
        assert!(
            !catalog.has_errors(),
            "inactive poisoned slots and item bytes must not be decoded"
        );
        assert!(catalog.bytes_read < 2048);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn schema_229_missing_root_f1_and_null_missing_or_empty_children_stay_distinct() {
    let mut bytes = root_f1_229_bytes(0);
    object(&mut bytes, 72, 7, &[(4, reference(6200))]);
    object(&mut bytes, 6200, 1, &[(2, reference(6320))]);
    object(&mut bytes, 6320, 0, &[(1, vec![1])]);
    let (_directory, path) = fixture(&bytes);
    let (catalog, unpack) = inspect_with_unpack(&path, MAX_BYTE_BUDGET).unwrap();
    assert_eq!(unpack, Observation::Value(true));
    assert!(
        catalog.collections.is_empty(),
        "settings-only source has no declared root.f1 directory"
    );
    assert!(
        catalog
            .objects
            .iter()
            .all(|entry| !entry.path.starts_with("root.f1"))
    );

    object(
        &mut bytes,
        72,
        7,
        &[(4, reference(6200)), (1, reference(0))],
    );
    let (_directory, path) = fixture(&bytes);
    let catalog = inspect(&path, MAX_BYTE_BUDGET).unwrap();
    assert_eq!(catalog.collections.len(), 4);
    assert!(catalog.collections.iter().all(
        |entry| matches!(&entry.state, Observation::Unavailable { parent } if parent == "root.f1")
    ));
    assert!(
        catalog
            .objects
            .iter()
            .any(|entry| entry.path == "root.f1" && entry.state == Observation::Null)
    );

    object(
        &mut bytes,
        72,
        7,
        &[(4, reference(6200)), (1, reference(256))],
    );
    object(
        &mut bytes,
        256,
        0,
        &[(0, reference(0)), (2, reference(768)), (3, reference(896))],
    );
    let (_directory, path) = fixture(&bytes);
    let catalog = inspect(&path, MAX_BYTE_BUDGET).unwrap();
    assert_eq!(catalog.collections[0].state, Observation::Null);
    assert_eq!(catalog.collections[1].state, Observation::Missing);
    for entry in &catalog.collections[2..] {
        let directory = value(&entry.state);
        assert_eq!(directory.logical_len, 0);
        assert!(value(&directory.sample).is_empty());
    }
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn schema_229_root_f1_faults_and_metadata_budget_never_become_empty_history() {
    for fault in [
        "negative",
        "width",
        "null-list",
        "active-reference",
        "branch-tag",
    ] {
        let mut bytes = root_f1_229_bytes(3);
        if fault == "branch-tag" {
            bytes[258] = 7;
        } else if fault == "active-reference" {
            bytes[4104..4112].copy_from_slice(&(-1_i64).to_be_bytes());
        } else {
            object(
                &mut bytes,
                512,
                0,
                &[
                    (0, int(if fault == "negative" { -1 } else { 3 })),
                    (1, int(if fault == "width" { 0 } else { 200 })),
                    (2, reference(if fault == "null-list" { 0 } else { 1024 })),
                    (3, int(0)),
                ],
            );
        }
        let (_directory, path) = fixture(&bytes);
        let catalog = inspect(&path, MAX_BYTE_BUDGET).unwrap();
        assert!(catalog.has_errors(), "{fault}");
        let entry = catalog
            .collections
            .iter()
            .find(|entry| entry.path == "root.f1.f0")
            .unwrap();
        assert!(
            match &entry.state {
                Observation::Failed(_) | Observation::Unavailable { .. } => true,
                Observation::Value(directory) =>
                    directory.logical_len == 3 && matches!(directory.sample, Observation::Failed(_)),
                _ => false,
            },
            "{fault} must retain an error or failed nonempty sample"
        );
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    let bytes = root_f1_229_bytes(3);
    let (_directory, path) = fixture(&bytes);
    for budget in [48, 128, 512, 1024, MAX_BYTE_BUDGET] {
        let catalog = inspect(&path, budget).unwrap();
        assert!(catalog.bytes_read <= budget);
    }
    assert_eq!(fs::read(path).unwrap(), bytes);
}
