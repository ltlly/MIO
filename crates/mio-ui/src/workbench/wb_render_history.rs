use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    button::{ButtonRounded, ButtonVariants},
    menu::{DropdownMenu, PopupMenuItem},
    resizable::{h_resizable, resizable_panel, v_resizable},
};

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
        let ipc_path = mio_core::env_ipc_socket_path()
            .unwrap_or_else(mio_core::default_ipc_socket_path);
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

    pub(in crate::workbench) fn resolve_active_issue_context(
        &self,
        cx: &App,
    ) -> Option<(String, Option<FlowId>, &'static str)> {
        let from_history = || -> Option<(String, Option<FlowId>, &'static str)> {
            let selected = self.selected_history?;
            let url = self.history_entry_url(selected, cx)?;
            let flow_id = match selected {
                HistoryEntryId::Live(fid) => Some(fid),
                HistoryEntryId::External { .. } => None,
            };
            Some((url, flow_id, "HTTP history"))
        };
        match self.tool {
            Tool::Proxy if self.proxy_page == ProxyPage::History => from_history(),
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => self
                .selected_intercept
                .and_then(|id| self.intercepts.iter().find(|p| p.id() == id))
                .and_then(|pending| match &pending.target {
                    InterceptTarget::Http { record, .. } => {
                        let url = if !record.request.url.is_empty() {
                            record.request.url.clone()
                        } else {
                            record.summary.url.clone()
                        };
                        (!url.is_empty()).then_some((
                            url,
                            Some(record.summary.id),
                            "Proxy Intercept",
                        ))
                    }
                    InterceptTarget::WebSocket { url, record } => (!url.is_empty()).then_some((
                        url.clone(),
                        (!record.flow_id.is_nil()).then_some(record.flow_id),
                        "Proxy Intercept",
                    )),
                }),
            Tool::Proxy if self.proxy_page == ProxyPage::WebSocketHistory => self
                .websocket
                .selected_summary
                .as_ref()
                .filter(|row| !row.url.is_empty())
                .map(|row| {
                    (
                        row.url.clone(),
                        (!row.flow_id.is_nil()).then_some(row.flow_id),
                        "WebSockets history",
                    )
                }),
            Tool::Repeater => {
                if let Some(tab_id) = self.workspace.active
                    && let Some(tab) = self.workspace.tabs.iter().find(|t| t.id == tab_id)
                {
                    let raw = self
                        .tab_editors
                        .get(&tab_id)
                        .map(|ed| ed.request.bytes_lossless(cx))
                        .filter(|b| !b.is_empty())
                        .unwrap_or_else(|| tab.draft.raw.clone());
                    let url = mio_core::copy_url_for_request(&raw, &tab.draft.target, tab.draft.version)
                        .ok()
                        .unwrap_or_else(|| tab.draft.target.clone());
                    if !url.is_empty() {
                        return Some((url, tab.last_flow, "Repeater"));
                    }
                }
                if let Some(ws_id) = self.workspace.active_websocket_tab
                    && let Some(tab) = self
                        .workspace
                        .websocket_tabs
                        .iter()
                        .find(|t| t.id == ws_id)
                    && !tab.url.is_empty()
                {
                    return Some((tab.url.clone(), tab.source_flow_id, "Repeater"));
                }
                from_history()
            }
            Tool::Intruder => {
                if let Some(id) = self.workspace.intruder.active
                    && let Some(tab) = self.workspace.intruder.tabs.iter().find(|t| t.id == id)
                {
                    let url = mio_core::copy_url_for_request(
                        &tab.spec.draft.raw,
                        &tab.spec.draft.target,
                        tab.spec.draft.version,
                    )
                    .ok()
                    .unwrap_or_else(|| tab.spec.draft.target.clone());
                    if !url.is_empty() {
                        return Some((url, None, "Intruder"));
                    }
                }
                from_history()
            }
            Tool::Target => {
                if let Some(row) = self.target_pane.selected_row.as_ref() {
                    match row {
                        HistoryRow::Live(summary) if !summary.url.is_empty() => {
                            return Some((summary.url.clone(), Some(summary.id), "Target"));
                        }
                        HistoryRow::ExtProxy(archive) => {
                            let req = archive.request.as_ref().or(archive.edited_request.as_ref());
                            let target = req
                                .and_then(|r| r.request_target.as_deref())
                                .unwrap_or_default();
                            let url = if target.starts_with('/') {
                                let origin = archive.origin.as_deref().unwrap_or_default();
                                if !origin.is_empty() {
                                    format!("{origin}{target}")
                                } else {
                                    String::new()
                                }
                            } else {
                                target.to_owned()
                            };
                            if !url.is_empty() {
                                return Some((url, None, "Target"));
                            }
                        }
                        _ => {}
                    }
                }
                from_history()
            }
            Tool::Logger => {
                if let Some(flow_id) = self.extra_tools.logger_selected {
                    if let Some((record, _)) = self
                        .extra_tools
                        .logger_loaded_detail
                        .as_deref()
                        .filter(|(r, _)| r.summary.id == flow_id)
                    {
                        let url = if !record.request.url.is_empty() {
                            record.request.url.clone()
                        } else {
                            record.summary.url.clone()
                        };
                        if !url.is_empty() {
                            return Some((url, Some(flow_id), "Logger"));
                        }
                    }
                }
                from_history()
            }
            Tool::Organiser => {
                if let Some(id) = self.organiser.selected_id
                    && let Some(item) = self
                        .workspace
                        .organiser_items
                        .iter()
                        .find(|item| item.id == id)
                {
                    let url = if !item.url.is_empty() {
                        item.url.clone()
                    } else {
                        item.target.clone()
                    };
                    if !url.is_empty() {
                        return Some((url, None, "Organizer"));
                    }
                }
                from_history()
            }
            Tool::Dashboard => {
                if self.audit_dashboard.new_scan_open {
                    let url = self
                        .audit_dashboard
                        .scan_url_input
                        .read(cx)
                        .value()
                        .trim()
                        .to_owned();
                    if !url.is_empty() {
                        return Some((url, None, "Dashboard"));
                    }
                } else if let Some(finding_id) = self.audit_dashboard.selected_finding_id
                    && let Some(finding) = self
                        .audit_dashboard
                        .snapshot
                        .findings
                        .iter()
                        .find(|f| f.id == finding_id)
                    && !finding.url.is_empty()
                {
                    return Some((finding.url.clone(), finding.flow_id, "Dashboard"));
                }
                from_history()
            }
            Tool::Sequencer => {
                let url = self
                    .sequencer_pane
                    .target_url
                    .read(cx)
                    .value()
                    .trim()
                    .to_owned();
                if !url.is_empty() {
                    Some((url, None, "Sequencer"))
                } else {
                    from_history()
                }
            }
            _ => from_history(),
        }
    }

    pub(crate) fn record_manual_issue_for_active_context(&mut self, cx: &mut Context<Self>) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let Some((url, flow_id, source)) = self.resolve_active_issue_context(cx) else {
            self.fail("当前上下文没有可记录问题的 HTTP 请求 URL。", cx);
            return;
        };
        self.dispatch_record_manual_issue(project, url, flow_id, source, cx);
    }

    pub(crate) fn attach_active_context_to_issue(
        &mut self,
        finding_id: mio_core::Uuid,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let Some((url, flow_id, source)) = self.resolve_active_issue_context(cx) else {
            self.fail("当前上下文没有可关联到问题的 HTTP 请求 URL。", cx);
            return;
        };
        self.dispatch_attach_to_issue(project, finding_id, url, flow_id, source, cx);
    }

    fn dispatch_record_manual_issue(
        &mut self,
        project: ProjectKey,
        url: String,
        flow_id: Option<FlowId>,
        source: &'static str,
        cx: &mut Context<Self>,
    ) {
        let host = url::Url::parse(&url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_owned))
            .unwrap_or_default();
        let now_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let finding = mio_core::AuditFinding {
            id: mio_core::Uuid::new_v4(),
            issue_id: "0x00800100".into(),
            title: format!("自定义问题 · {url}"),
            severity_label: "信息".into(),
            severity_rank: 1,
            confidence: mio_core::IssueConfidence::Certain,
            phase: mio_core::AuditPhase::Passive,
            host,
            url: url.clone(),
            parameter: None,
            flow_id,
            summary: format!("从 {source} 创建的自定义问题（{url}）。"),
            evidence: url.clone(),
            request_highlights: Vec::new(),
            response_highlights: Vec::new(),
            remediation: "核查关联的 HTTP 请求与响应报文并补充修复建议。".into(),
            detected_unix_ms: now_unix_ms,
        };
        if self.enqueue(
            Command::Audit(mio_core::AuditCommand::RecordManualFinding {
                project,
                finding: Box::new(finding),
            }),
            cx,
        ) {
            self.error = None;
            self.status = format!("已记录新问题：{url}");
            cx.notify();
        }
    }

    fn dispatch_attach_to_issue(
        &mut self,
        project: ProjectKey,
        finding_id: mio_core::Uuid,
        url: String,
        flow_id: Option<FlowId>,
        source: &'static str,
        cx: &mut Context<Self>,
    ) {
        if self.enqueue(
            Command::Audit(mio_core::AuditCommand::AttachFlowToFinding {
                project,
                finding_id,
                flow_id,
                url: url.clone(),
                evidence: format!("{source} 关联流量：{url}"),
            }),
            cx,
        ) {
            self.error = None;
            self.status = format!("已将当前消息附加到已有问题：{url}");
            cx.notify();
        }
    }

    pub(crate) fn existing_audit_finding_ids(&self) -> Vec<(mio_core::Uuid, String)> {
        self.audit_dashboard
            .snapshot
            .findings
            .iter()
            .take(15)
            .map(|f| {
                let label = if !f.url.is_empty() && !f.title.contains(&f.url) {
                    let chars: Vec<char> = f.url.chars().collect();
                    let short_url = if chars.len() <= 44 {
                        f.url.clone()
                    } else {
                        let prefix: String = chars[..20].iter().collect();
                        let suffix: String = chars[chars.len() - 21..].iter().collect();
                        format!("{prefix}...{suffix}")
                    };
                    format!("{} ({short_url})", f.title)
                } else {
                    f.title.clone()
                };
                (f.id, label)
            })
            .collect()
    }

    pub(crate) fn record_manual_issue_from_history(
        &mut self,
        row_entry: Option<HistoryRow>,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let target = row_entry
            .as_ref()
            .map(HistoryRow::id)
            .or(self.selected_history);
        let Some(selected) = target else {
            self.fail("请先选择一条 HTTP 历史记录。", cx);
            return;
        };
        let Some(url) = self.history_entry_url(selected, cx) else {
            self.fail("所选历史记录缺少有效的请求 URL。", cx);
            return;
        };
        let flow_id = match selected {
            HistoryEntryId::Live(fid) => Some(fid),
            HistoryEntryId::External { .. } => None,
        };
        self.dispatch_record_manual_issue(project, url, flow_id, "HTTP 历史记录", cx);
    }

    pub(crate) fn attach_selected_history_to_issue(
        &mut self,
        finding_id: mio_core::Uuid,
        row_entry: Option<HistoryRow>,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let target = row_entry
            .as_ref()
            .map(HistoryRow::id)
            .or(self.selected_history);
        let Some(selected) = target else {
            self.fail("请先选择一条 HTTP 历史记录。", cx);
            return;
        };
        let Some(url) = self.history_entry_url(selected, cx) else {
            self.fail("所选历史记录缺少有效的请求 URL。", cx);
            return;
        };
        let flow_id = match selected {
            HistoryEntryId::Live(fid) => Some(fid),
            HistoryEntryId::External { .. } => None,
        };
        self.dispatch_attach_to_issue(project, finding_id, url, flow_id, "HTTP history", cx);
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

    pub(crate) fn save_active_item_xml(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tool == Tool::Proxy && self.proxy_page == ProxyPage::History {
            self.save_selected_history_item_to_file(true, window, cx);
            return;
        }
        if self.transition.is_some()
            || self.project.is_none()
            || !self.backend_available()
            || self.file_picker_pending()
        {
            return;
        }
        let draft = match self.current_transfer_draft("保存条目", window, cx) {
            Ok(draft) => draft,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let url = match mio_core::copy_url_for_request(&draft.raw, &draft.target, draft.version) {
            Ok(url) => url,
            Err(_) if draft.target.starts_with("http://") || draft.target.starts_with("https://") => {
                draft.target.clone()
            }
            Err(error) => {
                self.fail(format!("导出 XML 失败：{error}"), cx);
                return;
            }
        };
        let method = std::str::from_utf8(&draft.raw)
            .ok()
            .and_then(|s| s.lines().next())
            .and_then(|line| line.split_ascii_whitespace().next())
            .filter(|m| !m.is_empty())
            .unwrap_or("GET")
            .to_owned();
        let resp_bytes = self.tool_message_bytes(true, window, cx).unwrap_or_default();
        let status = std::str::from_utf8(&resp_bytes)
            .ok()
            .and_then(|s| s.lines().next())
            .and_then(|line| line.split_ascii_whitespace().nth(1))
            .and_then(|code| code.parse::<u16>().ok());
        let now_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let time_str = crate::history::local_time(now_unix_ms);
        let comment = if self.tool == Tool::Organiser {
            self.organiser
                .selected_id
                .and_then(|id| self.workspace.organiser_items.iter().find(|i| i.id == id))
                .map(|item| item.notes.clone())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let payload = match build_ext_items_xml(
            &url,
            &method,
            status,
            &time_str,
            &draft.raw,
            &resp_bytes,
            &comment,
        ) {
            Ok(xml) => xml.into_bytes(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        self.spawn_save_path_picker(
            Some("item.xml"),
            false,
            "项目已切换，本次文件导出未写入磁盘。",
            "无法选择保存路径",
            window,
            cx,
            move |view, mut path, _, cx| {
                if path.extension().is_none() {
                    path.set_extension("xml");
                }
                match mio_core::ui_files::write_export_file(&path, &payload) {
                    Ok(()) => {
                        view.error = None;
                        view.status = format!("已导出当前条目到文件：{}", path.display());
                        cx.notify();
                    }
                    Err(err) => {
                        view.fail(format!("写入导出文件失败：{err}"), cx);
                    }
                }
            },
        );
    }

    pub(crate) fn save_selected_history_item_to_file(
        &mut self,
        as_xml: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || self.project.is_none()
            || !self.backend_available()
            || self.file_picker_pending()
        {
            return;
        }
        let Some(selected) = self.selected_history else {
            self.fail("请先选择一条 HTTP 历史记录。", cx);
            return;
        };
        let payload_result = if as_xml {
            self.serialize_selected_history_ext_xml(selected, window, cx)
        } else {
            self.serialize_selected_history_har(selected, cx)
        };
        let payload = match payload_result {
            Ok(bytes) => bytes,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let suggested_name = if as_xml {
            "history-item.xml"
        } else {
            "history-item.har"
        };
        self.spawn_save_path_picker(
            Some(suggested_name),
            false,
            "项目已切换，本次文件导出未写入磁盘。",
            "无法选择保存路径",
            window,
            cx,
            move |view, mut path, _, cx| {
                if path.extension().is_none() {
                    path.set_extension(if as_xml { "xml" } else { "har" });
                }
                match mio_core::ui_files::write_export_file(&path, &payload) {
                    Ok(()) => {
                        view.error = None;
                        view.status = format!("已导出选中历史记录到文件：{}", path.display());
                        cx.notify();
                    }
                    Err(err) => {
                        view.fail(format!("写入导出文件失败：{err}"), cx);
                    }
                }
            },
        );
    }

    fn history_entry_url(&self, id: HistoryEntryId, cx: &App) -> Option<String> {
        match id {
            HistoryEntryId::Live(flow_id) => {
                if let Some(detail) = self.detail.as_ref().filter(|d| d.summary.id == flow_id) {
                    if !detail.summary.url.is_empty() {
                        return Some(detail.summary.url.clone());
                    }
                    return Some(detail.request.url.clone());
                }
                self.history
                    .read(cx)
                    .delegate()
                    .row_by_id(id)
                    .and_then(HistoryRow::live)
                    .map(|flow| flow.url.clone())
                    .filter(|u| !u.is_empty())
            }
            HistoryEntryId::External { .. } => {
                let row = self
                    .history
                    .read(cx)
                    .delegate()
                    .row_by_id(id)
                    .or_else(|| self.imported_history.row.as_ref().filter(|r| r.id() == id))?;
                let HistoryRow::ExtProxy(archive) = row else {
                    return None;
                };
                let req = archive.request.as_ref().or(archive.edited_request.as_ref())?;
                let target = req.request_target.as_deref().unwrap_or_default();
                if target.is_empty() {
                    return None;
                }
                if target.starts_with('/') {
                    let origin = archive.origin.as_deref().unwrap_or_default();
                    (!origin.is_empty()).then(|| format!("{origin}{target}"))
                } else {
                    Some(target.to_owned())
                }
            }
        }
    }

    fn serialize_selected_history_har(
        &self,
        selected: HistoryEntryId,
        _cx: &App,
    ) -> Result<Vec<u8>, String> {
        match selected {
            HistoryEntryId::Live(flow_id) => {
                if let Some(error) = &self.detail_error {
                    return Err(format!("无法读取所选历史记录：{error}"));
                }
                let record = self
                    .detail
                    .as_deref()
                    .filter(|record| record.summary.id == flow_id)
                    .ok_or_else(|| "正在读取所选历史记录详情，请稍后重试导出 HAR。".to_owned())?;
                mio_core::export_flows_to_har(std::slice::from_ref(record))
                    .map_err(|err| err.to_string())
            }
            HistoryEntryId::External { .. } => {
                Err("归档记录请导出为 MIO XML 格式；实时记录支持 HAR JSON 与 MIO XML。".into())
            }
        }
    }

    fn serialize_selected_history_ext_xml(
        &self,
        selected: HistoryEntryId,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Vec<u8>, String> {
        let url = self
            .history_entry_url(selected, cx)
            .ok_or_else(|| "所选历史记录缺少有效 URL。".to_owned())?;
        let req_bytes = self.tool_message_bytes(false, window, cx)?;
        let response_present = match selected {
            HistoryEntryId::Live(flow_id) => self.detail.as_ref()
                .filter(|record| record.summary.id == flow_id)
                .ok_or("所选历史记录详情尚未读取完成。")?
                .response.is_some(),
            HistoryEntryId::External { .. } => {
                let row = self.imported_history.row.as_ref()
                    .filter(|row| row.id() == selected)
                    .ok_or("所选归档历史记录详情尚未读取完成。")?;
                let HistoryRow::ExtProxy(archive) = row else {
                    return Err("所选历史记录类型无效。".into());
                };
                archive.response.is_some() || archive.edited_response.is_some()
            }
        };
        let resp_bytes = if response_present {
            self.tool_message_bytes(true, window, cx)?
        } else {
            Vec::new()
        };
        let (method, status, time_str, comment) = match selected {
            HistoryEntryId::Live(flow_id) => {
                let delegate = self.history.read(cx).delegate();
                let note = delegate
                    .live_notes
                    .borrow()
                    .get(&flow_id)
                    .map(|(_, text)| text.clone())
                    .unwrap_or_default();
                if let Some(detail) = self.detail.as_ref().filter(|d| d.summary.id == flow_id) {
                    (
                        detail.request.method.clone(),
                        detail.response.as_ref().map(|r| r.status),
                        crate::history::local_time(detail.summary.started_unix_ms),
                        note,
                    )
                } else if let Some(flow) =
                    delegate.row_by_id(selected).and_then(HistoryRow::live)
                {
                    (
                        flow.method.clone(),
                        flow.status,
                        crate::history::local_time(flow.started_unix_ms),
                        note,
                    )
                } else {
                    ("GET".into(), None, String::new(), note)
                }
            }
            HistoryEntryId::External { .. } => {
                let row = self
                    .history
                    .read(cx)
                    .delegate()
                    .row_by_id(selected)
                    .or_else(|| {
                        self.imported_history
                            .row
                            .as_ref()
                            .filter(|r| r.id() == selected)
                    });
                if let Some(HistoryRow::ExtProxy(archive)) = row {
                    if let Some(error) = &archive.time_error {
                        return Err(format!("归档时间读取失败：{error}"));
                    }
                    if let Some(error) = &archive.notes_error {
                        return Err(format!("归档备注读取失败：{error}"));
                    }
                    if let Some(error) = &archive.highlight_error {
                        return Err(format!("归档高亮读取失败：{error}"));
                    }
                    let method = archive
                        .request
                        .as_ref()
                        .or(archive.edited_request.as_ref())
                        .and_then(|r| r.method.clone())
                        .ok_or("归档请求缺少方法。")?;
                    let status = archive
                        .response
                        .as_ref()
                        .or(archive.edited_response.as_ref())
                        .and_then(|r| r.status);
                    let t = archive
                        .time_unix_ms
                        .map(crate::history::local_time)
                        .unwrap_or_default();
                    (method, status, t, archive.notes_preview.clone())
                } else {
                    return Err("所选归档历史记录摘要不可用。".into());
                }
            }
        };
        Ok(build_ext_items_xml(
            &url,
            &method,
            status,
            &time_str,
            &req_bytes,
            &resp_bytes,
            &comment,
        )?
        .into_bytes())
    }

    #[cfg(test)]
    pub(crate) fn extract_response_links_for_history(
        base_url: &str,
        raw_response: &[u8],
    ) -> Vec<String> {
        extract_response_links_list(base_url, raw_response).expect("测试链接参数必须有效")
    }

    #[cfg(test)]
    pub(crate) fn build_ext_xml_for_history(
        url: &str,
        method: &str,
        status: Option<u16>,
        time_str: &str,
        request_bytes: &[u8],
        response_bytes: &[u8],
        comment: &str,
    ) -> String {
        build_ext_items_xml(
            url,
            method,
            status,
            time_str,
            request_bytes,
            response_bytes,
            comment,
        ).expect("测试 XML 参数必须有效")
    }
}

struct DetachedHttpHistoryWindow {
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
    fn new(owner: Entity<Workbench>, window: &mut Window, cx: &mut Context<Self>) -> Self {
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

    fn select_row(&mut self, id: HistoryEntryId, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(id);
        let Some(owner) = self.owner.upgrade() else {
            return;
        };
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
                    view.select_history_entry(id, wb_window, cx);
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
                        let url = archive
                            .request
                            .as_ref()
                            .or(archive.edited_request.as_ref())
                            .and_then(|r| r.request_target.clone())
                            .unwrap_or_default();
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

pub(crate) fn extract_response_links_list(
    base_url: &str,
    raw_response: &[u8],
) -> Result<Vec<String>, String> {
    mio_core::html_extract::extract_response_links(base_url, raw_response)
}

pub(crate) fn build_ext_items_xml(
    url: &str,
    method: &str,
    status: Option<u16>,
    time_str: &str,
    request_bytes: &[u8],
    response_bytes: &[u8],
    comment: &str,
) -> Result<String, String> {
    mio_core::export_xml::build_http_item_xml(&mio_core::export_xml::XmlHttpExportItem {
        url,
        method,
        status,
        time_str,
        request_bytes,
        response_bytes,
        comment,
    })
}

#[cfg(test)]
mod history_and_intercept_tests {
    use gpui_kit::TestAppContext;
    use gpui_kit::test::TestWindowExt;
    use mio_core::{
        AuditCommand, AuditTaskMode, Command, Event, Header, HttpResponse, HttpVersion,
        InterceptStage, ProxyInterceptRelation, ProxyInterceptType, Uuid, WebSocketDirection,
        WebSocketKind, WebSocketMessage,
    };

    use super::super::tests::{drain, fixture, record};
    use super::*;

    #[gpui_kit::test]
    fn history_menu_extracts_links_starts_passive_scan_and_saves_items_to_disk(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx, Vec::new(), 64);
        let initial = drain(&mut f.commands);
        let query_revision = initial
            .iter()
            .find_map(|cmd| match cmd {
                Command::QueryHttpHistory { query } => Some(query.query_revision),
                _ => None,
            })
            .expect("initial history query");

        let mut flow = record("https://app.example.com/index.html", "");
        flow.request.url = "https://app.example.com/index.html".into();
        flow.summary.status = Some(200);
        flow.response = Some(HttpResponse {
            version: HttpVersion::Http11,
            status: 200,
            reason_phrase: None,
            headers: vec![Header {
                name: "Content-Type".into(),
                value: b"text/html; charset=utf-8".to_vec(),
            }],
            body: b"<html><body><a href=\"/login\">Login</a><a href=\"https://cdn.example.com/app.js\">JS</a></body></html>".to_vec(),
            trailers: Vec::new(),
        });
        let flow_id = flow.summary.id;

        f.events
            .try_send(Event::HistoryPage {
                project: f.project,
                query_revision,
                offset: 0,
                total: 1,
                rows: vec![HistoryRow::Live(flow.summary.clone())],
            })
            .unwrap();
        cx.run_until_parked();

        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.proxy_page = ProxyPage::History;
                view.select_flow(flow_id, window, cx);
            });
        })
        .unwrap();
        let sel_rev = drain(&mut f.commands)
            .into_iter()
            .find_map(|cmd| match cmd {
                Command::ReadFlow {
                    selection_revision, ..
                } => Some(selection_revision),
                _ => None,
            })
            .expect("read flow command");
        f.events
            .try_send(Event::FlowDetail {
                project: f.project,
                selection_revision: sel_rev,
                record: Box::new(flow.clone()),
                original: Box::default(),
            })
            .unwrap();
        cx.run_until_parked();

        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.copy_selected_history_links(window, cx);
            });
        })
        .unwrap();
        let copied_links = cx
            .read(|cx| cx.read_from_clipboard().and_then(|i| i.text()))
            .expect("extracted links in clipboard");
        assert!(copied_links.contains("https://app.example.com/login"));
        assert!(copied_links.contains("https://cdn.example.com/app.js"));

        let out_dir = std::env::current_dir()
            .unwrap()
            .join(".local")
            .join(format!("test-history-export-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&out_dir).unwrap();
        let xml_path = out_dir.join("saved.xml");
        let har_path = out_dir.join("saved.har");

        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.save_selected_history_item_to_file(true, window, cx);
            });
        })
        .unwrap();
        assert!(cx.did_prompt_for_new_path());
        cx.simulate_new_path_selection(|_| Some(xml_path.clone()));
        cx.run_until_parked();
        let xml_content = std::fs::read_to_string(&xml_path).unwrap();
        assert!(xml_content.contains("<items "));
        assert!(xml_content.contains("<item>"));
        assert!(xml_content.contains("<url>https://app.example.com/index.html</url>"));

        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.save_selected_history_item_to_file(false, window, cx);
            });
        })
        .unwrap();
        assert!(cx.did_prompt_for_new_path());
        cx.simulate_new_path_selection(|_| Some(har_path.clone()));
        cx.run_until_parked();
        let har_content = std::fs::read_to_string(&har_path).unwrap();
        assert!(har_content.contains("\"log\""));
        assert!(har_content.contains("https://app.example.com/index.html"));

        let _ = std::fs::remove_dir_all(&out_dir);

        drain(&mut f.commands);
        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.send_to_passive_scanner(window, cx);
            });
        })
        .unwrap();
        let cmds = drain(&mut f.commands);
        assert!(cmds.iter().any(|cmd| matches!(
            cmd,
            Command::Audit(AuditCommand::StartScan {
                mode: AuditTaskMode::PassiveOnly,
                ..
            })
        )));
    }

    #[gpui_kit::test]
    fn intercept_actions_support_response_interception_exclusion_rules_and_websocket_menu(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx, Vec::new(), 64);
        drain(&mut f.commands);

        let req_flow = record("https://static.example.com/assets/logo.png", "");
        let flow_id = req_flow.summary.id;
        f.events
            .try_send(Event::InterceptPending {
                project: f.project,
                stage: InterceptStage::Request,
                response_head_only: false,
                record: Box::new(req_flow),
            })
            .unwrap();
        cx.run_until_parked();

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            f.view.update(cx, |view, cx| {
                view.toggle_intercept_response_for_selected(cx);
            });
        })
        .unwrap();
        let cmds = drain(&mut f.commands);
        assert!(cmds.iter().any(|cmd| matches!(
            cmd,
            Command::SetLiveFlowAnnotated {
                flow_id: id,
                annotated: true,
                ..
            } if *id == flow_id
        )));
        assert!(cmds.iter().any(|cmd| matches!(
            cmd,
            Command::SetIntercept {
                responses: true,
                ..
            }
        )));
        cx.read(|cx| {
            let ws = &f.view.read(cx).workspace;
            assert!(ws.proxy_interception.response.enabled);
            assert!(ws.proxy_interception.response.rules.iter().any(|r| {
                r.enabled
                    && r.match_type == ProxyInterceptType::Request
                    && r.relationship == ProxyInterceptRelation::WasAnnotated
            }));
        });

        cx.update_window(f.window, |_, _, cx| {
            f.view.update(cx, |view, cx| {
                view.add_do_not_intercept_host_rule(cx);
                view.add_do_not_intercept_extension_rule(cx);
            });
        })
        .unwrap();
        cx.read(|cx| {
            let rules = &f.view.read(cx).workspace.proxy_interception.request.rules;
            assert!(rules.iter().any(|r| {
                r.enabled
                    && r.match_type == ProxyInterceptType::DomainName
                    && r.relationship == ProxyInterceptRelation::DoesNotMatch
                    && r.condition == "^static\\.example\\.com$"
            }));
            assert!(rules.iter().any(|r| {
                r.enabled
                    && r.match_type == ProxyInterceptType::FileExtension
                    && r.relationship == ProxyInterceptRelation::DoesNotMatch
                    && r.condition == "^png$"
            }));
        });

        let ws_msg = WebSocketMessage {
            id: Uuid::new_v4(),
            revision: 1,
            flow_id,
            sequence: 1,
            direction: WebSocketDirection::ClientToServer,
            kind: WebSocketKind::Text,
            payload: b"{\"ping\":1}".to_vec(),
            observed_unix_ms: 100,
        };
        let ws_id = ws_msg.id;
        f.events
            .try_send(Event::WebSocketInterceptPending {
                project: f.project,
                url: "wss://ws.example.com/live".into(),
                record: Box::new(ws_msg),
            })
            .unwrap();
        cx.run_until_parked();

        cx.update_window(f.window, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.selected_intercept = Some(ws_id);
                cx.notify();
            });
            window.render_frame(cx);
            let action_btn = window.find("intercept-action-menu");
            assert_ne!(action_btn.disabled(), Some(true));
            f.view.update(cx, |view, cx| {
                view.copy_intercept_websocket_url(cx);
                view.modify_intercept_websocket_scope(true, cx);
                view.send_intercept_websocket_to_comparer(window, cx);
            });
        })
        .unwrap();
        assert_eq!(
            cx.read(|cx| cx.read_from_clipboard().and_then(|i| i.text())),
            Some("wss://ws.example.com/live".into())
        );
        cx.read(|cx| {
            let view = f.view.read(cx);
            assert!(
                view.workspace
                    .scope
                    .include
                    .iter()
                    .any(|r| r.prefix == "https://ws.example.com")
            );
            assert_eq!(view.extra_tools.comparer_items.len(), 1);
        });
    }

    #[gpui_kit::test]
    fn toggle_intercept_response_does_not_mutate_ui_state_when_enqueue_fails(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx, Vec::new(), 64);
        drain(&mut f.commands);
        let rec = record("https://example.invalid/login", "");
        let flow_id = rec.summary.id;
        f.events
            .try_send(Event::InterceptPending {
                project: f.project,
                stage: InterceptStage::Request,
                response_head_only: false,
                record: Box::new(rec),
            })
            .unwrap();
        cx.run_until_parked();

        drop(f.commands);
        cx.update_window(f.window, |_, _, cx| {
            f.view.update(cx, |view, cx| {
                view.selected_intercept = Some(flow_id);
                view.toggle_intercept_response_for_selected(cx);
            });
        })
        .unwrap();

        cx.read(|cx| {
            let view = f.view.read(cx);
            assert!(!view.annotated_live_flows.contains(&flow_id));
            assert!(!view.intercept_responses);
            assert!(!view.workspace.proxy_interception.response.enabled);
            assert!(view.error.is_some());
        });
    }
}
