use std::io::Write;
use std::ops::Range;

use flate2::{Compression, write::GzEncoder};

use crate::{
    decompress_gzip_bytes, expand_search_replace_all, expand_search_replace_one,
    format_pretty_message,
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
