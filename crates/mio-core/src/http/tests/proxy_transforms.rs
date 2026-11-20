use super::*;
use crate::{
    ProxyMessageSettings, ProxyResponseModificationSettings, SslPassThroughRule, Uuid,
    WsMatchAndReplaceRule, WsReplaceDirection,
};
use mio_plugin_api::{WebSocketDirection, WebSocketKind};

#[test]
fn html_form_response_modification_unhides_enables_highlights_and_skips_non_html() {
    let cancel = CancellationToken::new();
    let limits = (std::time::Instant::now() + Duration::from_secs(3), MAX_BODY);
    let raw_html = br#"<html><body><form action="https://bank.example/submit">
<input type="hidden" name="csrf" value="abc123" disabled>
<button type="submit" disabled="disabled">Pay</button>
<select name="tier" disabled><option value="1">Pro</option></select>
</form></body></html>"#;

    let mut record = interception_record();
    let resp = record.response.as_mut().unwrap();
    resp.headers = vec![
        Header {
            name: "Content-Type".into(),
            value: b"text/html; charset=utf-8".to_vec(),
        },
        Header {
            name: "Set-Cookie".into(),
            value: b"sid=1; Secure; HttpOnly".to_vec(),
        },
        Header {
            name: "Content-Security-Policy".into(),
            value: b"default-src 'self'".to_vec(),
        },
        Header {
            name: "Content-Length".into(),
            value: raw_html.len().to_string().into_bytes(),
        },
    ];
    resp.body = raw_html.to_vec();
    record.summary.response_bytes = raw_html.len() as u64;
    let original_snapshot = record.clone();

    let transform = build_flow_transform(
        Vec::new(),
        ProxyMessageSettings {
            unpack_compressed_requests: false,
            unpack_compressed_responses: false,
            response_modification: ProxyResponseModificationSettings {
                unhide_hidden_form_fields: true,
                enable_disabled_form_fields: true,
                highlight_unhidden_fields: true,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap()
    .unwrap();

    assert!(transform(&mut record, true, false, &cancel, limits).unwrap());
    assert_eq!(original_snapshot.response.as_ref().unwrap().body, raw_html);

    let updated = record.response.as_ref().unwrap();
    let modified_html = std::str::from_utf8(&updated.body).unwrap();
    assert!(modified_html.contains("type=\"text\" data-mio-unhidden=\"true\""));
    assert!(modified_html.contains("border: 2px dashed #d97706"));
    assert!(!modified_html.contains("disabled"));
    // Security boundaries: HTTPS URLs, Cookie Secure/HttpOnly, and CSP remain untouched.
    assert!(modified_html.contains("https://bank.example/submit"));
    assert!(
        updated
            .headers
            .iter()
            .any(|h| h.name == "Set-Cookie" && h.value == b"sid=1; Secure; HttpOnly")
    );
    assert!(
        updated
            .headers
            .iter()
            .any(|h| h.name == "Content-Security-Policy" && h.value == b"default-src 'self'")
    );
    let expected_cl = updated.body.len().to_string().into_bytes();
    assert!(
        updated
            .headers
            .iter()
            .any(|h| h.name == "Content-Length" && h.value == expected_cl)
    );

    // Non-HTML JSON response is safely skipped without mutating bytes.
    let mut json_record = interception_record();
    let json_resp = json_record.response.as_mut().unwrap();
    json_resp.headers = vec![Header {
        name: "Content-Type".into(),
        value: b"application/json".to_vec(),
    }];
    json_resp.body = br#"{"html":"<input type=\"hidden\" disabled>"}"#.to_vec();
    let json_before = json_record.clone();
    assert!(!transform(&mut json_record, true, false, &cancel, limits).unwrap());
    assert_eq!(json_record, json_before);

    let full_html = br#"<html><body>
<script>alert(1)</script>
<object data="movie.swf"></object>
<embed src="legacy.swf">
<form action="https://bank.example/pay" onsubmit="return check()">
<input name="pin" maxlength="4" minlength="2" required pattern="\d+" onchange="v()" oninput="i()">
<textarea maxlength="100" onblur="b()"></textarea>
<a href="javascript:evil()" onclick="c()">Link</a>
</form></body></html>"#;
    let mut full_record = interception_record();
    let full_resp = full_record.response.as_mut().unwrap();
    full_resp.headers = vec![
        Header {
            name: "Content-Type".into(),
            value: b"text/html".to_vec(),
        },
        Header {
            name: "Location".into(),
            value: b"https://bank.example/login".to_vec(),
        },
        Header {
            name: "Set-Cookie".into(),
            value: b"sid=1; Path=/; Secure; HttpOnly".to_vec(),
        },
        Header {
            name: "Content-Length".into(),
            value: full_html.len().to_string().into_bytes(),
        },
    ];
    full_resp.body = full_html.to_vec();
    full_record.summary.response_bytes = full_html.len() as u64;
    let full_transform = build_flow_transform(
        Vec::new(),
        ProxyMessageSettings {
            unpack_compressed_requests: false,
            unpack_compressed_responses: false,
            response_modification: ProxyResponseModificationSettings {
                remove_input_field_length_limits: true,
                remove_javascript_form_validation: true,
                remove_all_javascript: true,
                remove_object_tags: true,
                convert_https_links_to_http: true,
                remove_secure_flag_from_cookies: true,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap()
    .unwrap();
    assert!(full_transform(&mut full_record, true, false, &cancel, limits).unwrap());
    let out_resp = full_record.response.as_ref().unwrap();
    let out_html = std::str::from_utf8(&out_resp.body).unwrap();
    assert!(!out_html.contains("<script"));
    assert!(!out_html.contains("<object"));
    assert!(!out_html.contains("<embed"));
    assert!(!out_html.contains("maxlength"));
    assert!(!out_html.contains("minlength"));
    assert!(!out_html.contains("required"));
    assert!(!out_html.contains("pattern="));
    assert!(!out_html.contains("onsubmit"));
    assert!(!out_html.contains("onchange"));
    assert!(!out_html.contains("onclick"));
    assert!(!out_html.contains("javascript:"));
    assert!(out_html.contains("http://bank.example/pay"));
    assert!(
        out_resp
            .headers
            .iter()
            .any(|h| h.name == "Location" && h.value == b"http://bank.example/login")
    );
    assert!(
        out_resp
            .headers
            .iter()
            .any(|h| h.name == "Set-Cookie" && h.value == b"sid=1; Path=/; HttpOnly")
    );
}

#[test]
fn websocket_frame_match_and_replace_rewrites_text_per_direction_and_skips_binary() {
    let cancel = CancellationToken::new();
    let limits = (std::time::Instant::now() + Duration::from_secs(3), MAX_BODY);
    let upgrade = HttpRequest {
        method: "GET".into(),
        url: "wss://chat.example.com/ws".into(),
        version: HttpVersion::Http11,
        headers: Vec::new(),
        trailers: Vec::new(),
        body: Vec::new(),
    };
    let ws_transform = build_websocket_transform(vec![
        WsMatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            direction: WsReplaceDirection::ClientToServer,
            match_pattern: r#"role":"guest""#.into(),
            replace_with: r#"role":"tester""#.into(),
            is_regex: false,
            comment: "elevate debug role".into(),
        },
        WsMatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            direction: WsReplaceDirection::ServerToClient,
            match_pattern: r"otp=(\d{6})".into(),
            replace_with: "otp=[captured:$1]".into(),
            is_regex: true,
            comment: "tag otp".into(),
        },
        WsMatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            direction: WsReplaceDirection::Both,
            match_pattern: "env=prod".into(),
            replace_with: "env=staging".into(),
            is_regex: false,
            comment: "both legs".into(),
        },
    ])
    .unwrap()
    .unwrap();

    let c2s_in = br#"{"role":"guest","meta":"env=prod","msg":"otp=123456"}"#;
    let c2s_out = ws_transform(
        &upgrade,
        WebSocketDirection::ClientToServer,
        WebSocketKind::Text,
        c2s_in,
        &cancel,
        limits,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        std::str::from_utf8(&c2s_out).unwrap(),
        r#"{"role":"tester","meta":"env=staging","msg":"otp=123456"}"#
    );

    let s2c_in = br#"{"role":"guest","meta":"env=prod","msg":"otp=654321"}"#;
    let s2c_out = ws_transform(
        &upgrade,
        WebSocketDirection::ServerToClient,
        WebSocketKind::Text,
        s2c_in,
        &cancel,
        limits,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        std::str::from_utf8(&s2c_out).unwrap(),
        r#"{"role":"guest","meta":"env=staging","msg":"otp=[captured:654321]"}"#
    );

    // Binary frames must never be corrupted or coerced to UTF-8.
    let binary_frame = [
        0x00, 0xff, 0xfe, b'e', b'n', b'v', b'=', b'p', b'r', b'o', b'd',
    ];
    assert!(
        ws_transform(
            &upgrade,
            WebSocketDirection::ClientToServer,
            WebSocketKind::Binary,
            &binary_frame,
            &cancel,
            limits,
        )
        .unwrap()
        .is_none()
    );

    // Matcher check for SSL Pass Through host/port/regex evaluation.
    let matcher = build_ssl_pass_through_matcher(&[
        SslPassThroughRule {
            id: Uuid::new_v4(),
            enabled: true,
            host_pattern: "*.corp.example.com".into(),
            port: Some(8443),
            is_regex: false,
        },
        SslPassThroughRule {
            id: Uuid::new_v4(),
            enabled: true,
            host_pattern: r"^127\.0\.0\.1$".into(),
            port: None,
            is_regex: true,
        },
    ])
    .unwrap()
    .unwrap();
    assert!(matcher("api.corp.example.com", 8443));
    assert!(!matcher("api.corp.example.com", 443));
    assert!(!matcher("corp.example.com", 8443));
    assert!(matcher("127.0.0.1", 19443));
}
