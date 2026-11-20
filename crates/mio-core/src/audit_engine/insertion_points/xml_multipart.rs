use std::ops::Range;

use anyhow::{Context, Result, bail, ensure};

#[derive(Clone, Copy, PartialEq, Eq)]
enum XmlSpanKind {
    Text,
    Cdata,
    Attr(u8),
}

pub(super) struct XmlSpan {
    pub(super) name: String,
    pub(super) value: String,
    range: Range<usize>,
    kind: XmlSpanKind,
}

pub(super) struct MultipartSpan {
    pub(super) name: String,
    pub(super) value: String,
    range: Range<usize>,
}

fn xml_unescape(raw: &str) -> String {
    raw.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn xml_escape(val: &str, quote: Option<u8>) -> String {
    let mut out = String::with_capacity(val.len());
    for ch in val.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if quote == Some(b'"') => out.push_str("&quot;"),
            '\'' if quote == Some(b'\'') => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

fn find_bytes(raw: &[u8], start: usize, needle: &[u8]) -> Result<usize> {
    raw.get(start..)
        .and_then(|s| s.windows(needle.len()).position(|w| w == needle))
        .map(|off| start + off)
        .context("XML 结构未完整闭合")
}

pub(super) fn parse_xml_spans(body: &[u8]) -> Result<Vec<XmlSpan>> {
    let text = std::str::from_utf8(body).context("XML 正文必须是有效 UTF-8")?;
    let bom = if text.starts_with('\u{feff}') { 3 } else { 0 };
    roxmltree::Document::parse_with_options(
        &text[bom..],
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 4096,
            entity_resolver: None,
        },
    )
    .context("XML 语法无效或包含外部实体")?;

    let mut at = bom;
    let end = body.len();
    let mut stack: Vec<String> = Vec::new();
    let mut spans = Vec::new();
    while at < end {
        if body[at..end].starts_with(b"<!--") {
            at = find_bytes(body, at + 4, b"-->")? + 3;
        } else if body[at..end].starts_with(b"<![CDATA[") {
            let close = find_bytes(body, at + 9, b"]]>")?;
            if !stack.is_empty() {
                spans.push(XmlSpan {
                    name: stack.join("/"),
                    value: text[at + 9..close].to_owned(),
                    range: at + 9..close,
                    kind: XmlSpanKind::Cdata,
                });
            }
            at = close + 3;
        } else if body[at..end].starts_with(b"<?") {
            at = find_bytes(body, at + 2, b"?>")? + 2;
        } else if body[at..end].starts_with(b"<!") {
            bail!("不支持包含 DTD 的 XML");
        } else if body[at..end].starts_with(b"</") {
            stack.pop().context("XML 闭合标签不匹配")?;
            at = find_bytes(body, at + 2, b">")? + 1;
        } else if body[at] == b'<' {
            at += 1;
            let tag_start = at;
            while at < end && !body[at].is_ascii_whitespace() && !matches!(body[at], b'>' | b'/') {
                at += 1;
            }
            let tag_name = text[tag_start..at].to_owned();
            ensure!(stack.len() < 64, "XML 嵌套深度超过上限");
            stack.push(tag_name);
            loop {
                while at < end && body[at].is_ascii_whitespace() {
                    at += 1;
                }
                ensure!(at < end, "XML 标签未闭合");
                if body[at..end].starts_with(b"/>") {
                    stack.pop();
                    at += 2;
                    break;
                }
                if body[at] == b'>' {
                    at += 1;
                    break;
                }
                let attr_start = at;
                while at < end
                    && !body[at].is_ascii_whitespace()
                    && !matches!(body[at], b'=' | b'>' | b'/')
                {
                    at += 1;
                }
                let attr_name = &text[attr_start..at];
                while at < end && body[at].is_ascii_whitespace() {
                    at += 1;
                }
                ensure!(at < end && body[at] == b'=', "XML 属性缺少等号");
                at += 1;
                while at < end && body[at].is_ascii_whitespace() {
                    at += 1;
                }
                ensure!(
                    at < end && matches!(body[at], b'"' | b'\''),
                    "XML 属性缺少引号"
                );
                let quote = body[at];
                let val_start = at + 1;
                let val_end = body[val_start..end]
                    .iter()
                    .position(|&b| b == quote)
                    .context("XML 属性引号未闭合")?
                    + val_start;
                spans.push(XmlSpan {
                    name: format!("{}@{attr_name}", stack.join("/")),
                    value: xml_unescape(&text[val_start..val_end]),
                    range: val_start..val_end,
                    kind: XmlSpanKind::Attr(quote),
                });
                at = val_end + 1;
            }
        } else {
            let next_tag = body[at..end]
                .iter()
                .position(|&b| b == b'<')
                .map_or(end, |off| at + off);
            if !body[at..next_tag].trim_ascii().is_empty() && !stack.is_empty() {
                spans.push(XmlSpan {
                    name: stack.join("/"),
                    value: xml_unescape(&text[at..next_tag]),
                    range: at..next_tag,
                    kind: XmlSpanKind::Text,
                });
            }
            at = next_tag;
        }
    }
    Ok(spans)
}

pub(super) fn rebuild_xml_body(
    body: &[u8],
    name: &str,
    occurrence: usize,
    value: &str,
) -> Result<Vec<u8>> {
    ensure!(
        !value.chars().any(|c| {
            matches!(
                c as u32,
                0x00..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F | 0xFFFE | 0xFFFF
            )
        }),
        "XML 参数值包含非法控制字符"
    );
    let spans = parse_xml_spans(body)?;
    let span = spans
        .into_iter()
        .filter(|s| s.name == name)
        .nth(occurrence)
        .with_context(|| format!("XML 参数点不存在: {name}[{occurrence}]"))?;
    let encoded = match span.kind {
        XmlSpanKind::Text => xml_escape(value, None),
        XmlSpanKind::Attr(q) => xml_escape(value, Some(q)),
        XmlSpanKind::Cdata => value.replace("]]>", "]]]]><![CDATA[>"),
    };
    let mut out = Vec::with_capacity(body.len() + encoded.len());
    out.extend_from_slice(&body[..span.range.start]);
    out.extend_from_slice(encoded.as_bytes());
    out.extend_from_slice(&body[span.range.end..]);
    let check_text = std::str::from_utf8(&out).context("重构 XML 不是有效 UTF-8")?;
    let bom = if check_text.starts_with('\u{feff}') {
        3
    } else {
        0
    };
    roxmltree::Document::parse_with_options(
        &check_text[bom..],
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 4096,
            entity_resolver: None,
        },
    )
    .context("重构后的 XML 语法无效")?;
    Ok(out)
}

fn find_boundary_line(raw: &[u8], mut at: usize, marker: &[u8]) -> Result<(usize, usize, bool)> {
    while at < raw.len() {
        let end = raw[at..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(raw.len(), |off| at + off);
        let line_end = end - usize::from(end > at && raw[end - 1] == b'\r');
        if let Some(tail) = raw[at..line_end].strip_prefix(marker) {
            let (tail, closing) = tail
                .strip_prefix(b"--")
                .map_or((tail, false), |rest| (rest, true));
            if tail.iter().all(|b| matches!(b, b' ' | b'\t')) {
                return Ok((at, (end + 1).min(raw.len()), closing));
            }
        }
        at = end + 1;
    }
    bail!("Multipart 缺少结束 boundary")
}

fn extract_disposition_param(line: &str, key: &str) -> Option<String> {
    for seg in line.split(';').skip(1) {
        let (k, v) = seg.trim().split_once('=')?;
        if k.trim().eq_ignore_ascii_case(key) {
            let v = v.trim();
            if let Some(inner) = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                return Some(inner.replace("\\\"", "\"").replace("\\\\", "\\"));
            }
            return Some(v.to_owned());
        }
    }
    None
}

pub(super) fn parse_multipart_spans(body: &[u8], boundary: &[u8]) -> Result<Vec<MultipartSpan>> {
    ensure!(
        !boundary.is_empty() && boundary.len() <= 70,
        "boundary 无效"
    );
    let mut marker = b"--".to_vec();
    marker.extend_from_slice(boundary);
    let (_, mut cursor, mut closing) = find_boundary_line(body, 0, &marker)?;
    let mut spans = Vec::new();
    let mut parts = 0usize;
    while !closing {
        parts += 1;
        ensure!(parts <= 256, "Multipart 字段数超过上限");
        let mut disp_line = None;
        let mut part_ct = None;
        loop {
            let end = body
                .get(cursor..)
                .and_then(|s| s.iter().position(|&b| b == b'\n'))
                .map(|off| cursor + off)
                .context("Multipart 头未闭合")?;
            let line_end = end - usize::from(end > cursor && body[end - 1] == b'\r');
            let line = &body[cursor..line_end];
            cursor = end + 1;
            if line.is_empty() {
                break;
            }
            if let Ok(hdr_str) = std::str::from_utf8(line)
                && let Some((hn, hv)) = hdr_str.split_once(':')
            {
                if hn.trim().eq_ignore_ascii_case("content-disposition") {
                    disp_line = Some(hv.trim().to_owned());
                } else if hn.trim().eq_ignore_ascii_case("content-type") {
                    part_ct = Some(hv.trim().to_ascii_lowercase());
                }
            }
        }
        let body_start = cursor;
        let (next_start, next_after, next_closing) = find_boundary_line(body, body_start, &marker)?;
        let mut body_end = next_start;
        if body_end >= body_start + 2 && &body[body_end - 2..body_end] == b"\r\n" {
            body_end -= 2;
        } else if body_end > body_start && body[body_end - 1] == b'\n' {
            body_end -= 1;
        }
        cursor = next_after;
        closing = next_closing;

        let Some(disp) = disp_line else { continue };
        let has_file = extract_disposition_param(&disp, "filename").is_some()
            || extract_disposition_param(&disp, "filename*").is_some();
        let binary_ct = part_ct.as_deref().is_some_and(|ct| {
            ct.starts_with("application/octet-stream")
                || ct.starts_with("image/")
                || ct.starts_with("audio/")
                || ct.starts_with("video/")
        });
        let raw_val = &body[body_start..body_end];
        if !has_file
            && !binary_ct
            && !raw_val.contains(&0)
            && let Some(name) = extract_disposition_param(&disp, "name")
            && !name.is_empty()
            && let Ok(text_val) = std::str::from_utf8(raw_val)
        {
            spans.push(MultipartSpan {
                name,
                value: text_val.to_owned(),
                range: body_start..body_end,
            });
        }
    }
    Ok(spans)
}

pub(super) fn rebuild_multipart_body(
    body: &[u8],
    boundary: &[u8],
    name: &str,
    occurrence: usize,
    value: &str,
) -> Result<Vec<u8>> {
    let mut marker = b"--".to_vec();
    marker.extend_from_slice(boundary);
    ensure!(
        !value
            .as_bytes()
            .windows(marker.len())
            .any(|w| w == marker.as_slice()),
        "Multipart 参数值不能包含 boundary 分隔符"
    );
    let spans = parse_multipart_spans(body, boundary)?;
    let span = spans
        .into_iter()
        .filter(|s| s.name == name)
        .nth(occurrence)
        .with_context(|| format!("Multipart 参数点不存在: {name}[{occurrence}]"))?;
    let mut out = Vec::with_capacity(body.len() + value.len());
    out.extend_from_slice(&body[..span.range.start]);
    out.extend_from_slice(value.as_bytes());
    out.extend_from_slice(&body[span.range.end..]);
    Ok(out)
}
