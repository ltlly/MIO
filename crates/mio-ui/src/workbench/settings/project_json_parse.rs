use std::collections::BTreeMap;

use super::project_json::{
    PROJECT_SETTINGS_FORMAT, ParsedProjectSettings, apply_project_settings_subset,
    export_project_settings_json,
};
use super::*;
use mio_core::*;

const MAX_JSON_DEPTH: usize = 24;

#[derive(Clone, Debug)]
pub(super) enum JVal {
    Null,
    Bool(bool),
    Num(i128),
    Str(String),
    Arr(Vec<JVal>),
    Obj(BTreeMap<String, JVal>),
}

struct JParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JParser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len()
            && matches!(self.bytes[self.pos], b' ' | b'\t' | b'\r' | b'\n')
        {
            self.pos += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), String> {
        self.skip_ws();
        if self.bytes.get(self.pos) == Some(&expected) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("项目设置 JSON 语法错误（位置 {}）", self.pos))
        }
    }

    fn parse_val(&mut self, depth: usize) -> Result<JVal, String> {
        if depth > MAX_JSON_DEPTH {
            return Err("项目设置 JSON 嵌套层级过深。".into());
        }
        self.skip_ws();
        let Some(&b) = self.bytes.get(self.pos) else {
            return Err("项目设置 JSON 意外结束。".into());
        };
        match b {
            b'"' => self.parse_str().map(JVal::Str),
            b'{' => self.parse_obj(depth + 1),
            b'[' => self.parse_arr(depth + 1),
            b't' if self.bytes[self.pos..].starts_with(b"true") => {
                self.pos += 4;
                Ok(JVal::Bool(true))
            }
            b'f' if self.bytes[self.pos..].starts_with(b"false") => {
                self.pos += 5;
                Ok(JVal::Bool(false))
            }
            b'n' if self.bytes[self.pos..].starts_with(b"null") => {
                self.pos += 4;
                Ok(JVal::Null)
            }
            b'-' | b'0'..=b'9' => self.parse_num(),
            _ => Err(format!("项目设置 JSON 语法错误（位置 {}）", self.pos)),
        }
    }

    fn parse_num(&mut self) -> Result<JVal, String> {
        let start = self.pos;
        if self.bytes.get(self.pos) == Some(&b'-') {
            self.pos += 1;
        }
        let digits_start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if self.pos == digits_start || matches!(self.bytes.get(self.pos), Some(b'.' | b'e' | b'E'))
        {
            return Err("项目设置 JSON 数值字段须为有效整数。".into());
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| "项目设置 JSON 数值编码无效。".to_owned())?;
        let val = text
            .parse::<i128>()
            .map_err(|_| format!("项目设置 JSON 整数超出范围：{text}"))?;
        Ok(JVal::Num(val))
    }

    fn parse_str(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = String::new();
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == b'"' {
                self.pos += 1;
                return Ok(out);
            }
            if b == b'\\' {
                self.pos += 1;
                let esc = *self
                    .bytes
                    .get(self.pos)
                    .ok_or_else(|| "项目设置 JSON 字符串转义未闭合。".to_owned())?;
                self.pos += 1;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000c}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let end = self.pos.saturating_add(4);
                        let hex = self
                            .bytes
                            .get(self.pos..end)
                            .and_then(|s| std::str::from_utf8(s).ok())
                            .ok_or_else(|| "项目设置 JSON Unicode 转义无效。".to_owned())?;
                        self.pos = end;
                        let mut code = u32::from_str_radix(hex, 16)
                            .map_err(|_| "项目设置 JSON Unicode 十六进制无效。".to_owned())?;
                        if (0xd800..=0xdbff).contains(&code)
                            && self.bytes.get(self.pos..self.pos + 2) == Some(b"\\u")
                        {
                            let s_end = self.pos + 6;
                            if let Some(low_hex) = self
                                .bytes
                                .get(self.pos + 2..s_end)
                                .and_then(|s| std::str::from_utf8(s).ok())
                                && let Ok(low) = u32::from_str_radix(low_hex, 16)
                                && (0xdc00..=0xdfff).contains(&low)
                            {
                                self.pos = s_end;
                                code = 0x10000 + (((code - 0xd800) << 10) | (low - 0xdc00));
                            }
                        }
                        let ch = char::from_u32(code)
                            .ok_or_else(|| "项目设置 JSON Unicode 码点无效。".to_owned())?;
                        out.push(ch);
                    }
                    _ => return Err("项目设置 JSON 包含未知转义字符。".into()),
                }
                continue;
            }
            if b < 0x20 {
                return Err("项目设置 JSON 字符串包含未转义控制字符。".into());
            }
            let rest = std::str::from_utf8(&self.bytes[self.pos..])
                .map_err(|_| "项目设置 JSON 不是有效 UTF-8。".to_owned())?;
            let ch = rest.chars().next().unwrap();
            out.push(ch);
            self.pos += ch.len_utf8();
        }
        Err("项目设置 JSON 字符串未闭合。".into())
    }

    fn parse_arr(&mut self, depth: usize) -> Result<JVal, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.bytes.get(self.pos) == Some(&b']') {
            self.pos += 1;
            return Ok(JVal::Arr(items));
        }
        loop {
            items.push(self.parse_val(depth)?);
            self.skip_ws();
            if self.bytes.get(self.pos) == Some(&b',') {
                self.pos += 1;
                continue;
            }
            self.expect(b']')?;
            return Ok(JVal::Arr(items));
        }
    }

    fn parse_obj(&mut self, depth: usize) -> Result<JVal, String> {
        self.expect(b'{')?;
        let mut map = BTreeMap::new();
        self.skip_ws();
        if self.bytes.get(self.pos) == Some(&b'}') {
            self.pos += 1;
            return Ok(JVal::Obj(map));
        }
        loop {
            self.skip_ws();
            let key = self.parse_str()?;
            self.expect(b':')?;
            let val = self.parse_val(depth)?;
            if map.insert(key.clone(), val).is_some() {
                return Err(format!("项目设置 JSON 包含重复字段 `{key}`。"));
            }
            self.skip_ws();
            if self.bytes.get(self.pos) == Some(&b',') {
                self.pos += 1;
                continue;
            }
            self.expect(b'}')?;
            return Ok(JVal::Obj(map));
        }
    }
}

pub(super) fn parse_raw_json(raw: &str) -> Result<BTreeMap<String, JVal>, String> {
    let mut parser = JParser {
        bytes: raw.as_bytes(),
        pos: 0,
    };
    let root = parser.parse_val(0)?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() {
        return Err("项目设置 JSON 末尾包含多余字符。".into());
    }
    let JVal::Obj(obj) = root else {
        return Err("项目设置 JSON 顶层必须是对象。".into());
    };
    if obj.is_empty() {
        return Err("项目设置 JSON 不能为空对象 `{}`。".into());
    }
    Ok(obj)
}

pub(super) fn check_known_keys(
    obj: &BTreeMap<String, JVal>,
    allowed: &[&str],
    ctx: &str,
) -> Result<(), String> {
    for key in obj.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("{ctx} 包含未知字段 `{key}`。"));
        }
    }
    Ok(())
}

pub(super) fn req_bool(
    obj: &BTreeMap<String, JVal>,
    key: &str,
    default: bool,
) -> Result<bool, String> {
    match obj.get(key) {
        None => Ok(default),
        Some(JVal::Bool(v)) => Ok(*v),
        _ => Err(format!("字段 `{key}` 必须是布尔值。")),
    }
}

pub(super) fn req_str(obj: &BTreeMap<String, JVal>, key: &str) -> Result<String, String> {
    match obj.get(key) {
        Some(JVal::Str(s)) => Ok(s.clone()),
        _ => Err(format!("字段 `{key}` 必须是字符串。")),
    }
}

pub(super) fn opt_str(
    obj: &BTreeMap<String, JVal>,
    key: &str,
    default: &str,
) -> Result<String, String> {
    match obj.get(key) {
        None | Some(JVal::Null) => Ok(default.to_owned()),
        Some(JVal::Str(s)) => Ok(s.clone()),
        _ => Err(format!("字段 `{key}` 必须是字符串。")),
    }
}

pub(super) fn req_num(
    obj: &BTreeMap<String, JVal>,
    key: &str,
    default: i128,
) -> Result<i128, String> {
    match obj.get(key) {
        None => Ok(default),
        Some(JVal::Num(n)) => Ok(*n),
        _ => Err(format!("字段 `{key}` 必须是整数。")),
    }
}

pub(super) fn req_uuid(obj: &BTreeMap<String, JVal>, key: &str) -> Result<Uuid, String> {
    let s = req_str(obj, key)?;
    Uuid::parse_str(&s).map_err(|_| format!("字段 `{key}` 不是有效的 UUID：{s}"))
}

pub(super) fn parse_project_settings_json(raw: &str) -> Result<ParsedProjectSettings, String> {
    let root = parse_raw_json(raw)?;
    check_known_keys(
        &root,
        &[
            "format",
            "repeater_settings",
            "network_http",
            "upstream_proxy",
            "upstream_tls",
            "proxy_listeners",
            "client_certificates",
            "scope",
            "match_and_replace_rules",
            "proxy_interception",
            "proxy_messages",
            "proxy_websocket_interception",
            "sessions",
            "tasks_auto_start",
            "scheduled_tasks",
        ],
        "项目设置 JSON",
    )?;
    if let Some(fmt) = root.get("format") {
        let JVal::Str(tag) = fmt else {
            return Err("`format` 字段必须是字符串。".into());
        };
        if tag != PROJECT_SETTINGS_FORMAT {
            return Err(format!("不支持的项目设置 JSON 版本：`{tag}`。"));
        }
    }
    if root.keys().all(|k| k == "format") {
        return Err("项目设置 JSON 未包含任何可导入的项目配置段。".into());
    }
    super::project_json_decode::decode_project_settings(&root)
}

impl Settings {
    fn validate_all_settings_ime(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), &'static str> {
        for input in [
            &self.search_input,
            &self.repeater_timeout_input,
            &self.proxy_listener_input,
            &self.upstream_host,
            &self.upstream_port,
            &self.upstream_username,
            &self.upstream_password,
            &self.connections.rule_destination,
            &self.connections.rule_host,
            &self.connections.rule_port,
            &self.connections.rule_username,
            &self.connections.rule_password,
            &self.connections.rule_description,
            &self.connections.socks_bypass_input,
            &self.connections.normal_timeout_input,
            &self.connections.open_ended_timeout_input,
            &self.connections.dns_hostname_input,
            &self.connections.dns_ip_input,
            &self.http_streaming_scope_input,
            &self.scheduled_task_time,
            &self.scheduled_task_repeat,
            &self.scheduled_task_name,
            &self.scope_include_input,
            &self.scope_exclude_input,
            &self.scope_oos_custom_input,
            &self.proxy_advanced.ssl_host,
            &self.proxy_advanced.ssl_port,
            &self.proxy_advanced.ws_match,
            &self.proxy_advanced.ws_replace,
            &self.proxy_advanced.ws_comment,
            &self.sessions.rule_name,
            &self.sessions.rule_prefixes,
            &self.sessions.rule_req_param,
            &self.sessions.cookie_domain,
            &self.sessions.cookie_name,
            &self.sessions.cookie_value,
            &self.sessions.macro_name,
            &self.sessions.step_url,
        ] {
            editing::input_snapshot(input, window, cx)?;
        }
        Ok(())
    }

    pub(super) fn export_project_settings_to_clipboard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.enabled {
            return;
        }
        if let Err(err) = self.validate_all_settings_ime(window, cx) {
            self.project_json_feedback = Some((true, err.to_owned()));
            self.settings_error(err, cx);
            cx.notify();
            return;
        }
        let project = self.state.project;
        let res = self.owner.update(cx, |view, cx| {
            if project.is_none()
                || view.project != project
                || !view.backend_available()
                || view.transition.is_some()
            {
                return Err("当前项目尚未就绪或正在切换；未导出项目设置。".to_owned());
            }
            let json = export_project_settings_json(&view.workspace)?;
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(json));
            view.error = None;
            view.status = "已复制项目设置 JSON 到剪贴板。".into();
            cx.notify();
            Ok(())
        });
        match res {
            Ok(Ok(())) => {
                self.project_json_feedback = Some((false, "已复制项目设置 JSON 到剪贴板。".into()));
            }
            Ok(Err(err)) => {
                self.project_json_feedback = Some((true, err.clone()));
                self.settings_error(err, cx);
            }
            Err(_) => {
                self.project_json_feedback = Some((true, "主工作台已关闭；未导出设置。".into()));
            }
        }
        cx.notify();
    }

    pub(super) fn import_project_settings_from_clipboard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.enabled {
            return;
        }
        if let Err(err) = self.validate_all_settings_ime(window, cx) {
            self.project_json_feedback = Some((true, err.to_owned()));
            self.settings_error(err, cx);
            cx.notify();
            return;
        }
        let Some(raw) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            let msg = "剪贴板中没有可导入的项目设置 JSON 文本。";
            self.project_json_feedback = Some((true, msg.into()));
            self.settings_error(msg, cx);
            cx.notify();
            return;
        };
        let project = self.state.project;
        let res = self.owner.update(cx, |view, cx| {
            if project.is_none()
                || view.project != project
                || !view.backend_available()
                || view.transition.is_some()
                || view.choosing_save_as
            {
                return Err("当前项目尚未就绪或正在切换；未导入项目设置。".to_owned());
            }
            let mut candidate = view.workspace.clone();
            apply_project_settings_subset(&mut candidate, &raw)?;
            let unchanged = candidate.repeater_settings == view.workspace.repeater_settings
                && candidate.network_http == view.workspace.network_http
                && candidate.upstream_proxy == view.workspace.upstream_proxy
                && candidate.upstream_tls == view.workspace.upstream_tls
                && candidate.proxy_listeners == view.workspace.proxy_listeners
                && candidate.proxy_listener == view.workspace.proxy_listener
                && candidate.client_certificates == view.workspace.client_certificates
                && candidate.scope == view.workspace.scope
                && candidate.match_and_replace_rules == view.workspace.match_and_replace_rules
                && candidate.proxy_interception == view.workspace.proxy_interception
                && candidate.proxy_messages == view.workspace.proxy_messages
                && candidate.proxy_websocket_interception
                    == view.workspace.proxy_websocket_interception
                && candidate.sessions == view.workspace.sessions
                && candidate.tasks_auto_start == view.workspace.tasks_auto_start
                && candidate.scheduled_tasks == view.workspace.scheduled_tasks;
            if !unchanged {
                view.workspace = candidate;
                let primary = view.workspace.proxy_listener_or_default().to_owned();
                view.listener_input
                    .update(cx, |input, cx| input.set_value(primary, window, cx));
                view.target_pane.advanced_mode_input = view.workspace.scope.advanced_mode;
                view.target_pane.editing_simple_scope = [None, None];
                view.target_pane.editing_advanced_scope = [None, None];
                view.target_pane.settings_editing_scope = [None, None];
                if !view.save_workspace(cx) {
                    return Err(view
                        .error
                        .clone()
                        .unwrap_or_else(|| "保存项目设置失败。".into()));
                }
            }
            view.error = None;
            view.status = "已从剪贴板导入项目设置 JSON。".into();
            cx.notify();
            Ok(())
        });
        match res {
            Ok(Ok(())) => {
                self.project_json_feedback = Some((false, "已从剪贴板导入项目设置 JSON。".into()));
            }
            Ok(Err(err)) => {
                self.project_json_feedback = Some((true, err.clone()));
                self.settings_error(err, cx);
            }
            Err(_) => {
                self.project_json_feedback = Some((true, "主工作台已关闭；未导入设置。".into()));
            }
        }
        cx.notify();
    }
}
