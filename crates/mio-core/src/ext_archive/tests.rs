use super::*;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

mod fields;
mod pages;

struct Fixture {
    path: PathBuf,
}

impl Fixture {
    fn new(bytes: &[u8]) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..32 {
            let directory = std::env::temp_dir().join(format!(
                "mio-ext-structure-{}-{}",
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
                Err(error) => panic!("cannot create synthetic fixture: {error}"),
            }
        }
        panic!("could not allocate a unique synthetic fixture directory");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_dir(self.path.parent().unwrap());
    }
}

fn put_i64(bytes: &mut [u8], offset: usize, value: i64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
}

fn put_object(bytes: &mut [u8], offset: usize, fields: &[(u8, i16)]) {
    bytes[offset..offset + 4].copy_from_slice(&[0, 0, 7, fields.len().try_into().unwrap()]);
    for (index, (id, position)) in fields.iter().enumerate() {
        let entry = offset + 4 + index * 3;
        bytes[entry] = *id;
        bytes[entry + 1..entry + 3].copy_from_slice(&position.to_be_bytes());
    }
}

fn put_forward(bytes: &mut [u8], offset: usize, replacement: i64) {
    bytes[offset] = 1;
    put_i64(bytes, offset + 1, replacement);
}

fn fixture_bytes() -> Vec<u8> {
    let mut bytes = vec![0xa5; 512];
    bytes[..4].copy_from_slice(&MAGIC);
    bytes[4..8].copy_from_slice(&1_i32.to_be_bytes());
    bytes[8..12].copy_from_slice(&COMPATIBILITY.to_be_bytes());
    bytes[12..14].copy_from_slice(&226_u16.to_be_bytes());
    bytes[14..16].copy_from_slice(&226_u16.to_be_bytes());
    put_i64(&mut bytes, 40, 72);
    put_i64(&mut bytes, 48, i64::MAX);
    put_i64(&mut bytes, 56, 192);
    put_i64(&mut bytes, 64, 128);
    put_object(&mut bytes, 72, &[(0, 7)]);
    put_object(&mut bytes, 128, &[(0, 13), (15, 21), (16, 29)]);
    bytes
}

fn put_array(bytes: &mut [u8], offset: usize, width: usize, payload: &[u8]) {
    assert_eq!(payload.len() % width, 0);
    let total = 8 + payload.len().max(1);
    bytes[offset..offset + 4].copy_from_slice(&(total as i32).to_be_bytes());
    bytes[offset + 4..offset + 8].copy_from_slice(&((payload.len() / width) as i32).to_be_bytes());
    bytes[offset + 8..offset + 8 + payload.len()].copy_from_slice(payload);
}

fn put_refs(bytes: &mut [u8], offset: usize, values: &[i64]) {
    let payload = values
        .iter()
        .flat_map(|value| value.to_be_bytes())
        .collect::<Vec<_>>();
    put_array(bytes, offset, 8, &payload);
}

fn primitive_bytes() -> Vec<u8> {
    let mut bytes = fixture_bytes();
    bytes.resize(2048, 0);
    put_i64(&mut bytes, 56, 1024);
    put_object(&mut bytes, 192, &[(10, 10), (11, 14)]);
    bytes[202..206].copy_from_slice(&(-42_i32).to_be_bytes());
    put_i64(&mut bytes, 206, 72);
    put_array(&mut bytes, 256, 1, &(0..=255).collect::<Vec<u8>>());
    put_array(&mut bytes, 528, 1, &[]);
    put_refs(&mut bytes, 544, &[0, 72, 128, 72]);
    let units = [0x4e2d_u16, 0xd83d, 0xde00, 0xd800];
    let utf16 = units
        .iter()
        .flat_map(|unit| unit.to_be_bytes())
        .collect::<Vec<_>>();
    put_array(&mut bytes, 600, 2, &utf16);
    put_refs(&mut bytes, 800, &[848, 880, 1024]);
    put_refs(&mut bytes, 848, &[1024, 72]);
    put_refs(&mut bytes, 880, &[128, 192]);
    bytes
}

fn assert_error(bytes: &[u8], kind: io::ErrorKind, message: &str) {
    let fixture = Fixture::new(bytes);
    let error = inspect(&fixture.path).unwrap_err();
    assert_eq!(error.kind(), kind, "{error}");
    assert!(error.to_string().contains(message), "{error}");
}

#[test]
fn reports_only_structure_and_leaves_the_source_unchanged() {
    let bytes = fixture_bytes();
    let fixture = Fixture::new(&bytes);
    let report = inspect(&fixture.path).unwrap();
    assert_eq!(report.file_size, 512);
    assert_eq!(report.header.outer_version, 1);
    assert_eq!(report.header.compatibility, COMPATIBILITY);
    assert_eq!(report.header.schema_floor, 226);
    assert_eq!(report.header.schema_revision, 226);
    assert_eq!(report.header.allocation_cursor, 192);
    assert_eq!(report.header.physical_address_span, i64::MAX as u64);
    assert_eq!(report.metadata_root.unwrap().descriptors.len(), 1);
    let root = report.project_root.unwrap();
    assert_eq!(root.address, 128);
    assert_eq!((root.flags, root.object_type, root.subtype), (0, 0, 7));
    assert_eq!(
        root.descriptors,
        vec![
            FieldDescriptor {
                id: 0,
                relative_offset: 13
            },
            FieldDescriptor {
                id: 15,
                relative_offset: 21
            },
            FieldDescriptor {
                id: 16,
                relative_offset: 29
            },
        ]
    );
    assert!(root.forwarded_from.is_empty());
    assert_eq!(report.bytes_read, 48 + 7 + 13);
    assert_eq!(report.objects_read, 2);
    assert_eq!(fs::read(&fixture.path).unwrap(), bytes);
}

#[test]
fn retains_unknown_schema_flags_types_and_field_ids_without_interpreting_values() {
    let mut bytes = fixture_bytes();
    bytes[12..14].copy_from_slice(&228_u16.to_be_bytes());
    bytes[14..16].copy_from_slice(&65000_u16.to_be_bytes());
    bytes[128..131].copy_from_slice(&[0x80, 0xfe, 0xfd]);
    bytes[132] = 255;
    let fixture = Fixture::new(&bytes);
    let report = inspect(&fixture.path).unwrap();
    assert_eq!(
        (report.header.schema_floor, report.header.schema_revision),
        (228, 65000)
    );
    let root = report.project_root.unwrap();
    assert_eq!(
        (root.flags, root.object_type, root.subtype),
        (0x80, 0xfe, 0xfd)
    );
    assert_eq!(root.descriptors[0].id, 255);
    assert_eq!(report.bytes_read, 68);
}

#[test]
fn follows_bounded_forwarding_records_and_retains_the_chain() {
    let mut bytes = fixture_bytes();
    put_i64(&mut bytes, 56, 256);
    put_forward(&mut bytes, 128, 160);
    put_forward(&mut bytes, 160, 192);
    put_object(&mut bytes, 192, &[(99, 7)]);
    let fixture = Fixture::new(&bytes);
    let report = inspect(&fixture.path).unwrap();
    let root = report.project_root.unwrap();
    assert_eq!(root.address, 192);
    assert_eq!(root.descriptors[0].id, 99);
    assert_eq!(
        root.forwarded_from,
        vec![
            ForwardingRecord {
                address: 128,
                flags: 1,
                replacement_address: 160
            },
            ForwardingRecord {
                address: 160,
                flags: 1,
                replacement_address: 192
            },
        ]
    );
    assert_eq!(report.objects_read, 4);
    assert_eq!(report.bytes_read, 48 + 7 + 9 + 9 + 7);
}

#[test]
fn zero_roots_are_reported_as_absent() {
    let mut bytes = fixture_bytes();
    put_i64(&mut bytes, 40, 0);
    put_i64(&mut bytes, 64, 0);
    let fixture = Fixture::new(&bytes);
    let report = inspect(&fixture.path).unwrap();
    assert!(report.metadata_root.is_none());
    assert!(report.project_root.is_none());
    assert_eq!((report.bytes_read, report.objects_read), (48, 0));
}

#[test]
fn rejects_truncated_magic_and_unsupported_outer_formats() {
    for length in [0, 15, 71] {
        assert_error(
            &fixture_bytes()[..length],
            io::ErrorKind::UnexpectedEof,
            "truncated",
        );
    }
    let mut bytes = fixture_bytes();
    bytes[0] = 0;
    assert_error(&bytes, io::ErrorKind::InvalidData, "magic");
    for version in [-1_i32, 0, 2] {
        let mut bytes = fixture_bytes();
        bytes[4..8].copy_from_slice(&version.to_be_bytes());
        assert_error(&bytes, io::ErrorKind::Unsupported, "version");
    }
    let mut bytes = fixture_bytes();
    bytes[8] ^= 1;
    assert_error(&bytes, io::ErrorKind::Unsupported, "compatibility");
    let mut bytes = fixture_bytes();
    put_i64(&mut bytes, 48, 1 << 30);
    assert_error(&bytes, io::ErrorKind::Unsupported, "Multi-file");
}

#[test]
fn rejects_invalid_cursors_and_root_addresses_including_preallocated_padding() {
    for cursor in [-1, 0, 71, 513, i64::MAX] {
        let mut bytes = fixture_bytes();
        put_i64(&mut bytes, 56, cursor);
        assert_error(&bytes, io::ErrorKind::InvalidData, "External");
    }
    for field in [40, 64] {
        for root in [-1, 1, 71, 191, 192, 256, i64::MAX] {
            let mut bytes = fixture_bytes();
            put_i64(&mut bytes, field, root);
            assert_error(&bytes, io::ErrorKind::InvalidData, "External");
        }
    }
}

#[test]
fn rejects_oversized_duplicate_overlapping_negative_and_out_of_bounds_descriptors() {
    for case in 0..8 {
        let mut bytes = fixture_bytes();
        match case {
            0 => bytes[131] = 255,
            1 => bytes[135] = bytes[132],
            2 => bytes[133..135].copy_from_slice(&(-1_i16).to_be_bytes()),
            3 => bytes[133..135].copy_from_slice(&12_i16.to_be_bytes()),
            4 => bytes[136..138].copy_from_slice(&13_i16.to_be_bytes()),
            5 => bytes[139..141].copy_from_slice(&20_i16.to_be_bytes()),
            6 => bytes[139..141].copy_from_slice(&64_i16.to_be_bytes()),
            7 => {
                put_i64(&mut bytes, 64, 184);
                bytes[184..188].copy_from_slice(&[0, 0, 7, 3]);
            }
            _ => unreachable!(),
        }
        assert_error(&bytes, io::ErrorKind::InvalidData, "External");
    }
}

#[test]
fn accepts_the_descriptor_budget_boundary_and_rejects_one_more() {
    let mut bytes = fixture_bytes();
    bytes.resize(1024, 0);
    put_i64(&mut bytes, 56, 1024);
    let fields = (0..MAX_DESCRIPTORS)
        .map(|id| (id as u8, (4 + MAX_DESCRIPTORS * 3 + id) as i16))
        .collect::<Vec<_>>();
    put_object(&mut bytes, 128, &fields);
    let fixture = Fixture::new(&bytes);
    let report = inspect(&fixture.path).unwrap();
    assert_eq!(
        report.project_root.unwrap().descriptors.len(),
        MAX_DESCRIPTORS
    );
    assert_eq!(report.bytes_read, 48 + 7 + 4 + (MAX_DESCRIPTORS as u64) * 3);
    bytes[131] = (MAX_DESCRIPTORS + 1) as u8;
    assert_error(&bytes, io::ErrorKind::InvalidData, "descriptor count");
}

#[test]
fn rejects_forwarding_cycles_nulls_and_invalid_replacements() {
    for replacement in [-1, 0, 64, 128, 192, 256, i64::MAX] {
        let mut bytes = fixture_bytes();
        put_forward(&mut bytes, 128, replacement);
        assert_error(&bytes, io::ErrorKind::InvalidData, "External");
    }
    let mut bytes = fixture_bytes();
    put_forward(&mut bytes, 128, 160);
    put_forward(&mut bytes, 160, 128);
    assert_error(&bytes, io::ErrorKind::InvalidData, "cycle");
}

#[test]
fn enforces_forward_depth_and_total_object_budgets() {
    for depth in [MAX_FORWARD_DEPTH, MAX_FORWARD_DEPTH + 1] {
        let mut bytes = fixture_bytes();
        put_i64(&mut bytes, 56, 512);
        for index in 0..depth {
            put_forward(&mut bytes, 128 + index * 16, (144 + index * 16) as i64);
        }
        put_object(&mut bytes, 128 + depth * 16, &[]);
        let fixture = Fixture::new(&bytes);
        let result = inspect(&fixture.path);
        if depth == MAX_FORWARD_DEPTH {
            assert_eq!(
                result.unwrap().project_root.unwrap().forwarded_from.len(),
                depth
            );
        } else {
            assert!(result.unwrap_err().to_string().contains("depth"));
        }
    }
    let mut bytes = fixture_bytes();
    bytes.resize(2048, 0);
    put_i64(&mut bytes, 56, 2048);
    put_i64(&mut bytes, 64, 1024);
    for root in [72, 1024] {
        for index in 0..MAX_FORWARD_DEPTH {
            put_forward(
                &mut bytes,
                root + index * 16,
                (root + (index + 1) * 16) as i64,
            );
        }
        put_object(&mut bytes, root + MAX_FORWARD_DEPTH * 16, &[]);
    }
    assert_error(&bytes, io::ErrorKind::InvalidData, "object budget");
}

#[test]
fn byte_ranges_use_checked_64_bit_arithmetic() {
    assert!(check_range(u64::MAX, 1, u64::MAX).is_err());
    assert!(check_range(72, u64::MAX, u64::MAX).is_err());
    assert!(check_range(1 << 32, 8, (1 << 32) + 8).is_ok());
    assert!(check_range(192, 1, 192).is_err());
    assert!(check_range(191, 1, 192).is_ok());
}

#[test]
fn enforces_read_budget_and_reports_real_truncation_errors() {
    let fixture = Fixture::new(&fixture_bytes());
    let mut reader = Reader::new(File::open(&fixture.path).unwrap(), 192);
    reader.bytes_read = MAX_READ_BYTES;
    assert!(
        reader
            .read_at(72, &mut [0])
            .unwrap_err()
            .to_string()
            .contains("byte budget")
    );
    reader.bytes_read = 0;
    File::options()
        .write(true)
        .open(&fixture.path)
        .unwrap()
        .set_len(72)
        .unwrap();
    assert_eq!(
        reader.compact_object(128).unwrap_err().kind(),
        io::ErrorKind::UnexpectedEof
    );
}

#[test]
fn rejects_non_regular_inputs() {
    let fixture = Fixture::new(&fixture_bytes());
    assert_eq!(
        inspect(fixture.path.parent().unwrap()).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
}

#[cfg(unix)]
#[test]
fn the_opened_source_cannot_block_on_a_replaced_special_file() {
    use std::os::fd::AsRawFd;

    let fixture = Fixture::new(&fixture_bytes());
    let source = Container::open(&fixture.path).unwrap();
    let flags = unsafe { libc::fcntl(source.reader.file.as_raw_fd(), libc::F_GETFL) };
    assert_ne!(flags, -1);
    assert_ne!(
        flags & libc::O_NONBLOCK,
        0,
        "the same fd must be opened nonblocking before checking its type; path metadata can race"
    );
}
