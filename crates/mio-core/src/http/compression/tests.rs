use std::{
    io::Write,
    time::{Duration, Instant},
};

use flate2::{
    Compression,
    write::{GzEncoder, ZlibEncoder},
};
use mio_plugin_api::{Header, HttpRequest, HttpResponse, HttpVersion};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::http::{parse_request, parse_response, request_to_raw, response_to_raw};

pub(super) const BINARY: &[u8] = b"\0\xff\x80first\r\nsecond\0\xfe";
// Brotli-compressed "XXXXXXXXXXYYYYYYYYYY".
pub(super) const BR_REPEATED: &[u8] = &[
    0x1b, 0x13, 0x00, 0x00, 0xa4, 0xb0, 0xb2, 0xea, 0x81, 0x47, 0x02, 0x8a,
];

pub(super) fn gzip(body: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(body).unwrap();
    encoder.finish().unwrap()
}

pub(super) fn deflate(body: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(body).unwrap();
    encoder.finish().unwrap()
}

pub(super) fn br_stored(body: &[u8]) -> Vec<u8> {
    if body.is_empty() {
        return vec![0x06];
    }
    assert!(body.len() <= 1 << 16);
    // WBITS=16, an uncompressed metablock, then an empty last metablock.
    let header = (((body.len() - 1) as u32) << 4) | (1 << 20);
    let mut encoded = header.to_le_bytes()[..3].to_vec();
    encoded.extend_from_slice(body);
    encoded.push(0x03);
    encoded
}

pub(super) fn field(name: &str, value: &[u8]) -> Header {
    Header {
        name: name.into(),
        value: value.to_vec(),
    }
}

pub(super) fn response(encoding: &[u8], body: Vec<u8>) -> HttpResponse {
    HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![
            field("X-Before", b"one"),
            field("Content-Encoding", encoding),
            field("content-length", body.len().to_string().as_bytes()),
            field("X-Repeat", b"one"),
            field("X-Repeat", b"\xfftwo"),
            field("Content-Length", body.len().to_string().as_bytes()),
            field("ETag", b"original-tag"),
        ],
        body,
        trailers: vec![],
    }
}

pub(super) fn unpack(
    response: &HttpResponse,
    max: usize,
) -> Result<Option<HttpResponse>, UnpackError> {
    unpack_response(
        response,
        "GET",
        max,
        &CancellationToken::new(),
        Instant::now() + Duration::from_secs(5),
    )
}

pub(super) fn failed(response: &HttpResponse, max: usize) -> String {
    let before = response.clone();
    let UnpackError::Failed(message) = unpack(response, max).unwrap_err() else {
        panic!("expected a decoding failure");
    };
    assert_eq!(response, &before);
    message
}

#[test]
fn supported_codecs_preserve_binary_and_duplicate_headers() {
    for (encoding, body) in [
        (b"gzip".as_slice(), gzip(BINARY)),
        (b"deflate", deflate(BINARY)),
        (b"br", br_stored(BINARY)),
    ] {
        let original = response(encoding, body);
        let before = original.clone();
        let decoded = unpack(&original, 256).unwrap().unwrap();
        assert_eq!(original, before);
        assert_eq!(decoded.body, BINARY);
        assert_eq!(
            (decoded.status, decoded.version),
            (original.status, original.version)
        );
        let kept = |headers: &[Header]| {
            headers
                .iter()
                .filter(|h| {
                    !h.name.eq_ignore_ascii_case("content-encoding")
                        && !h.name.eq_ignore_ascii_case("content-length")
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(kept(&decoded.headers), kept(&original.headers));
        assert!(
            !decoded
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("content-encoding"))
        );
        let lengths = decoded
            .headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case("content-length"))
            .collect::<Vec<_>>();
        assert_eq!(lengths.len(), 1);
        assert_eq!(lengths[0].name, "content-length");
        assert_eq!(lengths[0].value, BINARY.len().to_string().as_bytes());
        assert_eq!(
            parse_response(&response_to_raw(&decoded), decoded.version).unwrap(),
            decoded
        );
    }
}

#[test]
fn request_unpack_uses_the_same_codecs_without_response_body_rules_or_raw_changes() {
    for (encoding, body) in [
        (b"gzip".as_slice(), gzip(BINARY)),
        (b"deflate", deflate(BINARY)),
        (b"br", br_stored(BINARY)),
    ] {
        let encoded_response = response(encoding, body);
        let request = HttpRequest {
            method: "HEAD".into(),
            url: "https://example.invalid/a/../b?q=%2F".into(),
            version: HttpVersion::Http11,
            headers: encoded_response.headers,
            body: encoded_response.body,
            trailers: Vec::new(),
        };
        let original = request.clone();
        let limit = BINARY.len().max(request.body.len());
        let cancel = CancellationToken::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        let decoded = unpack_request(&request, limit, &cancel, deadline)
            .unwrap()
            .unwrap();
        assert_eq!(request, original);
        assert_eq!(request.body.as_slice(), original.body.as_slice());
        assert_eq!(decoded.body, BINARY);
        assert_eq!(decoded.method, original.method);
        assert_eq!(decoded.url, original.url);
        assert_eq!(decoded.version, original.version);
        assert_eq!(decoded.trailers, original.trailers);
        assert_eq!(
            decoded
                .headers
                .iter()
                .filter(|header| header.name.eq_ignore_ascii_case("x-repeat"))
                .cloned()
                .collect::<Vec<_>>(),
            original
                .headers
                .iter()
                .filter(|header| header.name.eq_ignore_ascii_case("x-repeat"))
                .cloned()
                .collect::<Vec<_>>()
        );
        assert!(
            !decoded
                .headers
                .iter()
                .any(|header| header.name.eq_ignore_ascii_case("content-encoding"))
        );
        let lengths = decoded
            .headers
            .iter()
            .filter(|header| header.name.eq_ignore_ascii_case("content-length"))
            .collect::<Vec<_>>();
        assert_eq!(lengths.len(), 1);
        assert_eq!(lengths[0].value, BINARY.len().to_string().as_bytes());
        assert_eq!(
            parse_request(
                &request_to_raw(&decoded),
                "https://example.invalid",
                decoded.version
            )
            .unwrap(),
            decoded
        );
        assert!(matches!(
            unpack_request(&request, request.body.len() - 1, &cancel, deadline),
            Err(UnpackError::Failed(_))
        ));
        assert_eq!(
            unpack_request(&request, limit, &cancel, Instant::now()).unwrap_err(),
            UnpackError::Deadline
        );
        cancel.cancel();
        assert_eq!(
            unpack_request(&request, limit, &cancel, deadline).unwrap_err(),
            UnpackError::Cancelled
        );
        let mut chunked = original.clone();
        chunked.headers.push(field("Transfer-Encoding", b"chunked"));
        chunked.trailers = vec![field("X-End", b"\xffraw")];
        let chunked_original = chunked.clone();
        let chunked_decoded = unpack_request(&chunked, limit, &CancellationToken::new(), deadline)
            .unwrap()
            .unwrap();
        assert_eq!(chunked, chunked_original);
        assert_eq!(chunked_decoded.body, BINARY);
        assert_eq!(chunked_decoded.trailers, chunked.trailers);
        assert!(
            !chunked_decoded
                .headers
                .iter()
                .any(|header| header.name.eq_ignore_ascii_case("content-length"))
        );
        assert!(chunked_decoded.headers.iter().any(|header| {
            header.name.eq_ignore_ascii_case("transfer-encoding") && header.value == b"chunked"
        }));
    }
    let plain = vec![b'X'; 128];
    let br_plain = [vec![b'X'; 10], vec![b'Y'; 10]].concat();
    let deadline = Instant::now() + Duration::from_secs(3);
    let cancel = CancellationToken::new();
    for (encoding, encoded, expected) in [
        (b"gzip".as_slice(), gzip(&plain), plain.clone()),
        (b"deflate", deflate(&plain), plain),
        (b"br", BR_REPEATED.to_vec(), br_plain),
    ] {
        let request = HttpRequest {
            method: "HEAD".into(),
            url: "http://example.invalid/".into(),
            version: HttpVersion::Http11,
            headers: vec![field("Content-Encoding", encoding)],
            body: encoded,
            trailers: Vec::new(),
        };
        let original = request.clone();
        assert!(request.body.len() < expected.len());
        assert!(matches!(
            unpack_request(&request, expected.len() - 1, &cancel, deadline),
            Err(UnpackError::Failed(_))
        ));
        assert_eq!(request, original);
        for limit in [expected.len(), expected.len() + 1] {
            assert_eq!(
                unpack_request(&request, limit, &cancel, deadline)
                    .unwrap()
                    .unwrap()
                    .body,
                expected
            );
            assert_eq!(request, original);
        }
        assert!(matches!(
            unpack_request(&request, request.body.len() - 1, &cancel, deadline),
            Err(UnpackError::Failed(_))
        ));
        assert_eq!(request, original);
    }
}

#[test]
fn request_unpack_errors_identify_messages_without_exposing_encoded_content() {
    let cancel = CancellationToken::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut request = HttpRequest {
        method: "POST".into(),
        url: "http://example.invalid/".into(),
        version: HttpVersion::Http11,
        headers: vec![field("Content-Encoding", b"private-unsupported")],
        body: b"private-body".to_vec(),
        trailers: Vec::new(),
    };
    for (encoding, transfer_encoding, expected) in [
        (
            b"private-unsupported".as_slice(),
            None,
            "消息 Content-Encoding 含不支持的编码",
        ),
        (b"gzip,", None, "消息 Content-Encoding 格式无效"),
        (
            b"gzip",
            Some(b"private-transfer".as_slice()),
            "消息 Transfer-Encoding 不支持安全的 Content-Encoding 解包",
        ),
    ] {
        request.headers = vec![field("Content-Encoding", encoding)];
        if let Some(value) = transfer_encoding {
            request.headers.push(field("Transfer-Encoding", value));
        }
        let original = request.clone();
        let error = unpack_request(&request, 256, &cancel, deadline).unwrap_err();
        assert_eq!(error.to_string(), expected);
        assert_eq!(request, original);
        assert!(!error.to_string().contains("private"));
    }
    assert_eq!(UnpackError::Cancelled.to_string(), "消息解包已取消");
    assert_eq!(UnpackError::Deadline.to_string(), "消息解包超过截止时间");
}

#[test]
fn repeated_codings_decode_in_global_reverse_order() {
    let mut original = response(b"bR, identity", gzip(&deflate(&br_stored(BINARY))));
    original
        .headers
        .push(field("cOnTeNt-EnCoDiNg", b" DEFLATE\t,\tGZip "));
    assert_eq!(unpack(&original, 256).unwrap().unwrap().body, BINARY);
    original.headers[1].value = b"br, gzip".to_vec();
    assert!(failed(&original, 256).contains("gzip"));
}

#[test]
fn gzip_members_are_all_decoded_and_validated() {
    let mut encoded = gzip(b"first");
    encoded.extend(gzip(&[]));
    encoded.extend(gzip(BINARY));
    let original = response(b"gzip", encoded.clone());
    assert_eq!(
        unpack(&original, 256).unwrap().unwrap().body,
        [b"first".as_slice(), BINARY].concat()
    );
    let crc = encoded.len() - 8;
    encoded[crc] ^= 1;
    failed(&response(b"gzip", encoded), 256);
}

#[test]
fn every_truncation_and_trailing_bytes_fail() {
    for (encoding, encoded) in [
        (b"gzip".as_slice(), gzip(BINARY)),
        (b"deflate", deflate(BINARY)),
        (b"br", BR_REPEATED.to_vec()),
    ] {
        for end in 0..encoded.len() {
            failed(&response(encoding, encoded[..end].to_vec()), 256);
        }
        for trailing in [b"\0".as_slice(), b"private-trailing-data", &encoded] {
            let mut body = encoded.clone();
            body.extend_from_slice(trailing);
            if encoding == b"gzip" && trailing == encoded.as_slice() {
                assert_eq!(
                    unpack(&response(encoding, body), 256)
                        .unwrap()
                        .unwrap()
                        .body,
                    [BINARY, BINARY].concat()
                );
            } else {
                let message = failed(&response(encoding, body), 256);
                assert!(!message.contains("private-trailing-data"));
            }
        }
    }
}
