use mio_core::{HttpRequest, HttpResponse, HttpVersion, TrafficSource};

use super::*;

pub(super) struct ContextMenuTarget {
    pub(super) source: TrafficSource,
    pub(super) request: HttpRequest,
    pub(super) response: Option<HttpResponse>,
    pub(super) selection_start: u64,
    pub(super) selection_end: u64,
    pub(super) selected_bytes: Vec<u8>,
}

impl Workbench {
    pub(super) fn extract_plugin_context_menu_target(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<ContextMenuTarget, String> {
        let (selection_start, selection_end, selected_bytes) =
            self.extract_context_menu_selection(window, cx)?;
        let (source, request, response) = self.extract_context_menu_messages(window, cx);
        Ok(ContextMenuTarget {
            source,
            request,
            response,
            selection_start,
            selection_end,
            selected_bytes,
        })
    }

    fn extract_context_menu_messages(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> (TrafficSource, HttpRequest, Option<HttpResponse>) {
        if let Some(found) = self.extract_tool_messages(self.tool, window, cx) {
            return found;
        }
        if let Some(record) = &self.detail {
            return (
                record.summary.source.clone(),
                record.request.clone(),
                record.response.clone(),
            );
        }
        if let Some(found) = self.extract_tool_messages(Tool::Repeater, window, cx) {
            return found;
        }
        (
            TrafficSource::Repeater,
            HttpRequest {
                method: "GET".into(),
                url: "http://127.0.0.1/".into(),
                version: HttpVersion::Http11,
                headers: vec![],
                body: vec![],
                trailers: vec![],
            },
            None,
        )
    }

    pub(super) fn extract_owner_messages(
        &self,
        owner: &'static str,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(HttpRequest, Option<HttpResponse>), String> {
        match owner {
            "imported-history" => {
                let Some(HistoryRow::ExtProxy(row)) = self
                    .imported_history
                    .row
                    .as_ref()
                    .filter(|row| self.selected_history == Some(row.id()))
                else {
                    return Err("当前未选中 外部归档历史条目。".into());
                };
                if !self.imported_history_request_ready() {
                    return Err("外部归档请求正文尚未读取完成。".into());
                }
                let raw = self.history_request.bytes(window, cx)?;
                if raw.is_empty() {
                    return Err("外部归档请求报文为空。".into());
                }
                let req_summary = self
                    .imported_history_request()
                    .or(row.request.as_ref())
                    .or(row.edited_request.as_ref());
                let version = req_summary
                    .and_then(|s| s.declared_version)
                    .unwrap_or(HttpVersion::Http11);
                let target = req_summary
                    .and_then(|r| r.request_target.as_deref())
                    .map(|t| {
                        if t.starts_with('/') {
                            format!("{}{t}", row.origin.as_deref().unwrap_or_default())
                        } else {
                            t.to_owned()
                        }
                    })
                    .or_else(|| row.origin.clone())
                    .unwrap_or_else(|| "http://127.0.0.1/".into());
                let req = parse_repeater_request(&raw, &target, version, false)
                    .or_else(|_| parse_request(&raw, &target, version))
                    .map_err(|e| format!("无法解析 外部归档请求：{e}"))?;
                let resp = if self.imported_history_response_ready() {
                    let resp_raw = self.history_response.bytes(window, cx)?;
                    if resp_raw.is_empty() {
                        None
                    } else {
                        Some(
                            parse_response(&resp_raw, version)
                                .map_err(|e| format!("无法解析 外部归档响应：{e}"))?,
                        )
                    }
                } else {
                    None
                };
                Ok((req, resp))
            }
            "target" => {
                let (_, req, _) = self
                    .extract_tool_messages(Tool::Target, window, cx)
                    .ok_or("当前未选中已加载的站点地图请求。")?;
                let resp = self
                    .target_pane
                    .response
                    .bytes(window, cx)
                    .ok()
                    .filter(|b| !b.is_empty())
                    .map(|b| parse_response(&b, req.version))
                    .transpose()
                    .map_err(|e| format!("无法解析站点地图响应：{e}"))?;
                Ok((req, resp))
            }
            "organiser" => {
                let (_, req, resp) = self
                    .extract_tool_messages(Tool::Organiser, window, cx)
                    .ok_or("当前未选中有效的 Organiser 条目。")?;
                Ok((req, resp))
            }
            "dashboard" => {
                let Some(finding_id) = self.audit_dashboard.selected_finding_id else {
                    return Err("当前未选中 Dashboard 漏洞条目。".into());
                };
                let Some(finding) = self
                    .audit_dashboard
                    .snapshot
                    .findings
                    .iter()
                    .find(|f| f.id == finding_id)
                else {
                    return Err("当前选中的漏洞条目不存在。".into());
                };
                let Some(record) = self
                    .audit_dashboard
                    .loaded_flow
                    .as_ref()
                    .filter(|r| Some(r.summary.id) == finding.flow_id)
                else {
                    return Err("关联请求 / 响应报文尚未加载完成。".into());
                };
                Ok((record.request.clone(), record.response.clone()))
            }
            _ => Err("不支持的插件消息视图来源。".into()),
        }
    }

    pub(super) fn extract_tool_messages(
        &self,
        tool: Tool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<(TrafficSource, HttpRequest, Option<HttpResponse>)> {
        match tool {
            Tool::Repeater => {
                let tab_id = self.workspace.active?;
                let tab = self.workspace.tabs.iter().find(|t| t.id == tab_id)?;
                let editors = self.tab_editors.get(&tab_id);
                let raw_req = editors
                    .and_then(|ed| ed.request.bytes(window, cx).ok())
                    .filter(|b| !b.is_empty())
                    .unwrap_or_else(|| tab.draft.raw.clone());
                let req = parse_repeater_request(
                    &raw_req,
                    &tab.draft.target,
                    tab.draft.version,
                    self.workspace.repeater_settings.update_content_length,
                )
                .or_else(|_| parse_request(&raw_req, &tab.draft.target, tab.draft.version))
                .ok()?;
                let resp = self.repeater_response_for_tab(tab, window, cx).or_else(|| {
                    editors
                        .and_then(|ed| ed.loaded_flow.as_ref())
                        .and_then(|flow| flow.response.clone())
                });
                Some((TrafficSource::Repeater, req, resp))
            }
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => {
                let pending = self
                    .intercepts
                    .iter()
                    .find(|p| Some(p.id()) == self.selected_intercept)?;
                let InterceptTarget::Http { stage, record, .. } = &pending.target else {
                    return None;
                };
                let req = if *stage == InterceptStage::Request {
                    pending
                        .editor
                        .bytes(window, cx)
                        .ok()
                        .and_then(|raw| {
                            parse_request(&raw, &record.request.url, record.request.version).ok()
                        })
                        .unwrap_or_else(|| record.request.clone())
                } else {
                    record.request.clone()
                };
                let resp = if *stage == InterceptStage::Response {
                    let ver = record
                        .response
                        .as_ref()
                        .map_or(HttpVersion::Http11, |r| r.version);
                    pending
                        .editor
                        .bytes(window, cx)
                        .ok()
                        .and_then(|raw| parse_response(&raw, ver).ok())
                        .or_else(|| record.response.clone())
                } else {
                    record.response.clone()
                };
                Some((TrafficSource::Proxy, req, resp))
            }
            Tool::Proxy if self.proxy_page == ProxyPage::History => {
                if let Some(record) = &self.detail {
                    let req = if self.history_request_original_view {
                        self.detail_original
                            .request
                            .as_ref()
                            .unwrap_or(&record.request)
                            .clone()
                    } else {
                        record.request.clone()
                    };
                    let resp = if self.history_response_original_view {
                        self.detail_original
                            .response
                            .as_ref()
                            .or(record.response.as_ref())
                            .cloned()
                    } else {
                        record.response.clone()
                    };
                    return Some((record.summary.source.clone(), req, resp));
                }
                let draft = self.current_transfer_draft("上下文动作", window, cx).ok()?;
                let req = parse_request(&draft.raw, &draft.target, draft.version).ok()?;
                let resp = self
                    .history_response
                    .bytes(window, cx)
                    .ok()
                    .filter(|b| !b.is_empty())
                    .and_then(|b| parse_response(&b, draft.version).ok());
                Some((TrafficSource::Proxy, req, resp))
            }
            Tool::Intruder => {
                let id = self.workspace.intruder.active?;
                let draft = self.snapshot_intruder_spec(id, window, cx).ok()?.draft;
                let req = parse_repeater_request(&draft.raw, &draft.target, draft.version, false)
                    .or_else(|_| parse_request(&draft.raw, &draft.target, draft.version))
                    .ok()?;
                Some((TrafficSource::Intruder, req, None))
            }
            Tool::Logger => {
                let flow_id = self.extra_tools.logger_selected?;
                let (record, _) = self
                    .extra_tools
                    .logger_loaded_detail
                    .as_deref()
                    .filter(|(r, _)| r.summary.id == flow_id)?;
                Some((
                    record.summary.source.clone(),
                    record.request.clone(),
                    record.response.clone(),
                ))
            }
            Tool::Organiser => {
                let item = self.selected_organiser_item_cloned()?;
                let target = if !item.target.is_empty() {
                    item.target.clone()
                } else {
                    item.url.clone()
                };
                let req =
                    parse_repeater_request(&item.request_raw, &target, item.http_version, false)
                        .or_else(|_| parse_request(&item.request_raw, &target, item.http_version))
                        .ok()?;
                let resp = item
                    .response_raw
                    .as_deref()
                    .filter(|b| !b.is_empty())
                    .and_then(|b| parse_response(b, item.http_version).ok());
                Some((TrafficSource::Repeater, req, resp))
            }
            Tool::Target | Tool::Dashboard | Tool::Sequencer => {
                let draft = self.current_transfer_draft("上下文动作", window, cx).ok()?;
                let req = parse_repeater_request(&draft.raw, &draft.target, draft.version, false)
                    .or_else(|_| parse_request(&draft.raw, &draft.target, draft.version))
                    .ok()?;
                let resp = self
                    .tool_message_bytes(true, window, cx)
                    .ok()
                    .and_then(|b| parse_response(&b, draft.version).ok());
                let src = if tool == Tool::Dashboard {
                    TrafficSource::Scanner
                } else {
                    TrafficSource::Proxy
                };
                Some((src, req, resp))
            }
            _ => None,
        }
    }

    pub(in crate::workbench) fn extract_context_menu_selection(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(u64, u64, Vec<u8>), String> {
        if self.tool == Tool::Intruder
            && let Some(id) = self.workspace.intruder.active
            && let Some(editors) = self.intruder.editors.get(&id)
        {
            let request = &editors.request;
            let state = request.state();
            if state.update(cx, |s, cx| s.marked_text_range(window, cx).is_some()) {
                return Err(editing::COMPOSITION_PENDING.into());
            }
            let range = state.read(cx).selected_range();
            if range.is_empty() {
                return Ok((0, 0, vec![]));
            }
            let content = state.read(cx).content();
            let text = content.text();
            let before_markers = text[..range.start].matches('§').count() * '§'.len_utf8();
            let end_markers = text[..range.end].matches('§').count() * '§'.len_utf8();
            let selected = text[range.clone()].replace('§', "");
            return match request.view {
                MessageView::Raw => Ok((
                    (range.start - before_markers) as u64,
                    (range.end - end_markers) as u64,
                    selected.into_bytes(),
                )),
                MessageView::Hex => {
                    let start_digits = text[..range.start]
                        .chars()
                        .filter(|&ch| ch != '§' && !ch.is_ascii_whitespace())
                        .count();
                    let bytes = editing::hex_decode(&selected)?;
                    let start = (start_digits / 2) as u64;
                    Ok((start, start + bytes.len() as u64, bytes))
                }
            };
        }
        let editor = match self.tool {
            Tool::Repeater => self
                .workspace
                .active
                .and_then(|id| self.tab_editors.get(&id))
                .and_then(|ed| pick_active_editor(&ed.request, &ed.response, window, cx)),
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => self
                .intercepts
                .iter()
                .find(|p| Some(p.id()) == self.selected_intercept)
                .map(|p| &p.editor),
            Tool::Proxy if self.proxy_page == ProxyPage::History => {
                pick_active_editor(&self.history_request, &self.history_response, window, cx)
            }
            Tool::Target if self.target_pane.page == target::TargetPage::SiteMap => {
                pick_active_editor(
                    &self.target_pane.request,
                    &self.target_pane.response,
                    window,
                    cx,
                )
            }
            Tool::Logger => pick_active_editor(
                &self.extra_tools.logger_request,
                &self.extra_tools.logger_response,
                window,
                cx,
            ),
            Tool::Organiser => pick_active_editor(
                &self.organiser.request,
                &self.organiser.response,
                window,
                cx,
            ),
            Tool::Dashboard => {
                if self.audit_dashboard.new_scan_open
                    && self
                        .audit_dashboard
                        .scan_request_editor
                        .has_focus(window, cx)
                {
                    Some(&self.audit_dashboard.scan_request_editor)
                } else {
                    pick_active_editor(
                        &self.audit_dashboard.issue_request_editor,
                        &self.audit_dashboard.issue_response_editor,
                        window,
                        cx,
                    )
                }
            }
            Tool::Sequencer => Some(&self.sequencer_pane.request),
            Tool::Extensions => self
                .workspace
                .active
                .and_then(|id| self.tab_editors.get(&id))
                .and_then(|ed| pick_active_editor(&ed.request, &ed.response, window, cx))
                .or_else(|| {
                    pick_active_editor(&self.history_request, &self.history_response, window, cx)
                }),
            _ => None,
        };
        let Some(editor) = editor else {
            return Ok((0, 0, vec![]));
        };
        read_message_editor_selection(editor, window, cx)
    }
}

fn pick_active_editor<'a>(
    req: &'a MessageEditor,
    resp: &'a MessageEditor,
    window: &Window,
    cx: &App,
) -> Option<&'a MessageEditor> {
    if resp.has_focus(window, cx) {
        Some(resp)
    } else if req.has_focus(window, cx) {
        Some(req)
    } else if !resp.state().read(cx).selected_range().is_empty() {
        Some(resp)
    } else if !req.state().read(cx).selected_range().is_empty() {
        Some(req)
    } else {
        None
    }
}

fn read_message_editor_selection(
    editor: &MessageEditor,
    window: &mut Window,
    cx: &mut App,
) -> Result<(u64, u64, Vec<u8>), String> {
    let state = editor.state();
    editing::snapshot(&state, window, cx).map_err(str::to_owned)?;
    let range = state.read(cx).selected_range();
    if range.is_empty() {
        return Ok((0, 0, vec![]));
    }
    let selected = state.read(cx).selected_value();
    if selected.is_empty() {
        return Ok((0, 0, vec![]));
    }
    if editor.view() == MessageView::Hex
        && !editor.pretty_active()
        && !editor.is_readonly_preview()
    {
        let full = state.read(cx).value();
        let before_digits = full[..range.start]
            .bytes()
            .filter(|b| !b.is_ascii_whitespace())
            .count();
        let bytes = editing::hex_decode(&selected)?;
        let start = (before_digits / 2) as u64;
        return Ok((start, start + bytes.len() as u64, bytes));
    }
    let bytes = selected.as_bytes().to_vec();
    Ok((range.start as u64, range.end as u64, bytes))
}
