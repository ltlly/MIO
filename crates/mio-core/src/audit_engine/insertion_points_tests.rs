use serde_json::Value;

use super::*;
use crate::{Header, HttpRequest, HttpVersion};

fn make_req(url: &str, headers: &[(&str, &str)], body: &[u8]) -> HttpRequest {
    let mut hs: Vec<Header> = headers
        .iter()
        .map(|(k, v)| Header {
            name: (*k).into(),
            value: v.as_bytes().to_vec(),
        })
        .collect();
    hs.push(Header {
        name: "Content-Length".into(),
        value: body.len().to_string().into_bytes(),
    });
    HttpRequest {
        method: "POST".into(),
        url: url.into(),
        version: HttpVersion::Http11,
        headers: hs,
        body: body.to_vec(),
        trailers: Vec::new(),
    }
}

#[test]
fn extract_and_rebuild_json_cookie_and_custom_headers() {
    let json_body = br#"{"user":{"name":"alice","age":30},"tags":["dev","ops"]}"#;
    let req = make_req(
        "https://api.example.com/v1/update?tenant=acme",
        &[
            ("Content-Type", "application/json"),
            ("Cookie", "sid=tok_1; theme=\"dark\""),
            ("X-Trace-Id", "req-001"),
            ("User-Agent", "MIO/1.0"),
        ],
        json_body,
    );
    let pts = extract_structured_parameters(&req);
    let pairs: Vec<(InsertionLocation, &str, &str)> = pts
        .iter()
        .map(|p| (p.location, p.name.as_str(), p.value.as_str()))
        .collect();
    assert_eq!(
        pairs,
        vec![
            (InsertionLocation::Query, "tenant", "acme"),
            (InsertionLocation::Json, "user.name", "alice"),
            (InsertionLocation::Json, "user.age", "30"),
            (InsertionLocation::Json, "tags[0]", "dev"),
            (InsertionLocation::Json, "tags[1]", "ops"),
            (InsertionLocation::Cookie, "sid", "tok_1"),
            (InsertionLocation::Cookie, "theme", "dark"),
            (InsertionLocation::Header, "X-Trace-Id", "req-001"),
        ]
    );

    let rebuilt_json = rebuild_structured_parameter(&req, &pts[3], "qa\"team").unwrap();
    let parsed: Value = serde_json::from_slice(&rebuilt_json.body).unwrap();
    assert_eq!(parsed["tags"][0], "qa\"team");
    assert_eq!(parsed["user"]["name"], "alice");

    let rebuilt_cookie = rebuild_structured_parameter(&req, &pts[6], "light").unwrap();
    let cookie_hdr = rebuilt_cookie
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("cookie"))
        .unwrap();
    assert_eq!(
        std::str::from_utf8(&cookie_hdr.value).unwrap(),
        "sid=tok_1; theme=\"light\""
    );
    assert!(rebuild_structured_parameter(&req, &pts[5], "bad;inject=1").is_err());

    let rebuilt_hdr = rebuild_structured_parameter(&req, &pts[7], "req-999").unwrap();
    let trace_hdr = rebuilt_hdr
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("x-trace-id"))
        .unwrap();
    assert_eq!(trace_hdr.value, b"req-999");
    assert!(rebuild_structured_parameter(&req, &pts[7], "bad\r\nhdr: 1").is_err());
}

#[test]
fn extract_and_rebuild_xml_and_multipart_without_corrupting_binary_parts() {
    let xml_body =
        br#"<?xml version="1.0"?><order id="101"><item sku='A1'>book</item><note><![CDATA[raw <text>]]></note></order>"#;
    let xml_req = make_req(
        "https://api.example.com/xml",
        &[("Content-Type", "application/xml; charset=utf-8")],
        xml_body,
    );
    let xml_pts = extract_structured_parameters(&xml_req);
    assert_eq!(xml_pts.len(), 4);
    assert_eq!(xml_pts[0].name, "order@id");
    assert_eq!(xml_pts[1].name, "order/item@sku");
    assert_eq!(xml_pts[2].name, "order/item");
    assert_eq!(xml_pts[3].name, "order/note");

    let updated_xml = rebuild_structured_parameter(&xml_req, &xml_pts[2], "A & B <2>").unwrap();
    let xml_str = std::str::from_utf8(&updated_xml.body).unwrap();
    assert!(xml_str.contains("<item sku='A1'>A &amp; B &lt;2&gt;</item>"));

    let mut mp_body = Vec::new();
    mp_body.extend_from_slice(
        b"--BOUND\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nReport Q1\r\n",
    );
    mp_body.extend_from_slice(
        b"--BOUND\r\nContent-Disposition: form-data; name=\"blob\"; filename=\"a.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n\x00\xff\xfe\r\n",
    );
    mp_body.extend_from_slice(b"--BOUND--\r\n");
    let mp_req = make_req(
        "https://api.example.com/upload",
        &[("Content-Type", "multipart/form-data; boundary=BOUND")],
        &mp_body,
    );
    let mp_pts = extract_structured_parameters(&mp_req);
    assert_eq!(mp_pts.len(), 1);
    assert_eq!(mp_pts[0].location, InsertionLocation::Multipart);
    assert_eq!(mp_pts[0].name, "title");
    assert_eq!(mp_pts[0].value, "Report Q1");

    let updated_mp = rebuild_structured_parameter(&mp_req, &mp_pts[0], "Report Q2").unwrap();
    assert!(updated_mp.body.windows(3).any(|w| w == [0x00, 0xff, 0xfe]));
    assert!(
        String::from_utf8_lossy(&updated_mp.body).contains("name=\"title\"\r\n\r\nReport Q2\r\n")
    );
    assert!(rebuild_structured_parameter(&mp_req, &mp_pts[0], "x\r\n--BOUND\r\n").is_err());

    let many_query: String = (0..80)
        .map(|i| format!("k{i}=v{i}"))
        .collect::<Vec<_>>()
        .join("&");
    let capped_req = make_req(&format!("https://api.example.com/p?{many_query}"), &[], b"");
    assert_eq!(
        extract_structured_parameters(&capped_req).len(),
        MAX_STRUCTURED_PARAMETERS
    );
}
