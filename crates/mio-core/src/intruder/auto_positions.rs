use std::ops::Range;

use super::{MAX_POSITIONS, Position};

mod headers_mime;
mod multipart;
mod request;
mod structured;

use headers_mime::{
    cookies, ends_ascii_case, headers, headers_checked, line, mime, pairs, unique_header,
    utf8_charset,
};
use multipart::multipart;
pub(crate) use multipart::{
    intercept_body_is_structured, intercept_multipart_parameters, parse_intercept_content_type,
};
pub use request::automatic_positions;
pub(crate) use request::has_http_parameters;
use request::{add, trimmed};
use structured::{json, json_checked, xml, xml_checked};

// MIO parsing budgets, not External defaults. An exceeded budget never yields a partial result.
pub(crate) const MAX_AUTO_BYTES: usize = 4 * 1024 * 1024;
const MAX_DEPTH: usize = 64;
const MAX_NODES: usize = 8192;

#[derive(Debug)]
pub(crate) struct AutomaticParameterLimit(pub(crate) &'static str);

impl std::fmt::Display for AutomaticParameterLimit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for AutomaticParameterLimit {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutomaticPlacement {
    Replace,
    Append,
}

struct HeaderRange<'a> {
    name: &'a [u8],
    value: Range<usize>,
}

struct Mime<'a> {
    essence: &'a [u8],
    parameters: Vec<(&'a [u8], Vec<u8>)>,
}

pub(crate) struct InterceptContentType<'a> {
    pub(crate) essence: &'a [u8],
    boundary: Option<Vec<u8>>,
}

#[cfg(test)]
mod tests;
