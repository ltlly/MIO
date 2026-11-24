use super::*;

impl Workbench {
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
                if let Some(flow_id) = self.extra_tools.logger_selected
                    && let Some((record, _)) = self
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

    pub(super) fn history_entry_url(&self, id: HistoryEntryId, cx: &App) -> Option<String> {
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
