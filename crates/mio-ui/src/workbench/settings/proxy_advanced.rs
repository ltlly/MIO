use gpui_kit::accesskit::Role;
use gpui_kit::base::{Checkbox, CheckboxIndicator, CheckboxState};
use gpui_kit::component::{Icon, IconName};
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::component::table::{Column, TableDelegate};
use mio_core::{
    MatchAndReplaceRule, ProxyResponseModificationSettings, SslPassThroughRule, Uuid,
    WsMatchAndReplaceRule, WsReplaceDirection, validate_ssl_and_ws_rules,
};

use super::*;

#[path = "proxy_advanced_render.rs"]
mod proxy_advanced_render;
#[cfg(test)]
#[path = "proxy_advanced_tests.rs"]
mod proxy_advanced_tests;
#[path = "proxy_advanced_ws_mod.rs"]
mod proxy_advanced_ws_mod;

pub(super) use proxy_advanced_render::{HttpMatchReplaceTable, SslPassThroughTable};
pub(super) use proxy_advanced_ws_mod::WsMatchReplaceTable;
use gpui_kit::component::button::ButtonVariants;

pub(super) const WS_DIRECTION_LABELS: [&str; 3] = [
    "双向",
    "客户端到服务器",
    "服务器到客户端",
];

pub(super) fn ws_direction_to_index(direction: WsReplaceDirection) -> usize {
    match direction {
        WsReplaceDirection::Both => 0,
        WsReplaceDirection::ClientToServer => 1,
        WsReplaceDirection::ServerToClient => 2,
    }
}

pub(super) fn ws_direction_from_label(label: &str) -> WsReplaceDirection {
    match label {
        "客户端到服务器" | "Client to server" => WsReplaceDirection::ClientToServer,
        "服务器到客户端" | "Server to client" => WsReplaceDirection::ServerToClient,
        _ => WsReplaceDirection::Both,
    }
}

pub(super) fn ws_direction_label(direction: WsReplaceDirection) -> &'static str {
    WS_DIRECTION_LABELS[ws_direction_to_index(direction)]
}

pub(super) struct ProxyAdvancedUi {
    pub(super) http_rules: Vec<MatchAndReplaceRule>,
    pub(super) ssl_rules: Vec<SslPassThroughRule>,
    pub(super) ws_rules: Vec<WsMatchAndReplaceRule>,
    pub(super) http_table: Entity<TableState<HttpMatchReplaceTable>>,
    pub(super) ssl_table: Entity<TableState<SslPassThroughTable>>,
    pub(super) ws_table: Entity<TableState<WsMatchReplaceTable>>,
    pub(super) http_selected: Option<Uuid>,
    pub(super) ssl_selected: Option<Uuid>,
    pub(super) ws_selected: Option<Uuid>,
    pub(super) ssl_editor_open: bool,
    pub(super) ssl_editing: Option<Uuid>,
    pub(super) ssl_host: Entity<InputState>,
    pub(super) ssl_port: Entity<InputState>,
    pub(super) ssl_is_regex: bool,
    pub(super) ssl_error: Option<String>,
    pub(super) ws_editor_open: bool,
    pub(super) ws_editing: Option<Uuid>,
    pub(super) ws_direction: WsReplaceDirection,
    pub(super) ws_direction_select: Entity<SelectState<Vec<&'static str>>>,
    pub(super) ws_match: Entity<InputState>,
    pub(super) ws_replace: Entity<InputState>,
    pub(super) ws_comment: Entity<InputState>,
    pub(super) ws_is_regex: bool,
    pub(super) ws_error: Option<String>,
    pub(super) http_dialog: Option<gpui_kit::AnyWindowHandle>,
    pub(super) resp_mod_focus: [FocusHandle; 9],
}

impl ProxyAdvancedUi {
    pub(super) fn new(
        owner: &Entity<Workbench>,
        window: &mut Window,
        cx: &mut Context<Settings>,
    ) -> Self {
        let (http_rules, ssl_rules, ws_rules) = {
            let ws = &owner.read(cx).workspace;
            (
                ws.match_and_replace_rules.clone(),
                ws.ssl_pass_through_rules.clone(),
                ws.ws_match_and_replace_rules.clone(),
            )
        };
        let weak = cx.weak_entity();
        let http_table = cx.new(|tcx| {
            TableState::new(
                HttpMatchReplaceTable {
                    owner: weak.clone(),
                },
                window,
                tcx,
            )
            .sortable(false)
            .col_selectable(false)
        });
        cx.subscribe_in(&http_table, window, |this, _, event, window, cx| match event {
            TableEvent::SelectRow(row) => {
                this.proxy_advanced.http_selected = this
                    .proxy_advanced
                    .http_rules
                    .get(*row)
                    .map(|rule| rule.id);
                cx.notify();
            }
            TableEvent::DoubleClickedRow(row) => {
                if !this.state.enabled {
                    return;
                }
                if let Some(rule) = this.proxy_advanced.http_rules.get(*row).cloned() {
                    this.proxy_advanced.http_selected = Some(rule.id);
                    if let Some(project) = this.state.project {
                        if let Some(prev) = this.proxy_advanced.http_dialog.take() {
                            let _ = prev.update(cx, |_, window, _| window.remove_window());
                        }
                        this.proxy_advanced.http_dialog =
                            crate::workbench::tools_extra::open_http_match_replace_dialog(
                                this.owner.clone(),
                                this.owner_window,
                                project,
                                Some(rule),
                                "proxy-http-mr",
                                window,
                                cx,
                            )
                            .ok();
                    }
                }
            }
            _ => {}
        })
        .detach();

        let ssl_table = cx.new(|tcx| {
            TableState::new(
                SslPassThroughTable {
                    owner: weak.clone(),
                },
                window,
                tcx,
            )
            .sortable(false)
            .col_selectable(false)
        });
        cx.subscribe_in(&ssl_table, window, |this, _, event, window, cx| match event {
            TableEvent::SelectRow(row) => {
                this.proxy_advanced.ssl_selected = this
                    .proxy_advanced
                    .ssl_rules
                    .get(*row)
                    .map(|rule| rule.id);
                cx.notify();
            }
            TableEvent::DoubleClickedRow(row) => {
                if !this.state.enabled {
                    return;
                }
                if let Some(rule) = this.proxy_advanced.ssl_rules.get(*row).cloned() {
                    this.proxy_advanced.ssl_selected = Some(rule.id);
                    this.load_ssl_rule_into_editor(&rule, window, cx);
                }
            }
            _ => {}
        })
        .detach();

        let ws_table = cx.new(|tcx| {
            TableState::new(
                WsMatchReplaceTable {
                    owner: weak.clone(),
                },
                window,
                tcx,
            )
            .sortable(false)
            .col_selectable(false)
        });
        cx.subscribe_in(&ws_table, window, |this, _, event, window, cx| match event {
            TableEvent::SelectRow(row) => {
                this.proxy_advanced.ws_selected = this
                    .proxy_advanced
                    .ws_rules
                    .get(*row)
                    .map(|rule| rule.id);
                cx.notify();
            }
            TableEvent::DoubleClickedRow(row) => {
                if !this.state.enabled {
                    return;
                }
                if let Some(rule) = this.proxy_advanced.ws_rules.get(*row).cloned() {
                    this.proxy_advanced.ws_selected = Some(rule.id);
                    this.load_ws_rule_into_editor(&rule, window, cx);
                }
            }
            _ => {}
        })
        .detach();

        let ws_direction_select = cx.new(|cx| {
            SelectState::new(
                WS_DIRECTION_LABELS.to_vec(),
                Some(gpui_kit::component::IndexPath::new(0)),
                window,
                cx,
            )
        });
        cx.subscribe_in(&ws_direction_select, window, |this, _, event, _, cx| {
            let SelectEvent::Confirm(value) = event;
            if let Some(value) = value {
                this.proxy_advanced.ws_direction = ws_direction_from_label(value);
            }
            cx.notify();
        })
        .detach();

        cx.observe_in(owner, window, move |this, owner, _, cx| {
            let (http_changed, ssl_changed, ws_changed) = {
                let ws = &owner.read(cx).workspace;
                (
                    this.proxy_advanced.http_rules != ws.match_and_replace_rules,
                    this.proxy_advanced.ssl_rules != ws.ssl_pass_through_rules,
                    this.proxy_advanced.ws_rules != ws.ws_match_and_replace_rules,
                )
            };
            if !http_changed && !ssl_changed && !ws_changed {
                return;
            }
            if http_changed {
                this.proxy_advanced.http_rules = owner.read(cx).workspace.match_and_replace_rules.clone();
                let http_sel = this.proxy_advanced.http_selected.and_then(|id| {
                    this.proxy_advanced
                        .http_rules
                        .iter()
                        .position(|rule| rule.id == id)
                });
                if http_sel.is_none() {
                    this.proxy_advanced.http_selected = None;
                }
                this.proxy_advanced.http_table.update(cx, |table, cx| {
                    if let Some(row) = http_sel {
                        table.set_selected_row(row, cx);
                    } else {
                        table.clear_selection(cx);
                    }
                    table.refresh(cx);
                    cx.notify();
                });
            }
            if ssl_changed {
                this.proxy_advanced.ssl_rules = owner.read(cx).workspace.ssl_pass_through_rules.clone();
                let ssl_sel = this.proxy_advanced.ssl_selected.and_then(|id| {
                    this.proxy_advanced
                        .ssl_rules
                        .iter()
                        .position(|rule| rule.id == id)
                });
                if ssl_sel.is_none() {
                    this.proxy_advanced.ssl_selected = None;
                }
                this.proxy_advanced.ssl_table.update(cx, |table, cx| {
                    if let Some(row) = ssl_sel {
                        table.set_selected_row(row, cx);
                    } else {
                        table.clear_selection(cx);
                    }
                    table.refresh(cx);
                    cx.notify();
                });
            }
            if ws_changed {
                this.proxy_advanced.ws_rules = owner.read(cx).workspace.ws_match_and_replace_rules.clone();
                let ws_sel = this.proxy_advanced.ws_selected.and_then(|id| {
                    this.proxy_advanced
                        .ws_rules
                        .iter()
                        .position(|rule| rule.id == id)
                });
                if ws_sel.is_none() {
                    this.proxy_advanced.ws_selected = None;
                }
                this.proxy_advanced.ws_table.update(cx, |table, cx| {
                    if let Some(row) = ws_sel {
                        table.set_selected_row(row, cx);
                    } else {
                        table.clear_selection(cx);
                    }
                    table.refresh(cx);
                    cx.notify();
                });
            }

            let _ = &weak;
            cx.notify();
        })
        .detach();
        Self {
            http_rules,
            ssl_rules,
            ws_rules,
            http_table,
            ssl_table,
            ws_table,
            http_selected: None,
            ssl_selected: None,
            ws_selected: None,
            ssl_editor_open: false,
            ssl_editing: None,
            ssl_host: cx
                .new(|cx| InputState::new(window, cx).placeholder("主机或正则，如 *.example.com")),
            ssl_port: cx.new(|cx| InputState::new(window, cx).placeholder("端口（留空为任意）")),
            ssl_is_regex: false,
            ssl_error: None,
            ws_editor_open: false,
            ws_editing: None,
            ws_direction: WsReplaceDirection::Both,
            ws_direction_select,
            ws_match: cx.new(|cx| InputState::new(window, cx).placeholder("匹配文本或正则表达式")),
            ws_replace: cx.new(|cx| InputState::new(window, cx).placeholder("替换为（可留空）")),
            ws_comment: cx.new(|cx| InputState::new(window, cx).placeholder("备注（可选）")),
            ws_is_regex: false,
            ws_error: None,
            http_dialog: None,
            resp_mod_focus: std::array::from_fn(|_| cx.focus_handle()),
        }
    }
}

pub(super) fn adv_checkbox(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    checked: bool,
    enabled: bool,
) -> Checkbox {
    let label = label.into();
    Checkbox::new(id)
        .accessibility_label(label.clone())
        .checked(checked)
        .disabled(!enabled)
        .tab_stop(false)
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(2.))
        .py(px(3.))
        .rounded(px(2.))
        .focus_visible(|style| style.bg(theme::selected()))
        .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
        .child(
            CheckboxIndicator::new()
                .checked(checked)
                .size(px(14.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(settings_checkbox_border(checked))
                .bg(if checked {
                    settings_blue()
                } else {
                    theme::surface()
                })
                .rounded(px(3.))
                .when(checked, |this| {
                    this.child(
                        Icon::new(IconName::Check)
                            .size(px(11.5))
                            .text_color(rgb(0xffffff)),
                    )
                }),
        )
        .child(label)
}

impl Workbench {
    pub(in crate::workbench) fn set_proxy_message_flag(
        &mut self,
        project: Option<ProjectKey>,
        mutate: impl FnOnce(&mut mio_core::ProxyMessageSettings),
        cx: &mut Context<Self>,
    ) {
        if project.is_none() || self.project != project || !self.backend_available() || self.transition.is_some() {
            return;
        }
        let mut next = self.workspace.proxy_messages;
        mutate(&mut next);
        if next == self.workspace.proxy_messages {
            return;
        }
        self.workspace.proxy_messages = next;
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn update_proxy_advanced(
        &mut self,
        project: Option<ProjectKey>,
        mutate: impl FnOnce(
            &mut Vec<SslPassThroughRule>,
            &mut Vec<WsMatchAndReplaceRule>,
            &mut ProxyResponseModificationSettings,
        ),
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return Err("当前项目尚未就绪或后台不可用；设置未修改。".into());
        }
        let mut ssl = self.workspace.ssl_pass_through_rules.clone();
        let mut ws = self.workspace.ws_match_and_replace_rules.clone();
        let mut resp_mod = self.workspace.proxy_messages.response_modification;
        mutate(&mut ssl, &mut ws, &mut resp_mod);
        validate_ssl_and_ws_rules(&ssl, &ws).map_err(|error| error.to_string())?;
        let mut candidate = self.workspace.clone();
        candidate.ssl_pass_through_rules = ssl;
        candidate.ws_match_and_replace_rules = ws;
        candidate.proxy_messages.response_modification = resp_mod;
        candidate.validate().map_err(|error| error.to_string())?;
        if candidate.ssl_pass_through_rules == self.workspace.ssl_pass_through_rules
            && candidate.ws_match_and_replace_rules == self.workspace.ws_match_and_replace_rules
            && candidate.proxy_messages == self.workspace.proxy_messages
        {
            return Ok(());
        }
        self.workspace.ssl_pass_through_rules = candidate.ssl_pass_through_rules;
        self.workspace.ws_match_and_replace_rules = candidate.ws_match_and_replace_rules;
        self.workspace.proxy_messages = candidate.proxy_messages;
        self.save_workspace(cx);
        cx.notify();
        Ok(())
    }
}

impl Settings {
    pub(super) fn submit_ssl_pass_through(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let snapshots = [&self.proxy_advanced.ssl_host, &self.proxy_advanced.ssl_port]
            .map(|input| editing::input_snapshot(input, window, cx));
        let [Ok(host_raw), Ok(port_raw)] = snapshots else {
            self.proxy_advanced.ssl_error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        };
        let host_pattern = host_raw.trim().to_owned();
        if host_pattern.is_empty() {
            self.proxy_advanced.ssl_error = Some("请填写目标主机或正则表达式。".into());
            cx.notify();
            return;
        }
        let port = if port_raw.trim().is_empty() {
            None
        } else {
            match port_raw.trim().parse::<u16>() {
                Ok(p) if p > 0 => Some(p),
                _ => {
                    self.proxy_advanced.ssl_error =
                        Some("端口须为 1–65535 的有效整数，或留空表示任意端口。".into());
                    cx.notify();
                    return;
                }
            }
        };
        let is_regex = self.proxy_advanced.ssl_is_regex;
        let editing_id = self.proxy_advanced.ssl_editing;
        let project = self.state.project;
        let outcome = self.owner.update(cx, |view, cx| {
            view.update_proxy_advanced(
                project,
                |ssl, _, _| {
                    if let Some(id) = editing_id
                        && let Some(existing) = ssl.iter_mut().find(|r| r.id == id)
                    {
                        existing.host_pattern = host_pattern;
                        existing.port = port;
                        existing.is_regex = is_regex;
                    } else {
                        ssl.push(SslPassThroughRule {
                            id: Uuid::new_v4(),
                            enabled: true,
                            host_pattern,
                            port,
                            is_regex,
                        });
                    }
                },
                cx,
            )
        });
        match outcome {
            Ok(Ok(())) => {
                self.proxy_advanced.ssl_editing = None;
                self.proxy_advanced.ssl_editor_open = false;
                self.proxy_advanced.ssl_error = None;
                self.proxy_advanced.ssl_is_regex = false;
                self.proxy_advanced
                    .ssl_host
                    .update(cx, |s, cx| s.set_value("", window, cx));
                self.proxy_advanced
                    .ssl_port
                    .update(cx, |s, cx| s.set_value("", window, cx));
            }
            Ok(Err(err)) => self.proxy_advanced.ssl_error = Some(err),
            Err(_) => {
                self.proxy_advanced.ssl_error =
                    Some("主工作台已关闭；TLS 透传规则未保存。".into())
            }
        }
        cx.notify();
    }

    pub(super) fn load_ssl_rule_into_editor(
        &mut self,
        rule: &SslPassThroughRule,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.proxy_advanced.ssl_editing = Some(rule.id);
        self.proxy_advanced.ssl_editor_open = true;
        self.proxy_advanced.ssl_is_regex = rule.is_regex;
        self.proxy_advanced.ssl_error = None;
        let host = rule.host_pattern.clone();
        let port = rule.port.map(|p| p.to_string()).unwrap_or_default();
        self.proxy_advanced
            .ssl_host
            .update(cx, |s, cx| s.set_value(host, window, cx));
        self.proxy_advanced
            .ssl_port
            .update(cx, |s, cx| s.set_value(port, window, cx));
        cx.notify();
    }

    pub(super) fn paste_ssl_pass_through_url(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(raw) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.proxy_advanced.ssl_error = Some("剪贴板中没有可用的 URL。".into());
            cx.notify();
            return;
        };
        let trimmed = raw.trim();
        let Ok(parsed) = url::Url::parse(trimmed).or_else(|_| url::Url::parse(&format!("https://{trimmed}"))) else {
            self.proxy_advanced.ssl_error = Some("剪贴板文本不是有效的 URL。".into());
            cx.notify();
            return;
        };
        let Some(host) = parsed.host_str().map(str::to_owned) else {
            self.proxy_advanced.ssl_error = Some("URL 中未包含有效主机名。".into());
            cx.notify();
            return;
        };
        let port = parsed.port_or_known_default();
        self.proxy_advanced.ssl_host.update(cx, |s, cx| s.set_value(host.clone(), window, cx));
        self.proxy_advanced.ssl_port.update(cx, |s, cx| s.set_value(port.map(|p| p.to_string()).unwrap_or_default(), window, cx));
        let project = self.state.project;
        let outcome = self.owner.update(cx, |view, cx| {
            view.update_proxy_advanced(
                project,
                |ssl, _, _| {
                    ssl.push(SslPassThroughRule {
                        id: Uuid::new_v4(),
                        enabled: true,
                        host_pattern: host,
                        port,
                        is_regex: false,
                    });
                },
                cx,
            )
        });
        match outcome {
            Ok(Ok(())) => {
                self.proxy_advanced.ssl_error = None;
            }
            Ok(Err(err)) => {
                self.proxy_advanced.ssl_error = Some(err);
            }
            Err(_) => {
                self.proxy_advanced.ssl_error =
                    Some("主工作台已关闭；TLS 透传规则未保存。".into());
            }
        }
        cx.notify();
    }

    pub(super) fn submit_ws_match_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let snapshots = [
            &self.proxy_advanced.ws_match,
            &self.proxy_advanced.ws_replace,
            &self.proxy_advanced.ws_comment,
        ]
        .map(|input| editing::input_snapshot(input, window, cx));
        let [Ok(match_raw), Ok(replace_raw), Ok(comment_raw)] = snapshots else {
            self.proxy_advanced.ws_error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        };
        if match_raw.is_empty() {
            self.proxy_advanced.ws_error = Some("请填写 WebSocket 匹配模式（不能为空）。".into());
            cx.notify();
            return;
        }
        let match_pattern = match_raw.to_string();
        let replace_with = replace_raw.to_string();
        let comment = comment_raw.trim().to_owned();
        let direction = self.proxy_advanced.ws_direction;
        let is_regex = self.proxy_advanced.ws_is_regex;
        let editing_id = self.proxy_advanced.ws_editing;
        let project = self.state.project;
        let outcome = self.owner.update(cx, |view, cx| {
            view.update_proxy_advanced(
                project,
                |_, ws, _| {
                    if let Some(id) = editing_id
                        && let Some(existing) = ws.iter_mut().find(|r| r.id == id)
                    {
                        existing.direction = direction;
                        existing.match_pattern = match_pattern;
                        existing.replace_with = replace_with;
                        existing.is_regex = is_regex;
                        existing.comment = comment;
                    } else {
                        ws.push(WsMatchAndReplaceRule {
                            id: Uuid::new_v4(),
                            enabled: true,
                            direction,
                            match_pattern,
                            replace_with,
                            is_regex,
                            comment,
                        });
                    }
                },
                cx,
            )
        });
        match outcome {
            Ok(Ok(())) => {
                self.proxy_advanced.ws_editing = None;
                self.proxy_advanced.ws_editor_open = false;
                self.proxy_advanced.ws_error = None;
                self.proxy_advanced.ws_is_regex = false;
                self.proxy_advanced.ws_direction = WsReplaceDirection::Both;
                self.proxy_advanced
                    .ws_direction_select
                    .update(cx, |state, cx| {
                        state.set_selected_index(
                            Some(gpui_kit::component::IndexPath::new(0)),
                            window,
                            cx,
                        );
                    });
                for input in [
                    &self.proxy_advanced.ws_match,
                    &self.proxy_advanced.ws_replace,
                    &self.proxy_advanced.ws_comment,
                ] {
                    input.update(cx, |s, cx| s.set_value("", window, cx));
                }
            }
            Ok(Err(err)) => self.proxy_advanced.ws_error = Some(err),
            Err(_) => {
                self.proxy_advanced.ws_error =
                    Some("主工作台已关闭；WebSocket 替换规则未保存。".into())
            }
        }
        cx.notify();
    }

    pub(super) fn load_ws_rule_into_editor(
        &mut self,
        rule: &WsMatchAndReplaceRule,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.proxy_advanced.ws_editing = Some(rule.id);
        self.proxy_advanced.ws_editor_open = true;
        self.proxy_advanced.ws_direction = rule.direction;
        self.proxy_advanced
            .ws_direction_select
            .update(cx, |state, cx| {
                state.set_selected_index(
                    Some(gpui_kit::component::IndexPath::new(ws_direction_to_index(
                        rule.direction,
                    ))),
                    window,
                    cx,
                );
            });
        self.proxy_advanced.ws_is_regex = rule.is_regex;
        self.proxy_advanced.ws_error = None;
        let m = rule.match_pattern.clone();
        let r = rule.replace_with.clone();
        let c = rule.comment.clone();
        self.proxy_advanced
            .ws_match
            .update(cx, |s, cx| s.set_value(m, window, cx));
        self.proxy_advanced
            .ws_replace
            .update(cx, |s, cx| s.set_value(r, window, cx));
        self.proxy_advanced
            .ws_comment
            .update(cx, |s, cx| s.set_value(c, window, cx));
        cx.notify();
    }
}
