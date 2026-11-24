use std::io::Write;
use std::ops::Range;

use flate2::{Compression, write::GzEncoder};

use crate::{
    PrettyMessage, decompress_gzip_bytes, expand_search_replace_all, expand_search_replace_one,
    format_pretty_message, format_pretty_message_with_line_map,
};

#[test]
fn pretty_formats_xml_and_html_and_reports_explicit_parse_errors() {
    let xml_resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/xml; charset=utf-8\r\n\r\n<?xml version=\"1.0\" encoding=\"UTF-8\"?><!--head--><user id=\"7\"><name>alice &amp; bob</name><empty/><raw><![CDATA[x < y]]></raw><roles><role>admin</role></roles></user>";
    assert_eq!(
        format_pretty_message(xml_resp).unwrap(),
        concat!(
            "HTTP/1.1 200 OK\n",
            "Content-Type: application/xml; charset=utf-8\n\n",
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<!--head-->\n",
            "<user id=\"7\">\n",
            "  <name>alice &amp; bob</name>\n",
            "  <empty/>\n",
            "  <raw>\n",
            "    <![CDATA[x < y]]>\n",
            "  </raw>\n",
            "  <roles>\n",
            "    <role>admin</role>\n",
            "  </roles>\n",
            "</user>"
        )
    );

    let standalone_xml = b"<?xml version=\"1.0\"?><ping><seq>1</seq></ping>";
    assert_eq!(
        format_pretty_message(standalone_xml).unwrap(),
        "<?xml version=\"1.0\"?>\n<ping>\n  <seq>1</seq>\n</ping>"
    );

    let broken_xml = b"HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\n\r\n<root><item>1</root>";
    let xml_err = format_pretty_message(broken_xml).unwrap_err();
    assert!(xml_err.contains("XML 解析失败"));
    assert!(xml_err.contains("原始字节未改变"));

    let html_resp = b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>MIO Demo</title></head><body><div class=\"card\"><h1>Hello</h1><br><img src=\"/logo.png\" alt=\"mio\"><script>if (a < b && c > d) { ok(); }</script></div></body></html>";
    assert_eq!(
        format_pretty_message(html_resp).unwrap(),
        concat!(
            "HTTP/1.1 200 OK\n",
            "Content-Type: text/html; charset=utf-8\n\n",
            "<!DOCTYPE html>\n",
            "<html>\n",
            "  <head>\n",
            "    <meta charset=\"utf-8\">\n",
            "    <title>\n",
            "      MIO Demo\n",
            "    </title>\n",
            "  </head>\n",
            "  <body>\n",
            "    <div class=\"card\">\n",
            "      <h1>Hello</h1>\n",
            "      <br>\n",
            "      <img src=\"/logo.png\" alt=\"mio\">\n",
            "      <script>if (a < b && c > d) { ok(); }</script>\n",
            "    </div>\n",
            "  </body>\n",
            "</html>"
        )
    );

    let html5_optional_close = b"HTTP/1.1 404 Not Found\r\nContent-Type: text/html; charset=UTF-8\r\n\r\n<!DOCTYPE html><html lang=en><meta charset=utf-8><title>Error 404 (Not Found)!!1</title><style>*{margin:0}</style><a href=//www.google.com/><span></span></a><p><b>404.</b> <ins>That's an error.</ins><p>The requested URL <code>/generate_204</code> was not found.";
    let formatted_html5 = format_pretty_message(html5_optional_close).unwrap();
    assert!(formatted_html5.contains("<!DOCTYPE html>\n<html lang=en>\n  <meta charset=utf-8>"));
    assert!(formatted_html5.contains("  <p>\n    <b>404.</b>"));
    assert!(formatted_html5.contains("  <p>\n    The requested URL"));

    for bad_html in [
        &b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body><div><p>bad</div></body></html>"[..],
        &b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body><div>unclosed"[..],
        &b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<div title=\"unclosed>"[..],
        &b"<!DOCTYPE html><html><script>const x = 1;</html>"[..],
    ] {
        let err = format_pretty_message(bad_html).unwrap_err();
        assert!(err.contains("HTML 解析失败"), "unexpected error: {err}");
        assert!(err.contains("原始字节未改变"), "unexpected error: {err}");
    }
}

#[test]
fn search_replace_and_gzip_helpers_handle_captures_limits_and_binary() {
    let text = "token=Alpha&token=beta";
    assert_eq!(
        expand_search_replace_one(text, 0..11, "token=(\\w+)", "$1:$0", true, false, 128).unwrap(),
        "$1:$0"
    );
    assert_eq!(
        expand_search_replace_one(
            text,
            0..11,
            "token=(?P<val>\\w+)",
            "key=${val}$$",
            true,
            true,
            128
        )
        .unwrap(),
        "key=Alpha$"
    );
    assert_eq!(
        expand_search_replace_all(
            text,
            &[0..11, 12..22],
            "token=(\\w+)",
            "k=$1",
            true,
            true,
            128
        )
        .unwrap(),
        "k=Alpha&k=beta"
    );
    assert!(
        expand_search_replace_all(
            text,
            &[Range::<usize> { start: 0, end: 11 }],
            "[bad",
            "x",
            true,
            true,
            128
        )
        .unwrap_err()
        .contains("搜索正则表达式无效")
    );
    assert!(
        expand_search_replace_one(text, 0..5, "token", "0123456789", true, false, 24)
            .unwrap_err()
            .contains("限额")
    );

    let payload = b"\x00\xff\xe4\xb8\xad\xe6\x96\x87gzip-payload\r\n";
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(payload).unwrap();
    let compressed = encoder.finish().unwrap();
    assert_eq!(decompress_gzip_bytes(&compressed, 1024).unwrap(), payload);
    assert!(decompress_gzip_bytes(b"plain", 1024).is_err());
    assert!(decompress_gzip_bytes(&compressed[..compressed.len() - 4], 1024).is_err());
    assert!(decompress_gzip_bytes(&compressed, 5).is_err());
}

#[test]
fn pretty_line_map_tracks_exact_original_lines_across_json_xml_html_and_chunked() {
    let single_line_json =
        b"POST /api HTTP/1.1\r\nContent-Type: application/json\r\n\r\n{\"a\":1,\"b\":[true,false]}";
    let PrettyMessage { text, line_numbers } =
        format_pretty_message_with_line_map(single_line_json).unwrap();
    let lines: Vec<&str> = text.split('\n').collect();
    let map = line_numbers.expect("synthetic pretty lines must produce line mapping");
    assert_eq!(map.len(), lines.len());
    assert_eq!(map, vec![1, 2, 3, 4, 0, 0, 0, 0, 0, 0]);

    let multiline_json = concat!(
        "POST /api HTTP/1.1\r\n",
        "Content-Type: application/json\r\n\r\n",
        "{\r\n",
        "  \"a\": [1, 2],\r\n",
        "  \"b\": \"a\",\r\n",
        "  \"a\": {\"x\": 9},\r\n",
        "  \"empty\": {}\r\n",
        "}"
    );
    let PrettyMessage { text, line_numbers } =
        format_pretty_message_with_line_map(multiline_json.as_bytes()).unwrap();
    let val: serde_json::Value =
        serde_json::from_str(multiline_json.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(
        text,
        format!(
            "POST /api HTTP/1.1\nContent-Type: application/json\n\n{}",
            serde_json::to_string_pretty(&val).unwrap()
        )
    );
    let map = line_numbers.unwrap();
    assert_eq!(map.len(), text.split('\n').count());
    assert_eq!(map, vec![1, 2, 3, 4, 7, 0, 0, 0, 8, 9]);

    let already_pretty = "HTTP/1.1 200 OK\nContent-Type: application/json\n\n{\n  \"ok\": true\n}";
    let res = format_pretty_message_with_line_map(already_pretty.as_bytes()).unwrap();
    assert_eq!(res.text, already_pretty);
    assert_eq!(res.line_numbers, None);

    let chunked_with_trailer = concat!(
        "HTTP/1.1 200 OK\r\n",
        "Content-Type: application/json\r\n",
        "Transfer-Encoding: chunked\r\n\r\n",
        "1b\r\n",
        "{\"api\":\"demo\",\"ret\":[\"OK\"]}\r\n",
        "0\r\n",
        "X-Checksum: abc\r\n\r\n"
    );
    let res = format_pretty_message_with_line_map(chunked_with_trailer.as_bytes()).unwrap();
    let map = res.line_numbers.unwrap();
    assert_eq!(map.len(), res.text.split('\n').count());
    assert_eq!(map, vec![1, 2, 3, 4, 6, 0, 0, 0, 0, 0, 8, 9]);

    let multiline_html = concat!(
        "HTTP/1.1 200 OK\r\n",
        "Content-Type: text/html\r\n\r\n",
        "<!DOCTYPE html><html>\r\n",
        "<head><style>body{color:red;\r\nmargin:0;}</style><title>Hi</title></head>\r\n",
        "<body><!-- c1\r\nc2 --><script>\r\nconst x=1;\r\nconst y=2;\r\n</script></body></html>"
    );
    let res = format_pretty_message_with_line_map(multiline_html.as_bytes()).unwrap();
    let map = res.line_numbers.unwrap();
    assert_eq!(map.len(), res.text.split('\n').count());
    assert_eq!(map[..4], [1, 2, 3, 4]);
    for expected_orig in [5, 6, 7, 8, 9, 10, 11] {
        assert!(
            map.contains(&expected_orig),
            "expected orig line {expected_orig} in map {map:?} for:\n{}",
            res.text
        );
    }

    let plain_http = b"POST /login HTTP/1.1\r\nHost: example.com\r\n\r\nuser=alice";
    let res = format_pretty_message_with_line_map(plain_http).unwrap();
    assert_eq!(res.line_numbers, None);
}

#[test]
fn pretty_html_and_xml_handle_multibyte_utf8_across_prefix_boundaries() {
    let html_with_cjk = concat!(
        "HTTP/1.1 200 OK\r\n",
        "Content-Type: text/html; charset=utf-8\r\n\r\n",
        "<!DOCTYPE html><html><head><title>中文标题</title></head>",
        "<body><p>a中文测试文本</p><div title=\"中文属性测试\">一二三四五</div></body></html>"
    );
    let formatted_html = format_pretty_message_with_line_map(html_with_cjk.as_bytes())
        .expect("multibyte UTF-8 in HTML must not panic during markup tokenization");
    assert!(formatted_html.text.contains("a中文测试文本"));
    assert!(formatted_html.text.contains("一二三四五"));

    let xml_with_cjk = concat!(
        "HTTP/1.1 200 OK\r\n",
        "Content-Type: application/xml; charset=utf-8\r\n\r\n",
        "<root><item>a中文节点</item><note>😀多字节测试</note></root>"
    );
    let formatted_xml = format_pretty_message_with_line_map(xml_with_cjk.as_bytes())
        .expect("multibyte UTF-8 in XML must not panic during markup tokenization");
    assert!(formatted_xml.text.contains("a中文节点"));
    assert!(formatted_xml.text.contains("😀多字节测试"));

    let doc_32_boundary = concat!(
        "HTTP/1.1 200 OK\r\n\r\n",
        "0123456789012345678901234567890中文边界测试"
    );
    let formatted_boundary = format_pretty_message_with_line_map(doc_32_boundary.as_bytes())
        .expect("multibyte UTF-8 crossing byte 32 must not panic in looks_like_html_document");
    assert!(formatted_boundary.text.contains("中文边界测试"));
}

#[test]
fn pretty_html_style_handles_multiline_comments_and_deep_braces_without_panic_or_oom() {
    let multiline_css_html = concat!(
        "HTTP/1.1 200 OK\r\n",
        "Content-Type: text/html; charset=utf-8\r\n\r\n",
        "<!DOCTYPE html><html><head><style>\n",
        "/* first comment line\nsecond comment line */\n",
        "body { content: \"a\nb\"; color: red; }\n",
        "</style></head><body>ok</body></html>"
    );
    let res = format_pretty_message_with_line_map(multiline_css_html.as_bytes()).unwrap();
    if let Some(map) = res.line_numbers {
        assert_eq!(map.len(), res.text.split('\n').count());
    }
    assert!(res.text.contains("first comment line"));
    assert!(res.text.contains("second comment line"));

    let deep_braces = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<!DOCTYPE html><html><head><style>{}{}</style></head><body>ok</body></html>",
        "{".repeat(20_000),
        "}".repeat(20_000),
    );
    let bounded = format_pretty_message_with_line_map(deep_braces.as_bytes()).unwrap();
    assert!(bounded.text.len() <= crate::http::pretty::MAX_PRETTY_BYTES * 3);
}
