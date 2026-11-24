use std::collections::HashSet;
use std::fmt::Write;
use mio_core::{
    AuditCommand, AuditFinding, AuditPhase, ExtHttpHighlight, ExtHttpHighlightChange, FlowId,
    FlowRecord, FlowSummary, HistoryEntryId, HistoryTransferEntry, IssueConfidence,
    OrganiserItem, Uuid, next_organiser_number, validate_organiser_items,
};
use tokio_util::sync::CancellationToken;

use super::*;

impl Workbench {
    fn selected_logger_summary(&self) -> Option<FlowSummary> {
        let id = self.extra_tools.logger_selected?;
        self.extra_tools
            .logger_entries
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
    }

    pub(in crate::workbench::tools_extra::logger) fn send_logger_selected_to_mio_at(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(flow_id) = self.extra_tools.logger_selected else {
            self.fail("请先选择一条 Logger 流量记录。", cx);
            return;
        };
        let summary = self.selected_logger_summary();
        let detail = self
            .extra_tools
            .logger_loaded_detail
            .as_deref()
            .filter(|(record, _)| record.summary.id == flow_id);
        let url = detail
            .map(|(record, _)| record.request.url.clone())
            .or_else(|| summary.as_ref().map(|s| s.url.clone()))
            .unwrap_or_default();
        let method = detail
            .map(|(record, _)| record.request.method.clone())
            .or_else(|| summary.as_ref().map(|s| s.method.clone()))
            .unwrap_or_else(|| "GET".into());
        let status = detail
            .and_then(|(record, _)| record.response.as_ref().map(|r| r.status))
            .or_else(|| summary.as_ref().and_then(|s| s.status));
        let req_raw = detail
            .map(|(record, _)| request_to_raw(&record.request))
            .or_else(|| self.extra_tools.logger_request.bytes(window, cx).ok())
            .unwrap_or_default();
        let resp_raw = detail
            .and_then(|(record, _)| record.response.as_ref().map(response_to_raw))
            .or_else(|| self.extra_tools.logger_response.bytes(window, cx).ok())
            .unwrap_or_default();
        let req_preview = String::from_utf8_lossy(&req_raw[..req_raw.len().min(4096)]);
        let resp_preview = String::from_utf8_lossy(&resp_raw[..resp_raw.len().min(4096)]);
        let ipc_path = self
            .effective_ipc_socket_path()
            .display()
            .to_string();
        let mut ctx_buf = format!(
            "Logger Flow: {flow_id}\nURL: {method} {url}\nStatus: {}\nCLI: mio-cli --socket \"{ipc_path}\" flows get {flow_id} --json",
            status.map_or_else(|| "-".into(), |c| c.to_string()),
        );
        if !req_preview.is_empty() {
            let _ = write!(ctx_buf, "\n\n--- Request ---\n{req_preview}");
        }
        if !resp_preview.is_empty() {
            let _ = write!(ctx_buf, "\n\n--- Response ---\n{resp_preview}");
        }
        let title = if url.is_empty() {
            format!("Logger #{flow_id}")
        } else {
            format!("{method} {url}")
        };
        self.open_mio_ai_with_context(title, ctx_buf, cx);
    }

    pub(in crate::workbench::tools_extra::logger) fn record_manual_issue_from_logger(&mut self, cx: &mut Context<Self>) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let Some(flow_id) = self.extra_tools.logger_selected else {
            self.fail("请先选择一条 Logger 流量记录。", cx);
            return;
        };
        let url = self
            .extra_tools
            .logger_loaded_detail
            .as_deref()
            .filter(|(r, _)| r.summary.id == flow_id)
            .map(|(r, _)| r.request.url.clone())
            .or_else(|| self.selected_logger_summary().map(|s| s.url))
            .unwrap_or_default();
        if url.is_empty() {
            self.fail("所选 Logger 记录缺少有效的请求 URL。", cx);
            return;
        }
        let host = url::Url::parse(&url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_owned))
            .unwrap_or_default();
        let now_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let finding = AuditFinding {
            id: Uuid::new_v4(),
            issue_id: "0x00800100".into(),
            title: format!("自定义问题 · {url}"),
            severity_label: "信息".into(),
            severity_rank: 1,
            confidence: IssueConfidence::Certain,
            phase: AuditPhase::Passive,
            host,
            url: url.clone(),
            parameter: None,
            flow_id: Some(flow_id),
            summary: format!("从 Logger 流量记录创建的自定义问题（{url}）。"),
            evidence: url.clone(),
            request_highlights: Vec::new(),
            response_highlights: Vec::new(),
            remediation: "核查关联的 HTTP 请求与响应报文并补充修复建议。".into(),
            detected_unix_ms: now_unix_ms,
        };
        if self.enqueue(
            Command::Audit(AuditCommand::RecordManualFinding {
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

    pub(in crate::workbench::tools_extra::logger) fn attach_logger_flow_to_issue(
        &mut self,
        finding_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let Some(flow_id) = self.extra_tools.logger_selected else {
            self.fail("请先选择一条 Logger 流量记录。", cx);
            return;
        };
        let url = self
            .extra_tools
            .logger_loaded_detail
            .as_deref()
            .filter(|(r, _)| r.summary.id == flow_id)
            .map(|(r, _)| r.request.url.clone())
            .or_else(|| self.selected_logger_summary().map(|s| s.url))
            .unwrap_or_default();
        if url.is_empty() {
            self.fail("所选 Logger 记录缺少有效的请求 URL。", cx);
            return;
        }
        if self.enqueue(
            Command::Audit(AuditCommand::AttachFlowToFinding {
                project,
                finding_id,
                flow_id: Some(flow_id),
                url: url.clone(),
                evidence: format!("Logger 关联流量：{url}"),
            }),
            cx,
        ) {
            self.error = None;
            self.status = format!("已将当前 Logger 记录附加到现有问题：{url}");
            cx.notify();
        }
    }

    pub(in crate::workbench::tools_extra::logger) fn find_logger_references(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(flow_id) = self.extra_tools.logger_selected else {
            self.fail("请先选择一条 Logger 流量记录。", cx);
            return;
        };
        let url = self
            .extra_tools
            .logger_loaded_detail
            .as_deref()
            .filter(|(r, _)| r.summary.id == flow_id)
            .map(|(r, _)| r.request.url.clone())
            .or_else(|| self.selected_logger_summary().map(|s| s.url))
            .unwrap_or_default();
        if url.is_empty() {
            self.fail("所选 Logger 记录缺少可检索的 URL。", cx);
            return;
        }
        let parsed = parse_logger_url(&url);
        let needle = if parsed.path != "/" && !parsed.path.is_empty() {
            parsed.path
        } else if !parsed.host.is_empty() {
            parsed.host
        } else {
            url
        };
        self.global_search
            .input
            .update(cx, |input, cx| input.set_value(needle, window, cx));
        self.open_global_search(window, cx);
        self.query_global_search(window, cx);
    }

    pub(in crate::workbench::tools_extra::logger) fn generate_logger_csrf_poc(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(flow_id) = self.extra_tools.logger_selected else {
            self.fail("请先选择一条 Logger 流量记录。", cx);
            return;
        };
        let (url, raw_request) = if let Some((record, _)) = self
            .extra_tools
            .logger_loaded_detail
            .as_deref()
            .filter(|(r, _)| r.summary.id == flow_id)
        {
            (record.request.url.clone(), request_to_raw(&record.request))
        } else {
            let url = self
                .selected_logger_summary()
                .map(|s| s.url)
                .unwrap_or_default();
            let raw = match self.extra_tools.logger_request.bytes(window, cx) {
                Ok(bytes) if !bytes.is_empty() => bytes,
                Ok(_) => {
                    self.fail("当前 Logger 请求详情尚未读取完成，无法生成 CSRF PoC。", cx);
                    return;
                }
                Err(error) => {
                    self.fail(error, cx);
                    return;
                }
            };
            (url, raw)
        };
        if raw_request.is_empty() {
            self.fail("当前 Logger 请求正文为空，无法生成 CSRF PoC。", cx);
            return;
        }
        self.open_csrf_poc_modal(&url, &raw_request, cx);
    }

    pub(in crate::workbench::tools_extra::logger) fn set_logger_flow_highlight(
        &mut self,
        flow_id: FlowId,
        color: u8,
        cx: &mut Context<Self>,
    ) {
        if color > 9 || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let entry_id = HistoryEntryId::Live(flow_id);
        let in_history_cache = self
            .history
            .read(cx)
            .delegate()
            .row_by_id(entry_id)
            .is_some();
        if in_history_cache {
            self.set_http_highlights(&[entry_id], color, cx);
            return;
        }
        let prev = self
            .history
            .read(cx)
            .delegate()
            .live_highlights
            .borrow()
            .get(&flow_id)
            .cloned()
            .unwrap_or(ExtHttpHighlight {
                revision: 0,
                color: 0,
            });
        let next_rev = prev.revision.saturating_add(1);
        self.history.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.live_highlights.borrow_mut().insert(
                flow_id,
                ExtHttpHighlight {
                    revision: next_rev,
                    color,
                },
            );
            table.refresh(cx);
        });
        let _ = self.enqueue(
            Command::SetExtHttpHighlights {
                project,
                operation_id: Uuid::new_v4(),
                changes: vec![ExtHttpHighlightChange {
                    source_id: Uuid::nil(),
                    item_id: flow_id,
                    expected_highlight_revision: prev.revision,
                    color,
                }],
                cancel: CancellationToken::new(),
            },
            cx,
        );
        self.error = None;
        self.status = if color == 0 {
            "已清除 Logger 条目高亮。".into()
        } else {
            format!(
                "已将 Logger 条目标记为「{}」高亮。",
                LOGGER_HIGHLIGHT_NAMES[usize::from(color)]
            )
        };
        cx.notify();
    }

    pub(in crate::workbench::tools_extra::logger) fn save_selected_logger_item_to_file(
        &mut self,
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
        let Some(flow_id) = self.extra_tools.logger_selected else {
            self.fail("请先选择一条 Logger 流量记录。", cx);
            return;
        };
        let Some((record, _)) = self
            .extra_tools
            .logger_loaded_detail
            .as_deref()
            .filter(|(r, _)| r.summary.id == flow_id)
            .cloned()
        else {
            self.fail("Logger 流量详情尚未读取完成，请稍后重试保存条目。", cx);
            return;
        };
        let comment = {
            let live_notes = self.history.read(cx).delegate().live_notes.borrow();
            entry_comment(flow_id, &live_notes)
        };
        let url = if !record.request.url.is_empty() {
            record.request.url.clone()
        } else {
            record.summary.url.clone()
        };
        let method = if !record.request.method.is_empty() {
            record.request.method.clone()
        } else {
            record.summary.method.clone()
        };
        let status = record
            .response
            .as_ref()
            .map(|r| r.status)
            .or(record.summary.status);
        let time_str = crate::history::local_time(record.summary.started_unix_ms);
        let req_bytes = request_to_raw(&record.request);
        let resp_bytes = record
            .response
            .as_ref()
            .map(response_to_raw)
            .unwrap_or_default();
        let xml_payload = match crate::workbench::wb_render_history::build_ext_items_xml(
            &url,
            &method,
            status,
            &time_str,
            &req_bytes,
            &resp_bytes,
            &comment,
        ) {
            Ok(xml) => xml.into_bytes(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let project = self.project;
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &self.default_directory(),
            Some("logger-item.xml"),
        );
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = owner.update(cx, |_, _, cx| {
                this.update(cx, |view, cx| {
                    if !view.finish_file_picker(picker, cx) || !view.backend_available() {
                        return;
                    }
                    if view.project != project {
                        view.fail("项目已切换，本次条目导出未写入磁盘。", cx);
                        return;
                    }
                    match result {
                        Ok(Ok(Some(mut path))) => {
                            let is_har = path
                                .extension()
                                .and_then(|ext| ext.to_str())
                                .is_some_and(|ext| ext.eq_ignore_ascii_case("har"));
                            let bytes = if is_har {
                                match mio_core::export_flows_to_har(std::slice::from_ref(&record)) {
                                    Ok(har) => har,
                                    Err(err) => {
                                        view.fail(format!("导出 HAR 失败：{err}"), cx);
                                        return;
                                    }
                                }
                            } else {
                                if path.extension().is_none() {
                                    path.set_extension("xml");
                                }
                                xml_payload.clone()
                            };
                            match mio_core::ui_files::write_export_file(&path, &bytes) {
                                Ok(()) => {
                                    view.error = None;
                                    view.status = format!(
                                        "已保存 Logger 条目到文件：{}",
                                        path.display()
                                    );
                                    cx.notify();
                                }
                                Err(err) => {
                                    view.fail(format!("写入导出文件失败：{err}"), cx);
                                }
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(error)) => view.fail(format!("无法选择保存路径：{error}"), cx),
                        Err(_) => view.fail("文件选择器已意外关闭。", cx),
                    }
                })
            });
        })
        .detach();
    }

    pub(in crate::workbench::tools_extra::logger) fn handle_logger_row_click(
        &mut self,
        flow_id: FlowId,
        visible_ids: &[FlowId],
        shift: bool,
        toggle: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cur_selected = self.extra_tools.logger_selected;
        if shift {
            let anchor = self
                .extra_tools
                .logger_ui_table
                .borrow()
                .anchor_id
                .or(cur_selected)
                .unwrap_or(flow_id);
            let start_pos = visible_ids.iter().position(|id| *id == anchor);
            let end_pos = visible_ids.iter().position(|id| *id == flow_id);
            {
                let mut st = self.extra_tools.logger_ui_table.borrow_mut();
                st.explicit_empty = None;
                if let (Some(s), Some(e)) = (start_pos, end_pos) {
                    let (lo, hi) = (s.min(e), s.max(e));
                    st.selected_set = visible_ids[lo..=hi].iter().copied().collect();
                } else {
                    st.selected_set.insert(flow_id);
                }
                if st.anchor_id.is_none() {
                    st.anchor_id = Some(anchor);
                }
            }
            self.select_logger_entry(flow_id, window, cx);
            return;
        }
        if toggle {
            let mut next_primary = Some(flow_id);
            let mut deselected_last = false;
            {
                let mut st = self.extra_tools.logger_ui_table.borrow_mut();
                if st.selected_set.is_empty()
                    && st.explicit_empty != cur_selected
                    && let Some(cur) = cur_selected
                {
                    st.selected_set.insert(cur);
                }
                if st.selected_set.contains(&flow_id) {
                    st.selected_set.remove(&flow_id);
                    if st.selected_set.is_empty() {
                        st.anchor_id = None;
                        st.explicit_empty = None;
                        next_primary = None;
                        deselected_last = true;
                    } else if cur_selected == Some(flow_id) {
                        next_primary = visible_ids
                            .iter()
                            .copied()
                            .find(|id| st.selected_set.contains(id))
                            .or_else(|| st.selected_set.iter().copied().next());
                    } else {
                        next_primary = cur_selected;
                    }
                } else {
                    st.explicit_empty = None;
                    st.selected_set.insert(flow_id);
                    st.anchor_id = Some(flow_id);
                }
            }
            if deselected_last {
                if self.extra_tools.logger_selected.is_some() && !self.leave_http_notes(window, cx)
                {
                    return;
                }
                self.extra_tools.logger_selected = None;
                self.extra_tools.logger_pending_read = None;
                self.extra_tools.logger_loaded_detail = None;
                self.extra_tools.logger_request.load(b"", window, cx);
                self.extra_tools.logger_response.load(b"", window, cx);
                self.clear_context_viewer("logger", cx);
                if self.http_notes.open {
                    self.select_http_notes(window, cx);
                }
                cx.notify();
            } else if let Some(pid) = next_primary {
                self.select_logger_entry(pid, window, cx);
            } else {
                cx.notify();
            }
            return;
        }
        {
            let mut st = self.extra_tools.logger_ui_table.borrow_mut();
            st.explicit_empty = None;
            st.selected_set.clear();
            st.selected_set.insert(flow_id);
            st.anchor_id = Some(flow_id);
        }
        self.select_logger_entry(flow_id, window, cx);
    }

    pub(in crate::workbench::tools_extra::logger) fn handle_logger_row_right_down(
        &mut self,
        flow_id: FlowId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let already_in_multi = {
            let st = self.extra_tools.logger_ui_table.borrow();
            st.selected_set.contains(&flow_id) && st.selected_set.len() > 1
        };
        if already_in_multi {
            cx.notify();
            return;
        }
        {
            let mut st = self.extra_tools.logger_ui_table.borrow_mut();
            st.explicit_empty = None;
            st.selected_set.clear();
            st.selected_set.insert(flow_id);
            st.anchor_id = Some(flow_id);
        }
        if self.extra_tools.logger_selected != Some(flow_id) {
            self.select_logger_entry(flow_id, window, cx);
        }
    }

    pub(in crate::workbench::tools_extra::logger) fn set_logger_col_sort(
        &mut self,
        col_ix: usize,
        ascending: bool,
        cx: &mut Context<Self>,
    ) {
        match col_ix {
            0 => {
                self.extra_tools.logger_filters.extended_sort = None;
                self.extra_tools.logger_sort = LoggerSort {
                    column: LoggerSortColumn::Number,
                    ascending,
                };
            }
            1 => {
                self.extra_tools.logger_filters.extended_sort =
                    Some((LoggerExtendedSortColumn::Time, ascending));
            }
            2 => {
                self.extra_tools.logger_filters.extended_sort = None;
                self.extra_tools.logger_sort = LoggerSort {
                    column: LoggerSortColumn::Source,
                    ascending,
                };
            }
            3 => {
                self.extra_tools.logger_filters.extended_sort = None;
                self.extra_tools.logger_sort = LoggerSort {
                    column: LoggerSortColumn::Method,
                    ascending,
                };
            }
            4 => {
                self.extra_tools.logger_filters.extended_sort =
                    Some((LoggerExtendedSortColumn::Host, ascending));
            }
            5 => {
                self.extra_tools.logger_filters.extended_sort =
                    Some((LoggerExtendedSortColumn::Path, ascending));
            }
            6 => {
                self.extra_tools.logger_filters.extended_sort =
                    Some((LoggerExtendedSortColumn::Query, ascending));
            }
            7 => {
                self.extra_tools.logger_filters.extended_sort =
                    Some((LoggerExtendedSortColumn::ParamCount, ascending));
            }
            8 => {
                self.extra_tools.logger_filters.extended_sort = None;
                self.extra_tools.logger_sort = LoggerSort {
                    column: LoggerSortColumn::Status,
                    ascending,
                };
            }
            9 => {
                self.extra_tools.logger_filters.extended_sort = None;
                self.extra_tools.logger_sort = LoggerSort {
                    column: LoggerSortColumn::Length,
                    ascending,
                };
            }
            10 => {
                self.extra_tools.logger_filters.extended_sort =
                    Some((LoggerExtendedSortColumn::StartResponseTimer, ascending));
            }
            11 => {
                self.extra_tools.logger_filters.extended_sort =
                    Some((LoggerExtendedSortColumn::Comment, ascending));
            }
            _ => return,
        }
        cx.notify();
    }

    pub(in crate::workbench::tools_extra::logger) fn copy_logger_column(
        &mut self,
        col_ix: usize,
        rows: &[(usize, FlowId)],
        cx: &mut Context<Self>,
    ) {
        let entries = &self.extra_tools.logger_entries;
        let param_counts = &self.extra_tools.logger_filters.param_counts;
        let live_notes = self.history.read(cx).delegate().live_notes.borrow();
        let mut lines = Vec::with_capacity(rows.len());
        for &(number, flow_id) in rows {
            if let Some(entry) = entries
                .get(number.saturating_sub(1))
                .filter(|e| e.id == flow_id)
                .or_else(|| entries.iter().find(|e| e.id == flow_id))
            {
                lines.push(logger_cell_text_for_entry(
                    col_ix,
                    number,
                    entry,
                    param_counts,
                    &live_notes,
                ));
            }
        }
        drop(live_notes);
        let count = lines.len();
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(lines.join("\n")));
        self.error = None;
        self.status = format!(
            "已复制「{}」列（{count} 行）到剪贴板。",
            LOGGER_COL_NAMES.get(col_ix).copied().unwrap_or("Logger")
        );
        cx.notify();
    }

    pub(in crate::workbench::tools_extra::logger) fn toggle_logger_column_visibility(
        &mut self,
        col_ix: usize,
        cx: &mut Context<Self>,
    ) {
        if col_ix == 0 || col_ix >= LOGGER_COL_COUNT {
            return;
        }
        {
            let mut st = self.extra_tools.logger_ui_table.borrow_mut();
            let visible_count = st.col_visible.iter().filter(|&&v| v).count();
            if st.col_visible[col_ix] {
                if visible_count > 1 {
                    st.col_visible[col_ix] = false;
                }
            } else {
                st.col_visible[col_ix] = true;
            }
        }
        cx.notify();
    }

    pub(in crate::workbench::tools_extra::logger) fn fit_logger_column_width(
        &mut self,
        col_ix: usize,
        rows: &[(usize, FlowId)],
        cx: &mut Context<Self>,
    ) {
        if col_ix >= LOGGER_COL_COUNT {
            return;
        }
        let measure = |text: &str| -> f32 {
            text.chars()
                .map(|ch| if ch.is_ascii() { 7.2 } else { 13.5 })
                .sum()
        };
        let mut widest = measure(LOGGER_COL_NAMES[col_ix]) + 20.0;
        let entries = &self.extra_tools.logger_entries;
        let param_counts = &self.extra_tools.logger_filters.param_counts;
        let live_notes = self.history.read(cx).delegate().live_notes.borrow();
        for &(number, flow_id) in rows.iter().take(250) {
            if let Some(entry) = entries
                .get(number.saturating_sub(1))
                .filter(|e| e.id == flow_id)
                .or_else(|| entries.iter().find(|e| e.id == flow_id))
            {
                let text = logger_cell_text_for_entry(
                    col_ix,
                    number,
                    entry,
                    param_counts,
                    &live_notes,
                );
                let preview = theme::truncate_cell_preview(&text);
                widest = widest.max(measure(&preview));
            }
        }
        drop(live_notes);
        let fitted = (widest + 16.0).round().clamp(38.0, 520.0);
        self.extra_tools.logger_ui_table.borrow_mut().col_widths[col_ix] = fitted;
        cx.notify();
    }

    pub(in crate::workbench::tools_extra::logger) fn begin_logger_column_resize(
        &mut self,
        col_ix: usize,
        start_x: f32,
        _cx: &mut Context<Self>,
    ) {
        if col_ix >= LOGGER_COL_COUNT {
            return;
        }
        let mut st = self.extra_tools.logger_ui_table.borrow_mut();
        let start_w = st.col_widths[col_ix];
        st.resize_drag = Some((col_ix, start_x, start_w));
    }

    pub(in crate::workbench::tools_extra::logger) fn update_logger_column_resize(
        &mut self,
        col_ix: usize,
        current_x: f32,
        cx: &mut Context<Self>,
    ) {
        if col_ix >= LOGGER_COL_COUNT {
            return;
        }
        let scale = theme::ui_scale().max(0.75);
        let changed = {
            let mut st = self.extra_tools.logger_ui_table.borrow_mut();
            if let Some((drag_col, start_x, start_w)) = st.resize_drag
                && drag_col == col_ix
            {
                let delta_unscaled = (current_x - start_x) / scale;
                let next_w = (start_w + delta_unscaled).round().clamp(38.0, 520.0);
                if (st.col_widths[col_ix] - next_w).abs() >= 0.5 {
                    st.col_widths[col_ix] = next_w;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if changed {
            cx.notify();
        }
    }

    pub(in crate::workbench::tools_extra::logger) fn adjust_logger_column_width(
        &mut self,
        col_ix: usize,
        delta: f32,
        cx: &mut Context<Self>,
    ) {
        if col_ix >= LOGGER_COL_COUNT {
            return;
        }
        {
            let mut st = self.extra_tools.logger_ui_table.borrow_mut();
            st.col_widths[col_ix] = (st.col_widths[col_ix] + delta).clamp(38.0, 520.0);
        }
        cx.notify();
    }

    pub(in crate::workbench::tools_extra::logger) fn reset_logger_column_layout(&mut self, cx: &mut Context<Self>) {
        {
            let mut st = self.extra_tools.logger_ui_table.borrow_mut();
            st.col_widths = LOGGER_DEFAULT_COL_WIDTHS;
            st.col_visible = [true; LOGGER_COL_COUNT];
            st.resize_drag = None;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn clear_logger_pending_batch(&mut self) {
        self.extra_tools
            .logger_ui_table
            .borrow_mut()
            .pending_batch = None;
    }

    pub(in crate::workbench) fn clear_logger_selection_state(&mut self) {
        let mut st = self.extra_tools.logger_ui_table.borrow_mut();
        st.selected_set.clear();
        st.anchor_id = None;
        st.explicit_empty = None;
        st.pending_batch = None;
    }

    pub(in crate::workbench) fn logger_selected_target_ids(&self, fallback_id: FlowId) -> Vec<FlowId> {
        let selected_set = self
            .extra_tools
            .logger_ui_table
            .borrow()
            .selected_set
            .clone();
        if selected_set.contains(&fallback_id) && selected_set.len() > 1 {
            let ordered: Vec<FlowId> = self
                .extra_tools
                .logger_entries
                .iter()
                .map(|e| e.id)
                .filter(|id| selected_set.contains(id))
                .collect();
            if !ordered.is_empty() {
                return ordered;
            }
        }
        vec![fallback_id]
    }

    fn logger_live_transfer_entries(&self, ids: &[FlowId]) -> Vec<HistoryTransferEntry> {
        let id_set: HashSet<FlowId> = ids.iter().copied().collect();
        self.extra_tools
            .logger_entries
            .iter()
            .filter(|entry| id_set.contains(&entry.id) && entry.revision > 0)
            .map(|entry| HistoryTransferEntry::Live {
                flow_id: entry.id,
                revision: entry.revision,
                original: false,
            })
            .collect()
    }

    pub(in crate::workbench::tools_extra::logger) fn send_logger_batch_to_repeater(
        &mut self,
        ids: &[FlowId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.len() == 1
            && self.extra_tools.logger_selected == Some(ids[0])
            && self
                .extra_tools
                .logger_loaded_detail
                .as_deref()
                .is_some_and(|(r, _)| r.summary.id == ids[0])
        {
            self.copy_draft_to_repeater(window, cx);
            return;
        }
        let entries = self.logger_live_transfer_entries(ids);
        if entries.is_empty() {
            self.fail("所选 Logger 记录没有可转送的已落盘流量。", cx);
            return;
        }
        self.begin_history_transfer(None, entries, false, window, cx);
    }

    pub(in crate::workbench::tools_extra::logger) fn send_logger_batch_to_intruder(
        &mut self,
        ids: &[FlowId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.len() == 1
            && self.extra_tools.logger_selected == Some(ids[0])
            && self
                .extra_tools
                .logger_loaded_detail
                .as_deref()
                .is_some_and(|(r, _)| r.summary.id == ids[0])
        {
            self.open_in_intruder(window, cx);
            return;
        }
        let entries = self.logger_live_transfer_entries(ids);
        if entries.is_empty() {
            self.fail("所选 Logger 记录没有可转送的已落盘流量。", cx);
            return;
        }
        self.begin_history_transfer(None, entries, true, window, cx);
    }

    fn cached_logger_flow_record(&self, flow_id: FlowId) -> Option<FlowRecord> {
        self.extra_tools
            .logger_loaded_detail
            .as_deref()
            .filter(|(rec, _)| rec.summary.id == flow_id)
            .map(|(rec, _)| rec.clone())
            .or_else(|| {
                self.detail
                    .as_deref()
                    .filter(|rec| rec.summary.id == flow_id)
                    .cloned()
            })
    }

    pub(in crate::workbench) fn advance_logger_batch_op(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            self.clear_logger_pending_batch();
            return;
        }
        let mut next_to_read = None;
        let mut finished = None;
        loop {
            let front = self
                .extra_tools
                .logger_ui_table
                .borrow()
                .pending_batch
                .as_ref()
                .and_then(|b| b.remaining.front().copied());
            let Some(front_id) = front else {
                finished = self
                    .extra_tools
                    .logger_ui_table
                    .borrow_mut()
                    .pending_batch
                    .take();
                break;
            };
            if let Some(record) = self.cached_logger_flow_record(front_id) {
                let mut st = self.extra_tools.logger_ui_table.borrow_mut();
                if let Some(batch) = st.pending_batch.as_mut()
                    && batch.remaining.front() == Some(&front_id)
                {
                    batch.collected.push(record);
                    batch.remaining.pop_front();
                }
            } else {
                next_to_read = Some(front_id);
                break;
            }
        }
        if let Some(target_id) = next_to_read {
            self.select_logger_entry(target_id, window, cx);
            return;
        }
        let Some(done) = finished else {
            return;
        };
        if done.collected.is_empty() {
            self.fail("所选 Logger 记录没有可用的流量详情。", cx);
            return;
        }
        match done.kind {
            LoggerBatchKind::Organiser => {
                if !self.organiser_editable() {
                    return;
                }
                if let Err(err) = self.validate_organiser_composition(window, cx) {
                    self.fail(err, cx);
                    return;
                }
                if !self.flush_workspace(window, cx) {
                    return;
                }
                let mut added = 0usize;
                let mut first_id = None;
                for record in &done.collected {
                    let number = next_organiser_number(&self.workspace.organiser_items);
                    let Ok(item) = OrganiserItem::from_flow_record(number, "Logger", record) else {
                        continue;
                    };
                    let item_id = item.id;
                    self.workspace.organiser_items.push(item);
                    if validate_organiser_items(&self.workspace.organiser_items)
                        .and_then(|()| self.workspace.validate_structure())
                        .is_err()
                    {
                        self.workspace.organiser_items.pop();
                        break;
                    }
                    first_id.get_or_insert(item_id);
                    added += 1;
                }
                if added == 0 {
                    self.fail("未能将所选 Logger 记录添加到 Organiser。", cx);
                    return;
                }
                let had_selection = self.organiser.selected_id.is_some();
                if !had_selection {
                    self.organiser.selected_id = first_id;
                }
                self.sync_organiser_table(window, cx);
                if !had_selection {
                    self.reload_selected_organiser_detail(window, cx);
                }
                self.save_workspace(cx);
                self.error = None;
                self.status = format!("已批量添加 {added} 条 Logger 记录到 Organiser。");
                cx.notify();
            }
            LoggerBatchKind::Comparer { response } => {
                const MAX_COMPARER_ITEMS: usize = 64;
                const MAX_COMPARER_ITEM_BYTES: usize = 4 * 1024 * 1024;
                let mut added = 0usize;
                for record in &done.collected {
                    let bytes = if response {
                        record
                            .response
                            .as_ref()
                            .map(response_to_raw)
                            .unwrap_or_default()
                    } else {
                        request_to_raw(&record.request)
                    };
                    if bytes.is_empty() || bytes.len() > MAX_COMPARER_ITEM_BYTES {
                        continue;
                    }
                    if self.extra_tools.comparer_items.len() >= MAX_COMPARER_ITEMS {
                        break;
                    }
                    let id = self
                        .extra_tools
                        .comparer_items
                        .last()
                        .map_or(1, |item| item.id + 1);
                    let kind = if response { "响应" } else { "请求" };
                    self.extra_tools
                        .comparer_items
                        .push(crate::workbench::tools_extra::ComparerItem {
                            id,
                            label: format!("{id}: Logger {kind} ({} bytes)", bytes.len()),
                            bytes,
                        });
                    added += 1;
                }
                if added == 0 {
                    self.fail("所选 Logger 记录没有可发送到 Comparer 的有效报文。", cx);
                    return;
                }
                let count = self.extra_tools.comparer_items.len();
                self.extra_tools.comparer_selected = count - 1;
                if count >= 2 {
                    self.extra_tools.left_index = count - 2;
                    self.extra_tools.right_index = count - 1;
                }
                self.refresh_comparer(window, cx);
                self.cancel_plugin_interactions(cx);
                self.tool = Tool::Comparer;
                self.focus.focus(window, cx);
                self.error = None;
                self.status = format!("已批量发送 {added} 条 Logger 报文到 Comparer。");
                cx.notify();
            }
        }
    }

    fn start_logger_batch_op(
        &mut self,
        ids: &[FlowId],
        kind: LoggerBatchKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty()
            || self.transition.is_some()
            || self.project.is_none()
            || !self.backend_available()
        {
            return;
        }
        self.extra_tools
            .logger_ui_table
            .borrow_mut()
            .pending_batch = Some(PendingLoggerBatchOp {
            kind,
            remaining: ids.iter().copied().collect(),
            collected: Vec::with_capacity(ids.len()),
        });
        self.advance_logger_batch_op(window, cx);
    }

    pub(in crate::workbench::tools_extra::logger) fn send_logger_batch_to_organiser(
        &mut self,
        ids: &[FlowId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.len() <= 1 {
            self.send_selected_to_organiser(window, cx);
            return;
        }
        self.start_logger_batch_op(ids, LoggerBatchKind::Organiser, window, cx);
    }

    pub(in crate::workbench::tools_extra::logger) fn send_logger_batch_to_comparer(
        &mut self,
        ids: &[FlowId],
        response: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.len() <= 1 {
            self.send_to_comparer(response, window, cx);
            return;
        }
        self.start_logger_batch_op(ids, LoggerBatchKind::Comparer { response }, window, cx);
    }

    pub(in crate::workbench::tools_extra::logger) fn copy_logger_batch_urls(
        &mut self,
        ids: &[FlowId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.len() <= 1 {
            self.copy_url(window, cx);
            return;
        }
        let id_set: HashSet<FlowId> = ids.iter().copied().collect();
        let urls: Vec<String> = self
            .extra_tools
            .logger_entries
            .iter()
            .filter(|e| id_set.contains(&e.id) && !e.url.is_empty())
            .map(|e| e.url.clone())
            .collect();
        if urls.is_empty() {
            self.fail("所选 Logger 记录没有可复制的 URL。", cx);
            return;
        }
        let count = urls.len();
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(urls.join("\n")));
        self.error = None;
        self.status = format!("已复制 {count} 条 Logger URL 到剪贴板。");
        cx.notify();
    }

    pub(in crate::workbench::tools_extra::logger) fn export_logger_selected_csv(&mut self, ids: &[FlowId], cx: &mut Context<Self>) {
        let id_set: HashSet<FlowId> = ids.iter().copied().collect();
        let rows: Vec<(usize, &FlowSummary)> = self
            .extra_tools
            .logger_entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| id_set.contains(&entry.id))
            .map(|(idx, entry)| (idx + 1, entry))
            .collect();
        if rows.is_empty() {
            self.fail("没有选中的 Logger 记录可导出。", cx);
            return;
        }
        let count = rows.len();
        let csv = format_logger_csv(&rows);
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(csv));
        self.error = None;
        self.status = format!("已复制选中的 {count} 条 Logger 记录 (CSV) 到剪贴板。");
        cx.notify();
    }

    pub(in crate::workbench) fn set_logger_batch_highlight(
        &mut self,
        ids: &[FlowId],
        color: u8,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() || color > 9 || self.transition.is_some() || !self.backend_available() {
            return;
        }
        if ids.len() == 1 {
            self.set_logger_flow_highlight(ids[0], color, cx);
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let mut changes = Vec::with_capacity(ids.len());
        self.history.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            let mut live_hl = delegate.live_highlights.borrow_mut();
            for &flow_id in ids {
                let prev = live_hl
                    .get(&flow_id)
                    .cloned()
                    .unwrap_or(ExtHttpHighlight {
                        revision: 0,
                        color: 0,
                    });
                let next_rev = prev.revision.saturating_add(1);
                live_hl.insert(
                    flow_id,
                    ExtHttpHighlight {
                        revision: next_rev,
                        color,
                    },
                );
                changes.push(ExtHttpHighlightChange {
                    source_id: Uuid::nil(),
                    item_id: flow_id,
                    expected_highlight_revision: prev.revision,
                    color,
                });
            }
            drop(live_hl);
            table.refresh(cx);
        });
        let count = changes.len();
        let _ = self.enqueue(
            Command::SetExtHttpHighlights {
                project,
                operation_id: Uuid::new_v4(),
                changes,
                cancel: CancellationToken::new(),
            },
            cx,
        );
        self.error = None;
        self.status = if color == 0 {
            format!("已批量清除 {count} 条 Logger 条目高亮。")
        } else {
            format!(
                "已批量将 {count} 条 Logger 条目标记为「{}」高亮。",
                LOGGER_HIGHLIGHT_NAMES[usize::from(color)]
            )
        };
        cx.notify();
    }

    pub(in crate::workbench::tools_extra::logger) fn delete_logger_selected_entries(
        &mut self,
        ids: &[FlowId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() {
            return;
        }
        let remove_set: HashSet<FlowId> = ids.iter().copied().collect();
        let cur_selected = self.extra_tools.logger_selected;
        let selected_removed = cur_selected.is_some_and(|id| remove_set.contains(&id));
        if selected_removed && !self.leave_http_notes(window, cx) {
            return;
        }
        let next_selected = if selected_removed {
            let cur_pos = cur_selected
                .and_then(|sel| {
                    self.extra_tools
                        .logger_entries
                        .iter()
                        .position(|e| e.id == sel)
                })
                .unwrap_or(0);
            self.extra_tools.logger_entries[cur_pos..]
                .iter()
                .find(|e| !remove_set.contains(&e.id))
                .or_else(|| {
                    self.extra_tools.logger_entries[..cur_pos]
                        .iter()
                        .rev()
                        .find(|e| !remove_set.contains(&e.id))
                })
                .map(|e| e.id)
        } else {
            cur_selected
        };
        let before = self.extra_tools.logger_entries.len();
        self.extra_tools
            .logger_entries
            .retain(|entry| !remove_set.contains(&entry.id));
        let removed = before.saturating_sub(self.extra_tools.logger_entries.len());
        for id in &remove_set {
            self.extra_tools.logger_filters.param_counts.remove(id);
        }
        {
            let mut st = self.extra_tools.logger_ui_table.borrow_mut();
            st.selected_set.retain(|id| !remove_set.contains(id));
            if st.anchor_id.is_some_and(|id| remove_set.contains(&id)) {
                st.anchor_id = next_selected;
            }
            st.pending_batch = None;
        }
        if selected_removed {
            if let Some(next_id) = next_selected {
                self.select_logger_entry(next_id, window, cx);
            } else {
                self.extra_tools.logger_selected = None;
                self.extra_tools.logger_pending_read = None;
                self.extra_tools.logger_loaded_detail = None;
                self.extra_tools.logger_request.load(b"", window, cx);
                self.extra_tools.logger_response.load(b"", window, cx);
                self.clear_context_viewer("logger", cx);
                if self.http_notes.open {
                    self.select_http_notes(window, cx);
                }
            }
        }
        if removed > 0 {
            self.error = None;
            self.status = format!("已从 Logger 视图移除 {removed} 条记录。");
        }
        cx.notify();
    }
}
