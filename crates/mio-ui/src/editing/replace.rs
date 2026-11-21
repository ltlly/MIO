use std::ops::Range;

use super::{search::search_bar_button, *};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonVariants;

const MAX_EDITOR_REPLACE_BYTES: usize = 16 * 1024 * 1024;

pub(crate) fn validate_payload_markers(
    view: MessageView,
    text: &str,
    ranges: &[Range<usize>],
    replacement: &str,
) -> Result<(), String> {
    if view != MessageView::Raw {
        return Ok(());
    }
    if ranges
        .iter()
        .any(|range| text.get(range.clone()).is_some_and(|m| m.contains('§')))
    {
        return Err(
            "匹配范围包含 § payload 标记；为保护标记配对，请缩小匹配范围至标记内部文本后再替换。"
                .into(),
        );
    }
    if !replacement.matches('§').count().is_multiple_of(2) {
        return Err("替换内容包含未配对的 § payload 标记；原始字节未改变。".into());
    }
    Ok(())
}

impl MessageEditor {
    pub(super) fn has_any_composition(&self, window: &mut Window, cx: &mut App) -> bool {
        let state = self.state().clone();
        state.update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
            || self
                .search
                .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
            || self
                .replace
                .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
    }

    fn ensure_replaceable(
        &self,
        readonly: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        if readonly {
            return Err("当前消息为只读，不能执行查找替换。".into());
        }
        if self.is_readonly_preview() {
            return Err(
                "当前消息视图为只读预览，请关闭预览后执行查找替换。".into(),
            );
        }
        if self.has_any_composition(window, cx) {
            return Err(COMPOSITION_PENDING.into());
        }
        Ok(())
    }

    pub(super) fn active_pending_replace_all(&self, cx: &App) -> Option<usize> {
        let pending = self.pending_replace_all.as_ref()?;
        let active = self.state();
        let state = active.read(cx);
        let query = self.search.read(cx).value();
        let replacement = self.replace.read(cx).value();
        let s = self.shared.borrow();
        (!self.is_readonly_preview()
            && pending.view == s.view
            && pending.pretty_active == s.pretty_active
            && pending.document_revision == state.document_revision()
            && pending.query == query.as_ref()
            && pending.replacement == replacement.as_ref()
            && pending.case_sensitive == s.case_sensitive
            && pending.regex == s.regex)
            .then_some(pending.match_count)
    }

    pub fn toggle_replace(
        &mut self,
        readonly: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        self.ensure_replaceable(readonly, window, cx)?;
        self.replace_open = !self.replace_open;
        self.pending_replace_all = None;
        if self.replace_open {
            self.replace.update(cx, |input, cx| {
                input.focus(window, cx);
                input.select_all(window, cx);
            });
        }
        Ok(())
    }

    pub fn cancel_replace_all(&mut self) {
        self.pending_replace_all = None;
    }

    pub fn replace_current(
        &mut self,
        readonly: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        self.ensure_replaceable(readonly, window, cx)?;
        self.pending_replace_all = None;
        self.update_search(None, window, cx)?;
        let query = input_snapshot(&self.search, window, cx)
            .map_err(str::to_owned)?
            .to_string();
        let replacement = input_snapshot(&self.replace, window, cx)
            .map_err(str::to_owned)?
            .to_string();
        if query.is_empty() {
            return Err("请先输入要查找的内容。".into());
        }
        let (view, pretty_active, case_sensitive, regex) = {
            let s = self.shared.borrow();
            (s.view, s.pretty_active, s.case_sensitive, s.regex)
        };
        let (text, range) = {
            let active = self.state();
            let state = active.read(cx);
            let session = state.search_session();
            let ix = session
                .matcher
                .current()
                .ok_or_else(|| "当前没有可替换的匹配项。".to_owned())?;
            let range = session
                .matcher
                .matched_ranges()
                .get(ix)
                .cloned()
                .ok_or_else(|| "当前没有可替换的匹配项。".to_owned())?;
            (state.value().to_string(), range)
        };
        validate_payload_markers(view, &text, std::slice::from_ref(&range), &replacement)?;
        let expanded = mio_core::expand_search_replace_one(
            &text,
            range.clone(),
            &query,
            &replacement,
            !case_sensitive,
            regex,
            MAX_EDITOR_REPLACE_BYTES,
        )?;
        let target = self.state();
        let mut canonical_next_offset = None;
        if view == MessageView::Hex && !pretty_active {
            let mut candidate = String::with_capacity(text.len() - range.len() + expanded.len());
            candidate.push_str(&text[..range.start]);
            candidate.push_str(&expanded);
            candidate.push_str(&text[range.end..]);
            let decoded = hex_decode(&candidate).map_err(|err| {
                format!("替换结果不是有效的完整 Hex 字节（{err}）；原始字节未改变。")
            })?;
            let prefix_digits = text[..range.start]
                .bytes()
                .filter(u8::is_ascii_hexdigit)
                .count();
            let expanded_digits = expanded
                .bytes()
                .filter(u8::is_ascii_hexdigit)
                .count();
            let end_byte = (prefix_digits + expanded_digits)
                .div_ceil(2)
                .min(decoded.len());
            canonical_next_offset = Some(end_byte.saturating_mul(3));
            if let Some(grid) = self.hex_grid() {
                grid.update(cx, |g, cx| g.replace_all_bytes(decoded, window, cx));
            } else {
                let canonical = hex_encode(&decoded);
                let rev = target.update(cx, |state, cx| {
                    state.replace_all_without_history(canonical, window, cx);
                    state.document_revision()
                });
                let mut s = self.shared.borrow_mut();
                s.wire_bytes = decoded;
                s.hex_synced_revision = Some(rev);
            }
        } else {
            target.update(cx, |state, cx| {
                state.set_selected_range(range.clone(), cx);
                state.replace(expanded.clone(), window, cx);
            });
        }
        let next_offset = canonical_next_offset.unwrap_or(range.start + expanded.len());
        let _ = self.update_search(None, window, cx);
        let steps_needed = {
            let active = self.state();
            let state = active.read(cx);
            let matcher = &state.search_session().matcher;
            let ranges = matcher.matched_ranges();
            if ranges.is_empty() {
                0
            } else {
                let target_ix = ranges
                    .iter()
                    .position(|r| r.start >= next_offset)
                    .unwrap_or(0);
                let cur_ix = matcher.current_match_index();
                if target_ix >= cur_ix {
                    target_ix - cur_ix
                } else {
                    ranges.len() - cur_ix + target_ix
                }
            }
        };
        if steps_needed > 0 {
            let revealed = target.update(cx, |state, cx| {
                let mut last = None;
                for _ in 0..steps_needed {
                    last = state.next_search_match(cx);
                }
                last
            });
            if view == MessageView::Hex
                && !pretty_active
                && let Some(match_range) = revealed
                && let Some(grid) = self.hex_grid()
            {
                grid.update(cx, |g, cx| g.scroll_to_text_range(match_range, cx));
            }
        }
        Ok(())
    }

    pub fn request_replace_all(
        &mut self,
        readonly: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        self.ensure_replaceable(readonly, window, cx)?;
        self.update_search(None, window, cx)?;
        let query = input_snapshot(&self.search, window, cx)
            .map_err(str::to_owned)?
            .to_string();
        let replacement = input_snapshot(&self.replace, window, cx)
            .map_err(str::to_owned)?
            .to_string();
        if query.is_empty() {
            return Err("请先输入要查找的内容。".into());
        }
        let (view, pretty_active, case_sensitive, regex) = {
            let s = self.shared.borrow();
            (s.view, s.pretty_active, s.case_sensitive, s.regex)
        };
        let (text, ranges, doc_rev) = {
            let active = self.state();
            let state = active.read(cx);
            let ranges = state.search_session().matcher.matched_ranges().to_vec();
            (state.value().to_string(), ranges, state.document_revision())
        };
        if ranges.is_empty() {
            return Err("当前没有可替换的匹配项。".into());
        }
        validate_payload_markers(view, &text, &ranges, &replacement)?;
        let candidate = mio_core::expand_search_replace_all(
            &text,
            &ranges,
            &query,
            &replacement,
            !case_sensitive,
            regex,
            MAX_EDITOR_REPLACE_BYTES,
        )?;
        if view == MessageView::Hex && !pretty_active {
            hex_decode(&candidate).map_err(|err| {
                format!("全部替换结果不是有效的完整 Hex 字节（{err}）；原始字节未改变。")
            })?;
        }
        self.pending_replace_all = Some(PendingReplaceAll {
            view,
            pretty_active,
            document_revision: doc_rev,
            query,
            replacement,
            case_sensitive,
            regex,
            match_count: ranges.len(),
        });
        Ok(())
    }

    pub fn confirm_replace_all(
        &mut self,
        readonly: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        self.ensure_replaceable(readonly, window, cx)?;
        if self.active_pending_replace_all(cx).is_none() {
            self.pending_replace_all = None;
            return Err(
                "确认期间搜索条件或消息内容已变更，原始字节未改变，请重新点击全部替换。".into(),
            );
        }
        let pending = self.pending_replace_all.take().expect("checked pending");
        let (view, pretty_active) = {
            let s = self.shared.borrow();
            (s.view, s.pretty_active)
        };
        let (text, ranges) = {
            let active = self.state();
            let state = active.read(cx);
            (
                state.value().to_string(),
                state.search_session().matcher.matched_ranges().to_vec(),
            )
        };
        validate_payload_markers(view, &text, &ranges, &pending.replacement)?;
        let replaced = mio_core::expand_search_replace_all(
            &text,
            &ranges,
            &pending.query,
            &pending.replacement,
            !pending.case_sensitive,
            pending.regex,
            MAX_EDITOR_REPLACE_BYTES,
        )?;
        if view == MessageView::Hex && !pretty_active {
            let decoded = hex_decode(&replaced).map_err(|err| {
                format!("全部替换结果不是有效的完整 Hex 字节（{err}）；原始字节未改变。")
            })?;
            if let Some(grid) = self.hex_grid() {
                grid.update(cx, |g, cx| g.replace_all_bytes(decoded, window, cx));
            } else {
                let canonical = hex_encode(&decoded);
                let rev = self.state().update(cx, |state, cx| {
                    state.replace_all_without_history(canonical, window, cx);
                    state.document_revision()
                });
                let mut s = self.shared.borrow_mut();
                s.wire_bytes = decoded;
                s.hex_synced_revision = Some(rev);
            }
        } else {
            self.state()
                .update(cx, |state, cx| state.replace_all(replaced, window, cx));
        }
        let _ = self.update_search(None, window, cx);
        Ok(())
    }

    pub(super) fn render_replace_row<V: 'static>(
        &self,
        id: &'static str,
        cx: &mut gpui_kit::Context<V>,
        on_action: impl Fn(&mut V, SearchBarAction, &mut Window, &mut gpui_kit::Context<V>)
        + Clone
        + 'static,
    ) -> AnyElement {
        let has_matches = !self.is_readonly_preview()
            && !self.state().read(cx).search_session().matcher.is_empty()
            && !self.has_search_error();
        let pending_count = self.active_pending_replace_all(cx);
        let replace_one_action = on_action.clone();
        let replace_all_action = on_action.clone();
        let confirm_action = on_action.clone();
        let cancel_action = on_action;

        let mut row = div()
            .id(SharedString::from(format!("{id}-replace-bar")))
            .min_h(crate::theme::scaled_px(34.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .overflow_x_scroll()
            .gap(crate::theme::scaled_px(6.))
            .px(crate::theme::scaled_px(8.))
            .py(px(2.))
            .border_t_1()
            .border_color(crate::theme::border())
            .child(
                div()
                    .id(SharedString::from(format!("{id}-replace-input")))
                    .flex_1()
                    .min_w(crate::theme::scaled_px(80.))
                    .child(Input::new(&self.replace).small()),
            )
            .child(
                search_bar_button(format!("{id}-replace-one"), "替换")
                    .icon(IconName::Replace)
                    .tooltip("替换当前选中的单个搜索匹配项并跳至下一处")
                    .disabled(!has_matches)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        replace_one_action(this, SearchBarAction::Replace, window, cx);
                    })),
            );

        if let Some(count) = pending_count {
            row = row
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-replace-confirm-prompt")))
                        .test_support()
                        .child(format!("确认替换 {count} 处？")),
                )
                .child(
                    search_bar_button(format!("{id}-replace-confirm"), "确认替换")
                        .primary()
                        .icon(IconName::Check)
                        .tooltip("确认在当前报文中一次性替换全部匹配项")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            confirm_action(this, SearchBarAction::ConfirmReplaceAll, window, cx);
                        })),
                )
                .child(
                    search_bar_button(format!("{id}-replace-cancel"), "取消")
                        .icon(IconName::Close)
                        .tooltip("取消本次全部替换操作")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cancel_action(this, SearchBarAction::CancelReplaceAll, window, cx);
                        })),
                );
        } else {
            row = row.child(
                search_bar_button(format!("{id}-replace-all"), "全部替换")
                    .icon(IconName::ReplaceAll)
                    .tooltip("预览并批量替换当前报文中的所有匹配项")
                    .disabled(!has_matches)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        replace_all_action(this, SearchBarAction::RequestReplaceAll, window, cx);
                    })),
            );
        }
        row.into_any_element()
    }
}

#[cfg(test)]
pub(crate) fn replace_inline_token_matches(
    textarea: &Entity<gpui_kit::component::input::TextareaState>,
    pattern: &str,
    replacement: &str,
    case_sensitive: bool,
    regex: bool,
    window: &mut Window,
    cx: &mut App,
) -> Result<usize, String> {
    use gpui_kit::component::input::InputContent;

    if textarea.update(cx, |s, cx| s.marked_text_range(window, cx).is_some()) {
        return Err(COMPOSITION_PENDING.into());
    }
    textarea.update(cx, |state, cx| {
        state
            .set_search_regex(pattern.to_owned(), !case_sensitive, regex, cx)
            .map_err(|err| format!("搜索正则表达式无效：{err}"))?;
        let ranges = state.search_session().matcher.matched_ranges().to_vec();
        if ranges.is_empty() {
            return Ok(0);
        }
        let orig = state.content();
        for m in &ranges {
            if orig.tokens().iter().any(|span| {
                (span.token().text().as_ref() == "§"
                    || span.token().id().starts_with("intruder-position:"))
                    && span.range().start < m.end
                    && m.start < span.range().end
            }) {
                return Err(
                    "匹配范围包含 § payload 标记；为保护标记配对，请缩小匹配范围至标记内部文本后再替换。"
                        .into(),
                );
            }
        }
        let replaced_text = mio_core::expand_search_replace_all(
            orig.text(),
            &ranges,
            pattern,
            replacement,
            !case_sensitive,
            regex,
            MAX_EDITOR_REPLACE_BYTES,
        )?;
        let mut next = InputContent::new(replaced_text);
        for span in orig.tokens() {
            let mut shift = 0isize;
            for m in &ranges {
                if m.end <= span.range().start {
                    let expanded = mio_core::expand_search_replace_one(
                        orig.text(),
                        m.clone(),
                        pattern,
                        replacement,
                        !case_sensitive,
                        regex,
                        MAX_EDITOR_REPLACE_BYTES,
                    )?;
                    shift += expanded.len() as isize - m.len() as isize;
                }
            }
            let start = (span.range().start as isize + shift) as usize;
            let end = (span.range().end as isize + shift) as usize;
            next = next
                .with_token(start..end, span.token().clone())
                .map_err(|err| format!("更新 § 标记失败：{err}"))?;
        }
        state
            .replace_range_with_content(0..orig.text().len(), next, window, cx)
            .map_err(|err| format!("未完成替换：{err}"))?;
        Ok(ranges.len())
    })
}
