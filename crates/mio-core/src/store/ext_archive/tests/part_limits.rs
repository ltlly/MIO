use super::*;

#[test]
fn batch_and_metadata_limits_fail_before_persisting_any_rows() {
    let mut store = ProjectStore::create(None).unwrap();
    let source_id = Uuid::new_v4();
    let mut report = initial(256);
    report.source_bytes = u64::MAX;
    assert!(store.begin_source(source_id, &report).is_err());
    report = initial(256);
    report.gaps = vec!["x".repeat(MAX_METADATA_BYTES + 1)];
    assert!(store.begin_source(source_id, &report).is_err());
    report = initial(256);
    report.complete_archive = true;
    assert!(store.begin_source(source_id, &report).is_err());
    assert!(store.ext_sources().unwrap().is_empty());
    store.begin_source(source_id, &initial(256)).unwrap();
    assert!(
        store
            .append_source_chunks(source_id, 0, &vec![vec![0]; 9])
            .is_err()
    );
    assert!(store.append_source_chunk(source_id, 0, &[]).is_err());
    assert!(
        store
            .append_source_chunk(source_id, 0, &vec![0; EXT_CHUNK_BYTES + 1])
            .is_err()
    );
    assert!(
        store
            .append_source_chunk(source_id, u64::MAX, &[0])
            .is_err()
    );
    let record = item(source_id, 0);
    let summary = message(&record, 15);
    assert!(
        store
            .put_ext_batch(&vec![record.clone(); 257], &[])
            .is_err()
    );
    assert!(
        store
            .put_ext_batch(&[], &vec![summary.clone(); 513])
            .is_err()
    );
    for change in 0..4 {
        let mut invalid = record.clone();
        match change {
            0 => invalid.index = u64::MAX,
            1 => invalid.object_address = Some(u64::MAX),
            2 => invalid.collection.clear(),
            3 => invalid.state.clear(),
            _ => unreachable!(),
        }
        assert!(store.put_ext_item(&invalid).is_err());
    }
    let mut oversized = summary;
    oversized.diagnostic = Some("x".repeat(MAX_SUMMARY_BYTES + 1));
    assert!(
        store
            .put_ext_batch(std::slice::from_ref(&record), &[oversized])
            .is_err()
    );
    assert!(store.ext_item(record.id).is_err());
    assert_eq!(
        source_state(&store.connection, source_id)
            .unwrap()
            .written_bytes,
        0
    );
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM ext_items", [], |row| row
                .get::<_, usize>(0))
            .unwrap(),
        0
    );
}

#[test]
fn an_empty_archive_has_no_chunks_and_only_allows_the_empty_range() {
    let (mut store, id) = archived(&[]);
    store.finish_source(id, &final_report(0, &[], &[])).unwrap();
    assert!(store.read_source_range(id, 0, 0).unwrap().is_empty());
    assert!(store.read_source_range(id, 0, 1).is_err());
    assert!(store.read_source_range(id, 1, 0).is_err());
    assert_eq!(
        store.ext_message_page(id, None, 0, 1).unwrap(),
        ExtMessagePage {
            total: 0,
            rows: vec![],
            next_offset: None,
        }
    );
}

#[test]
fn source_listing_is_bounded_and_never_silently_discards_an_extra_archive() {
    let (mut store, id) = archived(&[1]);
    assert!(store.begin_source(Uuid::new_v4(), &initial(1)).is_err());
    assert_eq!(store.ext_sources().unwrap().len(), 1);
    for _ in 0..3 {
        store.connection.execute(
            "INSERT INTO ext_sources(id,source_bytes,written_bytes,next_chunk,complete,report)
            SELECT ?1,source_bytes,written_bytes,next_chunk,complete,report FROM ext_sources WHERE id=?2",
            params![Uuid::new_v4().to_string(), id.to_string()],
        ).unwrap();
    }
    store
        .connection
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE ext_sources SET report=zeroblob(?1)",
            [MAX_METADATA_BYTES as i64 + 1],
        )
        .unwrap();
    assert!(
        store
            .ext_sources()
            .unwrap_err()
            .to_string()
            .contains("多个"),
        "the bounded source-count check rejects extra sources before loading any report"
    );
}

#[test]
fn aggregate_page_budget_returns_a_complete_prefix_and_continuation_without_loss() {
    let (mut store, id) = archived(&[0; 256]);
    let item = item(id, 0);
    let summaries: Vec<_> = (0..5)
        .map(|field| {
            let mut summary = message(&item, field);
            summary.diagnostic = Some(String::new());
            let overhead = serde_json::to_vec(&summary).unwrap().len();
            summary.diagnostic = Some("x".repeat(MAX_SUMMARY_BYTES - overhead));
            assert_eq!(
                serde_json::to_vec(&summary).unwrap().len(),
                MAX_SUMMARY_BYTES
            );
            summary
        })
        .collect();
    store
        .put_ext_batch(std::slice::from_ref(&item), &summaries)
        .unwrap();
    store
        .finish_source(
            id,
            &final_report(256, std::slice::from_ref(&item), &summaries),
        )
        .unwrap();
    let page = store.ext_message_page(id, None, 0, 4).unwrap();
    assert_eq!(
        (page.total, page.rows.len(), page.next_offset),
        (5, 4, Some(4))
    );
    assert_eq!(page.rows[3].id, summaries[3].id);
    drop(page);
    let page = store.ext_message_page(id, None, 0, 5).unwrap();
    assert_eq!(
        (page.total, page.rows.len(), page.next_offset),
        (5, 4, Some(4))
    );
    assert_eq!(
        page.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        summaries[..4].iter().map(|row| row.id).collect::<Vec<_>>()
    );
    let next_offset = page.next_offset.unwrap();
    drop(page);
    let page = store.ext_message_page(id, None, next_offset, 5).unwrap();
    assert_eq!(
        (page.total, page.rows.len(), page.next_offset),
        (5, 1, None)
    );
    assert_eq!(page.rows[0].id, summaries[4].id);
}

#[test]
fn collection_paging_does_not_scan_unrelated_items() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    const COLLECTION: &str = "root.f2.f1[0].f2";
    let source_id = Uuid::new_v4();
    let selected_items: Vec<_> = (0..2)
        .map(|index| {
            let mut item = item(source_id, index);
            item.id = Uuid::from_u128(0x100 + u128::from(index));
            item.collection = COLLECTION.into();
            item
        })
        .collect();
    let selected_messages: Vec<_> = selected_items
        .iter()
        .flat_map(|item| [message(item, 15), message(item, 18)])
        .collect();
    // EOF and a following item key use different VM steps; hold that boundary fixed per case.
    for (position, unrelated_base) in [("before", 0_u128), ("after", 0x200)] {
        let unrelated_items: Vec<_> = (0..192)
            .map(|index| {
                let mut item = item(source_id, index);
                item.id = Uuid::from_u128(unrelated_base + u128::from(index + 1));
                item
            })
            .collect();
        let unrelated_messages: Vec<_> = unrelated_items
            .iter()
            .map(|item| message(item, 15))
            .collect();

        let measured_page = |unrelated_count: usize| {
            let mut store = ProjectStore::create(None).unwrap();
            store.begin_source(source_id, &initial(256)).unwrap();
            store.append_source_chunk(source_id, 0, &[0; 256]).unwrap();
            let mut items = unrelated_items[..unrelated_count].to_vec();
            let mut messages = unrelated_messages[..unrelated_count].to_vec();
            items.extend_from_slice(&selected_items);
            messages.extend_from_slice(&selected_messages);
            store.put_ext_batch(&items, &messages).unwrap();
            store
                .finish_source(source_id, &final_report(256, &items, &messages))
                .unwrap();

            let steps = Arc::new(AtomicUsize::new(0));
            let observed = Arc::clone(&steps);
            store.connection.progress_handler(
                1,
                Some(move || {
                    observed.fetch_add(1, Ordering::Relaxed);
                    false
                }),
            );
            let page = store.ext_message_page(source_id, Some(COLLECTION), 1, 2);
            store.connection.progress_handler(0, None::<fn() -> bool>);
            let steps = steps.load(Ordering::Relaxed);
            assert_eq!(
                page.unwrap(),
                ExtMessagePage {
                    total: 4,
                    rows: selected_messages[1..3].to_vec(),
                    next_offset: Some(3),
                }
            );
            store.close().unwrap();
            steps
        };

        let small = measured_page(8);
        let larger = measured_page(192);
        println!("unrelated {position}: small={small}, larger={larger}");
        // Selected rows and parameters are identical; larger retains every small-archive row.
        assert_eq!(
            larger, small,
            "filtered pagination must not run more SQLite VM steps for unrelated items ({position})"
        );
    }
}
