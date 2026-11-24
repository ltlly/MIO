use std::ops::Range;

use crate::{InspectorItem, InspectorMutation};

const MAX_BOUNDARY_BYTES: usize = 70;
const MAX_MULTIPART_INSPECTOR_BYTES: usize = 2 * 1024 * 1024;

struct BoundaryLine {
    start: usize,
    after: usize,
    closing: bool,
}

struct Part {
    name: String,
    full_range: Range<usize>,
    name_range: Range<usize>,
    name_quoted: bool,
    body_range: Range<usize>,
}

fn strip_line_ending(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

pub(super) fn multipart_boundary_from_headers(lines: &[&[u8]]) -> Option<Vec<u8>> {
    for line in lines {
        let content = strip_line_ending(line);
        let Some(colon) = content.iter().position(|byte| *byte == b':') else {
            continue;
        };
        let name = content[..colon].trim_ascii();
        if !name.eq_ignore_ascii_case(b"content-type") {
            continue;
        }
        let value = content[colon + 1..].trim_ascii();
        let mut segments = value.split(|byte| *byte == b';');
        let essence = segments.next()?.trim_ascii();
        if !essence.eq_ignore_ascii_case(b"multipart/form-data") {
            return None;
        }
        for param in segments {
            let param = param.trim_ascii();
            if param.len() >= 9 && param[..9].eq_ignore_ascii_case(b"boundary=") {
                let raw = param[9..].trim_ascii();
                let unquoted = if raw.len() >= 2 && raw[0] == b'"' && raw[raw.len() - 1] == b'"' {
                    &raw[1..raw.len() - 1]
                } else {
                    raw
                };
                if !unquoted.is_empty() && unquoted.len() <= MAX_BOUNDARY_BYTES {
                    return Some(unquoted.to_vec());
                }
            }
        }
        return None;
    }
    None
}

fn scan_boundaries(body: &[u8], marker: &[u8]) -> Result<Vec<BoundaryLine>, String> {
    let mut found = Vec::new();
    let mut pos = 0;
    while pos < body.len() {
        let line_end = body[pos..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(body.len(), |offset| pos + offset + 1);
        let content = strip_line_ending(&body[pos..line_end]);
        if let Some(tail) = content.strip_prefix(marker) {
            let (tail, closing) = tail
                .strip_prefix(b"--")
                .map_or((tail, false), |rest| (rest, true));
            if tail.iter().all(|byte| matches!(byte, b' ' | b'\t')) {
                found.push(BoundaryLine {
                    start: pos,
                    after: line_end,
                    closing,
                });
                if closing {
                    break;
                }
                if found.len() > 4096 {
                    return Err("Multipart 字段数量超过 4096 限额。".into());
                }
            }
        }
        pos = line_end;
    }
    if found.last().is_none_or(|last| !last.closing) {
        return Err("Multipart 请求正文缺少完整的结束 boundary。".into());
    }
    Ok(found)
}

fn find_name_in_disposition(header_line_content: &[u8]) -> Option<(Range<usize>, bool)> {
    let colon = header_line_content.iter().position(|byte| *byte == b':')?;
    if !header_line_content[..colon]
        .trim_ascii()
        .eq_ignore_ascii_case(b"content-disposition")
    {
        return None;
    }
    let mut cursor = colon + 1;
    let after_colon = &header_line_content[cursor..];
    let semi_rel = after_colon.iter().position(|byte| *byte == b';')?;
    let essence = after_colon[..semi_rel].trim_ascii();
    if !essence.eq_ignore_ascii_case(b"form-data") {
        return None;
    }
    cursor += semi_rel + 1;
    while cursor < header_line_content.len() {
        while cursor < header_line_content.len()
            && header_line_content[cursor].is_ascii_whitespace()
        {
            cursor += 1;
        }
        let param_start = cursor;
        let mut quote: Option<u8> = None;
        while cursor < header_line_content.len() {
            let byte = header_line_content[cursor];
            if let Some(q) = quote {
                if byte == b'\\' {
                    cursor = (cursor + 2).min(header_line_content.len());
                    continue;
                }
                if byte == q {
                    quote = None;
                }
            } else if matches!(byte, b'"' | b'\'') {
                quote = Some(byte);
            } else if byte == b';' {
                break;
            }
            cursor += 1;
        }
        let param_end = cursor;
        if cursor < header_line_content.len() && header_line_content[cursor] == b';' {
            cursor += 1;
        }
        let param = &header_line_content[param_start..param_end];
        if param.len() >= 5 && param[..5].eq_ignore_ascii_case(b"name=") {
            let val_start = param_start + 5;
            let raw_val = &header_line_content[val_start..param_end];
            let trimmed_end = param_end
                - raw_val
                    .iter()
                    .rev()
                    .take_while(|byte| byte.is_ascii_whitespace())
                    .count();
            let raw_trimmed = &header_line_content[val_start..trimmed_end];
            if raw_trimmed.len() >= 2
                && raw_trimmed[0] == b'"'
                && raw_trimmed[raw_trimmed.len() - 1] == b'"'
            {
                return Some(((val_start + 1)..(trimmed_end - 1), true));
            }
            if !raw_trimmed.is_empty() {
                return Some((val_start..trimmed_end, false));
            }
        }
    }
    None
}

fn parse_part(body: &[u8], part_start: usize, part_end: usize) -> Result<Part, String> {
    let slice = &body[part_start..part_end];
    let mut pos = 0;
    let mut found_name: Option<(Range<usize>, bool)> = None;
    let mut body_rel_start: Option<usize> = None;
    while pos < slice.len() {
        let line_end = slice[pos..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(slice.len(), |offset| pos + offset + 1);
        let content = strip_line_ending(&slice[pos..line_end]);
        if content.is_empty() {
            body_rel_start = Some(line_end);
            break;
        }
        if found_name.is_none()
            && let Some((rel_range, quoted)) = find_name_in_disposition(content)
        {
            found_name = Some((
                (part_start + pos + rel_range.start)..(part_start + pos + rel_range.end),
                quoted,
            ));
        }
        pos = line_end;
    }
    let body_start = part_start
        + body_rel_start.ok_or_else(|| "Multipart 字段缺少头与正文分隔空行。".to_owned())?;
    let (name_range, name_quoted) = found_name.ok_or_else(|| {
        "Multipart 字段缺少有效的 Content-Disposition form-data name。".to_owned()
    })?;
    let name = String::from_utf8_lossy(&body[name_range.clone()]).into_owned();
    Ok(Part {
        name,
        full_range: part_start..part_end,
        name_range,
        name_quoted,
        body_range: body_start..part_end,
    })
}

fn locate_parts(body: &[u8], boundaries: &[BoundaryLine]) -> Result<Vec<Part>, String> {
    let mut parts = Vec::new();
    for pair in boundaries.windows(2) {
        let (cur, next) = (&pair[0], &pair[1]);
        if cur.closing {
            break;
        }
        let part_start = cur.after;
        let part_end = if next.start == part_start {
            part_start
        } else if next.start >= part_start + 2 && &body[next.start - 2..next.start] == b"\r\n" {
            next.start - 2
        } else if next.start > part_start && body[next.start - 1] == b'\n' {
            next.start - 1
        } else {
            return Err("Multipart 字段与 boundary 之间缺少换行符。".into());
        };
        parts.push(parse_part(body, part_start, part_end)?);
    }
    Ok(parts)
}

pub(super) fn inspect_multipart_body(body: &[u8], boundary: &[u8]) -> Vec<InspectorItem> {
    if body.len() > MAX_MULTIPART_INSPECTOR_BYTES {
        return Vec::new();
    }
    let mut marker = Vec::with_capacity(boundary.len() + 2);
    marker.extend_from_slice(b"--");
    marker.extend_from_slice(boundary);
    let Ok(boundaries) = scan_boundaries(body, &marker) else {
        return Vec::new();
    };
    let Ok(parts) = locate_parts(body, &boundaries) else {
        return Vec::new();
    };
    parts
        .into_iter()
        .map(|part| InspectorItem {
            name: part.name,
            value: String::from_utf8_lossy(&body[part.body_range]).into_owned(),
        })
        .collect()
}

fn validate_multipart_kv(
    name: &str,
    value: &str,
    marker: &[u8],
    preserving_binary: bool,
) -> Result<(), String> {
    if name.is_empty() {
        return Err("Multipart 参数名称不能为空。".into());
    }
    if name.contains(['"', '\\', ';', '\r', '\n', '\0']) {
        return Err("Multipart 参数名称不能包含引号、分号、反斜杠、换行符或 NUL。".into());
    }
    if !preserving_binary && value.contains('\0') {
        return Err("Multipart 参数文本值不能包含 NUL。".into());
    }
    if value
        .as_bytes()
        .split(|byte| *byte == b'\n')
        .any(|line| strip_line_ending(line).starts_with(marker))
    {
        return Err("Multipart 参数值包含当前 boundary 分隔行，为保护报文结构未写入。".into());
    }
    Ok(())
}

pub(super) fn mutate_multipart_body(
    body: &[u8],
    boundary: &[u8],
    mutation: InspectorMutation,
) -> Result<Vec<u8>, String> {
    if body.len() > MAX_MULTIPART_INSPECTOR_BYTES {
        return Err("Multipart 请求正文超过 2 MiB 限额，无法在 Inspector 中修改。".into());
    }
    let mut marker = Vec::with_capacity(boundary.len() + 2);
    marker.extend_from_slice(b"--");
    marker.extend_from_slice(boundary);
    let boundaries = scan_boundaries(body, &marker)?;
    let parts = locate_parts(body, &boundaries)?;
    let first_boundary = &boundaries[0];
    let closing_boundary = boundaries.last().expect("scan_boundaries verified closing");
    let newline: &[u8] = if body[first_boundary.start..first_boundary.after].ends_with(b"\r\n") {
        b"\r\n"
    } else {
        b"\n"
    };

    let mut out = Vec::with_capacity(body.len() + 128);
    out.extend_from_slice(&body[..first_boundary.start]);

    match mutation {
        InspectorMutation::Add { name, value, .. } => {
            validate_multipart_kv(&name, &value, &marker, false)?;
            for part in &parts {
                out.extend_from_slice(&marker);
                out.extend_from_slice(newline);
                out.extend_from_slice(&body[part.full_range.clone()]);
                out.extend_from_slice(newline);
            }
            out.extend_from_slice(&marker);
            out.extend_from_slice(newline);
            out.extend_from_slice(b"Content-Disposition: form-data; name=\"");
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(b"\"");
            out.extend_from_slice(newline);
            out.extend_from_slice(newline);
            out.extend_from_slice(value.as_bytes());
            out.extend_from_slice(newline);
        }
        InspectorMutation::Update {
            index, name, value, ..
        } => {
            if index >= parts.len() {
                return Err("Inspector 项目索引超出范围。".into());
            }
            let orig_target = &body[parts[index].body_range.clone()];
            let preserving_binary = std::str::from_utf8(orig_target).is_err()
                && value == String::from_utf8_lossy(orig_target).as_ref();
            validate_multipart_kv(&name, &value, &marker, preserving_binary)?;
            for (idx, part) in parts.iter().enumerate() {
                out.extend_from_slice(&marker);
                out.extend_from_slice(newline);
                if idx != index {
                    out.extend_from_slice(&body[part.full_range.clone()]);
                    out.extend_from_slice(newline);
                    continue;
                }
                let orig_body = &body[part.body_range.clone()];
                let next_body: &[u8] = if std::str::from_utf8(orig_body).is_ok() {
                    value.as_bytes()
                } else {
                    let lossy = String::from_utf8_lossy(orig_body);
                    if value == lossy.as_ref() {
                        orig_body
                    } else if value.contains('\u{FFFD}') {
                        return Err(
                            "该 Multipart 字段包含非 UTF-8 二进制字节；请勿通过含替换符文本覆盖原始二进制，可在 Hex 视图编辑。"
                                .into(),
                        );
                    } else {
                        value.as_bytes()
                    }
                };
                out.extend_from_slice(&body[part.full_range.start..part.name_range.start]);
                if !part.name_quoted && name.contains(' ') {
                    out.push(b'"');
                    out.extend_from_slice(name.as_bytes());
                    out.push(b'"');
                } else {
                    out.extend_from_slice(name.as_bytes());
                }
                out.extend_from_slice(&body[part.name_range.end..part.body_range.start]);
                out.extend_from_slice(next_body);
                out.extend_from_slice(newline);
            }
        }
        InspectorMutation::Remove { index, .. } => {
            if index >= parts.len() {
                return Err("Inspector 项目索引超出范围。".into());
            }
            for (idx, part) in parts.iter().enumerate() {
                if idx == index {
                    continue;
                }
                out.extend_from_slice(&marker);
                out.extend_from_slice(newline);
                out.extend_from_slice(&body[part.full_range.clone()]);
                out.extend_from_slice(newline);
            }
        }
        InspectorMutation::Move { from, to, .. } => {
            if from >= parts.len() || to >= parts.len() {
                return Err("Inspector 项目索引超出范围。".into());
            }
            let mut order: Vec<usize> = (0..parts.len()).collect();
            if from != to {
                let idx = order.remove(from);
                order.insert(to, idx);
            }
            for part_idx in order {
                let part = &parts[part_idx];
                out.extend_from_slice(&marker);
                out.extend_from_slice(newline);
                out.extend_from_slice(&body[part.full_range.clone()]);
                out.extend_from_slice(newline);
            }
        }
    }

    out.extend_from_slice(&body[closing_boundary.start..]);
    Ok(out)
}
