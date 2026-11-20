use super::*;

impl Workbench {
    pub(in crate::workbench) fn open_configured_browser(&mut self, cx: &mut Context<Self>) {
        if !self.backend_available() || self.transition.is_some() || self.ext_import.visible {
            return;
        }
        let Some(project) = self.project else {
            self.fail("项目尚未打开，无法启动配置浏览器。", cx);
            return;
        };
        if self.enqueue(
            Command::OpenConfiguredBrowser {
                project,
                url: None,
                session: mio_core::BrowserRequestSession::Default,
            },
            cx,
        ) {
            self.error = None;
            self.status = "正在启动配置浏览器…".into();
            cx.notify();
        }
    }

    pub(in crate::workbench) fn toggle_intercept(&mut self, cx: &mut Context<Self>) {
        if !self.backend_available() || self.transition.is_some() {
            return;
        }
        let Some(project) = self.project else { return };
        let enabled = !(self.intercept_requests || self.intercept_responses);
        self.enqueue(
            Command::SetIntercept {
                project,
                requests: enabled,
                responses: enabled,
            },
            cx,
        );
    }

    pub(in crate::workbench) fn resolve_intercept(
        &mut self,
        id: mio_core::Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .intercepts
            .iter()
            .any(|pending| pending.id() == id && pending.editor.has_focus(window, cx))
        {
            self.focus.focus(window, cx);
        }
        self.intercepts.retain(|pending| pending.id() != id);
        if self.selected_intercept == Some(id) {
            self.clear_context_viewer("intercept", cx);
            self.selected_intercept = self.intercepts.first().map(PendingIntercept::id);
        }
    }

    pub(in crate::workbench) fn build_intercept_command(
        project: ProjectKey,
        messages: mio_core::ProxyMessageSettings,
        pending: &PendingIntercept,
        drop: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Command, String> {
        match &pending.target {
            InterceptTarget::Http {
                stage,
                response_head_only,
                record,
            } => {
                let flow_id = record.summary.id;
                let revision = record.summary.revision;
                let decision = if drop {
                    pending.editor.bytes(window, cx)?;
                    Decision::Drop
                } else {
                    let raw = pending.editor.bytes(window, cx)?;
                    match stage {
                        InterceptStage::Request => {
                            let normalized;
                            let effective_raw = if messages.auto_fix_request_newlines {
                                normalized = normalize_intercept_request_newlines(&raw);
                                normalized.as_slice()
                            } else {
                                raw.as_slice()
                            };
                            let mut request = parse_repeater_request(
                                effective_raw,
                                &record.request.url,
                                record.request.version,
                                messages.auto_update_request_content_length,
                            )
                            .map_err(|error| format!("请求无效：{error}"))?;
                            if messages.auto_fix_request_newlines {
                                for header in &mut request.headers {
                                    if header.value.iter().any(|&b| b == b'\r' || b == b'\n') {
                                        header.value.retain(|&b| b != b'\r' && b != b'\n');
                                    }
                                }
                            }
                            Decision::ForwardRequest(request)
                        }
                        InterceptStage::Response => {
                            let version = record
                                .response
                                .as_ref()
                                .map_or(HttpVersion::Http11, |response| response.version);
                            let mut response = parse_response(&raw, version)
                                .map_err(|error| format!("响应无效：{error}"))?;
                            if *response_head_only {
                                if !response.body.is_empty() || !response.trailers.is_empty() {
                                    return Err(
                                        "SSE 响应使用流式转发，只能修改响应头；正文修改仍保留，未转发。"
                                            .into(),
                                    );
                                }
                                if (100..200).contains(&response.status) {
                                    return Err(
                                        "SSE 最终响应不能改为临时响应或协议升级；修改仍保留，未转发。"
                                            .into(),
                                    );
                                }
                            } else if messages.auto_update_response_content_length {
                                sync_intercept_response_content_length(&mut response);
                            }
                            Decision::ForwardResponse(response)
                        }
                    }
                };
                Ok(Command::Decide {
                    project,
                    flow_id,
                    revision,
                    decision,
                })
            }
            InterceptTarget::WebSocket { record, .. } => {
                let decision = if drop {
                    pending.editor.bytes(window, cx)?;
                    mio_core::WebSocketDecision::Drop
                } else {
                    let raw = pending.editor.bytes(window, cx)?;
                    if raw.len() > 16 * 1024 * 1024 {
                        return Err("WebSocket 消息超过 16 MiB 限额；修改仍保留，未转发。".into());
                    }
                    if record.kind == mio_core::WebSocketKind::Text
                        && std::str::from_utf8(&raw).is_err()
                    {
                        return Err(
                            "WebSocket 文本消息必须是有效 UTF-8；修改仍保留，未转发。".into()
                        );
                    }
                    mio_core::WebSocketDecision::Forward(raw)
                };
                Ok(Command::DecideWebSocket {
                    project,
                    message_id: record.id,
                    decision,
                })
            }
        }
    }

    pub(in crate::workbench) fn decide(
        &mut self,
        drop: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.selected_intercept else {
            return;
        };
        self.decide_for(id, drop, window, cx);
    }

    pub(in crate::workbench) fn decide_for(
        &mut self,
        id: mio_core::Uuid,
        drop: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else {
            return;
        };
        if self.selected_intercept == Some(id)
            && self.begin_plugin_response_forward(drop, window, cx)
        {
            return;
        }
        let messages = self.workspace.proxy_messages;
        let Some(pending) = self.intercepts.iter().find(|pending| pending.id() == id) else {
            return;
        };
        if pending.deciding {
            return;
        }
        let command =
            match Self::build_intercept_command(project, messages, pending, drop, window, cx) {
                Ok(command) => command,
                Err(error) => {
                    self.fail(error, cx);
                    return;
                }
            };
        if self.enqueue(command, cx)
            && let Some(pending) = self
                .intercepts
                .iter_mut()
                .find(|pending| pending.id() == id)
        {
            pending.deciding = true;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn decide_all(
        &mut self,
        drop: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else {
            return;
        };
        if !self.backend_available() || self.transition.is_some() {
            return;
        }
        let messages = self.workspace.proxy_messages;
        let mut ready: Vec<(usize, Command)> = Vec::new();
        for (idx, pending) in self.intercepts.iter().enumerate() {
            if pending.deciding {
                continue;
            }
            let command =
                match Self::build_intercept_command(project, messages, pending, drop, window, cx) {
                    Ok(command) => command,
                    Err(error) => {
                        self.fail(error, cx);
                        return;
                    }
                };
            ready.push((idx, command));
        }
        if ready.is_empty() {
            return;
        }
        let mut count = 0usize;
        for (idx, command) in ready {
            if self.enqueue(command, cx) {
                self.intercepts[idx].deciding = true;
                count += 1;
            } else {
                break;
            }
        }
        if count > 0 {
            self.status = if drop {
                format!("已提交全部丢弃（{count} 条）")
            } else {
                format!("已提交全部转发（{count} 条）")
            };
        }
        cx.notify();
    }

    pub(in crate::workbench) fn open_in_repeater(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.start_history_transfer(false, window, cx) {
            return;
        }
        if self.transition.is_some() {
            return;
        }
        if self.tool == Tool::Proxy && self.proxy_page == ProxyPage::Intercept {
            let is_ws = self
                .selected_intercept
                .and_then(|id| self.intercepts.iter().find(|pending| pending.id() == id))
                .is_some_and(|pending| matches!(pending.target, InterceptTarget::WebSocket { .. }));
            if is_ws {
                self.send_intercept_websocket_to_repeater(window, cx);
                return;
            }
        }
        if self.tool == Tool::Proxy && self.proxy_page == ProxyPage::WebSocketHistory {
            if let Some(message_id) = self.websocket.selected {
                self.open_websocket_repeater_from_history(
                    message_id,
                    self.websocket.original_view,
                    None,
                    window,
                    cx,
                );
            }
            return;
        }
        if self.tool == Tool::Intruder
            || self.tool == Tool::Repeater
            || self.tool == Tool::Logger
            || self.tool == Tool::Target
            || self.tool == Tool::Sequencer
            || self.tool == Tool::Dashboard
            || self.tool == Tool::Organiser
            || (self.tool == Tool::Proxy && self.proxy_page == ProxyPage::Intercept)
        {
            self.copy_draft_to_repeater(window, cx);
            return;
        }
        if self.tool != Tool::Proxy || self.proxy_page != ProxyPage::History {
            return;
        }
        if matches!(self.selected_history, Some(HistoryEntryId::External { .. })) {
            self.open_ext_history_transfer(false, window, cx);
            return;
        }
        let (Some(project), Some(flow_id)) = (self.project, self.selected_flow) else {
            return;
        };
        if !self.flush_workspace(window, cx) {
            return;
        }
        if self.enqueue(
            Command::OpenRepeater {
                project,
                flow_id,
                original: self.history_original_view,
            },
            cx,
        ) {
            self.tool = Tool::Repeater;
        }
    }

    pub(in crate::workbench) fn toggle_intercept_response_for_selected(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else {
            return;
        };
        if !self.backend_available() || self.transition.is_some() {
            return;
        }
        let Some(flow_id) = self
            .selected_intercept
            .and_then(|id| self.intercepts.iter().find(|pending| pending.id() == id))
            .and_then(|pending| match &pending.target {
                InterceptTarget::Http {
                    stage: InterceptStage::Request,
                    record,
                    ..
                } => Some(record.summary.id),
                _ => None,
            })
        else {
            return;
        };
        let next_annotated = !self.annotated_live_flows.contains(&flow_id);
        if next_annotated {
            self.annotated_live_flows.insert(flow_id);
        } else {
            self.annotated_live_flows.remove(&flow_id);
        }
        let _ = self.enqueue(
            Command::SetLiveFlowAnnotated {
                project,
                flow_id,
                annotated: next_annotated,
            },
            cx,
        );
        if next_annotated {
            let mut settings = self.workspace.proxy_interception.clone();
            let mut changed = false;
            if !settings.response.enabled {
                settings.response.enabled = true;
                changed = true;
            }
            let has_annotated_rule = settings.response.rules.iter().any(|rule| {
                rule.enabled
                    && rule.match_type == mio_core::ProxyInterceptType::Request
                    && rule.relationship == mio_core::ProxyInterceptRelation::WasAnnotated
            });
            if !has_annotated_rule {
                if let Some(existing) = settings.response.rules.iter_mut().find(|rule| {
                    rule.match_type == mio_core::ProxyInterceptType::Request
                        && rule.relationship == mio_core::ProxyInterceptRelation::WasAnnotated
                }) {
                    existing.enabled = true;
                    existing.operator = mio_core::ProxyInterceptOperator::Or;
                } else if settings.request.rules.len() + settings.response.rules.len() < 256 {
                    settings.response.rules.push(mio_core::ProxyInterceptRule {
                        id: mio_core::Uuid::new_v4(),
                        enabled: true,
                        operator: mio_core::ProxyInterceptOperator::Or,
                        match_type: mio_core::ProxyInterceptType::Request,
                        relationship: mio_core::ProxyInterceptRelation::WasAnnotated,
                        condition: String::new(),
                    });
                }
                changed = true;
            }
            if changed {
                self.set_proxy_interception(Some(project), settings, cx);
            }
            if !self.intercept_responses {
                self.intercept_responses = true;
                let _ = self.enqueue(
                    Command::SetIntercept {
                        project,
                        requests: self.intercept_requests,
                        responses: true,
                    },
                    cx,
                );
            }
            self.status = "已标记拦截此请求的响应。".into();
        } else {
            self.status = "已取消标记拦截此请求的响应。".into();
        }
        cx.notify();
    }

    pub(in crate::workbench) fn add_do_not_intercept_host_rule(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.selected_intercept {
            self.add_do_not_intercept_host_rule_for(id, cx);
        }
    }

    pub(in crate::workbench) fn add_do_not_intercept_host_rule_for(
        &mut self,
        target_id: mio_core::Uuid,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else {
            return;
        };
        if !self.backend_available() || self.transition.is_some() {
            return;
        }
        let Some(url) = self
            .intercepts
            .iter()
            .find(|pending| pending.id() == target_id)
            .and_then(|pending| match &pending.target {
                InterceptTarget::Http { record, .. } => Some(
                    if record.summary.url.is_empty() {
                        record.request.url.clone()
                    } else {
                        record.summary.url.clone()
                    },
                ),
                _ => None,
            })
        else {
            return;
        };
        let Some(host) = extract_url_host(&url) else {
            self.fail("当前拦截请求缺少有效的主机名称。", cx);
            return;
        };
        let condition = format!("^{}$", escape_regex_literal(&host));
        let mut settings = self.workspace.proxy_interception.clone();
        if settings.request.rules.len() + settings.response.rules.len() >= 256 {
            self.fail("Proxy 拦截规则总数量已达 256 条上限。", cx);
            return;
        }
        if let Some(existing) = settings.request.rules.iter_mut().find(|rule| {
            rule.match_type == mio_core::ProxyInterceptType::DomainName
                && rule.relationship == mio_core::ProxyInterceptRelation::DoesNotMatch
                && rule.condition == condition
        }) {
            existing.enabled = true;
            existing.operator = mio_core::ProxyInterceptOperator::And;
        } else {
            settings.request.rules.push(mio_core::ProxyInterceptRule {
                id: mio_core::Uuid::new_v4(),
                enabled: true,
                operator: mio_core::ProxyInterceptOperator::And,
                match_type: mio_core::ProxyInterceptType::DomainName,
                relationship: mio_core::ProxyInterceptRelation::DoesNotMatch,
                condition,
            });
        }
        if self.set_proxy_interception(Some(project), settings, cx) {
            self.error = None;
            self.status = format!("已添加免拦截主机规则：{host}");
            cx.notify();
        }
    }

    pub(in crate::workbench) fn add_do_not_intercept_extension_rule(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.selected_intercept {
            self.add_do_not_intercept_extension_rule_for(id, cx);
        }
    }

    pub(in crate::workbench) fn add_do_not_intercept_extension_rule_for(
        &mut self,
        target_id: mio_core::Uuid,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else {
            return;
        };
        if !self.backend_available() || self.transition.is_some() {
            return;
        }
        let Some(url) = self
            .intercepts
            .iter()
            .find(|pending| pending.id() == target_id)
            .and_then(|pending| match &pending.target {
                InterceptTarget::Http { record, .. } => Some(
                    if record.summary.url.is_empty() {
                        record.request.url.clone()
                    } else {
                        record.summary.url.clone()
                    },
                ),
                _ => None,
            })
        else {
            return;
        };
        let Some(ext) = extract_url_file_extension(&url) else {
            self.fail("当前拦截请求 URL 不包含文件扩展名。", cx);
            return;
        };
        let condition = format!("^{}$", escape_regex_literal(&ext));
        let mut settings = self.workspace.proxy_interception.clone();
        if settings.request.rules.len() + settings.response.rules.len() >= 256 {
            self.fail("Proxy 拦截规则总数量已达 256 条上限。", cx);
            return;
        }
        if let Some(existing) = settings.request.rules.iter_mut().find(|rule| {
            rule.match_type == mio_core::ProxyInterceptType::FileExtension
                && rule.relationship == mio_core::ProxyInterceptRelation::DoesNotMatch
                && rule.condition == condition
        }) {
            existing.enabled = true;
            existing.operator = mio_core::ProxyInterceptOperator::And;
        } else {
            settings.request.rules.push(mio_core::ProxyInterceptRule {
                id: mio_core::Uuid::new_v4(),
                enabled: true,
                operator: mio_core::ProxyInterceptOperator::And,
                match_type: mio_core::ProxyInterceptType::FileExtension,
                relationship: mio_core::ProxyInterceptRelation::DoesNotMatch,
                condition,
            });
        }
        if self.set_proxy_interception(Some(project), settings, cx) {
            self.error = None;
            self.status = format!("已添加免拦截扩展名规则：.{ext}");
            cx.notify();
        }
    }

    pub(crate) fn can_operate_history_menu(&self) -> bool {
        self.project.is_some() && self.backend_available() && self.transition.is_none()
    }

    pub(crate) fn send_to_passive_scanner(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() {
            return;
        }
        let draft = match self.current_transfer_draft("发起被动扫描", window, cx) {
            Ok(draft) => draft,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        self.cancel_plugin_interactions(cx);
        self.send_draft_to_scanner(
            draft,
            mio_core::AuditTaskMode::PassiveOnly,
            true,
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn send_intercept_websocket_to_decoder(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_to_decoder(window, cx);
    }

    pub(in crate::workbench) fn send_intercept_websocket_to_comparer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_to_comparer(false, window, cx);
    }

    pub(in crate::workbench) fn copy_intercept_websocket_url(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() {
            return;
        }
        let Some(url) = self
            .selected_intercept
            .and_then(|id| self.intercepts.iter().find(|pending| pending.id() == id))
            .and_then(|pending| match &pending.target {
                InterceptTarget::WebSocket { url, .. } if !url.is_empty() => Some(url.clone()),
                _ => None,
            })
        else {
            self.fail("当前没有可复制的拦截 WebSocket URL。", cx);
            return;
        };
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(url));
        self.error = None;
        self.status = "已复制 URL 到剪贴板。".into();
        cx.notify();
    }

    pub(in crate::workbench) fn modify_intercept_websocket_scope(
        &mut self,
        include: bool,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let Some(url) = self
            .selected_intercept
            .and_then(|id| self.intercepts.iter().find(|pending| pending.id() == id))
            .and_then(|pending| match &pending.target {
                InterceptTarget::WebSocket { url, .. } if !url.is_empty() => Some(url.clone()),
                _ => None,
            })
        else {
            self.fail("当前没有可更新范围的拦截 WebSocket URL。", cx);
            return;
        };
        let Some(prefix) = ws_url_to_http_origin(&url) else {
            self.fail("WebSocket URL 格式无效，无法更新范围规则。", cx);
            return;
        };
        if self.workspace.scope.advanced_mode {
            self.add_advanced_scope_rule_from_prefix(prefix, include, cx);
        } else {
            self.add_scope_rule(self.project, include, prefix, cx);
        }
    }
}

pub(in crate::workbench) fn extract_url_host(url: &str) -> Option<String> {
    let (_, tail) = url.trim().split_once("://")?;
    let authority = tail
        .split(['/', '?', '#'])
        .next()?
        .rsplit('@')
        .next()?;
    let host = if let Some(rest) = authority.strip_prefix('[') {
        let (ipv6, _) = rest.split_once(']')?;
        ipv6
    } else {
        authority.split(':').next().unwrap_or(authority)
    };
    let trimmed = host.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

pub(in crate::workbench) fn extract_url_file_extension(url: &str) -> Option<String> {
    let without_query = url.trim().split(['?', '#']).next().unwrap_or_default();
    let path = if let Some((_, tail)) = without_query.split_once("://") {
        tail.find('/').map_or("", |idx| &tail[idx..])
    } else {
        without_query
    };
    let filename = path.rsplit('/').next().unwrap_or_default();
    let (_, ext) = filename.rsplit_once('.')?;
    let trimmed = ext.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn escape_regex_literal(raw: &str) -> String {
    let mut escaped = String::with_capacity(raw.len() + 8);
    for ch in raw.chars() {
        if matches!(
            ch,
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$'
        ) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn ws_url_to_http_origin(url: &str) -> Option<String> {
    let (scheme, tail) = url.trim().split_once("://")?;
    let mapped = if scheme.eq_ignore_ascii_case("wss") || scheme.eq_ignore_ascii_case("https") {
        "https"
    } else if scheme.eq_ignore_ascii_case("ws") || scheme.eq_ignore_ascii_case("http") {
        "http"
    } else {
        return None;
    };
    let authority_end = tail.find(['/', '?', '#']).unwrap_or(tail.len());
    let authority = &tail[..authority_end];
    (!authority.is_empty()).then(|| format!("{mapped}://{authority}"))
}

fn normalize_lf_to_crlf(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 16);
    for (idx, &b) in bytes.iter().enumerate() {
        if b == b'\n' && (idx == 0 || bytes[idx - 1] != b'\r') {
            out.push(b'\r');
        }
        out.push(b);
    }
    out
}

fn find_header_body_split(raw: &[u8]) -> Option<(usize, usize)> {
    let mut candidates = Vec::with_capacity(4);
    if let Some(p) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
        candidates.push((p, 4));
    }
    if let Some(p) = raw.windows(2).position(|w| w == b"\n\n") {
        candidates.push((p, 2));
    }
    if let Some(p) = raw.windows(3).position(|w| w == b"\r\n\n") {
        candidates.push((p, 3));
    }
    if let Some(p) = raw.windows(3).position(|w| w == b"\n\r\n") {
        candidates.push((p, 3));
    }
    candidates.into_iter().min_by_key(|&(pos, _)| pos)
}

fn extract_multipart_boundary(head: &[u8]) -> Option<Vec<u8>> {
    let head_str = std::str::from_utf8(head).ok()?;
    for line in head_str.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("content-type") {
            continue;
        }
        if !value.to_ascii_lowercase().contains("multipart/") {
            continue;
        }
        for part in value.split(';') {
            let trimmed = part.trim();
            if let Some(rest) = trimmed
                .strip_prefix("boundary=")
                .or_else(|| trimmed.strip_prefix("BOUNDARY="))
                .or_else(|| {
                    trimmed
                        .get(..9)
                        .filter(|p| p.eq_ignore_ascii_case("boundary="))
                        .map(|_| &trimmed[9..])
                })
            {
                let unquoted = rest.trim().trim_matches('"');
                if !unquoted.is_empty() {
                    return Some(unquoted.as_bytes().to_vec());
                }
            }
        }
        return Some(Vec::new());
    }
    None
}

fn normalize_multipart_body_newlines(body: &[u8], boundary: &[u8]) -> Vec<u8> {
    if body.is_empty() {
        return Vec::new();
    }
    let mut out = if std::str::from_utf8(body).is_ok() {
        normalize_lf_to_crlf(body)
    } else if !boundary.is_empty() {
        // Preserve binary part payload bytes; only normalize boundary & part-header newlines.
        let mut marker = Vec::with_capacity(boundary.len() + 2);
        marker.extend_from_slice(b"--");
        marker.extend_from_slice(boundary);
        let mut res = Vec::with_capacity(body.len() + 16);
        let mut cursor = 0usize;
        let mut in_part_headers = false;
        while cursor < body.len() {
            let line_end = body[cursor..]
                .iter()
                .position(|&b| b == b'\n')
                .map(|rel| cursor + rel + 1)
                .unwrap_or(body.len());
            let line = &body[cursor..line_end];
            let trimmed = line
                .strip_suffix(b"\r\n")
                .or_else(|| line.strip_suffix(b"\n"))
                .unwrap_or(line);
            if trimmed.starts_with(&marker) {
                res.extend_from_slice(trimmed);
                if line_end <= body.len() && line.ends_with(b"\n") {
                    res.extend_from_slice(b"\r\n");
                }
                in_part_headers = !trimmed.ends_with(b"--");
            } else if in_part_headers {
                res.extend_from_slice(trimmed);
                if line.ends_with(b"\n") {
                    res.extend_from_slice(b"\r\n");
                }
                if trimmed.is_empty() {
                    in_part_headers = false;
                }
            } else {
                res.extend_from_slice(line);
            }
            cursor = line_end;
        }
        res
    } else {
        body.to_vec()
    };
    if !out.ends_with(b"\r\n") {
        if out.ends_with(b"\n") {
            out.pop();
        }
        out.extend_from_slice(b"\r\n");
    }
    out
}

pub(in crate::workbench) fn normalize_intercept_request_newlines(raw: &[u8]) -> Vec<u8> {
    if raw.is_empty() {
        return Vec::new();
    }
    let (head_bytes, body_bytes) = match find_header_body_split(raw) {
        Some((pos, delim_len)) => (&raw[..pos], &raw[pos + delim_len..]),
        None => (raw, &[][..]),
    };
    let mut normalized_head = normalize_lf_to_crlf(head_bytes);
    while normalized_head.ends_with(b"\r\n") {
        normalized_head.truncate(normalized_head.len() - 2);
    }
    let multipart_boundary = extract_multipart_boundary(&normalized_head);
    let mut out = Vec::with_capacity(normalized_head.len() + 4 + body_bytes.len() + 8);
    out.extend_from_slice(&normalized_head);
    out.extend_from_slice(b"\r\n\r\n");
    if let Some(boundary) = multipart_boundary {
        out.extend_from_slice(&normalize_multipart_body_newlines(body_bytes, &boundary));
    } else {
        out.extend_from_slice(body_bytes);
    }
    out
}

pub(in crate::workbench) fn sync_intercept_response_content_length(response: &mut HttpResponse) {
    if !response.trailers.is_empty() {
        return;
    }
    let is_chunked = response.headers.iter().any(|h| {
        h.name.eq_ignore_ascii_case("transfer-encoding")
            && h.value
                .split(|&c| c == b',')
                .any(|part| part.trim_ascii().eq_ignore_ascii_case(b"chunked"))
    });
    if is_chunked {
        return;
    }
    let body_len = response.body.len();
    let len_bytes = body_len.to_string().into_bytes();
    let mut found = false;
    response.headers.retain_mut(|h| {
        if !h.name.eq_ignore_ascii_case("content-length") {
            return true;
        }
        if found {
            return false;
        }
        h.value = len_bytes.clone();
        found = true;
        true
    });
}

#[cfg(test)]
mod intercept_proxy_message_tests {
    use super::*;

    #[test]
    fn normalize_intercept_request_newlines_fixes_lf_headers_and_multipart() {
        let lf_req = b"POST /upload HTTP/1.1\nHost: example.com\nContent-Type: multipart/form-data; boundary=abc\n\n--abc\nContent-Disposition: form-data; name=\"k\"\n\nval\n--abc--";
        let fixed = normalize_intercept_request_newlines(lf_req);
        let text = std::str::from_utf8(&fixed).unwrap();
        assert!(text.starts_with(
            "POST /upload HTTP/1.1\r\nHost: example.com\r\nContent-Type: multipart/form-data; boundary=abc\r\n\r\n"
        ));
        assert!(text.ends_with("--abc--\r\n"));
        assert!(!text.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn sync_intercept_response_content_length_updates_and_respects_chunked() {
        let mut resp = parse_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhello world",
            HttpVersion::Http11,
        )
        .unwrap();
        sync_intercept_response_content_length(&mut resp);
        let cl = resp
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("content-length"))
            .unwrap();
        assert_eq!(cl.value, b"11");

        let mut chunked = parse_response(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
            HttpVersion::Http11,
        )
        .unwrap();
        sync_intercept_response_content_length(&mut chunked);
        assert!(!chunked
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("content-length")));
    }
}
