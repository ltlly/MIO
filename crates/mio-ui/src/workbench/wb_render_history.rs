use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    menu::{DropdownMenu, PopupMenuItem},
    resizable::{resizable_panel, v_resizable},
};

mod detached_window;
mod issue_and_export;
#[cfg(test)]
mod tests;

pub(super) use detached_window::DetachedHttpHistoryWindow;
pub(crate) use issue_and_export::{build_ext_items_xml, extract_response_links_list};

fn editor_raw_bytes_snapshot(editor: &MessageEditor, cx: &App) -> Vec<u8> {
    editor.bytes_lossless(cx)
}

impl Workbench {
    pub(in crate::workbench) fn render_message_provenance_menu(
        &self,
        slot: super::wb_editors::EditorSlot,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let is_resp = match slot {
            super::wb_editors::EditorSlot::HistoryRequest => false,
            super::wb_editors::EditorSlot::HistoryResponse => true,
            _ => return None,
        };
        let imported = if is_resp {
            self.render_imported_response_menu(cx)
        } else {
            self.render_imported_request_menu(cx)
        };
        if imported.is_some() {
            return imported;
        }
        self.detail.as_ref()?;
        let btn_id = if is_resp {
            "history-response-version"
        } else {
            "history-request-version"
        };
        let steps: Vec<(String, Vec<u8>)> = if is_resp {
            let s = &self.detail_original.provenance.response_steps;
            if s.len() > 1 {
                s.iter()
                    .enumerate()
                    .map(|(i, st)| (format!("{i}. {}", st.label), response_to_raw(&st.response)))
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            let s = &self.detail_original.provenance.request_steps;
            if s.len() > 1 {
                s.iter()
                    .enumerate()
                    .map(|(i, st)| (format!("{i}. {}", st.label), request_to_raw(&st.request)))
                    .collect()
            } else {
                Vec::new()
            }
        };
        if steps.len() > 1 && steps.iter().skip(1).any(|(_, raw)| raw != &steps[0].1) {
            let editor = if is_resp {
                &self.history_response
            } else {
                &self.history_request
            };
            let current = editor_raw_bytes_snapshot(editor, cx);
            let side_orig = if is_resp {
                self.history_response_original_view
            } else {
                self.history_request_original_view
            };
            let active_idx = steps
                .iter()
                .position(|(_, raw)| raw.as_slice() == current.as_slice())
                .unwrap_or_else(|| {
                    if side_orig {
                        0
                    } else {
                        steps.len() - 1
                    }
                });
            let current_label = steps[active_idx].0.clone();
            let owner = cx.weak_entity();
            return Some(
                super::wb_editors::message_version_trigger(
                    btn_id,
                    current_label,
                    "切换查看报文在规则替换、插件钩子或手动拦截各阶段的历史快照",
                )
                .dropdown_menu(move |menu, _, _| {
                    steps
                        .iter()
                        .enumerate()
                        .fold(menu.compact().min_w(px(156.)), |menu, (idx, (label, raw))| {
                            let owner = owner.clone();
                            let raw = raw.clone();
                            let is_orig = idx == 0;
                            menu.item(
                                PopupMenuItem::new(label.clone())
                                    .checked(idx == active_idx)
                                    .on_click(move |_, window, cx| {
                                        let _ = owner.update(cx, |view, cx| {
                                            if is_resp {
                                                view.history_response.load(&raw, window, cx);
                                                view.history_response_original_view = is_orig;
                                            } else {
                                                view.history_request.load(&raw, window, cx);
                                                view.history_request_original_view = is_orig;
                                            }
                                            view.sync_history_original_view_flag();
                                            cx.notify();
                                        });
                                    }),
                            )
                        })
                })
                .into_any_element(),
            );
        }
        let (orig_label, final_label) = if is_resp {
            let orig_resp = self.detail_original.response.as_ref()?;
            if self
                .detail
                .as_ref()
                .and_then(|record| record.response.as_ref())
                .is_some_and(|final_resp| {
                    final_resp == orig_resp
                        || response_to_raw(final_resp) == response_to_raw(orig_resp)
                })
                && self.detail_original.response_body_complete != Some(false)
            {
                return None;
            }
            ("原始响应", "最终响应")
        } else {
            let orig_req = self.detail_original.request.as_ref()?;
            if self.detail.as_ref().is_some_and(|record| {
                &record.request == orig_req
                    || request_to_raw(&record.request) == request_to_raw(orig_req)
            }) {
                return None;
            }
            ("原始请求", "最终请求")
        };
        let orig_selected = if is_resp {
            self.history_response_original_view
        } else {
            self.history_request_original_view
        };
        let incomplete_notice = if is_resp
            && orig_selected
            && self.detail_original.response_body_complete == Some(false)
        {
            self.detail.as_ref().map(|record| {
                let state = record.summary.state.label();
                format!("{state} · 原始响应正文未完整接收（仅显示已观察内容）")
            })
        } else {
            None
        };
        let owner = cx.weak_entity();
        let menu_btn = super::wb_editors::message_version_trigger(
            btn_id,
            if orig_selected {
                orig_label
            } else {
                final_label
            },
            "在代理收到的原始报文与经规则/拦截修改后的最终报文之间切换",
        )
        .dropdown_menu(move |menu, _, _| {
            [(true, orig_label), (false, final_label)].into_iter().fold(
                menu.compact().min_w(px(110.)),
                |menu, (orig, label)| {
                    let owner = owner.clone();
                    menu.item(
                        PopupMenuItem::element(move |_, _| {
                            div()
                                .id(SharedString::from(format!("history-version-item-{label}")))
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
                                if is_resp {
                                    view.show_history_response_version(orig, window, cx)
                                } else {
                                    view.show_history_request_version(orig, window, cx)
                                }
                            });
                        }),
                    )
                },
            )
        })
        .horizontal_offset(px(54.))
        .offset(px(-5.))
        .margin(px(0.));
        Some(
            if let Some(notice) = incomplete_notice {
                div()
                    .flex()
                    .items_center()
                    .gap(theme::scaled_px(6.))
                    .child(menu_btn)
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::muted_foreground())
                            .child(notice.clone().test_text(notice)),
                    )
                    .into_any_element()
            } else {
                menu_btn.into_any_element()
            },
        )
    }

    pub(in crate::workbench) fn render_history(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let detail = if let Some(detail) = self.render_imported_history(cx) {
            detail
        } else if self.detail_error.is_none()
            && (self.detail.is_some() || self.selected_flow.is_none())
        {
            self.render_context_message_pair(
                "history",
                &self.history_request,
                &self.history_response,
                true,
                cx,
            )
        } else {
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .child(if let Some(error) = &self.detail_error {
                    format!("无法读取消息：{error}")
                } else if self.selected_flow.is_some() {
                    "正在读取消息详情…".into()
                } else {
                    "选择一条消息查看请求与响应".into()
                })
                .when(self.detail_error.is_some(), |this| {
                    this.child(
                        button("history-retry", "重试读取")
                            .icon(IconName::RefreshCw)
                            .tooltip("从项目存储重新读取当前所选流量的完整请求与响应报文")
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(id) = this.selected_flow {
                                    this.select_flow(id, window, cx);
                                }
                            })),
                    )
                })
                .into_any_element()
        };
        let detail = self.render_http_notes(detail, cx);
        let top_table =
            div()
                .size_full()
                .min_h_0()
                .flex()
                .flex_col()
                .capture_key_down(cx.listener(|this, event, window, cx| {
                    this.history_selection_key(event, window, cx)
                }))
                .child(
                    DataTable::new(&self.history)
                        .with_size(theme::row_height())
                        .header_row_height(theme::table_header_height())
                        .stripe(true)
                        .scrollbar_mode(gpui_kit::component::scroll::ScrollbarMode::Always)
                        .bordered(false)
                        .scroll_bounce(false)
                        .accessibility_label("HTTP 历史记录"),
                );
        let bottom_detail = div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(div().flex_1().min_h_0().child(detail));
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.render_http_filter_bar(window, cx))
            .child(
                div().flex_1().min_h_0().border_t_1().border_color(theme::border()).child(
                    v_resizable("proxy-history-vsplit")
                        .child(
                            resizable_panel()
                                .size(px(265.))
                                .size_range(px(110.)..px(900.))
                                .child(top_table),
                        )
                        .child(
                            resizable_panel()
                                .size_range(px(180.)..px(10000.))
                                .child(bottom_detail),
                        ),
                ),
            )
            .into_any_element()
    }

    pub(crate) fn copy_selected_history_links(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let Some(selected) = self.selected_history else {
            self.fail("请先选择一条 HTTP 历史记录。", cx);
            return;
        };
        let Some(base_url) = self.history_entry_url(selected, cx) else {
            self.fail("所选历史记录缺少有效的请求 URL。", cx);
            return;
        };
        let raw_response = match self.tool_message_bytes(true, window, cx) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let links = match extract_response_links_list(&base_url, &raw_response) {
            Ok(links) => links,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        if links.is_empty() {
            self.error = None;
            self.status = "选中响应中未提取到有效的页面 URL 链接。".into();
            cx.notify();
            return;
        }
        let count = links.len();
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(links.join("\n")));
        self.error = None;
        self.status = format!("已提取并复制 {count} 条页面链接到剪贴板。");
        cx.notify();
    }

    pub(crate) fn send_selected_to_organiser_from_history(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_selected_to_organiser(window, cx);
    }

    pub(crate) fn send_selected_to_mio_at(&mut self, cx: &mut Context<Self>) {
        let (title, url, req_bytes, resp_bytes, flow_id) =
            self.snapshot_active_mio_at_context(cx);
        let ipc_path = self.effective_ipc_socket_path();
        let ipc_path_str = ipc_path.display().to_string();
        let cli_cmd = match flow_id {
            Some(flow_id) => format!(
                "mio-cli --socket \"{ipc_path_str}\" flows get {flow_id} --json"
            ),
            None => format!("mio-cli --socket \"{ipc_path_str}\" status"),
        };
        let mut sections = Vec::new();
        if !url.is_empty() {
            sections.push(format!("Target URL: {url}"));
        }
        if !req_bytes.is_empty() {
            let text = String::from_utf8_lossy(&req_bytes);
            let truncated: String = text.chars().take(16_384).collect();
            sections.push(format!("=== HTTP Request ===\n{truncated}"));
        }
        if !resp_bytes.is_empty() {
            let text = String::from_utf8_lossy(&resp_bytes);
            let truncated: String = text.chars().take(16_384).collect();
            sections.push(format!("=== HTTP Response ===\n{truncated}"));
        }
        sections.push(cli_cmd);
        let context = sections.join("\n\n");
        self.error = None;
        self.open_mio_ai_with_context(title, context, cx);
    }

    fn snapshot_active_mio_at_context(
        &self,
        cx: &App,
    ) -> (String, String, Vec<u8>, Vec<u8>, Option<FlowId>) {
        let (url, flow_id, source) = self
            .resolve_active_issue_context(cx)
            .unwrap_or_else(|| (String::new(), None, "Workbench"));
        let (req_bytes, resp_bytes) = match self.tool {
            Tool::Proxy if self.proxy_page == ProxyPage::History => (
                self.history_request.bytes_lossless(cx),
                self.history_response.bytes_lossless(cx),
            ),
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => {
                if let Some(pending) = self
                    .selected_intercept
                    .and_then(|id| self.intercepts.iter().find(|p| p.id() == id))
                {
                    match &pending.target {
                        InterceptTarget::Http { stage, record, .. } => match stage {
                            InterceptStage::Request => (
                                pending.editor.bytes_lossless(cx),
                                record
                                    .response
                                    .as_ref()
                                    .map(response_to_raw)
                                    .unwrap_or_default(),
                            ),
                            InterceptStage::Response => (
                                request_to_raw(&record.request),
                                pending.editor.bytes_lossless(cx),
                            ),
                        },
                        InterceptTarget::WebSocket { .. } => {
                            (pending.editor.bytes_lossless(cx), Vec::new())
                        }
                    }
                } else {
                    (Vec::new(), Vec::new())
                }
            }
            Tool::Proxy if self.proxy_page == ProxyPage::WebSocketHistory => {
                (self.websocket.payload.bytes_lossless(cx), Vec::new())
            }
            Tool::Repeater => {
                if let Some(tab_id) = self.workspace.active
                    && let Some(editors) = self.tab_editors.get(&tab_id)
                {
                    (
                        editors.request.bytes_lossless(cx),
                        editors.response.bytes_lossless(cx),
                    )
                } else if let Some(ws_id) = self.workspace.active_websocket_tab
                    && let Some(editors) = self.websocket_repeater.editors.get(&ws_id)
                {
                    (editors.draft.bytes_lossless(cx), Vec::new())
                } else {
                    (Vec::new(), Vec::new())
                }
            }
            Tool::Intruder => {
                if let Some(id) = self.workspace.intruder.active
                    && let Some(tab) = self.workspace.intruder.tabs.iter().find(|t| t.id == id)
                {
                    (tab.spec.draft.raw.clone(), Vec::new())
                } else {
                    (Vec::new(), Vec::new())
                }
            }
            Tool::Target => (
                self.target_pane.request.bytes_lossless(cx),
                self.target_pane.response.bytes_lossless(cx),
            ),
            Tool::Logger => (
                self.extra_tools.logger_request.bytes_lossless(cx),
                self.extra_tools.logger_response.bytes_lossless(cx),
            ),
            Tool::Organiser => (
                self.organiser.request.bytes_lossless(cx),
                self.organiser.response.bytes_lossless(cx),
            ),
            Tool::Dashboard => {
                if self.audit_dashboard.new_scan_open {
                    (
                        self.audit_dashboard.scan_request_editor.bytes_lossless(cx),
                        Vec::new(),
                    )
                } else {
                    (
                        self.audit_dashboard.issue_request_editor.bytes_lossless(cx),
                        self.audit_dashboard.issue_response_editor.bytes_lossless(cx),
                    )
                }
            }
            Tool::Sequencer => (self.sequencer_pane.request.bytes_lossless(cx), Vec::new()),
            Tool::Collaborator => (
                self.collaborator_pane.raw_editor.bytes_lossless(cx),
                Vec::new(),
            ),
            _ => (Vec::new(), Vec::new()),
        };
        let title = if url.is_empty() {
            source.to_owned()
        } else {
            format!("{source} · {url}")
        };
        (title, url, req_bytes, resp_bytes, flow_id)
    }

    pub(crate) fn open_detached_history_window(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else {
            return;
        };
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let owner = cx.entity();
        let bounds = gpui_kit::Bounds::centered(None, gpui_kit::size(px(1080.), px(720.)), cx);
        cx.defer(move |cx| {
            if owner.read(cx).project != Some(project) {
                return;
            }
            if let Some(existing) = owner.read(cx).http_filter.detached_history_window
                && cx.windows().contains(&existing)
                && crate::chrome::activate_popup_window(existing, cx)
            {
                return;
            }
            let opened = crate::chrome::open_popup_window(
                crate::chrome::mio_window_options(bounds, false, false),
                cx,
                |window, cx| cx.new(|cx| DetachedHttpHistoryWindow::new(owner.clone(), window, cx)),
            );
            owner.update(cx, |view, cx| match opened {
                Ok((handle, _)) => {
                    view.http_filter.detached_history_window = Some(handle);
                    view.error = None;
                    view.status = "已在新窗口中打开 HTTP 历史记录。".into();
                    cx.notify();
                }
                Err(error) => view.fail(format!("无法打开新 HTTP 历史记录窗口：{error}"), cx),
            });
        });
    }

    pub(crate) fn plugin_context_menu_items(&self) -> Vec<(mio_core::Uuid, String)> {
        self.active_context_menu_plugins()
    }

    pub(crate) fn invoke_plugin_context_menu_from_history(
        &mut self,
        instance_id: mio_core::Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.invoke_plugin_context_menu(instance_id, window, cx);
    }

    pub(crate) fn is_history_url_in_scope(&self, url: &str) -> bool {
        self.workspace.scope.has_enabled_includes() && self.workspace.scope.is_url_in_scope(url)
    }

    pub(crate) fn open_proxy_history_documentation(&mut self, cx: &mut Context<Self>) {
        let doc_url = "https://github.com/ltlly/MIO";
        cx.open_url(doc_url);
        self.error = None;
        self.status = format!("已打开 HTTP 历史记录文档：{doc_url}");
        cx.notify();
    }

    pub(crate) fn copy_selected_flow_har_from_history(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_selected_flow_har(window, cx);
    }
}
