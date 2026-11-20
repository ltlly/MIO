use super::*;

#[test]
fn aliases_collapse_but_distinct_failed_and_null_slots_survive_filter_save_and_reopen() {
    let (mut store, source, items) = fixture(0, false, false);
    let live = crate::store::tests::record();
    store.put(&live).unwrap();
    store
        .connection
        .authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Read {
                table_name: "ext_source_chunks",
                column_name: "data",
            }
            | AuthAction::Read {
                table_name: "messages",
                column_name: "body",
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }));
    let (total, rows) = store.history_page(0, 64, "").unwrap();
    assert_eq!(total, 5);
    assert_eq!(rows[0], HistoryRow::Live(live.summary.clone()));
    let archived_ids: Vec<_> = rows[1..].iter().map(HistoryRow::id).collect();
    assert_eq!(
        archived_ids,
        [3, 4, 2, 0].map(|index| crate::HistoryEntryId::External {
            source_id: source,
            item_id: items[index].id
        })
    );
    assert_eq!(store.history_page(1, 2, "").unwrap().1, rows[1..3]);
    assert_eq!(store.history_page(3, 2, "").unwrap().1, rows[3..]);
    assert_eq!(store.history_page(0, 64, "PoSt").unwrap().0, 3);
    assert_eq!(
        store
            .history_page(0, 64, "HTTP://127.0.0.1:18766/fixture")
            .unwrap()
            .1[0]
            .id(),
        crate::HistoryEntryId::External {
            source_id: source,
            item_id: items[2].id
        }
    );
    assert_eq!(store.history_page(0, 64, "header.invalid").unwrap().0, 0);
    assert!(store.history_page(5, 64, "").unwrap().1.is_empty());
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    let ExtHistoryRow {
        request: Some(request),
        response: Some(summary),
        origin,
        ..
    } = archived(&rows[4])
    else {
        panic!("mapped Proxy pair missing")
    };
    assert_eq!(origin.as_deref(), Some("http://127.0.0.1:18765"));
    assert_eq!(summary.status, Some(201));
    assert_eq!(summary.raw_bytes, 139);
    assert_eq!(summary.raw_bytes - summary.body_offset.unwrap(), 3);
    assert_eq!(
        store
            .ext_message_fragment(summary.id, 0, 139)
            .unwrap()
            .bytes,
        response()
    );
    assert_eq!(
        store
            .ext_message_fragment(request.id, 0, REQUEST.len())
            .unwrap()
            .bytes,
        REQUEST
    );
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("history.mio");
    let copy = directory.path().join("copy.mio");
    store.save(Some(&original)).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    for path in [original, copy] {
        let reopened = ProjectStore::open(&path).unwrap();
        assert_eq!(
            reopened.history_page(0, 64, "").unwrap(),
            (total, rows.clone())
        );
        assert_eq!(
            reopened
                .ext_message_fragment(summary.id, 0, 139)
                .unwrap()
                .bytes,
            response()
        );
        reopened.close().unwrap();
    }
}
