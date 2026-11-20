use super::*;

pub(crate) fn token(value: &[u8]) -> bool {
    !value.is_empty()
        && value
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(c))
}

pub(super) fn headers(lines: impl Iterator<Item = Vec<u8>>) -> Result<Vec<Header>> {
    let mut result = Vec::new();
    for line in lines {
        ensure!(result.len() < 1024, "header 数量超过 1024");
        let colon = line
            .iter()
            .position(|c| *c == b':')
            .context("header 缺少冒号")?;
        ensure!(token(&line[..colon]), "header 名称无效");
        let value = line[colon + 1..].trim_ascii().to_vec();
        ensure!(valid_header_value_bytes(&value), "header 含控制字符");
        result.push(Header {
            name: String::from_utf8(line[..colon].to_vec())?,
            value,
        });
    }
    Ok(result)
}

pub(crate) fn head(raw: &[u8]) -> Result<(Vec<u8>, Vec<Header>, &[u8])> {
    let (end, delimiter) = raw
        .windows(4)
        .position(|s| s == b"\r\n\r\n")
        .map(|p| (p, 4))
        .into_iter()
        .chain(raw.windows(2).position(|s| s == b"\n\n").map(|p| (p, 2)))
        .min_by_key(|(position, _)| *position)
        .context("报文需要空行分隔 header 与正文")?;
    ensure!(end <= MAX_HEAD, "header 超过 128 KiB");
    let mut lines = raw[..end]
        .split(|c| *c == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line).to_vec());
    let first = lines.next().context("报文没有起始行")?;
    Ok((first, headers(lines)?, &raw[end + delimiter..]))
}

pub(crate) fn is_chunked(fields: &[Header]) -> bool {
    fields.iter().any(|h| {
        h.name.eq_ignore_ascii_case("transfer-encoding")
            && h.value
                .split(|c| *c == b',')
                .any(|part| part.trim_ascii().eq_ignore_ascii_case(b"chunked"))
    })
}

pub(crate) fn body(fields: &[Header], mut raw: &[u8]) -> Result<(Vec<u8>, Vec<Header>)> {
    for field in fields
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case("transfer-encoding"))
    {
        ensure!(
            field
                .value
                .split(|c| *c == b',')
                .any(|part| part.trim_ascii().eq_ignore_ascii_case(b"chunked")),
            "当前发送路径不支持此 Transfer-Encoding"
        );
    }
    if !is_chunked(fields) {
        ensure!(raw.len() <= MAX_BODY, "可编辑正文超过 16 MiB 限额");
        return Ok((raw.to_vec(), Vec::new()));
    }
    let mut decoded = Vec::new();
    loop {
        let lf_pos = raw
            .iter()
            .position(|b| *b == b'\n')
            .context("chunk 长度行未结束")?;
        let (line_end, ending_len) = if lf_pos > 0 && raw[lf_pos - 1] == b'\r' {
            (lf_pos - 1, 2)
        } else {
            (lf_pos, 1)
        };
        ensure!(line_end <= 1024, "chunk 长度行过长");
        let size_text = raw[..line_end]
            .split(|c| *c == b';')
            .next()
            .unwrap_or_default();
        ensure!(
            !size_text.is_empty() && size_text.iter().all(u8::is_ascii_hexdigit),
            "chunk 长度无效"
        );
        let size = usize::from_str_radix(std::str::from_utf8(size_text)?, 16)?;
        raw = &raw[line_end + ending_len..];
        if size == 0 {
            ensure!(raw.len() <= MAX_HEAD, "trailers 过长");
            if raw == b"\r\n" || raw == b"\n" {
                return Ok((decoded, Vec::new()));
            }
            let (end, term_len) = if let Some(pos) = raw.windows(4).position(|s| s == b"\r\n\r\n") {
                (pos, 4)
            } else if let Some(pos) = raw.windows(2).position(|s| s == b"\n\n") {
                (pos, 2)
            } else {
                bail!("trailers 未结束");
            };
            ensure!(end + term_len == raw.len(), "chunked 正文后有额外数据");
            let trailers = headers(
                raw[..end]
                    .split(|c| *c == b'\n')
                    .map(|line| line.strip_suffix(b"\r").unwrap_or(line).to_vec()),
            )?;
            ensure!(
                !trailers
                    .iter()
                    .any(|h| ["content-length", "transfer-encoding", "host"]
                        .iter()
                        .any(|name| h.name.eq_ignore_ascii_case(name))),
                "trailers 含禁止的 framing header"
            );
            return Ok((decoded, trailers));
        }
        ensure!(
            size <= MAX_BODY.saturating_sub(decoded.len()),
            "可编辑正文超过 16 MiB 限额"
        );
        let chunk_ending_len = if raw.len() >= size + 2 && &raw[size..size + 2] == b"\r\n" {
            2
        } else if raw.len() > size && raw[size] == b'\n' {
            1
        } else {
            bail!("chunk 正文长度不匹配");
        };
        decoded.extend_from_slice(&raw[..size]);
        raw = &raw[size + chunk_ending_len..];
    }
}
