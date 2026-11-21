use super::*;

pub(in crate::http) mod json_body;
mod multipart_body;
mod selection_codec;
mod wire_helpers;

pub use selection_codec::{
    DecodedInspectorSelection, InspectorSelectionCodec, decode_inspector_selection,
    encode_inspector_selection,
};
pub use wire_helpers::{
    apply_inspector_response_mutation, detect_request_line_version, rewrite_request_line_version,
};

#[derive(Debug)]
enum BodyMode {
    Multipart(Vec<u8>),
    Json,
    Form,
    Other,
}

fn inspector_line_content(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn inspector_message_parts(raw: &[u8]) -> (Vec<&[u8]>, &[u8], &[u8]) {
    let mut lines = Vec::new();
    let mut offset = 0;
    for line in raw.split_inclusive(|byte| *byte == b'\n') {
        if inspector_line_content(line).is_empty() {
            return (lines, line, &raw[offset + line.len()..]);
        }
        lines.push(line);
        offset += line.len();
    }
    (lines, &[], &[])
}

fn inspector_request_parts(line: &[u8]) -> Option<[std::ops::Range<usize>; 3]> {
    let line = inspector_line_content(line);
    let mut ranges = Vec::new();
    let mut offset = 0;
    while offset < line.len() {
        if line[offset].is_ascii_whitespace() {
            offset += 1;
            continue;
        }
        let start = offset;
        while offset < line.len() && !line[offset].is_ascii_whitespace() {
            offset += 1;
        }
        ranges.push(start..offset);
    }
    let ranges: [std::ops::Range<usize>; 3] = ranges.try_into().ok()?;
    (token(&line[ranges[0].clone()]) && line[ranges[2].clone()].starts_with(b"HTTP/"))
        .then_some(ranges)
}

fn inspector_header(line: &[u8]) -> Option<(&[u8], &[u8])> {
    let line = inspector_line_content(line);
    let colon = line.iter().position(|byte| *byte == b':')?;
    let name = line[..colon].trim_ascii();
    if name.is_empty() {
        return None;
    }
    Some((name, line[colon + 1..].trim_ascii()))
}

fn inspector_item(name: &[u8], value: &[u8]) -> InspectorItem {
    InspectorItem {
        name: String::from_utf8_lossy(name).into_owned(),
        value: String::from_utf8_lossy(value).into_owned(),
    }
}

fn inspector_parameters(text: &str, separator: char) -> Vec<InspectorItem> {
    text.split(separator)
        .map(|part| if separator == ';' { part.trim() } else { part })
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (name, value) = part.split_once('=').unwrap_or((part, ""));
            InspectorItem {
                name: name.into(),
                value: value.into(),
            }
        })
        .collect()
}

fn detect_body_mode(header_lines: &[&[u8]], body: &[u8]) -> BodyMode {
    if let Some(boundary) = multipart_body::multipart_boundary_from_headers(header_lines) {
        return BodyMode::Multipart(boundary);
    }
    let mut saw_content_type = false;
    for line in header_lines {
        let Some((name, value)) = inspector_header(line) else {
            continue;
        };
        if !name.eq_ignore_ascii_case(b"content-type") {
            continue;
        }
        saw_content_type = true;
        let essence = value
            .split(|byte| *byte == b';')
            .next()
            .unwrap_or(&[])
            .trim_ascii();
        if essence.eq_ignore_ascii_case(b"application/x-www-form-urlencoded") {
            return BodyMode::Form;
        }
        if essence.eq_ignore_ascii_case(b"application/json")
            || (essence.len() > 5 && essence[essence.len() - 5..].eq_ignore_ascii_case(b"+json"))
        {
            return BodyMode::Json;
        }
    }
    if !saw_content_type {
        if json_body::inspect_json_body(body).is_some() {
            return BodyMode::Json;
        }
        if let Ok(text) = std::str::from_utf8(body)
            && !text.contains(['\r', '\n'])
            && text.contains('=')
            && !text.trim_start().starts_with(['{', '[', '<'])
        {
            return BodyMode::Form;
        }
    }
    BodyMode::Other
}

fn update_single_content_length(lines: &mut [Vec<u8>], body_len: usize) {
    let mut length_headers = Vec::new();
    let mut chunked = false;
    for (index, line) in lines.iter().enumerate().skip(1) {
        if let Some((name, value)) = inspector_header(line) {
            if name.eq_ignore_ascii_case(b"content-length") {
                length_headers.push(index);
            }
            if name.eq_ignore_ascii_case(b"transfer-encoding") {
                chunked |= value
                    .split(|byte| *byte == b',')
                    .any(|encoding| encoding.trim_ascii().eq_ignore_ascii_case(b"chunked"));
            }
        }
    }
    if !chunked && length_headers.len() == 1 {
        let index = length_headers[0];
        let line = &mut lines[index];
        let colon = line.iter().position(|byte| *byte == b':').unwrap();
        let content_len = inspector_line_content(line).len();
        let value_start = colon
            + 1
            + line[colon + 1..content_len]
                .iter()
                .take_while(|byte| byte.is_ascii_whitespace())
                .count();
        let value_end = content_len
            - line[value_start..content_len]
                .iter()
                .rev()
                .take_while(|byte| byte.is_ascii_whitespace())
                .count();
        line.splice(value_start..value_end, body_len.to_string().bytes());
    }
}

pub fn inspect_http_messages(request_raw: &[u8], response_raw: Option<&[u8]>) -> InspectorData {
    let mut data = InspectorData::default();
    let (lines, _, body) = inspector_message_parts(request_raw);
    if let Some(first) = lines.first()
        && let Some(parts) = inspector_request_parts(first)
    {
        let target = String::from_utf8_lossy(&first[parts[1].clone()]);
        let (path, query) = target.split_once('?').unwrap_or((&target, ""));
        data.attributes = vec![
            inspector_item(b"Method", &first[parts[0].clone()]),
            InspectorItem {
                name: "Path".into(),
                value: path.into(),
            },
            inspector_item(b"Protocol", &first[parts[2].clone()]),
        ];
        data.query_params = inspector_parameters(query, '&');
        for line in &lines[1..] {
            if let Some((name, value)) = inspector_header(line) {
                let item = inspector_item(name, value);
                if name.eq_ignore_ascii_case(b"cookie") {
                    data.cookies.extend(inspector_parameters(&item.value, ';'));
                }
                data.request_headers.push(item);
            }
        }
        match detect_body_mode(&lines[1..], body) {
            BodyMode::Multipart(boundary) => {
                data.body_params = multipart_body::inspect_multipart_body(body, &boundary);
            }
            BodyMode::Json => {
                data.body_params = json_body::inspect_json_body(body).unwrap_or_default();
            }
            BodyMode::Form => {
                data.body_params = inspector_parameters(&String::from_utf8_lossy(body), '&');
            }
            BodyMode::Other => {}
        }
    }
    if let Some(raw) = response_raw {
        let (lines, _, _) = inspector_message_parts(raw);
        for line in lines {
            if inspector_line_content(line).starts_with(b"HTTP/") {
                continue;
            }
            if let Some((name, value)) = inspector_header(line) {
                data.response_headers.push(inspector_item(name, value));
            }
        }
    }
    data
}

fn mutate_inspector_items(
    items: &mut Vec<InspectorItem>,
    mutation: InspectorMutation,
) -> std::result::Result<(), String> {
    match mutation {
        InspectorMutation::Add { name, value, .. } => items.push(InspectorItem { name, value }),
        InspectorMutation::Update {
            index, name, value, ..
        } => {
            let item = items
                .get_mut(index)
                .ok_or_else(|| "Inspector 项目索引超出范围。".to_owned())?;
            *item = InspectorItem { name, value };
        }
        InspectorMutation::Remove { index, .. } => {
            if index >= items.len() {
                return Err("Inspector 项目索引超出范围。".into());
            }
            items.remove(index);
        }
        InspectorMutation::Move { from, to, .. } => {
            if from >= items.len() || to >= items.len() {
                return Err("Inspector 项目索引超出范围。".into());
            }
            if from != to {
                let item = items.remove(from);
                items.insert(to, item);
            }
        }
    }
    Ok(())
}

fn join_inspector_items(items: &[InspectorItem], separator: &str) -> String {
    items
        .iter()
        .map(|item| format!("{}={}", item.name, item.value))
        .collect::<Vec<_>>()
        .join(separator)
}

pub fn apply_inspector_mutation(
    request_raw: &[u8],
    mutation: InspectorMutation,
) -> std::result::Result<Vec<u8>, String> {
    let section = match &mutation {
        InspectorMutation::Add {
            section,
            name,
            value,
        }
        | InspectorMutation::Update {
            section,
            name,
            value,
            ..
        } => {
            if name.contains(['\r', '\n', '\0'])
                || (*section != InspectorSection::BodyParams && value.contains(['\r', '\n', '\0']))
            {
                return Err("Inspector 名称和值不能包含换行符或 NUL。".into());
            }
            *section
        }
        InspectorMutation::Remove { section, .. } => *section,
        InspectorMutation::Move { section, .. } => *section,
    };
    if section == InspectorSection::ResponseHeaders {
        return Err("响应标头为只读，不能在 Inspector 中修改。".into());
    }
    let (original_lines, separator, original_body) = inspector_message_parts(request_raw);
    let first = original_lines
        .first()
        .ok_or_else(|| "请求缺少有效的 HTTP 起始行。".to_owned())?;
    let parts =
        inspector_request_parts(first).ok_or_else(|| "请求缺少有效的 HTTP 起始行。".to_owned())?;
    let newline: &[u8] = if first.ends_with(b"\r\n") {
        b"\r\n"
    } else {
        b"\n"
    };
    let mut lines = original_lines
        .iter()
        .map(|line| line.to_vec())
        .collect::<Vec<_>>();
    let mut new_body = None;
    match section {
        InspectorSection::RequestAttributes => {
            let InspectorMutation::Update {
                index, name, value, ..
            } = mutation
            else {
                return Err("请求属性仅支持修改 Method、Path 和 Protocol。".into());
            };
            let range = if index == 0 || name.eq_ignore_ascii_case("method") {
                if !token(value.as_bytes()) {
                    return Err("请求方法无效。".into());
                }
                parts[0].clone()
            } else if index == 1 || name.eq_ignore_ascii_case("path") {
                if value.is_empty() || value.bytes().any(|byte| byte.is_ascii_whitespace()) {
                    return Err("请求路径不能为空或包含空白字符。".into());
                }
                let target = &first[parts[1].clone()];
                let path_len = target
                    .iter()
                    .position(|byte| *byte == b'?')
                    .unwrap_or(target.len());
                parts[1].start..parts[1].start + path_len
            } else if index == 2 || name.eq_ignore_ascii_case("protocol") {
                let trimmed = value.trim();
                if !matches!(trimmed, "HTTP/1.0" | "HTTP/1.1" | "HTTP/2" | "HTTP/2.0") {
                    return Err("请求协议版本须为 HTTP/1.0、HTTP/1.1 或 HTTP/2。".into());
                }
                parts[2].clone()
            } else {
                return Err("请求属性仅支持修改 Method、Path 和 Protocol。".into());
            };
            lines[0].splice(range, value.trim().bytes());
        }
        InspectorSection::QueryParams => {
            let target = std::str::from_utf8(&first[parts[1].clone()])
                .map_err(|_| "请求目标含非 UTF-8 字节，无法修改查询参数。".to_owned())?;
            let (path, query) = target.split_once('?').unwrap_or((target, ""));
            let mut items = inspector_parameters(query, '&');
            mutate_inspector_items(&mut items, mutation)?;
            let target = if items.is_empty() {
                path.to_owned()
            } else {
                format!("{path}?{}", join_inspector_items(&items, "&"))
            };
            lines[0].splice(parts[1].clone(), target.bytes());
        }
        InspectorSection::BodyParams => {
            let body = match detect_body_mode(&original_lines[1..], original_body) {
                BodyMode::Multipart(boundary) => {
                    multipart_body::mutate_multipart_body(original_body, &boundary, mutation)?
                }
                BodyMode::Json => {
                    if let InspectorMutation::Add { value, .. }
                    | InspectorMutation::Update { value, .. } = &mutation
                        && value.contains('\0')
                    {
                        return Err("Inspector 名称和值不能包含换行符或 NUL。".into());
                    }
                    json_body::mutate_json_body(original_body, mutation)?
                }
                BodyMode::Form | BodyMode::Other => {
                    if let InspectorMutation::Add { value, .. }
                    | InspectorMutation::Update { value, .. } = &mutation
                        && value.contains(['\r', '\n', '\0'])
                    {
                        return Err("Inspector 名称和值不能包含换行符或 NUL。".into());
                    }
                    let text = std::str::from_utf8(original_body)
                        .map_err(|_| "请求正文含非 UTF-8 字节，无法修改正文参数。".to_owned())?;
                    let mut items = inspector_parameters(text, '&');
                    mutate_inspector_items(&mut items, mutation)?;
                    join_inspector_items(&items, "&").into_bytes()
                }
            };
            update_single_content_length(&mut lines, body.len());
            new_body = Some(body);
        }
        InspectorSection::Cookies => {
            let mut items = Vec::new();
            let mut cookie_indices = Vec::new();
            for (index, line) in lines.iter().enumerate().skip(1) {
                if let Some((name, value)) = inspector_header(line)
                    && name.eq_ignore_ascii_case(b"cookie")
                {
                    let text = std::str::from_utf8(value).map_err(|_| {
                        "Cookie 含非 UTF-8 字节，无法在 Inspector 中修改。".to_owned()
                    })?;
                    items.extend(inspector_parameters(text, ';'));
                    cookie_indices.push(index);
                }
            }
            mutate_inspector_items(&mut items, mutation)?;
            let replacement = if items.is_empty() {
                None
            } else {
                let mut line =
                    format!("Cookie: {}", join_inspector_items(&items, "; ")).into_bytes();
                line.extend_from_slice(newline);
                Some(line)
            };
            if let Some(&first_index) = cookie_indices.first() {
                for &index in cookie_indices.iter().rev().skip(1) {
                    let _ = index;
                }
                for &index in cookie_indices.iter().skip(1).rev() {
                    lines.remove(index);
                }
                if let Some(line) = replacement {
                    lines[first_index] = line;
                } else {
                    lines.remove(first_index);
                }
            } else if let Some(line) = replacement {
                if let Some(last) = lines.last_mut()
                    && !last.ends_with(b"\n")
                {
                    last.extend_from_slice(newline);
                }
                lines.push(line);
            }
        }
        InspectorSection::RequestHeaders => {
            let indices = lines
                .iter()
                .enumerate()
                .skip(1)
                .filter_map(|(index, line)| inspector_header(line).map(|_| index))
                .collect::<Vec<_>>();
            match mutation {
                InspectorMutation::Add { name, value, .. } => {
                    let mut line = format!("{name}: {value}").into_bytes();
                    line.extend_from_slice(newline);
                    if let Some(last) = lines.last_mut()
                        && !last.ends_with(b"\n")
                    {
                        last.extend_from_slice(newline);
                    }
                    lines.push(line);
                }
                InspectorMutation::Update {
                    index, name, value, ..
                } => {
                    let &index = indices
                        .get(index)
                        .ok_or_else(|| "Inspector 项目索引超出范围。".to_owned())?;
                    let mut line = format!("{name}: {value}").into_bytes();
                    if lines[index].ends_with(b"\n") {
                        line.extend_from_slice(newline);
                    }
                    lines[index] = line;
                }
                InspectorMutation::Remove { index, .. } => {
                    let &index = indices
                        .get(index)
                        .ok_or_else(|| "Inspector 项目索引超出范围。".to_owned())?;
                    lines.remove(index);
                }
                InspectorMutation::Move { from, to, .. } => {
                    if from >= indices.len() || to >= indices.len() {
                        return Err("Inspector 项目索引超出范围。".into());
                    }
                    if from != to {
                        let from_line_idx = indices[from];
                        let moved = lines.remove(from_line_idx);
                        let updated_indices = lines
                            .iter()
                            .enumerate()
                            .skip(1)
                            .filter_map(|(i, line)| inspector_header(line).map(|_| i))
                            .collect::<Vec<_>>();
                        let insert_at = if to < updated_indices.len() {
                            updated_indices[to]
                        } else {
                            lines.len()
                        };
                        lines.insert(insert_at, moved);
                    }
                }
            }
        }
        InspectorSection::ResponseHeaders => unreachable!(),
    }
    let mut raw = lines.concat();
    if new_body.is_some() && separator.is_empty() {
        if !raw.ends_with(b"\n") {
            raw.extend_from_slice(newline);
        }
        raw.extend_from_slice(newline);
    } else {
        raw.extend_from_slice(separator);
    }
    raw.extend_from_slice(new_body.as_deref().unwrap_or(original_body));
    Ok(raw)
}
