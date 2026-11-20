//! Read-only inspection of the external project archive container and compact-object descriptors.
//!
//! Reads the 72-byte binary header and object descriptors. `inspect` reads
//! no field values. Typed reads require the caller to establish the field/record kind;
//! no traffic, settings, or plugin semantics are inferred. These primitives are not an import.

pub mod catalog;
pub(crate) mod direct_import;
mod container;
pub mod http;
pub(crate) mod import;
pub mod messages;
mod reader;
pub mod records;
#[cfg(test)]
mod tests;

pub use container::inspect;

use std::{
    fs::{File, Metadata},
    io,
};

const HEADER_SIZE: u64 = 72;
const MAGIC: [u8; 4] = [0x66, 0x85, 0x82, 0x80];

/// Returns `true` when `path` is a regular file beginning with the 4-byte external archive magic.
pub fn is_external_archive_file(path: &std::path::Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.file_type().is_file() {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut header = [0u8; 4];
    std::io::Read::read_exact(&mut file, &mut header).is_ok() && header == MAGIC
}
const COMPATIBILITY: u32 = 0x8052_7974;
const MAX_DESCRIPTORS: usize = 128;
const MAX_FORWARD_DEPTH: usize = 16;
const MAX_OBJECTS: usize = 32;
const MAX_READ_BYTES: u64 = 4096;
const MAX_PAYLOAD_BYTES: usize = 64 * 1024;
const MAX_PAGE_ITEMS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub outer_version: i32,
    pub compatibility: u32,
    pub schema_floor: u16,
    pub schema_revision: u16,
    pub metadata_root: u64,
    pub physical_address_span: u64,
    pub allocation_cursor: u64,
    pub project_root: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDescriptor {
    pub id: u8,
    pub relative_offset: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardingRecord {
    pub address: u64,
    pub flags: u8,
    pub replacement_address: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactObject {
    pub address: u64,
    pub flags: u8,
    pub object_type: u8,
    pub subtype: u8,
    /// Raw IDs and starting offsets only; field widths and meanings are not established.
    pub descriptors: Vec<FieldDescriptor>,
    pub forwarded_from: Vec<ForwardingRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Structure {
    pub file_size: u64,
    pub header: Header,
    /// A zero root address is reported as absent, not as a complete empty project.
    pub metadata_root: Option<CompactObject>,
    pub project_root: Option<CompactObject>,
    pub bytes_read: u64,
    pub objects_read: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrayKind {
    Bytes,
    Utf16,
    References,
}

impl ArrayKind {
    fn width(self) -> u64 {
        match self {
            Self::Bytes => 1,
            Self::Utf16 => 2,
            Self::References => 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrayRecord {
    pub address: u64,
    pub kind: ArrayKind,
    pub total_size: u64,
    /// Allocated array slots, not a collection's logical item count.
    pub element_count: u64,
    pub payload_bytes: u64,
}

/// Explicit metadata supplied by a verified mapping, not inferred from object tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectList {
    pub logical_len: u64,
    pub addresses: ArrayRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkedList {
    pub logical_len: u64,
    pub chunk_width: u64,
    pub leading_offset: u64,
    pub chunks: DirectList,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn check_range(offset: u64, length: u64, boundary: u64) -> io::Result<()> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| invalid("External byte range overflows u64"))?;
    if end > boundary {
        return Err(invalid(
            "External byte range exceeds the file or allocation boundary",
        ));
    }
    Ok(())
}

fn address(bytes: &[u8]) -> io::Result<u64> {
    let bytes = bytes
        .try_into()
        .map_err(|_| invalid("External address is not eight bytes"))?;
    u64::try_from(i64::from_be_bytes(bytes)).map_err(|_| invalid("External address is negative"))
}

struct Reader {
    file: File,
    boundary: u64,
    bytes_read: u64,
    objects_read: usize,
    byte_budget: u64,
    object_budget: usize,
}

fn page_count(length: u64, start: u64, limit: usize, max: usize) -> io::Result<usize> {
    if limit > max {
        return Err(invalid("External page limit exceeds the bounded page size"));
    }
    let remaining = length
        .checked_sub(start)
        .ok_or_else(|| invalid("External page starts after the logical end"))?;
    Ok(remaining.min(limit as u64) as usize)
}

/// An inactive read-only source. Every operation is bounded and checks for source changes.
/// Metadata checks detect changes but are not an atomic filesystem snapshot.
pub struct Container {
    reader: Reader,
    header: Header,
    original: Metadata,
    bytes_read: u64,
    objects_read: usize,
}
