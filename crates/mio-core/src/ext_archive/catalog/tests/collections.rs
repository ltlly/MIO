use super::*;
use std::io::Write;

#[test]
fn catalogs_every_observed_collection_by_field_id_without_reading_items() {
    let bytes = fixture_bytes();
    let (_directory, path) = fixture(&bytes);
    let report = inspect(&path, MAX_BYTE_BUDGET).unwrap();
    assert!(!report.has_errors());
    assert_eq!(report.bytes_read, 769);
    assert_eq!(
        report
            .collections
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        [
            "root.f1.f0",
            "root.f1.f1",
            "root.f1.f2",
            "root.f1.f3",
            "root.f2.f1",
            "root.f2.f4"
        ]
    );
    let root = value(&report.objects[0].state);
    assert_eq!(
        root.fields.iter().map(|field| field.id).collect::<Vec<_>>(),
        [99, 2, 1]
    );
    for collection in &report.collections[..4] {
        let collection = value(&collection.state);
        assert_eq!(collection.logical_len, 3);
        assert_eq!(
            collection.layout,
            Layout::Chunked {
                chunk_width: 2,
                leading_offset: 1,
                active_chunks: 2,
                chunk_capacity: 3
            }
        );
        assert_eq!(
            value(&collection.sample),
            &[Slot::Present, Slot::Null, Slot::Present]
        );
    }
    let direct = value(&report.collections[4].state);
    assert_eq!(direct.logical_len, 2);
    assert_eq!(direct.layout, Layout::Direct { capacity: 4 });
    assert_eq!(value(&direct.sample), &[Slot::Present, Slot::Null]);
    let empty = value(&report.collections[5].state);
    assert_eq!(empty.logical_len, 0);
    assert_eq!(empty.layout, Layout::Direct { capacity: 0 });
    assert!(value(&empty.sample).is_empty());
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn missing_null_empty_and_unavailable_are_not_interchangeable() {
    let mut bytes = fixture_bytes();
    object(
        &mut bytes,
        256,
        0,
        &[(0, reference(0)), (2, reference(768)), (3, reference(896))],
    );
    object(
        &mut bytes,
        768,
        0,
        &[(0, int(0)), (1, int(2)), (2, reference(1280)), (3, int(0))],
    );
    let (_directory, path) = fixture(&bytes);
    let report = inspect(&path, MAX_BYTE_BUDGET).unwrap();
    assert!(!report.has_errors());
    assert_eq!(report.collections[0].state, Observation::Null);
    assert_eq!(report.collections[1].state, Observation::Missing);
    assert_eq!(value(&report.collections[2].state).logical_len, 0);
    assert!(value(&value(&report.collections[2].state).sample).is_empty());
    bytes[64..72].fill(0);
    let (_directory, path) = fixture(&bytes);
    let report = inspect(&path, MAX_BYTE_BUDGET).unwrap();
    assert_eq!(report.objects[0].state, Observation::Null);
    assert_eq!(report.bytes_read, HEADER_READ_BYTES);
    assert!(
        report
            .collections
            .iter()
            .all(|entry| matches!(entry.state, Observation::Unavailable { .. }))
    );
}

#[test]
fn other_schema_pairs_are_explicitly_rejected() {
    for (floor, revision) in [(226_u16, 226_u16), (228, 229), (227, 228)] {
        let mut bytes = fixture_bytes();
        bytes[12..14].copy_from_slice(&floor.to_be_bytes());
        bytes[14..16].copy_from_slice(&revision.to_be_bytes());
        let (_directory, path) = fixture(&bytes);
        assert_eq!(
            inspect(&path, MAX_BYTE_BUDGET).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
    }
}

#[test]
fn unknown_object_types_preserve_descriptors_and_do_not_hide_other_collections() {
    let mut bytes = fixture_bytes();
    bytes[513] = 91;
    let (_directory, path) = fixture(&bytes);
    let report = inspect(&path, MAX_BYTE_BUDGET).unwrap();
    assert!(report.has_errors());
    assert!(
        matches!(&report.collections[0].state, Observation::Failed(error) if error.kind == io::ErrorKind::Unsupported)
    );
    let object = report
        .objects
        .iter()
        .find(|entry| entry.path == "root.f1.f0")
        .unwrap();
    let object = value(&object.state);
    assert_eq!((object.object_type, object.subtype), (91, 0));
    assert!(!object.layout_supported);
    assert_eq!(object.fields.len(), 4);
    assert_eq!(value(&report.collections[1].state).logical_len, 3);
}

#[test]
fn malformed_or_null_metadata_is_not_treated_as_a_valid_empty_collection() {
    for (length, backing, expected) in [
        (-1, 3328, "negative"),
        (0, 0, "null"),
        (5, 3328, "capacity"),
    ] {
        let mut bytes = fixture_bytes();
        object(
            &mut bytes,
            3072,
            0,
            &[(0, int(length)), (1, reference(backing))],
        );
        let (_directory, path) = fixture(&bytes);
        let report = inspect(&path, MAX_BYTE_BUDGET).unwrap();
        assert!(report.has_errors());
        let error = match &report.collections[4].state {
            Observation::Failed(error) => error,
            Observation::Value(value) => match &value.sample {
                Observation::Failed(error) => error,
                _ => panic!("invalid geometry must fail"),
            },
            _ => panic!("invalid metadata must fail"),
        };
        assert!(error.message.contains(expected), "{}", error.message);
        assert_eq!(value(&report.collections[5].state).logical_len, 0);
    }
}

#[test]
fn every_operation_stays_within_the_cumulative_budget_even_on_errors() {
    let (_directory, path) = fixture(&fixture_bytes());
    for budget in [48, 580, 600, 800, 1024, 2048] {
        let report = inspect(&path, budget).unwrap();
        assert!(report.bytes_read <= budget);
        assert_eq!(report.collections.len(), 6);
        if budget <= 600 {
            assert!(report.has_errors());
        }
        if budget == 2048 {
            assert!(!report.has_errors());
            assert_eq!(report.bytes_read, 769);
        }
    }
    for budget in [0, 47, MAX_BYTE_BUDGET + 1, u64::MAX] {
        assert_eq!(
            inspect(&path, budget).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}

#[test]
fn sampling_is_bounded_to_eight_slots_and_never_follows_item_addresses() {
    let mut bytes = fixture_bytes();
    object(&mut bytes, 3072, 0, &[(0, int(10)), (1, reference(3328))]);
    array(
        &mut bytes,
        3328,
        &[4096, 0, 4096, 0, 4096, 0, 4096, 0, -1, -1],
    );
    let (_directory, path) = fixture(&bytes);
    let report = inspect(&path, MAX_BYTE_BUDGET).unwrap();
    assert!(!report.has_errors());
    let collection = value(&report.collections[4].state);
    assert_eq!(collection.logical_len, 10);
    let slots = value(&collection.sample);
    assert_eq!(slots.len(), SAMPLE_ITEMS);
    assert_eq!(slots.iter().filter(|slot| **slot == Slot::Null).count(), 4);
}

#[test]
fn all_six_sources_page_original_references_across_chunk_boundaries() {
    let bytes = fixture_bytes();
    let (_directory, path) = fixture(&bytes);
    let report = inspect(&path, MAX_BYTE_BUDGET).unwrap();
    assert_eq!(report.bytes_read, 769, "retaining lists adds no reads");
    let mut source = Container::open(&path).unwrap();
    for (index, entry) in report.collections[..4].iter().enumerate() {
        let collection = value(&entry.state);
        let CollectionSource::Chunked(list) = &collection.source else {
            panic!("root.f1 lists must preserve their chunked layout");
        };
        assert_eq!(list.chunks.addresses.address, (1536 + index * 128) as u64);
        assert_eq!(list.leading_offset, 1);
        assert_eq!(collection.source.logical_len(), 3);
        assert_eq!(
            collection.source.page(&mut source, 0, 3).unwrap(),
            [Some(4096), None, Some(4096)]
        );
        assert_eq!(
            collection.source.page(&mut source, 1, 2).unwrap(),
            [None, Some(4096)]
        );
        assert!(
            collection
                .source
                .page(&mut source, 3, 256)
                .unwrap()
                .is_empty()
        );
    }
    let direct = &value(&report.collections[4].state).source;
    let CollectionSource::Direct(list) = direct else {
        panic!("root.f2.f1 must preserve its direct layout");
    };
    assert_eq!(
        (
            list.logical_len,
            list.addresses.address,
            list.addresses.element_count
        ),
        (2, 3328, 4)
    );
    assert_eq!(direct.page(&mut source, 0, 2).unwrap(), [Some(4096), None]);
    assert!(direct.page(&mut source, 2, 256).unwrap().is_empty());
    let empty = &value(&report.collections[5].state).source;
    assert_eq!(empty.logical_len(), 0);
    assert!(empty.page(&mut source, 0, 256).unwrap().is_empty());
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn paging_beyond_the_redacted_sample_uses_real_slots_not_presence_hints() {
    let mut bytes = fixture_bytes();
    object(&mut bytes, 3072, 0, &[(0, int(10)), (1, reference(3328))]);
    array(
        &mut bytes,
        3328,
        &[4096, 0, 4096, 0, 4096, 0, 4096, 0, 6000, 4096, -1],
    );
    bytes[6000..6032].fill(0xff);
    let (_directory, path) = fixture(&bytes);
    let report = inspect(&path, MAX_BYTE_BUDGET).unwrap();
    assert!(!report.has_errors());
    let collection = value(&report.collections[4].state);
    assert_eq!(value(&collection.sample).len(), SAMPLE_ITEMS);
    assert_eq!(collection.source.logical_len(), 10);
    let mut source = Container::open(&path).unwrap();
    let before = source.bytes_read();
    assert_eq!(
        collection.source.page(&mut source, 8, 2).unwrap(),
        [Some(6000), Some(4096)]
    );
    assert_eq!(source.bytes_read() - before, 8 + 8 * 2);
    assert_eq!(
        collection.source.page(&mut source, 7, 3).unwrap(),
        [None, Some(6000), Some(4096)]
    );
    assert!(
        collection
            .source
            .page(&mut source, 10, 1)
            .unwrap()
            .is_empty()
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn failed_sample_or_later_page_retains_the_list_and_individual_error_positions() {
    for bad_index in [1, 8] {
        let mut bytes = fixture_bytes();
        object(&mut bytes, 3072, 0, &[(0, int(10)), (1, reference(3328))]);
        let mut slots = [4096; 10];
        slots[bad_index] = -1;
        slots[bad_index + 1] = 0;
        array(&mut bytes, 3328, &slots);
        let (_directory, path) = fixture(&bytes);
        let report = inspect(&path, MAX_BYTE_BUDGET).unwrap();
        assert_eq!(report.has_errors(), bad_index < SAMPLE_ITEMS);
        let collection = value(&report.collections[4].state);
        assert_eq!(collection.source.logical_len(), 10);
        assert_eq!(
            matches!(collection.sample, Observation::Failed(_)),
            bad_index < SAMPLE_ITEMS
        );
        let mut source = Container::open(&path).unwrap();
        assert!(
            collection
                .source
                .page(&mut source, 0, 0)
                .unwrap()
                .is_empty()
        );
        assert!(collection.source.page(&mut source, 0, 10).is_err());
        assert_eq!(
            collection.source.page(&mut source, 0, 1).unwrap(),
            [Some(4096)]
        );
        assert_eq!(
            collection
                .source
                .page(&mut source, bad_index as u64, 1)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            collection
                .source
                .page(&mut source, (bad_index + 1) as u64, 1)
                .unwrap(),
            [None]
        );
        assert_eq!(
            collection.source.page(&mut source, 4, 1).unwrap(),
            [Some(4096)]
        );
    }
}

#[test]
fn page_limits_geometry_and_changed_sources_are_revalidated() {
    let mut bytes = fixture_bytes();
    object(&mut bytes, 3072, 0, &[(0, int(5)), (1, reference(3328))]);
    let (_directory, path) = fixture(&bytes);
    let report = inspect(&path, MAX_BYTE_BUDGET).unwrap();
    let mut source = Container::open(&path).unwrap();
    let direct = &value(&report.collections[4].state).source;
    assert!(
        direct.page(&mut source, 0, 0).is_err(),
        "logical length must not exceed physical capacity"
    );
    for index in [0, 5] {
        let collection = &value(&report.collections[index].state).source;
        let before = source.bytes_read();
        for (start, limit) in [(0, 257), (collection.logical_len() + 1, 1), (u64::MAX, 1)] {
            assert!(collection.page(&mut source, start, limit).is_err());
            assert_eq!(source.bytes_read(), before);
        }
    }
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&[0])
        .unwrap();
    let before = source.bytes_read();
    for index in [0, 5] {
        let collection = &value(&report.collections[index].state).source;
        let error = collection.page(&mut source, 0, 0).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("changed"));
        assert_eq!(source.bytes_read(), before);
    }
}
