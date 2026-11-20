use super::super::filters::parameterized_archive_request;
use super::*;

#[test]
fn http_history_parameterized_filter_keeps_content_encoding_opaque_and_excludes_cookies() {
    use std::io::Write;

    let mut store = ProjectStore::create(None).unwrap();
    let mut expected = HashSet::new();
    for (index, (content_type, body, compress, matched)) in [
        (
            "application/x-www-form-urlencoded",
            b"name=value".as_slice(),
            true,
            true,
        ),
        ("application/json", b"{}", true, false),
        ("application/json", b"rawnotgzip", false, false),
    ]
    .into_iter()
    .enumerate()
    {
        let mut row = live(
            "http://example.com/encoded",
            Some(200),
            index as i64,
            TrafficSource::Proxy,
        );
        row.request.headers = vec![
            crate::Header {
                name: "Content-Type".into(),
                value: content_type.as_bytes().to_vec(),
            },
            crate::Header {
                name: "Content-Encoding".into(),
                value: b"gzip".to_vec(),
            },
        ];
        row.request.body = if compress {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(body).unwrap();
            encoder.finish().unwrap()
        } else {
            body.to_vec()
        };
        row.summary.request_bytes = row.request.body.len() as u64;
        store.put(&row).unwrap();
        if matched {
            expected.insert(HistoryEntryId::Live(row.summary.id));
        }
        assert_eq!(store.read(row.summary.id).unwrap(), row);
    }
    let mut cookie_only = live(
        "http://example.com/cookie",
        Some(200),
        5,
        TrafficSource::Proxy,
    );
    cookie_only.request.headers = vec![crate::Header {
        name: "Cookie".into(),
        value: b"name=value".to_vec(),
    }];
    cookie_only.request.body.clear();
    cookie_only.summary.request_bytes = 0;
    store.put(&cookie_only).unwrap();
    let mut q = query(&store);
    q.filter.parameterized_only = true;
    let result = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(result.total, expected.len());
    assert_eq!(
        result
            .rows
            .iter()
            .map(HistoryRow::id)
            .collect::<HashSet<_>>(),
        expected
    );
    assert_eq!(store.read(cookie_only.summary.id).unwrap(), cookie_only);
}

#[test]
fn http_history_parameterized_filter_uses_live_data_and_decodes_archive_transfer_framing_once() {
    let mut store = ProjectStore::create(None).unwrap();
    let mut row = live(
        "http://example.com/chunked",
        Some(200),
        1,
        TrafficSource::Proxy,
    );
    row.request.headers = vec![
        crate::Header {
            name: "Content-Type".into(),
            value: b"application/x-www-form-urlencoded".to_vec(),
        },
        crate::Header {
            name: "Transfer-Encoding".into(),
            value: b"chunked".to_vec(),
        },
    ];
    row.request.body = b"name=value".to_vec();
    row.summary.request_bytes = row.request.body.len() as u64;
    store.put(&row).unwrap();
    let mut q = query(&store);
    q.filter.parameterized_only = true;
    let result = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.rows[0].id(), HistoryEntryId::Live(row.summary.id));
    assert_eq!(store.read(row.summary.id).unwrap(), row);

    let source = Uuid::new_v4();
    let raw = b"POST /chunked HTTP/1.1\r\nContent-Type: application/x-www-form-urlencoded\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nname\r\n6\r\n=value\r\n0\r\n\r\n";
    let initial_report = crate::ExtImportReport {
        schema_floor: 229,
        schema_revision: 229,
        source_bytes: raw.len() as u64,
        indexed_items: 0,
        indexed_messages: 0,
        collections: Vec::new(),
        repeater_active_tab: None,
        gaps: Vec::new(),
        complete_archive: false,
    };
    store.begin_source(source, &initial_report).unwrap();
    store.append_source_chunk(source, 0, raw).unwrap();
    let mut completed_report = initial_report;
    completed_report.complete_archive = true;
    store.finish_source(source, &completed_report).unwrap();
    let summary = ExtMessageSummary {
        id: Uuid::new_v4(),
        source_id: source,
        item_id: Uuid::new_v4(),
        collection: "root.f1.f0".into(),
        item_index: 0,
        role: "request".into(),
        field_id: 15,
        raw_offset: 0,
        raw_bytes: raw.len() as u64,
        body_offset: raw
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .map(|offset| offset as u64 + 4),
        declared_version: Some(crate::HttpVersion::Http11),
        method: Some("POST".into()),
        request_target: Some("/chunked".into()),
        status: None,
        diagnostic: None,
    };
    assert!(
        parameterized_archive_request(&store.connection, &summary, &CancellationToken::new())
            .unwrap()
    );
    assert_eq!(store.read_source_range(source, 0, raw.len()).unwrap(), raw);
    assert_eq!(store.ext_report(source).unwrap(), completed_report);
}

#[test]
fn http_history_parameterized_only_filter_ram_request_keeps_original_body_and_requires_it() {
    let mut store = ProjectStore::create(None).unwrap();
    let mut first = live("http://example.com/ram", Some(200), 1, TrafficSource::Proxy);
    first.request.headers.clear();
    first.request.body = b"a=1".to_vec();
    first.summary.request_bytes = 3;
    let mut latest = first.request.clone();
    latest.body = b"&&&".to_vec();
    let mut pending = HttpHistoryPending {
        summary: first.summary.clone(),
        first_request_method: first.request.method.clone(),
        first_request_url: first.request.url.clone(),
        first_response_status: Some(200),
        first_request: Some(first.request.clone()),
        first_response: first.response.clone(),
        request: Some(latest),
        response: first.response.clone(),
        listener_port: None,
        capture_metrics: mio_proxy::CapturedFlowMetrics::default(),
    };
    let mut q = query(&store);
    q.filter.parameterized_only = true;
    let cancel = CancellationToken::new();
    let result = query_connection(
        &store.connection,
        &q,
        std::slice::from_ref(&pending),
        &cancel,
    )
    .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.rows[0].id(), HistoryEntryId::Live(first.summary.id));
    let mut persisted = first.clone();
    persisted.request.body = b"&&&".to_vec();
    store.put(&persisted).unwrap();
    assert_eq!(
        query_connection(
            &store.connection,
            &q,
            std::slice::from_ref(&pending),
            &cancel
        )
        .unwrap()
        .total,
        0,
        "persisted original request wins over a later recovery snapshot"
    );
    store
        .connection
        .execute(
            "DELETE FROM messages WHERE flow=?1",
            [first.summary.id.to_string()],
        )
        .unwrap();
    store
        .connection
        .execute(
            "DELETE FROM flows WHERE id=?1",
            [first.summary.id.to_string()],
        )
        .unwrap();
    store
        .connection
        .execute(
            "DELETE FROM metadata WHERE key=?1",
            [format!("http_history_meta:{}", first.summary.id)],
        )
        .unwrap();
    pending.first_request.as_mut().unwrap().body = b"&&&".to_vec();
    pending.request.as_mut().unwrap().body = b"a=1".to_vec();
    assert_eq!(
        query_connection(
            &store.connection,
            &q,
            std::slice::from_ref(&pending),
            &cancel
        )
        .unwrap()
        .total,
        0
    );
    pending.first_request = None;
    assert!(
        query_connection(
            &store.connection,
            &q,
            std::slice::from_ref(&pending),
            &cancel
        )
        .unwrap_err()
        .to_string()
        .contains("缺少原始请求")
    );
    pending.first_request_url = "http://example.com/ram?original=value".into();
    assert_eq!(
        query_connection(&store.connection, &q, &[pending], &cancel)
            .unwrap()
            .total,
        1
    );
}

#[test]
fn http_history_parameterized_only_filter_parser_limits_fail_without_partial_matches() {
    let mut store = ProjectStore::create(None).unwrap();
    let mut row = live(
        "http://example.com/large",
        Some(200),
        1,
        TrafficSource::Proxy,
    );
    row.request.headers.clear();
    row.request.body = (0..=crate::intruder::MAX_POSITIONS)
        .map(|index| format!("name{index}=value"))
        .collect::<Vec<_>>()
        .join("&")
        .into_bytes();
    row.summary.request_bytes = row.request.body.len() as u64;
    store.put(&row).unwrap();
    let mut q = query(&store);
    q.filter.parameterized_only = true;
    let error = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap_err();
    assert!(error.is::<crate::intruder::AutomaticParameterLimit>());
    let deep = format!(
        "POST / HTTP/1.1\r\nContent-Type: application/json\r\n\r\n{}1{}",
        "[".repeat(66),
        "]".repeat(66)
    );
    assert!(
        parsed_parameters(deep.as_bytes(), &CancellationToken::new())
            .unwrap_err()
            .is::<crate::intruder::AutomaticParameterLimit>()
    );
    let mut oversized =
        b"POST / HTTP/1.1\r\nContent-Type: application/octet-stream\r\n\r\n".to_vec();
    oversized.resize(crate::intruder::MAX_AUTO_BYTES + 1, b'x');
    assert!(
        parsed_parameters(&oversized, &CancellationToken::new())
            .unwrap_err()
            .is::<crate::intruder::AutomaticParameterLimit>()
    );
    assert!(
        parsed_parameters(
            b"POST / HTTP/1.1\r\nContent-Type: application/json\r\n\r\n{\"name\":}",
            &CancellationToken::new()
        )
        .is_err()
    );
    assert!(
        parsed_parameters(
            b"POST /gw/demo.search.upload/1.0/ HTTP/1.1\r\ncontent-type: application/x-www-form-urlencoded;charset=UTF-8\r\nContent-Type: application/x-www-form-urlencoded;charset=UTF-8\r\n\r\ndata=%7B%22count%22%3A%221%22%7D&type=originaljson",
            &CancellationToken::new()
        )
        .unwrap()
    );
    assert!(
        parsed_parameters(
            b"POST / HTTP/1.1\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Type: application/json\r\n\r\na=1",
            &CancellationToken::new()
        )
        .is_err()
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(parsed_parameters(b"GET /?name=1 HTTP/1.1\r\n\r\n", &cancel).is_err());
}
