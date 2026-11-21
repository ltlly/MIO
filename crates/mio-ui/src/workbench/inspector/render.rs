use super::section_singular_label;
use super::*;
use crate::workbench::http_notes::render::{
    NoteMarkdownAction, format_markdown_snippet, render_markdown_preview,
};
use crate::workbench::wb_render_chrome::sidebar_rail_tab_with_dock;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{ButtonRounded, ButtonVariants};

fn move_id_prefixes(section: InspectorSection) -> (&'static str, &'static str) {
    match section {
        InspectorSection::RequestAttributes => {
            ("inspector-move-up-attributes", "inspector-move-down-attributes")
        }
        InspectorSection::QueryParams => ("inspector-move-up-query", "inspector-move-down-query"),
        InspectorSection::BodyParams => ("inspector-move-up-body", "inspector-move-down-body"),
        InspectorSection::Cookies => ("inspector-move-up-cookies", "inspector-move-down-cookies"),
        InspectorSection::RequestHeaders => (
            "inspector-move-up-request-headers",
            "inspector-move-down-request-headers",
        ),
        InspectorSection::ResponseHeaders => (
            "inspector-move-up-response-headers",
            "inspector-move-down-response-headers",
        ),
    }
}

fn request_attributes_split(
    section: InspectorSection,
    items: &[InspectorItem],
) -> (Option<&InspectorItem>, &[InspectorItem]) {
    if section == InspectorSection::RequestAttributes
        && let Some(proto) = items
            .get(2)
            .filter(|item| item.name.eq_ignore_ascii_case("protocol"))
    {
        (Some(proto), &items[..2])
    } else {
        (None, items)
    }
}

fn section_items_slice(data: &InspectorData, section: InspectorSection) -> &[InspectorItem] {
    let items = match section {
        InspectorSection::RequestAttributes => &data.attributes,
        InspectorSection::QueryParams => &data.query_params,
        InspectorSection::BodyParams => &data.body_params,
        InspectorSection::Cookies => &data.cookies,
        InspectorSection::RequestHeaders => &data.request_headers,
        InspectorSection::ResponseHeaders => &data.response_headers,
    };
    request_attributes_split(section, items).1
}

impl Workbench {
    fn tool_supports_http_notes_dock(&self) -> bool {
        if self.ext_import.visible {
            return false;
        }
        match self.tool {
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => true,
            Tool::Target if self.target_pane.page == target::TargetPage::SiteMap => true,
            Tool::Logger => true,
            Tool::Dashboard if !self.audit_dashboard.new_scan_open => self
                .audit_dashboard
                .selected_finding_id
                .is_some_and(|fid| {
                    self.audit_dashboard
                        .snapshot
                        .findings
                        .iter()
                        .any(|finding| finding.id == fid && finding.flow_id.is_some())
                }),
            _ => false,
        }
    }

    pub(in crate::workbench) fn apply_repeater_notes_markdown(
        &mut self,
        action: NoteMarkdownAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_repeater_note_tab_id().is_none()
            || self.inspector.tab_notes_preview
            || self.transition.is_some()
            || self.project.is_none()
        {
            return;
        }
        let editor = self.inspector.tab_notes_editor.clone();
        if editor.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }) {
            self.inspector.tab_notes_error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        }
        let text = editor.read(cx).value().to_string();
        let range = editor.read(cx).selected_range();
        let reversed = editor.read(cx).is_selection_reversed();
        let (replacement, inner_offset, inner_len) =
            format_markdown_snippet(action, &text, range.clone());
        if text.len() - range.len() + replacement.len() > super::MAX_TAB_NOTE_BYTES {
            self.inspector.tab_notes_error =
                Some("工作标签备注超过 64 KiB 上限，请精简内容后再保存。".into());
            cx.notify();
            return;
        }
        let sel_start = range.start + inner_offset;
        let sel_end = sel_start + inner_len;
        editor.update(cx, |state, cx| {
            state.replace(replacement, window, cx);
            state.set_selected_range_with_direction(sel_start..sel_end, reversed, cx);
        });
        self.commit_active_repeater_tab_note(window, cx);
    }

    #[inline(never)]
    fn render_repeater_notes_toolbar(
        &self,
        can_format: bool,
        preview_mode: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        toolbar()
            .flex_nowrap()
            .gap(px(3.))
            .px(px(6.))
            .child(
                gpui_kit::component::Icon::new(IconName::FileText)
                    .size(theme::scaled_px(13.))
                    .text_color(theme::accent()),
            )
            .child(
                div()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .mr(px(2.))
                    .child("Notes"),
            )
            .child(
                Button::new("repeater-notes-bold")
                    .ghost()
                    .small()
                    .compact()
                    .h(theme::scaled_px(24.))
                    .px(px(6.))
                    .label("B")
                    .tooltip("为所选标签备注添加 Markdown 加粗格式 (**文本**)")
                    .disabled(!can_format)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.apply_repeater_notes_markdown(NoteMarkdownAction::Bold, window, cx)
                    })),
            )
            .child(
                Button::new("repeater-notes-italic")
                    .ghost()
                    .small()
                    .compact()
                    .h(theme::scaled_px(24.))
                    .px(px(6.))
                    .label("I")
                    .tooltip("为所选标签备注添加 Markdown 斜体格式 (*文本*)")
                    .disabled(!can_format)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.apply_repeater_notes_markdown(NoteMarkdownAction::Italic, window, cx)
                    })),
            )
            .child(
                Button::new("repeater-notes-code")
                    .ghost()
                    .small()
                    .compact()
                    .w(theme::scaled_px(24.))
                    .h(theme::scaled_px(24.))
                    .icon(IconName::Code)
                    .accessibility_label("Code")
                    .tooltip("将所选标签备注包裹为行内代码或代码块")
                    .disabled(!can_format)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.apply_repeater_notes_markdown(NoteMarkdownAction::Code, window, cx)
                    })),
            )
            .child(
                Button::new("repeater-notes-list")
                    .ghost()
                    .small()
                    .compact()
                    .w(theme::scaled_px(24.))
                    .h(theme::scaled_px(24.))
                    .icon(IconName::List)
                    .accessibility_label("List")
                    .tooltip("将所选标签备注行格式化为 Markdown 无序列表")
                    .disabled(!can_format)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.apply_repeater_notes_markdown(NoteMarkdownAction::List, window, cx)
                    })),
            )
            .child(
                Button::new("repeater-notes-heading")
                    .ghost()
                    .small()
                    .compact()
                    .h(theme::scaled_px(24.))
                    .px(px(6.))
                    .label("H")
                    .tooltip("将所选标签备注行格式化为 Markdown 标题")
                    .disabled(!can_format)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.apply_repeater_notes_markdown(NoteMarkdownAction::Heading, window, cx)
                    })),
            )
            .child(div().flex_1())
            .child(
                Button::new("repeater-notes-preview-toggle")
                    .ghost()
                    .small()
                    .compact()
                    .h(theme::scaled_px(24.))
                    .px(px(6.))
                    .icon(if preview_mode {
                        IconName::Pencil
                    } else {
                        IconName::Eye
                    })
                    .label(if preview_mode { "编辑" } else { "预览" })
                    .selected(preview_mode)
                    .tooltip(if preview_mode {
                        "切换回 Markdown 源码编辑模式"
                    } else {
                        "以富文本排版预览当前工作标签 Markdown 备注"
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        if !this.inspector.tab_notes_preview
                            && let Err(err) =
                                editing::snapshot(&this.inspector.tab_notes_editor, window, cx)
                        {
                            this.inspector.tab_notes_error = Some(err.to_owned());
                            cx.notify();
                            return;
                        }
                        this.inspector.tab_notes_preview = !this.inspector.tab_notes_preview;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("repeater-notes-close")
                    .ghost()
                    .small()
                    .compact()
                    .w(theme::scaled_px(24.))
                    .h(theme::scaled_px(24.))
                    .icon(IconName::Close)
                    .accessibility_label("关闭标签备注")
                    .tooltip("收起当前 Repeater 标签备注侧栏并自动保存")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Err(err) =
                            editing::snapshot(&this.inspector.tab_notes_editor, window, cx)
                        {
                            this.inspector.tab_notes_error = Some(err.to_owned());
                            this.fail(err, cx);
                            return;
                        }
                        this.commit_active_repeater_tab_note(window, cx);
                        let focused = this
                            .inspector
                            .tab_notes_editor
                            .focus_handle(cx)
                            .is_focused(window);
                        this.inspector.tab_notes_open = false;
                        if focused {
                            this.focus.focus(window, cx);
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_repeater_notes_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let preview_mode = self.inspector.tab_notes_preview;
        let editable = self.project.is_some() && self.transition.is_none();
        let can_format = editable && !preview_mode;
        let error = self.inspector.tab_notes_error.clone();
        div()
            .id("repeater-notes-sidebar")
            .test_support()
            .w(theme::scaled_px(336.).clamp(px(288.), px(420.)))
            .max_w(px(420.))
            .flex_shrink_0()
            .h_full()
            .border_l_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .flex()
            .flex_col()
            .child(self.render_repeater_notes_toolbar(can_format, preview_mode, cx))
            .when_some(error, |this, err| {
                this.child(
                    div()
                        .id("repeater-notes-error")
                        .test_support()
                        .px(px(8.))
                        .py(px(4.))
                        .text_size(theme::scaled_px(11.5))
                        .text_color(theme::danger_fg())
                        .child(err),
                )
            })
            .child(div().flex_1().min_h_0().child(if preview_mode {
                render_markdown_preview(self.inspector.tab_notes_editor.read(cx).value().as_ref())
            } else {
                Editor::new(&self.inspector.tab_notes_editor)
                    .readonly(!editable)
                    .size_full()
                    .bordered(false)
                    .aria_label("Repeater 工作标签备注")
                    .into_any_element()
            }))
            .into_any_element()
    }

    pub(in crate::workbench) fn render_inspector_dock(
        &self,
        body: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_inspector_dock_inner(body, None, cx)
    }

    pub(in crate::workbench) fn render_repeater_inspector_dock(
        &self,
        body: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_inspector_dock_inner(body, Some(window), cx)
    }

    fn close_repeater_notes_before_switch(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.inspector.tab_notes_open {
            return true;
        }
        if let Err(err) = editing::snapshot(&self.inspector.tab_notes_editor, window, cx) {
            self.inspector.tab_notes_error = Some(err.to_owned());
            self.fail(err, cx);
            return false;
        }
        self.commit_active_repeater_tab_note(window, cx);
        let focused = self
            .inspector
            .tab_notes_editor
            .focus_handle(cx)
            .is_focused(window);
        self.inspector.tab_notes_open = false;
        if focused {
            self.focus.focus(window, cx);
        }
        true
    }

    #[inline(never)]
    fn render_inspector_dock_inner(
        &self,
        body: AnyElement,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.tool_supports_http_notes_dock() {
            return self.render_http_notes(body, cx);
        }
        let repeater_note_tab = self.active_repeater_note_tab_id();
        let has_repeater_tab = repeater_note_tab.is_some();
        let has_non_empty_note = repeater_note_tab
            .and_then(|id| self.workspace.tab_notes.get(&id))
            .is_some_and(|text| !text.trim().is_empty());
        let notes_open = has_repeater_tab && self.inspector.tab_notes_open;
        let explanations_open =
            has_repeater_tab && self.repeater_explanations_open && !notes_open;
        let custom_actions_open = has_repeater_tab
            && self.repeater_custom_actions_open
            && !notes_open
            && !explanations_open;
        let inspector_open =
            self.inspector.open && !notes_open && !explanations_open && !custom_actions_open;
        let extra_drawer = if explanations_open {
            window.map(|w| self.render_repeater_explanations_sidebar(w, cx))
        } else if custom_actions_open {
            window.map(|w| self.render_repeater_custom_actions_sidebar(w, cx))
        } else {
            None
        };
        let rail = sidebar_rail_with_dock(self.inspector.dock_left)
                    .child(
                        sidebar_rail_tab_with_dock(
                            "inspector-toggle",
                            "Inspector",
                            "Inspector",
                            IconName::SlidersHorizontal,
                            inspector_open,
                            false,
                            self.inspector.dock_left,
                            if inspector_open {
                                "收起 Inspector 检视面板"
                            } else {
                                "展开 Inspector 检视面板（查看参数、Cookie、协议头与选区编解码）"
                            },
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            let has_tab = this.active_repeater_note_tab_id().is_some();
                            let other_open = has_tab
                                && (this.inspector.tab_notes_open
                                    || this.repeater_explanations_open
                                    || this.repeater_custom_actions_open);
                            if other_open {
                                this.repeater_explanations_open = false;
                                this.repeater_custom_actions_open = false;
                                if !this.open_inspector(window, cx) {
                                    return;
                                }
                                cx.notify();
                                return;
                            }
                            if this.inspector.open {
                                if !this.dismiss_inspector_for_current_selection(window, cx) {
                                    return;
                                }
                            } else if !this.open_inspector(window, cx) {
                                return;
                            }
                            cx.notify();
                        })),
                    )
                    .when(has_repeater_tab, |rail| {
                        rail.child(
                            sidebar_rail_tab_with_dock(
                                "repeater-notes-toggle",
                                if has_non_empty_note {
                                    "Notes\n•"
                                } else {
                                    "Notes"
                                },
                                "Notes",
                                IconName::FileText,
                                notes_open,
                                false,
                                self.inspector.dock_left,
                                if notes_open {
                                    "收起当前工作标签备注侧栏"
                                } else if has_non_empty_note {
                                    "展开当前工作标签备注侧栏（已有备注内容）"
                                } else {
                                    "展开当前工作标签备注侧栏（记录测试思路与载荷说明）"
                                },
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.active_repeater_note_tab_id().is_none() {
                                    return;
                                }
                                if this.inspector.tab_notes_open {
                                    if !this.close_repeater_notes_before_switch(window, cx) {
                                        return;
                                    }
                                } else {
                                    if !this.dismiss_inspector_for_current_selection(window, cx) {
                                        return;
                                    }
                                    this.repeater_explanations_open = false;
                                    this.repeater_custom_actions_open = false;
                                    this.sync_repeater_tab_notes(window, cx);
                                    this.inspector.tab_notes_open = true;
                                }
                                cx.notify();
                            })),
                        )
                        .child(
                            sidebar_rail_tab_with_dock(
                                "repeater-explanations-toggle",
                                "Explanations",
                                "Explanations",
                                IconName::Info,
                                explanations_open,
                                false,
                                self.inspector.dock_left,
                                if explanations_open {
                                    "收起当前请求与响应的离线安全解释面板"
                                } else {
                                    "展开当前请求与响应的离线安全分析（JWT 解码、Cookie 与安全头审计）"
                                },
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.active_repeater_note_tab_id().is_none() {
                                    return;
                                }
                                if this.repeater_explanations_open {
                                    this.repeater_explanations_open = false;
                                } else {
                                    if !this.close_repeater_notes_before_switch(window, cx) {
                                        return;
                                    }
                                    if !this.dismiss_inspector_for_current_selection(window, cx) {
                                        return;
                                    }
                                    this.repeater_custom_actions_open = false;
                                    this.repeater_explanations_open = true;
                                }
                                cx.notify();
                            })),
                        )
                        .child(
                            sidebar_rail_tab_with_dock(
                                "repeater-custom-actions-toggle",
                                "Custom actions",
                                "Custom actions",
                                IconName::Zap,
                                custom_actions_open,
                                false,
                                self.inspector.dock_left,
                                if custom_actions_open {
                                    "收起 Repeater Custom actions 脚本执行面板"
                                } else {
                                    "展开 Repeater Custom actions，使用 Rhai / 脚本实时分析请求与响应"
                                },
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.active_repeater_note_tab_id().is_none() {
                                    return;
                                }
                                if this.repeater_custom_actions_open {
                                    this.repeater_custom_actions_open = false;
                                } else {
                                    if !this.close_repeater_notes_before_switch(window, cx) {
                                        return;
                                    }
                                    if !this.dismiss_inspector_for_current_selection(window, cx) {
                                        return;
                                    }
                                    this.repeater_explanations_open = false;
                                    this.repeater_custom_actions_open = true;
                                }
                                cx.notify();
                            })),
                        )
                    });
        if self.inspector.dock_left {
            div()
                .size_full()
                .flex()
                .child(rail)
                .when(inspector_open, |r| {
                    r.child(self.render_inspector_sidebar(cx))
                        .child(div().w(px(1.)).h_full().flex_shrink_0().bg(theme::border()))
                })
                .when(notes_open, |r| r.child(self.render_repeater_notes_sidebar(cx)))
                .when_some(extra_drawer, |r, drawer| r.child(drawer))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(body),
                )
                .into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(body),
                )
                .when(inspector_open, |r| {
                    r.child(div().w(px(1.)).h_full().flex_shrink_0().bg(theme::border()))
                        .child(self.render_inspector_sidebar(cx))
                })
                .when(notes_open, |r| r.child(self.render_repeater_notes_sidebar(cx)))
                .when_some(extra_drawer, |r, drawer| r.child(drawer))
                .child(rail)
                .into_any_element()
        }
    }

    fn add_inspector_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = match editing::input_snapshot(&self.inspector.name_input, window, cx) {
            Ok(name) => name.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let mut value = match editing::input_snapshot(&self.inspector.value_input, window, cx) {
            Ok(value) => value.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let section = self.effective_inspector_section(cx);
        if self.inspector.detail_url_decoded
            && matches!(
                section,
                InspectorSection::QueryParams
                    | InspectorSection::BodyParams
                    | InspectorSection::Cookies
            )
            && let Ok(encoded) = editing::convert_selection(editing::ConvertKind::UrlEncodeKey, &value)
        {
            value = encoded;
        }
        self.inspector.editing_section = section;
        let _ = self.handle_inspector_mutation(
            InspectorMutation::Add {
                section,
                name,
                value,
            },
            window,
            cx,
        );
    }

    fn update_inspector_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let section = self.effective_inspector_section(cx);
        if self.inspector.editing_section != section {
            return;
        }
        let Some(index) = self.inspector.editing_index else {
            return;
        };
        let name = match editing::input_snapshot(&self.inspector.name_input, window, cx) {
            Ok(name) => name.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let mut value = match editing::input_snapshot(&self.inspector.value_input, window, cx) {
            Ok(value) => value.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        if self.inspector.detail_url_decoded
            && matches!(
                section,
                InspectorSection::QueryParams
                    | InspectorSection::BodyParams
                    | InspectorSection::Cookies
            )
            && let Ok(encoded) = editing::convert_selection(editing::ConvertKind::UrlEncodeKey, &value)
        {
            value = encoded;
        }
        let _ = self.handle_inspector_mutation(
            InspectorMutation::Update {
                section,
                index,
                name,
                value,
            },
            window,
            cx,
        );
    }

    fn move_inspector_item(
        &mut self,
        section: InspectorSection,
        from: usize,
        to: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if from == to {
            return;
        }
        if self
            .inspector
            .name_input
            .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
            || self
                .inspector
                .value_input
                .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
        {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        if self.handle_inspector_mutation(
            InspectorMutation::Move { section, from, to },
            window,
            cx,
        ) && self.inspector.editing_section == section
            && let Some(sel) = self.inspector.editing_index
        {
            if sel == from {
                self.inspector.editing_index = Some(to);
            } else if from < to && sel > from && sel <= to {
                self.inspector.editing_index = Some(sel - 1);
            } else if to < from && sel >= to && sel < from {
                self.inspector.editing_index = Some(sel + 1);
            }
        }
    }

    fn step_inspector_detail_item(
        &mut self,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .inspector
            .name_input
            .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
            || self
                .inspector
                .value_input
                .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
        {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        let Some(cur_idx) = self.inspector.editing_index else {
            return;
        };
        let parsed = if let Some((ws_data, _)) = self.websocket_repeater_inspector_data(cx) {
            ws_data
        } else {
            self.inspect_current_http(cx).0
        };
        let items = section_items_slice(&parsed, self.inspector.editing_section);
        let next_idx = cur_idx.checked_add_signed(delta);
        let Some(target_idx) = next_idx.filter(|&i| i < items.len()) else {
            return;
        };
        let item = items[target_idx].clone();
        self.inspector.editing_index = Some(target_idx);
        self.inspector.detail_url_decoded = false;
        self.inspector
            .name_input
            .update(cx, |s, cx| s.set_value(item.name, window, cx));
        self.inspector
            .value_input
            .update(cx, |s, cx| s.set_value(item.value, window, cx));
        cx.notify();
    }

    fn remove_inspector_item(
        &mut self,
        section: InspectorSection,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.inspector.editing_section == section
            && self.inspector.editing_index == Some(index)
            && (self
                .inspector
                .name_input
                .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
                || self
                    .inspector
                    .value_input
                    .update(cx, |s, cx| s.marked_text_range(window, cx).is_some()))
        {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        if self.handle_inspector_mutation(InspectorMutation::Remove { section, index }, window, cx)
            && self.inspector.editing_section == section
        {
            match self.inspector.editing_index {
                Some(k) if k == index => {
                    self.unfocus_inspector_detail_inputs(window, cx);
                    self.inspector.editing_index = None;
                    self.inspector.editor_open = false;
                }
                Some(k) if k > index => {
                    self.inspector.editing_index = Some(k - 1);
                }
                _ => {}
            }
        }
    }

    pub(super) fn switch_inspector_request_protocol(
        &mut self,
        protocol: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .inspector
            .name_input
            .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
            || self
                .inspector
                .value_input
                .update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
        {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        self.inspector.auto_opened_by_selection = false;
        let _ = self.handle_inspector_mutation(
            InspectorMutation::Update {
                section: InspectorSection::RequestAttributes,
                index: 2,
                name: "Protocol".into(),
                value: protocol.to_owned(),
            },
            window,
            cx,
        );
    }

    #[inline(never)]
    fn render_inspector_protocol_row(
        &self,
        proto_item: &InspectorItem,
        editable: bool,
        _subtle_divider: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let proto_trimmed = proto_item.value.trim();
        let is_http2 = proto_trimmed.eq_ignore_ascii_case("HTTP/2")
            || proto_trimmed.eq_ignore_ascii_case("HTTP/2.0");
        let is_http1 = !is_http2
            && (proto_trimmed.eq_ignore_ascii_case("HTTP/1")
                || proto_trimmed.eq_ignore_ascii_case("HTTP/1.1")
                || proto_trimmed.eq_ignore_ascii_case("HTTP/1.0"));
        let inactive_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x35383d } else { 0xdfdfdf }).into();
        let active_bg: gpui_kit::Hsla = rgb(0x26649d).into();
        let active_fg: gpui_kit::Hsla = rgb(0xffffff).into();
        div()
            .id("inspector-protocol-row")
            .test_support()
            .w_full()
            .h(px(41.))
            .pb(px(20.))
            .bg(theme::toolbar_surface())
            .flex()
            .items_center()
            .child(
                div()
                    .w(px(67.))
                    .flex_shrink_0()
                    .text_size(px(13.))
                    .font_weight(gpui_kit::FontWeight::NORMAL)
                    .text_color(theme::foreground())
                    .child("Protocol"),
            )
            .child(
                div()
                    .w(px(112.))
                    .h(px(21.))
                    .flex()
                    .items_center()
                    .gap_0()
                    .child(
                        Button::new("inspector-protocol-HTTP/1")
                            .ghost()
                            .small()
                            .rounded(ButtonRounded::None)
                            .w(px(56.))
                            .h(px(21.))
                            .p_0()
                            .bg(if is_http1 { active_bg } else { inactive_bg })
                            .text_color(if is_http1 { active_fg } else { theme::foreground() })
                            .label("HTTP/1")
                            .accessibility_label("HTTP/1")
                            .tooltip(if editable {
                                "切换请求协议为 HTTP/1.1"
                            } else {
                                "当前请求协议：HTTP/1（只读）"
                            })
                            .selected(is_http1)
                            .toggled(is_http1)
                            .disabled(!editable)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.switch_inspector_request_protocol("HTTP/1.1", window, cx);
                            })),
                    )
                    .child(
                        Button::new("inspector-protocol-HTTP/2")
                            .ghost()
                            .small()
                            .rounded(ButtonRounded::None)
                            .w(px(56.))
                            .h(px(21.))
                            .p_0()
                            .bg(if is_http2 { active_bg } else { inactive_bg })
                            .text_color(if is_http2 { active_fg } else { theme::foreground() })
                            .label("HTTP/2")
                            .accessibility_label("HTTP/2")
                            .tooltip(if editable {
                                "切换请求协议为 HTTP/2"
                            } else {
                                "当前请求协议：HTTP/2（只读）"
                            })
                            .selected(is_http2)
                            .toggled(is_http2)
                            .disabled(!editable)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.switch_inspector_request_protocol("HTTP/2", window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_inspector_section(
        &self,
        section: InspectorSection,
        items: &[InspectorItem],
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let idx = section_index(section);
        let is_expanded = self.inspector.expanded[idx];
        let wrap_enabled = self.inspector.section_wrap[idx];
        let (header_id, title, item_id_prefix, remove_id_prefix, add_target_id) =
            section_meta(section);
        let (move_up_prefix, move_down_prefix) = move_id_prefixes(section);
        let can_remove = editable && section != InspectorSection::RequestAttributes;
        let can_select = editable;
        let (protocol_item, table_items) = request_attributes_split(section, items);
        let count = table_items.len();
        let has_items = protocol_item.is_some() || !table_items.is_empty();
        let add_target = add_target_id.filter(|_| editable);
        let can_expand = has_items || add_target.is_some();
        let show_body = is_expanded && can_expand;
        let subtle_divider: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x34373c } else { 0xebeef2 }).into();
        let pill_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x35383d } else { 0xeeeeee }).into();
        let header_bar =
                div()
                    .w_full()
                    .h(if show_body { px(38.) } else { px(39.) })
                    .min_h(if show_body { px(38.) } else { px(39.) })
                    .flex_shrink_0()
                    .bg(theme::toolbar_surface())
                    .when(!show_body, |b| b.border_b_1().border_color(theme::border()))
                    .flex()
                    .items_center()
                    .child(
                        Button::new(header_id)
                            .ghost()
                            .rounded(ButtonRounded::None)
                            .flex_1()
                            .min_w_0()
                            .h(px(38.))
                            .px(px(10.))
                            .accessibility_label(format!("{title} ({count})"))
                            .tooltip(if !can_expand {
                                SharedString::from(format!("{title}（当前无条目）"))
                            } else if show_body {
                                SharedString::from(format!("折叠 {title}"))
                            } else if has_items {
                                SharedString::from(format!("展开 {title}（共 {count} 项）"))
                            } else {
                                SharedString::from(format!("展开 {title}（可新增条目）"))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.inspector.auto_opened_by_selection = false;
                                if can_expand {
                                    this.inspector.expanded[idx] = !this.inspector.expanded[idx];
                                }
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .w_full()
                                    .relative()
                                    .h(px(18.))
                                    .flex()
                                    .items_center()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .pr(px(80.))
                                            .truncate()
                                            .text_size(px(13.))
                                            .font_weight(gpui_kit::FontWeight::NORMAL)
                                            .text_color(theme::foreground())
                                            .child(title),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .right_0()
                                            .top_0()
                                            .flex()
                                            .h(px(18.))
                                            .items_center()
                                            .gap(px(20.))
                                            .child(
                                                div()
                                                    .w(if count >= 10 { px(32.) } else { px(25.) })
                                                    .h(px(18.))
                                                    .flex_shrink_0()
                                                    .px(px(6.))
                                                    .rounded(px(9.))
                                                    .bg(pill_bg)
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .text_size(px(13.))
                                                    .text_color(theme::foreground())
                                                    .child(count.to_string()),
                                            )
                                            .child(
                                                div()
                                                    .w(px(18.))
                                                    .h(px(18.))
                                                    .flex_shrink_0()
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .child(
                                                        gpui_kit::component::Icon::new(if show_body {
                                                            IconName::ChevronUp
                                                        } else {
                                                            IconName::ChevronDown
                                                        })
                                                        .size(px(14.))
                                                        .text_color(
                                                            if can_expand {
                                                                theme::foreground()
                                                            } else {
                                                                theme::muted_foreground().opacity(0.45)
                                                            },
                                                        ),
                                                    ),
                                            ),
                                    ),
                            ),
                    );
        let mut container = div().flex_shrink_0().flex().flex_col().child(header_bar);
        if show_body {
            let mut body = div()
                .w_full()
                .px(px(10.))
                .pb(px(10.))
                .bg(theme::toolbar_surface())
                .border_b_1()
                .border_color(theme::border())
                .flex()
                .flex_col();
            if let Some(proto_item) = protocol_item {
                body = body.child(self.render_inspector_protocol_row(
                    proto_item,
                    editable,
                    subtle_divider,
                    cx,
                ));
            }
            if !table_items.is_empty() {
                let table_header_bg: gpui_kit::Hsla =
                    rgb(if theme::is_dark() { 0x35383d } else { 0xeeeeee }).into();
                let table_header_fg: gpui_kit::Hsla = theme::foreground();
                body = body.child(
                    div()
                        .id(SharedString::from(format!("{header_id}-table-header")))
                        .test_support()
                        .w_full()
                        .h(px(27.))
                        .bg(table_header_bg)
                        .border_b_1()
                        .border_color(subtle_divider)
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .w(px(143.))
                                .flex_shrink_0()
                                .px(px(8.))
                                .border_r_1()
                                .border_color(subtle_divider)
                                .text_left()
                                .font_family(theme::ui_font_family())
                                .text_size(px(13.))
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(table_header_fg)
                                .child("Name"),
                        )
                        .child(
                            div()
                                .w(px(142.))
                                .flex_shrink_0()
                                .px(px(8.))
                                .border_r_1()
                                .border_color(subtle_divider)
                                .text_left()
                                .font_family(theme::ui_font_family())
                                .text_size(px(13.))
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(table_header_fg)
                                .child("Value"),
                        )
                        .child(
                            Button::new(SharedString::from(format!("{header_id}-wrap-toggle")))
                                .ghost()
                                .small()
                                .compact()
                                .rounded(ButtonRounded::None)
                                .w(px(28.))
                                .h_full()
                                .p_0()
                                .selected(wrap_enabled)
                                .toggled(wrap_enabled)
                                .when(wrap_enabled, |b| b.bg(rgb(0x26649d)))
                                .accessibility_label(format!("切换 {title} 表格自动换行"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.inspector.auto_opened_by_selection = false;
                                    this.inspector.section_wrap[idx] =
                                        !this.inspector.section_wrap[idx];
                                    cx.notify();
                                }))
                                .child(
                                    gpui_kit::component::Icon::default()
                                        .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M2 4h12M2 8h9a2 2 0 0 1 0 4H8m2-2-2 2 2 2M2 12h3\"/></svg>")
                                        .size(px(14.))
                                        .text_color(if wrap_enabled {
                                            gpui_kit::white()
                                        } else {
                                            table_header_fg
                                        }),
                                ),
                        ),
                );
            }
            for (item_idx, item) in table_items.iter().enumerate() {
                body = body.child(self.render_inspector_section_row(
                    section,
                    item,
                    item_idx,
                    count,
                    can_select,
                    can_remove,
                    wrap_enabled,
                    item_id_prefix,
                    move_up_prefix,
                    move_down_prefix,
                    remove_id_prefix,
                    subtle_divider,
                    cx,
                ));
            }
            if let Some(target_id) = add_target {
                body = body.child(self.render_inspector_section_add_bar(
                    section,
                    idx,
                    title,
                    target_id,
                    !table_items.is_empty(),
                    subtle_divider,
                    cx,
                ));
            }
            container = container.child(body);
        }
        container.into_any_element()
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn render_inspector_section_add_bar(
        &self,
        section: InspectorSection,
        section_idx: usize,
        title: &'static str,
        target_id: &'static str,
        has_items: bool,
        subtle_divider: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open_add_drawer = cx.listener(move |this, _, window, cx| {
            if this
                .inspector
                .name_input
                .update(cx, |state, cx| state.marked_text_range(window, cx).is_some())
                || this
                    .inspector
                    .value_input
                    .update(cx, |state, cx| state.marked_text_range(window, cx).is_some())
            {
                this.fail(editing::COMPOSITION_PENDING, cx);
                return;
            }
            this.inspector.auto_opened_by_selection = false;
            this.inspector.editing_section = section;
            this.inspector.editing_index = None;
            this.inspector.editor_open = true;
            this.inspector.detail_url_decoded = false;
            this.inspector.expanded[section_idx] = true;
            this.inspector
                .name_input
                .update(cx, |state, cx| state.set_value("", window, cx));
            this.inspector
                .value_input
                .update(cx, |state, cx| state.set_value("", window, cx));
            cx.notify();
        });
        if has_items {
            div()
                .w_full()
                .py(px(5.))
                .bg(theme::surface())
                .flex()
                .items_center()
                .justify_end()
                .child(
                    Button::new(target_id)
                        .ghost()
                        .small()
                        .compact()
                        .h(px(23.))
                        .px(px(8.))
                        .icon(IconName::Plus)
                        .label("Add")
                        .accessibility_label(format!("新增 {title}"))
                        .tooltip(format!("向 {title} 新增键值对条目"))
                        .on_click(open_add_drawer),
                )
                .into_any_element()
        } else {
            div()
                .w_full()
                .py(px(10.))
                .bg(theme::surface())
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::muted_foreground())
                        .border_b_1()
                        .border_color(subtle_divider)
                        .pb(px(4.))
                        .child("It's empty in here"),
                )
                .child(
                    Button::new(target_id)
                        .small()
                        .compact()
                        .h(px(23.))
                        .px(px(10.))
                        .icon(IconName::Plus)
                        .label("Add")
                        .accessibility_label(format!("新增 {title}"))
                        .tooltip(format!("向 {title} 新增键值对条目"))
                        .on_click(open_add_drawer),
                )
                .into_any_element()
        }
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn render_inspector_row_main_button(
        &self,
        section: InspectorSection,
        item_idx: usize,
        name: String,
        value: String,
        can_select: bool,
        is_selected: bool,
        wrap_enabled: bool,
        item_id_prefix: &'static str,
        subtle_divider: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = format!("{name}: {value}");
        let tip = if can_select {
            format!("{name}: {value}\n（点击在底部编辑此条目，再次点击收起）")
        } else {
            format!("{name}: {value}\n（点击查看完整键值或复制，再次点击收起）")
        };
        let name_display = name.clone();
        let value_display = value.clone();
        Button::new(SharedString::from(format!("{item_id_prefix}-{item_idx}")))
            .ghost()
            .rounded(ButtonRounded::None)
            .flex_1()
            .min_w_0()
            .map(|this| {
                if wrap_enabled {
                    this.h_auto().min_h(px(25.))
                } else {
                    this.h(px(25.))
                }
            })
            .px_0()
            .selected(is_selected)
            .when(is_selected, |b| b.bg(theme::selected()))
            .accessibility_label(label)
            .tooltip(tip)
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.inspector.name_input.update(cx, |s, cx| {
                    s.marked_text_range(window, cx).is_some()
                }) || this.inspector.value_input.update(cx, |s, cx| {
                    s.marked_text_range(window, cx).is_some()
                }) {
                    this.fail(editing::COMPOSITION_PENDING, cx);
                    return;
                }
                this.inspector.auto_opened_by_selection = false;
                if is_selected {
                    this.unfocus_inspector_detail_inputs(window, cx);
                    this.inspector.editing_index = None;
                    this.inspector.editor_open = false;
                    this.inspector.detail_url_decoded = false;
                    cx.notify();
                    return;
                }
                this.inspector.editing_section = section;
                this.inspector.editing_index = Some(item_idx);
                this.inspector.editor_open = true;
                this.inspector.detail_url_decoded = false;
                let name = name.clone();
                let value = value.clone();
                this.inspector
                    .name_input
                    .update(cx, |state, cx| state.set_value(name, window, cx));
                this.inspector
                    .value_input
                    .update(cx, |state, cx| state.set_value(value, window, cx));
                cx.notify();
            }))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .map(|this| {
                        if wrap_enabled {
                            this.min_h(px(25.)).items_stretch()
                        } else {
                            this.h_full().items_center()
                        }
                    })
                    .flex()
                    .child(
                        div()
                            .w(px(143.))
                            .flex_shrink_0()
                            .px(px(8.))
                            .border_r_1()
                            .border_color(subtle_divider)
                            .flex()
                            .items_center()
                            .map(|this| {
                                if wrap_enabled {
                                    this.min_h(px(25.)).py(px(4.)).whitespace_normal()
                                } else {
                                    this.h_full().truncate()
                                }
                            })
                            .text_left()
                            .font_family(theme::ui_font_family())
                            .text_size(px(13.))
                            .font_weight(gpui_kit::FontWeight::NORMAL)
                            .text_color(theme::foreground())
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .map(|this| {
                                        if wrap_enabled {
                                            this.whitespace_normal()
                                        } else {
                                            this.truncate()
                                        }
                                    })
                                    .child(name_display),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pl(px(4.))
                            .pr(px(2.))
                            .flex()
                            .items_center()
                            .map(|this| {
                                if wrap_enabled {
                                    this.min_h(px(25.)).py(px(4.)).whitespace_normal()
                                } else {
                                    this.h_full().truncate()
                                }
                            })
                            .text_left()
                            .font_family(theme::ui_font_family())
                            .text_size(px(13.))
                            .font_weight(gpui_kit::FontWeight::NORMAL)
                            .text_color(theme::foreground())
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .map(|this| {
                                        if wrap_enabled {
                                            this.whitespace_normal()
                                        } else {
                                            this.truncate()
                                        }
                                    })
                                    .child(value_display),
                            ),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn render_inspector_row_actions(
        &self,
        section: InspectorSection,
        item_idx: usize,
        count: usize,
        can_remove: bool,
        item_id_prefix: &'static str,
        move_up_prefix: &'static str,
        move_down_prefix: &'static str,
        remove_id_prefix: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_first = item_idx == 0;
        let is_last = item_idx + 1 == count;
        let mut actions_col = div()
            .h_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_end();
        if can_remove {
            actions_col = actions_col
                .child(
                    Button::new(SharedString::from(format!(
                        "{move_up_prefix}-{item_idx}"
                    )))
                    .ghost()
                    .small()
                    .compact()
                    .w(theme::scaled_px(20.))
                    .h(theme::scaled_px(20.))
                    .icon(IconName::ChevronUp)
                    .accessibility_label("上移")
                    .tooltip("将此条目向上移动一行并同步调整原始报文字节顺序")
                    .disabled(is_first)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.inspector.auto_opened_by_selection = false;
                        if item_idx > 0 {
                            this.move_inspector_item(section, item_idx, item_idx - 1, window, cx);
                        }
                    })),
                )
                .child(
                    Button::new(SharedString::from(format!(
                        "{move_down_prefix}-{item_idx}"
                    )))
                    .ghost()
                    .small()
                    .compact()
                    .w(theme::scaled_px(20.))
                    .h(theme::scaled_px(20.))
                    .icon(IconName::ChevronDown)
                    .accessibility_label("下移")
                    .tooltip("将此条目向下移动一行并同步调整原始报文字节顺序")
                    .disabled(is_last)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.inspector.auto_opened_by_selection = false;
                        if !is_last {
                            this.move_inspector_item(section, item_idx, item_idx + 1, window, cx);
                        }
                    })),
                )
                .child(
                    Button::new(SharedString::from(format!(
                        "{remove_id_prefix}-{item_idx}"
                    )))
                    .ghost()
                    .small()
                    .compact()
                    .w(theme::scaled_px(22.))
                    .h(theme::scaled_px(22.))
                    .icon(IconName::Trash)
                    .accessibility_label("删除")
                    .tooltip("从当前报文中移除此条目并同步更新原始字节")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.inspector.auto_opened_by_selection = false;
                        this.remove_inspector_item(section, item_idx, window, cx);
                    })),
                );
        } else {
            actions_col = actions_col.w(px(28.));
        }
        actions_col
            .child(
                div()
                    .id(SharedString::from(format!(
                        "{item_id_prefix}-chevron-{item_idx}"
                    )))
                    .test_support()
                    .w(px(14.))
                    .h(px(14.))
                    .ml(px(2.))
                    .mr(px(6.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        gpui_kit::component::Icon::new(IconName::ChevronRight)
                            .size(px(14.))
                            .text_color(theme::foreground()),
                    ),
            )
            .into_any_element()
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn render_inspector_section_row(
        &self,
        section: InspectorSection,
        item: &InspectorItem,
        item_idx: usize,
        count: usize,
        can_select: bool,
        can_remove: bool,
        wrap_enabled: bool,
        item_id_prefix: &'static str,
        move_up_prefix: &'static str,
        move_down_prefix: &'static str,
        remove_id_prefix: &'static str,
        subtle_divider: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_selected = self.inspector.editor_open
            && self.inspector.editing_section == section
            && self.inspector.editing_index == Some(item_idx);
        div()
            .w_full()
            .map(|this| {
                if wrap_enabled {
                    this.min_h(px(25.))
                } else {
                    this.h(px(25.))
                }
            })
            .flex()
            .items_center()
            .bg(if is_selected {
                theme::selected()
            } else {
                theme::surface()
            })
            .child(self.render_inspector_row_main_button(
                section,
                item_idx,
                item.name.clone(),
                item.value.clone(),
                can_select,
                is_selected,
                wrap_enabled,
                item_id_prefix,
                subtle_divider,
                cx,
            ))
            .child(self.render_inspector_row_actions(
                section,
                item_idx,
                count,
                can_remove,
                item_id_prefix,
                move_up_prefix,
                move_down_prefix,
                remove_id_prefix,
                cx,
            ))
            .into_any_element()
    }

    #[inline(never)]
    fn render_inspector_detail_drawer(
        &self,
        any_editable: bool,
        req_editable: bool,
        resp_editable: bool,
        parsed_data: Option<&InspectorData>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.inspector.editor_open {
            return None;
        }
        let section = if self.inspector.editing_index.is_some() || !any_editable {
            self.inspector.editing_section
        } else {
            self.effective_inspector_section_for(req_editable, resp_editable)
        };
        let section_len = parsed_data.map_or(0, |d| section_items_slice(d, section).len());
        let has_selection = self.inspector.editing_section == section
            && self
                .inspector
                .editing_index
                .is_some_and(|idx| idx < section_len);
        let section_editable = match section {
            InspectorSection::ResponseHeaders => resp_editable,
            _ => req_editable,
        };
        if !section_editable && !has_selection {
            return None;
        }
        let title = if section_editable {
            if has_selection {
                format!("编辑 · {}", section_singular_label(section))
            } else {
                format!("新增 · {}", section_singular_label(section))
            }
        } else {
            format!("键值详情 · {}", section_singular_label(section))
        };
        let current_idx = self.inspector.editing_index.filter(|_| has_selection);
        let can_prev = current_idx.is_some_and(|i| i > 0);
        let can_next = current_idx.is_some_and(|i| i + 1 < section_len);
        let cur_name_val = self.inspector.name_input.read(cx).value().to_string();
        let cur_value_val = self.inspector.value_input.read(cx).value().to_string();
        let raw_item_value = current_idx
            .and_then(|idx| parsed_data.and_then(|d| section_items_slice(d, section).get(idx)))
            .map(|it| it.value.clone());
        let supports_url_toggle = matches!(
            section,
            InspectorSection::QueryParams | InspectorSection::BodyParams | InspectorSection::Cookies
        ) && (self.inspector.detail_url_decoded
            || cur_value_val.contains('%')
            || cur_value_val.contains('+')
            || raw_item_value
                .as_deref()
                .is_some_and(|v| v.contains('%') || v.contains('+')));
        let readonly_box_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x232529 } else { 0xf6f7f9 }).into();
        let subtle_border: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x34373c } else { 0xdde1e7 }).into();
        Some(
            div()
                .id("inspector-detail-drawer")
                .test_support()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(theme::border())
                .bg(theme::surface())
                .when(section_editable, |drawer| {
                    drawer.child(
                        div()
                            .w_full()
                            .min_h(theme::scaled_px(26.))
                            .px(px(8.))
                            .bg(theme::toolbar_surface())
                            .border_b_1()
                            .border_color(theme::border())
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(theme::scaled_px(11.5))
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .text_color(theme::foreground())
                                    .child(title),
                            )
                            .when(has_selection && section_len > 1, |bar| {
                                bar.child(
                                    Button::new("inspector-prev-item")
                                        .ghost()
                                        .small()
                                        .compact()
                                        .w(theme::scaled_px(20.))
                                        .h(theme::scaled_px(20.))
                                        .icon(IconName::ChevronLeft)
                                        .accessibility_label("上一项")
                                        .tooltip("切换到当前分组中的上一项")
                                        .disabled(!can_prev)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.step_inspector_detail_item(-1, window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("inspector-next-item")
                                        .ghost()
                                        .small()
                                        .compact()
                                        .w(theme::scaled_px(20.))
                                        .h(theme::scaled_px(20.))
                                        .icon(IconName::ChevronRight)
                                        .accessibility_label("下一项")
                                        .tooltip("切换到当前分组中的下一项")
                                        .disabled(!can_next)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.step_inspector_detail_item(1, window, cx);
                                        })),
                                )
                            })
                            .child(div().flex_1())
                            .when(supports_url_toggle, |bar| {
                                let url_decoded = self.inspector.detail_url_decoded;
                                bar.child(
                                    Button::new("inspector-detail-url-decode-toggle")
                                        .ghost()
                                        .small()
                                        .compact()
                                        .h(theme::scaled_px(21.))
                                        .px(px(5.))
                                        .label("URL 解码")
                                        .selected(url_decoded)
                                        .tooltip(if url_decoded {
                                            "切换回 URL 原始编码视图"
                                        } else {
                                            "将当前参数值按 URL 解码展示（保存时自动重编码）"
                                        })
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            let Ok(current) = editing::input_snapshot(
                                                &this.inspector.value_input,
                                                window,
                                                cx,
                                            ) else {
                                                this.fail(editing::COMPOSITION_PENDING, cx);
                                                return;
                                            };
                                            let current = current.to_string();
                                            if this.inspector.detail_url_decoded {
                                                if let Ok(enc) = editing::convert_selection(
                                                    editing::ConvertKind::UrlEncodeKey,
                                                    &current,
                                                ) {
                                                    this.inspector.value_input.update(
                                                        cx,
                                                        |s, cx| s.set_value(enc, window, cx),
                                                    );
                                                    this.inspector.detail_url_decoded = false;
                                                }
                                            } else if let Ok(dec) = editing::convert_selection(
                                                editing::ConvertKind::UrlDecode,
                                                &current,
                                            ) {
                                                this.inspector
                                                    .value_input
                                                    .update(cx, |s, cx| s.set_value(dec, window, cx));
                                                this.inspector.detail_url_decoded = true;
                                            }
                                            cx.notify();
                                        })),
                                )
                            })
                            .child(self.render_inspector_detail_action_buttons(cx)),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(5.))
                        .p(px(8.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .w(theme::scaled_px(40.))
                                        .flex_shrink_0()
                                        .text_size(theme::scaled_px(11.5))
                                        .text_color(theme::muted_foreground())
                                        .child("Name"),
                                )
                                .child(div().flex_1().min_w_0().child(if section_editable {
                                    Input::new(&self.inspector.name_input)
                                        .id("inspector-name-input")
                                        .aria_label("名称")
                                        .disabled(section == InspectorSection::RequestAttributes)
                                        .small()
                                        .into_any_element()
                                } else {
                                    div()
                                        .id("inspector-readonly-name")
                                        .test_support()
                                        .w_full()
                                        .max_h(theme::scaled_px(48.))
                                        .overflow_y_scroll()
                                        .px(px(7.))
                                        .py(px(4.))
                                        .rounded(px(4.))
                                        .bg(readonly_box_bg)
                                        .border_1()
                                        .border_color(subtle_border)
                                        .font_family(theme::mono_font_family())
                                        .text_size(theme::scaled_px(11.5))
                                        .text_color(theme::foreground())
                                        .child(cur_name_val.clone())
                                        .into_any_element()
                                }))
                                .when(!section_editable, |row| {
                                    row.child(self.render_inspector_detail_action_buttons(cx))
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .w(theme::scaled_px(40.))
                                        .pt(px(4.))
                                        .flex_shrink_0()
                                        .text_size(theme::scaled_px(11.5))
                                        .text_color(theme::muted_foreground())
                                        .child("Value"),
                                )
                                .child(div().flex_1().min_w_0().child(if section_editable {
                                    Input::new(&self.inspector.value_input)
                                        .id("inspector-value-input")
                                        .aria_label("值")
                                        .small()
                                        .into_any_element()
                                } else {
                                    div()
                                        .id("inspector-readonly-value")
                                        .test_support()
                                        .w_full()
                                        .max_h(theme::scaled_px(132.))
                                        .overflow_y_scroll()
                                        .px(px(7.))
                                        .py(px(5.))
                                        .rounded(px(4.))
                                        .bg(readonly_box_bg)
                                        .border_1()
                                        .border_color(subtle_border)
                                        .font_family(theme::mono_font_family())
                                        .text_size(theme::scaled_px(11.5))
                                        .text_color(theme::foreground())
                                        .child(cur_value_val.clone())
                                        .into_any_element()
                                })),
                        )
                        .when(section_editable, |form| {
                            form.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .gap(px(6.))
                                    .pt(px(2.))
                                    .when(section != InspectorSection::RequestAttributes, |row| {
                                        row.child(
                                            button("inspector-add-button", "新增")
                                                .icon(IconName::Plus)
                                                .tooltip("作为新条目追加到当前报文分组")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.add_inspector_item(window, cx)
                                                })),
                                        )
                                    })
                                    .child(
                                        button("inspector-apply-button", "保存修改")
                                            .primary()
                                            .icon(IconName::Check)
                                            .tooltip("将修改后的名称与值写回报文中当前选中的条目")
                                            .disabled(!has_selection)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.update_inspector_item(window, cx)
                                            })),
                                    ),
                            )
                        }),
                )
                .into_any_element()
        )
    }

    #[inline(never)]
    fn render_inspector_detail_action_buttons(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                Button::new("inspector-copy-name")
                    .ghost()
                    .small()
                    .compact()
                    .h(theme::scaled_px(21.))
                    .px(px(5.))
                    .icon(IconName::Copy)
                    .label("复制名称")
                    .tooltip("复制当前条目的完整 Name 到剪贴板")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let name = this.inspector.name_input.read(cx).value().to_string();
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(name));
                        this.status = "已复制 Inspector 条目名称到剪贴板。".into();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("inspector-copy-value")
                    .ghost()
                    .small()
                    .compact()
                    .h(theme::scaled_px(21.))
                    .px(px(5.))
                    .icon(IconName::Copy)
                    .label("复制值")
                    .tooltip("复制当前条目的完整 Value 到剪贴板")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let val = this.inspector.value_input.read(cx).value().to_string();
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(val));
                        this.status = "已复制 Inspector 条目值到剪贴板。".into();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("inspector-editor-close")
                    .ghost()
                    .small()
                    .compact()
                    .w(theme::scaled_px(20.))
                    .h(theme::scaled_px(20.))
                    .icon(IconName::Close)
                    .accessibility_label("关闭条目面板")
                    .tooltip("收起底部键值面板")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.inspector.name_input.update(cx, |s, cx| {
                            s.marked_text_range(window, cx).is_some()
                        }) || this.inspector.value_input.update(cx, |s, cx| {
                            s.marked_text_range(window, cx).is_some()
                        }) {
                            this.fail(editing::COMPOSITION_PENDING, cx);
                            return;
                        }
                        this.unfocus_inspector_detail_inputs(window, cx);
                        this.inspector.editor_open = false;
                        this.inspector.editing_index = None;
                        this.inspector.detail_url_decoded = false;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_inspector_title_bar(&self, show_http: bool, cx: &mut Context<Self>) -> AnyElement {
        let dock_left = self.inspector.dock_left;
        let inactive_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x35383d } else { 0xdfdfdf }).into();
        div()
            .id("inspector-title-bar")
            .test_support()
            .w_full()
            .h(px(40.))
            .min_h(px(40.))
            .max_h(px(40.))
            .px(px(10.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(10.))
            .bg(theme::toolbar_surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .text_color(theme::foreground())
                    .child("Inspector"),
            )
            .child(div().flex_1())
            .child(
                div()
                    .w(px(50.))
                    .h(px(19.))
                    .flex()
                    .items_center()
                    .gap_0()
                    .child(
                        Button::new("inspector-dock-left")
                            .ghost()
                            .small()
                            .rounded(ButtonRounded::None)
                            .w(px(25.))
                            .h(px(19.))
                            .p_0()
                            .bg(if dock_left { rgb(0x26649d).into() } else { inactive_bg })
                            .text_color(if dock_left { rgb(0xffffff).into() } else { theme::foreground() })
                            .icon(
                                gpui_kit::component::Icon::default()
                                    .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 15 13\" fill=\"none\"><rect x=\"0.75\" y=\"1.25\" width=\"13.5\" height=\"10.5\" rx=\"1\" stroke=\"currentColor\" stroke-width=\"1.2\"/><rect x=\"1.2\" y=\"1.7\" width=\"5.2\" height=\"9.6\" fill=\"currentColor\"/></svg>")
                                    .size(px(14.))
                                    .text_color(if dock_left { rgb(0xffffff).into() } else { theme::foreground() }),
                            )
                            .accessibility_label("Dock on the left")
                            .tooltip("Dock on the left")
                            .selected(dock_left)
                            .toggled(dock_left)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.inspector.dock_left = true;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("inspector-dock-right")
                            .ghost()
                            .small()
                            .rounded(ButtonRounded::None)
                            .w(px(25.))
                            .h(px(19.))
                            .p_0()
                            .bg(if !dock_left { rgb(0x26649d).into() } else { inactive_bg })
                            .text_color(if !dock_left { rgb(0xffffff).into() } else { theme::foreground() })
                            .icon(
                                gpui_kit::component::Icon::default()
                                    .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 15 13\" fill=\"none\"><rect x=\"0.75\" y=\"1.25\" width=\"13.5\" height=\"10.5\" rx=\"1\" stroke=\"currentColor\" stroke-width=\"1.2\"/><rect x=\"8.6\" y=\"1.7\" width=\"5.2\" height=\"9.6\" fill=\"currentColor\"/></svg>")
                                    .size(px(14.))
                                    .text_color(if !dock_left { rgb(0xffffff).into() } else { theme::foreground() }),
                            )
                            .accessibility_label("Dock on the right")
                            .tooltip("Dock on the right")
                            .selected(!dock_left)
                            .toggled(!dock_left)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.inspector.dock_left = false;
                                cx.notify();
                            })),
                    ),
            )
            .when(show_http, |bar| {
                bar.child(
                    Button::new("inspector-expand-all")
                        .ghost()
                        .small()
                        .min_w(px(18.))
                        .max_w(px(18.))
                        .min_h(px(18.))
                        .max_h(px(18.))
                        .w(px(18.))
                        .h(px(18.))
                        .p_0()
                        .child(
                            gpui_kit::component::Icon::default()
                                .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.45\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M2.5 2.5h11M2.5 13.5h11M8 4.8v6.4M5.6 7.1L8 4.8l2.4 2.3M5.6 8.9L8 11.2l2.4-2.3\"/></svg>")
                                .size(px(15.))
                                .text_color(theme::foreground()),
                        )
                        .accessibility_label("全部展开")
                        .tooltip("展开全部非空检视分组")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.inspector.auto_opened_by_selection = false;
                            let parsed = if let Some((ws_data, _)) =
                                this.websocket_repeater_inspector_data(cx)
                            {
                                ws_data
                            } else {
                                this.inspect_current_http(cx).0
                            };
                            this.inspector.expanded = [
                                !parsed.attributes.is_empty(),
                                !parsed.query_params.is_empty(),
                                !parsed.body_params.is_empty(),
                                !parsed.cookies.is_empty(),
                                !parsed.request_headers.is_empty(),
                                !parsed.response_headers.is_empty(),
                            ];
                            this.inspector.selection_expanded = true;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("inspector-collapse-all")
                        .ghost()
                        .small()
                        .min_w(px(18.))
                        .max_w(px(18.))
                        .min_h(px(18.))
                        .max_h(px(18.))
                        .w(px(18.))
                        .h(px(18.))
                        .p_0()
                        .child(
                            gpui_kit::component::Icon::default()
                                .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.45\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M2.5 8h11M8 2.2v3.6M5.8 3.8L8 5.8l2.2-2M8 13.8v-3.6M5.8 12.2L8 10.2l2.2 2\"/></svg>")
                                .size(px(15.))
                                .text_color(theme::foreground()),
                        )
                        .accessibility_label("全部收起")
                        .tooltip("折叠全部检视分组")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.inspector.auto_opened_by_selection = false;
                            this.inspector.expanded = [false; 6];
                            this.inspector.selection_expanded = false;
                            cx.notify();
                        })),
                )
            })
            .child(
                Button::new("inspector-close")
                    .ghost()
                    .small()
                    .min_w(px(18.))
                    .max_w(px(18.))
                    .min_h(px(18.))
                    .max_h(px(18.))
                    .w(px(18.))
                    .h(px(18.))
                    .p_0()
                    .child(
                        gpui_kit::component::Icon::default()
                            .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.65\" stroke-linecap=\"round\"><path d=\"M3.2 3.2l9.6 9.6M12.8 3.2l-9.6 9.6\"/></svg>")
                            .size(px(14.))
                            .text_color(theme::foreground()),
                    )
                    .accessibility_label("收起 Inspector")
                    .tooltip("收起 Inspector 检视面板")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.dismiss_inspector_for_current_selection(window, cx) {
                            cx.notify();
                        }
                    })),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_inspector_sections_area(
        &self,
        parsed_http: Option<&InspectorData>,
        is_ws: bool,
        req_editable: bool,
        resp_editable: bool,
        any_editable: bool,
        has_selection_preview: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut sections = div()
            .id("inspector-sections")
            .test_support()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.inspector.sections_scroll_handle)
            .flex()
            .flex_col()
            .bg(theme::toolbar_surface())
            .child(self.render_inspector_selection_section(any_editable, cx));

        if is_ws && let Some(data) = parsed_http {
            sections = sections
                .when(req_editable || !data.attributes.is_empty(), |s| {
                    s.child(self.render_inspector_section(
                        InspectorSection::RequestAttributes,
                        &data.attributes,
                        req_editable,
                        cx,
                    ))
                })
                .when(req_editable || !data.request_headers.is_empty(), |s| {
                    s.child(self.render_inspector_section(
                        InspectorSection::RequestHeaders,
                        &data.request_headers,
                        req_editable,
                        cx,
                    ))
                });
        } else if let Some(data) = parsed_http {
            sections = sections
                .when(req_editable || !data.attributes.is_empty(), |s| {
                    s.child(self.render_inspector_section(
                        InspectorSection::RequestAttributes,
                        &data.attributes,
                        req_editable,
                        cx,
                    ))
                })
                .when(req_editable || !data.query_params.is_empty(), |s| {
                    s.child(self.render_inspector_section(
                        InspectorSection::QueryParams,
                        &data.query_params,
                        req_editable,
                        cx,
                    ))
                })
                .when(req_editable || !data.body_params.is_empty(), |s| {
                    s.child(self.render_inspector_section(
                        InspectorSection::BodyParams,
                        &data.body_params,
                        req_editable,
                        cx,
                    ))
                })
                .when(req_editable || !data.cookies.is_empty(), |s| {
                    s.child(self.render_inspector_section(
                        InspectorSection::Cookies,
                        &data.cookies,
                        req_editable,
                        cx,
                    ))
                })
                .when(req_editable || !data.request_headers.is_empty(), |s| {
                    s.child(self.render_inspector_section(
                        InspectorSection::RequestHeaders,
                        &data.request_headers,
                        req_editable,
                        cx,
                    ))
                })
                .when(resp_editable || !data.response_headers.is_empty(), |s| {
                    s.child(self.render_inspector_section(
                        InspectorSection::ResponseHeaders,
                        &data.response_headers,
                        resp_editable,
                        cx,
                    ))
                });
        } else if !has_selection_preview {
            sections = sections.child(
                div()
                    .p(px(14.))
                    .text_size(theme::scaled_px(12.))
                    .text_color(theme::muted_foreground())
                    .child("在编辑器中框选文本或字节，即可在此实时解码检视。"),
            );
        }

        let section_visible_and_expanded = |idx: usize, has_items: bool, has_add: bool| -> bool {
            self.inspector.expanded[idx] && (has_items || has_add)
        };
        let any_section_expanded = if is_ws {
            parsed_http.is_some_and(|data| {
                section_visible_and_expanded(0, !data.attributes.is_empty(), false)
                    || section_visible_and_expanded(4, !data.request_headers.is_empty(), req_editable)
            })
        } else {
            parsed_http.is_some_and(|data| {
                section_visible_and_expanded(0, !data.attributes.is_empty(), false)
                    || section_visible_and_expanded(1, !data.query_params.is_empty(), req_editable)
                    || section_visible_and_expanded(2, !data.body_params.is_empty(), req_editable)
                    || section_visible_and_expanded(3, !data.cookies.is_empty(), req_editable)
                    || section_visible_and_expanded(4, !data.request_headers.is_empty(), req_editable)
                    || section_visible_and_expanded(5, !data.response_headers.is_empty(), resp_editable)
            })
        };
        let any_expanded =
            (has_selection_preview && self.inspector.selection_expanded) || any_section_expanded;
        let scrollbar_track_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x2b2d30 } else { 0xf7f7f7 }).into();
        let scrollbar_thumb_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x5c626b } else { 0xc8c8c8 }).into();
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .bg(theme::toolbar_surface())
            .child(sections)
            .when(any_expanded, |row| {
                row.child(
                    div()
                        .id("inspector-sections-scrollbar")
                        .test_support()
                        .relative()
                        .w(px(10.))
                        .h_full()
                        .flex_shrink_0()
                        .bg(scrollbar_track_bg)
                        .child(
                            gpui_kit::component::scroll::Scrollbar::vertical(
                                &self.inspector.sections_scroll_handle,
                            )
                            .viewport_from_layout()
                            .mode(gpui_kit::component::scroll::ScrollbarMode::Always)
                            .styles(move |styles| {
                                styles
                                    .track(|s| s.width(px(10.)).bg(scrollbar_track_bg))
                                    .track_hover(|s| s.width(px(10.)).bg(scrollbar_track_bg))
                                    .track_active(|s| s.width(px(10.)).bg(scrollbar_track_bg))
                                    .thumb(|s| {
                                        s.width(px(6.))
                                            .inset(px(2.))
                                            .radius(px(3.))
                                            .min_length(px(24.))
                                            .bg(scrollbar_thumb_bg)
                                    })
                                    .thumb_hover(|s| {
                                        s.width(px(6.))
                                            .inset(px(2.))
                                            .radius(px(3.))
                                            .min_length(px(24.))
                                            .bg(scrollbar_thumb_bg)
                                    })
                                    .thumb_active(|s| {
                                        s.width(px(6.))
                                            .inset(px(2.))
                                            .radius(px(3.))
                                            .min_length(px(24.))
                                            .bg(scrollbar_thumb_bg)
                                    })
                            })
                            .max_fps(60),
                        ),
                )
            })
            .into_any_element()
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_inspector_sidebar(
        &self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let show_http = self.inspector_supports_http_sections();
        let ws_inspector = self.websocket_repeater_inspector_data(cx);
        let (parsed_http, req_editable, resp_editable) = if show_http {
            if let Some((ws_data, ws_editable)) = &ws_inspector {
                (Some(ws_data.clone()), *ws_editable, false)
            } else {
                let (data, req_ed, resp_ed) = self.inspect_current_http(cx);
                (Some(data), req_ed, resp_ed)
            }
        } else {
            (None, false, false)
        };
        let selection_writable = self
            .inspector
            .selection_identity
            .as_ref()
            .is_some_and(|id| id.writable);
        let any_editable = req_editable || resp_editable || selection_writable;
        let has_selection_preview = self.inspector.selection_preview.is_some();
        let detail_drawer = self.render_inspector_detail_drawer(
            any_editable,
            req_editable,
            resp_editable,
            parsed_http.as_deref(),
            cx,
        );
        let title_bar = self.render_inspector_title_bar(show_http, cx);
        let sections_with_scrollbar = self.render_inspector_sections_area(
            parsed_http.as_deref(),
            ws_inspector.is_some(),
            req_editable,
            resp_editable,
            any_editable,
            has_selection_preview,
            cx,
        );

        div()
            .id("inspector-sidebar")
            .test_support()
            .w(px(343.))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme::toolbar_surface())
            .child(title_bar)
            .child(sections_with_scrollbar)
            .when_some(detail_drawer, |sidebar, drawer| sidebar.child(drawer))
            .into_any_element()
    }
}
