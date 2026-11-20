use super::*;

#[test]
fn byte_windows_preserve_every_byte_and_empty_records() {
    let bytes = primitive_bytes();
    let fixture = Fixture::new(&bytes);
    let mut source = Container::open(&fixture.path).unwrap();
    let record = source.array_header(256, ArrayKind::Bytes).unwrap();
    assert_eq!(
        (
            record.total_size,
            record.element_count,
            record.payload_bytes
        ),
        (264, 256, 256)
    );
    let mut output = [0; 256];
    source.read_payload(&record, 0, &mut output).unwrap();
    assert_eq!(output.as_slice(), &(0..=255).collect::<Vec<u8>>());
    let mut slice = [0; 3];
    source.read_payload(&record, 253, &mut slice).unwrap();
    assert_eq!(slice, [253, 254, 255]);
    assert!(source.read_payload(&record, 254, &mut slice).is_err());
    assert!(source.read_payload(&record, u64::MAX, &mut slice).is_err());
    let empty = source.array_header(528, ArrayKind::Bytes).unwrap();
    assert_eq!((empty.total_size, empty.payload_bytes), (9, 0));
    source.read_payload(&empty, 0, &mut []).unwrap();
    assert!(source.read_payload(&empty, 0, &mut [0]).is_err());
    assert_eq!(fs::read(&fixture.path).unwrap(), bytes);
}

#[test]
fn reference_pages_preserve_nulls_duplicates_and_logical_order() {
    let fixture = Fixture::new(&primitive_bytes());
    let mut source = Container::open(&fixture.path).unwrap();
    let record = source.array_header(544, ArrayKind::References).unwrap();
    assert_eq!(
        source.references_page(&record, 0, 4).unwrap(),
        vec![None, Some(72), Some(128), Some(72)]
    );
    assert_eq!(
        source.references_page(&record, 1, 2).unwrap(),
        vec![Some(72), Some(128)]
    );
    assert!(source.references_page(&record, 4, 2).unwrap().is_empty());
    assert!(source.references_page(&record, 5, 1).is_err());
    assert!(source.references_page(&record, u64::MAX, 1).is_err());
    assert!(
        source
            .references_page(&record, 0, MAX_PAGE_ITEMS + 1)
            .is_err()
    );
    let bytes = source.array_header(256, ArrayKind::Bytes).unwrap();
    assert!(source.references_page(&bytes, 0, 1).is_err());
}

#[test]
fn utf16_pages_preserve_surrogates_across_page_boundaries() {
    let fixture = Fixture::new(&primitive_bytes());
    let mut source = Container::open(&fixture.path).unwrap();
    let record = source.array_header(600, ArrayKind::Utf16).unwrap();
    let mut units = source.utf16_page(&record, 0, 2).unwrap();
    assert_eq!(units, [0x4e2d, 0xd83d]);
    units.extend(source.utf16_page(&record, 2, 1).unwrap());
    assert_eq!(String::from_utf16(&units).unwrap(), "中😀");
    let unpaired = source.utf16_page(&record, 3, 1).unwrap();
    assert_eq!(unpaired, [0xd800]);
    assert!(String::from_utf16(&unpaired).is_err());
    assert!(source.utf16_page(&record, 4, 1).unwrap().is_empty());
    assert!(source.utf16_page(&record, 5, 1).is_err());
    assert!(
        source
            .utf16_page(&record, 0, MAX_PAYLOAD_BYTES / 2 + 1)
            .is_err()
    );
    let bytes = source.array_header(256, ArrayKind::Bytes).unwrap();
    assert!(source.utf16_page(&bytes, 0, 1).is_err());
}

#[test]
fn array_headers_reject_negative_huge_inconsistent_and_unallocated_records() {
    for (total, count) in [
        (-1_i32, 1_i32),
        (7, 1),
        (9, -1),
        (9, 2),
        (8, 0),
        (264, i32::MAX),
        (i32::MAX, 1),
    ] {
        let mut bytes = primitive_bytes();
        bytes[256..260].copy_from_slice(&total.to_be_bytes());
        bytes[260..264].copy_from_slice(&count.to_be_bytes());
        let fixture = Fixture::new(&bytes);
        let mut source = Container::open(&fixture.path).unwrap();
        assert_eq!(
            source
                .array_header(256, ArrayKind::Bytes)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
    let fixture = Fixture::new(&primitive_bytes());
    let mut source = Container::open(&fixture.path).unwrap();
    for offset in [0, 64, 1020, 1024, 1536, u64::MAX] {
        assert_eq!(
            source
                .array_header(offset, ArrayKind::Bytes)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
    assert!(source.array_header(256, ArrayKind::References).is_err());
    assert_eq!(
        source
            .array_header(528, ArrayKind::Utf16)
            .unwrap_err()
            .kind(),
        io::ErrorKind::Unsupported
    );
}

#[test]
fn empty_reference_arrays_use_the_verified_nine_byte_framing() {
    let fixture = Fixture::new(&primitive_bytes());
    let mut source = Container::open(&fixture.path).unwrap();
    let record = source.array_header(528, ArrayKind::References).unwrap();
    assert_eq!(
        (
            record.total_size,
            record.element_count,
            record.payload_bytes
        ),
        (9, 0, 0)
    );
    assert!(source.references_page(&record, 0, 1).unwrap().is_empty());
    let list = DirectList {
        logical_len: 0,
        addresses: record,
    };
    assert!(source.direct_page(&list, 0, 1).unwrap().is_empty());
    assert!(source.references_page(&list.addresses, 1, 1).is_err());
    let mut bytes = primitive_bytes();
    bytes[528..532].copy_from_slice(&8_i32.to_be_bytes());
    let fixture = Fixture::new(&bytes);
    let mut source = Container::open(&fixture.path).unwrap();
    assert_eq!(
        source
            .array_header(528, ArrayKind::References)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
}

#[test]
fn payload_windows_revalidate_metadata_and_enforce_the_byte_limit() {
    let fixture = Fixture::new(&primitive_bytes());
    let mut source = Container::open(&fixture.path).unwrap();
    let mut record = source.array_header(256, ArrayKind::Bytes).unwrap();
    let before = source.bytes_read();
    assert!(
        source
            .read_payload(&record, 0, &mut vec![0; MAX_PAYLOAD_BYTES + 1])
            .is_err()
    );
    assert_eq!(source.bytes_read(), before);
    record.payload_bytes += 1;
    assert!(
        source
            .read_payload(&record, 0, &mut [0])
            .unwrap_err()
            .to_string()
            .contains("metadata")
    );
}

#[test]
fn invalid_array_references_do_not_become_nulls() {
    for pointer in [-1, 64, 1024, i64::MAX] {
        let mut bytes = primitive_bytes();
        put_refs(&mut bytes, 544, &[pointer]);
        let fixture = Fixture::new(&bytes);
        let mut source = Container::open(&fixture.path).unwrap();
        let record = source.array_header(544, ArrayKind::References).unwrap();
        assert_eq!(
            source.references_page(&record, 0, 1).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}

#[test]
fn direct_pages_ignore_inactive_capacity_but_keep_active_holes() {
    let mut bytes = primitive_bytes();
    put_refs(&mut bytes, 544, &[72, 0, -1, 1024]);
    let fixture = Fixture::new(&bytes);
    let mut source = Container::open(&fixture.path).unwrap();
    let addresses = source.array_header(544, ArrayKind::References).unwrap();
    let list = DirectList {
        logical_len: 2,
        addresses,
    };
    assert_eq!(
        source.direct_page(&list, 0, 256).unwrap(),
        vec![Some(72), None]
    );
    assert!(source.direct_page(&list, 2, 1).unwrap().is_empty());
    assert!(source.references_page(&list.addresses, 0, 4).is_err());
    let invalid = DirectList {
        logical_len: 5,
        ..list.clone()
    };
    assert!(source.direct_page(&invalid, 0, 1).is_err());
    let empty = DirectList {
        logical_len: 0,
        ..list
    };
    assert!(source.direct_page(&empty, 0, 256).unwrap().is_empty());
}

fn chunked_fixture(source: &mut Container) -> ChunkedList {
    ChunkedList {
        logical_len: 3,
        chunk_width: 2,
        leading_offset: 1,
        chunks: DirectList {
            logical_len: 2,
            addresses: source.array_header(800, ArrayKind::References).unwrap(),
        },
    }
}

#[test]
fn chunked_pages_follow_leading_offsets_without_reading_stale_slots() {
    let fixture = Fixture::new(&primitive_bytes());
    let mut source = Container::open(&fixture.path).unwrap();
    let list = chunked_fixture(&mut source);
    let before = source.bytes_read();
    assert_eq!(
        source.chunked_page(&list, 0, 256).unwrap(),
        vec![Some(72), Some(128), Some(192)]
    );
    assert_eq!(source.bytes_read() - before, 64);
    assert_eq!(source.chunked_page(&list, 1, 1).unwrap(), vec![Some(128)]);
    assert!(source.chunked_page(&list, 3, 1).unwrap().is_empty());
    assert!(source.chunked_page(&list, 4, 1).is_err());
    assert!(source.chunked_page(&list, 0, MAX_PAGE_ITEMS + 1).is_err());
    let mut empty = list;
    empty.logical_len = 0;
    assert!(source.chunked_page(&empty, 0, 1).unwrap().is_empty());
}

#[test]
fn chunked_pages_reject_bad_geometry_missing_chunks_and_cycles() {
    for case in 0..7 {
        let mut bytes = primitive_bytes();
        match case {
            4 => put_refs(&mut bytes, 800, &[0, 880, 1024]),
            5 => put_refs(&mut bytes, 800, &[800, 880, 1024]),
            6 => put_refs(&mut bytes, 880, &[128]),
            _ => {}
        }
        let fixture = Fixture::new(&bytes);
        let mut source = Container::open(&fixture.path).unwrap();
        let mut list = chunked_fixture(&mut source);
        match case {
            0 => list.chunk_width = 0,
            1 => list.leading_offset = u64::MAX,
            2 => list.chunks.logical_len = 1,
            3 => list.chunk_width = u64::MAX,
            _ => {}
        }
        assert_eq!(
            source.chunked_page(&list, 0, 3).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}

#[test]
fn each_bounded_operation_gets_a_fresh_budget() {
    let fixture = Fixture::new(&primitive_bytes());
    let mut source = Container::open(&fixture.path).unwrap();
    let record = source.array_header(256, ArrayKind::Bytes).unwrap();
    for _ in 0..40 {
        assert_eq!(source.compact_object(128).unwrap().descriptors.len(), 3);
        source.read_payload(&record, 0, &mut [0; 256]).unwrap();
    }
    assert!(source.bytes_read() > MAX_READ_BYTES);
    assert!(source.objects_read > MAX_OBJECTS);
}

#[test]
fn a_changed_source_is_rejected_before_a_new_page_read() {
    let fixture = Fixture::new(&primitive_bytes());
    let mut source = Container::open(&fixture.path).unwrap();
    let record = source.array_header(256, ArrayKind::Bytes).unwrap();
    let before = source.bytes_read();
    File::options()
        .write(true)
        .open(&fixture.path)
        .unwrap()
        .set_len(1536)
        .unwrap();
    let error = source.read_payload(&record, 0, &mut [0]).unwrap_err();
    assert!(error.to_string().contains("source changed"));
    assert_eq!(source.bytes_read(), before);
}
