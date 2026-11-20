use super::*;
use gpui_kit::assets::IconName;

pub(in crate::workbench::http_notes) struct BatchEditor {
    owner: WeakEntity<Workbench>,
    owner_window: AnyWindowHandle,
    id: Uuid,
    pub(in crate::workbench::http_notes) input: Entity<EditorState>,
    count: usize,
    pub(in crate::workbench::http_notes) warning: bool,
    pending: bool,
    available: bool,
    error: Option<String>,
    observed_save: Option<u64>,
    initial_revision: u64,
    initialized: bool,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl BatchEditor {
    pub(super) fn new(
        owner: Entity<Workbench>,
        owner_window: AnyWindowHandle,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("plaintext")
                .line_number(false)
                .soft_wrap(true)
                .auto_close(false)
                .autocorrect(false)
        });
        input.update(cx, |editor, cx| editor.focus(window, cx));
        let initial_revision = input.read(cx).document_revision();
        let count = owner.read(cx).http_notes.batch.as_ref().unwrap().ids.len();
        let subscription = cx.observe_in(&owner, window, |this, owner, window, cx| {
            let view = owner.read(cx);
            let Some(batch) = view
                .http_notes
                .batch
                .as_ref()
                .filter(|batch| batch.id == this.id)
            else {
                window.remove_window();
                return;
            };
            this.pending = batch.pending();
            this.available = view.backend_available()
                && view.transition.is_none()
                && view.project_saves_in_flight == 0
                && !view.file_picker_pending();
            this.error = batch.error.clone();
            let loaded = !batch.states.is_empty() && batch.read.is_none();
            let common = loaded
                .then(|| effective_common(&view.http_notes, batch))
                .flatten();
            let warning = loaded
                && common.is_none()
                && (batch.states.iter().any(|(_, _, _, nonempty)| *nonempty)
                    || view
                        .http_notes
                        .drafts
                        .iter()
                        .any(|(id, draft)| batch.ids.contains(id) && !draft.text.is_empty()));
            let saved = batch.saved.clone();
            if batch.closing && !batch.pending() {
                this.close(window, cx);
                return;
            }
            if loaded {
                if this.warning != warning {
                    this.warning = warning;
                    resize(window, warning);
                }
                if !this.initialized {
                    this.initialized = true;
                    if this.input.read(cx).document_revision() == this.initial_revision
                        && editing::snapshot(&this.input, window, cx).is_ok()
                        && let Some(text) = common
                        && this.input.read(cx).value().as_ref() != text
                    {
                        this.input
                            .update(cx, |input, cx| input.set_value(text, window, cx));
                    }
                }
            }
            if let Some((revision, text)) = saved
                && this.observed_save != Some(revision)
            {
                this.observed_save = Some(revision);
                if this.input.read(cx).document_revision() == revision
                    && editing::snapshot(&this.input, window, cx)
                        .is_ok_and(|current| current.as_ref() == text)
                {
                    this.close(window, cx);
                    return;
                }
                this.error = Some("已保存提交的备注；后续输入仍保留，请再次保存或取消。".into());
            }
            cx.notify();
        });
        let weak = owner.downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            let _ = weak.update(cx, |view, cx| view.cancel_http_notes_batch(id, cx));
            false
        });
        let weak = owner.downgrade();
        cx.on_release(move |_, cx| {
            cx.defer(move |cx| {
                let _ = weak.update(cx, |view, cx| {
                    view.finish_http_notes_batch(id, cx);
                });
            });
        })
        .detach();
        window.set_window_title("Add notes");
        resize(window, false);
        Self {
            owner: owner.downgrade(),
            owner_window,
            id,
            input,
            count,
            warning: false,
            pending: true,
            available: true,
            error: None,
            observed_save: None,
            initial_revision,
            initialized: false,
            focus: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    fn close(&self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = self.owner.clone();
        let id = self.id;
        window.remove_window();
        cx.defer(move |cx| {
            let _ = owner.update(cx, |view, cx| view.finish_http_notes_batch(id, cx));
        });
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending || !self.available {
            return;
        }
        let text = match editing::snapshot(&self.input, window, cx) {
            Ok(text) => text.to_string(),
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let revision = self.input.read(cx).document_revision();
        let owner = self.owner.clone();
        let owner_window = self.owner_window;
        let id = self.id;
        let dialog = cx.weak_entity();
        self.pending = true;
        cx.defer(move |cx| {
            let result = owner_window.update(cx, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    view.submit_http_notes_batch(id, text, revision, window, cx)
                })
            });
            let _ = dialog.update(cx, |this, cx| {
                match result {
                    Ok(Ok(Ok(()))) => {}
                    Ok(Ok(Err(error))) => {
                        this.pending = false;
                        this.error = Some(error);
                    }
                    _ => {
                        this.pending = false;
                        this.error = Some("项目窗口不可用；备注输入仍保留。".into());
                    }
                }
                cx.notify();
            });
        });
        cx.notify();
    }
}

fn resize(window: &mut Window, warning: bool) {
    let frame = window.bounds().size;
    let viewport = window.viewport_size();
    let scale = theme::ui_scale().max(1.0);
    let target_w = px((415. * scale).round());
    let base_h = if warning { 292. } else { 248. };
    let target_h = px((base_h * scale).round());
    window.resize(size(
        (target_w - (frame.width - viewport.width)).max(px(300.)),
        (target_h - (frame.height - viewport.height)).max(px(200.)),
    ));
}

impl Render for BatchEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = self.owner.clone();
        let batch_id = self.id;
        let title_bar = crate::chrome::render_title_bar(
            "mio-notes-batch",
            "Add notes",
            crate::chrome::TitleBarControls::DIALOG,
            move |_, cx| {
                let _ = weak.update(cx, |view, cx| view.cancel_http_notes_batch(batch_id, cx));
            },
            window,
            cx,
        );
        let content = div()
            .id("http-notes-batch")
            .test_support()
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .font_family(".SystemUIFont")
            .text_size(theme::ui_font_size())
            .text_color(theme::foreground())
            .bg(theme::background())
            .p(theme::scaled_px(10.))
            .gap(theme::scaled_px(8.))
            .child(format!("为 {} 条所选记录输入备注", self.count))
            .when(self.warning, |this| {
                this.child(
                    div()
                        .id("http-notes-batch-warning")
                        .test_support()
                        .text_color(rgb(0xb45309))
                        .child("其中一条或多条记录已有备注，保存后将被覆盖。"),
                )
            })
            .child(
                div().flex_1().min_h_0().child(
                    Editor::new(&self.input)
                        .size_full()
                        .aria_label("批量 HTTP Notes"),
                ),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("http-notes-batch-error")
                        .test_support()
                        .role(gpui_kit::accesskit::Role::Alert)
                        .max_h(theme::scaled_px(44.))
                        .overflow_y_scroll()
                        .text_color(theme::danger_fg())
                        .child(error),
                )
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(theme::scaled_px(6.))
                    .flex_shrink_0()
                    .child(
                        button("http-notes-batch-cancel", "取消")
                            .icon(IconName::Close)
                            .tooltip("取消批量备注编辑并关闭对话框")
                            .min_w(theme::scaled_px(72.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let _ = this.owner.update(cx, |view, cx| {
                                    view.cancel_http_notes_batch(this.id, cx)
                                });
                            })),
                    )
                    .child(
                        button("http-notes-batch-save", "保存")
                            .primary()
                            .icon(IconName::Check)
                            .tooltip("将当前备注批量写入所有选中的 HTTP 历史记录")
                            .min_w(theme::scaled_px(72.))
                            .disabled(self.pending || !self.available)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
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
