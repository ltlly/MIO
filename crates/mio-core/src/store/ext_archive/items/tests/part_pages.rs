use super::*;

#[test]
fn item_pages_keep_empty_null_failed_and_unknown_slots_without_reading_details() {
    let (mut store, source_id) = pending();
    let first = item(source_id, 0);
    let same_object = item(source_id, 1);
    let mut null = item(source_id, 2);
    null.state = "null".into();
    null.object_address = None;
    null.metadata_json = br#"{"slot":"null"}"#.to_vec();
    let mut failed = item(source_id, 3);
    failed.state = "failed".into();
    failed.metadata_json = br#"{"f18":{"state":"failed","error":"invalid reference"}}"#.to_vec();
    let mut unknown = item(source_id, 4);
    unknown.state = "unmapped object".into();
    let mut empty_tab = item(source_id, 0);
    empty_tab.collection = "root.f2.f1".into();
    empty_tab.state = "available".into();
    empty_tab.metadata_json =
        br#"{"f2":{"state":"present","length":0},"f4":{"state":"null"}}"#.to_vec();
    let mut null_history = null.clone();
    null_history.id = Uuid::new_v4();
    null_history.index = 0;
    null_history.collection = "root.f2.f1[0].f2".into();
    let items = [
        null_history,
        failed,
        first,
        empty_tab,
        null,
        same_object,
        unknown,
    ];
    let messages = [
        message(&items[2], 18),
        message(&items[5], 15),
        message(&items[2], 15),
    ];
    finish(&mut store, source_id, &items, &messages);
    store.connection.authorizer(Some(deny_details));
    let mut expected: Vec<_> = items
        .iter()
        .map(|item| {
            summary(
                item,
                messages.iter().filter(|m| m.item_id == item.id).count(),
            )
        })
        .collect();
    expected.sort_by(|a, b| (&a.collection, a.index).cmp(&(&b.collection, b.index)));
    let mut offset = 0;
    let mut actual = Vec::new();
    loop {
        let page = store.ext_item_page(source_id, None, offset, 2).unwrap();
        assert_eq!(page.total, items.len());
        assert!(!page.rows.is_empty() && page.rows.len() <= 2);
        offset += page.rows.len();
        actual.extend(page.rows);
        assert_eq!(page.next_offset, (offset < items.len()).then_some(offset));
        if page.next_offset.is_none() {
            break;
        }
    }
    assert_eq!(actual, expected);
    assert_eq!(
        store
            .ext_item_page(source_id, Some("root.f1.f0"), 1, 500)
            .unwrap(),
        ExtItemPage {
            total: 5,
            rows: expected[1..5].to_vec(),
            next_offset: None,
        }
    );
    assert_eq!(
        store
            .ext_item_page(source_id, Some("root.absent"), 0, 1)
            .unwrap(),
        ExtItemPage {
            total: 0,
            rows: vec![],
            next_offset: None,
        }
    );
    assert!(
        store
            .ext_item_page(source_id, None, items.len(), 1)
            .unwrap()
            .rows
            .is_empty()
    );
    assert!(
        store
            .ext_item_detail(source_id, items[2].id, 0, 1)
            .is_err()
    );
}

#[test]
fn details_preserve_metadata_and_item_association_through_save_as_and_reopen() {
    let (mut store, source_id) = pending();
    let first = item(source_id, 0);
    let second = item(source_id, 1);
    let mut empty = item(source_id, 0);
    empty.collection = "root.f2.f1".into();
    empty.metadata_json =
        b"{\n \"current\": null, \"history\": [], \"f33\": {\"state\":\"unverified\"}\n}".to_vec();
    let mut response = message(&first, 18);
    response.role = "response".into();
    response.method = None;
    response.request_target = None;
    response.status = Some(200);
    let messages = [response, message(&second, 15), message(&first, 15)];
    let items = [first, second, empty];
    finish(&mut store, source_id, &items, &messages);
    let check = |store: &ProjectStore| {
        store.connection.authorizer(Some(deny_raw));
        let detail = store
            .ext_item_detail(source_id, items[0].id, 0, 1)
            .unwrap();
        assert_eq!(detail.item, items[0]);
        assert_eq!(
            detail.messages,
            ExtMessagePage {
                total: 2,
                rows: vec![messages[2].clone()],
                next_offset: Some(1),
            }
        );
        assert_eq!(
            store
                .ext_item_detail(source_id, items[0].id, 1, 500)
                .unwrap()
                .messages,
            ExtMessagePage {
                total: 2,
                rows: vec![messages[0].clone()],
                next_offset: None,
            }
        );
        let detail = store
            .ext_item_detail(source_id, items[1].id, 0, 500)
            .unwrap();
        assert_eq!(detail.item, items[1]);
        assert_eq!(detail.messages.rows, [messages[1].clone()]);
        let detail = store
            .ext_item_detail(source_id, items[2].id, 0, 500)
            .unwrap();
        assert_eq!(detail.item, items[2]);
        assert_eq!(
            detail.messages,
            ExtMessagePage {
                total: 0,
                rows: vec![],
                next_offset: None,
            }
        );
        let detail = store
            .ext_item_detail(source_id, items[0].id, 2, 1)
            .unwrap();
        assert!(detail.messages.rows.is_empty());
        assert_eq!(detail.messages.next_offset, None);
        store
            .connection
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    };
    check(&store);
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("items.mio");
    let copy = directory.path().join("items-copy.mio");
    store.save(Some(&original)).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    for path in [&original, &copy] {
        let store = ProjectStore::open(path).unwrap();
        check(&store);
        assert_eq!(
            store.read_source_range(source_id, 0, RAW.len()).unwrap(),
            RAW
        );
        store.close().unwrap();
    }
}

#[test]
fn queries_require_complete_matching_sources_and_checked_page_ranges() {
    let (mut store, source_id) = pending();
    let record = item(source_id, 0);
    store.put_ext_item(&record).unwrap();
    assert!(store.ext_item_page(source_id, None, 0, 1).is_err());
    assert!(store.ext_item_detail(source_id, record.id, 0, 1).is_err());
    finish(&mut store, source_id, std::slice::from_ref(&record), &[]);
    for (offset, limit) in [(0, 0), (0, 501), (usize::MAX, 1), (i64::MAX as usize, 1)] {
        assert!(
            store
                .ext_item_page(source_id, None, offset, limit)
                .is_err()
        );
        assert!(
            store
                .ext_item_detail(source_id, record.id, offset, limit)
                .is_err()
        );
    }
    for collection in [String::new(), "x".repeat(MAX_COLLECTION_BYTES + 1)] {
        assert!(
            store
                .ext_item_page(source_id, Some(&collection), 0, 1)
                .is_err()
        );
    }
    assert!(store.ext_item_page(Uuid::new_v4(), None, 0, 1).is_err());
    assert!(
        store
            .ext_item_detail(Uuid::new_v4(), record.id, 0, 1)
            .is_err()
    );
    assert!(
        store
            .ext_item_detail(source_id, Uuid::new_v4(), 0, 1)
            .is_err()
    );
    let other_source = Uuid::new_v4();
    store.connection.execute(
        "INSERT INTO ext_sources(id,source_bytes,written_bytes,next_chunk,complete,report)
        SELECT ?1,source_bytes,written_bytes,next_chunk,complete,report FROM ext_sources WHERE id=?2",
        params![other_source.to_string(), source_id.to_string()],
    ).unwrap();
    assert!(
        store
            .ext_item_detail(other_source, record.id, 0, 1)
            .is_err()
    );
    assert_eq!(
        store
            .ext_item_page(other_source, None, 0, 1)
            .unwrap()
            .total,
        0
    );
    store
        .connection
        .execute(
            "UPDATE ext_sources SET written_bytes=0 WHERE id=?1",
            [source_id.to_string()],
        )
        .unwrap();
    assert!(store.ext_item_page(source_id, None, 0, 1).is_err());
    assert!(store.ext_item_detail(source_id, record.id, 0, 1).is_err());
}

#[test]
fn detail_budget_counts_serialized_item_before_complete_message_prefixes() {
    let (mut store, source_id) = pending();
    let mut record = item(source_id, 0);
    record.metadata_json = vec![b' '; MAX_METADATA_BYTES - 2];
    record.metadata_json.extend_from_slice(b"{}");
    let messages: Vec<_> = (0..5)
        .map(|field_id| {
            let mut message = message(&record, field_id);
            message.diagnostic = Some(String::new());
            let overhead = serde_json::to_vec(&message).unwrap().len();
            let bytes = if field_id == 3 {
                MAX_SUMMARY_BYTES / 2
            } else {
                MAX_SUMMARY_BYTES
            };
            message.diagnostic = Some("x".repeat(bytes - overhead));
            assert_eq!(serde_json::to_vec(&message).unwrap().len(), bytes);
            message
        })
        .collect();
    finish(
        &mut store,
        source_id,
        std::slice::from_ref(&record),
        &messages,
    );
    let item_bytes = serde_json::to_vec(&record).unwrap().len();
    assert!(item_bytes > record.metadata_json.len());
    let expected_first = (MAX_PAGE_SUMMARY_BYTES - item_bytes) / MAX_SUMMARY_BYTES;
    assert_eq!(expected_first, 3);
    let first_four_bytes: usize = messages[..4]
        .iter()
        .map(|message| serde_json::to_vec(message).unwrap().len())
        .sum();
    assert!(record.metadata_json.len() + first_four_bytes <= MAX_PAGE_SUMMARY_BYTES);
    assert!(item_bytes + first_four_bytes > MAX_PAGE_SUMMARY_BYTES);
    let detail = store
        .ext_item_detail(source_id, record.id, 0, 500)
        .unwrap();
    assert_eq!(detail.item.metadata_json, record.metadata_json);
    assert_eq!(detail.messages.total, 5);
    assert_eq!(detail.messages.rows, messages[..expected_first]);
    assert_eq!(detail.messages.next_offset, Some(expected_first));
    assert!(item_bytes + detail.messages.rows.len() * MAX_SUMMARY_BYTES <= MAX_PAGE_SUMMARY_BYTES);
    let detail = store
        .ext_item_detail(
            source_id,
            record.id,
            detail.messages.next_offset.unwrap(),
            500,
        )
        .unwrap();
    assert_eq!(detail.messages.rows, messages[expected_first..]);
    assert_eq!(detail.messages.total, 5);
    assert_eq!(detail.messages.next_offset, None);
}

#[test]
fn item_page_budget_uses_encoded_fields_and_continues_at_actual_prefix_end() {
    let (mut store, source_id) = pending();
    let items: Vec<_> = (0..200)
        .map(|index| {
            let mut item = item(source_id, index);
            item.state = "\u{0001}".repeat(MAX_COLLECTION_BYTES);
            item
        })
        .collect();
    finish(&mut store, source_id, &items, &[]);
    let first = store.ext_item_page(source_id, None, 0, 500).unwrap();
    let consumed = first.rows.len();
    assert!(consumed > 0 && consumed < items.len());
    assert_eq!(first.total, items.len());
    assert_eq!(first.next_offset, Some(consumed));
    let encoded_bytes: usize = first
        .rows
        .iter()
        .map(|row| serde_json::to_vec(row).unwrap().len())
        .sum();
    assert!(encoded_bytes <= MAX_PAGE_SUMMARY_BYTES);
    assert!(
        encoded_bytes
            + serde_json::to_vec(&summary(&items[consumed], 0))
                .unwrap()
                .len()
            > MAX_PAGE_SUMMARY_BYTES
    );
    let second = store
        .ext_item_page(source_id, None, consumed, 500)
        .unwrap();
    assert_eq!(second.total, items.len());
    assert_eq!(second.next_offset, None);
    let actual: Vec<_> = first.rows.into_iter().chain(second.rows).collect();
    assert_eq!(
        actual,
        items
            .iter()
            .map(|item| summary(item, 0))
            .collect::<Vec<_>>()
    );
}
