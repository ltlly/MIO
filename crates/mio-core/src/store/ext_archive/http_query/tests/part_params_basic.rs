use super::*;

#[test]
fn http_history_parameterized_only_filter_matches_parsed_parameters_not_body_size() {
    let mut store = ProjectStore::create(None).unwrap();
    let mut plain_get = live(
        "http://example.com/plain",
        Some(200),
        10,
        TrafficSource::Proxy,
    );
    plain_get.request.body.clear();
    plain_get.summary.request_bytes = 0;

    let mut empty_query = live(
        "http://example.com/empty?#frag",
        Some(200),
        20,
        TrafficSource::Proxy,
    );
    empty_query.request.body.clear();
    empty_query.summary.request_bytes = 0;

    let mut with_query = live(
        "http://example.com/search?q=1#top",
        Some(200),
        30,
        TrafficSource::Proxy,
    );
    with_query.request.body.clear();
    with_query.summary.request_bytes = 0;

    let mut with_body = live(
        "http://example.com/submit",
        Some(200),
        40,
        TrafficSource::Proxy,
    );
    with_body.request.method = "POST".into();
    with_body.summary.method = "POST".into();
    with_body.request.headers.clear();
    with_body.request.trailers.clear();
    with_body.request.body = b"a=1".to_vec();
    with_body.summary.request_bytes = 3;

    for row in [&plain_get, &empty_query, &with_query, &with_body] {
        store.put(row).unwrap();
    }

    let mut q = query(&store);
    q.filter = HttpHistoryFilter {
        parameterized_only: true,
        ..Default::default()
    };
    let res = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(res.total, 2);
    let ids: Vec<_> = res.rows.iter().map(HistoryRow::id).collect();
    assert!(ids.contains(&HistoryEntryId::Live(with_query.summary.id)));
    assert!(ids.contains(&HistoryEntryId::Live(with_body.summary.id)));

    let cases: &[(&str, Option<&str>, &[u8], bool)] = &[
        ("/separators?&&", None, b"", false),
        ("/empty-value?name=", None, b"", true),
        ("/invalid-query?name=%ZZ", None, b"", true),
        ("/bare-json", None, br#"{"name":"value"}"#, true),
        ("/bare-empty-array", None, b"[]", false),
        ("/bare-text", None, b"plain text", true),
        ("/bare-space", None, b" ", true),
        (
            "/binary",
            Some("application/octet-stream"),
            b"\0\xffraw",
            true,
        ),
        ("/binary-form", Some("application/octet-stream"), b"name=value", true),
        ("/binary-separators", Some("application/octet-stream"), b"&&", false),
        ("/empty-json", Some("application/json"), b"{}", false),
        ("/empty-array", Some("application/json"), b"[]", false),
        ("/json-array", Some("application/json"), br#"["value"]"#, true),
        ("/json-empty-child-array", Some("application/json"), br#"{"a":[]}"#, false),
        ("/json-empty-child-object", Some("application/json"), br#"{"a":{}}"#, false),
        ("/json-number", Some("application/json"), b"1", false),
        ("/json-string", Some("application/json"), br#""value""#, false),
        ("/json-null", Some("application/json"), b"null", false),
        (
            "/form",
            Some("application/x-www-form-urlencoded"),
            b"name=value",
            true,
        ),
        ("/unknown-mime-form", Some("foo/bar"), b"name=value", true),
        (
            "/empty-form",
            Some("application/x-www-form-urlencoded"),
            b"&&",
            false,
        ),
        (
            "/xml",
            Some("application/xml"),
            b"<root><name>value</name></root>",
            true,
        ),
        ("/empty-xml", Some("application/xml"), b"<root/>", false),
        (
            "/multipart",
            Some("multipart/form-data; boundary=test"),
            b"--test\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nvalue\r\n--test--\r\n",
            true,
        ),
        (
            "/multipart-file",
            Some("multipart/form-data; boundary=test"),
            b"--test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"x.txt\"\r\n\r\nfilebytes\r\n--test--\r\n",
            true,
        ),
        (
            "/multipart-empty-field",
            Some("multipart/form-data; boundary=test"),
            b"--test\r\nContent-Disposition: form-data; name=\"field\"\r\n\r\n\r\n--test--\r\n",
            true,
        ),
        (
            "/multipart-no-parts",
            Some("multipart/form-data; boundary=test"),
            b"--test--\r\n",
            false,
        ),
        ("/text", Some("text/plain"), b"plain text", true),
        ("/text-json", Some("text/plain"), br#"{"a":1}"#, true),
        ("/text-separators", Some("text/plain"), b"&&", false),
        ("/text-empty", Some("text/plain"), b"", false),
        ("/text-empty-value", Some("text/plain"), b"name=", true),
        ("/text-invalid-percent", Some("text/plain"), b"name=%ZZ", true),
    ];
    let mut expected: HashSet<_> = ids.into_iter().collect();
    for (index, (path, content_type, body, matched)) in cases.iter().enumerate() {
        let mut row = live(
            &format!("http://example.com{path}"),
            Some(200),
            50 + index as i64,
            TrafficSource::Proxy,
        );
        row.request.headers.clear();
        row.request.trailers.clear();
        if let Some(content_type) = content_type {
            row.request.headers.push(crate::Header {
                name: "Content-Type".into(),
                value: content_type.as_bytes().to_vec(),
            });
        }
        row.request.body = body.to_vec();
        row.summary.request_bytes = body.len() as u64;
        store.put(&row).unwrap();
        if *matched {
            expected.insert(HistoryEntryId::Live(row.summary.id));
        }
    }
    let parsed = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(
        parsed
            .rows
            .iter()
            .map(HistoryRow::id)
            .collect::<HashSet<_>>(),
        expected
    );
    assert_eq!(parsed.total, expected.len());
    q.selected_ids = parsed.rows.iter().map(HistoryRow::id).collect();
    q.limit = 2;
    let mut pages = Vec::new();
    for offset in (0..parsed.total).step_by(q.limit) {
        q.offset = offset;
        let page = store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap();
        assert_eq!(page.total, parsed.total);
        assert_eq!(page.selected_ranks.len(), parsed.total);
        assert!(page.selected_matches.iter().all(|(_, matched)| *matched));
        pages.extend(page.rows);
    }
    assert_eq!(pages, parsed.rows);
}

#[test]
fn http_history_parameterized_only_filter_uses_original_live_request() {
    let mut store = ProjectStore::create(None).unwrap();
    let mut originals = Vec::new();
    for (index, body) in [b"a=1".as_slice(), b"&&&"].into_iter().enumerate() {
        let mut original = live(
            "http://example.com/original",
            Some(200),
            index as i64,
            TrafficSource::Proxy,
        );
        original.request.body = body.to_vec();
        original.request.headers.clear();
        original.request.trailers.clear();
        original.summary.request_bytes = body.len() as u64;
        store.put(&original).unwrap();
        let mut edited = original.clone();
        edited.summary.revision += 1;
        edited.request.body = if index == 0 {
            b"&&&".to_vec()
        } else {
            b"a=1".to_vec()
        };
        edited.request.url = if index == 0 {
            "http://example.com/edited"
        } else {
            "http://example.com/edited?added=1"
        }
        .into();
        edited.summary.url = edited.request.url.clone();
        store.put(&edited).unwrap();
        originals.push(original.summary.id);
    }
    let mut q = query(&store);
    q.filter.parameterized_only = true;
    q.selected_ids = originals
        .iter()
        .copied()
        .map(HistoryEntryId::Live)
        .collect();
    let result = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.rows[0].id(), q.selected_ids[0]);
    assert_eq!(
        result.selected_matches,
        [(q.selected_ids[0], true), (q.selected_ids[1], false)]
    );
}

#[test]
fn http_history_parameterized_only_filter_uses_original_archive_request() {
    for original_parameterized in [false, true] {
        let (store, source, items) =
            super::super::super::proxy_history::tests::fixture(0, true, false);
        let state = source_state(&store.connection, source).unwrap();
        let mut bytes = read_chunk(&store.connection, source, state.source_bytes, 0).unwrap();
        let mut statement = store
            .connection
            .prepare("SELECT summary FROM ext_messages WHERE field_id IN (15,17)")
            .unwrap();
        let messages: Vec<ExtMessageSummary> = statement
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .unwrap()
            .map(|row| serde_json::from_slice(&row.unwrap()).unwrap())
            .collect();
        drop(statement);
        for message in messages {
            let is_original = message.field_id == 15;
            let start = (message.raw_offset + message.body_offset.unwrap()) as usize;
            bytes[start..start + 3].copy_from_slice(if is_original == original_parameterized {
                b"a=1"
            } else {
                b"&&&"
            });
        }
        let (codec, encoded) = super::super::super::encode_chunk(&bytes).unwrap();
        store
            .connection
            .execute(
                "UPDATE ext_source_chunks SET codec=?1,data=?2 WHERE source=?3",
                params![codec, encoded, source.to_string()],
            )
            .unwrap();
        let mut q = query(&store);
        q.filter.parameterized_only = true;
        q.selected_ids = vec![HistoryEntryId::External {
            source_id: source,
            item_id: items[0].id,
        }];
        let result = store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap();
        assert_eq!(result.total, if original_parameterized { 2 } else { 0 });
        assert_eq!(
            result.selected_matches,
            [(q.selected_ids[0], original_parameterized)]
        );
    }
}

#[test]
fn http_history_parameterized_only_filter_query_match_does_not_read_bodies() {
    let mut store = ProjectStore::create(None).unwrap();
    let row = live(
        "http://example.com/query?name=",
        Some(200),
        1,
        TrafficSource::Proxy,
    );
    store.put(&row).unwrap();
    store.connection.authorizer(Some(no_bodies));
    let mut q = query(&store);
    q.filter.parameterized_only = true;
    let result = store
        .query_http_history(&q, &CancellationToken::new())
        .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.rows[0].id(), HistoryEntryId::Live(row.summary.id));
}

#[test]
fn http_history_parameterized_only_filter_generic_body_is_read_and_binary_bytes_are_retained() {
    let mut store = ProjectStore::create(None).unwrap();
    let mut row = live(
        "http://example.com/binary",
        Some(200),
        1,
        TrafficSource::Proxy,
    );
    row.request.headers = vec![crate::Header {
        name: "Content-Type".into(),
        value: b"application/octet-stream".to_vec(),
    }];
    row.request.body = vec![0, 255, b'x'];
    row.summary.request_bytes = 3;
    store.put(&row).unwrap();
    store.connection.authorizer(Some(no_bodies));
    let mut q = query(&store);
    q.filter.parameterized_only = true;
    assert!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .is_err(),
        "unknown/binary content types require their original body to classify parameters"
    );
    store
        .connection
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>);
    assert_eq!(
        store
            .query_http_history(&q, &CancellationToken::new())
            .unwrap()
            .total,
        1
    );
    assert_eq!(store.read(row.summary.id).unwrap(), row);
}
