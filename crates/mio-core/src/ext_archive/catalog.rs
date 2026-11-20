//! Bounded, read-only metadata for schema-228 collections and narrow schema-229 paths.
//!
//! The six collection paths and numeric tags were checked against a local 228/228 source.
//! Schema 229/229 also covers the four root.f1 chunked directories; only f0/f1 have
//! an observed HTTP-history association. The f2/f3 business meaning is unverified.
//! This catalog does not read item records, identify HTTP/WS ownership, or import a project.

mod inspect;
#[cfg(test)]
mod tests;

pub use inspect::inspect;
#[cfg(test)]
pub(crate) use inspect::inspect_with_unpack;

use super::{
    ChunkedList, Container, DirectList, FieldDescriptor, MAX_DESCRIPTORS, MAX_FORWARD_DEPTH,
};
use std::io;

pub const MAX_BYTE_BUDGET: u64 = 256 * 1024;
const HEADER_READ_BYTES: u64 = 48;
const SAMPLE_ITEMS: usize = 8;
const COMPACT_READ_BOUND: u64 = (MAX_FORWARD_DEPTH * 9 + 4 + MAX_DESCRIPTORS * 3) as u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub kind: io::ErrorKind,
    pub message: String,
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Self {
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation<T> {
    Missing,
    Null,
    Value(T),
    /// The parent could not be interpreted; this does not claim the child is absent.
    Unavailable {
        parent: String,
    },
    Failed(Failure),
}

impl<T> Observation<T> {
    fn and_then<U>(self, apply: impl FnOnce(T) -> Observation<U>) -> Observation<U> {
        match self {
            Self::Missing => Observation::Missing,
            Self::Null => Observation::Null,
            Self::Value(value) => apply(value),
            Self::Unavailable { parent } => Observation::Unavailable { parent },
            Self::Failed(error) => Observation::Failed(error),
        }
    }

    fn map<U>(self, apply: impl FnOnce(T) -> U) -> Observation<U> {
        self.and_then(|value| Observation::Value(apply(value)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectSummary {
    pub object_type: u8,
    pub subtype: u8,
    pub fields: Vec<FieldDescriptor>,
    pub forwarding_hops: usize,
    /// Only the expected numeric type/subtype at this particular path has matched.
    pub layout_supported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectEntry {
    pub path: String,
    pub state: Observation<ObjectSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Null,
    Present,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layout {
    /// f0 is the logical length; f1 points to the backing References array.
    Direct { capacity: u64 },
    /// f0 is length, f1 width, f2 a direct chunk list, and f3 the leading offset.
    Chunked {
        chunk_width: u64,
        leading_offset: u64,
        active_chunks: u64,
        chunk_capacity: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectionSource {
    Direct(DirectList),
    Chunked(ChunkedList),
}

impl CollectionSource {
    pub fn logical_len(&self) -> u64 {
        match self {
            Self::Direct(list) => list.logical_len,
            Self::Chunked(list) => list.logical_len,
        }
    }

    /// Page the same immutable source, retaining nulls, duplicates and original slot order.
    /// Each call revalidates metadata and returns <=256 logical slots, never reading item records.
    /// An invalid reference fails the page; callers can inspect individual slots to retain gaps.
    pub fn page(
        &self,
        source: &mut Container,
        start: u64,
        limit: usize,
    ) -> io::Result<Vec<Option<u64>>> {
        match self {
            Self::Direct(list) => source.direct_page(list, start, limit),
            Self::Chunked(list) => source.chunked_page(list, start, limit),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionSummary {
    pub logical_len: u64,
    pub layout: Layout,
    /// Original metadata, also retained when sampling fails; each later page validates it.
    pub source: CollectionSource,
    /// At most eight slots starting at logical index zero; this sample omits addresses.
    pub sample: Observation<Vec<Slot>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionEntry {
    /// Original field path, not a guessed business category.
    pub path: String,
    pub state: Observation<CollectionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    pub schema_floor: u16,
    pub schema_revision: u16,
    pub byte_budget: u64,
    /// Includes the header, metadata, reference slots, and failed read attempts.
    pub bytes_read: u64,
    pub objects: Vec<ObjectEntry>,
    pub collections: Vec<CollectionEntry>,
    /// Schema-229 root.f2.f0; raw signed observation before source-slot range validation.
    pub repeater_active_tab: Observation<i32>,
}

impl Catalog {
    /// A successful catalog is still not evidence of full project compatibility.
    pub fn has_errors(&self) -> bool {
        matches!(self.repeater_active_tab, Observation::Failed(_))
            || self.objects.iter().any(|entry| match &entry.state {
                Observation::Failed(_) => true,
                Observation::Value(value) => !value.layout_supported,
                _ => false,
            })
            || self.collections.iter().any(|entry| match &entry.state {
                Observation::Failed(_) => true,
                Observation::Value(value) => matches!(value.sample, Observation::Failed(_)),
                _ => false,
            })
    }
}
