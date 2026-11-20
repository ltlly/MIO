use super::*;
use std::{fs, path::PathBuf};

#[path = "tests/fragments.rs"]
mod fragments;
#[path = "tests/history.rs"]
mod history;
#[path = "tests/http_annotations.rs"]
mod http_annotations;
#[path = "tests/http_time.rs"]
mod http_time;
#[path = "tests/long_titles.rs"]
mod long_titles;
#[path = "tests/repeater_groups.rs"]
mod repeater_groups;
#[path = "tests/schema229_titles.rs"]
mod schema229_titles;
#[path = "tests/tabs.rs"]
mod tabs;

use schema229_titles::{chunked_current_229, replace_field};

struct Builder {
    bytes: Vec<u8>,
}

impl Builder {
    fn new() -> Self {
        let mut bytes = vec![0; 72];
        bytes[..4].copy_from_slice(&[0x66, 0x85, 0x82, 0x80]);
        bytes[4..8].copy_from_slice(&1_i32.to_be_bytes());
        bytes[8..12].copy_from_slice(&0x8052_7974_u32.to_be_bytes());
        bytes[12..14].copy_from_slice(&228_u16.to_be_bytes());
        bytes[14..16].copy_from_slice(&228_u16.to_be_bytes());
        bytes[48..56].copy_from_slice(&i64::MAX.to_be_bytes());
        Self { bytes }
    }

    fn raw(&mut self, value: &[u8]) -> u64 {
        if !self.bytes.len().is_multiple_of(2) {
            self.bytes.push(0);
        }
        let address = self.bytes.len() as u64;
        self.bytes.extend_from_slice(value);
        address
    }

    fn array(&mut self, width: usize, payload: &[u8]) -> u64 {
        assert!(payload.len().is_multiple_of(width));
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&((8 + payload.len().max(1)) as i32).to_be_bytes());
        bytes.extend_from_slice(&((payload.len() / width) as i32).to_be_bytes());
        bytes.extend_from_slice(payload);
        if payload.is_empty() {
            bytes.push(0);
        }
        self.raw(&bytes)
    }

    fn bytes(&mut self, payload: &[u8]) -> u64 {
        self.array(1, payload)
    }

    fn references(&mut self, references: &[i64]) -> u64 {
        let payload: Vec<_> = references
            .iter()
            .flat_map(|value| value.to_be_bytes())
            .collect();
        self.array(8, &payload)
    }

    fn object(&mut self, tag: (u8, u8), fields: Vec<(u8, Vec<u8>)>) -> u64 {
        let mut bytes = vec![0, tag.0, tag.1, fields.len().try_into().unwrap()];
        let mut offset = 4 + fields.len() * 3;
        for (id, value) in &fields {
            assert!(!value.is_empty());
            bytes.push(*id);
            bytes.extend_from_slice(&i16::try_from(offset).unwrap().to_be_bytes());
            offset += value.len();
        }
        for (_, value) in fields {
            bytes.extend(value);
        }
        self.raw(&bytes)
    }

    fn direct(&mut self, length: i32, addresses: u64) -> u64 {
        self.object((0, 0), vec![integer(0, length), reference(1, addresses)])
    }

    fn pair(&mut self, request: u64, response: u64) -> u64 {
        self.object((1, 0), vec![reference(3, response), reference(2, request)])
    }

    fn forward(&mut self, address: u64) -> u64 {
        let mut value = vec![1];
        value.extend_from_slice(&(address as i64).to_be_bytes());
        self.raw(&value)
    }

    fn finish(mut self, root: u64) -> Fixture {
        let length = self.bytes.len() as i64;
        self.bytes[56..64].copy_from_slice(&length.to_be_bytes());
        self.bytes[64..72].copy_from_slice(&(root as i64).to_be_bytes());
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("synthetic.archive");
        fs::write(&path, self.bytes).unwrap();
        Fixture {
            _directory: directory,
            path,
        }
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    path: PathBuf,
}

impl Fixture {
    fn open(&self) -> Container {
        Container::open(&self.path).unwrap()
    }
}

fn reference(id: u8, address: u64) -> (u8, Vec<u8>) {
    (id, (address as i64).to_be_bytes().to_vec())
}

fn integer(id: u8, value: i32) -> (u8, Vec<u8>) {
    (id, value.to_be_bytes().to_vec())
}

fn value<T>(state: &Observation<T>) -> &T {
    match state {
        Observation::Value(value) => value,
        _ => panic!("expected a decoded value"),
    }
}

fn failed<T>(state: &Observation<T>, kind: io::ErrorKind) {
    match state {
        Observation::Failed(error) => assert_eq!(error.kind, kind),
        _ => panic!("expected an explicit field failure"),
    }
}

#[test]
fn http_slots_are_lazy_and_keep_unverified_references_unknown_fields_missing_and_null() {
    let mut builder = Builder::new();
    let request = builder.bytes(b"GET /synthetic HTTP/1.1\r\nHost: fixture\r\n\r\n");
    let response = builder.bytes(&(0..=255).collect::<Vec<_>>());
    let unverified = builder.raw(&[0; 8]);
    let item = builder.object(
        (1, 0),
        vec![
            reference(18, response),
            integer(127, 1),
            reference(16, request),
            reference(15, request),
            reference(19, 0),
            (99, i64::MIN.to_be_bytes().to_vec()),
            reference(20, unverified),
        ],
    );
    let fixture = builder.finish(item);
    let before = fs::read(&fixture.path).unwrap();
    let mut source = fixture.open();
    let mut decoder = Decoder::new(&mut source, MAX_BYTE_BUDGET).unwrap();
    let decoded = decoder.http_history_item(item).unwrap();
    let fields = value(&decoded.value);
    assert_eq!(fields.request.id, 15);
    assert_eq!(fields.response.id, 18);
    assert_eq!(
        fields
            .unverified_request_references
            .each_ref()
            .map(|field| field.id),
        [16, 17]
    );
    assert_eq!(
        fields
            .unverified_response_references
            .each_ref()
            .map(|field| field.id),
        [19, 20]
    );
    assert_eq!(value(&fields.request.state).address, request);
    assert_eq!(
        fields.unverified_request_references[0].state,
        Observation::Value(request)
    );
    assert_eq!(
        fields.unverified_request_references[1].state,
        Observation::Missing
    );
    assert_eq!(
        fields.unverified_response_references[0].state,
        Observation::Null
    );
    assert_eq!(value(&fields.response.state).payload_bytes, 256);
    assert_eq!(
        fields.unverified_response_references[1].state,
        Observation::Value(unverified)
    );
    assert_eq!(
        decoded
            .object
            .descriptors
            .iter()
            .map(|field| field.id)
            .collect::<Vec<_>>(),
        [18, 127, 16, 15, 19, 99, 20]
    );
    // Only the two verified slots read array framing; no alternate prefix or payload reads.
    assert_eq!(decoder.bytes_read(), 4 + 3 * 7 + 8 * 5 + 8 * 2);
    assert_eq!(fs::read(&fixture.path).unwrap(), before);
}

#[test]
fn malformed_slots_are_visible_and_do_not_hide_empty_records_or_unverified_references() {
    let mut builder = Builder::new();
    let bad_array = builder.raw(&[0; 8]);
    let bytes = builder.bytes(&[0, 255, 0, 1]);
    let empty = builder.bytes(&[]);
    let item = builder.object(
        (1, 0),
        vec![
            reference(15, bad_array),
            (16, (-1_i64).to_be_bytes().to_vec()),
            reference(17, bytes),
            reference(18, empty),
            (19, vec![1]),
            reference(20, 0),
        ],
    );
    let fixture = builder.finish(item);
    let mut source = fixture.open();
    let mut decoder = Decoder::new(&mut source, MAX_BYTE_BUDGET).unwrap();
    let decoded = decoder.http_history_item(item).unwrap();
    let fields = value(&decoded.value);
    failed(&fields.request.state, io::ErrorKind::InvalidData);
    failed(
        &fields.unverified_request_references[0].state,
        io::ErrorKind::InvalidData,
    );
    failed(
        &fields.unverified_response_references[0].state,
        io::ErrorKind::InvalidData,
    );
    assert_eq!(
        fields.unverified_request_references[1].state,
        Observation::Value(bytes)
    );
    assert_eq!(value(&fields.response.state).payload_bytes, 0);
    assert_eq!(value(&fields.response.state).total_size, 9);
    assert_eq!(
        fields.unverified_response_references[1].state,
        Observation::Null
    );
}
