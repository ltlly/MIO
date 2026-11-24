use gpui_kit::component::{
    button::{ButtonRounded, ButtonVariants},
    resizable::{h_resizable, resizable_panel, v_resizable},
};

use super::*;

pub(in crate::workbench) struct DetachedHttpHistoryWindow {
    owner: gpui_kit::WeakEntity<Workbench>,
    project: Option<ProjectKey>,
    selected: Option<HistoryEntryId>,
    synced_detail_revision: u64,
    synced_req_rev: crate::workbench::inspector::MessageEditorRevKey,
    synced_resp_rev: crate::workbench::inspector::MessageEditorRevKey,
    request_editor: MessageEditor,
    response_editor: MessageEditor,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl DetachedHttpHistoryWindow {
    pub(super) fn new(owner: Entity<Workbench>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (project, selected, synced_detail_revision, synced_req_rev, synced_resp_rev, req_bytes, resp_bytes) = {
            let view = owner.read(cx);
            (
                view.project,
                view.selected_history,
                view.history_detail_revision,
                crate::workbench::inspector::message_editor_rev_key(&view.history_request, cx),
                crate::workbench::inspector::message_editor_rev_key(&view.history_response, cx),
                view.history_request.bytes_lossless(cx),
                view.history_response.bytes_lossless(cx),
            )
        };
        let mut request_editor = MessageEditor::new(window, cx);
        let mut response_editor = MessageEditor::new(window, cx);
        if !req_bytes.is_empty() {
            request_editor.load(&req_bytes, window, cx);
        }
        if !resp_bytes.is_empty() {
            response_editor.load(&resp_bytes, window, cx);
        }
        let handle = window.window_handle();
        let subscription = cx.observe_in(&owner, window, move |this, owner, window, cx| {
            if owner.read(cx).project != this.project {
                window.remove_window();
                return;
            }
            this.sync_detail_from_owner(&owner, window, cx);
            cx.notify();
        });
        let released = owner.downgrade();
        cx.on_release(move |_, cx| {
            cx.defer(move |cx| {
                let _ = released.update(cx, |view, _| {
                    if view.http_filter.detached_history_window == Some(handle) {
                        view.http_filter.detached_history_window = None;
                    }
                });
            });
        })
        .detach();
        window.set_window_title("HTTP history");
        Self {
            owner: owner.downgrade(),
            project,
            selected,
            synced_detail_revision,
            synced_req_rev,
            synced_resp_rev,
            request_editor,
            response_editor,
            focus: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    fn sync_detail_from_owner(
        &mut self,
        owner: &Entity<Workbench>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (owner_selected, detail_rev, req_rev, resp_rev) = {
            let view = owner.read(cx);
            (
                view.selected_history,
                view.history_detail_revision,
                crate::workbench::inspector::message_editor_rev_key(&view.history_request, cx),
                crate::workbench::inspector::message_editor_rev_key(&view.history_response, cx),
            )
        };
        if self.selected == owner_selected
            && self.synced_detail_revision == detail_rev
            && self.synced_req_rev == req_rev
            && self.synced_resp_rev == resp_rev
        {
            return;
        }
        self.selected = owner_selected;
        self.synced_detail_revision = detail_rev;
        if self.synced_req_rev != req_rev {
            let req_bytes = owner.read(cx).history_request.bytes_lossless(cx);
            self.request_editor.load(&req_bytes, window, cx);
            self.synced_req_rev = req_rev;
        }
        if self.synced_resp_rev != resp_rev {
            let resp_bytes = owner.read(cx).history_response.bytes_lossless(cx);
            self.response_editor.load(&resp_bytes, window, cx);
            self.synced_resp_rev = resp_rev;
        }
    }

    pub(in crate::workbench) fn apply_editor_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_editor.apply_editor_preferences(window, cx);
        self.response_editor.apply_editor_preferences(window, cx);
        cx.notify();
    }

    fn select_row(&mut self, id: HistoryEntryId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(owner) = self.owner.upgrade() else {
            return;
        };
        let project = self.project;
        let owner_window = owner.read(cx).owner_window;
        if owner.read(cx).selected_history == Some(id) {
            self.sync_detail_from_owner(&owner, window, cx);
            cx.notify();
            return;
        }
        let weak_owner = owner.downgrade();
        cx.defer(move |cx| {
            let _ = owner_window.update(cx, |_, wb_window, cx| {
                let _ = weak_owner.update(cx, |view, cx| {
                    if view.project != project
                        || view.transition.is_some()
                        || !view.backend_available()
                    {
                        return;
                    }
                    view.history_pointer_selection(
                        id,
                        gpui_kit::MouseButton::Left,
                        gpui_kit::Modifiers::default(),
                        wb_window,
                        cx,
                    );
                });
            });
        });
        cx.notify();
    }

    fn render_pane_header(
        &self,
        id: &'static str,
        title: &'static str,
        editor: &MessageEditor,
        is_response: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pretty = editor.pretty_active();
        let raw_selected = !pretty && editor.view() == MessageView::Raw;
        let hex_selected = !pretty && editor.view() == MessageView::Hex;
        let tab_btn = |tab_id: String, label: &'static str, selected: bool| {
            Button::new(SharedString::from(tab_id))
                .ghost()
                .small()
                .rounded(ButtonRounded::None)
                .h(px(26.))
                .px(px(10.))
                .selected(selected)
                .toggled(selected)
                .text_color(if selected {
                    theme::foreground()
                } else {
                    theme::muted_foreground()
                })
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(if selected {
                            gpui_kit::FontWeight::BOLD
                        } else {
                            gpui_kit::FontWeight::NORMAL
                        })
                        .child(label),
                )
        };
        div()
            .w_full()
            .h(px(30.))
            .flex_shrink_0()
            .px(px(10.))
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(theme::foreground())
                    .mr(px(6.))
                    .child(title),
            )
            .child(tab_btn(format!("{id}-pretty"), "Pretty", pretty).on_click(cx.listener(
                move |this, _, window, cx| {
                    let ed = if is_response {
                        &mut this.response_editor
                    } else {
                        &mut this.request_editor
                    };
                    let _ = ed.switch_pretty(window, cx);
                    cx.notify();
                },
            )))
            .child(tab_btn(format!("{id}-raw"), "Raw", raw_selected).on_click(cx.listener(
                move |this, _, window, cx| {
                    let ed = if is_response {
                        &mut this.response_editor
                    } else {
                        &mut this.request_editor
                    };
                    let _ = ed.switch(MessageView::Raw, window, cx);
                    cx.notify();
                },
            )))
            .child(tab_btn(format!("{id}-hex"), "Hex", hex_selected).on_click(cx.listener(
                move |this, _, window, cx| {
                    let ed = if is_response {
                        &mut this.response_editor
                    } else {
                        &mut this.request_editor
                    };
                    let _ = ed.switch(MessageView::Hex, window, cx);
                    cx.notify();
                },
            )))
            .into_any_element()
    }
}

impl Render for DetachedHttpHistoryWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(owner) = self.owner.upgrade() else {
            return div()
                .id("http-history-detached-window")
                .size_full()
                .into_any_element();
        };
        let title_bar = crate::chrome::render_title_bar(
            "http-history-detached",
            "HTTP history",
            crate::chrome::TitleBarControls::DIALOG,
            |window, _| window.remove_window(),
            window,
            cx,
        );
        let rows: Vec<(HistoryEntryId, String, String, String, String, Option<u8>)> = {
            let view = owner.read(cx);
            let delegate = view.history.read(cx).delegate();
            delegate
                .rows
                .iter()
                .enumerate()
                .map(|(idx, row)| match row {
                    HistoryRow::Live(flow) => {
                        let num = delegate
                            .live_numbers
                            .get(&flow.id)
                            .copied()
                            .unwrap_or((delegate.offset + idx + 1) as u64)
                            .to_string();
                        let status = flow.status.map(|s| s.to_string()).unwrap_or_default();
                        let hl = delegate
                            .live_highlights
                            .borrow()
                            .get(&flow.id)
                            .map(|h| h.color)
                            .filter(|c| (1..=9).contains(c));
                        (
                            row.id(),
                            num,
                            flow.method.clone(),
                            flow.url.clone(),
                            status,
                            hl,
                        )
                    }
                    HistoryRow::ExtProxy(archive) => {
                        let num = archive.item.index.saturating_add(1).to_string();
                        let method = archive
                            .request
                            .as_ref()
                            .or(archive.edited_request.as_ref())
                            .and_then(|r| r.method.clone())
                            .unwrap_or_default();
                        let raw_target = archive
                            .request
                            .as_ref()
                            .or(archive.edited_request.as_ref())
                            .and_then(|r| r.request_target.clone())
                            .unwrap_or_default();
                        let url = if raw_target.starts_with('/') {
                            archive
                                .origin
                                .as_deref()
                                .map(|origin| format!("{origin}{raw_target}"))
                                .unwrap_or(raw_target)
                        } else {
                            raw_target
                        };
                        let status = archive
                            .response
                            .as_ref()
                            .or(archive.edited_response.as_ref())
                            .and_then(|r| r.status)
                            .map(|s| s.to_string())
                            .unwrap_or_default();
                        let hl = archive
                            .highlight
                            .as_ref()
                            .map(|h| h.color)
                            .filter(|c| (1..=9).contains(c));
                        (row.id(), num, method, url, status, hl)
                    }
                })
                .collect()
        };
        let current_selected = self.selected;
        let table_header = div()
            .w_full()
            .h(theme::table_header_height())
            .flex_shrink_0()
            .px(px(10.))
            .bg(theme::toolbar_surface())
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .items_center()
            .gap(px(12.))
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .text_color(theme::muted_foreground())
            .child(div().w(px(52.)).child("#"))
            .child(div().w(px(64.)).child("方法"))
            .child(div().w(px(64.)).child("状态码"))
            .child(div().flex_1().min_w_0().child("URL"));
        let table_pane = div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(table_header)
            .child(
                div()
                    .id("http-history-detached-table")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(rows.into_iter().enumerate().map(
                        |(idx, (entry_id, num, method, url, status, hl))| {
                            let is_selected = current_selected == Some(entry_id);
                            let hover_bg = theme::toolbar_surface();
                            div()
                                .id(("detached-history-row", idx))
                                .test_support()
                                .aria_selected(is_selected)
                                .cursor_pointer()
                                .h(theme::row_height())
                                .flex_shrink_0()
                                .px(px(10.))
                                .flex()
                                .items_center()
                                .gap(px(12.))
                                .border_b_1()
                                .border_color(theme::border())
                                .when_some(
                                    hl.and_then(crate::history::highlight_background),
                                    |this, bg| {
                                        this.bg(bg).text_color(
                                            crate::history::highlight_foreground(hl.unwrap_or(0)),
                                        )
                                    },
                                )
                                .when(is_selected, |this| {
                                    this.bg(theme::selected()).text_color(theme::foreground())
                                })
                                .when(!is_selected && hl.is_none(), |this| {
                                    this.hover(move |s| s.bg(hover_bg))
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.select_row(entry_id, window, cx);
                                }))
                                .child(div().w(px(52.)).truncate().child(num))
                                .child(div().w(px(64.)).truncate().child(method))
                                .child(div().w(px(64.)).truncate().child(status))
                                .child(div().flex_1().min_w_0().truncate().child(url))
                        },
                    )),
            );
        let req_pane = div()
            .id("http-history-detached-request")
            .test_support()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .child(self.render_pane_header(
                "detached-req",
                "Request",
                &self.request_editor,
                false,
                cx,
            ))
            .child(
                div().flex_1().min_h_0().child(
                    editing::message_editor(&self.request_editor)
                        .readonly(true)
                        .size_full()
                        .aria_label("独立窗口请求报文"),
                ),
            )
            .child(self.request_editor.render_search_bar(
                "detached-request",
                cx,
                |this, action, window, cx| {
                    let _ = this
                        .request_editor
                        .handle_search_bar_action(action, window, cx);
                    cx.notify();
                },
            ));
        let resp_pane = div()
            .id("http-history-detached-response")
            .test_support()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .child(self.render_pane_header(
                "detached-resp",
                "Response",
                &self.response_editor,
                true,
                cx,
            ))
            .child(
                div().flex_1().min_h_0().child(
                    editing::message_editor(&self.response_editor)
                        .readonly(true)
                        .size_full()
                        .aria_label("独立窗口响应报文"),
                ),
            )
            .child(self.response_editor.render_search_bar(
                "detached-response",
                cx,
                |this, action, window, cx| {
                    let _ = this
                        .response_editor
                        .handle_search_bar_action(action, window, cx);
                    cx.notify();
                },
            ));
        let detail_split = h_resizable("detached-history-hsplit")
            .child(
                resizable_panel()
                    .size_range(px(180.)..px(10000.))
                    .child(req_pane),
            )
            .child(
                resizable_panel()
                    .size_range(px(180.)..px(10000.))
                    .child(resp_pane),
            );
        let body = v_resizable("detached-history-vsplit")
            .child(
                resizable_panel()
                    .size(px(280.))
                    .size_range(px(120.)..px(800.))
                    .child(table_pane),
            )
            .child(
                resizable_panel()
                    .size_range(px(180.)..px(10000.))
                    .child(detail_split),
            );
        div()
            .id("http-history-detached-window")
            .test_support()
            .size_full()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .font_family(theme::ui_font_family())
            .text_size(theme::ui_font_size())
            .text_color(theme::foreground())
            .bg(theme::background())
            .child(title_bar)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}
