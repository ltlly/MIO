use std::ops::Range;

use gpui_kit::component::input::{InputContent, TextareaState};
use mio_core::intruder::{MAX_POSITIONS, Position};

use super::*;

mod actions;
mod tokens;
use tokens::*;

const MARKER: &str = "§";
const TOKEN_PREFIX: &str = "intruder-position:";

#[derive(Clone, Debug, PartialEq, Eq)]
struct Mark {
    id: String,
    position: Position,
}

pub(super) struct PendingPositions {
    project: ProjectKey,
    operation_id: Uuid,
    tab_id: TabId,
    editor_id: gpui_kit::EntityId,
    view: MessageView,
    content: InputContent,
    selection: Range<usize>,
    target: SharedString,
    document_revision: u64,
    target_revision: u64,
    spec_revision: u64,
    draft_revision: u64,
    retained: Vec<Position>,
    repair: bool,
}

pub(in crate::workbench) struct PositionsEditor {
    pub(in crate::workbench) raw: Entity<TextareaState>,
    pub(super) hex: Entity<TextareaState>,
    pub(in crate::workbench) view: MessageView,
    pub(in crate::workbench) soft_wrap: bool,
    pub(in crate::workbench) search_case_sensitive: bool,
    pub(in crate::workbench) search_regex: bool,
    pub(in crate::workbench) search_error: Option<String>,
    pub(super) saved_positions: Vec<Position>,
    unplaced: Option<Vec<Position>>,
    pub(super) position_error: Option<String>,
}

impl PositionsEditor {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        let default_wrap = theme::editor_soft_wrap();
        let default_case = theme::highlight_search_case_sensitive_default();
        let mut state = || {
            cx.new(|cx| {
                TextareaState::new(window, cx)
                    .soft_wrap(default_wrap)
                    .autocorrect(false)
            })
        };
        Self {
            raw: state(),
            hex: state(),
            view: MessageView::Raw,
            soft_wrap: default_wrap,
            search_case_sensitive: default_case,
            search_regex: false,
            search_error: None,
            saved_positions: Vec::new(),
            unplaced: None,
            position_error: None,
        }
    }

    pub(super) fn toggle_soft_wrap(&mut self, window: &mut Window, cx: &mut App) {
        let next = !self.soft_wrap;
        self.soft_wrap = next;
        self.raw
            .update(cx, |state, cx| state.set_soft_wrap(next, window, cx));
        self.hex
            .update(cx, |state, cx| state.set_soft_wrap(next, window, cx));
    }

    pub(in crate::workbench) fn state(&self) -> &Entity<TextareaState> {
        match self.view {
            MessageView::Raw => &self.raw,
            MessageView::Hex => &self.hex,
        }
    }

    pub(super) fn load(
        &mut self,
        bytes: &[u8],
        positions: &[Position],
        window: &mut Window,
        cx: &mut App,
    ) {
        self.saved_positions = positions.to_vec();
        self.position_error = validate_positions(bytes.len(), positions).err();
        self.unplaced = self.position_error.as_ref().map(|_| positions.to_vec());
        let marks: Vec<_> = positions
            .iter()
            .enumerate()
            .filter(|_| self.unplaced.is_none())
            .map(|(index, position)| Mark {
                id: index.to_string(),
                position: *position,
            })
            .collect();
        let preferred_wrap = theme::editor_soft_wrap();
        if self.soft_wrap != preferred_wrap {
            self.soft_wrap = preferred_wrap;
            self.raw.update(cx, |state, cx| {
                state.set_soft_wrap(preferred_wrap, window, cx)
            });
            self.hex.update(cx, |state, cx| {
                state.set_soft_wrap(preferred_wrap, window, cx)
            });
        }
        let (view, content) = match content_for(bytes, &marks, MessageView::Raw) {
            Ok(content) => (MessageView::Raw, content),
            Err(_) => (
                MessageView::Hex,
                content_for(bytes, &marks, MessageView::Hex)
                    .expect("valid byte ranges can always be represented in Hex"),
            ),
        };
        self.raw
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.hex
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.view = view;
        self.state()
            .update(cx, |state, cx| state.set_value(content, window, cx));
    }

    pub(super) fn content(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<InputContent, String> {
        self.state().update(cx, |state, cx| {
            if state.marked_text_range(window, cx).is_some() {
                return Err(editing::COMPOSITION_PENDING.to_owned());
            }
            Ok(state.content())
        })
    }

    pub(super) fn snapshot(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(Vec<u8>, Vec<Position>), String> {
        let (bytes, marks) = snapshot_content(&self.content(window, cx)?, self.view)?;
        Ok((
            bytes,
            self.unplaced
                .clone()
                .unwrap_or_else(|| marks.into_iter().map(|mark| mark.position).collect()),
        ))
    }

    pub(super) fn bytes(&self, window: &mut Window, cx: &mut App) -> Result<Vec<u8>, String> {
        let (text, _) = without_markers(&self.content(window, cx)?);
        decode_text(text, self.view)
    }

    pub(in crate::workbench) fn has_unplaced(&self) -> bool {
        self.unplaced.is_some()
    }

    pub(super) fn has_markers(&self, cx: &App) -> bool {
        self.unplaced.is_some()
            || self
                .state()
                .read(cx)
                .tokens()
                .iter()
                .any(|span| token_id(span.token()).is_some())
    }

    pub(super) fn display_count(&self, cx: &App) -> String {
        if let Some(positions) = &self.unplaced {
            return format!("{} 个位置（范围无效，原配置已保留）", positions.len());
        }
        match self.position_count(cx) {
            Some(count) => format!("{count} 个 payload 位置"),
            None => "payload 标记未配对".into(),
        }
    }

    pub(super) fn position_count(&self, cx: &App) -> Option<usize> {
        if let Some(positions) = &self.unplaced {
            return Some(positions.len());
        }
        let mut opened: Option<&str> = None;
        let mut count = 0;
        for span in self.state().read(cx).tokens() {
            let Some((id, opening)) = token_id(span.token()) else {
                continue;
            };
            match (opening, opened) {
                (true, None) => opened = Some(id),
                (false, Some(expected)) if id == expected => {
                    opened = None;
                    count += 1;
                }
                _ => return None,
            }
        }
        if opened.is_some() { None } else { Some(count) }
    }

    pub(super) fn byte_length(&self, cx: &App) -> Option<usize> {
        let state = self.state().read(cx);
        match self.view {
            MessageView::Raw => Some(
                state.value().len()
                    - state
                        .tokens()
                        .iter()
                        .filter(|span| token_id(span.token()).is_some())
                        .map(|span| span.range().len())
                        .sum::<usize>(),
            ),
            MessageView::Hex => render::hex_length(&without_markers(&state.content()).0),
        }
    }

    pub(super) fn switch(
        &mut self,
        view: MessageView,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        if self.view == view {
            return Ok(());
        }
        let focused = self.state().focus_handle(cx).is_focused(window);
        let (bytes, marks) = snapshot_content(&self.content(window, cx)?, self.view)?;
        let content = content_for(&bytes, &marks, view)?;
        let state = match view {
            MessageView::Raw => &self.raw,
            MessageView::Hex => &self.hex,
        };
        if state.read(cx).content() != content {
            state.update(cx, |state, cx| state.set_value(content, window, cx));
        }
        self.view = view;
        if focused {
            self.state().update(cx, |state, cx| state.focus(window, cx));
        }
        Ok(())
    }

    pub(super) fn add(&mut self, window: &mut Window, cx: &mut App) -> Result<(), String> {
        if self.unplaced.is_some() {
            return Err("原 payload 位置范围无效且已保留；请先使用清除 § 或自动 § 修复。".into());
        }
        let content = self.content(window, cx)?;
        let (_, marks) = snapshot_content(&content, self.view)?;
        if marks.len() >= MAX_POSITIONS {
            return Err(format!(
                "Intruder 最多支持 {MAX_POSITIONS} 个位置；标记未改变。"
            ));
        }
        let selection = self.state().read(cx).selected_range();
        if selection.is_empty() {
            let inside = content
                .tokens()
                .iter()
                .filter(|span| span.range().end <= selection.start)
                .filter_map(|span| token_id(span.token()))
                .next_back()
                .is_some_and(|(_, opening)| opening);
            if inside {
                return Err("光标位于已有 payload 位置内；不能创建重叠位置。".into());
            }
        }
        if content.tokens().iter().any(|span| {
            let range = span.range();
            token_id(span.token()).is_some()
                && range.start < selection.end
                && selection.start < range.end
        }) {
            return Err("选择包含已有 payload 标记；请先清除该标记，不能创建重叠位置。".into());
        }
        let (text, _) = without_markers(&content);
        let start = unmarked_offset(&content, selection.start);
        let end = unmarked_offset(&content, selection.end);
        let offsets = byte_offsets(&text, &[start, end], self.view)?;
        let position = Position {
            start: offsets[0],
            end: offsets[1],
        };
        let at = marks.partition_point(|mark| mark.position.end <= position.start);
        if marks
            .get(at)
            .is_some_and(|mark| position.end > mark.position.start)
        {
            return Err("选择与已有 payload 位置重叠；标记未改变。".into());
        }
        let selected = &content.text()[selection.clone()];
        let id = Uuid::new_v4().to_string();
        let marked = pair_content(selected, &id)?;
        self.state().update(cx, |state, cx| {
            state
                .replace_range_with_content(selection.clone(), marked, window, cx)
                .map_err(|error| format!("未添加 payload 位置：{error}"))?;
            state.set_selected_range(
                selection.start + MARKER.len()..selection.end + MARKER.len(),
                cx,
            );
            state.focus(window, cx);
            Ok(())
        })
    }

    pub(super) fn clear(&mut self, window: &mut Window, cx: &mut App) -> Result<(), String> {
        let content = self.content(window, cx)?;
        let markers: Vec<_> = content
            .tokens()
            .iter()
            .filter(|span| token_id(span.token()).is_some())
            .collect();
        let (Some(first), Some(last)) = (markers.first(), markers.last()) else {
            return Ok(());
        };
        let range = first.range().start..last.range().end;
        let mut text = String::new();
        let mut cursor = range.start;
        for span in markers {
            text.push_str(&content.text()[cursor..span.range().start]);
            cursor = span.range().end;
        }
        let selection = self.state().read(cx).selected_range();
        let selection =
            unmarked_offset(&content, selection.start)..unmarked_offset(&content, selection.end);
        self.state().update(cx, |state, cx| {
            state
                .replace_range_with_content(range, InputContent::new(text), window, cx)
                .map_err(|error| format!("未清除 payload 标记：{error}"))?;
            state.set_selected_range(selection, cx);
            state.focus(window, cx);
            Ok(())
        })
    }

    pub(super) fn set_positions(
        &mut self,
        positions: &[Position],
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        let bytes = self.bytes(window, cx)?;
        validate_positions(bytes.len(), positions)?;
        let marks: Vec<_> = positions
            .iter()
            .map(|position| Mark {
                id: Uuid::new_v4().to_string(),
                position: *position,
            })
            .collect();
        let content = content_for(&bytes, &marks, self.view)?;
        self.state().update(cx, |state, cx| {
            state
                .replace_range_with_content(0..state.value().len(), content, window, cx)
                .map_err(|error| format!("未替换 payload 标记：{error}"))?;
            Ok::<_, String>(())
        })?;
        self.unplaced = None;
        self.position_error = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
