use super::*;
use gpui_kit::assets::IconName;

impl Workbench {
    pub(in crate::workbench) fn render_imported_history(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let row = self.imported_history.row.as_ref()?;
        if self
            .selected_history
            .is_some_and(|selected| selected != row.id())
        {
            return None;
        }
        let HistoryRow::ExtProxy(archive) = row else {
            return None;
        };
        let _ = archive;
        let mut body = div().size_full().flex().flex_col();
        for (response, error) in [
            (false, &self.imported_history.request_error),
            (true, &self.imported_history.response_error),
        ] {
            let Some(error) = error else {
                continue;
            };
            let mut notice = div()
                .px(px(8.))
                .text_color(theme::danger_fg())
                .child(format!(
                    "{}：{}",
                    if response { "响应" } else { "请求" },
                    error.message
                ));
            if !error.oversized {
                let id = if response {
                    "imported-history-response-retry"
                } else {
                    "imported-history-request-retry"
                };
                notice = notice.child(
                    button(id, "重试读取")
                        .icon(IconName::RefreshCw)
                        .tooltip(if response {
                            "重新从归档存储读取当前条目的响应报文"
                        } else {
                            "重新从归档存储读取当前条目的请求报文"
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if response {
                                this.imported_history.response_error = None;
                            } else {
                                this.imported_history.request_error = None;
                            }
                            if let Some(row) = this.imported_history.row.clone() {
                                this.refresh_imported_history(&row, window, cx);
                            }
                        })),
                );
            }
            body = body.child(notice);
        }
        if [
            &self.imported_history.request_error,
            &self.imported_history.response_error,
        ]
        .into_iter()
        .flatten()
        .any(|error| error.oversized)
        {
            body = body.child(
                button("imported-history-open-archive", "查看 外部导入记录")
                    .icon(IconName::Archive)
                    .tooltip("打开 外部导入与归档视图以按分片查看超大原始消息")
                    .on_click(cx.listener(|this, _, window, cx| this.show_ext_report(window, cx))),
            );
        }
        Some(
            body.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_context_message_pair(
                        "imported-history",
                        &self.history_request,
                        &self.history_response,
                        true,
                        cx,
                    )),
            )
            .into_any_element(),
        )
    }

    pub(in crate::workbench) fn select_imported_request_version(
        &mut self,
        edited: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.imported_history.row.clone() else {
            return;
        };
        if self
            .selected_history
            .is_some_and(|selected| selected != row.id())
            || self.imported_history.request_edited == edited
            || edited
                && !matches!(
                    &row,
                    HistoryRow::ExtProxy(row) if row.edited_request.is_some()
                )
        {
            return;
        }
        self.selection_revision += 1;
        let revision = self.selection_revision;
        self.history_detail_revision = revision;
        self.imported_history.request_edited = edited;
        self.imported_history.request_ready = false;
        if self
            .imported_history
            .pending
            .as_ref()
            .is_some_and(|pending| !pending.response)
        {
            self.imported_history.pending = None;
        }
        self.imported_history.request_error = None;
        if self.history_request.has_focus(window, cx) {
            self.focus.focus(window, cx);
        }
        self.history_request.load(&[], window, cx);
        self.refresh_imported_history(&row, window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn render_imported_request_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let row = self.imported_history.row.as_ref()?;
        if self
            .selected_history
            .is_some_and(|selected| selected != row.id())
            || !matches!(
                row,
                HistoryRow::ExtProxy(row) if row.edited_request.is_some()
            )
        {
            return None;
        }
        let selected = self.imported_history.request_edited;
        let owner = cx.weak_entity();
        Some(
            super::wb_editors::message_version_trigger(
                "imported-request-version",
                if selected {
                    "已编辑请求"
                } else {
                    "原始请求"
                },
                "在 外部归档记录的原始请求与拦截编辑后请求版本之间切换",
            )
            .dropdown_menu(move |menu, _, _| {
                [(false, "原始请求"), (true, "已编辑请求")]
                    .into_iter()
                    .fold(menu.compact().min_w(px(114.)), |menu, (edited, label)| {
                        let owner = owner.clone();
                        menu.item(
                            PopupMenuItem::element(move |_, _| {
                                div()
                                    .id(SharedString::from(format!("imported-req-version-{label}")))
                                    .test_support()
                                    .pl(px(16.))
                                    .pr(px(16.))
                                    .whitespace_nowrap()
                                    .text_size(theme::ui_font_size())
                                    .text_color(theme::foreground())
                                    .aria_label(label)
                                    .child(label)
                            })
                            .on_click(move |_, window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    view.select_imported_request_version(edited, window, cx)
                                });
                            }),
                        )
                    })
            })
            .horizontal_offset(px(54.))
            .offset(px(-5.))
            .margin(px(0.))
            .into_any_element(),
        )
    }
    pub(in crate::workbench) fn select_imported_response_version(
        &mut self,
        edited: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.imported_history.row.clone() else {
            return;
        };
        if self
            .selected_history
            .is_some_and(|selected| selected != row.id())
            || self.imported_history.response_edited == edited
            || edited
                && !matches!(
                    &row,
                    HistoryRow::ExtProxy(row) if row.edited_response.is_some()
                )
        {
            return;
        }
        self.selection_revision += 1;
        self.imported_history.response_edited = edited;
        self.imported_history.response_ready = false;
        if self
            .imported_history
            .pending
            .as_ref()
            .is_some_and(|pending| pending.response)
        {
            self.imported_history.pending = None;
        }
        self.imported_history.response_error = None;
        if self.history_response.has_focus(window, cx) {
            self.focus.focus(window, cx);
        }
        self.history_response.load(&[], window, cx);
        self.refresh_imported_history(&row, window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn render_imported_response_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let row = self.imported_history.row.as_ref()?;
        if self
            .selected_history
            .is_some_and(|selected| selected != row.id())
            || !matches!(
                row,
                HistoryRow::ExtProxy(row) if row.edited_response.is_some()
            )
        {
            return None;
        }
        let selected = self.imported_history.response_edited;
        let owner = cx.weak_entity();
        Some(
            super::wb_editors::message_version_trigger(
                "imported-response-version",
                if selected {
                    "已编辑响应"
                } else {
                    "原始响应"
                },
                "在 外部归档记录的原始响应与拦截编辑后响应版本之间切换",
            )
            .dropdown_menu(move |menu, _, _| {
                [(false, "原始响应"), (true, "已编辑响应")]
                    .into_iter()
                    .fold(menu.compact().min_w(px(114.)), |menu, (edited, label)| {
                        let owner = owner.clone();
                        menu.item(
                            PopupMenuItem::element(move |_, _| {
                                div()
                                    .id(SharedString::from(format!("imported-req-version-{label}")))
                                    .test_support()
                                    .pl(px(16.))
                                    .pr(px(16.))
                                    .whitespace_nowrap()
                                    .text_size(theme::ui_font_size())
                                    .text_color(theme::foreground())
                                    .aria_label(label)
                                    .child(label)
                            })
                            .on_click(move |_, window, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    view.select_imported_response_version(edited, window, cx)
                                });
                            }),
                        )
                    })
            })
            .horizontal_offset(px(54.))
            .offset(px(-5.))
            .margin(px(0.))
            .into_any_element(),
        )
    }
}
