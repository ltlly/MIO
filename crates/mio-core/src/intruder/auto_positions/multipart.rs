use anyhow::{Context, Result, bail, ensure};

use super::*;

struct Boundary {
    start: usize,
    after: usize,
    closing: bool,
}

fn boundary(raw: &[u8], mut at: usize, marker: &[u8]) -> Result<Boundary> {
    while at < raw.len() {
        let end = raw[at..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(raw.len(), |offset| at + offset);
        let mut content_end = end;
        if content_end > at && raw[content_end - 1] == b'\r' {
            content_end -= 1;
        }
        let line = &raw[at..content_end];
        if let Some(tail) = line.strip_prefix(marker) {
            let (tail, closing) = tail
                .strip_prefix(b"--")
                .map_or((tail, false), |tail| (tail, true));
            if tail.iter().all(|byte| matches!(byte, b' ' | b'\t')) {
                ensure!(
                    closing || end < raw.len(),
                    "Auto § multipart 起始 boundary 未完整结束"
                );
                return Ok(Boundary {
                    start: at,
                    after: (end + 1).min(raw.len()),
                    closing,
                });
            }
        }
        at = end + 1;
    }
    bail!("Auto § multipart 缺少完整结束 boundary")
}

pub(super) fn multipart(
    raw: &[u8],
    start: usize,
    value: &[u8],
    positions: &mut Vec<Position>,
    include_files: bool,
) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= 70 && value.last() != Some(&b' '),
        "Auto § multipart boundary 长度或结尾无效"
    );
    ensure!(
        value
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"'()+_,-./:=? ".contains(byte)),
        "Auto § multipart boundary 字符无效"
    );
    let mut marker = b"--".to_vec();
    marker.extend_from_slice(value);
    let mut next = boundary(raw, start, &marker)?;
    let mut count = 0;
    while !next.closing {
        count += 1;
        ensure!(
            count <= MAX_NODES,
            AutomaticParameterLimit("Auto § multipart part 数量超过 8192（MIO 资源限额）")
        );
        let mut body_start = next.after;
        let fields = headers(raw, &mut body_start)?;
        let disposition = unique_header(&fields, b"content-disposition")?
            .context("Auto § multipart part 缺少 Content-Disposition")?;
        let disposition = mime(raw, disposition.value.clone())?;
        ensure!(
            disposition.essence.eq_ignore_ascii_case(b"form-data")
                && disposition.parameter(b"name").is_some(),
            "Auto § multipart part 不是有效 form-data 字段"
        );
        next = boundary(raw, body_start, &marker)?;
        let mut body_end = next.start;
        if body_end == body_start {
            // MIME permits an empty part without an additional body line.
        } else if body_end >= body_start + 2 && &raw[body_end - 2..body_end] == b"\r\n" {
            body_end -= 2;
        } else if body_end > body_start && raw[body_end - 1] == b'\n' {
            body_end -= 1;
        } else {
            bail!("Auto § multipart 字段与 boundary 之间缺少换行");
        }
        if include_files
            || (disposition.parameter(b"filename").is_none()
                && disposition.parameter(b"filename*").is_none())
        {
            add(positions, body_start..body_end)?;
        }
    }
    Ok(())
}

impl InterceptContentType<'_> {
    pub(crate) fn is_json_or_xml(&self) -> bool {
        self.essence.eq_ignore_ascii_case(b"application/json")
            || ends_ascii_case(self.essence, b"+json")
            || self.essence.eq_ignore_ascii_case(b"application/xml")
            || self.essence.eq_ignore_ascii_case(b"text/xml")
            || ends_ascii_case(self.essence, b"+xml")
    }

    pub(crate) fn multipart_boundary(&self) -> Result<Option<&[u8]>> {
        if !self.essence.eq_ignore_ascii_case(b"multipart/form-data") {
            return Ok(None);
        }
        self.boundary
            .as_deref()
            .context("HTTP multipart 缺少 boundary")
            .map(Some)
    }
}

pub(crate) fn parse_intercept_content_type(raw: &[u8]) -> Result<InterceptContentType<'_>> {
    let parsed = mime(raw, 0..raw.len())?;
    ensure!(
        parsed.essence.iter().filter(|byte| **byte == b'/').count() == 1,
        "HTTP Content-Type 必须包含一个类型和一个子类型"
    );
    Ok(InterceptContentType {
        essence: parsed.essence,
        boundary: parsed.parameter(b"boundary").map(<[u8]>::to_vec),
    })
}

pub(crate) fn intercept_body_is_structured(
    raw: &[u8],
    check: &impl Fn() -> Result<()>,
) -> Result<bool> {
    check()?;
    ensure!(
        raw.len() <= MAX_AUTO_BYTES,
        AutomaticParameterLimit("Proxy 参数结构嗅探最多解析 4 MiB（MIO 资源限额）")
    );
    let mut content = trimmed(raw, 0..raw.len());
    if raw[content.clone()].starts_with(b"\xef\xbb\xbf") {
        content = trimmed(raw, content.start + 3..content.end);
    }
    let mut positions = Vec::new();
    let parsed = match raw.get(content.start) {
        Some(b'{' | b'[') => json_checked(raw, content, &mut positions, check),
        Some(b'<') => xml_checked(raw, content, &mut positions, check),
        _ => return Ok(false),
    };
    match parsed {
        Err(error) if error.is::<crate::http::UnpackError>() => Err(error),
        result => {
            check()?;
            match result {
                Ok(()) => Ok(true),
                Err(error) if error.is::<AutomaticParameterLimit>() => Err(error),
                Err(_) => Ok(false),
            }
        }
    }
}

pub(crate) fn intercept_multipart_parameters<'a>(
    raw: &'a [u8],
    boundary_value: &[u8],
    out: &mut Vec<(std::borrow::Cow<'a, [u8]>, &'a [u8])>,
    check: &impl Fn() -> Result<()>,
) -> Result<()> {
    check()?;
    ensure!(
        !boundary_value.is_empty()
            && boundary_value.len() <= 70
            && boundary_value.last() != Some(&b' '),
        "Auto § multipart boundary 长度或结尾无效"
    );
    ensure!(
        boundary_value
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"'()+_,-./:=? ".contains(byte)),
        "Auto § multipart boundary 字符无效"
    );
    let mut marker = b"--".to_vec();
    marker.extend_from_slice(boundary_value);
    let mut next = boundary_checked(raw, 0, &marker, check)?;
    let mut count = 0;
    while !next.closing {
        check()?;
        count += 1;
        ensure!(
            count <= MAX_NODES,
            AutomaticParameterLimit("Auto § multipart part 数量超过 8192（MIO 资源限额）")
        );
        let mut body_start = next.after;
        let fields = headers_checked(raw, &mut body_start, check)?;
        let disposition = unique_header(&fields, b"content-disposition")?
            .context("Auto § multipart part 缺少 Content-Disposition")?;
        let disposition = mime(raw, disposition.value.clone())?;
        let name = disposition
            .parameter(b"name")
            .filter(|_| disposition.essence.eq_ignore_ascii_case(b"form-data"))
            .context("Auto § multipart part 不是有效 form-data 字段")?
            .to_vec();
        next = boundary_checked(raw, body_start, &marker, check)?;
        let mut body_end = next.start;
        if body_end == body_start {
            // MIME permits an empty part without an additional body line.
        } else if body_end >= body_start + 2 && &raw[body_end - 2..body_end] == b"\r\n" {
            body_end -= 2;
        } else if body_end > body_start && raw[body_end - 1] == b'\n' {
            body_end -= 1;
        } else {
            bail!("Auto § multipart 字段与 boundary 之间缺少换行");
        }
        ensure!(
            out.len() < MAX_POSITIONS,
            AutomaticParameterLimit("Proxy 请求参数超过 256 个限额，未返回部分匹配")
        );
        out.push((std::borrow::Cow::Owned(name), &raw[body_start..body_end]));
    }
    check()?;
    Ok(())
}

fn boundary_checked(
    raw: &[u8],
    mut at: usize,
    marker: &[u8],
    check: &impl Fn() -> Result<()>,
) -> Result<Boundary> {
    while at < raw.len() {
        check()?;
        let end = raw[at..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(raw.len(), |offset| at + offset);
        let mut content_end = end;
        if content_end > at && raw[content_end - 1] == b'\r' {
            content_end -= 1;
        }
        let line = &raw[at..content_end];
        if let Some(tail) = line.strip_prefix(marker) {
            let (tail, closing) = tail
                .strip_prefix(b"--")
                .map_or((tail, false), |tail| (tail, true));
            if tail.iter().all(|byte| matches!(byte, b' ' | b'\t')) {
                ensure!(
                    closing || end < raw.len(),
                    "Auto § multipart 起始 boundary 未完整结束"
                );
                return Ok(Boundary {
                    start: at,
                    after: (end + 1).min(raw.len()),
                    closing,
                });
            }
        }
        at = end + 1;
    }
    bail!("Auto § multipart 缺少完整结束 boundary")
}
