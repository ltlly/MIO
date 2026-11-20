use super::*;

#[test]
fn proxy_history_isolates_live_sources_without_changing_counts_pages_or_saved_records() {
    use crate::{HistoryEntryId, TrafficSource};

    let (mut store, source, items) = fixture(0, false, false);
    let archive_rows = proxy_page(&store, 0, 64, "").unwrap().1;
    let legacy_archive_rows = store.history_page(0, 64, "").unwrap().1;
    let mut records = Vec::new();
    for (index, traffic_source) in [
        TrafficSource::Proxy,
        TrafficSource::Repeater,
        TrafficSource::Intruder,
        TrafficSource::Plugin("controlled-plugin".into()),
        TrafficSource::Proxy,
    ]
    .into_iter()
    .enumerate()
    {
        let mut record = crate::store::tests::record();
        record.summary.source = traffic_source;
        record.summary.started_unix_ms = 1000 + index as i64;
        record.request.url = format!(
            "http://127.0.0.1:19001/origin-match/{}-{index}",
            if record.summary.source == TrafficSource::Proxy {
                "proxy"
            } else {
                "foreign-only"
            }
        );
        record.summary.url = record.request.url.clone();
        record.request.method = if index == 0 { "GET" } else { "POST" }.into();
        record.summary.method = record.request.method.clone();
        store.put(&record).unwrap();
        records.push(record);
    }
    let expected = [
        vec![
            HistoryRow::Live(records[4].summary.clone()),
            HistoryRow::Live(records[0].summary.clone()),
        ],
        archive_rows,
    ]
    .concat();
    let all_sources = [
        records
            .iter()
            .rev()
            .map(|record| HistoryRow::Live(record.summary.clone()))
            .collect::<Vec<_>>(),
        legacy_archive_rows,
    ]
    .concat();
    let unchanged_archive: Vec<_> = items
        .iter()
        .map(|item| store.ext_item(item.id).unwrap().metadata_json)
        .collect();
    let report = store.ext_report(source).unwrap();
    let source_bytes = store
        .read_source_range(source, 0, report.source_bytes as usize)
        .unwrap();
    store
        .connection
        .pragma_update(None, "query_only", true)
        .unwrap();
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
    assert_eq!(
        proxy_page(&store, 0, 64, "").unwrap(),
        (expected.len(), expected.clone()),
        "Proxy HTTP history must not include Repeater, Intruder or plugin rows"
    );
    for offset in 0..=expected.len() {
        let (total, page) = proxy_page(&store, offset, 2, "").unwrap();
        assert_eq!(total, expected.len());
        assert_eq!(page, expected[offset..expected.len().min(offset + 2)]);
    }
    assert_eq!(
        proxy_page(&store, 0, 64, "ORIGIN-match").unwrap(),
        (2, expected[..2].to_vec())
    );
    assert_eq!(
        proxy_page(&store, 0, 64, "foreign-only").unwrap(),
        (0, vec![])
    );
    assert_eq!(proxy_page(&store, 0, 64, "PoSt").unwrap().0, 3);
    assert_eq!(
        proxy_page(&store, 1, 1, "origin-match").unwrap().1[0].id(),
        HistoryEntryId::Live(records[0].summary.id)
    );
    assert_eq!(
        store.history_page(0, 64, "").unwrap(),
        (all_sources.len(), all_sources.clone()),
        "legacy project history must retain every tool's flow and the External archive"
    );
    for offset in 0..=all_sources.len() {
        let (total, page) = store.history_page(offset, 2, "").unwrap();
        assert_eq!(total, all_sources.len());
        assert_eq!(page, all_sources[offset..all_sources.len().min(offset + 2)]);
    }
    assert_eq!(store.history_page(0, 64, "ORIGIN-match").unwrap().0, 5);
    assert_eq!(store.history_page(0, 64, "PoSt").unwrap().0, 6);
    assert_eq!(
        store.history_page(0, 64, "foreign-only").unwrap(),
        (
            3,
            records[1..4]
                .iter()
                .rev()
                .map(|record| HistoryRow::Live(record.summary.clone()))
                .collect()
        )
    );
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert_eq!(store.page(0, 64, "").unwrap().0, records.len());
    for record in &records {
        assert_eq!(store.read(record.summary.id).unwrap(), *record);
    }
    assert_eq!(store.ext_report(source).unwrap(), report);
    assert_eq!(
        store
            .read_source_range(source, 0, source_bytes.len())
            .unwrap(),
        source_bytes
    );
    for (item, metadata) in items.iter().zip(&unchanged_archive) {
        assert_eq!(store.ext_item(item.id).unwrap().metadata_json, *metadata);
    }
    store
        .connection
        .pragma_update(None, "query_only", false)
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("origin-history.mio");
    let copy = directory.path().join("origin-copy.mio");
    store.save(Some(&original)).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    for path in [original, copy] {
        let reopened = ProjectStore::open(&path).unwrap();
        assert_eq!(
            proxy_page(&reopened, 0, 64, "").unwrap(),
            (expected.len(), expected.clone())
        );
        assert_eq!(
            reopened.history_page(0, 64, "").unwrap(),
            (all_sources.len(), all_sources.clone())
        );
        assert_eq!(reopened.page(0, 64, "").unwrap().0, records.len());
        for record in &records {
            assert_eq!(reopened.read(record.summary.id).unwrap(), *record);
        }
        assert_eq!(reopened.ext_report(source).unwrap(), report);
        assert_eq!(
            reopened
                .read_source_range(source, 0, source_bytes.len())
                .unwrap(),
            source_bytes
        );
        for (item, metadata) in items.iter().zip(&unchanged_archive) {
            assert_eq!(
                reopened.ext_item(item.id).unwrap().metadata_json,
                *metadata
            );
        }
        reopened.close().unwrap();
    }
}

#[test]
fn proxy_history_does_not_silently_exclude_unreadable_or_unknown_live_sources() {
    for mode in [
        "json",
        "text",
        "missing",
        "null",
        "unknown",
        "plugin-type",
        "plugin-extra",
        "duplicate-source",
    ] {
        let mut store = ProjectStore::create(None).unwrap();
        let record = crate::store::tests::record();
        store.put(&record).unwrap();
        let mut summary = serde_json::to_value(&record.summary).unwrap();
        match mode {
            "missing" => {
                summary.as_object_mut().unwrap().remove("source");
            }
            "null" => summary["source"] = Value::Null,
            "unknown" => summary["source"] = json!("UnrecognizedTool"),
            "plugin-type" => summary["source"] = json!({"Plugin": 9}),
            "plugin-extra" => {
                summary["source"] = json!({"Plugin": "controlled-plugin", "Repeater": null})
            }
            _ => {}
        }
        let bytes = match mode {
            "json" => b"{".to_vec(),
            "duplicate-source" => {
                let mut bytes = b"{\"source\":\"Repeater\",".to_vec();
                bytes.extend_from_slice(&serde_json::to_vec(&summary).unwrap()[1..]);
                bytes
            }
            _ => serde_json::to_vec(&summary).unwrap(),
        };
        let sql = if mode == "text" {
            "UPDATE flows SET summary=CAST(?2 AS TEXT) WHERE id=?1"
        } else {
            "UPDATE flows SET summary=?2 WHERE id=?1"
        };
        store
            .connection
            .execute(sql, params![record.summary.id.to_string(), bytes])
            .unwrap();
        store
            .connection
            .pragma_update(None, "query_only", true)
            .unwrap();
        assert!(proxy_page(&store, 0, 64, "").is_err(), "{mode}");
        assert!(store.history_page(0, 64, "").is_err(), "legacy: {mode}");
        let after: Vec<u8> = store
            .connection
            .query_row(
                "SELECT CAST(summary AS BLOB) FROM flows WHERE id=?1",
                [record.summary.id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            after, bytes,
            "source classification must be read-only: {mode}"
        );
    }
}

#[test]
fn summary_byte_budget_returns_short_pages_without_skipping_entries() {
    let (store, _, _) = fixture(4, false, false);
    let mut query = store
        .connection
        .prepare("SELECT id,summary FROM ext_messages")
        .unwrap();
    let summaries: Vec<(String, Vec<u8>)> = query
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    drop(query);
    for (id, bytes) in summaries {
        let mut summary: ExtMessageSummary = serde_json::from_slice(&bytes).unwrap();
        summary.diagnostic = Some("x".repeat(700 * 1024));
        store
            .connection
            .execute(
                "UPDATE ext_messages SET summary=?2 WHERE id=?1",
                params![id, serde_json::to_vec(&summary).unwrap()],
            )
            .unwrap();
    }
    let (total, first) = store.history_page(0, 64, "").unwrap();
    assert_eq!(total, 8);
    assert_eq!(first.len(), 2);
    let mut collected = first.iter().map(HistoryRow::id).collect::<Vec<_>>();
    while collected.len() < total {
        let (next_total, page) = store.history_page(collected.len(), 64, "").unwrap();
        assert_eq!(next_total, total);
        assert!(!page.is_empty());
        for row in page {
            assert!(!collected.contains(&row.id()));
            collected.push(row.id());
        }
    }
    assert_eq!(collected.len(), total);
}

#[test]
fn only_complete_schema_229_sources_enter_proxy_history() {
    for (schema, complete, expect_err) in
        [(227, true, false), (228, true, true), (229, false, false)]
    {
        let (store, source, _) = fixture(0, false, false);
        let mut report = read_report(&store.connection, source).unwrap();
        report.schema_floor = schema;
        report.schema_revision = schema;
        report.complete_archive = complete;
        store
            .connection
            .execute(
                "UPDATE ext_sources SET complete=?2,report=?3 WHERE id=?1",
                params![
                    source.to_string(),
                    complete,
                    serde_json::to_vec(&report).unwrap()
                ],
            )
            .unwrap();
        if expect_err {
            assert!(store.history_page(0, 64, "").is_err());
        } else {
            assert_eq!(store.history_page(0, 64, "").unwrap(), (0, Vec::new()));
        }
    }
}

#[test]
fn proxy_draft_uses_bound_service_and_original_raw_without_an_archived_response() {
    let (store, source, items) = fixture(0, false, false);
    let (_, rows) = store.history_page(0, 64, "18765").unwrap();
    let ExtHistoryRow {
        request: Some(message),
        ..
    } = archived(&rows[0])
    else {
        panic!("missing request")
    };
    let workspace = store.workspace().unwrap();
    let tab = store
        .ext_repeater_tab(
            source,
            items[0].id,
            message.id,
            &workspace,
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(tab.draft.target, "http://127.0.0.1:18765");
    assert_eq!(tab.draft.raw, REQUEST);
    assert_eq!(tab.draft.version, HttpVersion::Http11);
    assert!(tab.archive_response.is_none() && tab.last_flow.is_none());
}
