use std::collections::VecDeque;

use super::*;
use mio_core::{
    InspectorMutation, InspectorSection, apply_inspector_mutation, inspect_http_messages,
    validate_target_origin,
};

#[derive(Clone, Debug)]
pub(in crate::workbench) struct SequentialGroupQueue {
    project: ProjectKey,
    group_id: mio_core::Uuid,
    mode: mio_core::RepeaterGroupSendMode,
    connection_pool_group_id: Option<u128>,
    active_tab: Option<TabId>,
    remaining: VecDeque<TabId>,
    total: usize,
    completed: usize,
}

pub(in crate::workbench) fn normalize_origin_from_candidate(raw: &str, prefer_tls: bool) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let has_http_scheme = trimmed
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        || trimmed
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"));
    let candidate = if has_http_scheme {
        trimmed.to_owned()
    } else if trimmed.contains("://") {
        return None;
    } else {
        let scheme = if prefer_tls || trimmed.ends_with(":443") {
            "https"
        } else {
            "http"
        };
        format!("{scheme}://{trimmed}")
    };
    let (_, remainder) = candidate.split_once("://")?;
    let authority_end = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    let raw_authority = &remainder[..authority_end];
    if raw_authority.is_empty() || raw_authority.contains(['@', '%', '\\']) {
        return None;
    }
    let parsed = url::Url::parse(&candidate).ok()?;
    let scheme = parsed.scheme();
    if !matches!(scheme, "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    let host = parsed.host_str()?;
    let port = parsed.port_or_known_default()?;
    if port == 0 {
        return None;
    }
    if matches!(parsed.host(), Some(url::Host::Ipv4(_))) {
        let raw_host = raw_authority
            .rsplit_once(':')
            .map_or(raw_authority, |(h, _)| h);
        if raw_host.parse::<std::net::Ipv4Addr>().is_err() {
            return None;
        }
    }
    let origin = format!("{scheme}://{host}:{port}");
    validate_target_origin(&origin).ok()?;
    Some(origin)
}

pub(in crate::workbench) fn validate_repeater_target_input(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("连接目标不能为空，请填写有效的 http:// 或 https:// 地址。".into());
    }
    if trimmed.len() > 4096 {
        return Err("连接目标长度不能超过 4096 字节。".into());
    }
    if trimmed.contains(['?', '#', '\\', '@', '%'])
        || trimmed.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("连接目标只能包含 HTTP/HTTPS 协议、主机与有效端口。".into());
    }
    let Some((_, remainder)) = trimmed.split_once("://") else {
        return Err("连接目标必须以 http:// 或 https:// 开头。".into());
    };
    if let Some((_, path)) = remainder.split_once('/')
        && !path.is_empty()
    {
        return Err("连接目标不能包含路径、查询参数或片段。".into());
    }
    let parsed = url::Url::parse(trimmed)
        .map_err(|_| "连接目标格式无效，请填写有效的 http:// 或 https:// 地址。".to_owned())?;
    let scheme = parsed.scheme();
    if !matches!(scheme, "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return Err("连接目标必须为仅包含协议、主机和端口的 HTTP/HTTPS origin。".into());
    }
    let Some(host) = parsed.host_str() else {
        return Err("连接目标缺少有效主机名。".into());
    };
    let Some(port) = parsed.port_or_known_default().filter(|&p| p > 0) else {
        return Err("连接目标端口必须在 1–65535 之间。".into());
    };
    let raw_authority = remainder.trim_end_matches('/');
    if matches!(parsed.host(), Some(url::Host::Ipv4(_))) {
        let raw_host = raw_authority
            .rsplit_once(':')
            .map_or(raw_authority, |(h, _)| h);
        if raw_host.parse::<std::net::Ipv4Addr>().is_err() {
            return Err("连接目标包含非标准 IPv4 地址格式。".into());
        }
    }
    let canonical_origin = format!("{scheme}://{host}:{port}");
    validate_target_origin(&canonical_origin).map_err(|err| err.to_string())?;
    Ok(canonical_origin)
}

pub(in crate::workbench) fn format_repeater_target_display(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Ok(parsed) = url::Url::parse(trimmed)
        && matches!(parsed.scheme(), "http" | "https")
        && let Some(host) = parsed.host_str()
    {
        let scheme = parsed.scheme();
        match parsed.port() {
            Some(port) if !((scheme == "http" && port == 80) || (scheme == "https" && port == 443)) => {
                format!("{scheme}://{host}:{port}")
            }
            _ => format!("{scheme}://{host}"),
        }
    } else {
        trimmed.to_owned()
    }
}

pub(in crate::workbench) fn toggle_target_scheme_candidate(current: &str) -> Option<String> {
    let trimmed = current.trim();
    if trimmed.is_empty() {
        return None;
    }
    let prefer_tls = trimmed
        .get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("https://"));
    let normalized = normalize_origin_from_candidate(trimmed, prefer_tls)?;
    let parsed = url::Url::parse(&normalized).ok()?;
    let scheme = parsed.scheme();
    let host = parsed.host_str()?;
    let port = parsed.port_or_known_default()?;
    let next_https = scheme != "https";
    let next_scheme = if next_https { "https" } else { "http" };
    let next_port = match (scheme, port) {
        ("http", 80) if next_https => 443,
        ("https", 443) if !next_https => 80,
        _ => port,
    };
    let origin = format!("{next_scheme}://{host}:{next_port}");
    validate_target_origin(&origin).ok()?;
    Some(origin)
}

pub(in crate::workbench) fn extract_origin_from_request_host_header(
    raw_request: &[u8],
    use_https: bool,
) -> Result<String, String> {
    let inspected = inspect_http_messages(raw_request, None);
    let host_value = inspected
        .request_headers
        .iter()
        .find(|h| {
            h.name.eq_ignore_ascii_case("host") || h.name.eq_ignore_ascii_case(":authority")
        })
        .map(|h| h.value.trim().to_owned())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| "当前请求报文中未找到有效的 Host 标头。".to_owned())?;
    let raw_host = host_value
        .strip_prefix("http://")
        .or_else(|| host_value.strip_prefix("https://"))
        .unwrap_or(&host_value);
    let explicit_origin = if use_https {
        format!("https://{raw_host}")
    } else {
        format!("http://{raw_host}")
    };
    normalize_origin_from_candidate(&explicit_origin, use_https)
        .ok_or_else(|| format!("Host 标头「{host_value}」无法解析为合法目标地址。"))
}

pub(in crate::workbench) fn format_host_header_for_origin(origin: &str) -> Option<String> {
    let parsed = url::Url::parse(origin.trim()).ok()?;
    let scheme = parsed.scheme();
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let host = parsed.host_str()?;
    let port = parsed.port_or_known_default()?;
    let default_port = if scheme == "https" { 443 } else { 80 };
    if port == default_port {
        Some(host.to_owned())
    } else {
        Some(format!("{host}:{port}"))
    }
}

pub(in crate::workbench) fn rewrite_request_host_header(raw: &[u8], host_value: &str) -> Result<Vec<u8>, String> {
    let inspected = inspect_http_messages(raw, None);
    if inspected.attributes.is_empty() {
        return Err("请求报文无法解析，不能更新 Host 标头。".into());
    }
    if let Some(index) = inspected
        .request_headers
        .iter()
        .position(|h| h.name.eq_ignore_ascii_case("host"))
    {
        apply_inspector_mutation(
            raw,
            InspectorMutation::Update {
                section: InspectorSection::RequestHeaders,
                index,
                name: "Host".into(),
                value: host_value.to_owned(),
            },
        )
    } else {
        apply_inspector_mutation(
            raw,
            InspectorMutation::Add {
                section: InspectorSection::RequestHeaders,
                name: "Host".into(),
                value: host_value.to_owned(),
            },
        )
    }
}

/// Computes the exact serialized wire byte length of an `HttpResponse`
/// (status line + headers + CRLFCRLF + body/trailers), matching `response_to_raw(response).len()`
/// without allocating a raw buffer on every render frame.
pub(in crate::workbench) fn raw_response_byte_len(response: &HttpResponse) -> usize {
    let status_digits = match response.status {
        0..=9 => 1,
        10..=99 => 2,
        100..=999 => 3,
        1000..=9999 => 4,
        _ => 5,
    };
    let reason_extra = response
        .reason_phrase
        .as_deref()
        .filter(|reason| !reason.is_empty())
        .map_or(0, |reason| 1 + reason.len());
    let mut len = response.version.as_str().len() + 1 + status_digits + reason_extra + 2;
    let chunked = !response.trailers.is_empty();
    for field in &response.headers {
        if chunked
            && (field.name.eq_ignore_ascii_case("content-length")
                || field.name.eq_ignore_ascii_case("transfer-encoding"))
        {
            continue;
        }
        len += field.name.len() + 2 + field.value.len() + 2;
    }
    if chunked {
        len += b"Transfer-Encoding: chunked\r\n".len();
    }
    len += 2;
    if chunked {
        if !response.body.is_empty() {
            let hex_digits = ((usize::BITS - response.body.len().leading_zeros()) as usize)
                .div_ceil(4);
            len += hex_digits + 2 + response.body.len() + 2;
        }
        len += b"0\r\n".len();
        for field in &response.trailers {
            len += field.name.len() + 2 + field.value.len() + 2;
        }
        len += 2;
    } else {
        len += response.body.len();
    }
    len
}

impl Workbench {
    pub(in crate::workbench) fn has_active_sequential_group_queue(&self) -> bool {
        let Some(project) = self.project else {
            return false;
        };
        self.sequential_group_queue
            .as_ref()
            .is_some_and(|q| q.project == project && (q.active_tab.is_some() || !q.remaining.is_empty()))
    }

    pub(in crate::workbench) fn can_send(&self, tab_id: TabId) -> bool {
        self.project.is_some()
            && self.backend_available()
            && self.transition.is_none()
            && self.pending_records == 0
            && !self.sending.contains_key(&tab_id)
            && !self.pending_sends.contains_key(&tab_id)
            && !self.plugin_waiting(tab_id)
    }

    pub(in crate::workbench) fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tool != Tool::Repeater || self.transition.is_some() {
            return;
        }
        if self.workspace.active.is_none()
            && let Some(ws_id) = self.workspace.active_websocket_tab
        {
            self.send_websocket_repeater(ws_id, window, cx);
            return;
        }
        if self.pending_records > 0 {
            self.fail("仍有流量尚未写入项目，请先保存或另存为恢复存储。", cx);
            return;
        }
        let (Some(_), Some(tab_id)) = (self.project, self.workspace.active) else {
            return;
        };
        if !self.can_send(tab_id) || self.begin_plugin_send(tab_id, window, cx) {
            return;
        }
        if !self.flush_workspace(window, cx) {
            return;
        }
        let Some(tab) = self.workspace.tabs.iter().find(|tab| tab.id == tab_id) else {
            return;
        };
        let send = self.capture_send(tab.draft.clone());
        if self.pending_workspace.is_some() || self.workspace_dirty {
            self.pending_sends.insert(tab_id, send);
            self.status = "正在保存当前草稿与 Repeater 设置；保存成功后发送。".into();
        } else {
            self.enqueue_send(tab_id, send, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn send_active_request(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send(window, cx);
    }

    pub(in crate::workbench) fn can_send_group(&self, group_id: mio_core::Uuid) -> bool {
        self.workspace
            .tabs
            .iter()
            .any(|tab| tab.group == Some(group_id) && self.can_send(tab.id))
    }

    pub(in crate::workbench) fn send_group(
        &mut self,
        group_id: mio_core::Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_group_with_mode(
            group_id,
            mio_core::RepeaterGroupSendMode::ParallelSinglePacketAttack,
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn send_group_single_packet_parallel(
        &mut self,
        group_id: mio_core::Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_group_with_mode(
            group_id,
            mio_core::RepeaterGroupSendMode::ParallelSinglePacketAttack,
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn send_group_parallel_separate_connections(
        &mut self,
        group_id: mio_core::Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_group_with_mode(
            group_id,
            mio_core::RepeaterGroupSendMode::ParallelSeparateConnections,
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn send_group_with_mode(
        &mut self,
        group_id: mio_core::Uuid,
        mode: mio_core::RepeaterGroupSendMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        if self.pending_records > 0 {
            self.fail("仍有流量尚未写入项目，请先保存或另存为恢复存储。", cx);
            return;
        }
        if self.project.is_none() || !self.flush_workspace(window, cx) {
            return;
        }
        self.sequential_group_queue = None;
        let ids: Vec<TabId> = self
            .workspace
            .tabs
            .iter()
            .filter(|tab| tab.group == Some(group_id))
            .map(|tab| tab.id)
            .collect();
        if ids.is_empty() {
            return;
        }
        let mut ready_sends: Vec<(TabId, PendingSend)> = Vec::new();
        let mut dispatched = 0usize;
        for tab_id in ids {
            if self.sending.len() + self.pending_sends.len() + ready_sends.len() >= 16 {
                self.fail(
                    format!("并发 Repeater 请求达到 16 个限额；已提交 {dispatched} 个标签发送。"),
                    cx,
                );
                break;
            }
            if !self.can_send(tab_id) || self.begin_plugin_send(tab_id, window, cx) {
                continue;
            }
            let Some(draft) = self
                .workspace
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .map(|tab| tab.draft.clone())
            else {
                continue;
            };
            let send = self.capture_send(draft);
            ready_sends.push((tab_id, send));
        }
        let barrier = (mode == mio_core::RepeaterGroupSendMode::ParallelSinglePacketAttack
            && !ready_sends.is_empty())
        .then(|| mio_core::SinglePacketBarrier::new(ready_sends.len()));
        let pool_group_id = (mode == mio_core::RepeaterGroupSendMode::ParallelSinglePacketAttack)
            .then(|| mio_core::Uuid::new_v4().as_u128());
        for (tab_id, mut send) in ready_sends {
            send.group_send = Some(mio_core::RepeaterGroupSendTicket {
                mode,
                connection_pool_group_id: pool_group_id,
                single_packet_barrier: barrier.clone(),
            });
            if self.pending_workspace.is_some() || self.workspace_dirty {
                self.pending_sends.insert(tab_id, send);
                dispatched += 1;
            } else if self.enqueue_send(tab_id, send, cx) {
                dispatched += 1;
            } else {
                break;
            }
        }
        if dispatched > 0 {
            self.tool = Tool::Repeater;
            self.status = if self.pending_workspace.is_some() || self.workspace_dirty {
                format!("正在保存当前草稿与设置；保存成功后发送本组 {dispatched} 个请求。")
            } else {
                format!("Repeater 已开始发送本组 {dispatched} 个请求…")
            };
        }
        cx.notify();
    }

    pub(in crate::workbench) fn send_group_sequential(
        &mut self,
        group_id: mio_core::Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_group_sequential_with_mode(
            group_id,
            mio_core::RepeaterGroupSendMode::SequentialSingleConnection,
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn send_group_sequential_single_connection(
        &mut self,
        group_id: mio_core::Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_group_sequential_with_mode(
            group_id,
            mio_core::RepeaterGroupSendMode::SequentialSingleConnection,
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn send_group_sequential_separate_connections(
        &mut self,
        group_id: mio_core::Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_group_sequential_with_mode(
            group_id,
            mio_core::RepeaterGroupSendMode::SequentialSeparateConnections,
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn send_group_sequential_with_mode(
        &mut self,
        group_id: mio_core::Uuid,
        mode: mio_core::RepeaterGroupSendMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        if self.pending_records > 0 {
            self.fail("仍有流量尚未写入项目，请先保存或另存为恢复存储。", cx);
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        if !self.flush_workspace(window, cx) {
            return;
        }
        let candidates: VecDeque<TabId> = self
            .workspace
            .tabs
            .iter()
            .filter(|tab| tab.group == Some(group_id) && self.can_send(tab.id))
            .map(|tab| tab.id)
            .collect();
        if candidates.is_empty() {
            return;
        }
        let total = candidates.len();
        let connection_pool_group_id =
            (mode == mio_core::RepeaterGroupSendMode::SequentialSingleConnection)
                .then(|| mio_core::Uuid::new_v4().as_u128());
        self.sequential_group_queue = Some(SequentialGroupQueue {
            project,
            group_id,
            mode,
            connection_pool_group_id,
            active_tab: None,
            remaining: candidates,
            total,
            completed: 0,
        });
        self.tool = Tool::Repeater;
        self.advance_sequential_group_queue(window, cx);
        cx.notify();
    }

    fn advance_sequential_group_queue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project else {
            self.sequential_group_queue = None;
            return;
        };
        if !self.backend_available() || self.transition.is_some() || self.pending_records > 0 {
            self.sequential_group_queue = None;
            return;
        }
        loop {
            let next_step = match self.sequential_group_queue.as_mut() {
                None => None,
                Some(state) if state.project != project => {
                    self.sequential_group_queue = None;
                    None
                }
                Some(state) if state.active_tab.is_some() => None,
                Some(state) => match state.remaining.pop_front() {
                    Some(tab_id) => Some((
                        Some(tab_id),
                        state.completed,
                        state.total,
                        state.group_id,
                        state.mode,
                        state.connection_pool_group_id,
                    )),
                    None => {
                        let summary = (
                            None,
                            state.completed,
                            state.total,
                            state.group_id,
                            state.mode,
                            state.connection_pool_group_id,
                        );
                        self.sequential_group_queue = None;
                        Some(summary)
                    }
                },
            };
            let Some((maybe_tab, completed, total, group_id, mode, connection_pool_group_id)) =
                next_step
            else {
                return;
            };
            let Some(tab_id) = maybe_tab else {
                if completed > 0 {
                    self.status = format!("Repeater 已按顺序完成本组 {completed}/{total} 个请求发送。");
                }
                return;
            };
            let still_in_group = self
                .workspace
                .tabs
                .iter()
                .any(|tab| tab.id == tab_id && tab.group == Some(group_id));
            if !still_in_group || !self.can_send(tab_id) {
                continue;
            }
            if self.sending.len() + self.pending_sends.len() >= 16 {
                self.sequential_group_queue = None;
                self.fail("并发 Repeater 请求达到 16 个限额；顺序发送队列已终止。", cx);
                return;
            }
            if !self.flush_workspace(window, cx) {
                self.sequential_group_queue = None;
                return;
            }
            if self.begin_plugin_send(tab_id, window, cx) {
                if let Some(state) = self.sequential_group_queue.as_mut() {
                    state.active_tab = Some(tab_id);
                }
                return;
            }
            let Some(draft) = self
                .workspace
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .map(|tab| tab.draft.clone())
            else {
                continue;
            };
            let mut send = self.capture_send(draft);
            let step_no = completed + 1;
            send.group_send = Some(mio_core::RepeaterGroupSendTicket {
                mode,
                connection_pool_group_id,
                single_packet_barrier: None,
            });
            if self.pending_workspace.is_some() || self.workspace_dirty {
                self.pending_sends.insert(tab_id, send);
                if let Some(state) = self.sequential_group_queue.as_mut() {
                    state.active_tab = Some(tab_id);
                }
                self.status = format!(
                    "正在保存草稿；保存成功后顺序发送本组第 {step_no}/{total} 个请求。"
                );
                return;
            }
            if self.enqueue_send(tab_id, send, cx) {
                if let Some(state) = self.sequential_group_queue.as_mut() {
                    state.active_tab = Some(tab_id);
                }
                self.status = format!("Repeater 正在顺序发送本组第 {step_no}/{total} 个请求…");
                return;
            } else {
                self.sequential_group_queue = None;
                return;
            }
        }
    }

    pub(in crate::workbench) fn repeater_group_send_finished(
        &mut self,
        tab_id: TabId,
        cancelled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else {
            self.sequential_group_queue = None;
            return;
        };
        let should_advance = match self.sequential_group_queue.as_mut() {
            None => false,
            Some(state) if state.project != project => {
                self.sequential_group_queue = None;
                false
            }
            Some(state) if state.active_tab != Some(tab_id) => false,
            Some(_) if cancelled => {
                self.sequential_group_queue = None;
                false
            }
            Some(state) => {
                state.active_tab = None;
                state.completed += 1;
                true
            }
        };
        if should_advance {
            self.advance_sequential_group_queue(window, cx);
        }
    }

    pub(in crate::workbench) fn repeater_response_for_tab(
        &self,
        tab: &WorkTab,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<HttpResponse> {
        let editors = self.tab_editors.get(&tab.id)?;
        let bytes = editors.response.bytes(window, cx).ok()?;
        if bytes.is_empty() {
            return None;
        }
        parse_response(&bytes, tab.draft.version).ok()
    }

    pub(in crate::workbench) fn can_follow_repeater_redirect(
        &self,
        tab: &WorkTab,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let Some(response) = self.repeater_response_for_tab(tab, window, cx) else {
            return false;
        };
        matches!(response.status, 301 | 302 | 303 | 307 | 308)
            && response
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("location") && !h.value.is_empty())
    }

    pub(in crate::workbench) fn follow_repeater_redirection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tool != Tool::Repeater || self.transition.is_some() {
            return;
        }
        if !self.flush_workspace(window, cx) {
            return;
        }
        let Some(tab_id) = self.workspace.active else {
            self.fail("当前没有可跟随的重定向响应。", cx);
            return;
        };
        if !self.can_send(tab_id) || self.extensions.active_messages.contains_key(&tab_id) {
            return;
        }
        let Some(tab) = self
            .workspace
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .cloned()
        else {
            self.fail("当前没有可跟随的重定向响应。", cx);
            return;
        };
        let Some(response) = self.repeater_response_for_tab(&tab, window, cx) else {
            self.fail("当前没有可跟随的重定向响应。", cx);
            return;
        };
        let current_request = match parse_repeater_request(
            &tab.draft.raw,
            &tab.draft.target,
            tab.draft.version,
            self.workspace.repeater_settings.update_content_length,
        ) {
            Ok(request) => request,
            Err(error) => {
                self.fail(format!("无法解析当前请求：{error}"), cx);
                return;
            }
        };
        match mio_core::resolve_redirect_request(
            &tab.draft.target,
            &current_request,
            &response,
            RepeaterRedirectMode::Always,
        ) {
            Ok(Some((next_origin, next_request, next_raw))) => {
                let next_revision = tab.draft.revision + 1;
                if let Some(active_tab) = self.workspace.tabs.iter_mut().find(|t| t.id == tab_id) {
                    active_tab.draft.target = next_origin.clone();
                    active_tab.draft.raw = next_raw.clone();
                    active_tab.draft.version = next_request.version;
                    active_tab.draft.revision = next_revision;
                }
                if let Some(editors) = self.tab_editors.get_mut(&tab_id) {
                    editors
                        .target
                        .update(cx, |state, cx| state.set_value(next_origin, window, cx));
                    editors.request.load(&next_raw, window, cx);
                    editors.loaded_revision = next_revision;
                    editors.dirty = false;
                }
                self.save_workspace(cx);
                self.send_active_request(window, cx);
            }
            Ok(None) => {
                self.fail("当前没有可跟随的重定向响应。", cx);
            }
            Err(error) => {
                self.fail(format!("无法跟随重定向：{error}"), cx);
            }
        }
    }

    pub(in crate::workbench) fn render_repeater_with_redirect(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_repeater(window, cx)
    }

    pub(in crate::workbench) fn capture_send(&self, draft: Draft) -> PendingSend {
        let required_workspace_revision =
            self.pending_workspace.unwrap_or(self.workspace.revision)
                + u64::from(self.workspace_dirty);
        PendingSend {
            draft,
            settings: self.workspace.repeater_settings.clone(),
            group_send: None,
            required_workspace_revision,
        }
    }

    pub(in crate::workbench) fn enqueue_send(
        &mut self,
        tab_id: TabId,
        send: PendingSend,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(project) = self.project else {
            return false;
        };
        let revision = send.draft.revision;
        let history_cursor = self
            .workspace
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.history.as_ref())
            .and_then(|history| history.cursor.clone());
        let run_id = mio_core::Uuid::new_v4();
        let cancel = CancellationToken::new();
        if self.enqueue(
            Command::Send {
                project,
                tab_id,
                run_id,
                draft: send.draft,
                settings: send.settings,
                group_send: send.group_send,
                cancel: cancel.clone(),
            },
            cx,
        ) {
            self.sending.insert(
                tab_id,
                RepeaterSend {
                    run_id,
                    draft_revision: revision,
                    history_cursor,
                    cancel,
                },
            );
            self.invalidate_archive_response(tab_id, cx);
            self.status = "Repeater 正在发送…".into();
            true
        } else {
            false
        }
    }

    pub(in crate::workbench) fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.tool == Tool::Sequencer {
            self.stop_sequencer_capture(cx);
            return;
        }
        if self.tool != Tool::Repeater {
            return;
        }
        let queued_tab = self
            .sequential_group_queue
            .take()
            .and_then(|q| q.active_tab);
        let (Some(_), Some(tab_id)) = (self.project, self.workspace.active) else {
            if let Some(q_tab) = queued_tab {
                self.cancel_plugin_send(q_tab, cx);
                if self.pending_sends.remove(&q_tab).is_none() {
                    self.cancel_send(q_tab, cx);
                }
            }
            return;
        };
        if let Some(q_tab) = queued_tab
            && q_tab != tab_id
        {
            self.cancel_plugin_send(q_tab, cx);
            if self.pending_sends.remove(&q_tab).is_some() {
                self.status = "顺序发送已取消，剩余队列已终止。".into();
                cx.notify();
                return;
            }
            if self.cancel_send(q_tab, cx) {
                return;
            }
        }
        self.cancel_plugin_send(tab_id, cx);
        if self.pending_sends.remove(&tab_id).is_some() {
            self.status = "发送已取消，尚未建立网络连接。".into();
            cx.notify();
        } else {
            self.cancel_send(tab_id, cx);
        }
    }

    pub(in crate::workbench) fn cancel_send(
        &mut self,
        tab_id: TabId,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(project) = self.project else {
            return false;
        };
        let Some(send) = self.sending.get(&tab_id) else {
            return false;
        };
        if self
            .sequential_group_queue
            .as_ref()
            .is_some_and(|q| q.active_tab == Some(tab_id))
        {
            self.sequential_group_queue = None;
        }
        if send.cancel.is_cancelled() {
            return true;
        }
        send.cancel.cancel();
        let command = Command::CancelSend {
            project,
            tab_id,
            run_id: send.run_id,
            draft_revision: send.draft_revision,
        };
        self.status = "Repeater 正在取消；等待后台确认。".into();
        self.enqueue(command, cx);
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests;
