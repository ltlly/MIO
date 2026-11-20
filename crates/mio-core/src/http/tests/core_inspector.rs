use super::*;

#[test]
fn inspector_parses_and_mutates_attributes_params_cookies_and_headers_losslessly() {
    let request = b"POST /api/v1/items?tag=a&tag=%E4%B8%AD&flag HTTP/1.1\r\nHost: example.com\r\nContent-Type: application/x-www-form-urlencoded; charset=utf-8\r\nContent-Length: 19\r\nCookie: sid=1; theme=dark\r\nCookie: lang=zh-CN\r\nX-Dup: first\r\nX-Dup: second\r\n\r\nuser=alice&role=dev";
    let response =
        b"HTTP/1.1 200 OK\r\nSet-Cookie: sid=1\r\nSet-Cookie: theme=dark\r\nX-Resp: ok\r\n\r\nbody";
    let inspected = inspect_http_messages(request, Some(response));
    assert_eq!(
        inspected.attributes,
        vec![
            InspectorItem {
                name: "Method".into(),
                value: "POST".into(),
            },
            InspectorItem {
                name: "Path".into(),
                value: "/api/v1/items".into(),
            },
            InspectorItem {
                name: "Protocol".into(),
                value: "HTTP/1.1".into(),
            },
        ]
    );
    assert_eq!(
        inspected.query_params,
        vec![
            InspectorItem {
                name: "tag".into(),
                value: "a".into(),
            },
            InspectorItem {
                name: "tag".into(),
                value: "%E4%B8%AD".into(),
            },
            InspectorItem {
                name: "flag".into(),
                value: "".into(),
            },
        ]
    );
    assert_eq!(
        inspected.body_params,
        vec![
            InspectorItem {
                name: "user".into(),
                value: "alice".into(),
            },
            InspectorItem {
                name: "role".into(),
                value: "dev".into(),
            },
        ]
    );
    assert_eq!(
        inspected.cookies,
        vec![
            InspectorItem {
                name: "sid".into(),
                value: "1".into(),
            },
            InspectorItem {
                name: "theme".into(),
                value: "dark".into(),
            },
            InspectorItem {
                name: "lang".into(),
                value: "zh-CN".into(),
            },
        ]
    );
    assert_eq!(inspected.request_headers.len(), 7);
    assert_eq!(
        inspected.response_headers,
        vec![
            InspectorItem {
                name: "Set-Cookie".into(),
                value: "sid=1".into(),
            },
            InspectorItem {
                name: "Set-Cookie".into(),
                value: "theme=dark".into(),
            },
            InspectorItem {
                name: "X-Resp".into(),
                value: "ok".into(),
            },
        ]
    );

    let mut raw = apply_inspector_mutation(
        request,
        InspectorMutation::Update {
            section: InspectorSection::RequestAttributes,
            index: 0,
            name: "Method".into(),
            value: "PUT".into(),
        },
    )
    .unwrap();
    raw = apply_inspector_mutation(
        &raw,
        InspectorMutation::Update {
            section: InspectorSection::RequestAttributes,
            index: 1,
            name: "Path".into(),
            value: "/api/v2/items".into(),
        },
    )
    .unwrap();
    raw = apply_inspector_mutation(
        &raw,
        InspectorMutation::Remove {
            section: InspectorSection::QueryParams,
            index: 2,
        },
    )
    .unwrap();
    raw = apply_inspector_mutation(
        &raw,
        InspectorMutation::Add {
            section: InspectorSection::QueryParams,
            name: "page".into(),
            value: "2".into(),
        },
    )
    .unwrap();
    raw = apply_inspector_mutation(
        &raw,
        InspectorMutation::Update {
            section: InspectorSection::BodyParams,
            index: 1,
            name: "role".into(),
            value: "admin".into(),
        },
    )
    .unwrap();
    raw = apply_inspector_mutation(
        &raw,
        InspectorMutation::Add {
            section: InspectorSection::BodyParams,
            name: "extra".into(),
            value: "1".into(),
        },
    )
    .unwrap();
    raw = apply_inspector_mutation(
        &raw,
        InspectorMutation::Remove {
            section: InspectorSection::Cookies,
            index: 1,
        },
    )
    .unwrap();
    raw = apply_inspector_mutation(
        &raw,
        InspectorMutation::Add {
            section: InspectorSection::Cookies,
            name: "mode".into(),
            value: "pro".into(),
        },
    )
    .unwrap();
    raw = apply_inspector_mutation(
        &raw,
        InspectorMutation::Update {
            section: InspectorSection::RequestHeaders,
            index: 5,
            name: "X-Dup".into(),
            value: "updated".into(),
        },
    )
    .unwrap();
    assert_eq!(
            raw,
            b"PUT /api/v2/items?tag=a&tag=%E4%B8%AD&page=2 HTTP/1.1\r\nHost: example.com\r\nContent-Type: application/x-www-form-urlencoded; charset=utf-8\r\nContent-Length: 29\r\nCookie: sid=1; lang=zh-CN; mode=pro\r\nX-Dup: first\r\nX-Dup: updated\r\n\r\nuser=alice&role=admin&extra=1"
        );

    let binary_lf = b"POST /bin?q=1 HTTP/1.1\nHost: example.com\nX-Custom: a\n\n\x00\xff\xfe";
    let mutated_binary = apply_inspector_mutation(
        binary_lf,
        InspectorMutation::Add {
            section: InspectorSection::RequestHeaders,
            name: "X-Added".into(),
            value: "yes".into(),
        },
    )
    .unwrap();
    assert_eq!(
        mutated_binary,
        b"POST /bin?q=1 HTTP/1.1\nHost: example.com\nX-Custom: a\nX-Added: yes\n\n\x00\xff\xfe"
    );
    assert!(
        apply_inspector_mutation(
            request,
            InspectorMutation::Remove {
                section: InspectorSection::ResponseHeaders,
                index: 0,
            },
        )
        .is_err()
    );
}

#[test]
fn match_and_replace_rules_modify_request_and_response_headers_and_bodies() {
    use mio_plugin_api::{FlowState, FlowSummary, TrafficSource, Uuid};

    let mut record = FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: "POST".into(),
            url: "https://example.com/api".into(),
            status: Some(200),
            state: FlowState::Complete,
            request_bytes: 9,
            response_bytes: 11,
            started_unix_ms: 0,
            elapsed_ms: 1,
        },
        request: HttpRequest {
            version: HttpVersion::Http11,
            method: "POST".into(),
            url: "https://example.com/api".into(),
            headers: vec![
                Header {
                    name: "Host".into(),
                    value: b"example.com".to_vec(),
                },
                Header {
                    name: "User-Agent".into(),
                    value: b"OldAgent/1.0".to_vec(),
                },
                Header {
                    name: "X-Remove-Me".into(),
                    value: b"secret".to_vec(),
                },
                Header {
                    name: "X-Binary".into(),
                    value: b"prefix-\xff\xfe".to_vec(),
                },
                Header {
                    name: "Content-Length".into(),
                    value: b"9".to_vec(),
                },
            ],
            body: b"role=user".to_vec(),
            trailers: Vec::new(),
        },
        response: Some(HttpResponse {
            version: HttpVersion::Http11,
            status: 200,
            reason_phrase: None,
            headers: vec![
                Header {
                    name: "Server".into(),
                    value: b"Apache".to_vec(),
                },
                Header {
                    name: "Content-Length".into(),
                    value: b"11".to_vec(),
                },
            ],
            body: b"hello world".to_vec(),
            trailers: Vec::new(),
        }),
    };

    let rules = vec![
        MatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            item_type: MatchReplaceTarget::RequestHeader,
            match_pattern: "User-Agent: OldAgent/1.0".into(),
            replace_with: "User-Agent: MIO/2.0".into(),
            regex: false,
            comment: String::new(),
            ..Default::default()
        },
        MatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            item_type: MatchReplaceTarget::RequestHeader,
            match_pattern: "^X-Remove-Me:.*$".into(),
            replace_with: String::new(),
            regex: true,
            comment: String::new(),
            ..Default::default()
        },
        MatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            item_type: MatchReplaceTarget::RequestHeader,
            match_pattern: "X-Binary: prefix-".into(),
            replace_with: "X-Binary: replaced-".into(),
            regex: false,
            comment: String::new(),
            ..Default::default()
        },
        MatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            item_type: MatchReplaceTarget::RequestHeader,
            match_pattern: String::new(),
            replace_with: "X-Injected: enabled".into(),
            regex: false,
            comment: String::new(),
            ..Default::default()
        },
        MatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            item_type: MatchReplaceTarget::RequestBody,
            match_pattern: r"role=(\w+)".into(),
            replace_with: "role=administrator".into(),
            regex: true,
            comment: String::new(),
            ..Default::default()
        },
        MatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            item_type: MatchReplaceTarget::ResponseHeader,
            match_pattern: "Server: Apache".into(),
            replace_with: "Server: MIO-Edge".into(),
            regex: false,
            comment: String::new(),
            ..Default::default()
        },
        MatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            item_type: MatchReplaceTarget::ResponseBody,
            match_pattern: "world".into(),
            replace_with: "proxies".into(),
            regex: false,
            comment: String::new(),
            ..Default::default()
        },
    ];

    assert!(apply_match_and_replace(&mut record, false, false, &rules));
    assert_eq!(
        record.request.headers,
        vec![
            Header {
                name: "Host".into(),
                value: b"example.com".to_vec(),
            },
            Header {
                name: "User-Agent".into(),
                value: b"MIO/2.0".to_vec(),
            },
            Header {
                name: "X-Binary".into(),
                value: b"prefix-\xff\xfe".to_vec(),
            },
            Header {
                name: "Content-Length".into(),
                value: b"18".to_vec(),
            },
            Header {
                name: "X-Injected".into(),
                value: b"enabled".to_vec(),
            },
        ]
    );
    assert_eq!(record.request.body, b"role=administrator");
    assert_eq!(record.summary.request_bytes, 18);

    assert!(apply_match_and_replace(&mut record, true, true, &rules));
    let response = record.response.as_ref().unwrap();
    assert_eq!(response.headers[0].value, b"MIO-Edge");
    assert_eq!(response.body, b"hello world");
    assert_eq!(record.summary.response_bytes, 11);

    assert!(apply_match_and_replace(&mut record, true, false, &rules));
    let response = record.response.as_ref().unwrap();
    assert_eq!(response.body, b"hello proxies");
    assert_eq!(response.headers[1].value, b"13");
    assert_eq!(record.summary.response_bytes, 13);

    record.request.url = "https://example.com/api/v1?user=guest&token=old".into();
    record.summary.url = record.request.url.clone();
    record.request.headers.push(Header {
        name: "Content-Type".into(),
        value: b"application/x-www-form-urlencoded".to_vec(),
    });
    record.request.body = b"user=guest&role=member".to_vec();
    record.summary.request_bytes = record.request.body.len() as u64;
    let extended_rules = vec![
        MatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            item_type: MatchReplaceTarget::RequestParamName,
            match_pattern: "user".into(),
            replace_with: "account".into(),
            ..Default::default()
        },
        MatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            item_type: MatchReplaceTarget::RequestParamValue,
            match_pattern: "guest".into(),
            replace_with: "admin".into(),
            ..Default::default()
        },
        MatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            item_type: MatchReplaceTarget::RequestFirstLine,
            match_pattern: "POST /api/v1".into(),
            replace_with: "PUT /api/v2".into(),
            ..Default::default()
        },
        MatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            item_type: MatchReplaceTarget::RequestScript,
            filter_script: r#"
                request.set_header("X-Digest", sha256_hex("abc"));
                request.set_query_param("sig", base64_encode(url_decode("a%20b")));
                request.body = regex_replace(request.body, "member", "owner");
                request
            "#
            .into(),
            ..Default::default()
        },
        MatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            item_type: MatchReplaceTarget::ResponseScript,
            filter_script: r#"
                response.status = 201;
                response.headers["X-Frame-Options"] = "DENY";
                response.body = "created";
                response
            "#
            .into(),
            ..Default::default()
        },
    ];
    assert!(apply_match_and_replace(&mut record, false, false, &extended_rules));
    assert_eq!(record.request.method, "PUT");
    assert_eq!(
        record.request.url,
        "https://example.com/api/v2?account=admin&token=old&sig=YSBi"
    );
    assert_eq!(record.request.body, b"account=admin&role=owner");
    assert_eq!(record.summary.request_bytes, 24);
    assert!(record.request.headers.iter().any(|h| {
        h.name.eq_ignore_ascii_case("x-digest")
            && h.value == b"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    }));
    assert!(apply_match_and_replace(&mut record, true, false, &extended_rules));
    let resp = record.response.as_ref().unwrap();
    assert_eq!(resp.status, 201);
    assert_eq!(resp.body, b"created");
    assert_eq!(record.summary.response_bytes, 7);
    assert!(
        resp.headers
            .iter()
            .any(|h| h.name == "X-Frame-Options" && h.value == b"DENY")
    );

    let preview_req = test_match_and_replace_rule(
        &extended_rules[3],
        b"POST /test?x=1 HTTP/1.1\r\nHost: example.com\r\nContent-Length: 6\r\n\r\nmember",
        false,
    )
    .unwrap();
    let preview_str = String::from_utf8(preview_req).unwrap();
    assert!(preview_str.contains("X-Digest: ba7816bf"));
    assert!(preview_str.contains("Content-Length: 5\r\n"));
    assert!(preview_str.ends_with("\r\n\r\nowner"));
    assert!(
        test_match_and_replace_rule(
            &MatchAndReplaceRule {
                item_type: MatchReplaceTarget::RequestScript,
                filter_script: "let broken = ;".into(),
                ..Default::default()
            },
            b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n",
            false,
        )
        .is_err()
    );

    let cancel = tokio_util::sync::CancellationToken::new();
    let mio_api_script = r#"
        return requestResponse.request().isInScope()
            && requestResponse.request().hasParameters()
            && requestResponse.request().hasParameter("x")
            && requestResponse.request().hasParameter("a")
            && !requestResponse.request().hasParameter("missing")
            && requestResponse.request().parameterValue("x") == "1"
            && requestResponse.request().parameterValue("a") == "1"
            && requestResponse.request().method() == "POST"
            && requestResponse.request().path() == "/api/v1?x=1"
            && requestResponse.request().pathWithoutQuery() == "/api/v1"
            && requestResponse.path() == "/api/v1?x=1"
            && requestResponse.request().url() == "https://example.com/api/v1?x=1"
            && requestResponse.hasResponse()
            && requestResponse.response().statusCode() == 200
            && requestResponse.mimeType() == "HTML"
            && requestResponse.listenerPort() == 8080
            && requestResponse.annotations().hasNotes()
            && requestResponse.annotations().notes() == "important note"
            && requestResponse.annotations().hasHighlightColor()
            && requestResponse.annotations().highlightColor() == "ORANGE"
            && requestResponse.contains("hello world", false)
            && utilities.base64Encode("ok") == "b2s=";
    "#;
    assert!(
        crate::http::eval_http_filter_script_with_highlight_color_and_cancel(
            mio_api_script,
            "POST",
            "https://example.com/api/v1?x=1",
            Some(200),
            64,
            Some(b"POST /api/v1?x=1 HTTP/1.1\r\nHost: example.com\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\na=1"),
            Some(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html>Hello World</html>"),
            "important note",
            2,
            Some(8080),
            true,
            &cancel,
        )
        .unwrap()
    );
    assert!(
        crate::http::eval_http_filter_script_with_highlight_color_and_cancel(
            r#"return requestResponse.mimeType() == "JSON" && !requestResponse.annotations().hasHighlightColor() && requestResponse.annotations().highlightColor() == "NONE";"#,
            "GET",
            "https://example.com/data.json",
            Some(200),
            16,
            Some(b"GET /data.json HTTP/1.1\r\nHost: example.com\r\n\r\n"),
            Some(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"ok\":true}"),
            "",
            0,
            Some(8080),
            false,
            &cancel,
        )
        .unwrap()
    );
    assert!(
        crate::http::eval_http_filter_script_with_scope_and_cancel(
            "return requestResponse.annotations().hasHighlightColor();",
            "GET",
            "https://example.com/",
            Some(200),
            0,
            None,
            None,
            "",
            true,
            Some(8080),
            false,
            &cancel,
        )
        .unwrap()
    );
    assert!(
        crate::http::eval_http_filter_script_with_scope_and_cancel(
            r#"return requestResponse.annotations().highlightColor() == "RED";"#,
            "GET",
            "https://example.com/",
            Some(200),
            0,
            None,
            None,
            "",
            true,
            Some(8080),
            false,
            &cancel,
        )
        .is_err()
    );
    assert!(
        crate::http::eval_http_filter_script_with_highlight_color_and_cancel(
            "return requestResponse.listenerPort() == 0;",
            "GET",
            "https://example.com/",
            Some(200),
            0,
            None,
            None,
            "",
            0,
            None,
            false,
            &cancel,
        )
        .is_err()
    );
}

#[test]
fn request_line_version_helpers_and_response_header_mutations_preserve_newlines_and_binary_body() {
    let crlf_req = b"POST /upload?x=1 HTTP/1.1\r\nHost: example.com\r\n\r\n\x00\xff\xfe";
    assert_eq!(
        detect_request_line_version(crlf_req),
        Some(HttpVersion::Http11)
    );
    let h2_req = rewrite_request_line_version(crlf_req, HttpVersion::Http2);
    assert_eq!(
        h2_req,
        b"POST /upload?x=1 HTTP/2\r\nHost: example.com\r\n\r\n\x00\xff\xfe"
    );
    assert_eq!(
        detect_request_line_version(&h2_req),
        Some(HttpVersion::Http2)
    );
    assert_eq!(
        detect_request_line_version(b"GET /legacy HTTP/2.0\nHost: example.com\n\n"),
        Some(HttpVersion::Http2)
    );
    let lf_back = rewrite_request_line_version(
        b"GET /legacy HTTP/2.0\nHost: example.com\n\n\x89PNG",
        HttpVersion::Http11,
    );
    assert_eq!(
        lf_back,
        b"GET /legacy HTTP/1.1\nHost: example.com\n\n\x89PNG"
    );

    let resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nX-Old: 1\r\n\r\n\x00\xff\xaa";
    let added = apply_inspector_response_mutation(
        resp,
        InspectorMutation::Add {
            section: InspectorSection::ResponseHeaders,
            name: "X-Added".into(),
            value: "yes".into(),
        },
    )
    .unwrap();
    let updated = apply_inspector_response_mutation(
        &added,
        InspectorMutation::Update {
            section: InspectorSection::ResponseHeaders,
            index: 1,
            name: "X-Old".into(),
            value: "2".into(),
        },
    )
    .unwrap();
    let removed = apply_inspector_response_mutation(
        &updated,
        InspectorMutation::Remove {
            section: InspectorSection::ResponseHeaders,
            index: 0,
        },
    )
    .unwrap();
    assert_eq!(
        removed,
        b"HTTP/1.1 200 OK\r\nX-Old: 2\r\nX-Added: yes\r\n\r\n\x00\xff\xaa"
    );
    assert!(
        apply_inspector_response_mutation(
            resp,
            InspectorMutation::Add {
                section: InspectorSection::QueryParams,
                name: "a".into(),
                value: "b".into(),
            },
        )
        .is_err()
    );
}
