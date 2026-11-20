use super::*;

#[test]
fn mismatched_archive_identity_is_rejected_before_status_filter_excludes_the_row() {
    let (store, source, items) =
        super::super::super::proxy_history::tests::fixture(0, false, false);
    let item = &items[0];
    let (message_id, summary): (String, Vec<u8>) = store
        .connection
        .query_row(
            "SELECT id,summary FROM ext_messages WHERE item=?1 AND field_id=18",
            [item.id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let archive: Vec<u8> = store
        .connection
        .query_row(
            "SELECT data FROM ext_source_chunks WHERE source=?1",
            [source.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let mut q = query(&store);
    q.url_filter = "18765".into();
    q.filter.hide_without_response = true;
    q.filter.status_classes = [true, false, false, false];
    let before = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(before.total, 1);
    assert_eq!(
        before.rows[0].id(),
        HistoryEntryId::External {
            source_id: source,
            item_id: item.id
        }
    );

    let mut damaged: crate::ExtMessageSummary = serde_json::from_slice(&summary).unwrap();
    damaged.item_id = Uuid::new_v4();
    damaged.status = Some(404);
    let damaged = serde_json::to_vec(&damaged).unwrap();
    store
        .connection
        .execute(
            "UPDATE ext_messages SET summary=?2 WHERE id=?1",
            params![&message_id, &damaged],
        )
        .unwrap();
    store.connection.authorizer(Some(no_bodies));
    assert!(
        store.history_page(0, 64, "18765").is_err(),
        "the established history reader already rejects this message identity mismatch"
    );
    let result = store.query_http_history(&q, &CancellationToken::new());
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT summary FROM ext_messages WHERE id=?1",
                [&message_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .unwrap(),
        damaged,
        "a read-only filter must not rewrite the damaged summary"
    );
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT data FROM ext_source_chunks WHERE source=?1",
                [source.to_string()],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .unwrap(),
        archive,
        "identity rejection must preserve every original archive byte"
    );
    assert!(
        result.is_err(),
        "a valid-shaped summary with a foreign item ID must not be hidden by its nonmatching status"
    );
}
