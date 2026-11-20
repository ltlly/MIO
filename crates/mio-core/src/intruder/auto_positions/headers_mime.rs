use anyhow::{Context, Result, ensure};

use super::*;

pub(super) fn line(raw: &[u8], cursor: &mut usize) -> Result<Range<usize>> {
    let start = *cursor;
    let length = raw[start..]
        .iter()
        .position(|byte| *byte == b'\n')
        .context("Auto § header 行未结束")?;
    *cursor = start + length + 1;
    let end = start + length - usize::from(length > 0 && raw[start + length - 1] == b'\r');
    ensure!(
        !raw[start..end].contains(&b'\r'),
        "Auto § header 中有孤立 CR"
    );
    Ok(start..end)
}

pub(super) fn headers<'a>(raw: &'a [u8], cursor: &mut usize) -> Result<Vec<HeaderRange<'a>>> {
    headers_checked(raw, cursor, &|| Ok(()))
}

pub(super) fn headers_checked<'a>(
    raw: &'a [u8],
    cursor: &mut usize,
    check: &impl Fn() -> Result<()>,
) -> Result<Vec<HeaderRange<'a>>> {
    let start = *cursor;
    let mut fields = Vec::new();
    loop {
        check()?;
        let range = line(raw, cursor)?;
        ensure!(
            *cursor - start <= crate::http::MAX_HEAD,
            AutomaticParameterLimit("Auto § header 超过 128 KiB")
        );
        if range.is_empty() {
            return Ok(fields);
        }
        ensure!(
            fields.len() < 1024,
            AutomaticParameterLimit("Auto § header 数量超过 1024")
        );
        let colon = raw[range.clone()]
            .iter()
            .position(|byte| *byte == b':')
            .context("Auto § header 缺少冒号")?
            + range.start;
        ensure!(
            crate::http::token(&raw[range.start..colon]),
            "Auto § header 名称无效"
        );
        ensure!(
            raw[colon + 1..range.end]
                .iter()
                .all(|byte| (*byte >= 32 && *byte != 127) || *byte == b'\t'),
            "Auto § header 含控制字符"
        );
        fields.push(HeaderRange {
            name: &raw[range.start..colon],
            value: trimmed(raw, colon + 1..range.end),
        });
        check()?;
    }
}

pub(super) fn unique_header<'a, 'raw>(
    fields: &'a [HeaderRange<'raw>],
    name: &[u8],
) -> Result<Option<&'a HeaderRange<'raw>>> {
    let mut found = fields
        .iter()
        .filter(|field| field.name.eq_ignore_ascii_case(name));
    let value = found.next();
    ensure!(
        found.next().is_none(),
        "Auto § 遇到重复的内容类型/编码 header，未猜测其含义"
    );
    Ok(value)
}

pub(super) fn ends_ascii_case(value: &[u8], suffix: &[u8]) -> bool {
    value.len() >= suffix.len() && value[value.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

pub(super) fn pairs(raw: &[u8], range: Range<usize>, positions: &mut Vec<Position>) -> Result<()> {
    let mut start = range.start;
    for pair in raw[range.clone()].split(|byte| *byte == b'&') {
        let end = start + pair.len();
        if !pair.is_empty() {
            let mut at = 0;
            while at < pair.len() {
                if pair[at] == b'%' {
                    ensure!(
                        at + 2 < pair.len()
                            && pair[at + 1..at + 3].iter().all(u8::is_ascii_hexdigit),
                        "Auto § form/query 含无效 percent 编码"
                    );
                    at += 2;
                }
                at += 1;
            }
            let value = pair
                .iter()
                .position(|byte| *byte == b'=')
                .map_or(end, |equal| start + equal + 1);
            add(positions, value..end)?;
        }
        start = end + 1;
    }
    Ok(())
}

pub(super) fn cookies(
    raw: &[u8],
    range: Range<usize>,
    positions: &mut Vec<Position>,
) -> Result<()> {
    let mut start = range.start;
    let mut quoted = false;
    for at in range.start..=range.end {
        if at < range.end && raw[at] == b'"' {
            quoted = !quoted;
        }
        if at == range.end || (raw[at] == b';' && !quoted) {
            ensure!(!quoted, "Auto § Cookie 引号未闭合");
            let pair = trimmed(raw, start..at);
            if !pair.is_empty() {
                let equal = raw[pair.clone()]
                    .iter()
                    .position(|byte| *byte == b'=')
                    .context("Auto § Cookie 缺少等号")?
                    + pair.start;
                ensure!(
                    crate::http::token(&raw[trimmed(raw, pair.start..equal)]),
                    "Auto § Cookie 名称无效"
                );
                let mut value = trimmed(raw, equal + 1..pair.end);
                if raw.get(value.start) == Some(&b'"') {
                    ensure!(
                        value.len() >= 2 && raw[value.end - 1] == b'"',
                        "Auto § Cookie 引号不匹配"
                    );
                    value.start += 1;
                    value.end -= 1;
                }
                ensure!(
                    !raw[value.clone()].contains(&b'"'),
                    "Auto § Cookie 值含无效引号"
                );
                add(positions, value)?;
            }
            start = at + 1;
        }
    }
    Ok(())
}

impl Mime<'_> {
    pub(super) fn parameter(&self, name: &[u8]) -> Option<&[u8]> {
        self.parameters
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_slice())
    }
}

pub(super) fn mime(raw: &[u8], range: Range<usize>) -> Result<Mime<'_>> {
    let split = raw[range.clone()]
        .iter()
        .position(|byte| *byte == b';')
        .map_or(range.end, |at| range.start + at);
    let essence = &raw[trimmed(raw, range.start..split)];
    ensure!(
        !essence.is_empty() && essence.split(|byte| *byte == b'/').all(crate::http::token),
        "Auto § MIME 类型无效"
    );
    let mut parameters: Vec<(&[u8], Vec<u8>)> = Vec::new();
    let mut at = split;
    while at < range.end {
        ensure!(raw[at] == b';', "Auto § MIME 参数之间缺少分号");
        at += 1;
        while at < range.end && raw[at].is_ascii_whitespace() {
            at += 1;
        }
        if at == range.end {
            break;
        }
        ensure!(
            parameters.len() < 32,
            AutomaticParameterLimit("Auto § MIME 参数超过 32（MIO 资源限额）")
        );
        let start = at;
        while at < range.end && raw[at] != b'=' && raw[at] != b';' {
            at += 1;
        }
        ensure!(
            at < range.end && raw[at] == b'=',
            "Auto § MIME 参数缺少等号"
        );
        let name = &raw[trimmed(raw, start..at)];
        ensure!(crate::http::token(name), "Auto § MIME 参数名无效");
        ensure!(
            !parameters
                .iter()
                .any(|(key, _)| key.eq_ignore_ascii_case(name)),
            "Auto § MIME 参数重复"
        );
        at += 1;
        while at < range.end && raw[at].is_ascii_whitespace() {
            at += 1;
        }
        let mut value = Vec::new();
        if raw.get(at) == Some(&b'"') {
            at += 1;
            while at < range.end && raw[at] != b'"' {
                if raw[at] == b'\\' {
                    at += 1;
                    ensure!(at < range.end, "Auto § MIME 引号转义未结束");
                }
                value.push(raw[at]);
                at += 1;
            }
            ensure!(at < range.end && raw[at] == b'"', "Auto § MIME 引号未闭合");
            at += 1;
            while at < range.end && raw[at].is_ascii_whitespace() {
                at += 1;
            }
        } else {
            let start = at;
            while at < range.end && raw[at] != b';' {
                at += 1;
            }
            value.extend_from_slice(&raw[trimmed(raw, start..at)]);
            ensure!(crate::http::token(&value), "Auto § MIME 参数值无效");
        }
        parameters.push((name, value));
    }
    Ok(Mime {
        essence,
        parameters,
    })
}

pub(super) fn utf8_charset(mime: &Mime<'_>) -> Result<()> {
    if let Some(charset) = mime.parameter(b"charset") {
        ensure!(
            [b"utf-8".as_slice(), b"utf8", b"us-ascii"]
                .iter()
                .any(|supported| charset.eq_ignore_ascii_case(supported)),
            "Auto § JSON/XML 只支持 UTF-8/ASCII；未转换原始字节"
        );
    }
    Ok(())
}
