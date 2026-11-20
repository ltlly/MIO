use gpui_kit::component::input::{InlineToken, InputContent};
use mio_core::intruder::{MAX_POSITIONS, Position};

use super::*;

pub(super) fn token_id(token: &InlineToken) -> Option<(&str, bool)> {
    if token.text().as_ref() != MARKER {
        return None;
    }
    let rest = token.id().strip_prefix(TOKEN_PREFIX)?;
    let (id, side) = rest.rsplit_once(':')?;
    match side {
        "start" if !id.is_empty() => Some((id, true)),
        "end" if !id.is_empty() => Some((id, false)),
        _ => None,
    }
}

pub(super) fn marker(id: &str, opening: bool) -> InlineToken {
    InlineToken::new(
        format!(
            "{TOKEN_PREFIX}{id}:{}",
            if opening { "start" } else { "end" }
        ),
        MARKER,
    )
}

pub(super) fn pair_content(text: &str, id: &str) -> Result<InputContent, String> {
    InputContent::new(format!("{MARKER}{text}{MARKER}"))
        .with_token(0..MARKER.len(), marker(id, true))
        .and_then(|content| {
            content.with_token(
                MARKER.len() + text.len()..2 * MARKER.len() + text.len(),
                marker(id, false),
            )
        })
        .map_err(|error| format!("无法标记当前选择：{error}；原始字节未改变。"))
}

pub(super) fn validate_positions(length: usize, positions: &[Position]) -> Result<(), String> {
    if positions.len() > MAX_POSITIONS {
        return Err(format!("Payload 位置超过 {MAX_POSITIONS}；原配置已保留。"));
    }
    let mut previous = 0;
    for position in positions {
        if position.start > position.end || position.end > length || position.start < previous {
            return Err(
                "Payload 位置范围无效或重叠；原配置已保留，请明确使用清除 § 或自动 § 修复。".into(),
            );
        }
        previous = position.end;
    }
    Ok(())
}

pub(super) fn content_for(
    bytes: &[u8],
    marks: &[Mark],
    view: MessageView,
) -> Result<InputContent, String> {
    validate_positions(
        bytes.len(),
        &marks.iter().map(|mark| mark.position).collect::<Vec<_>>(),
    )?;
    let plain = match view {
        MessageView::Raw => std::str::from_utf8(bytes)
            .map_err(|_| "消息含非 UTF-8 字节，请使用 Hex 视图查看原始字节。".to_owned())?
            .to_owned(),
        MessageView::Hex => editing::hex_encode(bytes),
    };
    let mut text = String::new();
    let mut tokens = Vec::new();
    let mut cursor = 0;
    for mark in marks {
        let position = mark.position;
        let range = match view {
            MessageView::Raw => position.start..position.end,
            MessageView::Hex => {
                let start = (position.start * 3).min(plain.len());
                let end = if position.start == position.end {
                    start
                } else {
                    position.end * 3 - 1
                };
                start..end
            }
        };
        let Some(before) = plain.get(cursor..range.start) else {
            return Err("Payload 位置切分 UTF-8 字节；请使用 Hex 视图。".into());
        };
        let Some(selected) = plain.get(range.clone()) else {
            return Err("Payload 位置切分 UTF-8 字节；请使用 Hex 视图。".into());
        };
        text.push_str(before);
        let start = text.len();
        text.push_str(MARKER);
        tokens.push((start..text.len(), marker(&mark.id, true)));
        text.push_str(selected);
        let end = text.len();
        text.push_str(MARKER);
        tokens.push((end..text.len(), marker(&mark.id, false)));
        cursor = range.end;
    }
    text.push_str(&plain[cursor..]);
    let mut content = InputContent::new(text);
    for (range, token) in tokens {
        content = content
            .with_token(range, token)
            .map_err(|error| format!("Payload 标记边界无效：{error}；请使用 Hex 视图。"))?;
    }
    Ok(content)
}

pub(super) fn without_markers(content: &InputContent) -> (String, Vec<(String, bool, usize)>) {
    let mut text = String::new();
    let mut markers = Vec::new();
    let mut cursor = 0;
    for span in content.tokens() {
        let Some((id, opening)) = token_id(span.token()) else {
            continue;
        };
        text.push_str(&content.text()[cursor..span.range().start]);
        markers.push((id.to_owned(), opening, text.len()));
        cursor = span.range().end;
    }
    text.push_str(&content.text()[cursor..]);
    (text, markers)
}

pub(super) fn unmarked_offset(content: &InputContent, offset: usize) -> usize {
    offset
        - content
            .tokens()
            .iter()
            .filter(|span| token_id(span.token()).is_some() && span.range().end <= offset)
            .map(|span| span.range().len())
            .sum::<usize>()
}

pub(super) fn decode_text(text: String, view: MessageView) -> Result<Vec<u8>, String> {
    match view {
        MessageView::Raw => Ok(text.into_bytes()),
        MessageView::Hex => editing::hex_decode(&text),
    }
}

pub(super) fn byte_offsets(
    text: &str,
    offsets: &[usize],
    view: MessageView,
) -> Result<Vec<usize>, String> {
    if view == MessageView::Raw {
        return Ok(offsets.to_vec());
    }
    let mut cursor = 0;
    let mut digits = 0;
    let mut bytes = Vec::with_capacity(offsets.len());
    for offset in offsets {
        digits += text[cursor..*offset]
            .bytes()
            .filter(|byte| !byte.is_ascii_whitespace())
            .count();
        if digits % 2 != 0 {
            return Err("Payload 标记必须位于完整 Hex 字节边界；标记和输入仍保留。".into());
        }
        bytes.push(digits / 2);
        cursor = *offset;
    }
    Ok(bytes)
}

pub(super) fn snapshot_content(
    content: &InputContent,
    view: MessageView,
) -> Result<(Vec<u8>, Vec<Mark>), String> {
    let (text, markers) = without_markers(content);
    let offsets = byte_offsets(
        &text,
        &markers
            .iter()
            .map(|(_, _, offset)| *offset)
            .collect::<Vec<_>>(),
        view,
    )?;
    let bytes = decode_text(text, view)?;
    let mut opened = None;
    let mut marks = Vec::new();
    for ((id, opening, _), offset) in markers.into_iter().zip(offsets) {
        match (opening, opened.take()) {
            (true, None) => opened = Some((id, offset)),
            (false, Some((expected, start))) if id == expected => marks.push(Mark {
                id,
                position: Position { start, end: offset },
            }),
            _ => {
                return Err("Payload 标记未配对；输入仍保留，请撤销编辑或使用清除 § 修复。".into());
            }
        }
    }
    if opened.is_some() {
        return Err("Payload 标记未配对；输入仍保留，请撤销编辑或使用清除 § 修复。".into());
    }
    validate_positions(
        bytes.len(),
        &marks.iter().map(|mark| mark.position).collect::<Vec<_>>(),
    )?;
    Ok((bytes, marks))
}
