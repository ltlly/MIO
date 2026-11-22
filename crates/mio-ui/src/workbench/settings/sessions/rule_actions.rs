use mio_core::{SessionParamCondition, SessionRule, SessionRuleCondition, SessionUrlScope};

use super::*;

impl Settings {
    pub(super) fn commit_session_settings(
        &mut self,
        next: SessionSettings,
        cx: &mut Context<Self>,
    ) -> bool {
        let project = self.state.project;
        let snapshot = next.clone();
        let res = self.owner.update(cx, |view, cx| {
            view.apply_session_settings(project, next, cx)
        });
        match res {
            Ok(Ok(())) => {
                self.state.sessions = snapshot;
                self.sessions.local_error = None;
                cx.notify();
                true
            }
            Ok(Err(err)) => {
                self.sessions.local_error = Some(err);
                cx.notify();
                false
            }
            Err(_) => {
                self.sessions.local_error = Some("主窗口已关闭；会话设置未修改。".into());
                cx.notify();
                false
            }
        }
    }

    #[cfg(test)]
    pub(super) fn add_draft_rule_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.add_draft_rule_action_with_jar_opts(true, true, window, cx);
    }

    pub(super) fn add_draft_rule_action_with_jar_opts(
        &mut self,
        jar_add_new_cookies: bool,
        jar_update_existing_cookies: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let action = match self.sessions.draft_action_kind {
            DraftActionKind::UseCookieJar => SessionRuleAction::UseCookieJar {
                add_new_cookies: jar_add_new_cookies,
                update_existing_cookies: jar_update_existing_cookies,
            },
            DraftActionKind::SetHeader => {
                let name =
                    match editing::input_snapshot(&self.sessions.action_header_name, window, cx) {
                        Ok(v) => v.trim().to_owned(),
                        Err(err) => {
                            self.sessions.local_error = Some(err.into());
                            self.settings_error(err, cx);
                            cx.notify();
                            return;
                        }
                    };
                let value =
                    match editing::input_snapshot(&self.sessions.action_header_val, window, cx) {
                        Ok(v) => v.to_string(),
                        Err(err) => {
                            self.sessions.local_error = Some(err.into());
                            self.settings_error(err, cx);
                            cx.notify();
                            return;
                        }
                    };
                if name.is_empty() {
                    let msg = "SetHeader 动作的请求头名称不能为空。";
                    self.sessions.local_error = Some(msg.into());
                    self.settings_error(msg, cx);
                    cx.notify();
                    return;
                }
                SessionRuleAction::SetHeader {
                    name,
                    value,
                    only_if_present: self.sessions.action_only_present,
                }
            }
            DraftActionKind::SetCookie => {
                let name =
                    match editing::input_snapshot(&self.sessions.action_cookie_name, window, cx) {
                        Ok(v) => v.trim().to_owned(),
                        Err(err) => {
                            self.sessions.local_error = Some(err.into());
                            self.settings_error(err, cx);
                            cx.notify();
                            return;
                        }
                    };
                let value =
                    match editing::input_snapshot(&self.sessions.action_cookie_val, window, cx) {
                        Ok(v) => v.to_string(),
                        Err(err) => {
                            self.sessions.local_error = Some(err.into());
                            self.settings_error(err, cx);
                            cx.notify();
                            return;
                        }
                    };
                if !mio_core::sessions::valid_cookie_name(&name) {
                    let msg = "SetCookie 动作的 Cookie 名称无效或为空。";
                    self.sessions.local_error = Some(msg.into());
                    self.settings_error(msg, cx);
                    cx.notify();
                    return;
                }
                if !mio_core::sessions::valid_cookie_value(&value) {
                    let msg = "SetCookie 动作的 Cookie 值包含非法字符或分号。";
                    self.sessions.local_error = Some(msg.into());
                    self.settings_error(msg, cx);
                    cx.notify();
                    return;
                }
                SessionRuleAction::SetCookie {
                    name,
                    value,
                    only_if_present: self.sessions.action_only_present,
                }
            }
            DraftActionKind::RewriteUrlParam => {
                let name =
                    match editing::input_snapshot(&self.sessions.action_param_name, window, cx) {
                        Ok(v) => v.trim().to_owned(),
                        Err(err) => {
                            self.sessions.local_error = Some(err.into());
                            self.settings_error(err, cx);
                            cx.notify();
                            return;
                        }
                    };
                let value =
                    match editing::input_snapshot(&self.sessions.action_param_val, window, cx) {
                        Ok(v) => v.to_string(),
                        Err(err) => {
                            self.sessions.local_error = Some(err.into());
                            self.settings_error(err, cx);
                            cx.notify();
                            return;
                        }
                    };
                if name.is_empty() {
                    let msg = "RewriteUrlParam 动作的参数名称不能为空。";
                    self.sessions.local_error = Some(msg.into());
                    self.settings_error(msg, cx);
                    cx.notify();
                    return;
                }
                SessionRuleAction::RewriteUrlParam {
                    name,
                    value,
                    remove: self.sessions.action_param_remove,
                    only_if_present: self.sessions.action_only_present,
                }
            }
            DraftActionKind::ReplaceRequestText => {
                let match_pattern =
                    match editing::input_snapshot(&self.sessions.action_replace_match, window, cx) {
                        Ok(v) => v.to_string(),
                        Err(err) => {
                            self.sessions.local_error = Some(err.into());
                            self.settings_error(err, cx);
                            cx.notify();
                            return;
                        }
                    };
                let replace_with =
                    match editing::input_snapshot(&self.sessions.action_replace_with, window, cx) {
                        Ok(v) => v.to_string(),
                        Err(err) => {
                            self.sessions.local_error = Some(err.into());
                            self.settings_error(err, cx);
                            cx.notify();
                            return;
                        }
                    };
                if match_pattern.is_empty() {
                    let msg = "ReplaceRequestText 动作的匹配模式不能为空。";
                    self.sessions.local_error = Some(msg.into());
                    self.settings_error(msg, cx);
                    cx.notify();
                    return;
                }
                SessionRuleAction::ReplaceRequestText {
                    match_pattern,
                    replace_with,
                    regex: self.sessions.action_replace_regex,
                }
            }
            DraftActionKind::RunMacro => {
                let Some(mac) = self
                    .state
                    .sessions
                    .macros
                    .get(self.sessions.action_macro_idx)
                    .or_else(|| self.state.sessions.macros.first())
                else {
                    let msg = "请先在下方 Macros 区域创建至少一个 Macro。";
                    self.sessions.local_error = Some(msg.into());
                    self.settings_error(msg, cx);
                    cx.notify();
                    return;
                };
                SessionRuleAction::RunMacro {
                    macro_id: mac.id,
                    update_cookies_from_jar: true,
                    apply_extractions: true,
                }
            }
        };
        if self.sessions.draft_actions.len() >= mio_core::MAX_RULE_ACTIONS {
            let msg = "单条规则动作数量已达上限。";
            self.sessions.local_error = Some(msg.into());
            cx.notify();
            return;
        }
        self.sessions.draft_actions.push(action);
        self.sessions.local_error = None;
        cx.notify();
    }

    pub(super) fn save_session_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = match editing::input_snapshot(&self.sessions.rule_name, window, cx) {
            Ok(v) => v.trim().to_owned(),
            Err(err) => {
                self.sessions.local_error = Some(err.into());
                self.settings_error(err, cx);
                cx.notify();
                return;
            }
        };
        let prefixes_raw = match editing::input_snapshot(&self.sessions.rule_prefixes, window, cx) {
            Ok(v) => v.to_string(),
            Err(err) => {
                self.sessions.local_error = Some(err.into());
                self.settings_error(err, cx);
                cx.notify();
                return;
            }
        };
        let req_param_raw = match editing::input_snapshot(&self.sessions.rule_req_param, window, cx)
        {
            Ok(v) => v.trim().to_owned(),
            Err(err) => {
                self.sessions.local_error = Some(err.into());
                self.settings_error(err, cx);
                cx.notify();
                return;
            }
        };
        if name.is_empty() {
            let msg = "Session 规则名称不能为空。";
            self.sessions.local_error = Some(msg.into());
            self.settings_error(msg, cx);
            cx.notify();
            return;
        }
        let url_scope = match self.sessions.rule_url_mode {
            1 => SessionUrlScope::SuiteScope,
            2 => {
                let list: Vec<String> = prefixes_raw
                    .split(',')
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .collect();
                SessionUrlScope::CustomPrefixes(list)
            }
            _ => SessionUrlScope::AllUrls,
        };
        let param_condition = (!req_param_raw.is_empty()).then(|| {
            SessionParamCondition::RequirePresent(
                req_param_raw
                    .split(',')
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .collect(),
            )
        });
        let rule = SessionRule {
            id: self.sessions.editing_rule_id.unwrap_or_else(Uuid::new_v4),
            name,
            enabled: self.sessions.rule_enabled,
            tools: self.sessions.rule_tools,
            condition: SessionRuleCondition {
                url_scope,
                param_condition,
                header_contains: None,
            },
            actions: self.sessions.draft_actions.clone(),
        };
        let mut next = self.state.sessions.clone();
        if let Some(edit_id) = self.sessions.editing_rule_id
            && let Some(slot) = next.rules.iter_mut().find(|r| r.id == edit_id)
        {
            *slot = rule.clone();
        } else {
            next.rules.push(rule.clone());
        }
        self.sessions.selected_rule = Some(rule.id);
        if self.commit_session_settings(next, cx) {
            self.sessions.editing_rule_id = None;
            self.sessions.rule_editor_open = false;
            self.sessions.default_rule_selected = false;
            self.sessions
                .rule_name
                .update(cx, |s, cx| s.set_value("", window, cx));
            self.sessions
                .rule_prefixes
                .update(cx, |s, cx| s.set_value("", window, cx));
            self.sessions
                .rule_req_param
                .update(cx, |s, cx| s.set_value("", window, cx));
            self.sessions.draft_actions = vec![SessionRuleAction::UseCookieJar {
                add_new_cookies: true,
                update_existing_cookies: true,
            }];
            cx.notify();
        }
    }

    pub(super) fn open_new_session_rule_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sessions.editing_rule_id = None;
        self.sessions.rule_editor_open = true;
        self.sessions.rule_enabled = true;
        self.sessions.rule_tools = SessionToolScope::default();
        self.sessions.rule_url_mode = 0;
        self.sessions.url_scope_select.update(cx, |s, cx| {
            s.set_selected_index(Some(gpui_kit::component::IndexPath::new(0)), window, cx);
        });
        self.sessions.draft_action_kind = DraftActionKind::UseCookieJar;
        self.sessions.action_kind_select.update(cx, |s, cx| {
            s.set_selected_index(Some(gpui_kit::component::IndexPath::new(0)), window, cx);
        });
        self.sessions.draft_actions = vec![SessionRuleAction::UseCookieJar {
            add_new_cookies: true,
            update_existing_cookies: true,
        }];
        self.sessions
            .rule_name
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.sessions
            .rule_prefixes
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.sessions
            .rule_req_param
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.sessions.local_error = None;
        cx.notify();
    }

    pub(super) fn load_selected_rule_for_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.sessions.rules.is_empty() && self.sessions.default_rule_selected {
            self.sessions.editing_rule_id = None;
            self.sessions.rule_editor_open = true;
            self.sessions.rule_enabled = self.sessions.default_rule_enabled;
            self.sessions.rule_tools = SessionToolScope {
                proxy: false,
                repeater: false,
                intruder: false,
                scanner: true,
                sequencer: false,
                extensions: false,
                mio_ai: false,
            };
            self.sessions.rule_url_mode = 0;
            self.sessions.url_scope_select.update(cx, |s, cx| {
                s.set_selected_index(Some(gpui_kit::component::IndexPath::new(0)), window, cx);
            });
            self.sessions.draft_action_kind = DraftActionKind::UseCookieJar;
            self.sessions.action_kind_select.update(cx, |s, cx| {
                s.set_selected_index(Some(gpui_kit::component::IndexPath::new(0)), window, cx);
            });
            self.sessions.draft_actions = vec![SessionRuleAction::UseCookieJar {
                add_new_cookies: true,
                update_existing_cookies: true,
            }];
            self.sessions.rule_name.update(cx, |s, cx| {
                s.set_value(tables::DEFAULT_RULE_DESCRIPTION, window, cx)
            });
            self.sessions
                .rule_prefixes
                .update(cx, |s, cx| s.set_value("", window, cx));
            self.sessions
                .rule_req_param
                .update(cx, |s, cx| s.set_value("", window, cx));
            self.sessions.local_error = None;
            cx.notify();
            return;
        }
        let Some(id) = self.sessions.selected_rule else {
            return;
        };
        let Some(rule) = self
            .state
            .sessions
            .rules
            .iter()
            .find(|r| r.id == id)
            .cloned()
        else {
            return;
        };
        self.sessions.editing_rule_id = Some(rule.id);
        self.sessions.rule_editor_open = true;
        self.sessions.rule_enabled = rule.enabled;
        self.sessions.rule_tools = rule.tools;
        let (mode, pref_text) = match &rule.condition.url_scope {
            SessionUrlScope::AllUrls => (0, String::new()),
            SessionUrlScope::SuiteScope => (1, String::new()),
            SessionUrlScope::CustomPrefixes(list) => (2, list.join(", ")),
        };
        self.sessions.rule_url_mode = mode;
        self.sessions.url_scope_select.update(cx, |s, cx| {
            s.set_selected_index(
                Some(gpui_kit::component::IndexPath::new(usize::from(mode))),
                window,
                cx,
            );
        });
        let param_text = match &rule.condition.param_condition {
            Some(SessionParamCondition::RequirePresent(list))
            | Some(SessionParamCondition::RequireAbsent(list)) => list.join(", "),
            None => String::new(),
        };
        let (action_kind, action_idx) = match rule.actions.first() {
            Some(SessionRuleAction::SetHeader { .. }) => (DraftActionKind::SetHeader, 1),
            Some(SessionRuleAction::RewriteUrlParam { .. }) => (DraftActionKind::RewriteUrlParam, 2),
            Some(SessionRuleAction::RunMacro { .. }) => (DraftActionKind::RunMacro, 3),
            Some(SessionRuleAction::SetCookie { .. }) => (DraftActionKind::SetCookie, 4),
            Some(SessionRuleAction::ReplaceRequestText { .. }) => {
                (DraftActionKind::ReplaceRequestText, 5)
            }
            _ => (DraftActionKind::UseCookieJar, 0),
        };
        self.sessions.draft_action_kind = action_kind;
        self.sessions.action_kind_select.update(cx, |s, cx| {
            s.set_selected_index(Some(gpui_kit::component::IndexPath::new(action_idx)), window, cx);
        });
        self.sessions.draft_actions = rule.actions.clone();
        self.sessions
            .rule_name
            .update(cx, |s, cx| s.set_value(rule.name, window, cx));
        self.sessions
            .rule_prefixes
            .update(cx, |s, cx| s.set_value(pref_text, window, cx));
        self.sessions
            .rule_req_param
            .update(cx, |s, cx| s.set_value(param_text, window, cx));
        self.sessions.local_error = None;
        cx.notify();
    }

    pub(super) fn apply_default_cookie_jar_template(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sessions.rule_enabled = true;
        self.sessions.rule_tools = SessionToolScope {
            proxy: false,
            repeater: false,
            intruder: false,
            scanner: true,
            sequencer: false,
            extensions: false,
            mio_ai: false,
        };
        self.sessions.rule_url_mode = 0;
        self.sessions.url_scope_select.update(cx, |s, cx| {
            s.set_selected_index(Some(gpui_kit::component::IndexPath::new(0)), window, cx);
        });
        self.sessions.draft_action_kind = DraftActionKind::UseCookieJar;
        self.sessions.action_kind_select.update(cx, |s, cx| {
            s.set_selected_index(Some(gpui_kit::component::IndexPath::new(0)), window, cx);
        });
        self.sessions.draft_actions = vec![SessionRuleAction::UseCookieJar {
            add_new_cookies: true,
            update_existing_cookies: true,
        }];
        self.sessions.rule_name.update(cx, |s, cx| {
            s.set_value(tables::DEFAULT_RULE_DESCRIPTION, window, cx)
        });
        self.sessions
            .rule_prefixes
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.sessions
            .rule_req_param
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.sessions.local_error = None;
        cx.notify();
    }

    pub(super) fn toggle_default_cookie_jar_rule(
        &mut self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        let mut next = self.state.sessions.clone();
        let rule = SessionRule {
            id: Uuid::new_v4(),
            name: tables::DEFAULT_RULE_DESCRIPTION.to_owned(),
            enabled,
            tools: SessionToolScope {
                proxy: false,
                repeater: false,
                intruder: false,
                scanner: true,
                sequencer: false,
                extensions: false,
                mio_ai: false,
            },
            condition: SessionRuleCondition {
                url_scope: SessionUrlScope::AllUrls,
                param_condition: None,
                header_contains: None,
            },
            actions: vec![SessionRuleAction::UseCookieJar {
                add_new_cookies: true,
                update_existing_cookies: true,
            }],
        };
        let rule_id = rule.id;
        next.rules.push(rule);
        if self.commit_session_settings(next, cx) {
            self.sessions.default_rule_enabled = enabled;
            self.sessions.default_rule_selected = false;
            self.sessions.selected_rule = Some(rule_id);
        }
    }

    pub(super) fn set_session_rule_enabled(
        &mut self,
        id: Uuid,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        let mut next = self.state.sessions.clone();
        if let Some(rule) = next.rules.iter_mut().find(|r| r.id == id)
            && rule.enabled != enabled
        {
            rule.enabled = enabled;
            let _ = self.commit_session_settings(next, cx);
        }
    }

    pub(super) fn duplicate_selected_session_rule(&mut self, cx: &mut Context<Self>) {
        if self.state.sessions.rules.is_empty() && self.sessions.default_rule_selected {
            let mut next = self.state.sessions.clone();
            let base_tools = SessionToolScope {
                proxy: false,
                repeater: false,
                intruder: false,
                scanner: true,
                sequencer: false,
                extensions: false,
                mio_ai: false,
            };
            let make_rule = |name: String| SessionRule {
                id: Uuid::new_v4(),
                name,
                enabled: self.sessions.default_rule_enabled,
                tools: base_tools,
                condition: SessionRuleCondition {
                    url_scope: SessionUrlScope::AllUrls,
                    param_condition: None,
                    header_contains: None,
                },
                actions: vec![SessionRuleAction::UseCookieJar {
                    add_new_cookies: true,
                    update_existing_cookies: true,
                }],
            };
            let first = make_rule(tables::DEFAULT_RULE_DESCRIPTION.to_owned());
            let dup = make_rule(format!("{} (副本)", tables::DEFAULT_RULE_DESCRIPTION));
            let dup_id = dup.id;
            next.rules.push(first);
            next.rules.push(dup);
            if self.commit_session_settings(next, cx) {
                self.sessions.selected_rule = Some(dup_id);
                self.sessions.default_rule_selected = false;
            }
            return;
        }
        let Some(id) = self.sessions.selected_rule else {
            return;
        };
        let mut next = self.state.sessions.clone();
        let Some(idx) = next.rules.iter().position(|r| r.id == id) else {
            return;
        };
        let mut copy = next.rules[idx].clone();
        copy.id = Uuid::new_v4();
        copy.name = format!("{} (副本)", copy.name);
        let new_id = copy.id;
        next.rules.insert(idx + 1, copy);
        if self.commit_session_settings(next, cx) {
            self.sessions.selected_rule = Some(new_id);
        }
    }

    pub(super) fn move_selected_session_rule(&mut self, down: bool, cx: &mut Context<Self>) {
        let Some(id) = self.sessions.selected_rule else {
            return;
        };
        let mut next = self.state.sessions.clone();
        let Some(idx) = next.rules.iter().position(|r| r.id == id) else {
            return;
        };
        let target = if down {
            idx.checked_add(1).filter(|i| *i < next.rules.len())
        } else {
            idx.checked_sub(1)
        };
        if let Some(target) = target {
            next.rules.swap(idx, target);
            let _ = self.commit_session_settings(next, cx);
        }
    }

    pub(super) fn remove_selected_session_rule(&mut self, cx: &mut Context<Self>) {
        if self.state.sessions.rules.is_empty() && self.sessions.default_rule_selected {
            self.sessions.default_rule_dismissed = true;
            self.sessions.default_rule_selected = false;
            self.sessions.rules_table.update(cx, |t, cx| {
                t.clear_selection(cx);
                t.refresh(cx);
                cx.notify();
            });
            cx.notify();
            return;
        }
        let Some(id) = self.sessions.selected_rule else {
            return;
        };
        let mut next = self.state.sessions.clone();
        let before = next.rules.len();
        next.rules.retain(|r| r.id != id);
        if next.rules.len() != before && self.commit_session_settings(next, cx) {
            self.sessions.selected_rule = None;
            if self.sessions.editing_rule_id == Some(id) {
                self.sessions.editing_rule_id = None;
                self.sessions.rule_editor_open = false;
            }
        }
    }
}
