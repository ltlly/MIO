use super::*;
use std::{fs, io::Write, path::PathBuf};

struct Fixture {
    _directory: tempfile::TempDir,
    path: PathBuf,
}

impl Fixture {
    fn new(payload: &[u8]) -> Self {
        Self::schema(payload, 228)
    }

    fn schema(payload: &[u8], schema: u16) -> Self {
        let mut bytes = vec![0; 80 + payload.len().max(1)];
        bytes[..4].copy_from_slice(&[0x66, 0x85, 0x82, 0x80]);
        bytes[4..8].copy_from_slice(&1_i32.to_be_bytes());
        bytes[8..12].copy_from_slice(&0x8052_7974_u32.to_be_bytes());
        bytes[12..14].copy_from_slice(&schema.to_be_bytes());
        bytes[14..16].copy_from_slice(&schema.to_be_bytes());
        bytes[48..56].copy_from_slice(&i64::MAX.to_be_bytes());
        let length = bytes.len() as i64;
        bytes[56..64].copy_from_slice(&length.to_be_bytes());
        bytes[72..76].copy_from_slice(&((8 + payload.len().max(1)) as i32).to_be_bytes());
        bytes[76..80].copy_from_slice(&(payload.len() as i32).to_be_bytes());
        bytes[80..80 + payload.len()].copy_from_slice(payload);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("synthetic.archive");
        fs::write(&path, bytes).unwrap();
        Self {
            _directory: directory,
            path,
        }
    }

    fn open(&self) -> (Container, ArrayRecord) {
        let mut source = Container::open(&self.path).unwrap();
        let record = source.array_header(72, ArrayKind::Bytes).unwrap();
        (source, record)
    }

    fn change_length(&self) {
        fs::OpenOptions::new()
            .append(true)
            .open(&self.path)
            .unwrap()
            .write_all(&[0])
            .unwrap();
    }
}

fn decoded(source: &mut Container, record: &ArrayRecord, kind: MessageKind) -> MessageHead {
    match read_head(source, &Observation::Value(record.clone()), kind) {
        Observation::Value(value) => value,
        _ => panic!("selected records must remain present even on errors"),
    }
}

fn parsed(head: &MessageHead) -> &ParsedHead {
    match &head.state {
        HeadState::Parsed(value) => value,
        _ => panic!("expected a parsed synthetic HTTP head"),
    }
}

#[test]
fn duplicate_binary_headers_and_origin_form_survive_without_inventing_a_destination() {
    let raw = b"PATCH /a?x=1&x=2 HTTP/2.0\r\nHost: fixture.invalid:443\r\nX-Repeat: one\r\nx-repeat:\t\xff\x80\t\r\n\r\n\x00\xffbody";
    let fixture = Fixture::new(raw);
    let before = fs::read(&fixture.path).unwrap();
    let (mut source, record) = fixture.open();
    let head = decoded(&mut source, &record, MessageKind::Request);
    let parsed = parsed(&head);
    assert_eq!(
        parsed.start_line,
        StartLine::Request {
            method: "PATCH".into(),
            request_target: b"/a?x=1&x=2".to_vec(),
        }
    );
    assert_eq!(parsed.declared_version, HttpVersion::Http2);
    assert_eq!(
        parsed.headers,
        [
            Header {
                name: "Host".into(),
                value: b"fixture.invalid:443".to_vec()
            },
            Header {
                name: "X-Repeat".into(),
                value: b"one".to_vec()
            },
            Header {
                name: "x-repeat".into(),
                value: vec![255, 128]
            },
        ]
    );
    assert_eq!(head.record, record);
    let body = head.body.unwrap();
    assert_eq!(head.raw, raw[..body.offset as usize]);
    assert_eq!(&raw[body.offset as usize..], b"\x00\xffbody");
    assert_eq!(body.length, 6);
    assert_eq!(fs::read(&fixture.path).unwrap(), before);
}

#[test]
fn separators_cross_chunk_boundaries_and_lf_precedes_crlf_inside_the_body() {
    for delimiter in [b"\r\n\r\n".as_slice(), b"\n\n"] {
        for end in READ_CHUNK - 3..=READ_CHUNK {
            let mut payload = b"GET /fixture HTTP/1.1\nX-Pad: ".to_vec();
            payload.resize(end, b'a');
            payload.extend_from_slice(delimiter);
            let body_offset = payload.len();
            payload.extend_from_slice(b"body\r\n\r\n\0\xff");
            let fixture = Fixture::new(&payload);
            let (mut source, record) = fixture.open();
            let head = decoded(&mut source, &record, MessageKind::Request);
            assert_eq!(head.raw, payload[..body_offset]);
            assert_eq!(
                head.body,
                Some(BodyRange {
                    offset: body_offset as u64,
                    length: 10
                })
            );
            assert_eq!(
                parsed(&head).headers[0].value.len(),
                end - b"GET /fixture HTTP/1.1\nX-Pad: ".len()
            );
        }
    }
    let payload = b"POST / HTTP/1.1\n\nbody\r\n\r\nrest";
    let fixture = Fixture::new(payload);
    let (mut source, record) = fixture.open();
    let head = decoded(&mut source, &record, MessageKind::Request);
    assert_eq!(head.raw, b"POST / HTTP/1.1\n\n");
    assert_eq!(head.body.unwrap().length, 12);
}

#[test]
fn body_is_a_lazy_exact_range_and_is_not_chunk_or_content_decoded() {
    let head_bytes =
        b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nContent-Encoding: gzip\r\n\r\n";
    // Deliberately not valid chunked/gzip content: a head reader must not interpret it.
    let body: Vec<u8> = (0..=255).cycle().take(READ_CHUNK * 2 + 7).collect();
    let mut payload = head_bytes.to_vec();
    payload.extend_from_slice(&body);
    let fixture = Fixture::new(&payload);
    let (mut source, record) = fixture.open();
    let before = source.bytes_read();
    let head = decoded(&mut source, &record, MessageKind::Request);
    parsed(&head);
    assert_eq!(source.bytes_read() - before, READ_CHUNK as u64 + 8);
    assert_eq!(head.raw, head_bytes);
    assert_eq!(
        head.body,
        Some(BodyRange {
            offset: head_bytes.len() as u64,
            length: body.len() as u64
        })
    );
    let mut restored = vec![0; body.len()];
    source
        .read_payload(&head.record, head.body.unwrap().offset, &mut restored)
        .unwrap();
    assert_eq!(restored, body);
}

#[test]
fn versions_are_declared_only_and_response_reason_bytes_are_preserved() {
    for (version, expected) in [
        ("HTTP/1.0", HttpVersion::Http10),
        ("HTTP/1.1", HttpVersion::Http11),
        ("HTTP/2", HttpVersion::Http2),
        ("HTTP/2.0", HttpVersion::Http2),
    ] {
        let mut payload = format!("{version} 218 ").into_bytes();
        payload.extend_from_slice(b"custom \xff\tphrase\r\nX: one\r\nX: two\r\n\r\nbody");
        let fixture = Fixture::new(&payload);
        let (mut source, record) = fixture.open();
        let head = decoded(&mut source, &record, MessageKind::Response);
        assert_eq!(parsed(&head).declared_version, expected);
        assert_eq!(
            parsed(&head).start_line,
            StartLine::Response {
                status: 218,
                reason: b"custom \xff\tphrase".to_vec()
            }
        );
        assert_eq!(parsed(&head).headers.len(), 2);
        assert!(head.raw.starts_with(version.as_bytes()));
        assert_eq!(head.body.unwrap().length, 4);
    }
    let fixture = Fixture::new(b"HTTP/2 204\r\n\r\n");
    let (mut source, record) = fixture.open();
    let head = decoded(&mut source, &record, MessageKind::Response);
    assert_eq!(
        parsed(&head).start_line,
        StartLine::Response {
            status: 204,
            reason: vec![]
        }
    );
}

#[test]
fn unknown_versions_and_malformed_heads_keep_the_record_and_errors_exclude_contents() {
    for payload in [
        b"GET / HTTP/9.9\r\n\r\nSECRET_BODY".as_slice(),
        b"GET / HTTP/3\r\n\r\nSECRET_BODY",
    ] {
        let fixture = Fixture::new(payload);
        let (mut source, record) = fixture.open();
        let head = decoded(&mut source, &record, MessageKind::Request);
        assert!(matches!(head.state, HeadState::Unsupported(_)));
        assert_eq!(head.body.unwrap().length, 11);
        assert_eq!(head.record, record);
        assert!(head.raw.starts_with(b"GET / HTTP/"));
        assert!(!format!("{:?}", head.state).contains("SECRET"));
    }
    for (payload, kind) in [
        (
            b"GET / HTTP/1.x\r\n\r\nSECRET".as_slice(),
            MessageKind::Request,
        ),
        (
            b"GET / HTTP/1.1\r\nBad Header: SECRET\r\n\r\n",
            MessageKind::Request,
        ),
        (
            b"GET / HTTP/1.1\r\nX: SECRET\0\r\n\r\n",
            MessageKind::Request,
        ),
        (
            b"GET / HTTP/1.1\r\nX: SECRET\x7f\r\n\r\n",
            MessageKind::Request,
        ),
        (
            b"GET / HTTP/1.1\r\nX: \x0bSECRET\r\n\r\n",
            MessageKind::Request,
        ),
        (
            b"GET / HTTP/1.1\r\nX: SECRET\rwrong\r\n\r\n",
            MessageKind::Request,
        ),
        (b"GET / SECRET HTTP/1.1\r\n\r\n", MessageKind::Request),
        (b"HTTP/1.1 099 SECRET\r\n\r\n", MessageKind::Response),
        (b"HTTP/1.1 2000 SECRET\r\n\r\n", MessageKind::Response),
        (b"HTTP/1.1 xyz SECRET\r\n\r\n", MessageKind::Response),
        (b"GET / HTTP/1.1\r\nSECRET", MessageKind::Request),
    ] {
        let fixture = Fixture::new(payload);
        let (mut source, record) = fixture.open();
        let head = decoded(&mut source, &record, kind);
        assert!(matches!(head.state, HeadState::Invalid(_)));
        assert_eq!(head.record, record);
        assert_eq!(
            head.raw,
            payload[..head
                .body
                .map(|body| body.offset as usize)
                .unwrap_or(payload.len())]
        );
        assert!(!format!("{:?}", head.state).contains("SECRET"));
    }
}

#[test]
fn absent_null_empty_unavailable_failed_schema_and_record_kind_are_distinct() {
    let fixture = Fixture::new(&[]);
    let (mut source, record) = fixture.open();
    let before = source.bytes_read();
    for (input, expected) in [
        (Observation::Missing, Observation::Missing),
        (Observation::Null, Observation::Null),
        (
            Observation::Unavailable {
                parent: "root.f1".into(),
            },
            Observation::Unavailable {
                parent: "root.f1".into(),
            },
        ),
        (
            Observation::Failed(Failure {
                kind: std::io::ErrorKind::InvalidData,
                message: "mapping failed".into(),
            }),
            Observation::Failed(Failure {
                kind: std::io::ErrorKind::InvalidData,
                message: "mapping failed".into(),
            }),
        ),
    ] {
        assert_eq!(
            read_head(&mut source, &input, MessageKind::Request),
            expected
        );
    }
    assert_eq!(source.bytes_read(), before);
    let head = decoded(&mut source, &record, MessageKind::Request);
    assert_eq!(head.state, HeadState::Empty);
    assert_eq!(head.body, None);
    assert_eq!(source.bytes_read() - before, 8);
    let before = source.bytes_read();
    let mut wrong_kind = record.clone();
    wrong_kind.kind = ArrayKind::References;
    let head = decoded(&mut source, &wrong_kind, MessageKind::Request);
    assert!(matches!(head.state, HeadState::Unsupported(_)));
    assert_eq!(source.bytes_read(), before);
    let fixture = Fixture::schema(b"GET / HTTP/1.1\r\n\r\n", 226);
    let (mut source, record) = fixture.open();
    let before = source.bytes_read();
    let head = decoded(&mut source, &record, MessageKind::Request);
    assert!(matches!(head.state, HeadState::Unsupported(_)));
    assert_eq!(source.bytes_read(), before);
}

#[test]
fn head_limit_is_exact_and_does_not_read_the_remaining_large_record() {
    for (end, delimiter, valid) in [
        (MAX_HEAD, b"\r\n\r\n".as_slice(), true),
        (MAX_HEAD + 1, b"\n\n".as_slice(), false),
        (MAX_HEAD + 1, b"\r\n\r\n".as_slice(), false),
    ] {
        let mut payload = b"GET / HTTP/1.1\r\nX-Pad: ".to_vec();
        payload.resize(end, b'a');
        payload.extend_from_slice(delimiter);
        payload.resize(payload.len() + READ_CHUNK, b'b');
        let fixture = Fixture::new(&payload);
        let (mut source, record) = fixture.open();
        let before = source.bytes_read();
        let head = decoded(&mut source, &record, MessageKind::Request);
        if valid {
            parsed(&head);
            assert_eq!(
                head.body,
                Some(BodyRange {
                    offset: (end + delimiter.len()) as u64,
                    length: READ_CHUNK as u64
                })
            );
        } else {
            assert_eq!(head.state, HeadState::LimitExceeded);
        }
        assert!(head.raw.len() <= MAX_HEAD + 4);
        assert!(
            source.bytes_read() - before
                <= (MAX_HEAD + 4 + 8 * (MAX_HEAD + 4).div_ceil(READ_CHUNK)) as u64
        );
        assert_eq!(head.record, record);
    }
}

#[test]
fn source_changes_and_forged_metadata_are_explicit_without_publishing_stale_head_bytes() {
    let fixture = Fixture::new(b"GET / HTTP/1.1\r\n\r\noriginal body");
    let (mut source, record) = fixture.open();
    let mut forged = record.clone();
    forged.payload_bytes += 1;
    let head = decoded(&mut source, &forged, MessageKind::Request);
    assert!(matches!(head.state, HeadState::ReadFailed(_)));
    assert!(head.raw.is_empty());
    assert_eq!(head.record, forged);
    let head = decoded(&mut source, &record, MessageKind::Request);
    parsed(&head);
    fixture.change_length();
    let before = source.bytes_read();
    let changed = decoded(&mut source, &record, MessageKind::Request);
    assert!(matches!(
        changed.state,
        HeadState::ReadFailed(Failure {
            kind: std::io::ErrorKind::InvalidData,
            ..
        })
    ));
    assert!(changed.raw.is_empty());
    assert_eq!(changed.body, None);
    assert_eq!(source.bytes_read(), before);
    assert!(
        source
            .read_payload(&record, head.body.unwrap().offset, &mut [0])
            .is_err()
    );
    assert_eq!(source.bytes_read(), before);
}

#[test]
fn schema_229_current_head_keeps_bytes_and_lazy_body_while_general_entry_stays_closed() {
    let head_bytes = b"POST /current?x=1&x=2 HTTP/2.0\r\nHost: fixture.invalid:443\r\nX-A: one\r\nX-A:\t\xff\x80\t\r\n\r\n";
    let body: Vec<u8> = (0..=255).cycle().take(READ_CHUNK * 2 + 7).collect();
    let payload = [head_bytes.as_slice(), &body].concat();
    let fixture = Fixture::schema(&payload, 229);
    let original = fs::read(&fixture.path).unwrap();
    let (mut source, record) = fixture.open();
    let before = source.bytes_read();
    for kind in [MessageKind::Request, MessageKind::Response] {
        let head = decoded(&mut source, &record, kind);
        assert!(matches!(head.state, HeadState::Unsupported(_)));
        assert_eq!(head.record, record);
        assert!(head.raw.is_empty() && head.body.is_none());
        assert_eq!(source.bytes_read(), before);
    }
    let head = read_current_request_229(&mut source, &record);
    assert_eq!(head.raw, head_bytes);
    assert_eq!(head.record, record);
    assert_eq!(
        head.body,
        Some(BodyRange {
            offset: head_bytes.len() as u64,
            length: body.len() as u64,
        })
    );
    let parsed = parsed(&head);
    assert_eq!(parsed.declared_version, HttpVersion::Http2);
    assert_eq!(
        parsed.start_line,
        StartLine::Request {
            method: "POST".into(),
            request_target: b"/current?x=1&x=2".to_vec(),
        }
    );
    assert_eq!(
        parsed.headers,
        [
            Header {
                name: "Host".into(),
                value: b"fixture.invalid:443".to_vec()
            },
            Header {
                name: "X-A".into(),
                value: b"one".to_vec()
            },
            Header {
                name: "X-A".into(),
                value: vec![255, 128]
            },
        ]
    );
    assert_eq!(source.bytes_read() - before, READ_CHUNK as u64 + 8);
    let mut recovered = vec![0; body.len()];
    source
        .read_payload(&record, head.body.unwrap().offset, &mut recovered)
        .unwrap();
    assert_eq!(recovered, body);
    assert_eq!(fs::read(&fixture.path).unwrap(), original);
}

#[test]
fn schema_229_current_head_rejects_wrong_schema_kind_response_and_changed_source() {
    for (floor, revision) in [
        (226_u16, 226_u16),
        (228, 228),
        (228, 229),
        (229, 228),
        (230, 230),
    ] {
        let fixture = Fixture::schema(b"GET /current HTTP/1.1\r\n\r\n", floor);
        let mut bytes = fs::read(&fixture.path).unwrap();
        bytes[14..16].copy_from_slice(&revision.to_be_bytes());
        fs::write(&fixture.path, &bytes).unwrap();
        let (mut source, record) = fixture.open();
        let before = source.bytes_read();
        let head = read_current_request_229(&mut source, &record);
        assert!(
            matches!(head.state, HeadState::Unsupported(_)),
            "{floor}/{revision}"
        );
        assert!(head.raw.is_empty() && head.body.is_none());
        assert_eq!(source.bytes_read(), before);
        assert_eq!(fs::read(&fixture.path).unwrap(), bytes);
    }
    let fixture = Fixture::schema(b"HTTP/1.1 200 OK\r\n\r\n", 229);
    let (mut source, record) = fixture.open();
    let mut wrong_kind = record.clone();
    wrong_kind.kind = ArrayKind::Utf16;
    let before = source.bytes_read();
    assert!(matches!(
        read_current_request_229(&mut source, &wrong_kind).state,
        HeadState::Unsupported(_)
    ));
    assert_eq!(source.bytes_read(), before);
    assert!(matches!(
        read_current_request_229(&mut source, &record).state,
        HeadState::Invalid(_)
    ));
    fixture.change_length();
    let before = source.bytes_read();
    let head = read_current_request_229(&mut source, &record);
    assert!(matches!(head.state, HeadState::ReadFailed(_)));
    assert_eq!(source.bytes_read(), before);
    assert!(head.raw.is_empty() && head.body.is_none());
}
