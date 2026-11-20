use super::*;

#[test]
fn logger_history_page_orders_over_500_multi_source_flows_and_filters_without_reading_bodies() {
    let (mut store, _, _) = fixture(0, false, false);
    let mut expected = Vec::with_capacity(520);
    for index in 0..520usize {
        let mut record = crate::store::tests::record();
        record.summary.source = match index % 5 {
            0 => TrafficSource::Proxy,
            1 => TrafficSource::Repeater,
            2 => TrafficSource::Intruder,
            3 => TrafficSource::Plugin("alpha-ext".into()),
            _ => TrafficSource::Plugin("beta-ext".into()),
        };
        record.summary.started_unix_ms = 10_000 + (index as i64 / 2);
        record.request.method = if index.is_multiple_of(3) {
            "POST"
        } else {
            "GET"
        }
        .into();
        record.summary.method = record.request.method.clone();
        record.request.url = format!("http://127.0.0.1:19090/logger/item-{index}");
        record.summary.url = record.request.url.clone();
        record.request.body = b"body-only-secret-token".to_vec();
        record.summary.request_bytes = record.request.body.len() as u64;
        if index.is_multiple_of(7) {
            record.response = None;
            record.summary.status = None;
            record.summary.response_bytes = 0;
            record.summary.state = crate::FlowState::Pending;
        } else {
            let status = 200 + (index % 5) as u16 * 100 + (index % 4) as u16;
            let body = b"body-only-secret-token".to_vec();
            record.summary.status = Some(status);
            record.summary.response_bytes = body.len() as u64;
            record.summary.state = crate::FlowState::Complete;
            record.response = Some(mio_plugin_api::HttpResponse {
                version: mio_plugin_api::HttpVersion::Http11,
                status,
                reason_phrase: None,
                headers: Vec::new(),
                body,
                trailers: Vec::new(),
            });
        }
        store.put(&record).unwrap();
        expected.push(record.summary);
    }
    expected.sort_by(|left, right| {
        right
            .started_unix_ms
            .cmp(&left.started_unix_ms)
            .then_with(|| left.id.to_string().cmp(&right.id.to_string()))
    });
    store
        .connection
        .pragma_update(None, "query_only", true)
        .unwrap();
    store
        .connection
        .authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Read {
                table_name: "messages" | "ext_items" | "ext_messages" | "ext_source_chunks",
                ..
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }));
    let cancel = CancellationToken::new();
    assert!(
        store
            .logger_history_page(0, 201, "", None, &cancel)
            .is_err()
    );
    let mut paged = Vec::new();
    while paged.len() < expected.len() {
        let (total, rows) = store
            .logger_history_page(paged.len(), 200, "", None, &cancel)
            .unwrap();
        assert_eq!(total, expected.len());
        assert!(!rows.is_empty() && rows.len() <= 200);
        paged.extend(rows);
    }
    assert_eq!(paged, expected);
    assert!(
        store
            .logger_history_page(expected.len(), 200, "", None, &cancel)
            .unwrap()
            .1
            .is_empty()
    );

    for source_filter in [
        Some(TrafficSource::Proxy),
        Some(TrafficSource::Repeater),
        Some(TrafficSource::Intruder),
        Some(TrafficSource::Plugin(String::new())),
        Some(TrafficSource::Plugin("alpha-ext".into())),
    ] {
        let filtered_expected: Vec<_> = expected
            .iter()
            .filter(|row| match &source_filter {
                Some(TrafficSource::Plugin(name)) if name.is_empty() => {
                    matches!(row.source, TrafficSource::Plugin(_))
                }
                Some(source) => &row.source == source,
                None => true,
            })
            .filter(|row| {
                row.url.to_lowercase().contains("item-1")
                    || row.method.to_lowercase().contains("item-1")
                    || row.status.is_some_and(|s| s.to_string().contains("item-1"))
            })
            .cloned()
            .collect();
        let (total, rows) = store
            .logger_history_page(0, 200, "ItEm-1", source_filter.as_ref(), &cancel)
            .unwrap();
        assert_eq!(total, filtered_expected.len());
        assert_eq!(rows, filtered_expected);
    }
    let status_expected: Vec<_> = expected
        .iter()
        .filter(|row| {
            matches!(row.source, TrafficSource::Plugin(_))
                && (row.url.contains("403")
                    || row.method.contains("403")
                    || row.status == Some(403))
        })
        .cloned()
        .collect();
    assert_eq!(
        store
            .logger_history_page(
                0,
                200,
                "403",
                Some(&TrafficSource::Plugin(String::new())),
                &cancel,
            )
            .unwrap(),
        (status_expected.len(), status_expected)
    );
    assert_eq!(
        store
            .logger_history_page(0, 200, "body-only-secret-token", None, &cancel)
            .unwrap(),
        (0, Vec::new())
    );
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    store.close().unwrap();
}

#[test]
fn logger_history_short_pages_on_1mib_budget_and_never_hides_corrupt_records_behind_filters() {
    let mut store = ProjectStore::create(None).unwrap();
    let cancel = CancellationToken::new();
    for index in 0..3usize {
        let mut record = crate::store::tests::record();
        record.summary.source = TrafficSource::Repeater;
        record.summary.started_unix_ms = 2_000 + index as i64;
        record.request.url = format!("http://127.0.0.1:19091/{index}/{}", "u".repeat(420 * 1024));
        record.summary.url = record.request.url.clone();
        store.put(&record).unwrap();
    }
    let (total, first) = store
        .logger_history_page(0, 200, "", Some(&TrafficSource::Repeater), &cancel)
        .unwrap();
    assert_eq!(total, 3);
    assert_eq!(first.len(), 2);
    let (next_total, second) = store
        .logger_history_page(
            first.len(),
            200,
            "",
            Some(&TrafficSource::Repeater),
            &cancel,
        )
        .unwrap();
    assert_eq!(next_total, 3);
    assert_eq!(second.len(), 1);
    assert_ne!(first[0].id, second[0].id);
    assert_ne!(first[1].id, second[0].id);

    for mode in [
        "unknown-source",
        "empty-plugin",
        "duplicate-key",
        "column-url-mismatch",
        "column-method-mismatch",
        "column-started-mismatch",
        "column-state-mismatch",
        "bad-status",
    ] {
        let mut store = ProjectStore::create(None).unwrap();
        let mut valid = crate::store::tests::record();
        valid.summary.source = TrafficSource::Proxy;
        valid.request.url = "http://127.0.0.1:19091/keep".into();
        valid.summary.url = valid.request.url.clone();
        store.put(&valid).unwrap();

        let mut corrupt = crate::store::tests::record();
        corrupt.summary.source = TrafficSource::Repeater;
        corrupt.request.url = "http://127.0.0.1:19091/excluded".into();
        corrupt.summary.url = corrupt.request.url.clone();
        store.put(&corrupt).unwrap();
        let id = corrupt.summary.id.to_string();
        let mut json = serde_json::to_value(&corrupt.summary).unwrap();
        match mode {
            "unknown-source" => json["source"] = json!("UnknownTool"),
            "empty-plugin" => json["source"] = json!({"Plugin": "   "}),
            "bad-status" => json["status"] = json!(99),
            _ => {}
        }
        let bytes = if mode == "duplicate-key" {
            let mut raw = b"{\"source\":\"Repeater\",".to_vec();
            raw.extend_from_slice(&serde_json::to_vec(&json).unwrap()[1..]);
            raw
        } else {
            serde_json::to_vec(&json).unwrap()
        };
        store
            .connection
            .execute(
                "UPDATE flows SET summary=?2 WHERE id=?1",
                params![id, bytes],
            )
            .unwrap();
        match mode {
            "column-url-mismatch" => {
                store
                    .connection
                    .execute(
                        "UPDATE flows SET url='http://127.0.0.1:19091/tampered' WHERE id=?1",
                        [corrupt.summary.id.to_string()],
                    )
                    .unwrap();
            }
            "column-method-mismatch" => {
                store
                    .connection
                    .execute(
                        "UPDATE flows SET method='DELETE' WHERE id=?1",
                        [corrupt.summary.id.to_string()],
                    )
                    .unwrap();
            }
            "column-started-mismatch" => {
                store
                    .connection
                    .execute(
                        "UPDATE flows SET started=started+99 WHERE id=?1",
                        [corrupt.summary.id.to_string()],
                    )
                    .unwrap();
            }
            "column-state-mismatch" => {
                store
                    .connection
                    .execute(
                        "UPDATE flows SET state='pending' WHERE id=?1",
                        [corrupt.summary.id.to_string()],
                    )
                    .unwrap();
            }
            _ => {}
        }
        assert!(
            store
                .logger_history_page(0, 50, "keep", Some(&TrafficSource::Proxy), &cancel)
                .is_err(),
            "filter/source must not hide corrupt record: {mode}"
        );
    }

    let pre_cancelled = CancellationToken::new();
    pre_cancelled.cancel();
    let err = store
        .logger_history_page(0, 50, "", None, &pre_cancelled)
        .unwrap_err();
    assert!(err.to_string().contains("已取消"));
}
