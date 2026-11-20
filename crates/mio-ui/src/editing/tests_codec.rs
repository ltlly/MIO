use super::*;

#[test]
fn binary_hex_roundtrip_includes_nul_invalid_utf8_and_line_breaks() {
    let bytes: Vec<u8> = (0..=255).collect();
    let encoded = hex_encode(&bytes);
    assert_eq!(encoded.lines().count(), 16);
    assert_eq!(hex_decode(&encoded).unwrap(), bytes);
    assert!(hex_decode("A").is_err());
    assert!(hex_decode("Ｇ0").is_err());
    assert!(hex_decode("00 zz").is_err());
}

#[test]
fn convert_selection_covers_url_codecs_with_utf8_and_binary_guards() {
    assert_eq!(
        convert_selection(ConvertKind::UrlEncodeKey, "Az09-_.~ a+b=中").unwrap(),
        "Az09-_.~%20a%2Bb%3D%E4%B8%AD"
    );
    assert_eq!(
        convert_selection(ConvertKind::UrlEncodeAll, "A-中").unwrap(),
        "%41%2D%E4%B8%AD"
    );
    assert_eq!(
        convert_selection(ConvertKind::UrlEncodeUnicode, "A中文😀").unwrap(),
        "%u0041%u4E2D%u6587%uD83D%uDE00"
    );
    assert_eq!(
        convert_selection(ConvertKind::UrlDecode, "%E4%B8%AD%E6%96%87+test%21").unwrap(),
        "中文 test!"
    );
    assert_eq!(
        convert_selection(ConvertKind::UrlDecode, "%u4E2D%u6587%uD83D%uDE00").unwrap(),
        "中文😀"
    );
    assert_eq!(
        convert_selection(ConvertKind::UrlDecode, "100%+25%2%GG%u12G4").unwrap(),
        "100% 25%2%GG%u12G4"
    );
    let invalid_utf8 = convert_selection(ConvertKind::UrlDecode, "%FF%FE").unwrap_err();
    assert_eq!(
        invalid_utf8,
        "URL 解码结果包含非 UTF-8 二进制字节；请切换到 Hex 视图处理，未替换或丢失原始字节。"
    );
}

#[test]
fn convert_selection_covers_html_base64_and_ascii_hex_codecs() {
    assert_eq!(
        convert_selection(ConvertKind::HtmlEncodeKey, "<a href=\"x\" title='y'>&</a>").unwrap(),
        "&lt;a href=&quot;x&quot; title=&#39;y&#39;&gt;&amp;&lt;/a&gt;"
    );
    assert_eq!(
        convert_selection(ConvertKind::HtmlEncodeDecimal, "A中").unwrap(),
        "&#65;&#20013;"
    );
    assert_eq!(
        convert_selection(ConvertKind::HtmlEncodeHex, "A中").unwrap(),
        "&#x41;&#x4e2d;"
    );
    assert_eq!(
        convert_selection(
            ConvertKind::HtmlDecode,
            "&lt;tag attr=&quot;1&quot; a=&apos;2&apos;&gt;&amp;&nbsp;&#65;&#20013;&#x4e2d;&#X6587;&unknown;&#99999999;"
        )
        .unwrap(),
        "<tag attr=\"1\" a='2'>&\u{00A0}A中中文&unknown;&#99999999;"
    );

    let encoded_b64 = convert_selection(ConvertKind::Base64Encode, "中文:test").unwrap();
    assert_eq!(encoded_b64, "5Lit5paHOnRlc3Q=");
    assert_eq!(
        convert_selection(ConvertKind::Base64Decode, " 5Lit 5paH\nOnRlc3Q ").unwrap(),
        "中文:test"
    );
    assert_eq!(
        convert_selection(ConvertKind::Base64Decode, "%%%").unwrap_err(),
        "所选内容不是有效的 Base64 编码。"
    );
    assert_eq!(
        convert_selection(ConvertKind::Base64Decode, "//4=").unwrap_err(),
        "Base64 解码结果包含非 UTF-8 二进制字节；请切换到 Hex 视图处理，未替换或丢失原始字节。"
    );

    let encoded_hex = convert_selection(ConvertKind::AsciiHexEncode, "A中文").unwrap();
    assert_eq!(encoded_hex, "41e4b8ade69687");
    assert_eq!(
        convert_selection(ConvertKind::AsciiHexDecode, "41 e4b8ad\ne69687").unwrap(),
        "A中文"
    );
    assert!(convert_selection(ConvertKind::AsciiHexDecode, "41e").is_err());
    assert!(convert_selection(ConvertKind::AsciiHexDecode, "41zz").is_err());
    assert_eq!(
        convert_selection(ConvertKind::AsciiHexDecode, "fffe").unwrap_err(),
        "ASCII Hex 解码结果包含非 UTF-8 二进制字节；请切换到 Hex 视图处理，未替换或丢失原始字节。"
    );
}

#[test]
fn sha256_and_md5_digests_match_standard_vectors_and_binary() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
    let all_bytes: Vec<u8> = (0..=255u8).collect();
    let digest = sha256_digest(&all_bytes);
    assert_eq!(
        sha256_hex(&all_bytes),
        hex_encode(&digest).replace([' ', '\n'], "")
    );
    assert_eq!(sha256_hex("中文".as_bytes()).len(), 64);

    assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(
        md5_hex(b"message digest"),
        "f96b697d7cb7938d525a2f31aaf161d0"
    );
    assert_eq!(
        md5_hex(b"abcdefghijklmnopqrstuvwxyz"),
        "c3fcd3d76192e4007dfb496cca67e13b"
    );
    assert_eq!(
        md5_hex(b"\x00\xffbinary"),
        "26bb835a2363365c993a230eb260495a"
    );
}

#[test]
fn smart_decode_unwraps_url_base64_hex_and_gzip_layers() {
    let payload = r#"{"ok":true,"lang":"中文"}"#;
    let gzipped: &[u8] = &[
        0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xab, 0x56, 0xca, 0xcf, 0x56,
        0xb2, 0x2a, 0x29, 0x2a, 0x4d, 0xd5, 0x51, 0xca, 0x49, 0xcc, 0x4b, 0x57, 0xb2, 0x52, 0x7a,
        0xb2, 0x63, 0xed, 0xb3, 0x69, 0xed, 0x4a, 0xb5, 0x00, 0xa6, 0xa8, 0x4c, 0x52, 0x1b, 0x00,
        0x00, 0x00,
    ];
    let (decoded, steps) = smart_decode_bytes(gzipped, 64 * 1024).unwrap();
    assert_eq!(decoded, payload.as_bytes());
    assert_eq!(steps, vec!["Gzip"]);

    let base64_gzip = base64_encode_std(gzipped);
    let (decoded, steps) = smart_decode_bytes(base64_gzip.as_bytes(), 64 * 1024).unwrap();
    assert_eq!(decoded, payload.as_bytes());
    assert_eq!(steps, vec!["Base64", "Gzip"]);

    let url_b64_gzip = format!(
        "{}%3D",
        base64_gzip.trim_end_matches('=').replace('/', "%2F")
    );
    let (decoded, steps) = smart_decode_bytes(url_b64_gzip.as_bytes(), 64 * 1024).unwrap();
    assert_eq!(decoded, payload.as_bytes());
    assert_eq!(steps, vec!["URL", "Base64", "Gzip"]);

    let (decoded, steps) = smart_decode_bytes("4142430a".as_bytes(), 64 * 1024).unwrap();
    assert_eq!(decoded, b"ABC\n");
    assert_eq!(steps, vec!["Hex"]);

    let (decoded, steps) = smart_decode_bytes("%E4%B8%AD%E6%96%87".as_bytes(), 64 * 1024).unwrap();
    assert_eq!(decoded, "中文".as_bytes());
    assert_eq!(steps, vec!["URL"]);

    let (decoded, steps) = smart_decode_bytes("plain-admin".as_bytes(), 64 * 1024).unwrap();
    assert_eq!(decoded, b"plain-admin");
    assert!(steps.is_empty());
}

fn base64_encode_std(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[test]
fn convert_selection_covers_base64_url_safe_and_construct_string_codecs() {
    let raw = "subjects?_d=1&x=+/中文";
    let encoded = convert_selection(ConvertKind::Base64UrlSafeEncode, raw).unwrap();
    assert!(!encoded.contains('+'));
    assert!(!encoded.contains('/'));
    assert!(!encoded.contains('='));
    assert_eq!(
        convert_selection(ConvertKind::Base64UrlSafeDecode, &encoded).unwrap(),
        raw
    );
    assert_eq!(
        convert_selection(ConvertKind::ConstructJsString, "a'b\n").unwrap(),
        "'a' + '\\'' + 'b' + '\\n'"
    );
    assert_eq!(
        convert_selection(ConvertKind::ConstructSqlChar, "Az").unwrap(),
        "CHAR(65)+CHAR(122)"
    );
}

#[test]
fn non_printable_preview_visualizes_crlf_tabs_spaces_and_nul_without_mutating_bytes() {
    let sample = b"GET /a b\tHTTP/1.1\r\nX-Null: \x00\x7f\r\n\r\n";
    let (preview, badges) = super::codec::format_non_printable_preview_with_badges(sample);
    assert!(preview.contains(" \\r   \\n \n"));
    assert!(preview.contains("⇥\t"));
    assert!(preview.contains("GET /a b"));
    assert!(preview.contains("\\x00"));
    assert!(preview.contains("\\x7f"));
    let badge_slices: Vec<&str> = badges.iter().map(|r| &preview[r.clone()]).collect();
    assert_eq!(
        badge_slices,
        vec![" \\r ", " \\n ", " \\r ", " \\n ", " \\r ", " \\n "]
    );

    let req_1402 = concat!(
        "POST /api/dispatchService?appkey=10000001&v=6.5&appVersion=1.0.0&cv=1 HTTP/1.1\r\n",
        "Host: api.example.test\r\n",
        "Accept: */*\r\n",
        "Content-Length: 339\r\n",
        "Content-Type: application/x-www-form-urlencoded\r\n",
        "Accept-Encoding: gzip, deflate, br\r\n",
        "Connection: keep-alive\r\n\r\n",
        "domain=api.example.test cdn.example.test&appName=com.example.demoapp",
    );
    let (filtered, line_numbers) = filter_http_headers_bytes(req_1402.as_bytes(), None).unwrap();
    let filtered_text = std::str::from_utf8(&filtered).unwrap();
    assert!(filtered_text.contains("Host: api.example.test\r\n"));
    assert!(filtered_text.contains("Content-Length: 339\r\n"));
    assert!(filtered_text.contains("Content-Type: application/x-www-form-urlencoded\r\n"));
    assert!(filtered_text.contains("Connection: keep-alive\r\n"));
    assert!(!filtered_text.contains("Accept: */*"));
    assert!(!filtered_text.contains("Accept-Encoding:"));
    assert_eq!(line_numbers, vec![1, 2, 4, 5, 7, 8, 9]);
}

#[test]
fn rendered_http_response_parses_html_json_and_image_metadata_offline() {
    let html_resp = concat!(
        "HTTP/1.1 200 OK\r\n",
        "Content-Type: text/html; charset=utf-8\r\n",
        "Location: /dashboard\r\n\r\n",
        "<!DOCTYPE html><html><head><title>管理后台登录</title>",
        "<script src=\"/static/app.js\"></script></head>",
        "<body><h1>欢迎访问</h1><p>请输入账号密码</p>",
        "<a href=\"/reset?lang=zh\">忘记密码</a>",
        "<form action=\"/api/login\" method=\"post\">",
        "<input type=\"text\" name=\"username\" value=\"admin\">",
        "<input type=\"password\" name=\"password\" value=\"\">",
        "</form></body></html>"
    );
    let parsed_html = parse_rendered_http_response(html_resp.as_bytes());
    assert_eq!(parsed_html.status_code, Some(200));
    assert_eq!(parsed_html.location.as_deref(), Some("/dashboard"));
    match parsed_html.summary {
        RenderedBodySummary::Html {
            title,
            headings,
            paragraphs,
            links,
            forms,
            scripts,
        } => {
            assert_eq!(title.as_deref(), Some("管理后台登录"));
            assert_eq!(headings, vec![(1, "欢迎访问".to_owned())]);
            assert!(paragraphs.iter().any(|p| p.contains("请输入账号密码")));
            assert_eq!(links.len(), 1);
            assert_eq!(links[0].href, "/reset?lang=zh");
            assert_eq!(links[0].text, "忘记密码");
            assert_eq!(forms.len(), 1);
            assert_eq!(forms[0].action, "/api/login");
            assert_eq!(forms[0].method, "POST");
            assert_eq!(forms[0].inputs.len(), 2);
            assert_eq!(forms[0].inputs[0].name, "username");
            assert_eq!(forms[0].inputs[0].value, "admin");
            assert_eq!(scripts, vec!["/static/app.js".to_owned()]);
        }
        other => panic!("expected Html summary, got {other:?}"),
    }

    let json_resp = b"HTTP/1.1 201 Created\r\nContent-Type: application/json\r\n\r\n{\"ok\":true,\"items\":[1,2]}";
    let parsed_json = parse_rendered_http_response(json_resp);
    assert_eq!(parsed_json.status_code, Some(201));
    match parsed_json.summary {
        RenderedBodySummary::Json { pretty, .. } => {
            assert!(pretty.contains("\"ok\": true"));
        }
        other => panic!("expected Json summary, got {other:?}"),
    }

    let mut png_resp = b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\n\r\n".to_vec();
    png_resp.extend_from_slice(&[
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D',
        b'R', 0x00, 0x00, 0x01, 0x40, 0x00, 0x00, 0x00, 0xF0, 0x08, 0x06, 0x00, 0x00, 0x00,
    ]);
    let parsed_png = parse_rendered_http_response(&png_resp);
    match parsed_png.summary {
        RenderedBodySummary::Image {
            format_name,
            mime_type,
            width,
            height,
            ..
        } => {
            assert_eq!(format_name, "PNG");
            assert_eq!(mime_type, "image/png");
            assert_eq!(width, Some(320));
            assert_eq!(height, Some(240));
        }
        other => panic!("expected Image summary, got {other:?}"),
    }
}

#[test]
fn request_body_encoding_roundtrips_form_multipart_json_and_multi_method_conversion() {
    use crate::workbench::{
        BodyEncodingTarget, change_http_body_encoding_bytes, set_http_request_method_bytes,
        toggle_http_request_method_bytes,
    };

    let form_req = concat!(
        "POST /submit?src=web HTTP/1.1\r\n",
        "Host: example.com\r\n",
        "Content-Type: application/x-www-form-urlencoded\r\n",
        "Content-Length: 27\r\n\r\n",
        "user=alice&role=admin%20ops"
    )
    .as_bytes();

    let (mp_raw, mp_label) =
        change_http_body_encoding_bytes(form_req, BodyEncodingTarget::Toggle).unwrap();
    assert_eq!(mp_label, "multipart/form-data");
    let mp_str = std::str::from_utf8(&mp_raw).unwrap();
    assert!(mp_str.contains("Content-Type: multipart/form-data; boundary="));
    assert!(mp_str.contains("name=\"user\"\r\n\r\nalice\r\n"));
    assert!(mp_str.contains("name=\"role\"\r\n\r\nadmin ops\r\n"));

    let (json_raw, json_label) =
        change_http_body_encoding_bytes(&mp_raw, BodyEncodingTarget::Toggle).unwrap();
    assert_eq!(json_label, "application/json");
    let json_str = std::str::from_utf8(&json_raw).unwrap();
    assert!(json_str.contains("Content-Type: application/json\r\n"));
    assert!(json_str.contains("\"user\":\"alice\"") || json_str.contains("\"user\": \"alice\""));
    assert!(
        json_str.contains("\"role\":\"admin ops\"") || json_str.contains("\"role\": \"admin ops\"")
    );

    let (back_form, form_label) =
        change_http_body_encoding_bytes(&json_raw, BodyEncodingTarget::Toggle).unwrap();
    assert_eq!(form_label, "application/x-www-form-urlencoded");
    let back_form_str = std::str::from_utf8(&back_form).unwrap();
    assert!(back_form_str.contains("user=alice&role=admin%20ops"));

    let multiline_json_req = concat!(
        "PUT /api/v1/profile?ver=2 HTTP/1.1\r\n",
        "Host: example.com\r\n",
        "Content-Type: application/json\r\n\r\n",
        "{\n  \"user\": \"alice\",\n  \"meta\": {\n    \"tag\": \"a b\"\n  }\n}"
    )
    .as_bytes();
    let (get_from_json, method_label) = toggle_http_request_method_bytes(multiline_json_req).unwrap();
    assert_eq!(method_label, "GET");
    let get_str = std::str::from_utf8(&get_from_json).unwrap();
    assert!(
        get_str.starts_with("GET /api/v1/profile?ver=2&user=alice&meta.tag=a%20b HTTP/1.1\r\n"),
        "unexpected GET request line: {get_str}"
    );
    assert!(!get_str.contains("Content-Type:"));
    assert!(!get_str.contains("Content-Length:"));

    let (get_from_mp, _) = set_http_request_method_bytes(&mp_raw, "GET").unwrap();
    let get_mp_str = std::str::from_utf8(&get_from_mp).unwrap();
    assert!(
        get_mp_str.starts_with("GET /submit?src=web&user=alice&role=admin%20ops HTTP/1.1\r\n"),
        "unexpected GET from multipart: {get_mp_str}"
    );

    for method in ["POST", "PUT", "PATCH", "DELETE", "OPTIONS"] {
        let (converted, m) = set_http_request_method_bytes(&get_from_mp, method).unwrap();
        assert_eq!(m, method);
        let text = std::str::from_utf8(&converted).unwrap();
        assert!(text.starts_with(&format!("{method} ")));
    }
}

#[test]
fn cjk_charsets_and_hex_byte_range_mapping_roundtrip_accurately() {
    for (label, sample) in [
        ("GBK", "中文测试GBK编码"),
        ("GB18030", "中文测试GB18030编码"),
        ("Big5", "繁體中文測試"),
        ("Shift_JIS", "日本語テスト"),
        ("EUC-KR", "한국어테스트"),
    ] {
        let cs = MessageDisplayCharset::from_label(label).expect("supported CJK charset");
        let encoded = cs.encode(sample);
        assert_ne!(encoded, sample.as_bytes(), "{label} must encode to non-UTF8 bytes");
        let decoded = cs.decode(&encoded).expect("decode CJK bytes");
        assert_eq!(decoded, sample, "{label} roundtrip failed");
    }

    assert_eq!(hex_byte_range_to_text_range(0..1), 0..2);
    assert_eq!(hex_byte_range_to_text_range(2..5), 6..14);
    assert_eq!(hex_text_range_to_byte_range(0..2, 16), Some(0..1));
    assert_eq!(hex_text_range_to_byte_range(6..14, 16), Some(2..5));
}
