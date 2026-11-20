use super::*;

fn duplicated_status(bytes: &[u8], first: u16) -> Vec<u8> {
    let json = std::str::from_utf8(bytes).unwrap();
    let original = format!("\"status\":{first}");
    assert_eq!(json.matches(&original).count(), 1);
    json.replacen(&original, &format!("{original},\"status\":404"), 1)
        .into_bytes()
}

#[test]
fn duplicate_live_status_is_rejected_even_when_its_first_value_is_filtered_out() {
    let mut store = ProjectStore::create(None).unwrap();
    let record = live(
        "http://local.test/duplicate",
        Some(200),
        1,
        TrafficSource::Proxy,
    );
    store.put(&record).unwrap();
    let mut q = query(&store);
    q.filter.hide_without_response = true;
    q.filter.status_classes = [false, false, true, false];
    assert_eq!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap()
            .total,
        0,
        "the uncorrupted 2xx row is legitimately excluded from a 4xx filter"
    );
    let damaged = duplicated_status(&serde_json::to_vec(&record.summary).unwrap(), 200);
    assert!(serde_json::from_slice::<FlowSummary>(&damaged).is_err());
    store
        .connection
        .execute(
            "UPDATE flows SET summary=?2 WHERE id=?1",
            params![record.summary.id.to_string(), &damaged],
        )
        .unwrap();
    store.connection.authorizer(Some(no_bodies));
    assert!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .is_err(),
        "a duplicate status must be rejected before its first value can silently exclude the row"
    );
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT summary FROM flows WHERE id=?1",
                [record.summary.id.to_string()],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .unwrap(),
        damaged,
        "rejecting a damaged summary must not rewrite it"
    );
}

#[test]
fn duplicate_archive_status_is_rejected_even_when_its_first_value_is_filtered_out() {
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
    q.filter.status_classes = [false, false, true, false];
    assert_eq!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap()
            .total,
        0,
        "the selected uncorrupted 201 response is legitimately excluded"
    );
    let damaged = duplicated_status(&summary, 201);
    assert!(serde_json::from_slice::<crate::ExtMessageSummary>(&damaged).is_err());
    store
        .connection
        .execute(
            "UPDATE ext_messages SET summary=?2 WHERE id=?1",
            params![message_id, &damaged],
        )
        .unwrap();
    store.connection.authorizer(Some(no_bodies));
    assert!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .is_err(),
        "an archive response with duplicate status must not disappear before strict summary decoding"
    );
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
        damaged
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
        "filter validation must leave the original archive unchanged"
    );
}

#[test]
fn duplicate_highlight_value_is_rejected_before_sql_and_preview_can_disagree() {
    let (store, source, items) =
        super::super::super::proxy_history::tests::fixture(0, false, false);
    let item = &items[0];
    let mut metadata: Value = serde_json::from_slice(&item.metadata_json).unwrap();
    let highlight = json!({
        "id": 13,
        "binding": {"source_id": source, "item_id": item.id},
        "observation": {"state": "value", "value": 3},
    });
    metadata["record"]["mapping"]["value"]["highlight"] = highlight.clone();
    let valid = serde_json::to_vec(&metadata).unwrap();
    store
        .connection
        .execute(
            "UPDATE ext_items SET metadata=?2 WHERE id=?1",
            params![item.id.to_string(), &valid],
        )
        .unwrap();
    let mut q = query(&store);
    q.url_filter = "18765".into();
    q.filter.only_highlighted = true;
    assert_eq!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap()
            .total,
        1,
        "a valid yellow source record matches the highlight filter"
    );

    let highlight_json = serde_json::to_string(&highlight).unwrap();
    assert_eq!(highlight_json.matches("\"value\":3").count(), 1);
    let conflicting = highlight_json.replacen("\"value\":3", "\"value\":0,\"value\":3", 1);
    let metadata_json = std::str::from_utf8(&valid).unwrap();
    assert_eq!(metadata_json.matches(&highlight_json).count(), 1);
    let damaged = metadata_json
        .replacen(&highlight_json, &conflicting, 1)
        .into_bytes();
    let parsed: Value = serde_json::from_slice(&damaged).unwrap();
    assert_eq!(
        parsed["record"]["mapping"]["value"]["highlight"]["observation"]["value"],
        3
    );
    let sqlite_value: u8 = store.connection.query_row(
        "SELECT json_extract(CAST(?1 AS TEXT),'$.record.mapping.value.highlight.observation.value')",
        [&damaged],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(
        sqlite_value, 0,
        "the same duplicated metadata has conflicting interpretations"
    );
    let archive: Vec<u8> = store
        .connection
        .query_row(
            "SELECT data FROM ext_source_chunks WHERE source=?1",
            [source.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE ext_items SET metadata=?2 WHERE id=?1",
            params![item.id.to_string(), &damaged],
        )
        .unwrap();
    store.connection.authorizer(Some(no_bodies));
    assert!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .is_err(),
        "duplicate annotation values must not pass preview validation and disappear in SQL filtering"
    );
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT metadata FROM ext_items WHERE id=?1",
                [item.id.to_string()],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .unwrap(),
        damaged
    );
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM ext_http_highlights", [], |row| row
                .get::<_, usize>(
                0
            ),)
            .unwrap(),
        0
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
        archive
    );
}
