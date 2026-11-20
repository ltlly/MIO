//! Explicit schema-228 mappings and separate schema-229 Repeater entries.
//!
//! Layout descriptors cover schema-226/228 byte-record offsets for HTTP and Repeater entries.
//! Schema-228 nonempty byte-record positives: HTTP f15/f18, Repeater tab f4 and pair f2/f3.
//! HTTP f16/f17/f19/f20 were null in 64 sampled collection positions; their nonempty
//! storage kinds and producer meanings are not established for schema 228.
//! Prefix classifications do not establish full HTTP, target, version, or project compatibility.

#[cfg(test)]
mod decoder;
#[cfg(test)]
mod http_annotations;
#[cfg(test)]
mod repeater_groups;
#[cfg(test)]
mod schema229;
#[cfg(test)]
#[path = "messages/tests.rs"]
mod tests;

#[cfg(test)]
use decoder::decoder_229;
#[cfg(test)]
pub(super) use http_annotations::HttpAnnotations229;
#[cfg(test)]
pub(super) use repeater_groups::repeater_group_229;
#[cfg(test)]
pub(super) use schema229::{proxy_http_229, repeater_current_229};

#[cfg(test)]
use super::MAX_PAGE_ITEMS;
use super::{ArrayRecord, CompactObject, DirectList, catalog::Observation};
#[cfg(test)]
use super::{ArrayKind, Container, MAX_DESCRIPTORS, MAX_FORWARD_DEPTH, catalog::Failure};
#[cfg(test)]
use std::io;
#[cfg(test)]
use tokio_util::sync::CancellationToken;

pub const MAX_BYTE_BUDGET: u64 = 256 * 1024;
pub(crate) const MAX_REPEATER_TITLE_UNITS: usize = 4096;
#[cfg(test)]
const REPEATER_TITLE_CHUNK_UNITS: usize = 32;
#[cfg(test)]
const MAX_REPEATER_TITLE_BYTES: usize = 4096;
#[cfg(test)]
const COMPACT_READ_BOUND: u64 = (MAX_FORWARD_DEPTH * 9 + 4 + MAX_DESCRIPTORS * 3) as u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field<T> {
    pub id: u8,
    pub state: Observation<T>,
}

/// Original descriptors survive unsupported tags and malformed mapped fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded<T> {
    pub object: CompactObject,
    pub value: Observation<T>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpHistoryItem {
    /// f15 is the observed request byte-record candidate.
    pub request: Field<ArrayRecord>,
    /// f16/f17: keep non-null references without assuming their storage kind or producer role.
    pub unverified_request_references: [Field<u64>; 2],
    /// f18 is the observed response candidate. Null is not an empty response record.
    pub response: Field<ArrayRecord>,
    /// f19/f20: their non-null storage kinds have not been observed in schema 228.
    pub unverified_response_references: [Field<u64>; 2],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepeaterTab {
    /// f4 is independent of the history; a missing/null draft is never replaced by a pair.
    pub current_request: Field<ArrayRecord>,
    pub history: Field<Decoded<DirectList>>,
    /// Public-226 group/UUID candidates f32/f33; no identity/string semantics are decoded.
    pub identity_candidates: [Field<u64>; 2],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepeaterPair {
    pub request: Field<ArrayRecord>,
    pub response: Field<ArrayRecord>,
    /// f0/f1 candidate service/message objects: preserve their tags/descriptors, not guessed
    /// host, port, TLS, or HTTP-version values. Their field payloads are not read here.
    pub metadata_candidates: [Field<CompactObject>; 2],
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepeaterCurrent229 {
    pub current_request: Field<ArrayRecord>,
    pub title: Field<Decoded<String>>,
    pub history: Field<Decoded<DirectList>>,
    pub selected_history: Field<i32>,
    pub service: Field<Decoded<RepeaterService229>>,
    pub group: Field<CompactObject>,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepeaterService229 {
    pub host: Field<Decoded<String>>,
    pub port: Field<i32>,
    pub tls: Field<bool>,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProxyHttp229 {
    /// f11 matches the displayed Time column in Unix milliseconds; the event stage is unverified.
    pub time: Field<i64>,
    pub request: Field<ArrayRecord>,
    pub edited_request: Field<ArrayRecord>,
    pub response: Field<ArrayRecord>,
    pub edited_response: Field<ArrayRecord>,
    pub parsed_request: Field<Decoded<ProxyRequest229>>,
    pub unverified_variants: [Field<u64>; 2],
    pub highlight: Field<u8>,
    pub annotations: Field<Decoded<HttpAnnotations229>>,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProxyRequest229 {
    pub service: Field<Decoded<RepeaterService229>>,
}

/// A cumulative metadata budget, additional to reads already made by the supplied Container.
/// Byte records contain only framing metadata; use Container::read_payload or records::read_fragment
/// explicitly for later <=64 KiB byte windows after releasing this borrow.
#[cfg(test)]
pub(super) struct Decoder<'a> {
    source: &'a mut Container,
    started_at: u64,
    byte_budget: u64,
    cancel: Option<&'a CancellationToken>,
}

#[cfg(test)]
fn observed<T>(result: io::Result<T>) -> Observation<T> {
    match result {
        Ok(value) => Observation::Value(value),
        Err(error) => Observation::Failed(error.into()),
    }
}
