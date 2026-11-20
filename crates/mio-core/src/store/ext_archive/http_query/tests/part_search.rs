use super::*;

#[test]
fn length_sort_globally_orders_live_and_archive_and_keeps_null_responses_last() {
    let (mut store, source, items) =
        super::super::super::proxy_history::tests::fixture(0, false, false);
    let mut short = live(
        "https://live.test/short",
        Some(200),
        10,
        TrafficSource::Proxy,
    );
    short.response.as_mut().unwrap().body = b"12".to_vec();
    short.summary.response_bytes = 2;
    let mut long = live(
        "https://live.test/long",
        Some(200),
        20,
        TrafficSource::Proxy,
    );
    long.response.as_mut().unwrap().body = vec![b'x'; 250];
    long.summary.response_bytes = 250;
    let no_response = live("https://live.test/pending", None, 30, TrafficSource::Proxy);
    for row in [&short, &long, &no_response] {
        store.put(row).unwrap();
    }
    store.connection.authorizer(Some(no_bodies));

    let mut q = query(&store);
    q.sort = HttpHistorySort {
        column: HttpHistorySortColumn::Length,
        ascending: true,
    };
    let ascending = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    let lengths_asc: Vec<Option<u64>> = ascending
        .rows
        .iter()
        .map(|row| ascending.row_metadata_for(row.id()).and_then(|m| m.response_length))
        .collect();
    assert_eq!(
        lengths_asc,
        [
            Some(21),
            Some(139),
            Some(139),
            Some(269),
            None,
            None,
            None
        ]
    );
    assert_eq!(
        ascending.rows[0].id(),
        HistoryEntryId::Live(short.summary.id)
    );
    assert_eq!(
        ascending.rows[3].id(),
        HistoryEntryId::Live(long.summary.id)
    );
    let mut null_ids = [items[3].id, items[4].id];
    null_ids.sort_by_key(|id| id.to_string());
    assert_eq!(
        &ascending.rows[4..]
            .iter()
            .map(HistoryRow::id)
            .collect::<Vec<_>>(),
        &[
            HistoryEntryId::External {
                source_id: source,
                item_id: null_ids[0]
            },
            HistoryEntryId::External {
                source_id: source,
                item_id: null_ids[1]
            },
            HistoryEntryId::Live(no_response.summary.id),
        ]
    );

    q.sort.ascending = false;
    let descending = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    let lengths_desc: Vec<Option<u64>> = descending
        .rows
        .iter()
        .map(|row| descending.row_metadata_for(row.id()).and_then(|m| m.response_length))
        .collect();
    assert_eq!(
        lengths_desc,
        [
            Some(269),
            Some(139),
            Some(139),
            Some(21),
            None,
            None,
            None
        ]
    );
}

#[test]
fn length_sort_and_script_response_length_use_original_full_response_and_sse_final_bytes_and_legacy_fallback()
 {
    let mut store = ProjectStore::create(None).unwrap();

    let mut header_heavy_orig = live(
        "https://length.test/header-heavy",
        Some(200),
        10,
        TrafficSource::Proxy,
    );
    header_heavy_orig.response.as_mut().unwrap().headers = vec![crate::Header {
        name: "X-Big-Header".into(),
        value: vec![b'H'; 786],
    }];
    header_heavy_orig.response.as_mut().unwrap().body = b"0123456789".to_vec();
    header_heavy_orig.summary.response_bytes = 10;
    store.put(&header_heavy_orig).unwrap();

    let mut header_heavy_edited = header_heavy_orig.clone();
    header_heavy_edited.summary.revision += 1;
    header_heavy_edited.response.as_mut().unwrap().headers.clear();
    header_heavy_edited.response.as_mut().unwrap().body = b"1".to_vec();
    header_heavy_edited.summary.response_bytes = 1;
    store.put(&header_heavy_edited).unwrap();

    let mut body_only = live(
        "https://length.test/body-only",
        Some(200),
        20,
        TrafficSource::Proxy,
    );
    body_only.response.as_mut().unwrap().headers.clear();
    body_only.response.as_mut().unwrap().body = vec![b'B'; 50];
    body_only.summary.response_bytes = 50;
    store.put(&body_only).unwrap();

    // 2. SSE streamed response: initial empty body, then completed with 120 bytes of events.
    let mut sse_flow = live(
        "https://length.test/sse",
        Some(200),
        30,
        TrafficSource::Proxy,
    );
    let sse_headers = vec![crate::Header {
        name: "Content-Type".into(),
        value: b"text/event-stream".to_vec(),
    }];
    sse_flow.response.as_mut().unwrap().headers = sse_headers.clone();
    sse_flow.response.as_mut().unwrap().body.clear();
    sse_flow.summary.response_bytes = 0;
    sse_flow.summary.state = FlowState::Pending;
    let stream_initial = mio_proxy::StreamedResponse {
        original_status: 200,
        original_version: crate::HttpVersion::Http11,
        original_headers: sse_headers.clone(),
        body_complete: false,
    };
    store
        .put_capture(&sse_flow, Some(&stream_initial))
        .unwrap();

    sse_flow.summary.revision += 1;
    sse_flow.summary.state = FlowState::Complete;
    let sse_events = vec![b'e'; 120];
    sse_flow.response.as_mut().unwrap().body = sse_events.clone();
    sse_flow.summary.response_bytes = sse_events.len() as u64;
    let stream_done = mio_proxy::StreamedResponse {
        body_complete: true,
        ..stream_initial
    };
    store.put_capture(&sse_flow, Some(&stream_done)).unwrap();

    // 3. Legacy database compatibility: response head without `body_bytes` and without `http_history_meta:*`.
    let mut legacy_flow = live(
        "https://length.test/legacy",
        Some(200),
        40,
        TrafficSource::Proxy,
    );
    legacy_flow.response.as_mut().unwrap().headers = vec![crate::Header {
        name: "X-Legacy".into(),
        value: b"yes".to_vec(),
    }];
    legacy_flow.response.as_mut().unwrap().body = vec![b'L'; 30];
    legacy_flow.summary.response_bytes = 30;
    store.put(&legacy_flow).unwrap();
    let legacy_head_without_body_bytes = json!({
        "status": 200,
        "version": crate::HttpVersion::Http11,
        "headers": legacy_flow.response.as_ref().unwrap().headers,
        "trailers": []
    });
    store
        .connection
        .execute(
            "UPDATE messages SET head=?2 WHERE flow=?1 AND side=1",
            params![
                legacy_flow.summary.id.to_string(),
                serde_json::to_vec(&legacy_head_without_body_bytes).unwrap()
            ],
        )
        .unwrap();
    store
        .connection
        .execute(
            "DELETE FROM metadata WHERE key=?1",
            [format!("http_history_meta:{}", legacy_flow.summary.id)],
        )
        .unwrap();

    // 4. Pending / no-response flow must have NULL length and sort last in both directions.
    let pending_flow = live(
        "https://length.test/no-response",
        None,
        50,
        TrafficSource::Proxy,
    );
    store.put(&pending_flow).unwrap();

    let expected_header_heavy_len = crate::http::intruder_response_len(
        crate::HttpVersion::Http11,
        200,
        None,
        &header_heavy_orig.response.as_ref().unwrap().headers,
        10,
    );
    let expected_body_only_len = crate::http::intruder_response_len(
        crate::HttpVersion::Http11,
        200,
        None,
        &[],
        50,
    );
    let expected_sse_len = crate::http::intruder_response_len(
        crate::HttpVersion::Http11,
        200,
        None,
        &sse_headers,
        120,
    );
    let expected_legacy_len = crate::http::intruder_response_len(
        crate::HttpVersion::Http11,
        200,
        None,
        &legacy_flow.response.as_ref().unwrap().headers,
        30,
    );
    assert_eq!(expected_body_only_len, 69);
    assert_eq!(expected_legacy_len, 64);
    assert_eq!(expected_sse_len, 172);
    assert_eq!(expected_header_heavy_len, 831);

    store.connection.authorizer(Some(no_bodies));
    let mut q = query(&store);
    q.sort = HttpHistorySort {
        column: HttpHistorySortColumn::Length,
        ascending: true,
    };
    let asc = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(
        asc.rows.iter().map(HistoryRow::id).collect::<Vec<_>>(),
        [
            HistoryEntryId::Live(legacy_flow.summary.id),
            HistoryEntryId::Live(body_only.summary.id),
            HistoryEntryId::Live(sse_flow.summary.id),
            HistoryEntryId::Live(header_heavy_orig.summary.id),
            HistoryEntryId::Live(pending_flow.summary.id),
        ]
    );
    for (flow_id, expected_len) in [
        (legacy_flow.summary.id, Some(expected_legacy_len)),
        (body_only.summary.id, Some(expected_body_only_len)),
        (sse_flow.summary.id, Some(expected_sse_len)),
        (header_heavy_orig.summary.id, Some(expected_header_heavy_len)),
        (pending_flow.summary.id, None),
    ] {
        assert_eq!(
            asc.row_metadata_for(HistoryEntryId::Live(flow_id))
                .unwrap()
                .response_length,
            expected_len
        );
    }

    q.sort.ascending = false;
    let desc = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(
        desc.rows.iter().map(HistoryRow::id).collect::<Vec<_>>(),
        [
            HistoryEntryId::Live(header_heavy_orig.summary.id),
            HistoryEntryId::Live(sse_flow.summary.id),
            HistoryEntryId::Live(body_only.summary.id),
            HistoryEntryId::Live(legacy_flow.summary.id),
            HistoryEntryId::Live(pending_flow.summary.id),
        ]
    );
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);

    // MioScript `response.length` must equal the same original full response length.
    q.filter.filter_script = format!("response.length == {expected_header_heavy_len}");
    let script_orig = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(script_orig.total, 1);
    assert_eq!(
        script_orig.rows[0].id(),
        HistoryEntryId::Live(header_heavy_orig.summary.id)
    );

    q.filter.filter_script = format!("response.length == {expected_sse_len}");
    let script_sse = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(script_sse.total, 1);
    assert_eq!(
        script_sse.rows[0].id(),
        HistoryEntryId::Live(sse_flow.summary.id)
    );
}

#[test]
fn content_search_matches_live_and_archive_requests_and_responses_with_regex_case_and_negative_modes()
 {
    let (mut store, source, items) =
        super::super::super::proxy_history::tests::fixture(0, true, true);
    let mut live_req = live(
        "https://live.test/search-req",
        Some(200),
        100,
        TrafficSource::Proxy,
    );
    live_req.request.headers.push(crate::Header {
        name: "X-Token".into(),
        value: b"AlphaToken-123".to_vec(),
    });
    live_req.request.body = vec![0xff, b'R', b'E', b'Q'];
    live_req.summary.request_bytes = 4;
    store.put(&live_req).unwrap();

    let mut live_resp = live(
        "https://live.test/search-resp",
        Some(200),
        200,
        TrafficSource::Proxy,
    );
    live_resp.response.as_mut().unwrap().body = b"BetaPayload-999".to_vec();
    live_resp.summary.response_bytes = 15;
    store.put(&live_resp).unwrap();

    let mut q = query(&store);
    q.sort = HttpHistorySort {
        column: HttpHistorySortColumn::Number,
        ascending: false,
    };
    q.selected_ids = vec![
        HistoryEntryId::Live(live_req.summary.id),
        HistoryEntryId::Live(live_resp.summary.id),
        HistoryEntryId::External {
            source_id: source,
            item_id: items[0].id,
        },
    ];

    // Empty search_term must never read bodies or archive chunks.
    store.connection.authorizer(Some(no_bodies));
    let all = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(all.total, 6);
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);

    // Case-insensitive substring matching on live request header.
    q.filter.search_term = "alphatoken".into();
    let res = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(res.total, 1);
    assert_eq!(res.rows[0].id(), HistoryEntryId::Live(live_req.summary.id));
    assert_eq!(
        res.selected_matches,
        [
            (q.selected_ids[0], true),
            (q.selected_ids[1], false),
            (q.selected_ids[2], false),
        ]
    );
    assert_eq!(res.selected_ranks, [(q.selected_ids[0], 0)]);

    // Case-sensitive mismatch vs match.
    q.filter.case_sensitive = true;
    assert_eq!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap()
            .total,
        0
    );
    q.filter.search_term = "AlphaToken-123".into();
    assert_eq!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap()
            .total,
        1
    );

    // Regex matching across live response and archive edited messages.
    q.filter.case_sensitive = false;
    q.filter.regex = true;
    q.filter.search_term = r"BetaPayload-\d+|202 Accepted".into();
    let regex_res = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(regex_res.total, 3);
    assert_eq!(
        regex_res
            .rows
            .iter()
            .map(HistoryRow::id)
            .collect::<Vec<_>>(),
        [
            HistoryEntryId::Live(live_resp.summary.id),
            HistoryEntryId::External {
                source_id: source,
                item_id: items[2].id
            },
            HistoryEntryId::External {
                source_id: source,
                item_id: items[0].id
            },
        ]
    );
    assert_eq!(
        regex_res.selected_ranks,
        [(q.selected_ids[1], 0), (q.selected_ids[2], 2)]
    );

    // Archive rows also match terms present only in original request/response even when edited_* exists.
    q.filter.search_term = r"/fixture|201 Created".into();
    let orig_archive_res = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(orig_archive_res.total, 2);

    // Live edited rows also match original request/response sides (2 and 3).
    let mut edited_live = live_req.clone();
    edited_live.summary.revision += 1;
    edited_live.request.headers.clear();
    edited_live.request.body = b"REPLACED".to_vec();
    edited_live.summary.request_bytes = 8;
    store.put(&edited_live).unwrap();
    q.filter.search_term = r"AlphaToken-123".into();
    assert_eq!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap()
            .total,
        1
    );

    // Latin-1 raw byte search (\xff\x00\x81 in edited archive messages, \xffREQ in live_req).
    q.filter.search_term = r"\xFF(REQ|\x00\x81)".into();
    let latin1_res = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(latin1_res.total, 3);

    // Negative search inverts raw_match.
    q.filter.negative_search = true;
    let neg_res = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(neg_res.total, 3);
    assert_eq!(
        neg_res.rows[0].id(),
        HistoryEntryId::Live(live_resp.summary.id)
    );

    // Invalid regex and oversized search_term fail validation explicitly.
    q.filter.search_term = "(".into();
    assert!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .is_err()
    );
    q.filter.regex = false;
    q.filter.negative_search = false;
    let mut utf8_live = live(
        "http://127.0.0.1:18799/utf8",
        Some(200),
        9_000,
        TrafficSource::Proxy,
    );
    utf8_live.response.as_mut().unwrap().body = "响应包含中文关键字：秋山澪".as_bytes().to_vec();
    utf8_live.summary.response_bytes = utf8_live.response.as_ref().unwrap().body.len() as u64;
    store.put(&utf8_live).unwrap();
    q.filter.search_term = "秋山澪".into();
    let utf8_res = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(utf8_res.total, 1);
    assert_eq!(
        utf8_res.rows[0].id(),
        HistoryEntryId::Live(utf8_live.summary.id)
    );

    q.filter.search_term = "x".repeat(4097);
    assert!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .is_err()
    );
}

#[test]
fn http_history_in_scope_only_filter_matches_include_and_exclude_rules() {
    let mut store = ProjectStore::create(None).unwrap();
    let public_row = live(
        "http://example.com/public",
        Some(200),
        10,
        TrafficSource::Proxy,
    );
    let other_row = live("http://other.org/", Some(200), 20, TrafficSource::Proxy);
    let admin_row = live(
        "http://example.com/admin/panel",
        Some(200),
        30,
        TrafficSource::Proxy,
    );
    for row in [&public_row, &other_row, &admin_row] {
        store.put(row).unwrap();
    }
    let mut workspace = store.workspace().unwrap();
    workspace.revision += 1;
    workspace.scope = crate::TargetScope {
        include: vec![crate::ScopeRule {
            id: Uuid::new_v4(),
            enabled: true,
            prefix: "http://example.com".into(),
            include_subdomains: false,
        }],
        exclude: vec![crate::ScopeRule {
            id: Uuid::new_v4(),
            enabled: true,
            prefix: "http://example.com/admin".into(),
            include_subdomains: false,
        }],
        ..Default::default()
    };
    store.save_workspace(&workspace).unwrap();

    let mut q = query(&store);
    q.filter = HttpHistoryFilter {
        in_scope_only: true,
        ..Default::default()
    };
    let res = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(res.total, 1);
    assert_eq!(res.rows.len(), 1);
    assert_eq!(
        res.rows[0].id(),
        HistoryEntryId::Live(public_row.summary.id)
    );

    let converted = crate::convert_http_filter_to_script(&HttpHistoryFilter {
        in_scope_only: true,
        hide_without_response: true,
        status_classes: [true, false, false, false],
        search_term: "public".into(),
        ..Default::default()
    });
    assert!(crate::eval_http_filter_script(
        "request.method == \"GET\" && response.status == 200",
        "GET",
        "http://example.com/public",
        Some(200),
        10,
        None,
        None,
        "",
        false,
        None,
    )
    .unwrap());
    q.filter = HttpHistoryFilter {
        filter_script: converted,
        ..Default::default()
    };
    let scripted = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(scripted.total, 1);
    assert_eq!(
        scripted.rows[0].id(),
        HistoryEntryId::Live(public_row.summary.id)
    );
}

#[test]
fn filter_script_uses_original_messages_full_notes_and_propagates_parameter_and_cancel_errors() {
    let mut store = ProjectStore::create(None).unwrap();
    let owner = ProjectKey {
        id: store.id,
        generation: 1,
    };
    let cancel = CancellationToken::new();

    // 1. Live edited flow: original is POST /orig-path with JSON parameters and 201; edited is PUT /edited-path with 404.
    let mut orig_live = live(
        "https://mio_script.test/orig-path?from=orig",
        Some(201),
        100,
        TrafficSource::Proxy,
    );
    orig_live.summary.method = "POST".into();
    orig_live.request.method = "POST".into();
    orig_live.request.version = crate::HttpVersion::Http11;
    orig_live.request.trailers.clear();
    orig_live.request.headers = vec![
        crate::Header {
            name: "Host".into(),
            value: b"mio_script.test".to_vec(),
        },
        crate::Header {
            name: "Content-Type".into(),
            value: b"application/json".to_vec(),
        },
        crate::Header {
            name: "X-Stage".into(),
            value: b"original-req".to_vec(),
        },
    ];
    orig_live.request.body = br#"{"token":"orig-secret"}"#.to_vec();
    orig_live.summary.request_bytes = orig_live.request.body.len() as u64;
    orig_live.response.as_mut().unwrap().headers = vec![crate::Header {
        name: "X-Resp-Stage".into(),
        value: b"original-resp".to_vec(),
    }];
    orig_live.response.as_mut().unwrap().body = b"orig-resp-body".to_vec();
    orig_live.summary.response_bytes = orig_live.response.as_ref().unwrap().body.len() as u64;
    store
        .put_captured_flow(&orig_live, None, Some(18088))
        .unwrap();

    let mut edited_live = orig_live.clone();
    edited_live.summary.revision += 1;
    edited_live.summary.method = "PUT".into();
    edited_live.summary.url = "https://mio_script.test/edited-path".into();
    edited_live.summary.status = Some(404);
    edited_live.request.method = "PUT".into();
    edited_live.request.url = edited_live.summary.url.clone();
    edited_live.request.headers = vec![
        crate::Header {
            name: "Host".into(),
            value: b"mio_script.test".to_vec(),
        },
        crate::Header {
            name: "X-Stage".into(),
            value: b"edited-req".to_vec(),
        },
    ];
    edited_live.request.body = b"edited-req-body".to_vec();
    edited_live.summary.request_bytes = edited_live.request.body.len() as u64;
    edited_live.response.as_mut().unwrap().status = 404;
    edited_live.response.as_mut().unwrap().headers = vec![crate::Header {
        name: "X-Resp-Stage".into(),
        value: b"edited-resp".to_vec(),
    }];
    edited_live.response.as_mut().unwrap().body = b"edited-resp-body".to_vec();
    edited_live.summary.response_bytes = edited_live.response.as_ref().unwrap().body.len() as u64;
    store.put(&edited_live).unwrap();

    let long_prefix = "备".repeat(300);
    let live_long_note = format!("{long_prefix}:LIVE_TAIL_MARKER");
    store
        .set_ext_http_notes(
            owner,
            Uuid::nil(),
            orig_live.summary.id,
            0,
            &live_long_note,
            &cancel,
        )
        .unwrap();

    // 2. XML and multipart parameterized live flows (without URL query params).
    let mut xml_live = live(
        "https://mio_script.test/xml",
        Some(200),
        200,
        TrafficSource::Proxy,
    );
    xml_live.summary.method = "POST".into();
    xml_live.request.method = "POST".into();
    xml_live.request.version = crate::HttpVersion::Http11;
    xml_live.request.trailers.clear();
    xml_live.request.headers = vec![
        crate::Header {
            name: "Host".into(),
            value: b"mio_script.test".to_vec(),
        },
        crate::Header {
            name: "Content-Type".into(),
            value: b"application/xml".to_vec(),
        },
    ];
    xml_live.request.body = b"<root attr=\"1\"><item>val</item></root>".to_vec();
    xml_live.summary.request_bytes = xml_live.request.body.len() as u64;
    store.put(&xml_live).unwrap();

    let mut multipart_live = live(
        "https://mio_script.test/multipart",
        Some(200),
        300,
        TrafficSource::Proxy,
    );
    multipart_live.summary.method = "POST".into();
    multipart_live.request.method = "POST".into();
    multipart_live.request.version = crate::HttpVersion::Http11;
    multipart_live.request.trailers.clear();
    multipart_live.request.headers = vec![
        crate::Header {
            name: "Host".into(),
            value: b"mio_script.test".to_vec(),
        },
        crate::Header {
            name: "Content-Type".into(),
            value: b"multipart/form-data; boundary=bound".to_vec(),
        },
    ];
    multipart_live.request.body =
        b"--bound\r\nContent-Disposition: form-data; name=\"field\"\r\n\r\nvalue\r\n--bound--\r\n"
            .to_vec();
    multipart_live.summary.request_bytes = multipart_live.request.body.len() as u64;
    store.put(&multipart_live).unwrap();

    let mut q = query(&store);
    q.url_filter = "orig-path".into();
    for predicate in [
        "request.method == \"POST\"",
        "request.path == \"/orig-path\"",
        "request.query == \"from=orig\"",
        "request.header_value(\"x-stage\") == \"original-req\"",
        "request.body.contains(\"orig-secret\")",
        "request.raw.contains(\"POST /orig-path?from=orig HTTP/1.1\")",
        "response.status == 201",
        "response.header_value(\"x-resp-stage\") == \"original-resp\"",
        "response.body == \"orig-resp-body\"",
        "response.raw.contains(\"HTTP/1.1 201\")",
        "notes.contains(\"LIVE_TAIL_MARKER\")",
        "parameterized",
        "listener_port == 18088",
    ] {
        q.filter.filter_script = predicate.into();
        let matched = store.query_http_history(&q, &cancel).unwrap();
        assert_eq!(matched.total, 1, "{predicate}");
        assert_eq!(matched.rows[0].id(), HistoryEntryId::Live(orig_live.summary.id));
    }
    q.url_filter.clear();
    q.filter.filter_script = r#"
        let req_stage = request.header_value("x-stage");
        let resp_stage = response.header_value("x-resp-stage");
        request.method == "POST"
            && request.path == "/orig-path"
            && request.query == "from=orig"
            && req_stage == "original-req"
            && request.body.contains("orig-secret")
            && request.raw.contains("POST /orig-path?from=orig HTTP/1.1")
            && !request.raw.contains("edited-req")
            && response.status == 201
            && resp_stage == "original-resp"
            && response.body == "orig-resp-body"
            && response.raw.contains("HTTP/1.1 201")
            && !response.raw.contains("edited-resp")
            && notes.contains("LIVE_TAIL_MARKER")
            && parameterized
            && listener_port == 18088
    "#
    .into();
    let res_live = store.query_http_history(&q, &cancel).unwrap();
    assert_eq!(res_live.total, 1);
    assert_eq!(
        res_live.rows[0].id(),
        HistoryEntryId::Live(orig_live.summary.id)
    );
    // Returned live_annotations preview remains 256 chars while script saw full notes.
    let (_, _, preview, _) = res_live
        .live_annotations
        .iter()
        .find(|(id, _, _, _)| *id == orig_live.summary.id)
        .unwrap();
    assert_eq!(preview.chars().count(), 256);
    assert!(!preview.contains("LIVE_TAIL_MARKER"));

    q.filter.filter_script = r#"
        request.method == "PUT"
            || request.path == "/edited-path"
            || request.path == "/edited"
            || request.raw.contains("edited-req")
            || request.raw.contains("X-Edited")
            || response.status == 404
            || response.status == 202
            || response.raw.contains("edited-resp")
            || response.raw.contains("202 Accepted")
    "#
    .into();
    assert_eq!(store.query_http_history(&q, &cancel).unwrap().total, 0);

    q.filter = HttpHistoryFilter {
        search_term: "edited-resp-body".into(),
        ..Default::default()
    };
    assert_eq!(store.query_http_history(&q, &cancel).unwrap().total, 1);

    // Script `parameterized` matches JSON, XML, and multipart bodies using the same rules as `parameterized_only`.
    q.filter = HttpHistoryFilter {
        filter_script: "parameterized && (request.path == \"/xml\" || request.path == \"/multipart\" || request.path == \"/orig-path\")".into(),
        ..Default::default()
    };
    let param_res = store.query_http_history(&q, &cancel).unwrap();
    assert_eq!(param_res.total, 3);

    // Pre-cancelled token aborts MioScript script evaluation immediately with a visible error.
    let pre_cancelled = CancellationToken::new();
    pre_cancelled.cancel();
    assert!(
        crate::http::eval_http_filter_script_with_scope_and_cancel(
            "true",
            "GET",
            "https://mio_script.test/",
            Some(200),
            0,
            None,
            None,
            "",
            false,
            None,
            false,
            &pre_cancelled,
        )
        .unwrap_err()
        .to_string()
        .contains("已取消")
    );
    assert!(store.query_http_history(&q, &pre_cancelled).is_err());

    // Malformed / conflicting Content-Type or parameter limit errors in MioScript script propagate instead of being swallowed.
    let bad_json_raw =
        b"POST /bad-json HTTP/1.1\r\nHost: mio_script.test\r\nContent-Type: application/json\r\n\r\n{\"broken\":";
    assert!(
        crate::eval_http_filter_script(
            "parameterized",
            "POST",
            "https://mio_script.test/bad-json",
            Some(200),
            0,
            Some(bad_json_raw),
            None,
            "",
            false,
            None,
        )
        .is_err()
    );

    let mut bad_flow = live(
        "https://mio_script.test/bad-multipart",
        Some(200),
        400,
        TrafficSource::Proxy,
    );
    bad_flow.summary.method = "POST".into();
    bad_flow.request.method = "POST".into();
    bad_flow.request.version = crate::HttpVersion::Http11;
    bad_flow.request.trailers.clear();
    bad_flow.request.headers = vec![
        crate::Header {
            name: "Host".into(),
            value: b"mio_script.test".to_vec(),
        },
        crate::Header {
            name: "Content-Type".into(),
            value: b"multipart/form-data".to_vec(),
        },
    ];
    bad_flow.request.body = b"--missing-boundary\r\n".to_vec();
    bad_flow.summary.request_bytes = bad_flow.request.body.len() as u64;
    store.put(&bad_flow).unwrap();

    q.filter.filter_script = "request.method == \"POST\"".into();
    let err = store
        .query_http_history(&q, &cancel)
        .unwrap_err()
        .to_string();
    assert!(err.contains("boundary") || err.contains("参数分析"));

    if let Some(real_ext) = std::env::var_os("MIO_REAL_ARCHIVE_SAMPLE").map(std::path::PathBuf::from)
        && real_ext.is_file()
    {
        let mut imported_store = ProjectStore::open_ext(&real_ext, &cancel).unwrap();
        let imported_owner = ProjectKey {
            id: imported_store.id,
            generation: 1,
        };
        let mut real_q = query(&imported_store);
        let all_real = imported_store.query_http_history(&real_q, &cancel).unwrap();
        assert!(all_real.total > 0);
        let first_live = all_real
            .rows
            .iter()
            .filter_map(HistoryRow::live)
            .find(|row| {
                row.url.contains('?')
                    && all_real
                        .row_metadata_for(HistoryEntryId::Live(row.id))
                        .is_some_and(|metadata| metadata.params_known && metadata.has_params)
            })
            .expect("真实来源包含已确认的 URL 参数请求")
            .clone();
        let stored_notes = imported_store
            .ext_http_notes(imported_owner, Uuid::nil(), first_live.id)
            .unwrap();
        let real_long_note = format!("{}:REAL_IMPORT_NOTE_TAIL", "注".repeat(300));
        imported_store
            .set_ext_http_notes(
                imported_owner,
                Uuid::nil(),
                first_live.id,
                stored_notes.revision,
                &real_long_note,
                &cancel,
            )
            .unwrap();
        real_q.url_filter = first_live.url.clone();
        real_q.filter.filter_script = format!(
            "request.method == {:?} && request.url == {:?} && notes.contains(\"REAL_IMPORT_NOTE_TAIL\")",
            first_live.method, first_live.url
        );
        let matched_real = imported_store.query_http_history(&real_q, &cancel).unwrap();
        assert_eq!(matched_real.total, 1);
        assert_eq!(
            matched_real.rows[0].id(),
            HistoryEntryId::Live(first_live.id)
        );
        imported_store.close().unwrap();
    }
}

#[test]
fn filter_script_pending_recovery_uses_first_request_and_first_response_and_rejects_missing_originals()
{
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("script-pending.mio");
    let store = ProjectStore::create(Some(&path)).unwrap();
    let cancel = CancellationToken::new();

    let mut first = live(
        "https://pending.test/orig?k=1",
        Some(201),
        10,
        TrafficSource::Proxy,
    );
    first.summary.method = "POST".into();
    first.request.method = "POST".into();
    first.request.headers = vec![
        crate::Header {
            name: "Host".into(),
            value: b"pending.test".to_vec(),
        },
        crate::Header {
            name: "X-Orig-Req".into(),
            value: b"yes".to_vec(),
        },
    ];
    first.request.body = b"orig-pending-req".to_vec();
    first.summary.request_bytes = first.request.body.len() as u64;
    first.response.as_mut().unwrap().headers = vec![crate::Header {
        name: "X-Orig-Resp".into(),
        value: b"yes".to_vec(),
    }];
    first.response.as_mut().unwrap().body = b"orig-pending-resp".to_vec();
    first.summary.response_bytes = first.response.as_ref().unwrap().body.len() as u64;

    let mut edited = first.clone();
    edited.summary.revision += 1;
    edited.summary.method = "DELETE".into();
    edited.summary.url = "https://pending.test/edited".into();
    edited.summary.status = Some(500);
    edited.request.method = "DELETE".into();
    edited.request.url = edited.summary.url.clone();
    edited.request.headers.clear();
    edited.request.body = b"edited-pending-req".to_vec();
    edited.summary.request_bytes = edited.request.body.len() as u64;
    edited.response.as_mut().unwrap().status = 500;
    edited.response.as_mut().unwrap().headers.clear();
    edited.response.as_mut().unwrap().body = b"edited-pending-resp".to_vec();
    edited.summary.response_bytes = edited.response.as_ref().unwrap().body.len() as u64;

    let mut pending = HttpHistoryPending {
        summary: edited.summary.clone(),
        first_request_method: first.request.method.clone(),
        first_request_url: first.request.url.clone(),
        first_response_status: Some(201),
        first_request: Some(first.request.clone()),
        first_response: first.response.clone(),
        request: Some(edited.request.clone()),
        response: edited.response.clone(),
        listener_port: Some(18099),
        capture_metrics: mio_proxy::CapturedFlowMetrics::default(),
    };

    let mut q = query(&store);
    q.filter.filter_script = r#"
        request.method == "POST"
            && request.path == "/orig"
            && request.query == "k=1"
            && request.header_value("x-orig-req") == "yes"
            && request.body == "orig-pending-req"
            && response.status == 201
            && response.header_value("x-orig-resp") == "yes"
            && response.body == "orig-pending-resp"
            && parameterized
            && listener_port == 18099
    "#
    .into();
    let matched =
        ProjectStore::http_history_snapshot(&path, &q, std::slice::from_ref(&pending), &cancel)
            .unwrap();
    assert_eq!(matched.total, 1);
    assert_eq!(matched.rows[0].id(), HistoryEntryId::Live(first.summary.id));

    // Missing first_request when no DB row exists must return an explicit error rather than falling back to edited request.
    pending.first_request = None;
    assert!(
        ProjectStore::http_history_snapshot(&path, &q, std::slice::from_ref(&pending), &cancel)
            .unwrap_err()
            .to_string()
            .contains("缺少原始请求")
    );
}
