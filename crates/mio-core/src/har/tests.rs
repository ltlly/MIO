use super::*;
use mio_plugin_api::{FlowState, FlowSummary, HttpRequest, HttpResponse, TrafficSource, Uuid};

fn decode_base64_for_test(encoded: &str) -> Vec<u8> {
    assert_eq!(encoded.len() % 4, 0);
    let mut out = Vec::with_capacity(encoded.len() / 4 * 3);
    let (chunks, _) = encoded.as_bytes().as_chunks::<4>();
    for chunk in chunks {
        let padding = chunk.iter().rev().take_while(|&&byte| byte == b'=').count();
        assert!(padding <= 2);
        let mut acc = 0u32;
        for &byte in chunk {
            let sextet = if byte == b'=' {
                0
            } else {
                u32::try_from(
                    BASE64_TABLE
                        .iter()
                        .position(|&candidate| candidate == byte)
                        .expect("valid base64 character"),
                )
                .unwrap()
            };
            acc = (acc << 6) | sextet;
        }
        let bytes = [(acc >> 16) as u8, (acc >> 8) as u8, acc as u8];
        out.extend_from_slice(&bytes[..3 - padding]);
    }
    out
}

fn sample_flow(
    url: &str,
    req_version: HttpVersion,
    resp_version: HttpVersion,
    req_body: Vec<u8>,
    resp_body: Option<Vec<u8>>,
    started_unix_ms: i64,
    elapsed_ms: u64,
) -> FlowRecord {
    let response = resp_body.map(|body| HttpResponse {
        status: 302,
        version: resp_version,
        reason_phrase: None,
        headers: vec![
            Header {
                name: "Set-Cookie".into(),
                value: b"sid=1; Path=/".to_vec(),
            },
            Header {
                name: "Set-Cookie".into(),
                value: b"theme=dark; Path=/".to_vec(),
            },
            Header {
                name: "Content-Type".into(),
                value: b"application/octet-stream".to_vec(),
            },
            Header {
                name: "Location".into(),
                value: b"https://127.0.0.1:8443/next".to_vec(),
            },
        ],
        body,
        trailers: Vec::new(),
    });
    FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: "POST".into(),
            url: url.to_owned(),
            status: response.as_ref().map(|resp| resp.status),
            state: FlowState::Complete,
            request_bytes: u64::try_from(req_body.len()).unwrap(),
            response_bytes: response
                .as_ref()
                .map_or(0, |resp| u64::try_from(resp.body.len()).unwrap()),
            started_unix_ms,
            elapsed_ms,
        },
        request: HttpRequest {
            method: "POST".into(),
            url: url.to_owned(),
            version: req_version,
            headers: vec![
                Header {
                    name: "X-Dup".into(),
                    value: b"first".to_vec(),
                },
                Header {
                    name: "X-Dup".into(),
                    value: b"second".to_vec(),
                },
                Header {
                    name: "Cookie".into(),
                    value: b"a=1; b=2".to_vec(),
                },
                Header {
                    name: "Content-Type".into(),
                    value: b"application/json; charset=utf-8".to_vec(),
                },
            ],
            body: req_body,
            trailers: Vec::new(),
        },
        response,
    }
}

#[test]
fn har_export_preserves_utf8_text_duplicate_headers_query_params_and_versions() {
    let first = sample_flow(
        "https://127.0.0.1:8443/search?tag=one&tag=two&q=%E4%B8%AD%E6%96%87+ok&flag#frag",
        HttpVersion::Http2,
        HttpVersion::Http11,
        "{\"hello\":\"世界\"}".as_bytes().to_vec(),
        Some("响应正文 OK".as_bytes().to_vec()),
        1_700_000_000_491,
        87,
    );
    let mut second = sample_flow(
        "http://127.0.0.1:8080/legacy",
        HttpVersion::Http10,
        HttpVersion::Http10,
        Vec::new(),
        None,
        0,
        0,
    );
    second.request.method = "GET".into();
    second.summary.method = "GET".into();

    let exported = export_flows_to_har(&[first, second]).unwrap();
    let parsed: Value = serde_json::from_slice(&exported).unwrap();
    let log = &parsed["log"];
    assert_eq!(log["version"], "1.2");
    assert_eq!(log["creator"]["name"], "MIO");
    assert_eq!(log["creator"]["version"], "0.1.0");

    let entries = log["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);

    let entry0 = &entries[0];
    assert_eq!(entry0["startedDateTime"], "2023-11-14T22:13:20.491Z");
    assert_eq!(entry0["time"], 87);
    assert_eq!(entry0["timings"]["wait"], 87);
    assert_eq!(entry0["request"]["httpVersion"], "HTTP/2.0");
    assert_eq!(
        entry0["request"]["queryString"],
        serde_json::json!([
            {"name": "tag", "value": "one"},
            {"name": "tag", "value": "two"},
            {"name": "q", "value": "中文 ok"},
            {"name": "flag", "value": ""},
        ])
    );
    let req_headers = entry0["request"]["headers"].as_array().unwrap();
    assert_eq!(req_headers.len(), 4);
    assert_eq!(
        req_headers[0],
        serde_json::json!({"name": "X-Dup", "value": "first"})
    );
    assert_eq!(
        req_headers[1],
        serde_json::json!({"name": "X-Dup", "value": "second"})
    );
    assert_eq!(
        entry0["request"]["postData"]["text"],
        "{\"hello\":\"世界\"}"
    );
    assert!(entry0["request"]["postData"].get("encoding").is_none());

    assert_eq!(entry0["response"]["httpVersion"], "HTTP/1.1");
    assert_eq!(entry0["response"]["status"], 302);
    assert_eq!(
        entry0["response"]["redirectURL"],
        "https://127.0.0.1:8443/next"
    );
    let resp_headers = entry0["response"]["headers"].as_array().unwrap();
    assert_eq!(resp_headers.len(), 4);
    assert_eq!(
        resp_headers[0],
        serde_json::json!({"name": "Set-Cookie", "value": "sid=1; Path=/"})
    );
    assert_eq!(
        resp_headers[1],
        serde_json::json!({"name": "Set-Cookie", "value": "theme=dark; Path=/"})
    );
    assert_eq!(entry0["response"]["content"]["text"], "响应正文 OK");
    assert!(entry0["response"]["content"].get("encoding").is_none());

    let entry1 = &entries[1];
    assert_eq!(entry1["startedDateTime"], "1970-01-01T00:00:00.000Z");
    assert_eq!(entry1["request"]["httpVersion"], "HTTP/1.0");
    assert!(entry1["request"].get("postData").is_none());
    assert_eq!(entry1["response"]["status"], 0);
    assert_eq!(entry1["response"]["bodySize"], -1);
}

#[test]
fn har_export_roundtrips_binary_request_and_response_bodies_via_base64() {
    for len in [1usize, 2, 3, 4, 5, 31] {
        let req_bytes: Vec<u8> = (0..len)
            .map(|idx| 0x80u8.wrapping_add(u8::try_from(idx * 7).unwrap()))
            .collect();
        let resp_bytes: Vec<u8> = (0..=255u8).take(len + 3).rev().collect();
        let flow = sample_flow(
            "http://127.0.0.1:9001/bin",
            HttpVersion::Http11,
            HttpVersion::Http2,
            req_bytes.clone(),
            Some(resp_bytes.clone()),
            1_700_000_000_000,
            5,
        );
        let exported = export_flows_to_har(&[flow]).unwrap();
        let parsed: Value = serde_json::from_slice(&exported).unwrap();
        let entry = &parsed["log"]["entries"][0];

        assert_eq!(entry["request"]["postData"]["encoding"], "base64");
        let decoded_req =
            decode_base64_for_test(entry["request"]["postData"]["text"].as_str().unwrap());
        assert_eq!(decoded_req, req_bytes);

        assert_eq!(entry["response"]["httpVersion"], "HTTP/2.0");
        assert_eq!(entry["response"]["content"]["encoding"], "base64");
        let decoded_resp =
            decode_base64_for_test(entry["response"]["content"]["text"].as_str().unwrap());
        assert_eq!(decoded_resp, resp_bytes);
    }
}

#[test]
fn iso8601_utc_timestamp_handles_epoch_leap_years_and_negative_boundaries() {
    assert_eq!(started_date_time(0).unwrap(), "1970-01-01T00:00:00.000Z");
    assert_eq!(started_date_time(-1).unwrap(), "1969-12-31T23:59:59.999Z");
    assert_eq!(
        started_date_time(-1_000).unwrap(),
        "1969-12-31T23:59:59.000Z"
    );
    assert_eq!(
        started_date_time(-86_400_000).unwrap(),
        "1969-12-31T00:00:00.000Z"
    );
    assert_eq!(
        started_date_time(951_782_400_005).unwrap(),
        "2000-02-29T00:00:00.005Z"
    );
    assert_eq!(
        started_date_time(253_402_300_799_999).unwrap(),
        "9999-12-31T23:59:59.999Z"
    );
    assert!(started_date_time(i64::MIN).unwrap().ends_with('Z'));
    assert!(started_date_time(i64::MAX).unwrap().ends_with('Z'));
}

#[test]
fn har_export_allows_empty_list_and_enforces_record_and_byte_limits() {
    let empty = export_flows_to_har(&[]).unwrap();
    let parsed: Value = serde_json::from_slice(&empty).unwrap();
    assert_eq!(
        parsed,
        serde_json::json!({
            "log": {
                "version": "1.2",
                "creator": {"name": "MIO", "version": "0.1.0"},
                "entries": []
            }
        })
    );

    let unit = sample_flow(
        "http://127.0.0.1:8080/ok",
        HttpVersion::Http11,
        HttpVersion::Http11,
        Vec::new(),
        None,
        1_000,
        1,
    );
    assert!(export_flows_to_har(&vec![unit.clone(); MAX_RECORDS]).is_ok());
    let count_err = export_flows_to_har(&vec![unit; MAX_RECORDS + 1])
        .unwrap_err()
        .to_string();
    assert!(count_err.contains("500"));

    let huge_utf8 = sample_flow(
        "http://127.0.0.1:8080/huge",
        HttpVersion::Http11,
        HttpVersion::Http11,
        vec![b'x'; MAX_OUTPUT_BYTES],
        None,
        1_000,
        1,
    );
    let size_err = export_flows_to_har(&[huge_utf8]).unwrap_err().to_string();
    assert!(size_err.contains("64 MiB"));

    let near_escape = sample_flow(
        "http://127.0.0.1:8080/escape",
        HttpVersion::Http11,
        HttpVersion::Http11,
        vec![b'"'; 40 * 1024 * 1024],
        None,
        1_000,
        1,
    );
    let writer_err = export_flows_to_har(&[near_escape]).unwrap_err().to_string();
    assert!(writer_err.contains("64 MiB"));
}
