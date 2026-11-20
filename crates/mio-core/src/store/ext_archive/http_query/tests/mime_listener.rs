use super::{super::*, live, query};
use crate::store::ext_archive::http_query::filters::{HistoryMime, infer_history_mime};
use crate::store::ext_archive::proxy_history::tests::fixture;
use crate::{FlowState, HttpResponse, HttpVersion};
use rusqlite::params;

fn response(content_type: Option<&str>, body: &[u8], status: u16) -> HttpResponse {
    let headers = content_type
        .map(|value| Header {
            name: "Content-Type".into(),
            value: value.as_bytes().to_vec(),
        })
        .into_iter()
        .collect();
    HttpResponse {
        status,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers,
        trailers: Vec::new(),
        body: body.to_vec(),
    }
}

#[test]
fn infer_history_mime_prefers_content_type_and_sniffs_unknown_bodies() {
    let cases = [
        (Some("text/html"), b"not magic" as &[u8], HistoryMime::Html),
        (Some("application/javascript"), b"", HistoryMime::Script),
        (Some("application/xml"), b"", HistoryMime::Xml),
        (Some("text/css"), b"", HistoryMime::Css),
        (Some("image/png"), b"", HistoryMime::Images),
        (
            Some("application/x-shockwave-flash"),
            b"",
            HistoryMime::Flash,
        ),
        (
            Some("application/octet-stream"),
            b"",
            HistoryMime::OtherBinary,
        ),
        (Some("unknown/type"), b"<html></html>", HistoryMime::Html),
        (None, b"\x89PNG\r\n\x1a\n", HistoryMime::Images),
        (None, b"", HistoryMime::OtherBinary),
    ];
    for (content_type, body, expected) in cases {
        let response = response(content_type, body, 200);
        assert_eq!(
            infer_history_mime(&response.headers, &response.body),
            expected
        );
    }
    let empty = response(None, b"", 204);
    assert_eq!(
        infer_history_mime(&empty.headers, &empty.body),
        HistoryMime::OtherBinary
    );
    let json_resp = response(
        Some("application/json; charset=UTF-8"),
        br#"{"api":"","ret":["SUCCESS::ok"]}"#,
        200,
    );
    assert_eq!(
        infer_history_mime(&json_resp.headers, &json_resp.body),
        HistoryMime::OtherText
    );
    assert_eq!(
        crate::http::infer_history_display_mime_type(&json_resp.headers, &json_resp.body),
        "JSON"
    );
    let vendor_json = response(Some("application/problem+json"), b"", 400);
    assert_eq!(
        infer_history_mime(&vendor_json.headers, &vendor_json.body),
        HistoryMime::OtherText
    );
    assert_eq!(
        crate::http::infer_history_display_mime_type(&vendor_json.headers, &vendor_json.body),
        "JSON"
    );
    let explicit_plain_json = response(Some("text/plain; charset=utf-8"), br#"{"ok":true}"#, 200);
    assert_eq!(
        infer_history_mime(&explicit_plain_json.headers, &explicit_plain_json.body),
        HistoryMime::OtherText
    );
    assert_eq!(
        crate::http::infer_history_display_mime_type(
            &explicit_plain_json.headers,
            &explicit_plain_json.body
        ),
        "text"
    );
    let mut large_signed_json = Vec::from(b"\xef\xbb\xbf \n\t{\"v\":\"1794917702647\",\"i\":0,\"packs\":{");
    for idx in 0..24 {
        if idx > 0 {
            large_signed_json.push(b',');
        }
        large_signed_json.extend_from_slice(
            format!(
                "\"pkg_{idx}\":{{\"s\":1371622,\"p\":[\"//cdn.example.test/app/bundle/cart/{idx}?c=view\"]}}"
            )
            .as_bytes(),
        );
    }
    large_signed_json.extend_from_slice(b"}}||kWaMRBcYj9wr7tlENn4yRi6mbE3SoXID7F4+WbeHpWjR1m2VL0np==");
    assert!(large_signed_json.len() > 1500);
    let sniffed_json = response(None, &large_signed_json, 200);
    assert_eq!(
        infer_history_mime(&sniffed_json.headers, &sniffed_json.body),
        HistoryMime::OtherText
    );
    assert_eq!(
        crate::http::infer_history_display_mime_type(&sniffed_json.headers, &sniffed_json.body),
        "JSON"
    );
    let sniffed_array = response(None, br#"[1,{"ok":true}]||sig"#, 200);
    assert_eq!(
        crate::http::infer_history_display_mime_type(&sniffed_array.headers, &sniffed_array.body),
        "JSON"
    );
    for invalid_json in [
        br#"{"unterminated": 1"#.as_slice(),
        br#"{"trailing": 1,}"#.as_slice(),
        b"{\nnot_json: true}".as_slice(),
        b"true".as_slice(),
        b"12345".as_slice(),
    ] {
        let invalid = response(None, invalid_json, 200);
        assert_eq!(
            infer_history_mime(&invalid.headers, &invalid.body),
            HistoryMime::OtherText
        );
        assert_eq!(
            crate::http::infer_history_display_mime_type(&invalid.headers, &invalid.body),
            "text"
        );
    }
}

#[test]
fn mime_and_listener_filters_match_live_and_archive_independently() {
    let (mut store, source, items) = fixture(0, false, false);
    let mut html = live(
        "http://mime.test/page.html",
        Some(200),
        1,
        TrafficSource::Proxy,
    );
    html.response.as_mut().unwrap().headers.push(crate::Header {
        name: "Content-Type".into(),
        value: b"text/html".to_vec(),
    });
    let mut binary = live("http://mime.test/blob", Some(204), 2, TrafficSource::Proxy);
    binary.response.as_mut().unwrap().body.clear();
    binary.summary.response_bytes = 0;
    for row in [&html, &binary] {
        store.put(row).unwrap();
    }
    store
        .connection
        .execute(
            "UPDATE flows SET listener_port=18765 WHERE id=?1",
            params![html.summary.id.to_string()],
        )
        .unwrap();
    let mut q = query(&store);
    q.filter.mime_types = [true, false, false, false, false, false, false, false];
    let mime = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    let ids: Vec<_> = mime.rows.iter().map(HistoryRow::id).collect();
    assert!(ids.contains(&HistoryEntryId::Live(html.summary.id)));
    assert!(!ids.contains(&HistoryEntryId::External {
        source_id: source,
        item_id: items[0].id
    }));
    assert!(!ids.contains(&HistoryEntryId::Live(binary.summary.id)));

    q.filter.mime_types = [false, false, false, false, false, false, false, true];
    let other_binary = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    let other_binary_ids: Vec<_> = other_binary.rows.iter().map(HistoryRow::id).collect();
    assert!(other_binary_ids.contains(&HistoryEntryId::External {
        source_id: source,
        item_id: items[0].id
    }));
    assert!(other_binary_ids.contains(&HistoryEntryId::Live(binary.summary.id)));
    assert!(!other_binary_ids.contains(&HistoryEntryId::Live(html.summary.id)));

    q.filter = HttpHistoryFilter::default();
    q.filter.listener_port = Some(0);
    assert!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .is_err()
    );
    q.filter.listener_port = Some(18765);
    let listener_result = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    let listener_ids: Vec<_> = listener_result.rows.iter().map(HistoryRow::id).collect();
    assert!(listener_ids.contains(&HistoryEntryId::Live(html.summary.id)));
    assert!(!listener_ids.contains(&HistoryEntryId::External {
        source_id: source,
        item_id: items[0].id
    }));
    assert!(!listener_ids.contains(&HistoryEntryId::Live(binary.summary.id)));
}

#[test]
fn put_captured_flow_preserves_listener_port_across_revisions_and_rejects_port_mutations() {
    let mut store = ProjectStore::create(None).unwrap();

    let mut same_port_flow = live(
        "http://listener.test/same-port",
        None,
        10,
        TrafficSource::Proxy,
    );
    store
        .put_captured_flow(&same_port_flow, None, Some(18081))
        .unwrap();
    same_port_flow.summary.revision += 1;
    same_port_flow.summary.state = FlowState::Complete;
    same_port_flow.summary.status = Some(200);
    same_port_flow.summary.response_bytes = 2;
    same_port_flow.response = Some(response(Some("text/plain"), b"ok", 200));
    store
        .put_captured_flow(&same_port_flow, None, Some(18081))
        .unwrap();

    let mut none_port_flow = live(
        "http://listener.test/none-port-on-response",
        None,
        20,
        TrafficSource::Proxy,
    );
    store
        .put_captured_flow(&none_port_flow, None, Some(18081))
        .unwrap();
    none_port_flow.summary.revision += 1;
    none_port_flow.summary.state = FlowState::Complete;
    none_port_flow.summary.status = Some(201);
    none_port_flow.summary.response_bytes = 4;
    none_port_flow.response = Some(response(Some("text/html"), b"done", 201));
    store
        .put_captured_flow(&none_port_flow, None, None)
        .unwrap();

    for flow in [&same_port_flow, &none_port_flow] {
        let stored_port: Option<u16> = store
            .connection
            .query_row(
                "SELECT listener_port FROM flows WHERE id=?1",
                [flow.summary.id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_port, Some(18081));
        assert_eq!(store.read(flow.summary.id).unwrap(), *flow);
    }

    let mut q = query(&store);
    q.filter.listener_port = Some(18081);
    let matched = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    let matched_ids: Vec<_> = matched.rows.iter().map(HistoryRow::id).collect();
    assert_eq!(matched.total, 2);
    assert!(matched_ids.contains(&HistoryEntryId::Live(same_port_flow.summary.id)));
    assert!(matched_ids.contains(&HistoryEntryId::Live(none_port_flow.summary.id)));

    q.filter.listener_port = Some(18082);
    let missed = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(missed.total, 0);
    assert!(missed.rows.is_empty());

    let mut conflicting = same_port_flow.clone();
    conflicting.summary.revision += 1;
    conflicting.summary.status = Some(500);
    conflicting.response = Some(response(Some("text/plain"), b"tampered", 500));
    conflicting.summary.response_bytes = 8;
    assert!(
        store
            .put_captured_flow(&conflicting, None, Some(18082))
            .is_err()
    );
    assert!(
        store
            .put_captured_flow(&same_port_flow, None, Some(18082))
            .is_err()
    );
    assert!(
        store
            .put_captured_flow(&conflicting, None, Some(0))
            .is_err()
    );
    let fresh_zero = live(
        "http://listener.test/invalid-zero",
        None,
        30,
        TrafficSource::Proxy,
    );
    assert!(store.put_captured_flow(&fresh_zero, None, Some(0)).is_err());
    assert!(
        store
            .read_optional(fresh_zero.summary.id)
            .unwrap()
            .is_none()
    );

    let stored_port: Option<u16> = store
        .connection
        .query_row(
            "SELECT listener_port FROM flows WHERE id=?1",
            [same_port_flow.summary.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_port, Some(18081));
    assert_eq!(
        store.read(same_port_flow.summary.id).unwrap(),
        same_port_flow
    );
}

#[test]
fn http_history_snapshot_combines_pending_listener_port_and_mime_filters() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pending-listener-mime.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();

    let mut persisted_other_port = live(
        "http://pending.test/persisted.js",
        Some(200),
        1,
        TrafficSource::Proxy,
    );
    persisted_other_port.response = Some(response(
        Some("application/javascript; charset=utf-8"),
        b"console.log(0);",
        200,
    ));
    persisted_other_port.summary.response_bytes =
        persisted_other_port.response.as_ref().unwrap().body.len() as u64;
    store
        .put_captured_flow(&persisted_other_port, None, Some(19091))
        .unwrap();

    let mut script_flow = live(
        "http://pending.test/app.js",
        Some(200),
        2,
        TrafficSource::Proxy,
    );
    let script_response = response(Some("application/javascript"), b"window.__mio = true;", 200);
    script_flow.summary.response_bytes = script_response.body.len() as u64;
    script_flow.response = Some(script_response);

    let mut png_flow = live(
        "http://pending.test/sprite.bin",
        Some(200),
        3,
        TrafficSource::Proxy,
    );
    let png_response = response(None, b"\x89PNG\r\n\x1a\npayload", 200);
    png_flow.summary.response_bytes = png_response.body.len() as u64;
    png_flow.response = Some(png_response);

    let mut wrong_port_script = live(
        "http://pending.test/other-port.js",
        Some(200),
        4,
        TrafficSource::Proxy,
    );
    let wrong_port_response = response(Some("application/javascript"), b"ignored();", 200);
    wrong_port_script.summary.response_bytes = wrong_port_response.body.len() as u64;
    wrong_port_script.response = Some(wrong_port_response);

    let make_pending = |flow: &crate::FlowRecord, listener_port: Option<u16>| HttpHistoryPending {
        summary: flow.summary.clone(),
        first_request_method: flow.request.method.clone(),
        first_request_url: flow.request.url.clone(),
        first_response_status: flow.response.as_ref().map(|resp| resp.status),
        first_request: Some(flow.request.clone()),
        first_response: flow.response.clone(),
        request: Some(flow.request.clone()),
        response: flow.response.clone(),
        listener_port,
        capture_metrics: mio_proxy::CapturedFlowMetrics::default(),
    };
    let pending = [
        make_pending(&script_flow, Some(19090)),
        make_pending(&png_flow, Some(19090)),
        make_pending(&wrong_port_script, Some(19091)),
    ];

    let cancel = CancellationToken::new();
    let mut q = query(&store);
    q.filter.listener_port = Some(19090);
    let by_port = ProjectStore::http_history_snapshot(&path, &q, &pending, &cancel).unwrap();
    let port_ids: Vec<_> = by_port.rows.iter().map(HistoryRow::id).collect();
    assert_eq!(by_port.total, 2);
    assert!(port_ids.contains(&HistoryEntryId::Live(script_flow.summary.id)));
    assert!(port_ids.contains(&HistoryEntryId::Live(png_flow.summary.id)));
    assert!(!port_ids.contains(&HistoryEntryId::Live(wrong_port_script.summary.id)));
    assert!(!port_ids.contains(&HistoryEntryId::Live(persisted_other_port.summary.id)));

    q.filter.listener_port = Some(19092);
    let missed_port = ProjectStore::http_history_snapshot(&path, &q, &pending, &cancel).unwrap();
    assert_eq!(missed_port.total, 0);
    assert!(missed_port.rows.is_empty());

    q.filter.listener_port = Some(19090);
    q.filter.mime_types = [false, true, false, false, false, false, false, false];
    let script_only = ProjectStore::http_history_snapshot(&path, &q, &pending, &cancel).unwrap();
    assert_eq!(script_only.total, 1);
    assert_eq!(
        script_only.rows,
        [HistoryRow::Live(script_flow.summary.clone())]
    );

    q.filter.mime_types = [false, false, false, false, false, true, false, false];
    let images_only = ProjectStore::http_history_snapshot(&path, &q, &pending, &cancel).unwrap();
    assert_eq!(images_only.total, 1);
    assert_eq!(
        images_only.rows,
        [HistoryRow::Live(png_flow.summary.clone())]
    );

    assert!(
        store
            .read_optional(script_flow.summary.id)
            .unwrap()
            .is_none()
    );
    assert!(store.read_optional(png_flow.summary.id).unwrap().is_none());
}

#[test]
fn schema_27_ten_thousand_flows_indexed_source_and_lazy_sort_work() {
    use rusqlite::TransactionBehavior;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("scale-10k.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let json_resp = response(Some("application/json"), b"{\"ok\":true}", 200);
    let html_resp = response(
        Some("text/html"),
        b"<!DOCTYPE html><html><head><title>Zeta</title></head><body></body></html>",
        201,
    );
    {
        let tx = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        for i in 0..10_000usize {
            let flow_id = Uuid::from_u128(((u64::MAX - i as u64) as u128) << 64 | (i as u128 + 1));
            let resp = if i % 50 == 0 { &html_resp } else { &json_resp };
            let mut record = live(
                &format!("http://scale.example/item/{i}"),
                Some(resp.status),
                1_700_000_000_000 + i as i64,
                TrafficSource::Proxy,
            );
            record.summary.id = flow_id;
            record.summary.response_bytes = resp.body.len() as u64;
            record.summary.elapsed_ms = (i % 97) as u64;
            record.response = Some(resp.clone());
            let meta = crate::store::messages::ImportedFlowHeadMeta {
                time_known: true,
                capture_number: (i + 1) as u64,
                remote_ip: "127.0.0.1",
                response_received_ms: Some((i % 25) as u64),
            };
            crate::store::messages::write_imported_capture(&tx, &record, None, Some(8080), &meta)
                .unwrap();
        }
        for i in 0..500usize {
            let source = if i % 2 == 0 {
                TrafficSource::Repeater
            } else {
                TrafficSource::Intruder
            };
            let record = live(
                &format!("http://scale.example/tool/{i}"),
                Some(200),
                1_700_000_500_000 + i as i64,
                source,
            );
            crate::store::messages::write_record(&tx, &record).unwrap();
        }
        crate::store::messages::write_next_proxy_capture_number(&tx, 10_001).unwrap();
        tx.commit().unwrap();
    }

    let plan_started: String = store
        .connection
        .query_row(
            "EXPLAIN QUERY PLAN SELECT id FROM flows WHERE source='Proxy' ORDER BY started DESC, id LIMIT 200",
            [],
            |row| row.get(3),
        )
        .unwrap();
    assert!(
        plan_started.contains("flows_source_started"),
        "expected flows_source_started index, got: {plan_started}"
    );
    let plan_capture: String = store
        .connection
        .query_row(
            "EXPLAIN QUERY PLAN SELECT id FROM flows WHERE source='Proxy' ORDER BY capture_number DESC LIMIT 200",
            [],
            |row| row.get(3),
        )
        .unwrap();
    assert!(
        plan_capture.contains("flows_source_capture"),
        "expected flows_source_capture index, got: {plan_capture}"
    );

    let cancel = CancellationToken::new();
    for (column, ascending) in [
        (HttpHistorySortColumn::Number, false),
        (HttpHistorySortColumn::Length, false),
        (HttpHistorySortColumn::Status, false),
        (HttpHistorySortColumn::Time, false),
        (HttpHistorySortColumn::Title, false),
        (HttpHistorySortColumn::MimeType, true),
    ] {
        let mut q = query(&store);
        q.limit = 200;
        q.sort = crate::HttpHistorySort { column, ascending };
        let res = ProjectStore::http_history_snapshot(&path, &q, &[], &cancel).unwrap();
        assert_eq!(res.total, 10_000);
        assert_eq!(res.rows.len(), 200);
        if column == HttpHistorySortColumn::Number {
            let first_id = res.rows[0].live().unwrap().id;
            let first_num = res
                .live_numbers
                .iter()
                .find_map(|&(id, num)| (id == first_id).then_some(num));
            assert_eq!(first_num, Some(10_000));
        }
        if column == HttpHistorySortColumn::Title {
            let first_meta = res.row_metadata_for(res.rows[0].id()).unwrap();
            assert_eq!(first_meta.title, "Zeta");
        }
    }

    let (logger_total, logger_rows) = store
        .logger_history_page(0, 200, "", Some(&TrafficSource::Proxy), &cancel)
        .unwrap();
    assert_eq!(logger_total, 10_000);
    assert_eq!(logger_rows.len(), 200);
    assert!(
        logger_rows
            .iter()
            .all(|row| row.source == TrafficSource::Proxy)
    );
}
