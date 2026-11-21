use std::borrow::Cow;
use std::collections::HashMap;

use super::*;

mod markup;
use markup::{
    format_pretty_html_with_lines, format_pretty_xml_with_lines, looks_like_html_document,
    looks_like_xml_document,
};

const PRETTY_NON_UTF8_ERROR: &str =
    "消息含非 UTF-8 字节，无法切换到 Pretty 视图；请使用 Hex 视图查看原始字节。";
pub(super) const MAX_PRETTY_BYTES: usize = 2 * 1024 * 1024;
const MAX_JSON_DEPTH: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrettyMessage {
    pub text: String,
    /// Optional 1-based original line number per output line (`0` for synthetic
    /// continuation lines). `None` when output lines map 1:1 to `1..=N`.
    pub line_numbers: Option<Vec<usize>>,
}

pub(super) enum SourceLineMap {
    Contiguous {
        base_line: usize,
        line_starts: Vec<usize>,
    },
    Chunked {
        spans: Vec<(usize, usize)>,
    },
}

impl SourceLineMap {
    pub(super) fn for_contiguous(slice: &str, base_line: usize) -> Self {
        let mut line_starts = vec![0usize];
        for (idx, b) in slice.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(idx + 1);
            }
        }
        Self::Contiguous {
            base_line,
            line_starts,
        }
    }

    pub(super) fn line_at(&self, offset: usize) -> usize {
        match self {
            Self::Contiguous {
                base_line,
                line_starts,
            } => {
                let idx = line_starts.partition_point(|&start| start <= offset);
                base_line + idx.saturating_sub(1)
            }
            Self::Chunked { spans } => {
                let idx = spans.partition_point(|&(off, _)| off <= offset);
                if idx == 0 {
                    spans.first().map_or(1, |&(_, line)| line)
                } else {
                    spans[idx - 1].1
                }
            }
        }
    }
}

pub(super) struct PrettyLineBuilder {
    text: String,
    mapping: Vec<usize>,
    last_assigned: usize,
    has_lines: bool,
}

impl PrettyLineBuilder {
    fn new(capacity: usize) -> Self {
        Self {
            text: String::with_capacity(capacity),
            mapping: Vec::new(),
            last_assigned: 0,
            has_lines: false,
        }
    }

    pub(super) fn push_line(&mut self, line: &str, orig_line: Option<usize>) {
        if self.has_lines {
            self.text.push('\n');
        }
        self.text.push_str(line);
        self.has_lines = true;
        match orig_line {
            Some(n) if n > self.last_assigned => {
                self.mapping.push(n);
                self.last_assigned = n;
            }
            _ => self.mapping.push(0),
        }
    }

    pub(super) fn push_multiline_from_base(&mut self, block: &str, first_orig_line: usize) {
        for (idx, part) in block.split('\n').enumerate() {
            self.push_line(part, Some(first_orig_line + idx));
        }
    }

    fn append_to_last_line(&mut self, suffix: &str) {
        self.text.push_str(suffix);
    }

    fn push_normalized_lines(&mut self, slice: &str, base_line: usize) {
        let normalized = slice.replace("\r\n", "\n");
        self.push_multiline_from_base(&normalized, base_line);
    }

    fn finalize(self) -> PrettyMessage {
        if self.text.is_empty() {
            return PrettyMessage {
                text: String::new(),
                line_numbers: None,
            };
        }
        debug_assert_eq!(self.mapping.len(), self.text.split('\n').count());
        let is_identity = self
            .mapping
            .iter()
            .enumerate()
            .all(|(idx, &line_no)| line_no == idx + 1);
        PrettyMessage {
            text: self.text,
            line_numbers: (!is_identity).then_some(self.mapping),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BodyKind {
    Json,
    Xml,
    Html,
    OtherSpecified,
    Unspecified,
}

fn is_http_start_line(first_line: &str) -> bool {
    if first_line.starts_with("HTTP/") {
        return true;
    }
    let mut parts = first_line.split_ascii_whitespace();
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(method), Some(target), Some(version), None)
            if token(method.as_bytes()) && !target.is_empty() && version.starts_with("HTTP/")
    )
}

fn header_body_kind(header_lines: &[&str]) -> BodyKind {
    for line in header_lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("content-type") {
            continue;
        }
        let essence = value
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if essence.is_empty() {
            continue;
        }
        return if essence == "application/json" || essence.ends_with("+json") {
            BodyKind::Json
        } else if essence == "application/xml" || essence.ends_with("+xml") || essence == "text/xml"
        {
            BodyKind::Xml
        } else if essence == "text/html" {
            BodyKind::Html
        } else {
            BodyKind::OtherSpecified
        };
    }
    BodyKind::Unspecified
}

enum RawJsonLines {
    Scalar(usize),
    Array {
        open_line: usize,
        items: Vec<RawJsonLines>,
        close_line: usize,
    },
    Object {
        open_line: usize,
        entries: Vec<(usize, RawJsonLines)>,
        close_line: usize,
    },
}

struct JsonOffsetScanner<'a> {
    bytes: &'a [u8],
    src: &'a str,
    pos: usize,
    base_offset: usize,
    line_map: &'a SourceLineMap,
}

impl<'a> JsonOffsetScanner<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len()
            && matches!(self.bytes[self.pos], b' ' | b'\t' | b'\r' | b'\n')
        {
            self.pos += 1;
        }
    }

    fn scan_string_bounds(&mut self) -> Option<(usize, usize)> {
        if self.bytes.get(self.pos) != Some(&b'"') {
            return None;
        }
        let start = self.pos;
        self.pos += 1;
        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b'\\' => {
                    self.pos += 2;
                }
                b'"' => {
                    self.pos += 1;
                    return Some((start, self.pos));
                }
                _ => {
                    self.pos += 1;
                }
            }
        }
        None
    }

    fn parse_value(&mut self, depth: usize) -> Option<RawJsonLines> {
        if depth >= MAX_JSON_DEPTH {
            return None;
        }
        self.skip_ws();
        let start = self.pos;
        match self.bytes.get(self.pos)? {
            b'{' => self.parse_object(depth + 1),
            b'[' => self.parse_array(depth + 1),
            b'"' => {
                self.scan_string_bounds()?;
                Some(RawJsonLines::Scalar(
                    self.line_map.line_at(self.base_offset + start),
                ))
            }
            _ => {
                while self.pos < self.bytes.len()
                    && !matches!(
                        self.bytes[self.pos],
                        b' ' | b'\t' | b'\r' | b'\n' | b',' | b']' | b'}'
                    )
                {
                    self.pos += 1;
                }
                if self.pos == start {
                    return None;
                }
                Some(RawJsonLines::Scalar(
                    self.line_map.line_at(self.base_offset + start),
                ))
            }
        }
    }

    fn parse_array(&mut self, depth: usize) -> Option<RawJsonLines> {
        let open_line = self.line_map.line_at(self.base_offset + self.pos);
        self.pos += 1;
        self.skip_ws();
        if self.bytes.get(self.pos) == Some(&b']') {
            let close_line = self.line_map.line_at(self.base_offset + self.pos);
            self.pos += 1;
            return Some(RawJsonLines::Array {
                open_line,
                items: Vec::new(),
                close_line,
            });
        }
        let mut items = Vec::new();
        loop {
            items.push(self.parse_value(depth)?);
            self.skip_ws();
            match self.bytes.get(self.pos)? {
                b',' => {
                    self.pos += 1;
                }
                b']' => {
                    let close_line = self.line_map.line_at(self.base_offset + self.pos);
                    self.pos += 1;
                    return Some(RawJsonLines::Array {
                        open_line,
                        items,
                        close_line,
                    });
                }
                _ => return None,
            }
        }
    }

    fn parse_object(&mut self, depth: usize) -> Option<RawJsonLines> {
        let open_line = self.line_map.line_at(self.base_offset + self.pos);
        self.pos += 1;
        self.skip_ws();
        if self.bytes.get(self.pos) == Some(&b'}') {
            let close_line = self.line_map.line_at(self.base_offset + self.pos);
            self.pos += 1;
            return Some(RawJsonLines::Object {
                open_line,
                entries: Vec::new(),
                close_line,
            });
        }
        let mut entries: Vec<(usize, RawJsonLines)> = Vec::new();
        let mut small_keys: Vec<Cow<'a, str>> = Vec::new();
        let mut index_by_key: Option<HashMap<Cow<'a, str>, usize>> = None;
        loop {
            self.skip_ws();
            let (k_start, k_end) = self.scan_string_bounds()?;
            let key_line = self.line_map.line_at(self.base_offset + k_start);
            let inner = &self.src[k_start + 1..k_end - 1];
            let key_name: Cow<'a, str> = if !inner.contains('\\') {
                Cow::Borrowed(inner)
            } else {
                Cow::Owned(serde_json::from_str::<String>(&self.src[k_start..k_end]).ok()?)
            };
            self.skip_ws();
            if self.bytes.get(self.pos) != Some(&b':') {
                return None;
            }
            self.pos += 1;
            let val_node = self.parse_value(depth)?;

            if let Some(map) = index_by_key.as_mut() {
                if let Some(&idx) = map.get(&key_name) {
                    entries[idx] = (key_line, val_node);
                } else {
                    let idx = entries.len();
                    entries.push((key_line, val_node));
                    map.insert(key_name, idx);
                }
            } else if let Some(idx) = small_keys.iter().position(|existing| *existing == key_name) {
                entries[idx] = (key_line, val_node);
            } else {
                let idx = entries.len();
                entries.push((key_line, val_node));
                if idx >= 8 {
                    let mut map = HashMap::with_capacity(16);
                    for (existing_idx, existing_key) in small_keys.drain(..).enumerate() {
                        map.insert(existing_key, existing_idx);
                    }
                    map.insert(key_name, idx);
                    index_by_key = Some(map);
                } else {
                    small_keys.push(key_name);
                }
            }

            self.skip_ws();
            match self.bytes.get(self.pos)? {
                b',' => {
                    self.pos += 1;
                }
                b'}' => {
                    let close_line = self.line_map.line_at(self.base_offset + self.pos);
                    self.pos += 1;
                    return Some(RawJsonLines::Object {
                        open_line,
                        entries,
                        close_line,
                    });
                }
                _ => return None,
            }
        }
    }
}

fn push_indent(buf: &mut String, depth: usize) {
    for _ in 0..depth {
        buf.push_str("  ");
    }
}

fn json_structure_matches(val: &serde_json::Value, raw: &RawJsonLines) -> bool {
    match (val, raw) {
        (
            serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_),
            RawJsonLines::Scalar(_),
        ) => true,
        (serde_json::Value::Array(arr), RawJsonLines::Array { items, .. }) => {
            arr.len() == items.len()
                && arr
                    .iter()
                    .zip(items)
                    .all(|(v, r)| json_structure_matches(v, r))
        }
        (serde_json::Value::Object(map), RawJsonLines::Object { entries, .. }) => {
            map.len() == entries.len()
                && map
                    .values()
                    .zip(entries)
                    .all(|(v, (_, r))| json_structure_matches(v, r))
        }
        _ => false,
    }
}

fn format_scalar_or_empty(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::Null => Some("null".to_owned()),
        serde_json::Value::Bool(b) => Some(if *b { "true" } else { "false" }.to_owned()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(_) => serde_json::to_string(val).ok(),
        serde_json::Value::Array(arr) if arr.is_empty() => Some("[]".to_owned()),
        serde_json::Value::Object(map) if map.is_empty() => Some("{}".to_owned()),
        _ => None,
    }
}

fn emit_pretty_json_value(
    val: &serde_json::Value,
    raw: &RawJsonLines,
    depth: usize,
    prefix: Option<(&str, usize)>,
    out: &mut PrettyLineBuilder,
) {
    if let Some(inline) = format_scalar_or_empty(val) {
        let mut line = String::new();
        let line_no = if let Some((pfx, key_line)) = prefix {
            line.push_str(pfx);
            key_line
        } else {
            push_indent(&mut line, depth);
            match raw {
                RawJsonLines::Scalar(l)
                | RawJsonLines::Array { open_line: l, .. }
                | RawJsonLines::Object { open_line: l, .. } => *l,
            }
        };
        line.push_str(&inline);
        out.push_line(&line, Some(line_no));
        return;
    }

    match (val, raw) {
        (
            serde_json::Value::Array(arr),
            RawJsonLines::Array {
                open_line,
                items,
                close_line,
            },
        ) => {
            let mut open = String::new();
            let first_line = if let Some((pfx, key_line)) = prefix {
                open.push_str(pfx);
                key_line
            } else {
                push_indent(&mut open, depth);
                *open_line
            };
            open.push('[');
            out.push_line(&open, Some(first_line));
            for (idx, (item_val, item_raw)) in arr.iter().zip(items).enumerate() {
                emit_pretty_json_value(item_val, item_raw, depth + 1, None, out);
                if idx + 1 < arr.len() {
                    out.append_to_last_line(",");
                }
            }
            let mut close = String::new();
            push_indent(&mut close, depth);
            close.push(']');
            out.push_line(&close, Some(*close_line));
        }
        (
            serde_json::Value::Object(map),
            RawJsonLines::Object {
                open_line,
                entries,
                close_line,
            },
        ) => {
            let mut open = String::new();
            let first_line = if let Some((pfx, key_line)) = prefix {
                open.push_str(pfx);
                key_line
            } else {
                push_indent(&mut open, depth);
                *open_line
            };
            open.push('{');
            out.push_line(&open, Some(first_line));
            for (idx, ((key, val_item), (key_line, val_raw))) in
                map.iter().zip(entries).enumerate()
            {
                let mut key_prefix = String::new();
                push_indent(&mut key_prefix, depth + 1);
                key_prefix.push_str(&serde_json::to_string(key).unwrap_or_default());
                key_prefix.push_str(": ");
                emit_pretty_json_value(
                    val_item,
                    val_raw,
                    depth + 1,
                    Some((&key_prefix, *key_line)),
                    out,
                );
                if idx + 1 < map.len() {
                    out.append_to_last_line(",");
                }
            }
            let mut close = String::new();
            push_indent(&mut close, depth);
            close.push('}');
            out.push_line(&close, Some(*close_line));
        }
        _ => {}
    }
}

fn format_json_if_valid_with_lines(
    body_str: &str,
    trimmed: &str,
    line_map: &SourceLineMap,
    out: &mut PrettyLineBuilder,
) -> bool {
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) || body_str.len() > MAX_PRETTY_BYTES {
        return false;
    }
    let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return false;
    };
    let trimmed_offset = body_str.len() - body_str.trim_start().len();
    let mut scanner = JsonOffsetScanner {
        bytes: trimmed.as_bytes(),
        src: trimmed,
        pos: 0,
        base_offset: trimmed_offset,
        line_map,
    };
    if let Some(raw_lines) = scanner.parse_value(0)
        && json_structure_matches(&val, &raw_lines)
    {
        emit_pretty_json_value(&val, &raw_lines, 0, None, out);
        return true;
    }
    let Ok(pretty) = serde_json::to_string_pretty(&val) else {
        return false;
    };
    let start_line = line_map.line_at(trimmed_offset);
    for (idx, line) in pretty.split('\n').enumerate() {
        out.push_line(line, (idx == 0).then_some(start_line));
    }
    true
}

fn format_body_by_kind_with_lines(
    kind: BodyKind,
    body_str: &str,
    line_map: &SourceLineMap,
    out: &mut PrettyLineBuilder,
) -> std::result::Result<(), String> {
    let trimmed = body_str.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    match kind {
        BodyKind::Json => {
            if !format_json_if_valid_with_lines(body_str, trimmed, line_map, out) {
                out.push_normalized_lines(body_str, line_map.line_at(0));
            }
            Ok(())
        }
        BodyKind::Xml => format_pretty_xml_with_lines(body_str, line_map, out),
        BodyKind::Html => format_pretty_html_with_lines(body_str, line_map, out),
        BodyKind::OtherSpecified => {
            out.push_normalized_lines(body_str, line_map.line_at(0));
            Ok(())
        }
        BodyKind::Unspecified => {
            if format_json_if_valid_with_lines(body_str, trimmed, line_map, out) {
                Ok(())
            } else if looks_like_html_document(trimmed) {
                format_pretty_html_with_lines(body_str, line_map, out)
            } else if looks_like_xml_document(trimmed) {
                format_pretty_xml_with_lines(body_str, line_map, out)
            } else {
                out.push_normalized_lines(body_str, line_map.line_at(0));
                Ok(())
            }
        }
    }
}

fn is_chunked_header_lines(header_lines: &[&str]) -> bool {
    header_lines.iter().any(|line| {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        name.trim().eq_ignore_ascii_case("transfer-encoding")
            && value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("chunked"))
    })
}

fn parse_chunked_with_lines(
    raw_text: &str,
    body_start_line: usize,
) -> Option<(String, SourceLineMap, Vec<(String, String, usize)>)> {
    let dummy_headers = [Header {
        name: "Transfer-Encoding".into(),
        value: b"chunked".to_vec(),
    }];
    let (decoded, trailers) = super::body(&dummy_headers, raw_text.as_bytes()).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let bytes = raw_text.as_bytes();
    let mut pos = 0usize;
    let mut line = body_start_line;
    let mut dechunked_pos = 0usize;
    let mut spans = Vec::new();
    let mut trailer_lines = Vec::with_capacity(trailers.len());
    loop {
        let lf_rel = bytes.get(pos..)?.iter().position(|&b| b == b'\n')?;
        let line_end = if lf_rel > 0 && bytes[pos + lf_rel - 1] == b'\r' {
            pos + lf_rel - 1
        } else {
            pos + lf_rel
        };
        let size_text = std::str::from_utf8(
            bytes[pos..line_end]
                .split(|&c| c == b';')
                .next()
                .unwrap_or_default(),
        )
        .ok()?;
        let size = usize::from_str_radix(size_text, 16).ok()?;
        pos += lf_rel + 1;
        line += 1;
        if size == 0 {
            if bytes.get(pos..) != Some(b"\r\n") && bytes.get(pos..) != Some(b"\n") {
                for _ in 0..trailers.len() {
                    trailer_lines.push(line);
                    if let Some(rel) = bytes.get(pos..).and_then(|s| s.iter().position(|&b| b == b'\n')) {
                        pos += rel + 1;
                        line += 1;
                    }
                }
            }
            break;
        }
        spans.push((dechunked_pos, line));
        for i in 0..size {
            if bytes[pos + i] == b'\n' {
                line += 1;
                spans.push((dechunked_pos + i + 1, line));
            }
        }
        dechunked_pos += size;
        pos += size;
        if bytes.get(pos..pos + 2) == Some(b"\r\n") {
            pos += 2;
            line += 1;
        } else if bytes.get(pos) == Some(&b'\n') {
            pos += 1;
            line += 1;
        }
    }
    if spans.is_empty() {
        spans.push((0, body_start_line));
    }
    let trailer_pairs = trailers
        .into_iter()
        .enumerate()
        .map(|(idx, h)| {
            (
                h.name,
                String::from_utf8_lossy(&h.value).into_owned(),
                trailer_lines.get(idx).copied().unwrap_or(line),
            )
        })
        .collect();
    Some((text, SourceLineMap::Chunked { spans }, trailer_pairs))
}

fn dechunk_pretty_body_with_lines(
    body_str: &str,
    body_start_line: usize,
) -> Option<(String, SourceLineMap, Vec<(String, String, usize)>)> {
    if let Some(parsed) = parse_chunked_with_lines(body_str, body_start_line) {
        return Some(parsed);
    }
    if !body_str.contains("\r\n") {
        let crlf_body = body_str.replace('\n', "\r\n");
        return parse_chunked_with_lines(&crlf_body, body_start_line);
    }
    None
}

pub fn format_pretty_message_with_line_map(raw: &[u8]) -> std::result::Result<PrettyMessage, String> {
    if raw.is_empty() {
        return Ok(PrettyMessage {
            text: String::new(),
            line_numbers: None,
        });
    }
    let text = std::str::from_utf8(raw).map_err(|_| PRETTY_NON_UTF8_ERROR.to_owned())?;
    let mut out = PrettyLineBuilder::new(text.len());
    let split = text
        .find("\r\n\r\n")
        .map(|pos| (pos, 4))
        .into_iter()
        .chain(text.find("\n\n").map(|pos| (pos, 2)))
        .min_by_key(|(pos, _)| *pos);
    if let Some((head_end, delim_len)) = split {
        let head_str = &text[..head_end];
        let body_str = &text[head_end + delim_len..];
        let lines: Vec<&str> = head_str
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .collect();
        if let Some(&first_line) = lines.first()
            && is_http_start_line(first_line)
        {
            for (idx, line) in lines.iter().enumerate() {
                out.push_line(line, Some(idx + 1));
            }
            let blank_line = lines.len() + 1;
            let body_start_line = lines.len() + 2;
            out.push_line("", Some(blank_line));
            let kind = header_body_kind(&lines[1..]);
            if is_chunked_header_lines(&lines[1..])
                && let Some((dechunked, chunk_map, trailers)) =
                    dechunk_pretty_body_with_lines(body_str, body_start_line)
            {
                if dechunked.trim().is_empty() {
                    if !dechunked.is_empty() || trailers.is_empty() {
                        let normalized = dechunked.replace("\r\n", "\n");
                        for (idx, line) in normalized.split('\n').enumerate() {
                            out.push_line(line, Some(chunk_map.line_at(0) + idx));
                        }
                    }
                } else {
                    format_body_by_kind_with_lines(kind, &dechunked, &chunk_map, &mut out)?;
                }
                if !trailers.is_empty() {
                    let last_trailer_line = trailers.last().map_or(body_start_line, |t| t.2);
                    for (name, val, orig_line) in trailers {
                        out.push_line(&format!("{name}: {val}"), Some(orig_line));
                    }
                    out.push_line("", Some(last_trailer_line + 1));
                }
                return Ok(out.finalize());
            }
            if body_str.trim().is_empty() {
                out.push_normalized_lines(body_str, body_start_line);
            } else {
                let body_map = SourceLineMap::for_contiguous(body_str, body_start_line);
                format_body_by_kind_with_lines(kind, body_str, &body_map, &mut out)?;
            }
            return Ok(out.finalize());
        }
    }
    let full_map = SourceLineMap::for_contiguous(text, 1);
    let trimmed = text.trim();
    if format_json_if_valid_with_lines(text, trimmed, &full_map, &mut out) {
        return Ok(out.finalize());
    }
    if looks_like_html_document(trimmed) {
        format_pretty_html_with_lines(text, &full_map, &mut out)?;
        return Ok(out.finalize());
    }
    if looks_like_xml_document(trimmed) {
        format_pretty_xml_with_lines(text, &full_map, &mut out)?;
        return Ok(out.finalize());
    }
    out.push_normalized_lines(text, 1);
    Ok(out.finalize())
}

pub fn format_pretty_message(raw: &[u8]) -> std::result::Result<String, String> {
    format_pretty_message_with_line_map(raw).map(|pretty| pretty.text)
}
