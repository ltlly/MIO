use mio_plugin_api::Header;

use super::*;

fn header(name: &str, value: &[u8]) -> Header {
    Header {
        name: name.to_owned(),
        value: value.to_vec(),
    }
}

#[test]
fn infer_proxy_mime_type_matches_content_type_parameters_and_8_categories() {
    let cases: &[(&[u8], &[u8], &str)] = &[
        (b"text/html; charset=utf-8", b"plain", "html"),
        (b"application/xhtml+xml; charset=UTF-8", b"", "html"),
        (b"application/javascript; charset=utf-8", b"", "script"),
        (b"text/ecmascript", b"", "script"),
        (b"application/xml; version=1.0", b"", "xml"),
        (b"text/xml", b"", "xml"),
        (b"text/css; charset=utf-8", b"", "css"),
        (b"application/json; charset=utf-8", b"", "text"),
        (b"application/problem+json", b"", "text"),
        (b"application/soap+xml", b"", "text"),
        (b"text/plain; charset=gbk", b"", "text"),
        (b"image/png", b"", "image"),
        (b"image/svg+xml", b"", "image"),
        (b"application/x-shockwave-flash", b"", "flash"),
        (b"application/futuresplash", b"", "flash"),
        (b"application/octet-stream", b"", "binary"),
    ];
    for &(ct, body, expected) in cases {
        assert_eq!(
            infer_proxy_mime_type(&[header("Content-Type", ct)], body),
            expected,
            "Content-Type {:?}",
            String::from_utf8_lossy(ct)
        );
    }
    assert_eq!(infer_proxy_mime_type(&[], b""), "");
    assert_eq!(infer_proxy_mime_type(&[header("X-Other", b"1")], b""), "");
}

#[test]
fn infer_proxy_mime_type_sniffs_magic_bytes_and_bounds_large_payloads() {
    let octet = [header("Content-Type", b"application/octet-stream")];
    let cases: &[(&[Header], &[u8], &str)] = &[
        (&[], b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR", "image"),
        (&octet, b"GIF89a\x01\x00\x01\x00", "image"),
        (&[], b"GIF87a\x01\x00", "image"),
        (&[], b"\xff\xd8\xff\xe0JFIF", "image"),
        (&[], b"RIFF\x24\x00\x00\x00WEBPVP8 ", "image"),
        (&[], b"%PDF-1.7\n%\xe2\xe3\xcf\xd3", "binary"),
        (&[], b"FWS\x09\x10\x00\x00\x00", "flash"),
        (&[], b"   \r\n\t<!DOCTYPE html>\n<html>", "html"),
        (&octet, b"\xef\xbb\xbf  <HTML lang=\"zh\">", "html"),
        (&[], b" \n<?xml version=\"1.0\"?><root/>", "xml"),
        (&octet, b"  {\"ok\":true}", "text"),
        (&[], b"function init() { return 1; }", "text"),
        (&[], b"\x00\xff\xfe\xfd\x80", "binary"),
    ];
    for &(headers, body, expected) in cases {
        assert_eq!(infer_proxy_mime_type(headers, body), expected);
    }

    // Late HTML marker past the 512-byte sniff window in a binary stream must not scan MBs or match HTML.
    let mut padded_binary = vec![0xff_u8; MAX_MIME_SNIFF_BYTES + 1024];
    padded_binary[MAX_MIME_SNIFF_BYTES + 16..MAX_MIME_SNIFF_BYTES + 30]
        .copy_from_slice(b"<html></html>!");
    assert_eq!(infer_proxy_mime_type(&[], &padded_binary), "binary");
}

#[test]
fn extract_html_page_title_handles_mixed_case_attributes_and_avoids_titles_tag() {
    let html = r#"<!doctype html>
        <html>
        <head>
            <titles>not-a-page-title</titles>
            <title-meta content="ignored"/>
            <TITLE lang="zh-CN" data-custom="1 > 0">
                MIO   Proxy
                控制台	标题
            </TiTlE >
            <title>second title ignored</title>
        </head>
        </html>"#
        .as_bytes();
    assert_eq!(
        extract_html_page_title(html).unwrap(),
        "MIO Proxy 控制台 标题"
    );
    assert_eq!(
        extract_html_page_title(b"<titles>only plural</titles><div>body</div>").unwrap(),
        ""
    );
    assert_eq!(extract_html_page_title(b"<title/>").unwrap(), "");
}

#[test]
fn extract_html_page_title_decodes_named_and_chinese_numeric_entities_and_latin1() {
    let html =
        b"<html><head><title>  A &amp; B &lt;tag&gt; &quot;q&quot; &apos;s&#39; &#20013;&#25991; &#x6d4b;&#X8BD5; &unknown;  </title></head></html>";
    assert_eq!(
        extract_html_page_title(html).unwrap(),
        "A & B <tag> \"q\" 's' 中文 测试 &unknown;"
    );

    // Non-UTF-8 Latin-1 bytes inside <title> preserve byte values instead of failing or dropping.
    let latin1 = b"<title>Caf\xe9\t\xa0Test</title>";
    assert_eq!(
        extract_html_page_title(latin1).unwrap(),
        "Caf\u{00e9} \u{00a0}Test"
    );
}

#[test]
fn extract_html_page_title_errors_on_unclosed_or_oversized_title_within_256_kib() {
    let mut unclosed = b"<html><head><title>unclosed ".to_vec();
    unclosed.extend(std::iter::repeat_n(b'a', MAX_TITLE_SCAN_BYTES));
    unclosed.extend_from_slice(b"</title></html>");
    assert!(extract_html_page_title(&unclosed).is_err());

    let unclosed_open_tag = b"<html><head><title attr=\"unterminated";
    assert!(extract_html_page_title(unclosed_open_tag).is_err());

    let mut oversized_title = b"<title>".to_vec();
    oversized_title.extend(std::iter::repeat_n(b'x', MAX_PAGE_TITLE_BYTES + 1));
    oversized_title.extend_from_slice(b"</title>");
    assert!(extract_html_page_title(&oversized_title).is_err());

    // A <title> starting strictly beyond 256 KiB is outside the scan window and returns empty.
    let mut beyond_window = vec![b' '; MAX_TITLE_SCAN_BYTES + 32];
    beyond_window.extend_from_slice(b"<title>late</title>");
    assert_eq!(extract_html_page_title(&beyond_window).unwrap(), "");
}

#[test]
fn normalize_ip_candidate_handles_ipv4_ipv6_brackets_ports_and_missing_peer() {
    assert_eq!(normalize_ip_candidate(None), None);
    assert_eq!(normalize_ip_candidate(Some("")), None);
    assert_eq!(normalize_ip_candidate(Some("example.com")), None);
    assert_eq!(normalize_ip_candidate(Some("example.com:443")), None);
    assert_eq!(normalize_ip_candidate(Some("[::1]:99999")), None);
    assert_eq!(normalize_ip_candidate(Some("[::1")), None);

    assert_eq!(
        normalize_ip_candidate(Some(" 127.0.0.1:8080 ")).as_deref(),
        Some("127.0.0.1")
    );
    assert_eq!(
        normalize_ip_candidate(Some("10.0.0.5")).as_deref(),
        Some("10.0.0.5")
    );
    assert_eq!(
        normalize_ip_candidate(Some("[0:0:0:0:0:0:0:1]:443")).as_deref(),
        Some("::1")
    );
    assert_eq!(
        normalize_ip_candidate(Some("[2001:0db8:0000:0000::1]")).as_deref(),
        Some("2001:db8::1")
    );
    assert_eq!(
        normalize_ip_candidate(Some("fe80::1%lo0")).as_deref(),
        Some("fe80::1")
    );
    assert_eq!(
        normalize_ip_candidate(Some("::ffff:192.168.1.10")).as_deref(),
        Some("192.168.1.10")
    );
}

#[test]
fn matches_annotation_condition_distinguishes_annotated_and_unannotated() {
    assert!(matches_annotation_condition(true, true));
    assert!(!matches_annotation_condition(false, true));
    assert!(matches_annotation_condition(false, false));
    assert!(!matches_annotation_condition(true, false));
}
