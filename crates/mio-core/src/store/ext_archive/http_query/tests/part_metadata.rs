use super::*;
use std::collections::HashMap;

#[test]
fn metadata_query_globally_orders_live_and_archive_and_keeps_exact_pages_without_body_reads() {
    let (mut store, source, items) =
        super::super::super::proxy_history::tests::fixture(0, false, false);
    let a = live(
        "https://z.test/alpha.HTML?x=.png",
        Some(200),
        20,
        TrafficSource::Proxy,
    );
    let b = live(
        "https://a.test/bravo.png#image",
        Some(404),
        10,
        TrafficSource::Proxy,
    );
    let c = live(
        "https://ignored.test/",
        Some(201),
        15,
        TrafficSource::Repeater,
    );
    for row in [&a, &b, &c] {
        store.put(row).unwrap();
    }
    let before_source: Vec<u8> = store
        .connection
        .query_row(
            "SELECT data FROM ext_source_chunks WHERE source=?1",
            [source.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    store.connection.authorizer(Some(no_bodies));
    let mut q = query(&store);
    q.selected_ids = vec![
        HistoryEntryId::Live(a.summary.id),
        HistoryEntryId::Live(c.summary.id),
        HistoryEntryId::External {
            source_id: source,
            item_id: items[0].id,
        },
    ];
    let full = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(full.total, 6);
    assert!(
        full.selected_rows.is_empty(),
        "selected rows already present in the page are not copied twice"
    );
    assert_eq!(full.rows[0].id(), HistoryEntryId::Live(a.summary.id));
    let statuses: Vec<_> = full
        .rows
        .iter()
        .map(|row| match row {
            HistoryRow::Live(row) => row.status,
            HistoryRow::ExtProxy(row) => row.response.as_ref().and_then(|r| r.status),
        })
        .collect();
    assert_eq!(
        statuses,
        [Some(200), Some(201), Some(201), Some(404), None, None]
    );
    assert_eq!(
        full.selected_matches,
        [
            (q.selected_ids[0], true),
            (q.selected_ids[2], true),
            (q.selected_ids[1], false)
        ]
    );
    assert_eq!(
        full.selected_ranks,
        [
            (q.selected_ids[0], 0),
            (
                q.selected_ids[2],
                full.rows
                    .iter()
                    .position(|row| row.id() == q.selected_ids[2])
                    .unwrap()
            )
        ]
    );
    let mut pages = Vec::new();
    q.limit = 2;
    for offset in [0, 2, 4] {
        q.offset = offset;
        let page = store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap();
        assert_eq!(page.total, full.total);
        assert_eq!(page.selected_ranks, full.selected_ranks);
        let current: HashSet<_> = page.rows.iter().map(HistoryRow::id).collect();
        let off_page: HashSet<_> = page.selected_rows.iter().map(HistoryRow::id).collect();
        let expected: HashSet<_> = full
            .selected_ranks
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| !current.contains(id))
            .collect();
        assert_eq!(off_page, expected);
        assert!(page.selected_rows.iter().all(|row| full.rows.contains(row)));
        pages.extend(page.rows);
    }
    assert_eq!(pages, full.rows);
    q.offset = 0;
    q.limit = 200;
    q.sort = HttpHistorySort {
        column: HttpHistorySortColumn::Url,
        ascending: false,
    };
    let descending = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    let mut urls: Vec<_> = descending
        .rows
        .iter()
        .map(|row| match row {
            HistoryRow::Live(row) => crate::http_history_path_and_query(&row.url),
            HistoryRow::ExtProxy(row) => row
                .request
                .as_ref()
                .and_then(|r| r.request_target.as_deref())
                .map(crate::http_history_path_and_query)
                .unwrap_or_default(),
        })
        .collect();
    let expected = urls.clone();
    urls.sort_by_key(|s| std::cmp::Reverse(s.to_ascii_lowercase()));
    assert_eq!(urls, expected);
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert_eq!(store.read(c.summary.id).unwrap(), c);
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT data FROM ext_source_chunks WHERE source=?1",
                [source.to_string()],
                |r| r.get::<_, Vec<u8>>(0)
            )
            .unwrap(),
        before_source
    );
}

#[test]
fn status_response_extension_and_url_filters_share_count_page_and_selection_semantics() {
    let mut store = ProjectStore::create(None).unwrap();
    let rows = [
        live(
            "https://same.test/a.HTML?next=b.png",
            Some(200),
            1,
            TrafficSource::Proxy,
        ),
        live(
            "https://same.test/b.png#x",
            Some(404),
            2,
            TrafficSource::Proxy,
        ),
        live("https://same.test/c", None, 3, TrafficSource::Proxy),
        live("https://same.test/d.js", Some(101), 4, TrafficSource::Proxy),
    ];
    for row in &rows {
        store.put(row).unwrap();
    }
    store.connection.authorizer(Some(no_bodies));
    let mut q = query(&store);
    q.selected_ids = rows
        .iter()
        .map(|r| HistoryEntryId::Live(r.summary.id))
        .collect();
    q.filter.status_classes = [false, false, true, false];
    let result = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(result.total, 2);
    assert_eq!(
        result
            .selected_matches
            .iter()
            .map(|(_, m)| *m)
            .collect::<Vec<_>>(),
        [true, true, false, false]
    );
    assert_eq!(
        result.selected_ranks,
        [(q.selected_ids[1], 0), (q.selected_ids[2], 1)]
    );
    q.filter.hide_without_response = true;
    assert_eq!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap()
            .total,
        1
    );
    q.filter = HttpHistoryFilter::default();
    q.filter.extensions = HttpExtensionFilter::Only(".html, JS".into());
    assert_eq!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap()
            .total,
        2
    );
    q.filter.extensions = HttpExtensionFilter::Hide("PNG".into());
    assert_eq!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap()
            .total,
        3
    );
    q.url_filter = "A.HTML".into();
    assert_eq!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap()
            .rows[0]
            .id(),
        HistoryEntryId::Live(rows[0].summary.id)
    );
    q.filter.extensions = HttpExtensionFilter::Only("p?g".into());
    assert!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .is_err()
    );
}

#[test]
fn annotation_filters_use_overlay_values_keep_clears_and_reject_unknown_source_instead_of_hiding_it()
 {
    let (store, source, items) =
        super::super::super::proxy_history::tests::fixture(0, false, false);
    let item = &items[0];
    let mut metadata: Value = serde_json::from_slice(&item.metadata_json).unwrap();
    metadata["record"]["mapping"]["value"]["annotations"] = json!({"id":27,"binding":{"source_id":source,"item_id":item.id},"observation":{"state":"null"}});
    metadata["record"]["mapping"]["value"]["highlight"] = json!({"id":13,"binding":{"source_id":source,"item_id":item.id},"observation":{"state":"value","value":3}});
    store
        .connection
        .execute(
            "UPDATE ext_items SET metadata=?2 WHERE id=?1",
            params![item.id.to_string(), serde_json::to_vec(&metadata).unwrap()],
        )
        .unwrap();
    store
        .connection
        .execute(
            "INSERT INTO ext_http_notes VALUES(?1,1,'saved note')",
            [item.id.to_string()],
        )
        .unwrap();
    let mut q = query(&store);
    q.url_filter = "18765".into();
    q.filter.only_with_notes = true;
    q.filter.only_highlighted = true;
    store.connection.authorizer(Some(no_bodies));
    let result = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(
        result.rows[0].id(),
        HistoryEntryId::External {
            source_id: source,
            item_id: item.id
        }
    );
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    store
        .connection
        .execute(
            "INSERT INTO ext_http_highlights VALUES(?1,1,0)",
            [item.id.to_string()],
        )
        .unwrap();
    assert_eq!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap()
            .total,
        0
    );
    q.filter.only_highlighted = false;
    store
        .connection
        .execute(
            "UPDATE ext_http_notes SET text='',revision=2 WHERE item=?1",
            [item.id.to_string()],
        )
        .unwrap();
    assert_eq!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap()
            .total,
        0
    );
    q.url_filter.clear();
    assert!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap_err()
            .to_string()
            .contains("备注筛选")
    );
}

#[test]
fn snapshot_pending_revisions_override_disk_without_writes_and_cancel_and_identity_remain_explicit()
{
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let row = live("http://local.test/old", Some(200), 1, TrafficSource::Proxy);
    store.put(&row).unwrap();
    let mut pending = row.summary.clone();
    pending.revision += 1;
    pending.url = "http://local.test/new".into();
    pending.status = Some(500);
    let mut q = query(&store);
    q.url_filter = "old".into();
    let recovery = HttpHistoryPending {
        summary: pending.clone(),
        first_request_method: row.request.method.clone(),
        first_request_url: row.request.url.clone(),
        first_response_status: row.response.as_ref().map(|response| response.status),
        first_request: Some(row.request.clone()),
        first_response: row.response.clone(),
        request: None,
        response: None,
        listener_port: None,
        capture_metrics: mio_proxy::CapturedFlowMetrics::default(),
    };
    let result = ProjectStore::http_history_snapshot(
        &path,
        &q,
        std::slice::from_ref(&recovery),
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(result.total, 1);
    pending.method = row.request.method.clone();
    pending.url = row.request.url.clone();
    pending.status = row.response.as_ref().map(|response| response.status);
    assert_eq!(result.rows, [HistoryRow::Live(pending)]);
    assert_eq!(store.read(row.summary.id).unwrap(), row);
    q.url_filter = "new".into();
    assert_eq!(
        ProjectStore::http_history_snapshot(&path, &q, &[recovery], &CancellationToken::new())
            .unwrap()
            .total,
        0,
        "new recovery revisions must not change original request filtering"
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        ProjectStore::http_history_snapshot(&path, &q, &[], &cancel)
            .unwrap_err()
            .to_string()
            .contains("取消")
    );
    q.project.id = Uuid::new_v4();
    assert!(
        ProjectStore::http_history_snapshot(&path, &q, &[], &CancellationToken::new())
            .unwrap_err()
            .to_string()
            .contains("ID")
    );
}

#[test]
fn invalid_live_source_and_corrupt_sort_values_are_not_silently_filtered_away() {
    let mut store = ProjectStore::create(None).unwrap();
    let row = live("http://local.test/", Some(200), 1, TrafficSource::Proxy);
    store.put(&row).unwrap();
    let q = query(&store);
    let mut summary = serde_json::to_value(&row.summary).unwrap();
    for (field, value) in [
        ("source", json!("unknown")),
        ("status", json!("200")),
        ("response_bytes", json!("3")),
        ("started_unix_ms", Value::Null),
    ] {
        summary[field] = value;
        store
            .connection
            .execute(
                "UPDATE flows SET summary=?2 WHERE id=?1",
                params![
                    row.summary.id.to_string(),
                    serde_json::to_vec(&summary).unwrap()
                ],
            )
            .unwrap();
        assert!(
            store
                .query_http_history(&q, &CancellationToken::new())
                .is_err(),
            "{field}"
        );
        summary = serde_json::to_value(&row.summary).unwrap();
    }
}

#[test]
fn capture_numbers_remain_fixed_across_filters_and_sorts_and_support_large_limits() {
    let (mut store, source, items) =
        super::super::super::proxy_history::tests::fixture(0, false, false);
    let archive_base = items.iter().map(|i| i.index + 1).max().unwrap_or(0);
    assert!(archive_base > 0);

    let mut first = live("https://cap.test/a", Some(500), 100, TrafficSource::Proxy);
    first.summary.id = Uuid::from_u128((100_u128 << 64) | 1);
    let mut second = live("https://cap.test/b", Some(200), 100, TrafficSource::Proxy);
    second.summary.id = Uuid::from_u128((99_u128 << 64) | 1);
    let mut third = live("https://cap.test/c", Some(404), 200, TrafficSource::Proxy);
    third.summary.id = Uuid::from_u128((98_u128 << 64) | 1);
    for row in [&first, &second, &third] {
        store.put(row).unwrap();
    }

    let cancel = CancellationToken::new();
    let mut q = query(&store);
    q.limit = 100_000;
    q.sort = HttpHistorySort {
        column: HttpHistorySortColumn::Number,
        ascending: false,
    };
    let desc = store.query_http_history(&q, &cancel).unwrap();
    let expected_numbers = HashMap::from([
        (first.summary.id, archive_base + 1),
        (second.summary.id, archive_base + 2),
        (third.summary.id, archive_base + 3),
    ]);
    let desc_live_ids: Vec<_> = desc
        .rows
        .iter()
        .filter_map(|row| row.live().map(|f| f.id))
        .collect();
    assert_eq!(
        desc_live_ids,
        [third.summary.id, second.summary.id, first.summary.id]
    );
    assert_eq!(
        desc.live_numbers.iter().copied().collect::<HashMap<_, _>>(),
        expected_numbers
    );

    q.sort.ascending = true;
    let asc = store.query_http_history(&q, &cancel).unwrap();
    assert_eq!(
        asc.rows[0].id(),
        HistoryEntryId::External {
            source_id: source,
            item_id: items[0].id,
        }
    );
    let asc_live_ids: Vec<_> = asc
        .rows
        .iter()
        .filter_map(|row| row.live().map(|f| f.id))
        .collect();
    assert_eq!(
        asc_live_ids,
        [first.summary.id, second.summary.id, third.summary.id]
    );
    assert_eq!(
        asc.live_numbers.iter().copied().collect::<HashMap<_, _>>(),
        expected_numbers
    );

    q.url_filter = "cap.test".into();
    q.sort = HttpHistorySort {
        column: HttpHistorySortColumn::Status,
        ascending: true,
    };
    let by_status = store.query_http_history(&q, &cancel).unwrap();
    assert_eq!(
        by_status
            .rows
            .iter()
            .filter_map(|row| row.live().map(|f| f.id))
            .collect::<Vec<_>>(),
        [second.summary.id, third.summary.id, first.summary.id]
    );
    assert_eq!(
        by_status
            .live_numbers
            .iter()
            .copied()
            .collect::<HashMap<_, _>>(),
        expected_numbers,
        "filtering and status sorting must never renumber captured live rows"
    );

    q.filter.status_classes = [false, false, true, false];
    q.filter.search_term = "GET /c".into();
    q.selected_ids = vec![HistoryEntryId::Live(first.summary.id)];
    let searched = store.query_http_history(&q, &cancel).unwrap();
    assert_eq!(searched.total, 1);
    assert_eq!(searched.rows[0].id(), HistoryEntryId::Live(third.summary.id));
    assert_eq!(
        searched.live_numbers,
        vec![(third.summary.id, archive_base + 3)]
    );

    q.limit = 0;
    assert!(store.query_http_history(&q, &cancel).is_err());
    q.limit = 100_001;
    assert!(
        store
            .query_http_history(&q, &cancel)
            .unwrap_err()
            .to_string()
            .contains("1–100000")
    );

    store
        .delete_live_proxy_flows(&[second.summary.id])
        .unwrap();
    q.limit = 100_000;
    q.url_filter = "cap.test".into();
    q.filter = HttpHistoryFilter::default();
    q.selected_ids.clear();
    q.sort = HttpHistorySort {
        column: HttpHistorySortColumn::Number,
        ascending: true,
    };
    let after_delete = store.query_http_history(&q, &cancel).unwrap();
    assert_eq!(
        after_delete.live_numbers,
        vec![
            (first.summary.id, archive_base + 1),
            (third.summary.id, archive_base + 3),
        ],
        "deleting middle live flow must never renumber remaining live flows"
    );
    let fourth = live("https://cap.test/d", Some(200), 50, TrafficSource::Proxy);
    store.put(&fourth).unwrap();
    let after_insert = store.query_http_history(&q, &cancel).unwrap();
    assert_eq!(
        after_insert.live_numbers,
        vec![
            (first.summary.id, archive_base + 1),
            (third.summary.id, archive_base + 3),
            (fourth.summary.id, archive_base + 4),
        ]
    );
}

#[test]
fn row_metadata_extracts_all_columns_from_stored_heads_without_reading_bodies() {
    let mut store = ProjectStore::create(None).unwrap();
    let mut original = live(
        "https://10.20.30.40:8443/app/index.html",
        Some(200),
        1_700_000_000_000,
        TrafficSource::Proxy,
    );
    original.summary.elapsed_ms = 48;
    original.request.method = "POST".into();
    original.summary.method = "POST".into();
    original.request.headers = vec![
        crate::Header {
            name: "Content-Type".into(),
            value: b"application/json".to_vec(),
        },
        crate::Header {
            name: "Cookie".into(),
            value: b"sid=alpha; theme=dark".to_vec(),
        },
    ];
    original.request.body = br#"{"query":"mio"}"#.to_vec();
    original.summary.request_bytes = original.request.body.len() as u64;
    let html_body = b"<!doctype html><html><head><title>  MIO &amp; Console  </title></head><body>ok</body></html>";
    original.response = Some(crate::HttpResponse {
        status: 200,
        version: crate::HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![
            crate::Header {
                name: "Content-Type".into(),
                value: b"text/html; charset=utf-8".to_vec(),
            },
            crate::Header {
                name: "Set-Cookie".into(),
                value: b"csrf=token123; Path=/; HttpOnly".to_vec(),
            },
        ],
        trailers: Vec::new(),
        body: html_body.to_vec(),
    });
    original.summary.response_bytes = html_body.len() as u64;
    let metrics = mio_proxy::CapturedFlowMetrics {
        remote_ip: Some("10.20.30.40".parse().unwrap()),
        response_received_ms: Some(48),
        reason_phrase: None,
    };
    store
        .put_captured_flow_with_metrics(&original, None, Some(18080), &metrics)
        .unwrap();

    let mut edited = original.clone();
    edited.summary.revision += 1;
    edited.request.body = b"{}".to_vec();
    edited.summary.request_bytes = 2;
    store
        .put_captured_flow_with_metrics(&edited, None, Some(18080), &metrics)
        .unwrap();

    let mut empty_json_post = live(
        "https://example.com/api/empty",
        Some(204),
        1_700_000_001_000,
        TrafficSource::Proxy,
    );
    empty_json_post.request.method = "POST".into();
    empty_json_post.summary.method = "POST".into();
    empty_json_post.request.headers = vec![crate::Header {
        name: "Content-Type".into(),
        value: b"application/json".to_vec(),
    }];
    empty_json_post.request.body = b"{}".to_vec();
    empty_json_post.summary.request_bytes = 2;
    empty_json_post.response = Some(crate::HttpResponse {
        status: 200,
        version: crate::HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![crate::Header {
            name: "Content-Type".into(),
            value: b"application/json".to_vec(),
        }],
        trailers: Vec::new(),
        body: br#"{"api":"","data":{},"ret":["SUCCESS::ok"],"v":""}"#.to_vec(),
    });
    empty_json_post.summary.status = Some(200);
    empty_json_post.summary.response_bytes =
        empty_json_post.response.as_ref().unwrap().body.len() as u64;
    store.put(&empty_json_post).unwrap();

    let mut duplicate_form_post = live(
        "https://gateway.example.test:443/gw/demo.search.upload/1.0/",
        Some(200),
        1_700_000_002_000,
        TrafficSource::Proxy,
    );
    duplicate_form_post.request.method = "POST".into();
    duplicate_form_post.summary.method = "POST".into();
    duplicate_form_post.request.headers = vec![
        crate::Header {
            name: "content-type".into(),
            value: b"application/x-www-form-urlencoded;charset=UTF-8".to_vec(),
        },
        crate::Header {
            name: "Content-Type".into(),
            value: b"application/x-www-form-urlencoded;charset=UTF-8".to_vec(),
        },
    ];
    duplicate_form_post.request.body =
        b"data=%7B%22count%22%3A%221%22%7D&type=originaljson".to_vec();
    duplicate_form_post.summary.request_bytes = duplicate_form_post.request.body.len() as u64;
    duplicate_form_post.response = Some(crate::HttpResponse {
        status: 200,
        version: crate::HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![crate::Header {
            name: "Content-Type".into(),
            value: b"application/json".to_vec(),
        }],
        trailers: Vec::new(),
        body: br#"{"api":"","data":{},"ret":["SUCCESS::ok"],"v":""}"#.to_vec(),
    });
    duplicate_form_post.summary.response_bytes =
        duplicate_form_post.response.as_ref().unwrap().body.len() as u64;
    store.put(&duplicate_form_post).unwrap();

    store.connection.authorizer(Some(no_bodies));
    let q = query(&store);
    let result = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(result.total, 3);

    let meta = result
        .row_metadata_for(HistoryEntryId::Live(original.summary.id))
        .expect("row metadata present for captured flow");
    assert!(meta.has_params);
    assert!(meta.params_known);
    assert_eq!(meta.params_state(), Some(true));
    assert_eq!(meta.params_cell_text(), "✓");
    assert!(meta.params_error.is_none());
    assert!(meta.edited);
    assert!(meta.has_original_request);
    assert!(!meta.has_original_response);
    assert_eq!(meta.mime_type, "html");
    assert_eq!(meta.title, "MIO & Console");
    assert!(meta.title_error.is_none());
    assert_eq!(meta.cookies, "sid, theme, csrf");
    assert!(meta.cookies_error.is_none());
    assert_eq!(meta.remote_ip, "10.20.30.40");
    assert_eq!(meta.listener_port, Some(18080));
    assert_eq!(meta.response_received_ms, Some(48));
    assert!(meta.time_known);
    assert!(meta.response_length.unwrap() > html_body.len() as u64);

    let empty_meta = result
        .row_metadata_for(HistoryEntryId::Live(empty_json_post.summary.id))
        .expect("row metadata present for empty json post");
    assert!(
        !empty_meta.has_params,
        "POST with empty JSON object {{}} must not be marked as parameterized"
    );
    assert!(empty_meta.params_known);
    assert_eq!(empty_meta.params_state(), Some(false));
    assert_eq!(empty_meta.params_cell_text(), "");
    assert!(empty_meta.params_error.is_none());
    assert!(!empty_meta.edited);
    assert_eq!(empty_meta.mime_type, "JSON");
    assert_eq!(empty_meta.remote_ip, "");
    assert_eq!(empty_meta.listener_port, None);
    assert_eq!(empty_meta.response_received_ms, None);

    let duplicate_form_meta = result
        .row_metadata_for(HistoryEntryId::Live(duplicate_form_post.summary.id))
        .expect("row metadata present for duplicate content-type form post");
    assert!(duplicate_form_meta.has_params);
    assert!(duplicate_form_meta.params_known);
    assert_eq!(duplicate_form_meta.params_state(), Some(true));
    assert_eq!(duplicate_form_meta.params_cell_text(), "✓");
    assert!(duplicate_form_meta.params_error.is_none());
    assert_eq!(duplicate_form_meta.mime_type, "JSON");
}

#[test]
fn row_metadata_preserves_raw_capture_and_reports_explicit_params_title_and_cookie_diagnostics() {
    let mut store = ProjectStore::create(None).unwrap();

    let mut overlimit_params = live(
        "https://diag.test/overlimit",
        Some(200),
        100,
        TrafficSource::Proxy,
    );
    overlimit_params.request.method = "POST".into();
    overlimit_params.summary.method = "POST".into();
    overlimit_params.request.headers = vec![
        crate::Header {
            name: "Content-Type".into(),
            value: b"application/x-www-form-urlencoded".to_vec(),
        },
        crate::Header {
            name: "Cookie".into(),
            value: b"sid=1; bare_without_equals".to_vec(),
        },
    ];
    overlimit_params.request.body = (0..=crate::intruder::MAX_POSITIONS)
        .map(|index| format!("k{index}=v"))
        .collect::<Vec<_>>()
        .join("&")
        .into_bytes();
    overlimit_params.summary.request_bytes = overlimit_params.request.body.len() as u64;
    overlimit_params.response = Some(crate::HttpResponse {
        status: 200,
        version: crate::HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![
            crate::Header {
                name: "Content-Type".into(),
                value: b"text/html".to_vec(),
            },
            crate::Header {
                name: "Content-Encoding".into(),
                value: b"gzip".to_vec(),
            },
        ],
        trailers: Vec::new(),
        body: b"not-a-valid-gzip-stream".to_vec(),
    });
    overlimit_params.summary.response_bytes =
        overlimit_params.response.as_ref().unwrap().body.len() as u64;
    store.put(&overlimit_params).unwrap();
    assert_eq!(store.read(overlimit_params.summary.id).unwrap(), overlimit_params);

    let mut bad_html = live(
        "https://diag.test/bad-html",
        Some(200),
        200,
        TrafficSource::Proxy,
    );
    bad_html.request.headers = vec![crate::Header {
        name: "Cookie".into(),
        value: b"quoted=\"a;b\"; token=ok".to_vec(),
    }];
    bad_html.request.body.clear();
    bad_html.summary.request_bytes = 0;
    bad_html.response = Some(crate::HttpResponse {
        status: 200,
        version: crate::HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![
            crate::Header {
                name: "Content-Type".into(),
                value: b"text/html; charset=utf-8".to_vec(),
            },
            crate::Header {
                name: "Set-Cookie".into(),
                value: b"resp_cookie=val; Path=/; HttpOnly".to_vec(),
            },
        ],
        trailers: Vec::new(),
        body: b"<!doctype html><html><head><title>unclosed".to_vec(),
    });
    bad_html.summary.response_bytes = bad_html.response.as_ref().unwrap().body.len() as u64;
    store.put(&bad_html).unwrap();
    assert_eq!(store.read(bad_html.summary.id).unwrap(), bad_html);

    store.connection.authorizer(Some(no_bodies));
    let q = query(&store);
    let result = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();

    let overlimit_meta = result
        .row_metadata_for(HistoryEntryId::Live(overlimit_params.summary.id))
        .unwrap();
    assert!(!overlimit_meta.has_params);
    assert!(!overlimit_meta.params_known);
    assert_eq!(overlimit_meta.params_state(), None);
    assert_eq!(overlimit_meta.params_cell_text(), "?");
    assert!(overlimit_meta.params_error.as_deref().unwrap().contains("256"));
    assert_eq!(overlimit_meta.title, "");
    assert!(overlimit_meta.title_error.as_deref().unwrap().contains("gzip"));
    assert_eq!(overlimit_meta.cookies, "");
    assert!(overlimit_meta.cookies_error.as_deref().unwrap().contains("等号"));

    let bad_html_meta = result
        .row_metadata_for(HistoryEntryId::Live(bad_html.summary.id))
        .unwrap();
    assert_eq!(bad_html_meta.params_state(), Some(false));
    assert_eq!(bad_html_meta.params_cell_text(), "");
    assert!(bad_html_meta.params_error.is_none());
    assert_eq!(bad_html_meta.cookies, "quoted, token, resp_cookie");
    assert!(bad_html_meta.cookies_error.is_none());
    assert_eq!(bad_html_meta.title, "");
    assert!(bad_html_meta.title_error.as_deref().unwrap().contains("HTML"));

    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);

    // Legacy head without params_known/has_params on a body request must remain unknown rather than faking false.
    let mut legacy_unknown = live(
        "https://diag.test/legacy-unknown",
        Some(200),
        300,
        TrafficSource::Proxy,
    );
    legacy_unknown.request.method = "POST".into();
    legacy_unknown.summary.method = "POST".into();
    legacy_unknown.request.body = b"a=1".to_vec();
    legacy_unknown.summary.request_bytes = 3;
    store.put(&legacy_unknown).unwrap();
    let legacy_req_head = json!({
        "method": "POST",
        "url": "https://diag.test/legacy-unknown",
        "version": crate::HttpVersion::Http11,
        "headers": [],
        "trailers": []
    });
    store
        .connection
        .execute(
            "UPDATE messages SET head=?2 WHERE flow=?1 AND side=0",
            params![
                legacy_unknown.summary.id.to_string(),
                serde_json::to_vec(&legacy_req_head).unwrap()
            ],
        )
        .unwrap();
    store
        .connection
        .execute(
            "DELETE FROM metadata WHERE key=?1",
            [format!("http_history_meta:{}", legacy_unknown.summary.id)],
        )
        .unwrap();
    store.connection.authorizer(Some(no_bodies));
    let legacy_result = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    let legacy_meta = legacy_result
        .row_metadata_for(HistoryEntryId::Live(legacy_unknown.summary.id))
        .unwrap();
    assert_eq!(legacy_meta.params_state(), None);
    assert_eq!(legacy_meta.params_cell_text(), "?");
    assert!(legacy_meta.params_error.is_some());
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);

    // Corrupted http_history_meta:* revision or identity must fast-fail in schema_guard / query.
    let corrupt_meta = json!({
        "flow_id": bad_html.summary.id,
        "revision": 999,
        "metadata": bad_html_meta,
    });
    store
        .connection
        .execute(
            "UPDATE metadata SET value=?2 WHERE key=?1",
            params![
                format!("http_history_meta:{}", bad_html.summary.id),
                serde_json::to_vec(&corrupt_meta).unwrap()
            ],
        )
        .unwrap();
    assert!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap_err()
            .to_string()
            .contains("revision")
    );
}

#[test]
fn host_and_url_sorts_use_normalized_host_origin_and_preserve_raw_request_path_and_query() {
    let mut store = ProjectStore::create(None).unwrap();
    let cases = [
        ("http://[::1]:8080/a//b?x=1", "http://[::1]:8080", "/a//b?x=1"),
        ("http://[::1]:80/root", "http://[::1]", "/root"),
        ("http://alpha.test:80?only_query=1", "http://alpha.test", "/?only_query=1"),
        ("http://alpha.test:9000/custom", "http://alpha.test:9000", "/custom"),
        ("https://beta.test:443/a/../b?k=%2F#frag", "https://beta.test", "/a/../b?k=%2F"),
        ("https://beta.test:8443?z=2", "https://beta.test:8443", "/?z=2"),
    ];
    let mut ids_by_url = HashMap::new();
    for (idx, (url, expected_host, expected_path_and_query)) in cases.iter().enumerate() {
        assert_eq!(super::sql::http_history_host_origin(url), *expected_host);
        assert_eq!(
            crate::http_history_path_and_query(url),
            *expected_path_and_query
        );
        let row = live(url, Some(200), (idx + 1) as i64, TrafficSource::Proxy);
        ids_by_url.insert(*url, HistoryEntryId::Live(row.summary.id));
        store.put(&row).unwrap();
    }

    store.connection.authorizer(Some(no_bodies));
    let mut q = query(&store);
    q.sort = HttpHistorySort {
        column: HttpHistorySortColumn::Host,
        ascending: true,
    };
    let host_asc = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    let actual_hosts: Vec<String> = host_asc
        .rows
        .iter()
        .map(|row| super::sql::http_history_host_origin(&row.live().unwrap().url))
        .collect();
    let mut expected_hosts = actual_hosts.clone();
    expected_hosts.sort_by_key(|h| h.to_ascii_lowercase());
    assert_eq!(actual_hosts, expected_hosts);
    assert_eq!(
        actual_hosts,
        [
            "http://[::1]",
            "http://[::1]:8080",
            "http://alpha.test",
            "http://alpha.test:9000",
            "https://beta.test",
            "https://beta.test:8443",
        ]
    );

    q.sort = HttpHistorySort {
        column: HttpHistorySortColumn::Url,
        ascending: true,
    };
    let url_asc = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    let actual_urls: Vec<String> = url_asc
        .rows
        .iter()
        .map(|row| crate::http_history_path_and_query(&row.live().unwrap().url))
        .collect();
    assert_eq!(
        actual_urls,
        [
            "/?only_query=1",
            "/?z=2",
            "/a/../b?k=%2F",
            "/a//b?x=1",
            "/custom",
            "/root",
        ]
    );
    assert_eq!(
        url_asc.rows[2].id(),
        ids_by_url["https://beta.test:443/a/../b?k=%2F#frag"]
    );
}

#[test]
fn all_eighteen_columns_sort_ascending_and_descending_with_nulls_last_fixed_capture_numbers_and_search_parity()
 {
    let mut store = ProjectStore::create(None).unwrap();
    let project = ProjectKey {
        id: store.id,
        generation: 1,
    };
    let cancel = CancellationToken::new();

    let mut r1 = live(
        "https://alpha.test:443/dir/page.HTML?b=2",
        Some(200),
        1_000,
        TrafficSource::Proxy,
    );
    r1.summary.id = Uuid::from_u128(1);
    r1.request.method = "post".into();
    r1.summary.method = "post".into();
    r1.request.headers = vec![
        crate::Header {
            name: "Content-Type".into(),
            value: b"application/x-www-form-urlencoded".to_vec(),
        },
        crate::Header {
            name: "Cookie".into(),
            value: b"zebra=1".to_vec(),
        },
    ];
    r1.request.body = b"k=v".to_vec();
    r1.summary.request_bytes = 3;
    let r1_body = b"<!doctype html><html><head><title>bravo Title</title></head><body>ok</body></html>";
    r1.response = Some(crate::HttpResponse {
        status: 200,
        version: crate::HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![crate::Header {
            name: "Content-Type".into(),
            value: b"text/html; charset=utf-8".to_vec(),
        }],
        trailers: Vec::new(),
        body: r1_body.to_vec(),
    });
    r1.summary.response_bytes = r1_body.len() as u64;
    let m1 = mio_proxy::CapturedFlowMetrics {
        remote_ip: Some("10.0.0.2".parse().unwrap()),
        response_received_ms: Some(120),
        reason_phrase: None,
    };
    store
        .put_captured_flow_with_metrics(&r1, None, Some(8080), &m1)
        .unwrap();
    let mut r1_edited = r1.clone();
    r1_edited.summary.revision += 1;
    r1_edited.request.body = b"k=edited".to_vec();
    r1_edited.summary.request_bytes = 8;
    store
        .put_captured_flow_with_metrics(&r1_edited, None, Some(8080), &m1)
        .unwrap();
    store
        .set_ext_http_notes(
            project,
            Uuid::nil(),
            r1.summary.id,
            0,
            "charlie\r\nnote",
            &cancel,
        )
        .unwrap();

    let mut r2 = live(
        "http://BETA.test:80/a/script.js",
        Some(404),
        3_000,
        TrafficSource::Proxy,
    );
    r2.summary.id = Uuid::from_u128(2);
    r2.request.method = "GET".into();
    r2.summary.method = "GET".into();
    r2.request.headers = vec![crate::Header {
        name: "Cookie".into(),
        value: b"Alpha=1".to_vec(),
    }];
    r2.request.body.clear();
    r2.summary.request_bytes = 0;
    let r2_body = b"<!doctype html><html><head><title>Alpha Title</title></head><body>missing is long payload....................................................</body></html>";
    r2.response = Some(crate::HttpResponse {
        status: 404,
        version: crate::HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![crate::Header {
            name: "Content-Type".into(),
            value: b"text/html".to_vec(),
        }],
        trailers: Vec::new(),
        body: r2_body.to_vec(),
    });
    r2.summary.response_bytes = r2_body.len() as u64;
    let m2 = mio_proxy::CapturedFlowMetrics {
        remote_ip: Some("10.0.0.1".parse().unwrap()),
        response_received_ms: Some(15),
        reason_phrase: None,
    };
    store
        .put_captured_flow_with_metrics(&r2, None, Some(18080), &m2)
        .unwrap();
    store
        .set_ext_http_notes(
            project,
            Uuid::nil(),
            r2.summary.id,
            0,
            "alpha\tnote",
            &cancel,
        )
        .unwrap();

    let mut r3 = live(
        "http://gamma.test:9090/z/noext",
        Some(204),
        2_000,
        TrafficSource::Proxy,
    );
    r3.summary.id = Uuid::from_u128(3);
    r3.request.method = "DELETE".into();
    r3.summary.method = "DELETE".into();
    r3.request.headers.clear();
    r3.request.body.clear();
    r3.summary.request_bytes = 0;
    r3.response = Some(crate::HttpResponse {
        status: 204,
        version: crate::HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![crate::Header {
            name: "Content-Type".into(),
            value: b"application/json".to_vec(),
        }],
        trailers: Vec::new(),
        body: br#"{"empty":true}"#.to_vec(),
    });
    r3.summary.response_bytes = r3.response.as_ref().unwrap().body.len() as u64;
    let m3 = mio_proxy::CapturedFlowMetrics {
        remote_ip: Some("10.0.0.9".parse().unwrap()),
        response_received_ms: Some(50),
        reason_phrase: None,
    };
    store
        .put_captured_flow_with_metrics(&r3, None, Some(19090), &m3)
        .unwrap();

    let mut r4 = live(
        "http://delta.test:7070/pending/noext",
        None,
        4_000,
        TrafficSource::Proxy,
    );
    r4.summary.id = Uuid::from_u128(4);
    r4.request.method = "PATCH".into();
    r4.summary.method = "PATCH".into();
    r4.request.headers.clear();
    r4.request.body.clear();
    r4.summary.request_bytes = 0;
    store.put(&r4).unwrap();

    let id1 = HistoryEntryId::Live(r1.summary.id);
    let id2 = HistoryEntryId::Live(r2.summary.id);
    let id3 = HistoryEntryId::Live(r3.summary.id);
    let id4 = HistoryEntryId::Live(r4.summary.id);
    let expected_capture_numbers = HashMap::from([
        (r1.summary.id, 1_u64),
        (r2.summary.id, 2_u64),
        (r3.summary.id, 3_u64),
        (r4.summary.id, 4_u64),
    ]);

    store.connection.authorizer(Some(no_bodies));

    let cases: &[(HttpHistorySortColumn, [HistoryEntryId; 4], [HistoryEntryId; 4])] = &[
        (
            HttpHistorySortColumn::Number,
            [id1, id2, id3, id4],
            [id4, id3, id2, id1],
        ),
        (
            HttpHistorySortColumn::Host,
            [id2, id4, id3, id1],
            [id1, id3, id4, id2],
        ),
        (
            HttpHistorySortColumn::Method,
            [id3, id2, id4, id1],
            [id1, id4, id2, id3],
        ),
        (
            HttpHistorySortColumn::Url,
            [id2, id1, id4, id3],
            [id3, id4, id1, id2],
        ),
        (
            HttpHistorySortColumn::Params,
            [id1, id2, id3, id4],
            [id1, id2, id3, id4],
        ),
        (
            HttpHistorySortColumn::Edited,
            [id1, id2, id3, id4],
            [id1, id2, id3, id4],
        ),
        (
            HttpHistorySortColumn::Status,
            [id1, id3, id2, id4],
            [id2, id3, id1, id4],
        ),
        (
            HttpHistorySortColumn::Length,
            [id3, id1, id2, id4],
            [id2, id1, id3, id4],
        ),
        (
            HttpHistorySortColumn::MimeType,
            [id1, id2, id3, id4],
            [id3, id1, id2, id4],
        ),
        (
            HttpHistorySortColumn::Extension,
            [id1, id2, id3, id4],
            [id2, id1, id3, id4],
        ),
        (
            HttpHistorySortColumn::Title,
            [id2, id1, id3, id4],
            [id1, id2, id3, id4],
        ),
        (
            HttpHistorySortColumn::Notes,
            [id2, id1, id3, id4],
            [id1, id2, id3, id4],
        ),
        (
            HttpHistorySortColumn::Tls,
            [id1, id2, id3, id4],
            [id1, id2, id3, id4],
        ),
        (
            HttpHistorySortColumn::Ip,
            [id2, id1, id3, id4],
            [id3, id1, id2, id4],
        ),
        (
            HttpHistorySortColumn::Cookies,
            [id2, id1, id3, id4],
            [id1, id2, id3, id4],
        ),
        (
            HttpHistorySortColumn::Time,
            [id1, id3, id2, id4],
            [id4, id2, id3, id1],
        ),
        (
            HttpHistorySortColumn::ListenerPort,
            [id1, id2, id3, id4],
            [id3, id2, id1, id4],
        ),
        (
            HttpHistorySortColumn::StartResponseTimer,
            [id2, id3, id1, id4],
            [id1, id3, id2, id4],
        ),
    ];

    for (column, expected_asc, expected_desc) in cases {
        for (ascending, expected) in [(true, expected_asc), (false, expected_desc)] {
            let mut q = query(&store);
            q.sort = HttpHistorySort {
                column: *column,
                ascending,
            };
            q.selected_ids = vec![expected[3]];
            let res = store.query_http_history(&q, &cancel).unwrap();
            let ids: Vec<HistoryEntryId> = res.rows.iter().map(HistoryRow::id).collect();
            assert_eq!(
                ids, *expected,
                "column={column:?} ascending={ascending}"
            );
            assert_eq!(res.selected_ranks, vec![(expected[3], 3)]);
            assert_eq!(
                res.live_numbers.iter().copied().collect::<HashMap<_, _>>(),
                expected_capture_numbers,
                "capture numbers must stay fixed for column={column:?} ascending={ascending}"
            );

            q.limit = 1;
            q.offset = 0;
            let first_page = store.query_http_history(&q, &cancel).unwrap();
            assert_eq!(first_page.rows.len(), 1);
            assert_eq!(first_page.rows[0].id(), expected[0]);
            assert_eq!(first_page.selected_ranks, vec![(expected[3], 3)]);
            assert_eq!(first_page.selected_rows.len(), 1);
            assert_eq!(first_page.selected_rows[0].id(), expected[3]);
        }
    }

    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);

    for (column, expected_asc, expected_desc) in cases {
        for (ascending, expected) in [(true, expected_asc), (false, expected_desc)] {
            let mut q = query(&store);
            q.filter.search_term = "HTTP/".into();
            q.sort = HttpHistorySort {
                column: *column,
                ascending,
            };
            let res = store.query_http_history(&q, &cancel).unwrap();
            let ids: Vec<HistoryEntryId> = res.rows.iter().map(HistoryRow::id).collect();
            assert_eq!(
                ids, *expected,
                "search_page parity failed for column={column:?} ascending={ascending}"
            );
        }
    }

    let (mut mixed_store, source, items) =
        super::super::super::proxy_history::tests::fixture(0, true, false);
    let mut live_row = live(
        "https://alpha.test:443/live.html",
        Some(200),
        5_000,
        TrafficSource::Proxy,
    );
    live_row.request.headers = vec![crate::Header {
        name: "Cookie".into(),
        value: b"a_cookie=1".to_vec(),
    }];
    live_row.response = Some(crate::HttpResponse {
        status: 200,
        version: crate::HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![crate::Header {
            name: "Content-Type".into(),
            value: b"text/html".to_vec(),
        }],
        trailers: Vec::new(),
        body: b"<!doctype html><html><head><title>Archive Mixed Title</title></head><body>ok</body></html>".to_vec(),
    });
    live_row.summary.response_bytes = live_row.response.as_ref().unwrap().body.len() as u64;
    mixed_store.put(&live_row).unwrap();
    let live_id = HistoryEntryId::Live(live_row.summary.id);
    let archive_id = HistoryEntryId::External {
        source_id: source,
        item_id: items[0].id,
    };
    for column in [
        HttpHistorySortColumn::Params,
        HttpHistorySortColumn::Edited,
        HttpHistorySortColumn::MimeType,
        HttpHistorySortColumn::Title,
        HttpHistorySortColumn::Cookies,
        HttpHistorySortColumn::Tls,
        HttpHistorySortColumn::Ip,
    ] {
        let mut q = query(&mixed_store);
        q.sort = HttpHistorySort {
            column,
            ascending: true,
        };
        let asc = mixed_store.query_http_history(&q, &cancel).unwrap();
        assert!(asc.rows.iter().any(|row| row.id() == live_id));
        assert!(asc.rows.iter().any(|row| row.id() == archive_id));
        q.sort.ascending = false;
        let desc = mixed_store.query_http_history(&q, &cancel).unwrap();
        assert_eq!(desc.total, asc.total);
    }
}
