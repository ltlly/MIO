use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonVariants;

impl Workbench {
    pub(in crate::workbench) fn leave_websocket_notes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Err(error) = self.capture_websocket_notes(window, cx) {
            self.websocket.notes.error = Some(error.clone());
            self.fail(error, cx);
            return false;
        }
        self.send_websocket_notes(false, cx);
        true
    }

    pub(in crate::workbench) fn select_websocket_notes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.websocket.notes.open {
            return;
        }
        let Some(id) = self.websocket.selected else {
            return;
        };
        if self.websocket.notes.active == Some(id)
            && (self.websocket.notes.stored.is_some() || self.websocket.notes.reading)
        {
            return;
        }
        if self
            .websocket
            .notes
            .editor
            .focus_handle(cx)
            .is_focused(window)
        {
            self.focus.focus(window, cx);
        }
        let Some(project) = self.project else { return };
        let notes = &mut self.websocket.notes;
        notes.active = Some(id);
        notes.stored = None;
        notes.error = None;
        notes.read_revision = match notes.read_revision.checked_add(1) {
            Some(next) => next,
            None => {
                notes.error = Some("备注读取 revision 已耗尽，请重新打开项目。".into());
                return;
            }
        };
        if let Some(draft) = notes.drafts.get(&id) {
            notes.stored = Some(draft.stored.clone());
            notes.error = draft.error.clone();
            let text = draft.text.clone();
            notes
                .editor
                .update(cx, |editor, cx| editor.set_value(text, window, cx));
            notes.reading = false;
        } else {
            let revision = notes.read_revision;
            notes.reading = true;
            notes
                .editor
                .update(cx, |editor, cx| editor.set_value("", window, cx));
            if !self.enqueue(
                Command::ReadWebSocketNotes {
                    project,
                    selection_revision: revision,
                    message_id: id,
                },
                cx,
            ) {
                self.websocket.notes.reading = false;
                self.websocket.notes.error = self.error.clone();
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn on_websocket_notes_event(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            Event::WebSocketNotes {
                project,
                selection_revision,
                message_id,
                result,
            } if self.project == Some(project)
                && self.websocket.notes.active == Some(message_id)
                && self.websocket.notes.read_revision == selection_revision =>
            {
                let notes = &mut self.websocket.notes;
                notes.reading = false;
                match result {
                    Ok(stored) => {
                        let text = stored.text.clone();
                        notes.stored = Some(stored);
                        notes.error = None;
                        notes
                            .editor
                            .update(cx, |editor, cx| editor.set_value(text, window, cx));
                    }
                    Err(error) => notes.error = Some(error),
                }
            }
            Event::WebSocketNotesChanged {
                project,
                operation_id,
                message_id,
                result,
            } if self.project == Some(project) => {
                self.receive_websocket_notes(message_id, operation_id, result, window, cx)
            }
            _ => {}
        }
        cx.notify();
    }

    fn receive_websocket_notes(
        &mut self,
        id: Uuid,
        operation: Uuid,
        result: Result<WebSocketNotes, WebSocketNotesError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .websocket
            .notes
            .drafts
            .get(&id)
            .and_then(|draft| draft.pending.as_ref())
            .is_none_or(|pending| pending.operation != operation)
        {
            return;
        }
        let _ = self.capture_websocket_notes(window, cx);
        let draft = self.websocket.notes.drafts.get_mut(&id).unwrap();
        let pending = draft.pending.take().unwrap();
        match result {
            Ok(stored)
                if (stored.revision == draft.stored.revision
                    || Some(stored.revision) == draft.stored.revision.checked_add(1))
                    && stored.text == pending.text =>
            {
                draft.stored = stored.clone();
                draft.error = None;
                if self.websocket.notes.active == Some(id) {
                    self.websocket.notes.stored = Some(stored.clone());
                    self.websocket.notes.error = None;
                }
                if draft.text == stored.text {
                    self.websocket.notes.drafts.remove(&id);
                }
                self.update_websocket_notes_summary(id, &stored, cx);
                self.websocket.dirty = true;
                self.send_websocket_notes(false, cx);
            }
            Ok(_) => {
                draft.error =
                    Some("备注保存回执内容或 revision 不匹配；草稿保留，请重新读取后重试。".into())
            }
            Err(error) => {
                if let Some(stored) = error.stored {
                    if self.websocket.notes.active == Some(id) {
                        self.websocket.notes.stored = Some(stored.clone());
                    }
                    draft.stored = stored;
                }
                draft.error = Some(error.message);
            }
        }
    }

    pub(in crate::workbench) fn render_websocket_notes(
        &self,
        body: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let has_selected = self.websocket.selected.is_some();
        let notes = &self.websocket.notes;
        let notes_open = has_selected && notes.open;
        let inspector_open = self.inspector.open && !notes_open;
        let active = notes.active == self.websocket.selected && notes.active.is_some();
        let error = notes
            .active
            .and_then(|id| notes.drafts.get(&id))
            .and_then(|draft| draft.error.clone())
            .or_else(|| notes.error.clone());
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
            .when(inspector_open, |this| {
                this.child(self.render_inspector_sidebar(cx))
            })
            .when(notes_open, |this| {
                this.child(
                    div()
                        .w(theme::scaled_px(336.).clamp(px(288.), px(420.)))
                        .max_w(px(420.))
                        .flex_shrink_0()
                        .h_full()
                        .border_l_1()
                        .border_color(theme::border())
                        .bg(theme::surface())
                        .flex()
                        .flex_col()
                        .child(
                            toolbar()
                                .gap(px(4.))
                                .child(
                                    gpui_kit::component::Icon::new(IconName::FileText)
                                        .size(theme::scaled_px(13.))
                                        .text_color(theme::accent()),
                                )
                                .child(
                                    div()
                                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                        .child("Notes"),
                                )
                                .child(div().flex_1())
                                .child(
                                    Button::new("websocket-notes-close")
                                        .ghost()
                                        .small()
                                        .compact()
                                        .w(theme::scaled_px(24.))
                                        .h(theme::scaled_px(24.))
                                        .icon(IconName::Close)
                                        .accessibility_label("关闭备注")
                                        .tooltip("收起 WebSocket 消息备注侧栏并自动保存草稿")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            if this.leave_websocket_notes(window, cx) {
                                                let notes_focused = this
                                                    .websocket
                                                    .notes
                                                    .editor
                                                    .focus_handle(cx)
                                                    .is_focused(window);
                                                this.websocket.notes.open = false;
                                                if notes_focused {
                                                    this.focus.focus(window, cx);
                                                }
                                                cx.notify();
                                            }
                                        })),
                                ),
                        )
                        .when_some(error, |this, error| {
                            this.child(
                                div()
                                    .id("websocket-notes-error")
                                    .test_support()
                                    .role(gpui_kit::accesskit::Role::Alert)
                                    .aria_label(error.clone())
                                    .px(px(8.))
                                    .max_h(px(54.))
                                    .overflow_y_scroll()
                                    .text_color(theme::danger_fg())
                                    .child(error)
                                    .child(
                                        button("websocket-notes-retry", "重试")
                                            .icon(IconName::RefreshCw)
                                            .tooltip("重新提交并同步当前 WebSocket 消息备注")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                if this.leave_websocket_notes(window, cx) {
                                                    this.websocket.notes.error = None;
                                                    this.send_websocket_notes(true, cx);
                                                    this.select_websocket_notes(window, cx);
                                                }
                                            })),
                                    ),
                            )
                        })
                        .child(div().flex_1().min_h_0().child(
                            if active && notes.stored.is_some() {
                                Editor::new(&notes.editor)
                                    .readonly(!self.backend_available())
                                    .size_full()
                                    .bordered(false)
                                    .aria_label("WebSocket Notes")
                                    .into_any_element()
                            } else {
                                div()
                                    .p(px(10.))
                                    .child(if notes.reading {
                                        "正在读取备注…"
                                    } else {
                                        "选择一条消息查看备注"
                                    })
                                    .into_any_element()
                            },
                        )),
                )
            })
            .child(
                sidebar_rail()
                    .child(
                        sidebar_rail_tab(
                            "inspector-toggle",
                            "检\n视",
                            "Inspector",
                            IconName::SlidersHorizontal,
                            inspector_open,
                            false,
                            if inspector_open {
                                "收起 Inspector 选区编解码面板"
                            } else {
                                "展开 Inspector 选区编解码面板"
                            },
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            let notes_open =
                                this.websocket.selected.is_some() && this.websocket.notes.open;
                            if notes_open {
                                if !this.open_inspector(window, cx) {
                                    return;
                                }
                                let notes_focused = this
                                    .websocket
                                    .notes
                                    .editor
                                    .focus_handle(cx)
                                    .is_focused(window);
                                this.websocket.notes.open = false;
                                if notes_focused {
                                    this.focus.focus(window, cx);
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
                            } else {
                                this.websocket.notes.open = false;
                            }
                            cx.notify();
                        })),
                    )
                    .child(
                        sidebar_rail_tab(
                            "websocket-notes-toggle",
                            "备\n注",
                            "备注",
                            IconName::FileText,
                            notes_open,
                            !has_selected,
                            if !has_selected {
                                "请先在列表中选择一条 WebSocket 消息以编辑备注"
                            } else if notes_open {
                                "收起 WebSocket 消息备注侧栏"
                            } else {
                                "展开 WebSocket 消息备注侧栏"
                            },
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.websocket.selected.is_none()
                                || !this.leave_websocket_notes(window, cx)
                            {
                                return;
                            }
                            let notes_open =
                                this.websocket.selected.is_some() && this.websocket.notes.open;
                            if notes_open {
                                let notes_focused = this
                                    .websocket
                                    .notes
                                    .editor
                                    .focus_handle(cx)
                                    .is_focused(window);
                                this.websocket.notes.open = false;
                                if notes_focused {
                                    this.focus.focus(window, cx);
                                }
                            } else {
                                if !this.dismiss_inspector_for_current_selection(window, cx) {
                                    return;
                                }
                                this.websocket.notes.open = true;
                                this.select_websocket_notes(window, cx);
                            }
                            cx.notify();
                        })),
                    ),
            )
            .into_any_element()
    }
}
