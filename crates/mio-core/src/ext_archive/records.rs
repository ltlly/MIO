//! Typed record envelopes and bounded byte fragments, not a project-field mapping.
//!
//! Describes direct byte-record layouts for selected compact-object slots.
//! Callers must supply the expected storage kind and retain every failed/unknown result.

use crate::ext_archive::{ArrayKind, ArrayRecord, CompactObject, Container};
use std::io;

const PREFIX_BYTES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedRecord {
    Compact,
    Bytes,
    Utf16,
    References,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    Compact(CompactObject),
    Array(ArrayRecord),
}

/// No fallback is attempted: compact and array headers are not disjoint type tags.
pub fn read(source: &mut Container, address: u64, expected: ExpectedRecord) -> io::Result<Record> {
    let kind = match expected {
        ExpectedRecord::Compact => return source.compact_object(address).map(Record::Compact),
        ExpectedRecord::Bytes => ArrayKind::Bytes,
        ExpectedRecord::Utf16 => ArrayKind::Utf16,
        ExpectedRecord::References => ArrayKind::References,
    };
    source.array_header(address, kind).map(Record::Array)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePrefix {
    Empty,
    RequestLike,
    ResponseLike,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrefixSample {
    pub category: MessagePrefix,
    pub bytes_examined: u8,
}

fn bytes_record(record: &Record) -> io::Result<&ArrayRecord> {
    match record {
        Record::Array(array) if array.kind == ArrayKind::Bytes => Ok(array),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Message fragments require an explicitly selected byte record",
        )),
    }
}

fn classify_prefix(bytes: &[u8]) -> MessagePrefix {
    if bytes.is_empty() {
        return MessagePrefix::Empty;
    }
    for version in [
        b"HTTP/1.0 ".as_slice(),
        b"HTTP/1.1 ",
        b"HTTP/2 ",
        b"HTTP/3 ",
    ] {
        if let Some(status) = bytes.strip_prefix(version)
            && status.len() >= 4
            && (b'1'..=b'5').contains(&status[0])
            && status[1..3].iter().all(u8::is_ascii_digit)
            && matches!(status[3], b' ' | b'\r')
        {
            return MessagePrefix::ResponseLike;
        }
    }
    for method in [
        b"GET ".as_slice(),
        b"HEAD ",
        b"POST ",
        b"PUT ",
        b"DELETE ",
        b"CONNECT ",
        b"OPTIONS ",
        b"TRACE ",
        b"PATCH ",
    ] {
        if let Some(target) = bytes.strip_prefix(method)
            && target.first().is_some_and(|byte| byte.is_ascii_graphic())
        {
            return MessagePrefix::RequestLike;
        }
    }
    MessagePrefix::Other
}

/// A hint only, not HTTP validation, protocol negotiation, or product ownership.
/// Unknown methods and non-text messages remain `Other`, never omitted records.
pub fn probe_prefix(source: &mut Container, record: &Record) -> io::Result<PrefixSample> {
    let array = bytes_record(record)?;
    let count = array.payload_bytes.min(PREFIX_BYTES as u64) as usize;
    let mut bytes = [0; PREFIX_BYTES];
    source.read_payload(array, 0, &mut bytes[..count])?;
    Ok(PrefixSample {
        category: classify_prefix(&bytes[..count]),
        bytes_examined: count as u8,
    })
}

/// Reads exact original bytes with the Container's 64 KiB limit and change checks.
pub fn read_fragment(
    source: &mut Container,
    record: &Record,
    offset: u64,
    output: &mut [u8],
) -> io::Result<()> {
    source.read_payload(bytes_record(record)?, offset, output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Fixture {
        path: PathBuf,
    }

    impl Fixture {
        fn new(payload: &[u8]) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let mut bytes = vec![0; 1024];
            bytes[..4].copy_from_slice(&[0x66, 0x85, 0x82, 0x80]);
            bytes[4..8].copy_from_slice(&1_i32.to_be_bytes());
            bytes[8..12].copy_from_slice(&0x8052_7974_u32.to_be_bytes());
            bytes[12..14].copy_from_slice(&228_u16.to_be_bytes());
            bytes[14..16].copy_from_slice(&228_u16.to_be_bytes());
            bytes[48..56].copy_from_slice(&i64::MAX.to_be_bytes());
            bytes[56..64].copy_from_slice(&1024_i64.to_be_bytes());
            bytes[64..72].copy_from_slice(&72_i64.to_be_bytes());
            bytes[72..82].copy_from_slice(&[0, 0, 0, 2, 1, 0, 10, 2, 0, 18]);
            bytes[82..90].copy_from_slice(&256_i64.to_be_bytes());
            put_array(&mut bytes, 256, 1, payload);
            put_array(&mut bytes, 800, 2, &[0x4e, 0x2d, 0xd8, 0x00]);
            put_array(&mut bytes, 832, 8, &72_i64.to_be_bytes());
            for _ in 0..32 {
                let directory = std::env::temp_dir().join(format!(
                    "mio-ext-records-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&directory) {
                    Ok(()) => {
                        let path = directory.join("synthetic.archive");
                        fs::write(&path, bytes).unwrap();
                        return Self { path };
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => panic!("cannot create synthetic record fixture: {error}"),
                }
            }
            panic!("could not allocate a unique synthetic record fixture directory");
        }

        fn open(&self) -> Container {
            Container::open(&self.path).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
            let _ = fs::remove_dir(self.path.parent().unwrap());
        }
    }

    fn put_array(bytes: &mut [u8], address: usize, width: usize, payload: &[u8]) {
        assert_eq!(payload.len() % width, 0);
        bytes[address..address + 4]
            .copy_from_slice(&((8 + payload.len().max(1)) as i32).to_be_bytes());
        bytes[address + 4..address + 8]
            .copy_from_slice(&((payload.len() / width) as i32).to_be_bytes());
        bytes[address + 8..address + 8 + payload.len()].copy_from_slice(payload);
    }

    #[test]
    fn envelopes_read_metadata_only_and_preserve_the_explicit_kind() {
        let fixture = Fixture::new(b"GET /fixture HTTP/1.1\r\n\r\n");
        let mut source = fixture.open();
        let before = source.bytes_read();
        let compact = read(&mut source, 72, ExpectedRecord::Compact).unwrap();
        assert!(matches!(compact, Record::Compact(ref value) if value.descriptors.len() == 2));
        assert_eq!(source.bytes_read() - before, 10);
        for (address, expected, kind) in [
            (256, ExpectedRecord::Bytes, ArrayKind::Bytes),
            (800, ExpectedRecord::Utf16, ArrayKind::Utf16),
            (832, ExpectedRecord::References, ArrayKind::References),
        ] {
            let before = source.bytes_read();
            assert!(
                matches!(read(&mut source, address, expected).unwrap(), Record::Array(value) if value.kind == kind)
            );
            assert_eq!(source.bytes_read() - before, 8);
        }
        assert!(read(&mut source, 72, ExpectedRecord::Bytes).is_err());
        assert!(read(&mut source, 800, ExpectedRecord::Bytes).is_err());
    }

    #[test]
    fn prefix_probes_are_bounded_and_return_no_payload() {
        for (payload, category) in [
            (
                b"GET /fixture HTTP/1.1\r\n\r\n".as_slice(),
                MessagePrefix::RequestLike,
            ),
            (
                b"HTTP/1.1 200 OK\r\n\r\nfixture",
                MessagePrefix::ResponseLike,
            ),
            (
                b"HTTP/2 204 No Content\r\n\r\n",
                MessagePrefix::ResponseLike,
            ),
            (b"PROPFIND /fixture HTTP/1.1\r\n\r\n", MessagePrefix::Other),
            (b"\x00\xff\r\n", MessagePrefix::Other),
            (b"", MessagePrefix::Empty),
        ] {
            let fixture = Fixture::new(payload);
            let mut source = fixture.open();
            let record = read(&mut source, 256, ExpectedRecord::Bytes).unwrap();
            let before = source.bytes_read();
            let sample = probe_prefix(&mut source, &record).unwrap();
            assert_eq!(sample.category, category);
            assert_eq!(usize::from(sample.bytes_examined), payload.len().min(16));
            assert_eq!(
                source.bytes_read() - before,
                8 + u64::from(sample.bytes_examined)
            );
        }
    }

    #[test]
    fn partial_or_invalid_signatures_are_not_promoted_to_http() {
        for bytes in [
            b"HTTP/1.1 ".as_slice(),
            b"HTTP/1.1 20",
            b"HTTP/1.1 200",
            b"HTTP/1.1 000 OK",
            b"HTTP/1.1 2000",
            b"HTTP/1.1 xyz OK",
            b"GET ",
            b"GET \r\n",
            b"get /",
            b"GETTING /",
        ] {
            assert_eq!(classify_prefix(bytes), MessagePrefix::Other);
        }
        assert_eq!(
            classify_prefix(b"HTTP/1.0 218 \r\n"),
            MessagePrefix::ResponseLike
        );
        assert_eq!(classify_prefix(b"OPTIONS *"), MessagePrefix::RequestLike);
        assert_eq!(
            classify_prefix(b"CONNECT fixture"),
            MessagePrefix::RequestLike
        );
    }

    #[test]
    fn fragments_preserve_all_bytes_without_implicit_text_conversion() {
        let payload = (0..=255).collect::<Vec<u8>>();
        let fixture = Fixture::new(&payload);
        let original = fs::read(&fixture.path).unwrap();
        let mut source = fixture.open();
        let record = read(&mut source, 256, ExpectedRecord::Bytes).unwrap();
        let mut output = [0; 256];
        read_fragment(&mut source, &record, 0, &mut output[..127]).unwrap();
        read_fragment(&mut source, &record, 127, &mut output[127..]).unwrap();
        assert_eq!(output.as_slice(), payload);
        assert_eq!(fs::read(&fixture.path).unwrap(), original);
        assert!(read_fragment(&mut source, &record, 256, &mut [0]).is_err());
        assert!(read_fragment(&mut source, &record, u64::MAX, &mut [0]).is_err());
        assert!(read_fragment(&mut source, &record, 0, &mut vec![0; 65537]).is_err());
    }

    #[test]
    fn non_byte_envelopes_cannot_be_treated_as_messages() {
        let fixture = Fixture::new(b"fixture");
        let mut source = fixture.open();
        for (address, expected) in [
            (72, ExpectedRecord::Compact),
            (800, ExpectedRecord::Utf16),
            (832, ExpectedRecord::References),
        ] {
            let record = read(&mut source, address, expected).unwrap();
            let before = source.bytes_read();
            assert_eq!(
                probe_prefix(&mut source, &record).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
            assert_eq!(
                read_fragment(&mut source, &record, 0, &mut [0])
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
            assert_eq!(source.bytes_read(), before);
        }
    }

    #[test]
    fn source_changes_and_forged_record_metadata_remain_errors() {
        let fixture = Fixture::new(b"fixture");
        let mut source = fixture.open();
        let mut record = read(&mut source, 256, ExpectedRecord::Bytes).unwrap();
        if let Record::Array(array) = &mut record {
            array.payload_bytes += 1;
        }
        assert!(
            probe_prefix(&mut source, &record)
                .unwrap_err()
                .to_string()
                .contains("metadata")
        );
        let record = read(&mut source, 256, ExpectedRecord::Bytes).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&fixture.path)
            .unwrap()
            .set_len(900)
            .unwrap();
        let before = source.bytes_read();
        assert!(
            probe_prefix(&mut source, &record)
                .unwrap_err()
                .to_string()
                .contains("source changed")
        );
        assert!(read_fragment(&mut source, &record, 0, &mut [0]).is_err());
        assert_eq!(source.bytes_read(), before);
    }
}
