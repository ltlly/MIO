//! Lazy HTTP heads from explicitly mapped byte records, with version-specific entry points.
//!
//! The original record remains authoritative. No body decoding, service inference, URL
//! construction, protocol negotiation, or network operation is performed here.

use super::{
    ArrayKind, ArrayRecord, Container,
    catalog::{Failure, Observation},
};
use crate::http::{self, MAX_HEAD};
use mio_plugin_api::{Header, HttpVersion};

const READ_CHUNK: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Request,
    Response,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyRange {
    /// Relative to the original array payload, not the container file.
    pub offset: u64,
    /// All bytes after the delimiter; Content-Length, transfer framing and trailers are not decoded.
    pub length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartLine {
    Request {
        method: String,
        /// Exact request-line token, not a connection destination. Host does not establish TLS.
        request_target: Vec<u8>,
    },
    Response {
        status: u16,
        reason: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHead {
    pub start_line: StartLine,
    /// Only the request/status line's declaration, never an observed negotiated protocol.
    /// The original spelling (including HTTP/2 versus HTTP/2.0) remains in MessageHead::raw.
    pub declared_version: HttpVersion,
    /// Ordered, duplicate-preserving fields; values remain bytes. Raw whitespace is in `raw`.
    pub headers: Vec<Header>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadState {
    Empty,
    /// Head syntax only, not validation of the remaining HTTP message or its body framing.
    Parsed(ParsedHead),
    Invalid(&'static str),
    Unsupported(&'static str),
    LimitExceeded,
    ReadFailed(Failure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageHead {
    pub record: ArrayRecord,
    /// Original head including its delimiter, or the bounded prefix on incomplete/error cases.
    /// Never contains the read-ahead body bytes once a delimiter has been found.
    pub raw: Vec<u8>,
    /// Known only after finding a head delimiter, even if the head itself is invalid.
    pub body: Option<BodyRange>,
    pub state: HeadState,
}

/// Preserves absent/null/failed field observations and every selected record, including invalid
/// messages. Reads <=4 KiB per payload call and <=128 KiB plus four delimiter bytes in total.
/// A chunk may read ahead by <=4095 body bytes; those are not returned, decoded, or excluded
/// from the body range. Container's framing reads and source-change checks still apply.
pub fn read_head(
    source: &mut Container,
    record: &Observation<ArrayRecord>,
    kind: MessageKind,
) -> Observation<MessageHead> {
    match record {
        Observation::Missing => Observation::Missing,
        Observation::Null => Observation::Null,
        Observation::Unavailable { parent } => Observation::Unavailable {
            parent: parent.clone(),
        },
        Observation::Failed(error) => Observation::Failed(error.clone()),
        Observation::Value(record) => Observation::Value(read_record(source, record, kind, 228)),
    }
}

/// Only for the independent f4 current request mapped from a schema-229 root.f2.f1 tab.
#[cfg(test)]
pub(super) fn read_current_request_229(
    source: &mut Container,
    record: &ArrayRecord,
) -> MessageHead {
    read_record(source, record, MessageKind::Request, 229)
}

fn read_record(
    source: &mut Container,
    record: &ArrayRecord,
    kind: MessageKind,
    schema: u16,
) -> MessageHead {
    let mut result = MessageHead {
        record: record.clone(),
        raw: Vec::new(),
        body: None,
        state: HeadState::Empty,
    };
    if (
        source.header().schema_floor,
        source.header().schema_revision,
    ) != (schema, schema)
    {
        result.state = HeadState::Unsupported(if schema == 228 {
            "HTTP record mapping requires schema 228/228"
        } else {
            "Repeater record mapping requires schema 229/229"
        });
        return result;
    }
    if record.kind != ArrayKind::Bytes {
        result.state =
            HeadState::Unsupported("HTTP heads require an explicitly mapped byte record");
        return result;
    }
    if record.payload_bytes == 0 {
        if let Err(error) = source.read_payload(record, 0, &mut []) {
            result.state = HeadState::ReadFailed(error.into());
        }
        return result;
    }
    let limit = record.payload_bytes.min((MAX_HEAD + 4) as u64) as usize;
    let mut chunk = [0; READ_CHUNK];
    while result.raw.len() < limit {
        let start = result.raw.len();
        let count = READ_CHUNK.min(limit - start);
        if let Err(error) = source.read_payload(record, start as u64, &mut chunk[..count]) {
            result.state = HeadState::ReadFailed(error.into());
            return result;
        }
        result.raw.extend_from_slice(&chunk[..count]);
        let scan_start = start.saturating_sub(3);
        if let Some((end, delimiter)) = boundary(&result.raw[scan_start..]) {
            let end = scan_start + end;
            let body_offset = end + delimiter;
            result.raw.truncate(body_offset);
            result.body = Some(BodyRange {
                offset: body_offset as u64,
                length: record.payload_bytes - body_offset as u64,
            });
            result.state = if end > MAX_HEAD {
                HeadState::LimitExceeded
            } else {
                parse(&result.raw, end, kind)
            };
            return result;
        }
    }
    result.state = if record.payload_bytes > MAX_HEAD as u64 {
        HeadState::LimitExceeded
    } else {
        HeadState::Invalid("HTTP head has no terminating empty line")
    };
    result
}

fn boundary(raw: &[u8]) -> Option<(usize, usize)> {
    raw.windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .map(|end| (end, 4))
        .into_iter()
        .chain(
            raw.windows(2)
                .position(|bytes| bytes == b"\n\n")
                .map(|end| (end, 2)),
        )
        .min_by_key(|(end, _)| *end)
}

fn parse(raw: &[u8], end: usize, kind: MessageKind) -> HeadState {
    // Reject controls before the shared parser trims optional whitespace from field values.
    if raw[..end]
        .iter()
        .enumerate()
        .any(|(index, byte)| match *byte {
            b'\r' => raw.get(index + 1) != Some(&b'\n'),
            b'\n' | b'\t' => false,
            value => value < 32 || value == 127,
        })
    {
        return HeadState::Invalid("HTTP head contains an invalid control byte");
    }
    let Ok((first, headers, _)) = http::head(raw) else {
        return HeadState::Invalid("HTTP header fields could not be parsed");
    };
    let (start_line, declared_version) = match start_line(&first, kind) {
        Ok(value) => value,
        Err(state) => return state,
    };
    HeadState::Parsed(ParsedHead {
        start_line,
        declared_version,
        headers,
    })
}

fn start_line(raw: &[u8], kind: MessageKind) -> Result<(StartLine, HttpVersion), HeadState> {
    let invalid = HeadState::Invalid("Invalid HTTP request/status line");
    match kind {
        MessageKind::Request => {
            let mut parts = raw.split(|byte| *byte == b' ');
            let method = parts.next().ok_or(invalid.clone())?;
            let target = parts.next().ok_or(invalid.clone())?;
            let version = parts.next().ok_or(invalid.clone())?;
            if parts.next().is_some()
                || !http::token(method)
                || target.is_empty()
                || target.iter().any(|byte| *byte <= 32 || *byte == 127)
            {
                return Err(invalid);
            }
            Ok((
                StartLine::Request {
                    method: String::from_utf8(method.to_vec()).expect("validated ASCII token"),
                    request_target: target.to_vec(),
                },
                declared_version(version)?,
            ))
        }
        MessageKind::Response => {
            let mut parts = raw.splitn(3, |byte| *byte == b' ');
            let version = parts.next().ok_or(invalid.clone())?;
            let status = parts.next().ok_or(invalid.clone())?;
            if status.len() != 3 || !status.iter().all(u8::is_ascii_digit) {
                return Err(invalid);
            }
            let status = u16::from(status[0] - b'0') * 100
                + u16::from(status[1] - b'0') * 10
                + u16::from(status[2] - b'0');
            if !(100..=599).contains(&status) {
                return Err(invalid);
            }
            Ok((
                StartLine::Response {
                    status,
                    reason: parts.next().unwrap_or_default().to_vec(),
                },
                declared_version(version)?,
            ))
        }
    }
}

fn declared_version(raw: &[u8]) -> Result<HttpVersion, HeadState> {
    match raw {
        b"HTTP/1.0" => Ok(HttpVersion::Http10),
        b"HTTP/1.1" => Ok(HttpVersion::Http11),
        b"HTTP/2" | b"HTTP/2.0" => Ok(HttpVersion::Http2),
        _ => {
            if let Some(suffix) = raw.strip_prefix(b"HTTP/") {
                let parts: Vec<_> = suffix.split(|byte| *byte == b'.').collect();
                if parts.len() <= 2
                    && parts
                        .iter()
                        .all(|part| !part.is_empty() && part.iter().all(u8::is_ascii_digit))
                {
                    return Err(HeadState::Unsupported("Unrecognized declared HTTP version"));
                }
            }
            Err(HeadState::Invalid("Malformed declared HTTP version"))
        }
    }
}

#[cfg(test)]
#[path = "http/tests.rs"]
mod tests;
