use mio_core::{
    CookieJarEntry, Header, MacroApplyTarget, MacroExtractSource, MacroExtraction, MacroStep,
    SessionMacro, SessionTool, sessions::normalize_cookie_domain,
};

use super::*;

impl Settings {
    pub(super) fn set_cookie_jar_monitor(
        &mut self,
        tool: SessionTool,
        checked: bool,
        cx: &mut Context<Self>,
    ) {
        let mut next = self.state.sessions.clone();
        match tool {
            SessionTool::Proxy => next.cookie_jar.monitor_proxy = checked,
            SessionTool::Repeater => next.cookie_jar.monitor_repeater = checked,
            SessionTool::Intruder => next.cookie_jar.monitor_intruder = checked,
            SessionTool::Scanner => next.cookie_jar.monitor_scanner = checked,
            SessionTool::Sequencer => next.cookie_jar.monitor_sequencer = checked,
            SessionTool::Extensions => next.cookie_jar.monitor_extensions = checked,
            SessionTool::MioAi => next.cookie_jar.monitor_mio_ai = checked,
        }
        let _ = self.commit_session_settings(next, cx);
    }

    pub(super) fn save_cookie_jar_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw_domain = match editing::input_snapshot(&self.sessions.cookie_domain, window, cx) {
            Ok(v) => v.to_string(),
            Err(err) => return self.report_local_input_err(err, cx),
        };
        let raw_path = match editing::input_snapshot(&self.sessions.cookie_path, window, cx) {
            Ok(v) => v.trim().to_owned(),
            Err(err) => return self.report_local_input_err(err, cx),
        };
        let name = match editing::input_snapshot(&self.sessions.cookie_name, window, cx) {
            Ok(v) => v.trim().to_owned(),
            Err(err) => return self.report_local_input_err(err, cx),
        };
        let value = match editing::input_snapshot(&self.sessions.cookie_value, window, cx) {
            Ok(v) => v.to_string(),
            Err(err) => return self.report_local_input_err(err, cx),
        };
        let expires_raw = match editing::input_snapshot(&self.sessions.cookie_expires, window, cx) {
            Ok(v) => v.trim().to_owned(),
            Err(err) => return self.report_local_input_err(err, cx),
        };
        let domain = match normalize_cookie_domain(&raw_domain) {
            Ok(d) => d,
            Err(err) => return self.report_local_input_err(&err.to_string(), cx),
        };
        let has_leading_dot = raw_domain.trim().starts_with('.');
        let host_only = !self.sessions.cookie_subdomains && !has_leading_dot;
        let path = if raw_path.is_empty() {
            "/".to_owned()
        } else {
            raw_path
        };
        let expires_unix_ms = if expires_raw.is_empty() {
            None
        } else {
            match expires_raw.parse::<i64>() {
                Ok(ms) if ms > 0 => Some(ms),
                _ => {
                    return self
                        .report_local_input_err("Cookie 过期时间须为正整数 Unix 毫秒或留空。", cx);
                }
            }
        };

        let id = self.sessions.editing_cookie_id.unwrap_or_else(Uuid::new_v4);
        let entry = CookieJarEntry {
            id,
            domain,
            host_only,
            path,
            name,
            value,
            secure: self.sessions.cookie_secure,
            http_only: self.sessions.cookie_http_only,
            expires_unix_ms,
        };
        if let Err(err) = entry.validate() {
            return self.report_local_input_err(&err.to_string(), cx);
        }
        let mut next = self.state.sessions.clone();
        if let Some(edit_id) = self.sessions.editing_cookie_id
            && let Some(slot) = next.cookie_jar.entries.iter_mut().find(|e| e.id == edit_id)
        {
            *slot = entry.clone();
        } else if let Some(slot) = next.cookie_jar.entries.iter_mut().find(|e| {
            e.domain == entry.domain
                && e.host_only == entry.host_only
                && e.path == entry.path
                && e.name == entry.name
        }) {
            let existing_id = slot.id;
            *slot = CookieJarEntry {
                id: existing_id,
                ..entry.clone()
            };
        } else {
            next.cookie_jar.entries.push(entry.clone());
        }
        if self.commit_session_settings(next, cx) {
            self.sessions.editing_cookie_id = None;
            self.sessions
                .cookie_domain
                .update(cx, |s, cx| s.set_value("", window, cx));
            self.sessions
                .cookie_path
                .update(cx, |s, cx| s.set_value("/", window, cx));
            self.sessions
                .cookie_name
                .update(cx, |s, cx| s.set_value("", window, cx));
            self.sessions
                .cookie_value
                .update(cx, |s, cx| s.set_value("", window, cx));
            self.sessions
                .cookie_expires
                .update(cx, |s, cx| s.set_value("", window, cx));
            cx.notify();
        }
    }

    pub(super) fn load_selected_cookie_for_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.sessions.selected_cookie else {
            return;
        };
        let Some(entry) = self
            .state
            .sessions
            .cookie_jar
            .entries
            .iter()
            .find(|e| e.id == id)
            .cloned()
        else {
            return;
        };
        self.sessions.editing_cookie_id = Some(entry.id);
        self.sessions.cookie_subdomains = !entry.host_only;
        self.sessions.cookie_secure = entry.secure;
        self.sessions.cookie_http_only = entry.http_only;
        self.sessions
            .cookie_domain
            .update(cx, |s, cx| s.set_value(entry.domain, window, cx));
        self.sessions
            .cookie_path
            .update(cx, |s, cx| s.set_value(entry.path, window, cx));
        self.sessions
            .cookie_name
            .update(cx, |s, cx| s.set_value(entry.name, window, cx));
        self.sessions
            .cookie_value
            .update(cx, |s, cx| s.set_value(entry.value, window, cx));
        let exp_str = entry
            .expires_unix_ms
            .map_or_else(String::new, |v| v.to_string());
        self.sessions
            .cookie_expires
            .update(cx, |s, cx| s.set_value(exp_str, window, cx));
        self.sessions.local_error = None;
        cx.notify();
    }

    pub(super) fn remove_selected_cookie(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.sessions.selected_cookie else {
            return;
        };
        let mut next = self.state.sessions.clone();
        next.cookie_jar.entries.retain(|e| e.id != id);
        if self.commit_session_settings(next, cx) {
            self.sessions.selected_cookie = None;
            if self.sessions.editing_cookie_id == Some(id) {
                self.sessions.editing_cookie_id = None;
            }
        }
    }

    pub(super) fn clear_cookie_jar(&mut self, cx: &mut Context<Self>) {
        if self.state.sessions.cookie_jar.entries.is_empty() {
            return;
        }
        let mut next = self.state.sessions.clone();
        next.cookie_jar.entries.clear();
        if self.commit_session_settings(next, cx) {
            self.sessions.selected_cookie = None;
            self.sessions.editing_cookie_id = None;
        }
    }

    pub(super) fn import_active_macro_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.enabled {
            return;
        }
        for input in [
            self.sessions.step_method.clone(),
            self.sessions.step_url.clone(),
            self.sessions.step_headers.clone(),
            self.sessions.step_body.clone(),
        ] {
            if let Err(err) = editing::input_snapshot(&input, window, cx) {
                return self.report_local_input_err(err, cx);
            }
        }
        let Some(owner) = self.owner.upgrade() else {
            return self.report_local_input_err("主工作台已关闭；未载入请求。", cx);
        };
        let skip_cookie = self.sessions.step_use_jar;
        let imported = {
            let view = owner.read(cx);
            if view.project != self.state.project || view.transition.is_some() {
                Err("当前项目正在切换；未载入步骤请求。".to_owned())
            } else {
                macro_recorder::snapshot_active_request_for_macro(view, skip_cookie, cx)
            }
        };
        let draft = match imported {
            Ok(draft) => draft,
            Err(err) => return self.report_local_input_err(&err, cx),
        };
        self.sessions
            .step_method
            .update(cx, |s, cx| s.set_value(draft.method, window, cx));
        self.sessions
            .step_url
            .update(cx, |s, cx| s.set_value(draft.url, window, cx));
        self.sessions
            .step_headers
            .update(cx, |s, cx| s.set_value(draft.headers_line, window, cx));
        self.sessions
            .step_body
            .update(cx, |s, cx| s.set_value(draft.body_text, window, cx));
        self.sessions.local_error = None;
        cx.notify();
    }

    pub(super) fn add_draft_macro_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let method = match editing::input_snapshot(&self.sessions.step_method, window, cx) {
            Ok(v) => v.trim().to_ascii_uppercase(),
            Err(err) => return self.report_local_input_err(err, cx),
        };
        let url = match editing::input_snapshot(&self.sessions.step_url, window, cx) {
            Ok(v) => v.trim().to_owned(),
            Err(err) => return self.report_local_input_err(err, cx),
        };
        let raw_headers = match editing::input_snapshot(&self.sessions.step_headers, window, cx) {
            Ok(v) => v.to_string(),
            Err(err) => return self.report_local_input_err(err, cx),
        };
        let body = match editing::input_snapshot(&self.sessions.step_body, window, cx) {
            Ok(v) => v.as_bytes().to_vec(),
            Err(err) => return self.report_local_input_err(err, cx),
        };
        let status_raw = match editing::input_snapshot(&self.sessions.step_status, window, cx) {
            Ok(v) => v.trim().to_owned(),
            Err(err) => return self.report_local_input_err(err, cx),
        };
        let ext_name = match editing::input_snapshot(&self.sessions.ext_name, window, cx) {
            Ok(v) => v.trim().to_owned(),
            Err(err) => return self.report_local_input_err(err, cx),
        };
        let ext_arg1 = match editing::input_snapshot(&self.sessions.ext_arg1, window, cx) {
            Ok(v) => v.to_string(),
            Err(err) => return self.report_local_input_err(err, cx),
        };
        let ext_arg2 = match editing::input_snapshot(&self.sessions.ext_arg2, window, cx) {
            Ok(v) => v.to_string(),
            Err(err) => return self.report_local_input_err(err, cx),
        };
        let ext_target = match editing::input_snapshot(&self.sessions.ext_target_name, window, cx) {
            Ok(v) => v.trim().to_owned(),
            Err(err) => return self.report_local_input_err(err, cx),
        };

        if !mio_core::sessions::valid_macro_step_url(&url) {
            return self.report_local_input_err("Macro 步骤 URL 必须为合法 http/https 地址。", cx);
        }
        let expected_status = if status_raw.is_empty() {
            None
        } else {
            match status_raw.parse::<u16>() {
                Ok(code) if (100..=599).contains(&code) => Some(code),
                _ => return self.report_local_input_err("预期状态码须在 100 到 599 之间。", cx),
            }
        };
        let mut headers = Vec::new();
        for seg in raw_headers
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let Some((h_name, h_val)) = seg.split_once(':') else {
                return self
                    .report_local_input_err("附加请求头格式须为 Name: Value（分号分隔）。", cx);
            };
            headers.push(Header {
                name: h_name.trim().to_owned(),
                value: h_val.trim().as_bytes().to_vec(),
            });
        }
        let mut extractions = Vec::new();
        if !ext_name.is_empty() {
            let source = match self.sessions.ext_kind {
                DraftExtractKind::Delimiter => {
                    if ext_arg1.is_empty() || ext_arg2.is_empty() {
                        return self
                            .report_local_input_err("分隔符提取必须提供非空的起止分隔符。", cx);
                    }
                    MacroExtractSource::Delimiter {
                        start: ext_arg1,
                        end: ext_arg2,
                    }
                }
                DraftExtractKind::Regex => {
                    if ext_arg1.is_empty() {
                        return self.report_local_input_err("正则提取必须提供正则表达式。", cx);
                    }
                    MacroExtractSource::Regex { pattern: ext_arg1 }
                }
                DraftExtractKind::ResponseHeader => {
                    if ext_arg1.trim().is_empty() {
                        return self.report_local_input_err("响应头提取必须提供响应头名称。", cx);
                    }
                    MacroExtractSource::ResponseHeader {
                        header_name: ext_arg1.trim().to_owned(),
                    }
                }
            };
            let target = match self.sessions.ext_target_kind {
                DraftTargetKind::UrlOrBodyParam => MacroApplyTarget::UrlOrBodyParam,
                DraftTargetKind::Header => MacroApplyTarget::Header {
                    header_name: ext_target,
                },
                DraftTargetKind::Cookie => MacroApplyTarget::Cookie {
                    cookie_name: ext_target,
                },
            };
            extractions.push(MacroExtraction {
                name: ext_name,
                source,
                target,
            });
        }
        self.sessions.draft_steps.push(MacroStep {
            id: Uuid::new_v4(),
            method: if method.is_empty() {
                "GET".into()
            } else {
                method
            },
            url,
            headers,
            body,
            use_cookie_jar: self.sessions.step_use_jar,
            expected_status,
            extractions,
        });
        self.sessions
            .step_url
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.sessions
            .ext_name
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.sessions.local_error = None;
        cx.notify();
    }

    pub(super) fn save_session_macro(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = match editing::input_snapshot(&self.sessions.macro_name, window, cx) {
            Ok(v) => v.trim().to_owned(),
            Err(err) => return self.report_local_input_err(err, cx),
        };
        let retries_raw = match editing::input_snapshot(&self.sessions.macro_retries, window, cx) {
            Ok(v) => v.trim().to_owned(),
            Err(err) => return self.report_local_input_err(err, cx),
        };
        let timeout_raw = match editing::input_snapshot(&self.sessions.macro_timeout_ms, window, cx)
        {
            Ok(v) => v.trim().to_owned(),
            Err(err) => return self.report_local_input_err(err, cx),
        };
        if name.is_empty() {
            return self.report_local_input_err("Macro 名称不能为空。", cx);
        }
        if self.sessions.draft_steps.is_empty() {
            return self.report_local_input_err("保存 Macro 前请至少添加一个请求步骤。", cx);
        }
        let failure_policy = match self.sessions.macro_failure_policy {
            MacroFailurePolicy::Abort => MacroFailurePolicy::Abort,
            MacroFailurePolicy::Retry { .. } => {
                let Ok(max_retries) = retries_raw.parse::<u8>() else {
                    return self.report_local_input_err("Macro 重试次数须为 1–3。", cx);
                };
                MacroFailurePolicy::Retry { max_retries }
            }
        };
        let Ok(timeout_ms) = timeout_raw.parse::<u64>() else {
            return self.report_local_input_err("Macro 超时毫秒须为 100–60000。", cx);
        };
        let mac = SessionMacro {
            id: self.sessions.editing_macro_id.unwrap_or_else(Uuid::new_v4),
            name,
            steps: self.sessions.draft_steps.clone(),
            failure_policy,
            timeout_ms,
        };
        let mut next = self.state.sessions.clone();
        if let Some(edit_id) = self.sessions.editing_macro_id
            && let Some(slot) = next.macros.iter_mut().find(|m| m.id == edit_id)
        {
            *slot = mac.clone();
        } else {
            next.macros.push(mac.clone());
        }
        if self.commit_session_settings(next, cx) {
            self.sessions.selected_macro = Some(mac.id);
            self.sessions.editing_macro_id = None;
            self.sessions.macro_editor_open = false;
            self.sessions.draft_steps.clear();
            self.sessions
                .macro_name
                .update(cx, |s, cx| s.set_value("", window, cx));
            cx.notify();
        }
    }

    pub(super) fn open_new_macro_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sessions.editing_macro_id = None;
        self.sessions.macro_editor_open = true;
        self.sessions.draft_steps.clear();
        self.sessions.macro_failure_policy = MacroFailurePolicy::Abort;
        self.sessions
            .macro_name
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.sessions
            .macro_retries
            .update(cx, |s, cx| s.set_value("1", window, cx));
        self.sessions
            .macro_timeout_ms
            .update(cx, |s, cx| s.set_value("10000", window, cx));
        self.sessions.local_error = None;
        cx.notify();
    }

    pub(super) fn load_selected_macro_for_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.sessions.selected_macro else {
            return;
        };
        let Some(mac) = self
            .state
            .sessions
            .macros
            .iter()
            .find(|m| m.id == id)
            .cloned()
        else {
            return;
        };
        self.sessions.editing_macro_id = Some(mac.id);
        self.sessions.macro_editor_open = true;
        self.sessions.draft_steps = mac.steps.clone();
        self.sessions.macro_failure_policy = mac.failure_policy;
        let retries = match mac.failure_policy {
            MacroFailurePolicy::Abort => "1".to_owned(),
            MacroFailurePolicy::Retry { max_retries } => max_retries.to_string(),
        };
        self.sessions
            .macro_name
            .update(cx, |s, cx| s.set_value(mac.name, window, cx));
        self.sessions
            .macro_retries
            .update(cx, |s, cx| s.set_value(retries, window, cx));
        self.sessions
            .macro_timeout_ms
            .update(cx, |s, cx| s.set_value(mac.timeout_ms.to_string(), window, cx));
        self.sessions.local_error = None;
        cx.notify();
    }

    pub(super) fn remove_selected_macro(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.sessions.selected_macro else {
            return;
        };
        if self.state.sessions.rules.iter().any(|r| {
            r.actions.iter().any(
                |a| matches!(a, SessionRuleAction::RunMacro { macro_id, .. } if *macro_id == id),
            )
        }) {
            return self.report_local_input_err(
                "该 Macro 正被至少一条 Session 规则引用，请先删除或修改对应规则。",
                cx,
            );
        }
        let mut next = self.state.sessions.clone();
        next.macros.retain(|m| m.id != id);
        if self.commit_session_settings(next, cx) {
            self.sessions.selected_macro = None;
            if self.sessions.editing_macro_id == Some(id) {
                self.sessions.editing_macro_id = None;
                self.sessions.macro_editor_open = false;
            }
        }
    }

    pub(super) fn report_local_input_err(&mut self, msg: &str, cx: &mut Context<Self>) {
        self.sessions.local_error = Some(msg.to_owned());
        self.settings_error(msg, cx);
        cx.notify();
    }
}
