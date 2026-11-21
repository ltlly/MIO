use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::Instant,
};

use mio_proxy::SendConfig;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::*;

enum PendingBrowserNavigationSession {
    OriginalHeaders {
        origin: String,
        cookie_header: Option<Vec<u8>>,
        authorization_header: Option<Vec<u8>>,
    },
    CurrentCookieJar {
        origin: String,
    },
}

fn origin_key(raw_url: &str) -> Option<String> {
    let parsed = url::Url::parse(raw_url).ok()?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    let host = parsed.host_str()?.to_ascii_lowercase();
    let port = parsed.port_or_known_default()?;
    Some(format!("{scheme}://{host}:{port}"))
}

struct SessionRuntimeInner {
    project: ProjectKey,
    workspace_revision: u64,
    configured_entries: Vec<CookieJarEntry>,
    live_entries: Vec<CookieJarEntry>,
    monitor_proxy: bool,
    monitor_repeater: bool,
    monitor_intruder: bool,
    monitor_scanner: bool,
    monitor_sequencer: bool,
    monitor_extensions: bool,
    monitor_mio_ai: bool,
    rules: Vec<SessionRule>,
    macros: HashMap<Uuid, SessionMacro>,
    scope: TargetScope,
    proxy_send_config: SendConfig,
    pending_browser_navigations: Vec<PendingBrowserNavigationSession>,
    trace_tx: Option<tokio::sync::mpsc::Sender<(ProjectKey, SessionTraceEntry)>>,
}

fn summarize_request_headers(req: &HttpRequest) -> String {
    let mut lines = vec![format!("{} {}", req.method, req.url)];
    for h in req.headers.iter().take(12) {
        let val = String::from_utf8_lossy(&h.value);
        lines.push(format!("{}: {}", h.name, val.chars().take(160).collect::<String>()));
    }
    lines.join("\n")
}

#[derive(Clone)]
pub struct SessionRuntime {
    inner: Arc<RwLock<SessionRuntimeInner>>,
}

impl SessionRuntime {
    pub fn from_workspace(project: ProjectKey, workspace: &Workspace) -> Self {
        let macros = workspace
            .sessions
            .macros
            .iter()
            .map(|m| (m.id, m.clone()))
            .collect();
        Self {
            inner: Arc::new(RwLock::new(SessionRuntimeInner {
                project,
                workspace_revision: workspace.revision,
                configured_entries: workspace.sessions.cookie_jar.entries.clone(),
                live_entries: workspace.sessions.cookie_jar.entries.clone(),
                monitor_proxy: workspace.sessions.cookie_jar.monitor_proxy,
                monitor_repeater: workspace.sessions.cookie_jar.monitor_repeater,
                monitor_intruder: workspace.sessions.cookie_jar.monitor_intruder,
                monitor_scanner: workspace.sessions.cookie_jar.monitor_scanner,
                monitor_sequencer: workspace.sessions.cookie_jar.monitor_sequencer,
                monitor_extensions: workspace.sessions.cookie_jar.monitor_extensions,
                monitor_mio_ai: workspace.sessions.cookie_jar.monitor_mio_ai,
                rules: workspace.sessions.rules.clone(),
                macros,
                scope: workspace.scope.clone(),
                proxy_send_config: SendConfig::default(),
                pending_browser_navigations: Vec::new(),
                trace_tx: None,
            })),
        }
    }

    pub fn set_trace_sender(
        &self,
        tx: Option<tokio::sync::mpsc::Sender<(ProjectKey, SessionTraceEntry)>>,
    ) {
        let mut guard = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.trace_tx = tx;
    }

    fn emit_trace(&self, project: ProjectKey, entry: SessionTraceEntry) {
        let tx = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .trace_tx
            .clone();
        if let Some(tx) = tx {
            let _ = tx.try_send((project, entry));
        }
    }

    pub fn reset_project(&self, project: ProjectKey, workspace: &Workspace) {
        let mut guard = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.project = project;
        guard.workspace_revision = workspace.revision;
        guard.configured_entries = workspace.sessions.cookie_jar.entries.clone();
        guard.live_entries = workspace.sessions.cookie_jar.entries.clone();
        guard.monitor_proxy = workspace.sessions.cookie_jar.monitor_proxy;
        guard.monitor_repeater = workspace.sessions.cookie_jar.monitor_repeater;
        guard.monitor_intruder = workspace.sessions.cookie_jar.monitor_intruder;
        guard.monitor_scanner = workspace.sessions.cookie_jar.monitor_scanner;
        guard.monitor_sequencer = workspace.sessions.cookie_jar.monitor_sequencer;
        guard.monitor_extensions = workspace.sessions.cookie_jar.monitor_extensions;
        guard.monitor_mio_ai = workspace.sessions.cookie_jar.monitor_mio_ai;
        guard.rules = workspace.sessions.rules.clone();
        guard.macros = workspace
            .sessions
            .macros
            .iter()
            .map(|m| (m.id, m.clone()))
            .collect();
        guard.scope = workspace.scope.clone();
        guard.proxy_send_config = SendConfig::default();
        guard.pending_browser_navigations.clear();
    }

    /// Updates session rules, macros, and Cookie Jar with monotonic revision protection.
    /// Returns `false` if `workspace.revision` is older than the already-applied revision.
    pub fn sync_workspace(&self, project: ProjectKey, workspace: &Workspace) -> bool {
        let mut guard = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.project != project {
            drop(guard);
            self.reset_project(project, workspace);
            return true;
        }
        if workspace.revision < guard.workspace_revision {
            return false;
        }
        guard.workspace_revision = workspace.revision;
        if guard.configured_entries != workspace.sessions.cookie_jar.entries {
            guard.configured_entries = workspace.sessions.cookie_jar.entries.clone();
            guard.live_entries = workspace.sessions.cookie_jar.entries.clone();
        }
        guard.monitor_proxy = workspace.sessions.cookie_jar.monitor_proxy;
        guard.monitor_repeater = workspace.sessions.cookie_jar.monitor_repeater;
        guard.monitor_intruder = workspace.sessions.cookie_jar.monitor_intruder;
        guard.monitor_scanner = workspace.sessions.cookie_jar.monitor_scanner;
        guard.monitor_sequencer = workspace.sessions.cookie_jar.monitor_sequencer;
        guard.monitor_extensions = workspace.sessions.cookie_jar.monitor_extensions;
        guard.monitor_mio_ai = workspace.sessions.cookie_jar.monitor_mio_ai;
        guard.rules = workspace.sessions.rules.clone();
        guard.macros = workspace
            .sessions
            .macros
            .iter()
            .map(|m| (m.id, m.clone()))
            .collect();
        guard.scope = workspace.scope.clone();
        true
    }

    pub fn set_proxy_send_config(&self, config: SendConfig) {
        let mut guard = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.proxy_send_config = config;
    }

    pub fn has_enabled_rules(&self, tool: SessionTool) -> bool {
        let guard = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .rules
            .iter()
            .any(|r| r.enabled && r.tools.matches(tool))
    }

    pub fn proxy_send_config(&self) -> SendConfig {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .proxy_send_config
            .clone()
    }

    pub fn workspace_revision(&self) -> u64 {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .workspace_revision
    }

    pub fn live_cookie_jar(&self) -> Vec<CookieJarEntry> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .live_entries
            .clone()
    }

    pub fn has_proxy_hooks(&self) -> bool {
        let guard = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.monitor_proxy
            || !guard.pending_browser_navigations.is_empty()
            || guard
                .rules
                .iter()
                .any(|r| r.enabled && r.tools.matches(SessionTool::Proxy))
    }

    pub fn stage_browser_navigation_session(
        &self,
        target_url: &str,
        session: &BrowserRequestSession,
    ) -> anyhow::Result<()> {
        let Some(origin) = origin_key(target_url) else {
            return Ok(());
        };
        let next = match session {
            BrowserRequestSession::Default => return Ok(()),
            BrowserRequestSession::CurrentSession => {
                PendingBrowserNavigationSession::CurrentCookieJar { origin }
            }
            BrowserRequestSession::OriginalSession { raw_request } => {
                anyhow::ensure!(!raw_request.is_empty(), "原始请求报文不能为空");
                let parsed = crate::parse_request(raw_request, target_url, HttpVersion::Http11)
                    .or_else(|_| crate::parse_request(raw_request, target_url, HttpVersion::Http2))
                    .map_err(|e| anyhow::anyhow!("解析原始请求报文失败：{e}"))?;
                let cookies: Vec<&[u8]> = parsed
                    .headers
                    .iter()
                    .filter(|h| h.name.eq_ignore_ascii_case("cookie") && !h.value.is_empty())
                    .map(|h| h.value.as_slice())
                    .collect();
                let cookie_header = (!cookies.is_empty()).then(|| cookies.join(&b"; "[..]));
                let authorization_header = parsed
                    .headers
                    .iter()
                    .find(|h| h.name.eq_ignore_ascii_case("authorization") && !h.value.is_empty())
                    .map(|h| h.value.clone());
                PendingBrowserNavigationSession::OriginalHeaders {
                    origin,
                    cookie_header,
                    authorization_header,
                }
            }
        };
        let mut guard = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.pending_browser_navigations.len() >= 32 {
            guard.pending_browser_navigations.remove(0);
        }
        guard.pending_browser_navigations.push(next);
        Ok(())
    }

    pub fn observe_response(
        &self,
        tool: SessionTool,
        request_url: &str,
        response: &HttpResponse,
    ) -> bool {
        let has_set_cookie = response
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("set-cookie"));
        if !has_set_cookie {
            return false;
        }
        let mut guard = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let monitored = match tool {
            SessionTool::Proxy => guard.monitor_proxy,
            SessionTool::Repeater => guard.monitor_repeater,
            SessionTool::Intruder => guard.monitor_intruder,
            SessionTool::Scanner => guard.monitor_scanner,
            SessionTool::Sequencer => guard.monitor_sequencer,
            SessionTool::Extensions => guard.monitor_extensions,
        };
        if !monitored {
            return false;
        }
        let updated = ingest_set_cookie_headers(
            &mut guard.live_entries,
            request_url,
            response,
            current_unix_ms(),
        );
        let project = guard.project;
        let tx = guard.trace_tx.clone();
        let jar_count = guard.live_entries.len();
        drop(guard);
        if updated && let Some(tx) = tx {
            let _ = tx.try_send((
                project,
                SessionTraceEntry {
                    id: Uuid::new_v4(),
                    timestamp_ms: current_unix_ms(),
                    tool,
                    url: request_url.to_owned(),
                    rule_name: "Cookie Jar".into(),
                    action_summary: "更新 Cookie Jar".into(),
                    detail: format!("从响应 Set-Cookie 更新了 Cookie Jar（当前共 {jar_count} 项）"),
                    before_preview: String::new(),
                    after_preview: format!("Status {}", response.status),
                },
            ));
        }
        updated
    }

    fn merge_macro_cookies(&self, project: ProjectKey, updated_jar: &[CookieJarEntry]) {
        let mut guard = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.project != project {
            return;
        }
        let now = current_unix_ms();
        guard
            .live_entries
            .retain(|e| e.expires_unix_ms.is_none_or(|exp| now < exp));
        for item in updated_jar {
            if item.expires_unix_ms.is_some_and(|exp| now >= exp) {
                continue;
            }
            if let Some(existing) = guard.live_entries.iter_mut().find(|e| {
                e.domain == item.domain
                    && e.host_only == item.host_only
                    && e.path == item.path
                    && e.name == item.name
            }) {
                existing.value = item.value.clone();
                existing.secure = item.secure;
                existing.http_only = item.http_only;
                existing.expires_unix_ms = item.expires_unix_ms;
            } else {
                if guard.live_entries.len() >= MAX_COOKIE_JAR_ENTRIES {
                    guard.live_entries.remove(0);
                }
                guard.live_entries.push(item.clone());
            }
        }
    }

    pub async fn apply_before_send(
        &self,
        tool: SessionTool,
        request: &mut HttpRequest,
        send_config: &SendConfig,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> std::result::Result<bool, SessionExecError> {
        self.apply_before_send_with_steps(tool, request, send_config, cancel, deadline)
            .await
            .map(|(changed, _)| changed)
    }

    pub async fn apply_before_send_with_steps(
        &self,
        tool: SessionTool,
        request: &mut HttpRequest,
        send_config: &SendConfig,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> std::result::Result<(bool, Vec<(String, HttpRequest)>), SessionExecError> {
        if cancel.is_cancelled() {
            return Err(SessionExecError::Cancelled);
        }
        let before = request.clone();
        let mut rule_steps = Vec::new();
        if tool == SessionTool::Proxy
            && let Some(req_origin) = origin_key(&request.url)
        {
            let staged = {
                let mut guard = self
                    .inner
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                guard
                    .pending_browser_navigations
                    .iter()
                    .position(|item| match item {
                        PendingBrowserNavigationSession::OriginalHeaders { origin, .. }
                        | PendingBrowserNavigationSession::CurrentCookieJar { origin } => {
                            *origin == req_origin
                        }
                    })
                    .map(|idx| {
                        (
                            guard.pending_browser_navigations.remove(idx),
                            guard.live_entries.clone(),
                        )
                    })
            };
            if let Some((staged_nav, live_jar)) = staged {
                let before_nav = request.clone();
                match staged_nav {
                    PendingBrowserNavigationSession::OriginalHeaders {
                        cookie_header,
                        authorization_header,
                        ..
                    } => {
                        request
                            .headers
                            .retain(|h| !h.name.eq_ignore_ascii_case("cookie"));
                        if let Some(cookie) = cookie_header {
                            request.headers.push(Header {
                                name: "Cookie".into(),
                                value: cookie,
                            });
                        }
                        request
                            .headers
                            .retain(|h| !h.name.eq_ignore_ascii_case("authorization"));
                        if let Some(auth) = authorization_header {
                            request.headers.push(Header {
                                name: "Authorization".into(),
                                value: auth,
                            });
                        }
                        if *request != before_nav {
                            rule_steps.push(("Request in browser (Original session)".into(), request.clone()));
                        }
                    }
                    PendingBrowserNavigationSession::CurrentCookieJar { .. } => {
                        let _ = apply_cookie_jar_to_request(
                            &live_jar,
                            request,
                            true,
                            true,
                            current_unix_ms(),
                        );
                        if *request != before_nav {
                            rule_steps.push(("Request in browser (Current session)".into(), request.clone()));
                        }
                    }
                }
            }
        }
        let snapshot = {
            let guard = self
                .inner
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let has_candidate = guard
                .rules
                .iter()
                .any(|r| r.enabled && r.tools.matches(tool));
            if !has_candidate {
                return Ok((*request != before, rule_steps));
            }
            (
                guard.project,
                guard.rules.clone(),
                guard.macros.clone(),
                guard.scope.clone(),
                guard.live_entries.clone(),
            )
        };
        let (project, rules, macros, scope, mut working_jar) = snapshot;

        for rule in &rules {
            if cancel.is_cancelled() {
                return Err(SessionExecError::Cancelled);
            }
            if !rule_matches_request(rule, tool, request, &scope) {
                continue;
            }
            let before_rule = request.clone();
            let mut action_labels: Vec<String> = Vec::new();
            for action in &rule.actions {
                if cancel.is_cancelled() {
                    return Err(SessionExecError::Cancelled);
                }
                match action {
                    SessionRuleAction::UseCookieJar { .. } => {
                        action_labels.push("使用 Cookie Jar".into());
                        apply_rule_sync_action(action, &working_jar, request, current_unix_ms())
                            .map_err(|e| {
                                SessionExecError::Failed(format!(
                                    "会话规则 '{}' 动作失败：{e}",
                                    rule.name
                                ))
                            })?;
                    }
                    SessionRuleAction::SetHeader { name, .. } => {
                        action_labels.push(format!("设置请求头 {name}"));
                        apply_rule_sync_action(action, &working_jar, request, current_unix_ms())
                            .map_err(|e| {
                                SessionExecError::Failed(format!(
                                    "会话规则 '{}' 动作失败：{e}",
                                    rule.name
                                ))
                            })?;
                    }
                    SessionRuleAction::SetCookie { name, .. } => {
                        action_labels.push(format!("设置 Cookie {name}"));
                        apply_rule_sync_action(action, &working_jar, request, current_unix_ms())
                            .map_err(|e| {
                                SessionExecError::Failed(format!(
                                    "会话规则 '{}' 动作失败：{e}",
                                    rule.name
                                ))
                            })?;
                    }
                    SessionRuleAction::RewriteUrlParam { name, .. } => {
                        action_labels.push(format!("改写参数 {name}"));
                        apply_rule_sync_action(action, &working_jar, request, current_unix_ms())
                            .map_err(|e| {
                                SessionExecError::Failed(format!(
                                    "会话规则 '{}' 动作失败：{e}",
                                    rule.name
                                ))
                            })?;
                    }
                    SessionRuleAction::ReplaceRequestText { .. } => {
                        action_labels.push("替换请求文本".into());
                        apply_rule_sync_action(action, &working_jar, request, current_unix_ms())
                            .map_err(|e| {
                                SessionExecError::Failed(format!(
                                    "会话规则 '{}' 动作失败：{e}",
                                    rule.name
                                ))
                            })?;
                    }
                    SessionRuleAction::RunMacro {
                        macro_id,
                        update_cookies_from_jar,
                        apply_extractions,
                    } => {
                        let mac = macros.get(macro_id).ok_or_else(|| {
                            SessionExecError::Failed(format!(
                                "会话规则 '{}' 引用的 Macro ({macro_id}) 不存在",
                                rule.name
                            ))
                        })?;
                        action_labels.push(format!("执行 Macro {}", mac.name));
                        let extractions =
                            execute_macro(mac, &mut working_jar, send_config, cancel, deadline)
                                .await?;
                        self.merge_macro_cookies(project, &working_jar);
                        if *update_cookies_from_jar {
                            apply_cookie_jar_to_request(
                                &working_jar,
                                request,
                                true,
                                true,
                                current_unix_ms(),
                            )
                            .map_err(|e| {
                                SessionExecError::Failed(format!(
                                    "会话规则 '{}' 同步 Macro Cookie 失败：{e}",
                                    rule.name
                                ))
                            })?;
                        }
                        if *apply_extractions && !extractions.is_empty() {
                            macros::apply_extractions_to_final_request(request, &extractions)
                                .map_err(|e| {
                                    SessionExecError::Failed(format!(
                                        "会话规则 '{}' 回填提取参数失败：{e}",
                                        rule.name
                                    ))
                                })?;
                        }
                    }
                }
            }
            let modified = *request != before_rule;
            self.emit_trace(
                project,
                SessionTraceEntry {
                    id: Uuid::new_v4(),
                    timestamp_ms: current_unix_ms(),
                    tool,
                    url: request.url.clone(),
                    rule_name: rule.name.clone(),
                    action_summary: if action_labels.is_empty() {
                        "无动作".into()
                    } else {
                        action_labels.join("；")
                    },
                    detail: if modified {
                        "规则命中并已修改请求".into()
                    } else {
                        "规则命中（请求内容无变化）".into()
                    },
                    before_preview: summarize_request_headers(&before_rule),
                    after_preview: summarize_request_headers(request),
                },
            );
            if modified {
                rule_steps.push((rule.name.clone(), request.clone()));
            }
        }
        Ok((*request != before, rule_steps))
    }

    pub(crate) async fn apply_to_flow_record(
        &self,
        tool: SessionTool,
        record: &mut FlowRecord,
        send_config: &SendConfig,
        cancel: &CancellationToken,
        deadline: Instant,
        started: Instant,
    ) -> Option<bool> {
        self.apply_to_flow_record_with_steps(tool, record, send_config, cancel, deadline, started)
            .await
            .map(|(changed, _)| changed)
    }

    pub(crate) async fn apply_to_flow_record_with_steps(
        &self,
        tool: SessionTool,
        record: &mut FlowRecord,
        send_config: &SendConfig,
        cancel: &CancellationToken,
        deadline: Instant,
        started: Instant,
    ) -> Option<(bool, Vec<(String, HttpRequest)>)> {
        let mut next_request = record.request.clone();
        match self
            .apply_before_send_with_steps(tool, &mut next_request, send_config, cancel, deadline)
            .await
        {
            Ok((changed, steps)) => {
                if changed {
                    record.request = next_request;
                    record.summary.method = record.request.method.clone();
                    record.summary.url = record.request.url.clone();
                    record.summary.request_bytes = record.request.body.len() as u64;
                }
                Some((changed, steps))
            }
            Err(SessionExecError::Cancelled) => {
                record.summary.state = FlowState::Cancelled;
                record.summary.revision = record
                    .summary
                    .revision
                    .checked_add(1)
                    .filter(|next| *next <= i64::MAX as u64)
                    .unwrap_or(record.summary.revision);
                record.summary.elapsed_ms =
                    started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                None
            }
            Err(SessionExecError::Failed(msg)) => {
                record.summary.state =
                    FlowState::Failed(format!("会话规则执行失败，未发送请求：{msg}"));
                record.summary.revision = record
                    .summary
                    .revision
                    .checked_add(1)
                    .filter(|next| *next <= i64::MAX as u64)
                    .unwrap_or(record.summary.revision);
                record.summary.elapsed_ms =
                    started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                None
            }
        }
    }
}
