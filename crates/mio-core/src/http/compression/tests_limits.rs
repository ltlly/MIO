use std::{io::Write, time::Instant};

use flate2::{Compression, GzBuilder, write::DeflateEncoder};
use mio_plugin_api::HttpVersion;
use tokio_util::sync::CancellationToken;

use super::{tests::*, *};
use crate::http::{parse_response, response_to_raw};

#[test]
fn checksum_errors_and_raw_deflate_fail_at_the_output_limit() {
    let plain = vec![b'x'; 128];
    let encoded = gzip(&plain);
    for index in [encoded.len() - 8, encoded.len() - 4] {
        let mut corrupt = encoded.clone();
        corrupt[index] ^= 1;
        failed(&response(b"gzip", corrupt), plain.len());
    }
    let mut corrupt = deflate(&plain);
    *corrupt.last_mut().unwrap() ^= 1;
    failed(&response(b"deflate", corrupt), plain.len());
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&plain).unwrap();
    failed(
        &response(b"deflate", encoder.finish().unwrap()),
        plain.len(),
    );
}

#[test]
fn exact_output_limit_and_one_byte_overflow_are_checked() {
    let plain = vec![b'x'; 128];
    let br_plain = [vec![b'X'; 10], vec![b'Y'; 10]].concat();
    for (encoding, encoded, expected) in [
        (b"gzip".as_slice(), gzip(&plain), plain.clone()),
        (b"deflate", deflate(&plain), plain),
        (b"br", BR_REPEATED.to_vec(), br_plain),
    ] {
        assert!(encoded.len() < expected.len());
        let original = response(encoding, encoded);
        assert!(failed(&original, expected.len() - 1).contains("解包正文超过"));
        for limit in [expected.len(), expected.len() + 1] {
            assert_eq!(unpack(&original, limit).unwrap().unwrap().body, expected);
        }
        assert!(failed(&original, original.body.len() - 1).contains("编码正文超过"));
    }
}

#[test]
fn intermediate_limits_and_inner_failures_keep_the_outer_response() {
    let mut encoder = GzBuilder::new()
        .filename(vec![b'n'; 512])
        .write(Vec::new(), Compression::fast());
    encoder.write_all(b"ok").unwrap();
    let inner = encoder.finish().unwrap();
    let outer = deflate(&inner);
    assert!(inner.len() > 128 && outer.len() <= 128);
    assert!(failed(&response(b"gzip, deflate", outer), 128).contains("解包正文超过"));

    let mut corrupt = gzip(BINARY);
    let crc = corrupt.len() - 8;
    corrupt[crc] ^= 1;
    assert!(failed(&response(b"gzip, deflate", deflate(&corrupt)), 256).contains("第 1 层 gzip"));
}

#[test]
fn coding_validation_is_bounded_and_diagnostics_are_redacted() {
    let encoded = gzip(b"private-body");
    for encoding in [
        b"".as_slice(),
        b",gzip",
        b"gzip,",
        b"g zip",
        b"gzip;secret=value",
        b"private-unsupported",
        b"\xff",
        b"\rgzip",
        b"gzip\n",
    ] {
        let message = failed(&response(encoding, encoded.clone()), 256);
        for secret in ["private-body", "secret=value", "private-unsupported"] {
            assert!(!message.contains(secret));
        }
    }
    let accepted = format!("{}gzip", "identity,".repeat(15));
    assert_eq!(
        unpack(&response(accepted.as_bytes(), encoded.clone()), 256)
            .unwrap()
            .unwrap()
            .body,
        b"private-body"
    );
    let rejected = format!("identity,{accepted}");
    assert!(failed(&response(rejected.as_bytes(), encoded.clone()), 256).contains("16 层"));
    assert!(failed(&response(&vec![b' '; MAX_HEAD + 1], encoded), 256).contains("header 限额"));
}

#[test]
fn unmodified_and_no_body_responses_are_no_ops() {
    let mut original = response(b"identity, IDENTITY", BINARY.to_vec());
    assert_eq!(unpack(&original, 0).unwrap(), None);
    original
        .headers
        .retain(|h| !h.name.eq_ignore_ascii_case("content-encoding"));
    assert_eq!(unpack(&original, 0).unwrap(), None);
    for (method, status) in [
        ("HEAD", 200),
        ("GET", 101),
        ("GET", 204),
        ("GET", 205),
        ("GET", 304),
        ("CONNECT", 200),
        ("CONNECT", 299),
    ] {
        let mut original = response(b"private-unsupported", vec![]);
        original.status = status;
        let before = original.clone();
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            unpack_response(&original, method, 0, &cancel, Instant::now()).unwrap(),
            None
        );
        assert_eq!(original, before);
    }
    for (encoding, body) in [
        (b"gzip".as_slice(), gzip(&[])),
        (b"deflate", deflate(&[])),
        (b"br", br_stored(&[])),
    ] {
        assert!(
            unpack(&response(encoding, body), 64)
                .unwrap()
                .unwrap()
                .body
                .is_empty()
        );
        failed(&response(encoding, vec![]), 64);
    }
}

#[test]
fn transfer_codings_are_rejected_before_content_decoding() {
    let cases: &[&[&[u8]]] = &[
        &[b"gzip, chunked"],
        &[b"GZip,\tChUnKeD"],
        &[b"gzip", b"chunked"],
        &[b"chunked", b"CHUNKED"],
        &[b"chunked, chunked"],
        &[b"chunked, gzip"],
        &[b"gzip"],
        &[b"identity"],
        &[b""],
        &[b" chunked"],
        &[b"chunked\t"],
        &[b"\tCHUNKED "],
        &[b"\rchunked"],
        &[b"chunked\n"],
        &[b"chunked;private=value"],
        &[b"private-unsupported, chunked"],
        &[b"\xff"],
    ];
    for values in cases {
        for with_trailers in [false, true] {
            let mut original = response(b"gzip", gzip(&gzip(BINARY)));
            original
                .headers
                .retain(|h| !h.name.eq_ignore_ascii_case("content-length"));
            original
                .headers
                .extend(values.iter().map(|value| field("tRaNsFeR-EnCoDiNg", value)));
            if with_trailers {
                original.trailers = vec![field("X-End", b"one"), field("X-End", b"\xfftwo")];
            }
            assert_eq!(
                failed(&original, 256),
                "消息 Transfer-Encoding 不支持安全的 Content-Encoding 解包"
            );
        }
    }
}

#[test]
fn transfer_coding_is_rejected_on_non_http11_models() {
    for version in [HttpVersion::Http10, HttpVersion::Http2] {
        let mut original = response(b"gzip", gzip(BINARY));
        original.version = version;
        original
            .headers
            .push(field("Transfer-Encoding", b"chunked"));
        assert_eq!(
            failed(&original, 256),
            "消息 Transfer-Encoding 不支持安全的 Content-Encoding 解包"
        );
    }
}

#[test]
fn transfer_coding_without_a_content_transform_remains_a_no_op() {
    let mut original = response(b"identity", gzip(BINARY));
    original
        .headers
        .push(field("Transfer-Encoding", b"gzip, chunked"));
    let before = original.clone();
    assert_eq!(unpack(&original, 0).unwrap(), None);
    assert_eq!(original, before);
    original
        .headers
        .retain(|header| !header.name.eq_ignore_ascii_case("content-encoding"));
    let before = original.clone();
    assert_eq!(unpack(&original, 0).unwrap(), None);
    assert_eq!(original, before);
}

#[test]
fn non_http11_without_transfer_coding_keeps_the_existing_raw_model() {
    for (version, with_trailers) in [
        (HttpVersion::Http10, false),
        (HttpVersion::Http2, false),
        (HttpVersion::Http2, true),
    ] {
        let mut original = response(b"gzip", gzip(BINARY));
        original.version = version;
        if with_trailers {
            original.trailers = vec![field("X-End", b"one"), field("X-End", b"\xfftwo")];
        }
        let before = original.clone();
        let decoded = unpack(&original, 256).unwrap().unwrap();
        assert_eq!(original, before);
        assert_eq!(decoded.version, version);
        assert_eq!(decoded.body, BINARY);
        assert_eq!(decoded.trailers, original.trailers);
        assert!(!decoded.headers.iter().any(|header| {
            header.name.eq_ignore_ascii_case("transfer-encoding")
                || header.name.eq_ignore_ascii_case("content-encoding")
                || (with_trailers && header.name.eq_ignore_ascii_case("content-length"))
        }));
        // The shared Raw format uses chunks as a trailer container, not HTTP/2 wire frames.
        let raw = parse_response(&response_to_raw(&decoded), version).unwrap();
        assert_eq!(raw.version, version);
        assert_eq!(raw.body, BINARY);
        assert_eq!(raw.trailers, original.trailers);
        assert_eq!(is_chunked(&raw.headers), with_trailers);
    }
}
