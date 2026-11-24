use anyhow::{Context, Result, bail, ensure};

use super::*;

struct Json<'a, 'out> {
    raw: &'a [u8],
    cursor: usize,
    end: usize,
    nodes: usize,
    positions: &'out mut Vec<Position>,
    check: &'out dyn Fn() -> Result<()>,
}

pub(super) fn json(raw: &[u8], range: Range<usize>, positions: &mut Vec<Position>) -> Result<()> {
    json_checked(raw, range, positions, &|| Ok(()))
}

pub(super) fn json_checked(
    raw: &[u8],
    range: Range<usize>,
    positions: &mut Vec<Position>,
    check: &impl Fn() -> Result<()>,
) -> Result<()> {
    check()?;
    std::str::from_utf8(&raw[range.clone()]).context("Auto § JSON 必须是 UTF-8")?;
    let cursor = range.start
        + if raw[range.clone()].starts_with(b"\xef\xbb\xbf") {
            3
        } else {
            0
        };
    let mut parser = Json {
        raw,
        cursor,
        end: range.end,
        nodes: 0,
        positions,
        check,
    };
    parser.value(0)?;
    parser.space();
    ensure!(parser.cursor == parser.end, "Auto § JSON 结束后有额外内容");
    check()?;
    Ok(())
}

impl Json<'_, '_> {
    fn space(&mut self) {
        while self.cursor < self.end
            && matches!(self.raw[self.cursor], b' ' | b'\t' | b'\r' | b'\n')
        {
            self.cursor += 1;
        }
    }

    fn consume(&mut self, byte: u8) -> bool {
        self.space();
        if self.cursor < self.end && self.raw[self.cursor] == byte {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<()> {
        (self.check)()?;
        self.nodes += 1;
        ensure!(
            depth <= MAX_DEPTH && self.nodes <= MAX_NODES,
            AutomaticParameterLimit("Auto § JSON 超过深度 64 或节点 8192 限额（MIO）")
        );
        self.space();
        ensure!(self.cursor < self.end, "Auto § JSON 值未完整结束");
        match self.raw[self.cursor] {
            b'{' => {
                self.cursor += 1;
                if self.consume(b'}') {
                    return Ok(());
                }
                loop {
                    self.space();
                    self.string()?;
                    ensure!(self.consume(b':'), "Auto § JSON 对象缺少冒号");
                    self.value(depth + 1)?;
                    if self.consume(b'}') {
                        break;
                    }
                    ensure!(self.consume(b','), "Auto § JSON 对象缺少逗号");
                }
            }
            b'[' => {
                self.cursor += 1;
                if self.consume(b']') {
                    return Ok(());
                }
                loop {
                    self.value(depth + 1)?;
                    if self.consume(b']') {
                        break;
                    }
                    ensure!(self.consume(b','), "Auto § JSON 数组缺少逗号");
                }
            }
            b'"' => {
                let range = self.string()?;
                add(self.positions, range)?;
            }
            _ => {
                let start = self.cursor;
                while self.cursor < self.end
                    && !matches!(
                        self.raw[self.cursor],
                        b',' | b']' | b'}' | b' ' | b'\t' | b'\r' | b'\n'
                    )
                {
                    self.cursor += 1;
                    if self.cursor.is_multiple_of(8 * 1024) {
                        (self.check)()?;
                    }
                }
                let scalar = &self.raw[start..self.cursor];
                ensure!(
                    matches!(scalar, b"true" | b"false" | b"null") || json_number(scalar),
                    "Auto § JSON 标量无效"
                );
                add(self.positions, start..self.cursor)?;
            }
        }
        Ok(())
    }

    fn string(&mut self) -> Result<Range<usize>> {
        (self.check)()?;
        ensure!(
            self.cursor < self.end && self.raw[self.cursor] == b'"',
            "Auto § JSON 需要双引号字符串"
        );
        let start = self.cursor;
        let mut next_check = start.saturating_add(8 * 1024);
        self.cursor += 1;
        while self.cursor < self.end {
            match self.raw[self.cursor] {
                b'"' => {
                    self.cursor += 1;
                    // Validate escapes/surrogates, but never serialize or keep a decoded key map.
                    serde_json::from_slice::<String>(&self.raw[start..self.cursor])
                        .context("Auto § JSON 字符串转义无效")?;
                    (self.check)()?;
                    return Ok(start + 1..self.cursor - 1);
                }
                b'\\' => {
                    self.cursor += 1;
                    ensure!(self.cursor < self.end, "Auto § JSON 字符串转义未结束");
                }
                _ => {}
            }
            self.cursor += 1;
            if self.cursor >= next_check {
                (self.check)()?;
                next_check = self.cursor.saturating_add(8 * 1024);
            }
        }
        bail!("Auto § JSON 字符串未闭合")
    }
}

fn json_number(value: &[u8]) -> bool {
    let mut at = usize::from(value.first() == Some(&b'-'));
    match value.get(at) {
        Some(b'0') => at += 1,
        Some(b'1'..=b'9') => {
            at += 1;
            while value.get(at).is_some_and(u8::is_ascii_digit) {
                at += 1;
            }
        }
        _ => return false,
    }
    if value.get(at) == Some(&b'.') {
        at += 1;
        let start = at;
        while value.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        if start == at {
            return false;
        }
    }
    if matches!(value.get(at), Some(b'e' | b'E')) {
        at += 1;
        if matches!(value.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        let start = at;
        while value.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        if start == at {
            return false;
        }
    }
    at == value.len()
}

fn find(raw: &[u8], range: Range<usize>, needle: &[u8]) -> Result<usize> {
    raw[range.clone()]
        .windows(needle.len())
        .position(|part| part == needle)
        .map(|offset| range.start + offset)
        .context("Auto § 结构未完整闭合")
}

pub(super) fn xml(raw: &[u8], range: Range<usize>, positions: &mut Vec<Position>) -> Result<()> {
    xml_checked(raw, range, positions, &|| Ok(()))
}

pub(super) fn xml_checked(
    raw: &[u8],
    range: Range<usize>,
    positions: &mut Vec<Position>,
    check: &impl Fn() -> Result<()>,
) -> Result<()> {
    check()?;
    let text = std::str::from_utf8(&raw[range.clone()]).context("Auto § XML 必须是 UTF-8")?;
    let mut at = range.start
        + if raw[range.clone()].starts_with(b"\xef\xbb\xbf") {
            3
        } else {
            0
        };
    let mut depth = 0_usize;
    let mut nodes = 0;
    // roxmltree merges adjacent text/CDATA and compresses attribute offset metadata.
    // Raw lexical ranges preserve every original fragment, even long attribute spacing.
    while at < range.end {
        check()?;
        nodes += 1;
        ensure!(
            nodes <= MAX_NODES,
            AutomaticParameterLimit("Auto § XML 超过 8192 个节点（MIO 资源限额）")
        );
        if raw[at..range.end].starts_with(b"<!--") {
            at = find(raw, at + 4..range.end, b"-->")? + 3;
        } else if raw[at..range.end].starts_with(b"<![CDATA[") {
            let end = find(raw, at + 9..range.end, b"]]>")?;
            add(positions, at + 9..end)?;
            at = end + 3;
        } else if raw[at..range.end].starts_with(b"<?") {
            at = find(raw, at + 2..range.end, b"?>")? + 2;
        } else if raw[at..range.end].starts_with(b"<!") {
            bail!("Auto § 不解析 DTD 或实体声明，未应用标记");
        } else if raw[at..range.end].starts_with(b"</") {
            depth = depth
                .checked_sub(1)
                .context("Auto § XML 结束标签没有起始标签")?;
            at = find(raw, at + 2..range.end, b">")? + 1;
        } else if raw[at] == b'<' {
            depth += 1;
            ensure!(
                depth <= MAX_DEPTH,
                AutomaticParameterLimit("Auto § XML 深度超过 64（MIO 资源限额）")
            );
            at += 1;
            while at < range.end
                && !raw[at].is_ascii_whitespace()
                && !matches!(raw[at], b'>' | b'/')
            {
                at += 1;
            }
            loop {
                check()?;
                while at < range.end && raw[at].is_ascii_whitespace() {
                    at += 1;
                }
                ensure!(at < range.end, "Auto § XML 起始标签未结束");
                if raw[at..range.end].starts_with(b"/>") {
                    depth -= 1;
                    at += 2;
                    break;
                }
                if raw[at] == b'>' {
                    at += 1;
                    break;
                }
                while at < range.end
                    && !raw[at].is_ascii_whitespace()
                    && !matches!(raw[at], b'=' | b'>' | b'/')
                {
                    at += 1;
                }
                while at < range.end && raw[at].is_ascii_whitespace() {
                    at += 1;
                }
                ensure!(at < range.end && raw[at] == b'=', "Auto § XML 属性缺少等号");
                at += 1;
                while at < range.end && raw[at].is_ascii_whitespace() {
                    at += 1;
                }
                ensure!(
                    at < range.end && matches!(raw[at], b'\'' | b'"'),
                    "Auto § XML 属性需要引号"
                );
                let quote = raw[at];
                let start = at + 1;
                at = raw[start..range.end]
                    .iter()
                    .position(|byte| *byte == quote)
                    .context("Auto § XML 属性引号未闭合")?
                    + start;
                add(positions, start..at)?;
                at += 1;
            }
        } else {
            let end = raw[at..range.end]
                .iter()
                .position(|byte| *byte == b'<')
                .map_or(range.end, |offset| at + offset);
            if !raw[at..end].trim_ascii().is_empty() {
                add(positions, at..end)?;
            }
            at = end;
        }
    }
    ensure!(depth == 0, "Auto § XML 标签未闭合");
    check()?;
    roxmltree::Document::parse_with_options(
        text.strip_prefix('\u{feff}').unwrap_or(text),
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: MAX_NODES as u32,
            entity_resolver: None,
        },
    )
    .map_err(|error| {
        if matches!(error, roxmltree::Error::NodesLimitReached) {
            anyhow::Error::new(error).context(AutomaticParameterLimit("Auto § XML 格式无效"))
        } else {
            anyhow::Error::new(error).context("Auto § XML 格式无效")
        }
    })?;
    check()?;
    Ok(())
}
