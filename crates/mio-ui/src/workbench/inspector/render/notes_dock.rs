use super::*;
use crate::workbench::http_notes::render::{
    NoteMarkdownAction, format_markdown_snippet, render_markdown_preview,
};
use crate::workbench::wb_render_chrome::sidebar_rail_tab_with_dock;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonVariants;

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
        if text.len() - range.len() + replacement.len() > super::super::MAX_TAB_NOTE_BYTES {
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
}
