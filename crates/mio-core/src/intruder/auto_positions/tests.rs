use super::*;

fn values(raw: &[u8], selection: Option<Range<usize>>) -> Vec<&[u8]> {
    let positions = automatic_positions(raw, selection, AutomaticPlacement::Replace).unwrap();
    positions
        .into_iter()
        .map(|position| &raw[position.start..position.end])
        .collect()
}

fn request(content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut raw =
        format!("POST / HTTP/1.1\r\nHost: fixture.invalid\r\nContent-Type: {content_type}\r\n\r\n")
            .into_bytes();
    raw.extend_from_slice(body);
    raw
}

#[test]
fn query_form_and_cookie_values_keep_duplicates_percent_case_empty_and_quoted_bytes() {
    let raw = b"POST /a/../b?q=%2f&q=%2F&empty=&flag HTTP/1.1\r\nHost: fixture.invalid\r\nCookie: a=one; a=\"two;three\"; blank=\r\ncOoKiE: z=%00\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\nf=%ff&f=a+b&=anonymous&flag";
    assert_eq!(
        values(raw, None),
        vec![
            b"%2f".as_slice(),
            b"%2F",
            b"",
            b"",
            b"one",
            b"two;three",
            b"",
            b"%00",
            b"%ff",
            b"a+b",
            b"anonymous",
            b""
        ]
    );
    let replace = automatic_positions(raw, None, AutomaticPlacement::Replace).unwrap();
    let append = automatic_positions(raw, None, AutomaticPlacement::Append).unwrap();
    assert_eq!(append.len(), replace.len());
    for (before, after) in replace.iter().zip(append) {
        assert_eq!(
            after,
            Position {
                start: before.end,
                end: before.end
            }
        );
    }
}

#[test]
fn selection_in_a_request_uses_original_parameter_ranges_and_ignores_an_unselected_body() {
    let raw = b"POST /?q=one&q=two HTTP/1.1\r\nHost: fixture.invalid\r\nCookie: session=abc\r\nContent-Type: application/octet-stream\r\n\r\n\xff\0opaque";
    let first = raw.windows(3).position(|value| value == b"one").unwrap();
    assert_eq!(values(raw, Some(first..first + 3)), vec![b"one".as_slice()]);
    assert!(values(raw, Some(first + 1..first + 3)).is_empty());
    let second = raw.windows(3).position(|value| value == b"two").unwrap();
    assert_eq!(
        values(raw, Some(first..second + 3)),
        vec![b"one".as_slice(), b"two"],
    );
    let cookie = raw.windows(3).position(|value| value == b"abc").unwrap();
    assert_eq!(
        values(raw, Some(cookie..cookie + 3)),
        vec![b"abc".as_slice()]
    );
    assert_eq!(
        automatic_positions(raw, Some(first..first + 3), AutomaticPlacement::Append).unwrap(),
        vec![Position {
            start: first + 3,
            end: first + 3
        }],
    );
}

#[test]
fn selected_header_values_ignore_unselected_unsupported_content_and_keep_complete_ranges() {
    let raw = b"POST /?x=123 HTTP/1.1\r\nCookie: malformed\r\nContent-Type: application/octet-stream\r\n\r\n\xffopaque";
    let start = raw.windows(3).position(|bytes| bytes == b"123").unwrap();
    assert_eq!(values(raw, Some(start..start + 3)), vec![b"123".as_slice()]);
    assert!(values(raw, Some(start..start + 2)).is_empty());

    let raw =
        b"POST /?bad=%xx HTTP/1.1\r\nCookie: session=kept\r\nContent-Encoding: gzip\r\n\r\nopaque";
    let start = raw.windows(4).position(|bytes| bytes == b"kept").unwrap();
    assert_eq!(
        values(raw, Some(start..start + 4)),
        vec![b"kept".as_slice()]
    );
    assert!(automatic_positions(raw, None, AutomaticPlacement::Replace).is_err());

    let raw = b"POST /?x=query HTTP/1.1\r\nCookie: session=cookie\r\nContent-Type: application/json\r\n\r\n{\"x\":\"body\"}";
    assert_eq!(values(raw, Some(0..raw.len())), values(raw, None));
    let start = raw.iter().position(|byte| *byte == b'{').unwrap();
    assert_eq!(
        values(raw, Some(start..raw.len())),
        vec![b"body".as_slice()]
    );
}

#[test]
fn structured_selections_with_http_version_text_are_parsed_as_data() {
    let raw = br#"{"x": "a HTTP/1.1"}"#;
    assert_eq!(
        automatic_positions(raw, Some(0..raw.len()), AutomaticPlacement::Replace).unwrap(),
        vec![Position { start: 7, end: 17 }]
    );
    let raw = br#"{"x": "a HTTP/1.1 suffix"}"#;
    assert_eq!(
        values(raw, Some(0..raw.len())),
        vec![b"a HTTP/1.1 suffix".as_slice()]
    );
}

#[test]
fn structured_content_type_cannot_have_extra_type_components() {
    for media in [
        "application/extra/problem+json",
        "text/extra/problem+xml",
        "problem+json",
        "problem+xml",
    ] {
        let body = if media.ends_with("json") {
            b"{\"x\":1}".as_slice()
        } else {
            b"<x>one</x>"
        };
        assert!(
            automatic_positions(&request(media, body), None, AutomaticPlacement::Replace).is_err()
        );
    }
}

#[test]
fn json_keeps_duplicate_keys_escapes_scalar_spelling_and_utf8_byte_ranges() {
    let body = br#"{"same":"a\u0062","same":"c\\d","nested":[-0.10E+003,true,false,null,{},[]],"empty":""}"#;
    let raw = request("Application/Problem+JSON; Charset=\"UTF-8\"", body);
    assert_eq!(
        values(&raw, None),
        vec![
            b"a\\u0062".as_slice(),
            b"c\\\\d",
            b"-0.10E+003",
            b"true",
            b"false",
            b"null",
            b""
        ]
    );
    let body = "{\"键\":\"中文§\",\"键\":\"后值\"}".as_bytes();
    let raw = request("application/json", body);
    assert_eq!(
        values(&raw, None),
        vec!["中文§".as_bytes(), "后值".as_bytes()]
    );
    let selected = b"prefix flag=1&name=%E4%B8%AD suffix";
    assert_eq!(
        values(selected, Some(7..selected.len() - b" suffix".len())),
        vec![b"1".as_slice(), b"%E4%B8%AD"]
    );
}

#[test]
fn xml_ranges_preserve_entities_quotes_cdata_and_long_attribute_spacing() {
    let body = b"<root a='&quot;raw&amp;' empty=\"\"><v>A&amp;B<![CDATA[C<d>]]>E</v><!--not data--><empty/></root>";
    let raw = request("text/xml", body);
    assert_eq!(
        values(&raw, None),
        vec![b"&quot;raw&amp;".as_slice(), b"", b"A&amp;B", b"C<d>", b"E"]
    );
    let body = format!("<r a{}={}\"retained\"/>", " ".repeat(300), " ".repeat(300));
    let raw = request("application/xml", body.as_bytes());
    assert_eq!(values(&raw, None), vec![b"retained".as_slice()]);
}

#[test]
fn multipart_file_default_is_unverified_so_only_fields_and_explicit_structured_selection_are_marked()
 {
    let body = b"preamble\r\n--AaB\r\nContent-Disposition: form-data; name=\"first\"\r\n\r\n\0\xffvalue\r\n--AaB-not-a-boundary\r\nremain\r\n--AaB\r\nContent-Disposition: form-data; name=\"file\"; filename=\"x.json\"\r\nContent-Type: application/json\r\n\r\n{\"duplicate\":\"first\",\"duplicate\":\"second\"}\r\n--AaB\r\nContent-Disposition: form-data; name=\"empty\"\r\n\r\n\r\n--AaB--\r\nepilogue";
    let raw = request("multipart/form-data; boundary=\"AaB\"", body);
    assert_eq!(
        values(&raw, None),
        vec![
            b"\0\xffvalue\r\n--AaB-not-a-boundary\r\nremain".as_slice(),
            b""
        ]
    );
    let start = raw.iter().position(|byte| *byte == b'{').unwrap();
    let end = raw.iter().rposition(|byte| *byte == b'}').unwrap() + 1;
    assert_eq!(
        values(&raw, Some(start..end)),
        vec![b"first".as_slice(), b"second"]
    );
    let xml = b"prefix<root key='raw&amp;'><v>text</v></root>suffix";
    let end = xml.iter().rposition(|byte| *byte == b'>').unwrap() + 1;
    assert_eq!(
        values(xml, Some(6..end)),
        vec![b"raw&amp;".as_slice(), b"text"]
    );
}

#[test]
fn valid_parameterless_requests_and_empty_containers_return_empty_without_fake_positions() {
    for raw in [
        b"GET / HTTP/1.1\r\nHost: fixture.invalid\r\n\r\n".as_slice(),
        b"GET /? HTTP/1.1\n\n",
    ] {
        assert!(values(raw, None).is_empty());
    }
    for (media, body) in [
        ("application/json", b"{}".as_slice()),
        ("application/json", b"[]"),
        ("application/xml", b"<root/>"),
        ("application/x-www-form-urlencoded", b""),
    ] {
        assert!(values(&request(media, body), None).is_empty());
    }
}

#[test]
fn invalid_or_unsupported_input_is_an_error_not_partial_positions() {
    for raw in [
        b"GET /?ok=1&bad=%xx HTTP/1.1\r\n\r\n".as_slice(),
        b"GET / HTTP/1.1\r\nCookie: first=good; wrong\r\n\r\n",
        b"POST /?good=value HTTP/1.1\r\nContent-Type: application/json\r\n\r\n{\"a\":\"ok\",\"b\":}",
        b"POST / HTTP/1.1\r\nContent-Type: application/json\r\nContent-Type: text/xml\r\n\r\n{}",
        b"POST / HTTP/1.1\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n",
        b"POST / HTTP/1.1\r\nContent-Type: application/octet-stream\r\n\r\nbinary",
    ] {
        let before = raw.to_vec();
        assert!(automatic_positions(raw, None, AutomaticPlacement::Replace).is_err());
        assert_eq!(raw, before);
    }
    for body in [
        b"{\"a\":1,}".as_slice(),
        b"[1,]",
        b"[01]",
        b"[1.]",
        b"[NaN]",
        b"[+1]",
        b"[1e]",
        b"{\"a\":\"\\uD800\"}",
        b"[\"bad\nstring\"]",
        b"{} trailing",
    ] {
        assert!(
            automatic_positions(
                &request("application/json", body),
                None,
                AutomaticPlacement::Replace
            )
            .is_err()
        );
    }
    for body in [
        b"<!DOCTYPE r><r/>".as_slice(),
        b"<!DOCTYPE r [<!ENTITY x SYSTEM 'file:///unread'>]><r>&x;</r>",
        b"<r a='one' a='two'/>",
        b"<r><x></r>",
        b"<r>&unknown;</r>",
    ] {
        assert!(
            automatic_positions(
                &request("application/xml", body),
                None,
                AutomaticPlacement::Replace
            )
            .is_err()
        );
    }
    for selection in [Some(1..100), Some(1..1), Some(Range { start: 9, end: 3 })] {
        assert!(
            automatic_positions(b"plain text", selection, AutomaticPlacement::Replace).is_err()
        );
    }
    assert!(automatic_positions(b"plain text", Some(0..10), AutomaticPlacement::Replace).is_err());
}

#[test]
fn budgets_reject_oversized_deep_node_heavy_or_too_many_positions_without_truncation() {
    let raw = vec![b'x'; MAX_AUTO_BYTES + 1];
    assert!(automatic_positions(&raw, Some(0..1), AutomaticPlacement::Replace).is_err());
    let json = format!(
        "{}0{}",
        "[".repeat(MAX_DEPTH + 1),
        "]".repeat(MAX_DEPTH + 1)
    );
    assert!(
        automatic_positions(
            json.as_bytes(),
            Some(0..json.len()),
            AutomaticPlacement::Replace
        )
        .is_err()
    );
    let json = format!("[{}]", vec!["{}"; MAX_NODES].join(","));
    assert!(
        automatic_positions(
            json.as_bytes(),
            Some(0..json.len()),
            AutomaticPlacement::Replace
        )
        .is_err()
    );
    let fields = vec!["x=value"; MAX_POSITIONS + 1].join("&");
    assert!(
        automatic_positions(
            fields.as_bytes(),
            Some(0..fields.len()),
            AutomaticPlacement::Replace
        )
        .is_err()
    );
    let xml = format!(
        "{}{}",
        "<r>".repeat(MAX_DEPTH + 1),
        "</r>".repeat(MAX_DEPTH + 1)
    );
    assert!(
        automatic_positions(
            xml.as_bytes(),
            Some(0..xml.len()),
            AutomaticPlacement::Replace
        )
        .is_err()
    );
}
