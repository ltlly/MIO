use anyhow::{Context, Result, bail, ensure};

use super::*;

/// Discovers value ranges without decoding/re-encoding the request or performing I/O.
/// Header selections retain complete query/Cookie values; body selections parse independently.
/// Full-request Auto excludes file parts until their native External default is verified.
pub fn automatic_positions(
    raw: &[u8],
    selection: Option<Range<usize>>,
    placement: AutomaticPlacement,
) -> Result<Vec<Position>> {
    ensure!(
        raw.len() <= MAX_AUTO_BYTES,
        AutomaticParameterLimit("Auto § 最多解析 4 MiB（MIO 资源限额）")
    );
    let mut positions = Vec::new();
    if let Some(range) = selection {
        ensure!(
            range.start < range.end && range.end <= raw.len(),
            "Auto § 选区无效或为空"
        );
        let first = raw.split(|byte| *byte == b'\n').next().unwrap_or_default();
        let mut words = first
            .split(u8::is_ascii_whitespace)
            .filter(|word| !word.is_empty());
        let request = words.next().is_some_and(crate::http::token)
            && words.next().is_some()
            && words
                .next()
                .is_some_and(|version| version.starts_with(b"HTTP/"))
            && words.next().is_none();
        if request {
            detect_request(raw, Some(&range), &mut positions)?;
            positions.retain(|position| range.start <= position.start && position.end <= range.end);
        } else {
            detect_selected(raw, range, &mut positions)?;
        }
    } else {
        detect_request(raw, None, &mut positions)?;
    }
    positions.sort_unstable_by_key(|position| (position.start, position.end));
    positions.dedup();
    ensure!(
        positions
            .windows(2)
            .all(|pair| pair[0].end <= pair[1].start),
        "Auto § 检测到重叠位置，未应用标记"
    );
    if placement == AutomaticPlacement::Append {
        for position in &mut positions {
            position.start = position.end;
        }
        positions.dedup();
    }
    Ok(positions)
}

/// History counts query/body parameters, not cookies; body bytes are transfer-decoded only.
pub(crate) fn has_http_parameters(raw: &[u8]) -> Result<bool> {
    ensure!(
        raw.len() <= MAX_AUTO_BYTES,
        AutomaticParameterLimit("Auto § 最多解析 4 MiB（MIO 资源限额）")
    );
    let mut cursor = 0;
    let first = line(raw, &mut cursor)?;
    ensure!(
        first.len() <= crate::http::MAX_HEAD,
        AutomaticParameterLimit("Auto § 请求行过长")
    );
    let words = raw[first]
        .split(u8::is_ascii_whitespace)
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    ensure!(
        words.len() == 3 && crate::http::token(words[0]),
        "HTTP 参数筛选需要完整 HTTP 请求行"
    );
    ensure!(
        [b"HTTP/1.0".as_slice(), b"HTTP/1.1", b"HTTP/2", b"HTTP/2.0"].contains(&words[2]),
        "HTTP 参数筛选不支持此 HTTP 版本"
    );
    let target = words[1]
        .split(|byte| *byte == b'#')
        .next()
        .unwrap_or_default();
    if let Some(query) = target.iter().position(|byte| *byte == b'?')
        && history_pairs(&target[query + 1..])?
    {
        return Ok(true);
    }
    let fields = headers(raw, &mut cursor)?;
    if cursor == raw.len() {
        return Ok(false);
    }
    let content_type = unique_header(&fields, b"content-type")?
        .map(|field| mime(raw, field.value.clone()))
        .transpose()?;
    if let Some(content_type) = &content_type {
        ensure!(
            content_type
                .essence
                .iter()
                .filter(|byte| **byte == b'/')
                .count()
                == 1,
            "HTTP 参数筛选 Content-Type 必须包含一个类型和一个子类型"
        );
        if content_type
            .essence
            .eq_ignore_ascii_case(b"application/x-www-form-urlencoded")
        {
            return history_pairs(&raw[cursor..]);
        }
    }
    let body = cursor..raw.len();
    let mut content = trimmed(raw, body.clone());
    if raw[content.clone()].starts_with(b"\xef\xbb\xbf") {
        content = trimmed(raw, content.start + 3..content.end);
    }
    let mut positions = Vec::new();
    if let Some(content_type) = &content_type
        && (content_type
            .essence
            .eq_ignore_ascii_case(b"application/json")
            || ends_ascii_case(content_type.essence, b"+json"))
    {
        utf8_charset(content_type)?;
        if !matches!(raw.get(content.start), Some(b'{' | b'[')) {
            return Ok(false);
        }
        json(raw, body, &mut positions)?;
    } else if let Some(content_type) = &content_type
        && (content_type
            .essence
            .eq_ignore_ascii_case(b"application/xml")
            || content_type.essence.eq_ignore_ascii_case(b"text/xml")
            || ends_ascii_case(content_type.essence, b"+xml"))
    {
        utf8_charset(content_type)?;
        xml(raw, body, &mut positions)?;
    } else if let Some(content_type) = &content_type
        && content_type
            .essence
            .eq_ignore_ascii_case(b"multipart/form-data")
    {
        let boundary = content_type
            .parameter(b"boundary")
            .context("HTTP multipart 缺少 boundary")?;
        multipart(raw, body.start, boundary, &mut positions, true)?;
    } else {
        match raw.get(content.start) {
            Some(b'{' | b'[') => json(raw, body, &mut positions)?,
            Some(b'<') => xml(raw, body, &mut positions)?,
            _ => return history_pairs(&raw[body]),
        }
    }
    Ok(!positions.is_empty())
}

fn history_pairs(raw: &[u8]) -> Result<bool> {
    let mut count = 0;
    for pair in raw.split(|byte| *byte == b'&') {
        if !pair.is_empty() {
            count += 1;
            ensure!(
                count <= MAX_POSITIONS,
                AutomaticParameterLimit("Auto § 超过 256 个位置（MIO 资源限额）")
            );
        }
    }
    Ok(count != 0)
}

pub(super) fn add(positions: &mut Vec<Position>, range: Range<usize>) -> Result<()> {
    ensure!(
        positions.len() < MAX_POSITIONS,
        AutomaticParameterLimit("Auto § 超过 256 个位置（MIO 资源限额）")
    );
    positions.push(Position {
        start: range.start,
        end: range.end,
    });
    Ok(())
}

pub(super) fn trimmed(raw: &[u8], mut range: Range<usize>) -> Range<usize> {
    while range.start < range.end && raw[range.start].is_ascii_whitespace() {
        range.start += 1;
    }
    while range.end > range.start && raw[range.end - 1].is_ascii_whitespace() {
        range.end -= 1;
    }
    range
}

fn detect_selected(raw: &[u8], range: Range<usize>, positions: &mut Vec<Position>) -> Result<()> {
    let mut content = trimmed(raw, range.clone());
    if raw[content.clone()].starts_with(b"\xef\xbb\xbf") {
        content = trimmed(raw, content.start + 3..content.end);
    }
    ensure!(!content.is_empty(), "Auto § 选区仅含空白，未应用标记");
    match raw.get(content.start) {
        Some(b'{' | b'[' | b'"') => json(raw, range, positions),
        Some(b'<') => xml(raw, range, positions),
        _ if raw[range.clone()].contains(&b'=') => pairs(raw, range, positions),
        Some(b'-' | b'0'..=b'9' | b't' | b'f' | b'n') => json(raw, range, positions),
        _ => bail!("Auto § 选区不是可识别的 JSON、XML 或 form 参数，未应用标记"),
    }
}

fn detect_request(
    raw: &[u8],
    selection: Option<&Range<usize>>,
    positions: &mut Vec<Position>,
) -> Result<()> {
    let mut cursor = 0;
    let first = line(raw, &mut cursor)?;
    ensure!(
        first.len() <= crate::http::MAX_HEAD,
        AutomaticParameterLimit("Auto § 请求行过长")
    );
    let mut words = Vec::new();
    let mut at = first.start;
    while at < first.end {
        if raw[at].is_ascii_whitespace() {
            at += 1;
            continue;
        }
        let start = at;
        while at < first.end && !raw[at].is_ascii_whitespace() {
            at += 1;
        }
        words.push(start..at);
    }
    ensure!(
        words.len() == 3 && crate::http::token(&raw[words[0].clone()]),
        "Auto § 需要完整 HTTP 请求行"
    );
    ensure!(
        [b"HTTP/1.0".as_slice(), b"HTTP/1.1", b"HTTP/2", b"HTTP/2.0"]
            .contains(&&raw[words[2].clone()]),
        "Auto § 不支持此 HTTP 版本"
    );
    let target = words[1].clone();
    ensure!(
        !raw[target.clone()].contains(&b'#'),
        "Auto § 请求目标包含 fragment，未应用标记"
    );
    if let Some(query) = raw[target.clone()].iter().position(|byte| *byte == b'?') {
        let query = target.start + query + 1..target.end;
        if selection.is_none_or(|range| range.start <= query.end && query.start <= range.end) {
            pairs(raw, query, positions)?;
        }
    }
    let fields = headers(raw, &mut cursor)?;
    for field in &fields {
        if field.name.eq_ignore_ascii_case(b"cookie")
            && selection.is_none_or(|range| {
                range.start <= field.value.end && field.value.start <= range.end
            })
        {
            cookies(raw, field.value.clone(), positions)?;
        }
    }
    if cursor == raw.len() || selection.is_some_and(|range| range.end <= cursor) {
        return Ok(());
    }
    if let Some(range) = selection.filter(|range| range.start >= cursor) {
        return detect_selected(raw, range.clone(), positions);
    }
    for name in [b"content-encoding".as_slice(), b"transfer-encoding"] {
        if let Some(field) = unique_header(&fields, name)? {
            ensure!(
                raw[field.value.clone()].eq_ignore_ascii_case(b"identity"),
                "Auto § 不解码压缩或分块正文；请选中可识别的明文片段"
            );
        }
    }
    let Some(content_type) = unique_header(&fields, b"content-type")? else {
        return detect_selected(raw, cursor..raw.len(), positions);
    };
    let mime = mime(raw, content_type.value.clone())?;
    ensure!(
        mime.essence.iter().filter(|byte| **byte == b'/').count() == 1,
        "Auto § Content-Type 必须包含一个类型和一个子类型"
    );
    let body = cursor..raw.len();
    if mime
        .essence
        .eq_ignore_ascii_case(b"application/x-www-form-urlencoded")
    {
        pairs(raw, body, positions)
    } else if mime.essence.eq_ignore_ascii_case(b"application/json")
        || ends_ascii_case(mime.essence, b"+json")
    {
        utf8_charset(&mime)?;
        json(raw, body, positions)
    } else if mime.essence.eq_ignore_ascii_case(b"application/xml")
        || mime.essence.eq_ignore_ascii_case(b"text/xml")
        || ends_ascii_case(mime.essence, b"+xml")
    {
        utf8_charset(&mime)?;
        xml(raw, body, positions)
    } else if mime.essence.eq_ignore_ascii_case(b"multipart/form-data") {
        let boundary = mime
            .parameter(b"boundary")
            .context("Auto § multipart 缺少 boundary")?;
        multipart(raw, body.start, boundary, positions, false)
    } else {
        bail!("Auto § 不支持此正文 Content-Type；请选中 JSON、XML 或 form 片段，原标记未更改")
    }
}
