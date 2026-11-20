use super::*;

const SEARCH_PREVIEW_LIMIT: usize = 16 * 1024;
const MAX_COMPARER_ITEMS: usize = 32;
const MAX_COMPARER_ITEM_BYTES: usize = 256 * 1024;

impl Workbench {
    pub(in crate::workbench) fn selected_organiser_item_cloned(&self) -> Option<OrganiserItem> {
        self.organiser
            .selected_id
            .and_then(|id| {
                self.workspace
                    .organiser_items
                    .iter()
                    .find(|item| item.id == id)
            })
            .cloned()
    }

    pub(in crate::workbench) fn send_organiser_to_repeater(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        let Some(item) = self.selected_organiser_item_cloned() else {
            self.fail("请先选中一条 Organiser 条目。", cx);
            return;
        };
        let draft = Draft {
            revision: 1,
            target: resolved_organiser_target(&item),
            raw: item.request_raw,
            version: item.http_version,
        };
        let id = mio_core::Uuid::new_v4();
        let previous = self.workspace.active;
        let title = self.next_repeater_tab_title(None, Some(&draft.raw));
        self.workspace.tabs.push(WorkTab {
            id,
            title,
            group: None,
            draft,
            last_flow: None,
            archive_response: None,
            history: None,
        });
        self.workspace.active = Some(id);
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.tabs.pop();
            self.workspace.active = previous;
            self.fail(format!("未创建 Repeater 标签：{error}"), cx);
            return;
        }
        self.cancel_plugin_interactions(cx);
        self.sync_tab_editors(window, cx);
        self.save_workspace(cx);
        self.tool = Tool::Repeater;
        self.tab_editors[&id]
            .request
            .state()
            .update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    pub(in crate::workbench) fn send_organiser_to_intruder(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        let Some(item) = self.selected_organiser_item_cloned() else {
            self.fail("请先选中一条 Organiser 条目。", cx);
            return;
        };
        let draft = Draft {
            revision: 1,
            target: resolved_organiser_target(&item),
            raw: item.request_raw,
            version: item.http_version,
        };
        self.open_intruder_draft(draft, window, cx);
    }

    pub(in crate::workbench) fn send_organiser_to_comparer(
        &mut self,
        response: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() {
            return;
        }
        if let Err(err) = self.validate_organiser_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let Some(item) = self.selected_organiser_item_cloned() else {
            self.fail("请先选中一条 Organiser 条目。", cx);
            return;
        };
        let bytes = if response {
            match item.response_raw.filter(|raw| !raw.is_empty()) {
                Some(raw) => raw,
                None => {
                    self.fail("所选 Organiser 条目没有响应报文。", cx);
                    return;
                }
            }
        } else {
            item.request_raw
        };
        if bytes.len() > MAX_COMPARER_ITEM_BYTES {
            self.fail(
                format!(
                    "Comparer 单项超过 {} KiB 上限，未添加。",
                    MAX_COMPARER_ITEM_BYTES / 1024
                ),
                cx,
            );
            return;
        }
        if self.extra_tools.comparer_items.len() >= MAX_COMPARER_ITEMS {
            self.fail(
                format!("Comparer 最多保存 {MAX_COMPARER_ITEMS} 项，未添加。"),
                cx,
            );
            return;
        }
        let next_id = self
            .extra_tools
            .comparer_items
            .last()
            .map_or(1, |entry| entry.id + 1);
        let kind = if response { "响应" } else { "请求" };
        self.extra_tools
            .comparer_items
            .push(tools_extra::ComparerItem {
                id: next_id,
                label: format!(
                    "{next_id}: Organiser #{} {kind} ({} bytes)",
                    item.number,
                    bytes.len()
                ),
                bytes,
            });
        let count = self.extra_tools.comparer_items.len();
        if count >= 2 {
            self.extra_tools.left_index = count - 2;
            self.extra_tools.right_index = count - 1;
        }
        self.cancel_plugin_interactions(cx);
        self.tool = Tool::Comparer;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn copy_organiser_url(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() {
            return;
        }
        if let Err(err) = self.validate_organiser_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let Some(item) = self.selected_organiser_item_cloned() else {
            self.fail("请先选中一条 Organiser 条目。", cx);
            return;
        };
        let result = if item.target.is_empty() && item.url.starts_with("http") {
            Ok(item.url)
        } else {
            copy_url_for_request(
                &item.request_raw,
                &resolved_organiser_target(&item),
                item.http_version,
            )
            .map_err(|error| error.to_string())
        };
        match result {
            Ok(url) => {
                cx.write_to_clipboard(ClipboardItem::new_string(url));
                self.error = None;
                self.status = "已复制 URL 到剪贴板。".into();
                cx.notify();
            }
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn copy_organiser_curl(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() {
            return;
        }
        if let Err(err) = self.validate_organiser_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let Some(item) = self.selected_organiser_item_cloned() else {
            self.fail("请先选中一条 Organiser 条目。", cx);
            return;
        };
        let redact = theme::redact_sensitive_headers_in_copy_curl()
            || self
                .user_preferences
                .misc
                .redact_sensitive_headers_in_copy_curl;
        match mio_core::curl_command_for_request_with_options(
            &item.request_raw,
            &resolved_organiser_target(&item),
            item.http_version,
            redact,
        ) {
            Ok(command) => {
                cx.write_to_clipboard(ClipboardItem::new_string(command));
                self.error = None;
                self.status = "已复制 curl 命令到剪贴板。".into();
                cx.notify();
            }
            Err(error) => self.fail(error.to_string(), cx),
        }
    }

    pub(in crate::workbench) fn delete_selected_organiser_item(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.organiser.selected_id {
            self.delete_organiser_item(id, window, cx);
        }
    }

    pub(in crate::workbench) fn delete_organiser_item(
        &mut self,
        id: mio_core::Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() {
            return;
        }
        if let Err(err) = self.validate_organiser_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let Some(number) = self
            .workspace
            .organiser_items
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.number)
        else {
            return;
        };
        self.workspace.organiser_items.retain(|item| item.id != id);
        self.sync_organiser_table(window, cx);
        self.save_workspace(cx);
        self.error = None;
        self.status = format!("已从 Organiser 删除 #{number}。");
        cx.notify();
    }

    pub(in crate::workbench) fn clear_completed_organiser_items(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() {
            return;
        }
        if let Err(err) = self.validate_organiser_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let before = self.workspace.organiser_items.len();
        self.workspace
            .organiser_items
            .retain(|item| !item.workflow_status.is_terminal_cleanup_target());
        let removed = before - self.workspace.organiser_items.len();
        if removed == 0 {
            self.status = "没有已完成或已忽略的 Organiser 条目。".into();
            cx.notify();
            return;
        }
        self.sync_organiser_table(window, cx);
        self.save_workspace(cx);
        self.error = None;
        self.status = format!("已清理 {removed} 条已完成/已忽略的 Organiser 条目。");
        cx.notify();
    }

    pub(in crate::workbench) fn send_organiser_to_mio_at(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.selected_organiser_item_cloned() else {
            self.fail("请先选中一条 Organiser 条目。", cx);
            return;
        };
        let req_preview =
            String::from_utf8_lossy(&item.request_raw[..item.request_raw.len().min(4096)]);
        let resp_preview = item
            .response_raw
            .as_deref()
            .map(|r| String::from_utf8_lossy(&r[..r.len().min(4096)]).into_owned())
            .unwrap_or_default();
        let mut ctx_buf = format!(
            "Organizer Item #{}\nSource: {}\nURL: {} {}\nStatus: {}\nWorkflow: {}",
            item.number,
            item.source_tool,
            item.method,
            item.url,
            item.status_code
                .map_or_else(|| "-".into(), |c| c.to_string()),
            item.workflow_status.label(),
        );
        if !item.notes.trim().is_empty() {
            ctx_buf.push_str("\nNotes: ");
            ctx_buf.push_str(item.notes.trim());
        }
        if !req_preview.is_empty() {
            ctx_buf.push_str("\n\n--- Request ---\n");
            ctx_buf.push_str(&req_preview);
        }
        if !resp_preview.is_empty() {
            ctx_buf.push_str("\n\n--- Response ---\n");
            ctx_buf.push_str(&resp_preview);
        }
        let title = format!("Organizer #{} · {} {}", item.number, item.method, item.url);
        self.open_mio_ai_with_context(title, ctx_buf, cx);
    }

    pub(in crate::workbench) fn record_manual_issue_from_organiser(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let Some(item) = self.selected_organiser_item_cloned() else {
            self.fail("请先选中一条 Organiser 条目。", cx);
            return;
        };
        if item.url.is_empty() {
            self.fail("所选 Organiser 条目缺少有效的 URL。", cx);
            return;
        }
        let host = url::Url::parse(&item.url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_owned))
            .unwrap_or_else(|| item.host().to_owned());
        let now_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let summary = if item.notes.trim().is_empty() {
            format!("从 Organizer #{} 创建的自定义问题（{}）。", item.number, item.url)
        } else {
            format!(
                "从 Organizer #{} 创建的自定义问题（{}）。备注：{}",
                item.number,
                item.url,
                item.notes.trim()
            )
        };
        let finding = mio_core::AuditFinding {
            id: mio_core::Uuid::new_v4(),
            issue_id: "0x00800100".into(),
            title: format!("自定义问题 · {}", item.url),
            severity_label: "信息".into(),
            severity_rank: 1,
            confidence: mio_core::IssueConfidence::Certain,
            phase: mio_core::AuditPhase::Passive,
            host,
            url: item.url.clone(),
            parameter: None,
            flow_id: None,
            summary,
            evidence: item.url.clone(),
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
            self.status = format!("已记录新问题：{}", item.url);
            cx.notify();
        }
    }

    pub(in crate::workbench) fn attach_organiser_item_to_issue(
        &mut self,
        finding_id: mio_core::Uuid,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let Some(item) = self.selected_organiser_item_cloned() else {
            self.fail("请先选中一条 Organiser 条目。", cx);
            return;
        };
        if item.url.is_empty() {
            self.fail("所选 Organiser 条目缺少有效的 URL。", cx);
            return;
        }
        if self.enqueue(
            Command::Audit(mio_core::AuditCommand::AttachFlowToFinding {
                project,
                finding_id,
                flow_id: None,
                url: item.url.clone(),
                evidence: format!("Organizer #{} 关联条目：{}", item.number, item.url),
            }),
            cx,
        ) {
            self.error = None;
            self.status = format!("已将 Organizer #{} 附加到现有问题。", item.number);
            cx.notify();
        }
    }

    pub(in crate::workbench) fn generate_organiser_csrf_poc(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self.selected_organiser_item_cloned() else {
            self.fail("请先选中一条 Organiser 条目。", cx);
            return;
        };
        if item.request_raw.is_empty() {
            self.fail("所选 Organiser 条目请求报文为空，无法生成 CSRF PoC。", cx);
            return;
        }
        self.open_csrf_poc_modal(&item.url, &item.request_raw, cx);
    }

    pub(in crate::workbench) fn focus_organiser_notes_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.dismiss_inspector_for_current_selection(window, cx) {
            return;
        }
        self.organiser.notes_open = true;
        self.organiser
            .notes
            .update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    pub(in crate::workbench) fn save_selected_organiser_item_to_file(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() || self.file_picker_pending() {
            return;
        }
        let Some(item) = self.selected_organiser_item_cloned() else {
            self.fail("请先选中一条 Organiser 条目。", cx);
            return;
        };
        let time_str = super::table::format_local_time(item.added_unix_ms);
        let xml_payload = match crate::workbench::wb_render_history::build_ext_items_xml(
            &item.url,
            &item.method,
            item.status_code,
            &time_str,
            &item.request_raw,
            item.response_raw.as_deref().unwrap_or_default(),
            &item.notes,
        ) {
            Ok(xml) => xml.into_bytes(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let suggested = format!("organiser-item-{}.xml", item.number);
        let project = self.project;
        let picker = mio_core::Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &self.default_directory(),
            Some(&suggested),
        );
        let owner = self.owner_window;
        let item_number = item.number;
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
                            if path.extension().is_none() {
                                path.set_extension("xml");
                            }
                            match std::fs::write(&path, &xml_payload) {
                                Ok(()) => {
                                    view.error = None;
                                    view.status = format!(
                                        "已保存 Organizer #{item_number} 到文件：{}",
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
}

pub(super) fn resolved_organiser_target(item: &OrganiserItem) -> String {
    if !item.target.is_empty() {
        return item.target.clone();
    }
    if let Some((scheme, tail)) = item.url.split_once("://") {
        let authority_end = tail.find(['/', '?', '#']).unwrap_or(tail.len());
        return format!("{scheme}://{}", &tail[..authority_end]);
    }
    String::new()
}

pub(super) fn item_matches_query(item: &OrganiserItem, query_lower: &str) -> bool {
    if query_lower.is_empty() {
        return true;
    }
    if item.host().to_lowercase().contains(query_lower)
        || item.url.to_lowercase().contains(query_lower)
        || item.method.to_lowercase().contains(query_lower)
        || item.source_tool.to_lowercase().contains(query_lower)
        || item.notes.to_lowercase().contains(query_lower)
        || item.workflow_status.label().contains(query_lower)
        || format!("#{}", item.number).contains(query_lower)
        || item
            .status_code
            .is_some_and(|code| code.to_string().contains(query_lower))
    {
        return true;
    }
    let req_preview = &item.request_raw[..item.request_raw.len().min(SEARCH_PREVIEW_LIMIT)];
    if String::from_utf8_lossy(req_preview)
        .to_lowercase()
        .contains(query_lower)
    {
        return true;
    }
    if let Some(resp) = &item.response_raw {
        let resp_preview = &resp[..resp.len().min(SEARCH_PREVIEW_LIMIT)];
        if String::from_utf8_lossy(resp_preview)
            .to_lowercase()
            .contains(query_lower)
        {
            return true;
        }
    }
    false
}
