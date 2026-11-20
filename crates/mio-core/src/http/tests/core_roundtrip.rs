use super::*;

#[test]
fn binary_duplicates_and_trailers_roundtrip() {
    let raw = b"POST /echo?q=%E4%B8%AD HTTP/1.1\r\nHost: localhost\r\nX-A: a\r\nX-A: \xff\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n\0\xffa\r\n0\r\nX-End: yes\r\n\r\n";
    let message = parse_request(raw, "https://localhost:8443", HttpVersion::Http11).unwrap();
    assert_eq!(message.body, [0, 255, b'a']);
    assert_eq!(
        message.headers.iter().filter(|h| h.name == "X-A").count(),
        2
    );
    assert_eq!(message.trailers.len(), 1);
    let again = parse_request(&request_to_raw(&message), &message.url, message.version).unwrap();
    assert_eq!(message, again);
}

#[test]
fn chinese_draft_and_invalid_framing_are_explicit() {
    let request = parse_request(
        "POST / HTTP/1.1\nHost: localhost\n\n中文🙂".as_bytes(),
        "http://localhost",
        HttpVersion::Http11,
    )
    .unwrap();
    assert_eq!(request.body, "中文🙂".as_bytes());
    for raw in [
        b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\nffffffffff\r\n".as_slice(),
        b"GET //example.com HTTP/1.1\r\n\r\n",
        b"GET / HTTP/1.1\r\nBad Header: x\r\n\r\n",
    ] {
        assert!(parse_request(raw, "http://localhost", HttpVersion::Http11).is_err());
    }
}

#[test]
fn response_roundtrip_preserves_empty_trailers_and_binary() {
    let message = HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![Header {
            name: "Content-Length".into(),
            value: b"2".to_vec(),
        }],
        body: vec![0, 255],
        trailers: vec![],
    };
    assert_eq!(
        parse_response(&response_to_raw(&message), message.version).unwrap(),
        message
    );
    let json_body = b"{\r\n    \"api\":\"demo.o2o.page.gateway.data.delivery\",\r\n    \"v\":\"1.0\",\r\n    \"ret\":[\"FAIL_SYS_REQUEST_EXPIRED::\xe8\xaf\xb7\xe6\xb1\x82\xe5\xa4\xb1\xe6\x95\x88\"],\r\n    \"data\":{\r\n\"systime\": \"1794915652\"\r\n    }\r\n}";
    assert_eq!(json_body.len(), 0xab);
    let chunked_no_trailers = HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![
            Header {
                name: "Content-Type".into(),
                value: b"application/json;charset=UTF-8".to_vec(),
            },
            Header {
                name: "Transfer-Encoding".into(),
                value: b"chunked".to_vec(),
            },
        ],
        body: json_body.to_vec(),
        trailers: vec![],
    };
    let serialized = response_to_raw(&chunked_no_trailers);
    assert_eq!(
        serialized,
        [
            b"HTTP/1.1 200\r\nContent-Type: application/json;charset=UTF-8\r\nTransfer-Encoding: chunked\r\n\r\n".as_slice(),
            json_body.as_slice(),
        ]
        .concat()
    );
    assert_eq!(
        parse_response(&serialized, chunked_no_trailers.version).unwrap(),
        chunked_no_trailers
    );
}

#[test]
fn response_raw_len_matches_serialized_length() {
    let normal = HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![
            Header {
                name: "Content-Length".into(),
                value: b"7".to_vec(),
            },
            Header {
                name: "X-A".into(),
                value: b"hello".to_vec(),
            },
        ],
        body: b"payload".to_vec(),
        trailers: vec![],
    };
    assert_eq!(
        response_raw_len(
            normal.version,
            normal.status,
            &normal.headers,
            &normal.trailers,
            normal.body.len() as u64,
        ),
        response_to_raw(&normal).len() as u64
    );

    let chunked = HttpResponse {
        status: 201,
        version: HttpVersion::Http2,
        reason_phrase: None,
        headers: vec![Header {
            name: "Transfer-Encoding".into(),
            value: b"chunked".to_vec(),
        }],
        body: vec![0, 255, 42],
        trailers: vec![Header {
            name: "X-End".into(),
            value: b"yes".to_vec(),
        }],
    };
    assert_eq!(
        response_raw_len(
            chunked.version,
            chunked.status,
            &chunked.headers,
            &chunked.trailers,
            chunked.body.len() as u64,
        ),
        response_to_raw(&chunked).len() as u64
    );
}

#[test]
fn intruder_response_len_matches_ext_baseline_cases_a_through_f() {
    use flate2::{Compression, write::GzEncoder};
    use std::{
        io::Write,
        time::{Duration, Instant},
    };
    use tokio_util::sync::CancellationToken;

    let field = |name: &str, value: &[u8]| Header {
        name: name.into(),
        value: value.to_vec(),
    };

    assert_eq!(
        intruder_response_len(
            HttpVersion::Http11,
            400,
            None,
            &[field("Content-Length", b"0"), field("Connection", b"close"),],
            0,
        ),
        66
    );

    let standard_headers = [
        field("Content-Type", b"text/plain"),
        field("Content-Length", b"23"),
        field("Connection", b"close"),
    ];
    assert_eq!(
        intruder_response_len(HttpVersion::Http11, 200, None, &standard_headers, 23),
        107
    );

    let chunked_headers = [
        field("Content-Type", b"text/plain"),
        field("Transfer-Encoding", b"chunked"),
        field("Trailer", b"X-Metrics-Trailer"),
        field("Connection", b"close"),
    ];
    assert_eq!(
        intruder_response_len(HttpVersion::Http11, 200, None, &chunked_headers, 23),
        135
    );

    assert_eq!(
        intruder_response_len(
            HttpVersion::Http11,
            200,
            Some(b"MIO nonstandard reason"),
            &standard_headers,
            23,
        ),
        127
    );

    let plain_body = b"0123456789abcdefghijklm";
    assert_eq!(plain_body.len(), 23);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(plain_body).unwrap();
    let compressed = encoder.finish().unwrap();
    let gzip_response = HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![
            field("Content-Type", b"text/plain"),
            field("Content-Encoding", b"gzip"),
            field("Content-Length", compressed.len().to_string().as_bytes()),
            field("Connection", b"close"),
        ],
        body: compressed,
        trailers: vec![],
    };
    let unpacked = unpack_response(
        &gzip_response,
        "GET",
        MAX_BODY,
        &CancellationToken::new(),
        Instant::now() + Duration::from_secs(2),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        intruder_response_len(
            unpacked.version,
            unpacked.status,
            None,
            &unpacked.headers,
            unpacked.body.len() as u64,
        ),
        107
    );
}

#[test]
fn regression_lf_headers_do_not_consume_crlf_in_binary_body() {
    let request = parse_request(
        b"POST / HTTP/1.1\nHost: localhost\n\nabc\r\n\r\nrest",
        "http://localhost:8912",
        HttpVersion::Http11,
    )
    .unwrap();
    assert_eq!(request.body, b"abc\r\n\r\nrest");
}

#[test]
fn regression_request_target_cannot_escape_selected_authority() {
    for raw in [
        b"GET /\\example.invalid/path HTTP/1.1\r\n\r\n".as_slice(),
        b"GET http://u:p@localhost:8912/path HTTP/1.1\r\n\r\n",
    ] {
        assert!(
            parse_request(raw, "http://localhost:8912", HttpVersion::Http11).is_err(),
            "untrusted target must not change connection authority or inject credentials"
        );
    }
}
