use super::*;

#[test]
fn copy_url_and_curl_command_escape_quotes_newlines_and_binary_body() {
    let raw = b"POST /api/v1/item?name=a'b HTTP/1.1\r\nHost: 127.0.0.1:8080\r\nContent-Length: 999\r\nX-Note: hi'there\\ok\t!\r\n\r\na'b\r\n\x00\xff";
    let url = copy_url_for_request(raw, "http://127.0.0.1:8080", HttpVersion::Http11).unwrap();
    assert_eq!(url, "http://127.0.0.1:8080/api/v1/item?name=a'b");

    let command =
        curl_command_for_request(raw, "http://127.0.0.1:8080", HttpVersion::Http11).unwrap();
    assert_eq!(
        command,
        r"curl --path-as-is -i -s -k -X $'POST' -H $'Host: 127.0.0.1:8080' -H $'X-Note: hi\'there\\ok\t!' --data-binary $'a\'b\r\n\x00\xff' $'http://127.0.0.1:8080/api/v1/item?name=a\'b'"
    );
}

#[test]
fn redirect_resolution_handles_status_rewrites_relative_paths_and_cross_origin_headers() {
    let post_request = parse_repeater_request(
            b"POST /dir/sub/submit?old=1 HTTP/1.1\r\nHost: 127.0.0.1:8080\r\nContent-Type: text/plain\r\nAuthorization: Bearer secret\r\nCookie: sid=1\r\n\r\nhello",
            "http://127.0.0.1:8080",
            HttpVersion::Http11,
            true,
        )
        .unwrap();
    let response_302 = HttpResponse {
        status: 302,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![Header {
            name: "Location".into(),
            value: b"../next/step?ok=1#frag".to_vec(),
        }],
        body: Vec::new(),
        trailers: Vec::new(),
    };
    assert_eq!(
        redirect_location_url(&post_request.url, &response_302).as_deref(),
        Some("http://127.0.0.1:8080/dir/next/step?ok=1")
    );
    let (origin_302, next_302, _) = resolve_redirect_request(
        "http://127.0.0.1:8080",
        &post_request,
        &response_302,
        crate::RepeaterRedirectMode::OnSiteOnly,
    )
    .unwrap()
    .unwrap();
    assert_eq!(origin_302, "http://127.0.0.1:8080");
    assert_eq!(next_302.method, "GET");
    assert!(next_302.body.is_empty());
    assert!(
        !next_302
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("content-length")
                || h.name.eq_ignore_ascii_case("content-type"))
    );
    assert!(
        next_302
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("cookie"))
    );

    let cross_307 = HttpResponse {
        status: 307,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![Header {
            name: "Location".into(),
            value: b"https://localhost:8443/ keep".to_vec(),
        }],
        body: Vec::new(),
        trailers: Vec::new(),
    };
    assert!(redirect_location_url(&post_request.url, &cross_307).is_none());

    let cross_308 = HttpResponse {
        status: 308,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![Header {
            name: "Location".into(),
            value: b"https://localhost:8443/keep?x=1#section".to_vec(),
        }],
        body: Vec::new(),
        trailers: Vec::new(),
    };
    assert!(
        resolve_redirect_request(
            "http://127.0.0.1:8080",
            &post_request,
            &cross_308,
            crate::RepeaterRedirectMode::OnSiteOnly,
        )
        .unwrap()
        .is_none()
    );
    let (origin_308, next_308, _) = resolve_redirect_request(
        "http://127.0.0.1:8080",
        &post_request,
        &cross_308,
        crate::RepeaterRedirectMode::Always,
    )
    .unwrap()
    .unwrap();
    assert_eq!(origin_308, "https://localhost:8443");
    assert_eq!(next_308.method, "POST");
    assert_eq!(next_308.body, b"hello");
    assert!(
        !next_308
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("authorization")
                || h.name.eq_ignore_ascii_case("cookie"))
    );
    assert!(
        next_308
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("host") && h.value == b"localhost:8443")
    );

    let custom_cl_post = parse_repeater_request(
        b"POST /dir/sub/submit HTTP/1.1\r\nHost: 127.0.0.1:8080\r\nContent-Length: 99\r\n\r\ntest",
        "http://127.0.0.1:8080",
        HttpVersion::Http11,
        false,
    )
    .unwrap();
    for status in [307, 308] {
        let response = HttpResponse {
            status,
            version: HttpVersion::Http11,
            reason_phrase: None,
            headers: vec![Header {
                name: "Location".into(),
                value: b"/next".to_vec(),
            }],
            body: Vec::new(),
            trailers: Vec::new(),
        };
        let (_, next_req, _) = resolve_redirect_request(
            "http://127.0.0.1:8080",
            &custom_cl_post,
            &response,
            crate::RepeaterRedirectMode::Always,
        )
        .unwrap()
        .unwrap();
        assert_eq!(next_req.method, "POST");
        assert_eq!(next_req.body, b"test");
        assert!(
            next_req
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("content-length") && h.value == b"99")
        );
    }
}

#[test]
fn pretty_format_beautifies_json_and_form_bodies_and_rejects_binary() {
    assert_eq!(format_pretty_message(b"").unwrap(), "");

    let json_req = b"POST /api/items HTTP/1.1\r\nHost: example.com\r\nContent-Type: application/json\r\n\r\n{\"items\":[1,2],\"name\":\"mio\"}";
    assert_eq!(
        format_pretty_message(json_req).unwrap(),
        "POST /api/items HTTP/1.1\nHost: example.com\nContent-Type: application/json\n\n{\n  \"items\": [\n    1,\n    2\n  ],\n  \"name\": \"mio\"\n}"
    );

    let json_resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n[{\"ok\":true}]";
    assert_eq!(
        format_pretty_message(json_resp).unwrap(),
        "HTTP/1.1 200 OK\nContent-Type: application/json\n\n[\n  {\n    \"ok\": true\n  }\n]"
    );

    let chunked_json_resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/json;charset=UTF-8\r\nTransfer-Encoding: chunked\r\n\r\n1b\r\n{\"api\":\"demo\",\"ret\":[\"OK\"]}\r\n0\r\n\r\n";
    assert_eq!(
        format_pretty_message(chunked_json_resp).unwrap(),
        "HTTP/1.1 200 OK\nContent-Type: application/json;charset=UTF-8\nTransfer-Encoding: chunked\n\n{\n  \"api\": \"demo\",\n  \"ret\": [\n    \"OK\"\n  ]\n}"
    );

    let form_req = b"POST /login HTTP/1.1\r\nHost: example.com\r\nContent-Type: application/x-www-form-urlencoded; charset=utf-8\r\n\r\nuser=alice&role=admin&token=123";
    assert_eq!(
        format_pretty_message(form_req).unwrap(),
        "POST /login HTTP/1.1\nHost: example.com\nContent-Type: application/x-www-form-urlencoded; charset=utf-8\n\nuser=alice&role=admin&token=123"
    );

    let ws_json = b"{\"event\":\"ping\",\"seq\":42}";
    assert_eq!(
        format_pretty_message(ws_json).unwrap(),
        "{\n  \"event\": \"ping\",\n  \"seq\": 42\n}"
    );

    let err = format_pretty_message(b"HTTP/1.1 200 OK\r\n\r\n\xff\x00binary").unwrap_err();
    assert_eq!(
        err,
        "消息含非 UTF-8 字节，无法切换到 Pretty 视图；请使用 Hex 视图查看原始字节。"
    );
}
