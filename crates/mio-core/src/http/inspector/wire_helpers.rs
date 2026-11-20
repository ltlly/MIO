use super::*;

pub fn detect_request_line_version(raw: &[u8]) -> Option<HttpVersion> {
    let first = raw
        .split_inclusive(|byte| *byte == b'\n')
        .next()
        .unwrap_or(raw);
    let parts = inspector_request_parts(first)?;
    let line = inspector_line_content(first);
    match &line[parts[2].clone()] {
        b"HTTP/1.0" => Some(HttpVersion::Http10),
        b"HTTP/1.1" => Some(HttpVersion::Http11),
        b"HTTP/2" | b"HTTP/2.0" => Some(HttpVersion::Http2),
        _ => None,
    }
}

pub fn rewrite_request_line_version(raw: &[u8], version: HttpVersion) -> Vec<u8> {
    let Some(first) = raw.split_inclusive(|byte| *byte == b'\n').next() else {
        return raw.to_vec();
    };
    let Some(parts) = inspector_request_parts(first) else {
        return raw.to_vec();
    };
    let mut out = Vec::with_capacity(raw.len() + 4);
    out.extend_from_slice(&first[..parts[2].start]);
    out.extend_from_slice(version.as_str().as_bytes());
    out.extend_from_slice(&first[parts[2].end..]);
    out.extend_from_slice(&raw[first.len()..]);
    out
}

pub fn apply_inspector_response_mutation(
    response_raw: &[u8],
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
            let trimmed = name.trim();
            if trimmed.is_empty()
                || !token(trimmed.as_bytes())
                || name.contains(['\r', '\n', '\0'])
                || value.contains(['\r', '\n', '\0'])
            {
                return Err("Inspector 响应标头名称或值无效。".into());
            }
            *section
        }
        InspectorMutation::Remove { section, .. } => *section,
        InspectorMutation::Move { section, .. } => *section,
    };
    if section != InspectorSection::ResponseHeaders {
        return Err("响应拦截仅支持通过 Inspector 修改响应标头。".into());
    }
    let (original_lines, separator, original_body) = inspector_message_parts(response_raw);
    let first = original_lines
        .first()
        .ok_or_else(|| "响应缺少有效的 HTTP 状态行。".to_owned())?;
    if !inspector_line_content(first).starts_with(b"HTTP/") {
        return Err("响应缺少有效的 HTTP 状态行。".into());
    }
    let newline: &[u8] = if first.ends_with(b"\r\n") {
        b"\r\n"
    } else {
        b"\n"
    };
    let mut lines = original_lines
        .iter()
        .map(|line| line.to_vec())
        .collect::<Vec<_>>();
    let indices = lines
        .iter()
        .enumerate()
        .skip(1)
        .filter_map(|(index, line)| inspector_header(line).map(|_| index))
        .collect::<Vec<_>>();
    match mutation {
        InspectorMutation::Add { name, value, .. } => {
            let mut line = format!("{}: {value}", name.trim()).into_bytes();
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
            let mut line = format!("{}: {value}", name.trim()).into_bytes();
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
    let mut raw = lines.concat();
    if separator.is_empty() && !original_body.is_empty() {
        if !raw.ends_with(b"\n") {
            raw.extend_from_slice(newline);
        }
        raw.extend_from_slice(newline);
    } else {
        raw.extend_from_slice(separator);
    }
    raw.extend_from_slice(original_body);
    Ok(raw)
}
