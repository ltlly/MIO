use super::*;

#[test]
fn message_pages_are_body_free_snapshot_ordered_and_bounded() {
    let (mut store, id) = archived(&[0; 256]);
    let first = item(id, 0);
    let second = item(id, 1);
    let mut nested = item(id, 0);
    nested.collection = "root.f2.f1[0].f2".into();
    let messages = [
        message(&second, 18),
        message(&nested, 3),
        message(&first, 18),
        message(&first, 15),
    ];
    let items = [nested, second, first];
    store.put_ext_batch(&items, &messages).unwrap();
    store
        .finish_source(id, &final_report(256, &items, &messages))
        .unwrap();
    store.connection.authorizer(Some(deny_archive_reads));
    assert_eq!(
        store.ext_message_page(id, None, 0, 2).unwrap(),
        ExtMessagePage {
            total: 4,
            rows: vec![messages[3].clone(), messages[2].clone()],
            next_offset: Some(2),
        }
    );
    assert_eq!(
        store.ext_message_page(id, None, 2, 2).unwrap(),
        ExtMessagePage {
            total: 4,
            rows: vec![messages[0].clone(), messages[1].clone()],
            next_offset: None,
        }
    );
    assert_eq!(
        store
            .ext_message_page(id, Some("root.f1.f0"), 1, 2)
            .unwrap(),
        ExtMessagePage {
            total: 3,
            rows: vec![messages[2].clone(), messages[0].clone()],
            next_offset: None,
        }
    );
    assert_eq!(
        store
            .ext_message_page(id, Some("root.unknown"), 0, 1)
            .unwrap(),
        ExtMessagePage {
            total: 0,
            rows: vec![],
            next_offset: None,
        }
    );
    assert!(
        store
            .ext_message_page(id, None, 4, 1)
            .unwrap()
            .rows
            .is_empty()
    );
    assert!(store.ext_message_fragment(messages[0].id, 0, 1).is_err());
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert!(store.ext_message_page(id, None, 0, 0).is_err());
    assert!(store.ext_message_page(id, None, 0, 501).is_err());
    assert!(store.ext_message_page(id, None, usize::MAX, 1).is_err());
    assert!(
        store
            .ext_message_page(id, None, i64::MAX as usize, 1)
            .is_err()
    );
    assert!(store.ext_message_page(id, Some(""), 0, 1).is_err());
    assert!(
        store
            .ext_message_page(id, Some(&"x".repeat(MAX_COLLECTION_BYTES + 1)), 0, 1)
            .is_err()
    );
    assert!(store.ext_message_page(Uuid::new_v4(), None, 0, 1).is_err());
}

#[test]
fn exact_ranges_and_ownership_reject_overflow_or_unfinished_sources_without_truncation() {
    let (mut store, id) = archived(&[7; 256]);
    let item = item(id, 0);
    let good = message(&item, 15);
    store.put_ext_item(&item).unwrap();
    assert!(store.read_source_range(id, 0, 1).is_err());
    assert!(store.ext_message_page(id, None, 0, 1).is_err());
    for change in 0..7 {
        let mut bad = good.clone();
        match change {
            0 => bad.source_id = Uuid::new_v4(),
            1 => bad.item_index += 1,
            2 => bad.collection.push_str(".other"),
            3 => bad.raw_offset = u64::MAX,
            4 => bad.raw_bytes = 257,
            5 => bad.body_offset = Some(bad.raw_bytes + 1),
            6 => bad.status = Some(99),
            _ => unreachable!(),
        }
        assert!(
            store.put_ext_message(&bad).is_err(),
            "invalid variant {change}"
        );
    }
    store.put_ext_message(&good).unwrap();
    store
        .finish_source(
            id,
            &final_report(
                256,
                std::slice::from_ref(&item),
                std::slice::from_ref(&good),
            ),
        )
        .unwrap();
    assert!(store.read_source_range(id, 256, 0).unwrap().is_empty());
    assert!(
        store
            .ext_message_fragment(good.id, good.raw_bytes, 0)
            .unwrap()
            .bytes
            .is_empty()
    );
    for (offset, length) in [
        (257, 0),
        (255, 2),
        (u64::MAX, 1),
        (0, EXT_FRAGMENT_BYTES + 1),
    ] {
        assert!(store.read_source_range(id, offset, length).is_err());
    }
    for (offset, length) in [
        (21, 0),
        (19, 2),
        (u64::MAX, 1),
        (0, EXT_FRAGMENT_BYTES + 1),
    ] {
        assert!(
            store
                .ext_message_fragment(good.id, offset, length)
                .is_err()
        );
    }
    assert!(store.ext_message_fragment(Uuid::new_v4(), 0, 1).is_err());
    assert!(store.read_source_range(Uuid::new_v4(), 0, 0).is_err());
}

#[test]
fn compressed_chunks_reject_truncated_extra_short_long_invalid_or_concatenated_streams() {
    let raw = vec![b'Z'; 1024];
    let (mut store, id) = archived(&raw);
    store
        .finish_source(id, &final_report(raw.len(), &[], &[]))
        .unwrap();
    let (codec, encoded) = encode_chunk(&raw).unwrap();
    assert_eq!(codec, 1);
    let mut trailing = encoded.clone();
    trailing.push(0);
    let mut truncated = encoded.clone();
    truncated.pop();
    let mut concatenated = encoded.clone();
    concatenated.extend_from_slice(&encoded);
    for corrupt in [
        trailing,
        truncated,
        concatenated,
        encode_chunk(&[b'Z'; 1023]).unwrap().1,
        encode_chunk(&[b'Z'; 1025]).unwrap().1,
        vec![0, 255, 1, 2],
    ] {
        store
            .connection
            .execute("UPDATE ext_source_chunks SET data=?1", [corrupt])
            .unwrap();
        assert!(store.read_source_range(id, 0, 1).is_err());
    }
    store
        .connection
        .execute("UPDATE ext_source_chunks SET data=?1", [encoded])
        .unwrap();
    assert_eq!(store.read_source_range(id, 0, raw.len()).unwrap(), raw);
}
