use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonVariants;

use super::*;

pub(in crate::workbench::intruder) struct ResultNotesWindow {
    target: ResultTarget,
    ordinal: u64,
    title: String,
    initial_text: String,
    truncated_preview: bool,
    expected_revision: u64,
    pub(in crate::workbench::intruder) input: Entity<InputState>,
    pending: Option<Uuid>,
    submitting: bool,
    error: Option<String>,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl ResultNotesWindow {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        target: ResultTarget,
        ordinal: u64,
        request_number: Option<u64>,
        initial_text: String,
        expected_revision: u64,
        truncated_preview: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let frame = window.bounds().size;
        let viewport = window.viewport_size();
        let scale = theme::ui_scale();
        let mut target_w = if scale > 1.0 {
            theme::scaled_px(420.)
        } else {
            px(420.)
        };
        let mut target_h = if scale > 1.0 {
            theme::scaled_px(260.)
        } else {
            px(260.)
        };
        if let Some(display) = window.display(cx) {
            let visible = display.visible_bounds().size;
            target_w = target_w.min(visible.width);
            target_h = target_h.min(visible.height);
        }
        let w = target_w - (frame.width - viewport.width);
        let h = target_h - (frame.height - viewport.height);
        if w > px(0.) && h > px(0.) {
            window.resize(size(w, h));
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("输入结果备注")
                .default_value(initial_text.clone())
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        let title = request_number.map_or_else(
            || format!("编辑结果备注 — Ordinal #{ordinal}"),
            |number| format!("编辑结果备注 — Request #{number}"),
        );
        let owner = target.workbench.upgrade().unwrap();
        let subscription = cx.observe_in(&owner, window, |this, owner, window, cx| {
            let workbench = owner.read(cx);
            if !this.target.active(workbench) {
                window.remove_window();
                return;
            }
            let view = &workbench.intruder.results.views[&this.target.view_id];
            if let Some(row) = view.query.view_row(this.ordinal)
                && this.pending.is_none()
                && !this.submitting
                && row.notes_revision > this.expected_revision
            {
                this.expected_revision = row.notes_revision;
            }
            if let Some(operation) = this.pending
                && let Some((actual, outcome)) = &view.notes_outcome
                && *actual == operation
            {
                this.pending = None;
                match outcome {
                    Ok(()) => {
                        window.remove_window();
                        return;
                    }
                    Err(error) => {
                        if let Some(row) = view.query.view_row(this.ordinal)
                            && row.notes_revision > this.expected_revision
                        {
                            this.expected_revision = row.notes_revision;
                        }
                        this.error = Some(error.clone());
                    }
                }
            }
            cx.notify();
        });
        let released = target.clone();
        let handle = window.window_handle();
        cx.on_release(move |_, cx| {
            cx.defer(move |cx| {
                let _ = released.workbench.update(cx, |workbench, cx| {
                    if released.owns(workbench) {
                        let view = workbench
                            .intruder
                            .results
                            .views
                            .get_mut(&released.view_id)
                            .unwrap();
                        if view.notes_window == Some(handle) {
                            view.notes_window = None;
                            view.notes_ordinal = None;
                            cx.notify();
                        }
                    }
                });
            });
        })
        .detach();
        let focus = cx.focus_handle();
        window.set_window_title(&title);
        Self {
            target,
            ordinal,
            title,
            initial_text,
            truncated_preview,
            expected_revision,
            input,
            pending: None,
            submitting: false,
            error: None,
            focus,
            _subscription: subscription,
        }
    }

    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_some() || self.submitting {
            return;
        }
        if let Err(error) = editing::input_snapshot(&self.input, window, cx) {
            self.error = Some(error.into());
            cx.notify();
            return;
        }
        self.truncated_preview = false;
        self.input
            .update(cx, |input, cx| input.set_value(String::new(), window, cx));
        self.error = None;
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_some() || self.submitting {
            return;
        }
        let text = match editing::input_snapshot(&self.input, window, cx) {
            Ok(text) => text.to_string(),
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        if text.len() > MAX_RESULT_NOTE_BYTES {
            self.error = Some("结果备注不能超过 64 KiB（65536 字节）；草稿仍保留。".into());
            cx.notify();
            return;
        }
        if self.truncated_preview && text == self.initial_text {
            self.error = Some(
                "当前仅载入前 256 字节预览；未修改时不会截断覆盖完整备注，请编辑或清空后再保存。"
                    .into(),
            );
            cx.notify();
            return;
        }
        let target = self.target.clone();
        let ordinal = self.ordinal;
        let expected = self.expected_revision;
        let dialog = cx.weak_entity();
        let handle = window.window_handle();
        self.submitting = true;
        self.error = None;
        cx.notify();
        cx.defer(move |cx| {
            if !cx.windows().contains(&handle) || dialog.upgrade().is_none() {
                return;
            }
            let mut outcome = Err("结果窗口已关闭；备注未保存。".to_owned());
            let _ = target.owner.update(cx, |_, window, cx| {
                target.update(window, cx, |workbench, _, cx| {
                    outcome = workbench.submit_intruder_result_notes(
                        target.view_id,
                        ordinal,
                        expected,
                        text,
                        cx,
                    );
                });
            });
            let _ = dialog.update(cx, |this, cx| {
                this.submitting = false;
                match outcome {
                    Ok(operation) => {
                        this.pending = Some(operation);
                        this.error = None;
                    }
                    Err(error) => {
                        this.pending = None;
                        this.error = Some(error);
                    }
                }
                cx.notify();
            });
        });
    }
}

impl Render for ResultNotesWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title_bar = crate::chrome::render_title_bar(
            "mio-intruder-notes",
            "Add notes",
            crate::chrome::TitleBarControls::DIALOG,
            |window, _| window.remove_window(),
            window,
            cx,
        );
        let enabled = !self.submitting && self.pending.is_none();
        let content = div()
            .id("intruder-notes-dialog")
            .test_support()
            .track_focus(&self.focus)
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .bg(theme::background())
            .text_color(theme::foreground())
            .text_size(theme::ui_font_size())
            .child(
                div()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(self.title.clone()),
            )
            .when(self.truncated_preview, |this| {
                this.child(
                    div()
                        .text_size(theme::scaled_px(12.))
                        .text_color(rgb(0xb45309))
                        .child("当前仅显示前 256 字节预览；修改后保存将替换原完整备注。"),
                )
            })
            .child(
                Input::new(&self.input)
                    .id("intruder-notes-input")
                    .disabled(!enabled),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("intruder-notes-error")
                        .test_support()
                        .role(gpui_kit::accesskit::Role::Alert)
                        .aria_label(error.clone())
                        .max_h(theme::scaled_px(52.).max(px(52.)))
                        .overflow_y_scroll()
                        .text_color(rgb(0xb42318))
                        .child(error),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        button("intruder-notes-clear", "清空")
                            .icon(IconName::Eraser)
                            .tooltip("清空当前条目的备注内容")
                            .disabled(!enabled)
                            .on_click(cx.listener(|this, _, window, cx| this.clear(window, cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        button("intruder-notes-cancel", "取消")
                            .tooltip("放弃修改并关闭备注窗口")
                            .on_click(cx.listener(|this, _, window, cx| {
                                match editing::input_snapshot(&this.input, window, cx) {
                                    Ok(_) => window.remove_window(),
                                    Err(error) => {
                                        this.error = Some(error.into());
                                        cx.notify();
                                    }
                                }
                            })),
                    )
                    .child(
                        button("intruder-notes-save", "保存")
                            .primary()
                            .icon(IconName::Check)
                            .tooltip("保存当前攻击结果条目的备注")
                            .disabled(!enabled)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::background())
            .text_color(theme::foreground())
            .child(title_bar)
            .child(div().flex_1().min_h_0().child(content))
    }
}
