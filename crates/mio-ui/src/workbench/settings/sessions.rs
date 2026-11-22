use gpui_kit::accesskit::Role;
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::base::{Checkbox, CheckboxIndicator};
use gpui_kit::component::{
    Icon, IconName, IndexPath,
    select::{SelectEvent, SelectState},
};
use mio_core::{MacroFailurePolicy, SessionRuleAction, SessionSettings, SessionToolScope, Uuid};

use super::*;

mod jar_macro_actions;
mod macro_recorder;
mod render_jar;
mod render_macros;
mod render_rule_editor;
mod render_rules;
mod rule_actions;
mod tables;
#[cfg(test)]
mod tests;

use tables::{CookieJarTable, SessionMacrosTable, SessionRulesTable};

const TABLE_CARET_RIGHT_SVG: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><path fill="currentColor" d="M2 1.5 L8.5 5 L2 8.5 Z"/></svg>"#;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DraftActionKind {
    UseCookieJar,
    SetHeader,
    SetCookie,
    RewriteUrlParam,
    ReplaceRequestText,
    RunMacro,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DraftExtractKind {
    Delimiter,
    Regex,
    ResponseHeader,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DraftTargetKind {
    UrlOrBodyParam,
    Header,
    Cookie,
}

pub(super) struct SessionsUi {
    pub(super) scroll: ScrollHandle,
    pub(super) rules_table: Entity<TableState<SessionRulesTable>>,
    pub(super) jar_table: Entity<TableState<CookieJarTable>>,
    pub(super) macros_table: Entity<TableState<SessionMacrosTable>>,
    pub(super) selected_rule: Option<Uuid>,
    pub(super) selected_cookie: Option<Uuid>,
    pub(super) selected_macro: Option<Uuid>,
    pub(super) local_error: Option<String>,
    pub(super) rule_editor_open: bool,
    pub(super) default_rule_enabled: bool,
    pub(super) default_rule_dismissed: bool,
    pub(super) default_rule_selected: bool,
    pub(super) tracer_open: bool,
    pub(super) cookie_jar_open: bool,
    pub(super) macro_editor_open: bool,

    pub(super) editing_rule_id: Option<Uuid>,
    pub(super) rule_name: Entity<InputState>,
    pub(super) rule_enabled: bool,
    pub(super) rule_tools: SessionToolScope,
    pub(super) rule_url_mode: u8,
    pub(super) url_scope_select: Entity<SelectState<Vec<&'static str>>>,
    pub(super) rule_prefixes: Entity<InputState>,
    pub(super) rule_req_param: Entity<InputState>,
    pub(super) draft_action_kind: DraftActionKind,
    pub(super) action_kind_select: Entity<SelectState<Vec<&'static str>>>,
    pub(super) action_header_name: Entity<InputState>,
    pub(super) action_header_val: Entity<InputState>,
    pub(super) action_cookie_name: Entity<InputState>,
    pub(super) action_cookie_val: Entity<InputState>,
    pub(super) action_only_present: bool,
    pub(super) action_param_name: Entity<InputState>,
    pub(super) action_param_val: Entity<InputState>,
    pub(super) action_param_remove: bool,
    pub(super) action_replace_match: Entity<InputState>,
    pub(super) action_replace_with: Entity<InputState>,
    pub(super) action_replace_regex: bool,
    pub(super) action_macro_idx: usize,
    pub(super) action_macro_select: Entity<SelectState<Vec<String>>>,
    pub(super) draft_actions: Vec<SessionRuleAction>,

    pub(super) editing_cookie_id: Option<Uuid>,
    pub(super) cookie_domain: Entity<InputState>,
    pub(super) cookie_subdomains: bool,
    pub(super) cookie_path: Entity<InputState>,
    pub(super) cookie_name: Entity<InputState>,
    pub(super) cookie_value: Entity<InputState>,
    pub(super) cookie_secure: bool,
    pub(super) cookie_http_only: bool,
    pub(super) cookie_expires: Entity<InputState>,

    pub(super) editing_macro_id: Option<Uuid>,
    pub(super) macro_name: Entity<InputState>,
    pub(super) macro_failure_policy: MacroFailurePolicy,
    pub(super) macro_retries: Entity<InputState>,
    pub(super) macro_timeout_ms: Entity<InputState>,
    pub(super) step_method: Entity<InputState>,
    pub(super) step_url: Entity<InputState>,
    pub(super) step_headers: Entity<InputState>,
    pub(super) step_body: Entity<InputState>,
    pub(super) step_status: Entity<InputState>,
    pub(super) step_use_jar: bool,
    pub(super) ext_name: Entity<InputState>,
    pub(super) ext_kind: DraftExtractKind,
    pub(super) ext_arg1: Entity<InputState>,
    pub(super) ext_arg2: Entity<InputState>,
    pub(super) ext_target_kind: DraftTargetKind,
    pub(super) ext_target_name: Entity<InputState>,
    pub(super) draft_steps: Vec<mio_core::MacroStep>,
}

impl SessionsUi {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Settings>) -> Self {
        let weak = cx.weak_entity();
        let rules_table = cx.new(|tcx| {
            TableState::new(
                SessionRulesTable {
                    owner: weak.clone(),
                },
                window,
                tcx,
            )
            .sortable(false)
            .col_selectable(false)
        });
        cx.subscribe_in(&rules_table, window, |this, _, event, _, cx| {
            if let TableEvent::SelectRow(row) = event {
                if this.state.sessions.rules.is_empty() && !this.sessions.default_rule_dismissed {
                    this.sessions.default_rule_selected = *row == 0;
                    this.sessions.selected_rule = None;
                } else {
                    this.sessions.default_rule_selected = false;
                    this.sessions.selected_rule =
                        this.state.sessions.rules.get(*row).map(|r| r.id);
                }
                cx.notify();
            }
        })
        .detach();

        let jar_table = cx.new(|tcx| {
            TableState::new(
                CookieJarTable {
                    owner: weak.clone(),
                },
                window,
                tcx,
            )
            .sortable(false)
            .col_selectable(false)
        });
        cx.subscribe_in(&jar_table, window, |this, _, event, _, cx| {
            if let TableEvent::SelectRow(row) = event {
                this.sessions.selected_cookie = this
                    .state
                    .sessions
                    .cookie_jar
                    .entries
                    .get(*row)
                    .map(|e| e.id);
                cx.notify();
            }
        })
        .detach();

        let macros_table = cx.new(|tcx| {
            TableState::new(SessionMacrosTable { owner: weak }, window, tcx)
                .sortable(false)
                .col_selectable(false)
        });
        cx.subscribe_in(&macros_table, window, |this, _, event, _, cx| {
            if let TableEvent::SelectRow(row) = event {
                this.sessions.selected_macro = this.state.sessions.macros.get(*row).map(|m| m.id);
                cx.notify();
            }
        })
        .detach();
        let url_scope_select = cx.new(|scx| {
            SelectState::new(
                vec!["All URLs", "Use suite scope", "Include custom URL prefixes"],
                Some(IndexPath::new(0)),
                window,
                scx,
            )
        });
        cx.subscribe_in(&url_scope_select, window, |this, _, event, _, cx| {
            let SelectEvent::Confirm(Some(value)) = event else {
                return;
            };
            this.sessions.rule_url_mode = match *value {
                "Use suite scope" => 1,
                "Include custom URL prefixes" => 2,
                _ => 0,
            };
            cx.notify();
        })
        .detach();
        let action_kind_select = cx.new(|scx| {
            SelectState::new(
                vec![
                    "Use cookies from session cookie jar",
                    "Set a specific header",
                    "Set a specific URL parameter",
                    "Run a macro",
                    "Set a specific cookie",
                    "Replace matching part of the request",
                ],
                Some(IndexPath::new(0)),
                window,
                scx,
            )
        });
        cx.subscribe_in(&action_kind_select, window, |this, _, event, _, cx| {
            let SelectEvent::Confirm(Some(value)) = event else {
                return;
            };
            this.sessions.draft_action_kind = match *value {
                "Set a specific header" => DraftActionKind::SetHeader,
                "Set a specific cookie" => DraftActionKind::SetCookie,
                "Set a specific URL parameter" => DraftActionKind::RewriteUrlParam,
                "Replace matching part of the request" => DraftActionKind::ReplaceRequestText,
                "Run a macro" => DraftActionKind::RunMacro,
                _ => DraftActionKind::UseCookieJar,
            };
            cx.notify();
        })
        .detach();
        let action_macro_select = cx.new(|scx| {
            SelectState::new(
                vec!["（尚无已保存 Macro）".to_owned()],
                Some(IndexPath::new(0)),
                window,
                scx,
            )
        });
        cx.subscribe_in(&action_macro_select, window, |this, _, event, _, cx| {
            let SelectEvent::Confirm(Some(name)) = event else {
                return;
            };
            if let Some(pos) = this
                .state
                .sessions
                .macros
                .iter()
                .position(|m| &m.name == name)
            {
                this.sessions.action_macro_idx = pos;
            }
            cx.notify();
        })
        .detach();

        Self {
            scroll: ScrollHandle::new(),
            rules_table,
            jar_table,
            macros_table,
            selected_rule: None,
            selected_cookie: None,
            selected_macro: None,
            local_error: None,
            rule_editor_open: false,
            default_rule_enabled: true,
            default_rule_dismissed: false,
            default_rule_selected: false,
            tracer_open: false,
            cookie_jar_open: false,
            macro_editor_open: false,
            editing_rule_id: None,
            rule_name: cx
                .new(|cx| InputState::new(window, cx).placeholder("规则名称，例如 API 会话更新")),
            rule_enabled: true,
            rule_tools: SessionToolScope::default(),
            rule_url_mode: 0,
            url_scope_select,
            rule_prefixes: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("逗号分隔 URL 前缀，如 https://api.example.com/")
            }),
            rule_req_param: cx
                .new(|cx| InputState::new(window, cx).placeholder("必需参数名（可选）")),
            draft_action_kind: DraftActionKind::UseCookieJar,
            action_kind_select,
            action_header_name: cx.new(|cx| InputState::new(window, cx).placeholder("Header 名称")),
            action_header_val: cx.new(|cx| InputState::new(window, cx).placeholder("Header 值")),
            action_cookie_name: cx.new(|cx| InputState::new(window, cx).placeholder("Cookie 名称")),
            action_cookie_val: cx.new(|cx| InputState::new(window, cx).placeholder("Cookie 值")),
            action_only_present: false,
            action_param_name: cx.new(|cx| InputState::new(window, cx).placeholder("URL 参数名")),
            action_param_val: cx.new(|cx| InputState::new(window, cx).placeholder("URL 参数值")),
            action_param_remove: false,
            action_replace_match: cx
                .new(|cx| InputState::new(window, cx).placeholder("匹配文本或正则表达式")),
            action_replace_with: cx
                .new(|cx| InputState::new(window, cx).placeholder("替换为文本")),
            action_replace_regex: false,
            action_macro_idx: 0,
            action_macro_select,
            draft_actions: vec![SessionRuleAction::UseCookieJar {
                add_new_cookies: true,
                update_existing_cookies: true,
            }],
            editing_cookie_id: None,
            cookie_domain: cx.new(|cx| InputState::new(window, cx).placeholder("example.com")),
            cookie_subdomains: false,
            cookie_path: cx.new(|cx| InputState::new(window, cx).default_value("/")),
            cookie_name: cx.new(|cx| InputState::new(window, cx).placeholder("session_id")),
            cookie_value: cx.new(|cx| InputState::new(window, cx).placeholder("cookie_value")),
            cookie_secure: false,
            cookie_http_only: true,
            cookie_expires: cx
                .new(|cx| InputState::new(window, cx).placeholder("过期 Unix ms（留空为会话级）")),
            editing_macro_id: None,
            macro_name: cx
                .new(|cx| InputState::new(window, cx).placeholder("Macro 名称，例如 自动刷新令牌")),
            macro_failure_policy: MacroFailurePolicy::Abort,
            macro_retries: cx.new(|cx| InputState::new(window, cx).default_value("1")),
            macro_timeout_ms: cx.new(|cx| InputState::new(window, cx).default_value("10000")),
            step_method: cx.new(|cx| InputState::new(window, cx).default_value("GET")),
            step_url: cx
                .new(|cx| InputState::new(window, cx).placeholder("https://example.com/login")),
            step_headers: cx.new(|cx| {
                InputState::new(window, cx).placeholder("附加头 Name: Value（分号分隔，可选）")
            }),
            step_body: cx.new(|cx| InputState::new(window, cx).placeholder("请求正文（可选）")),
            step_status: cx
                .new(|cx| InputState::new(window, cx).placeholder("预期状态码（如 200，可选）")),
            step_use_jar: true,
            ext_name: cx
                .new(|cx| InputState::new(window, cx).placeholder("提取参数名（可选，如 csrf）")),
            ext_kind: DraftExtractKind::Delimiter,
            ext_arg1: cx
                .new(|cx| InputState::new(window, cx).placeholder("起始分隔符 / 正则 / 响应头名")),
            ext_arg2: cx
                .new(|cx| InputState::new(window, cx).placeholder("结束分隔符（仅分隔符模式）")),
            ext_target_kind: DraftTargetKind::UrlOrBodyParam,
            ext_target_name: cx.new(|cx| {
                InputState::new(window, cx).placeholder("目标头/Cookie 名（留空同参数名）")
            }),
            draft_steps: Vec::new(),
        }
    }
}

impl Settings {
    pub(super) fn sync_sessions_tables(
        &mut self,
        project_changed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if project_changed {
            self.sessions.selected_rule = None;
            self.sessions.selected_cookie = None;
            self.sessions.selected_macro = None;
            self.sessions.editing_rule_id = None;
            self.sessions.editing_cookie_id = None;
            self.sessions.editing_macro_id = None;
            self.sessions.rule_editor_open = false;
            self.sessions.default_rule_enabled = true;
            self.sessions.default_rule_dismissed = false;
            self.sessions.default_rule_selected = false;
            self.sessions.tracer_open = false;
            self.sessions.cookie_jar_open = false;
            self.sessions.macro_editor_open = false;
            self.sessions.local_error = None;
        }
        if !self.state.sessions.rules.is_empty() {
            self.sessions.default_rule_selected = false;
        }
        let r_sel = self
            .sessions
            .selected_rule
            .and_then(|id| self.state.sessions.rules.iter().position(|r| r.id == id));
        if r_sel.is_none() {
            self.sessions.selected_rule = None;
        }
        self.sessions.rules_table.update(cx, |table, cx| {
            if let Some(row) = r_sel {
                table.set_selected_row(row, cx);
            } else {
                table.clear_selection(cx);
            }
            table.refresh(cx);
            cx.notify();
        });

        let c_sel = self.sessions.selected_cookie.and_then(|id| {
            self.state
                .sessions
                .cookie_jar
                .entries
                .iter()
                .position(|e| e.id == id)
        });
        if c_sel.is_none() {
            self.sessions.selected_cookie = None;
        }
        self.sessions.jar_table.update(cx, |table, cx| {
            if let Some(row) = c_sel {
                table.set_selected_row(row, cx);
            } else {
                table.clear_selection(cx);
            }
            table.refresh(cx);
            cx.notify();
        });

        let m_sel = self
            .sessions
            .selected_macro
            .and_then(|id| self.state.sessions.macros.iter().position(|m| m.id == id));
        if m_sel.is_none() {
            self.sessions.selected_macro = None;
        }
        self.sessions.macros_table.update(cx, |table, cx| {
            if let Some(row) = m_sel {
                table.set_selected_row(row, cx);
            } else {
                table.clear_selection(cx);
            }
            table.refresh(cx);
            cx.notify();
        });
        let macro_names: Vec<String> = if self.state.sessions.macros.is_empty() {
            vec!["（尚无已保存 Macro）".into()]
        } else {
            self.state
                .sessions
                .macros
                .iter()
                .map(|m| m.name.clone())
                .collect()
        };
        let selected_idx = self
            .sessions
            .action_macro_idx
            .min(macro_names.len().saturating_sub(1));
        self.sessions.action_macro_select.update(cx, |state, cx| {
            state.set_items(macro_names, window, cx);
            state.set_selected_index(Some(IndexPath::new(selected_idx)), window, cx);
        });
    }
}

impl Workbench {
    pub(in crate::workbench) fn apply_session_settings(
        &mut self,
        project: Option<ProjectKey>,
        next: SessionSettings,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return Err("当前项目尚未就绪或后台不可用；会话设置未修改。".into());
        }
        if let Err(err) = next.validate() {
            let msg = err.to_string();
            self.fail(msg.clone(), cx);
            return Err(msg);
        }
        if self.workspace.sessions == next {
            self.error = None;
            return Ok(());
        }
        let previous = std::mem::replace(&mut self.workspace.sessions, next);
        if !self.save_workspace(cx) {
            self.workspace.sessions = previous;
            let msg = self
                .error
                .clone()
                .unwrap_or_else(|| "保存会话设置失败，已恢复原配置。".into());
            cx.notify();
            return Err(msg);
        }
        self.error = None;
        cx.notify();
        Ok(())
    }
}

pub(super) fn sessions_project_badge() -> gpui_kit::Div {
    let dark = theme::is_dark();
    div()
        .flex_shrink_0()
        .px(px(10.))
        .py(px(2.))
        .rounded_full()
        .bg(rgb(if dark { 0x273b54 } else { 0xc8d7ec }))
        .text_size(theme::scaled_px(12.))
        .text_color(rgb(if dark { 0xd4e6fa } else { 0x1f2d3d }))
        .whitespace_nowrap()
        .child("项目设置")
}

pub(super) fn sessions_section_heading(
    id: &'static str,
    title: &'static str,
) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(9.))
                .min_w_0()
                .child(
                    div()
                        .w(px(20.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(AssetIconName::CircleQuestionMark)
                                .size(px(19.))
                                .text_color(theme::foreground()),
                        ),
                )
                .child(
                    div()
                        .id(id)
                        .test_support()
                        .role(Role::Heading)
                        .aria_label(title)
                        .text_size(theme::scaled_px(15.))
                        .font_weight(gpui_kit::FontWeight::BOLD)
                        .text_color(theme::foreground())
                        .child(title),
                ),
        )
        .child(sessions_project_badge())
}

pub(super) fn sessions_desc_row(desc: &'static str) -> gpui_kit::Div {
    div()
        .flex()
        .items_start()
        .gap(px(9.))
        .child(
            div()
                .w(px(20.))
                .pt(px(1.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(AssetIconName::Settings)
                        .size(px(19.))
                        .text_color(theme::foreground()),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(theme::foreground())
                .child(desc),
        )
}

pub(super) fn sessions_mio_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    disabled: bool,
) -> Button {
    let dark = theme::is_dark();
    let (bg, border, fg, hover_bg) = if disabled {
        (
            rgb(if dark { 0x32353a } else { 0xebebeb }),
            rgb(if dark { 0x41454d } else { 0xdfdfdf }),
            rgb(if dark { 0x6f737a } else { 0x7c7c7c }),
            rgb(if dark { 0x32353a } else { 0xebebeb }),
        )
    } else {
        (
            rgb(if dark { 0x3c3f44 } else { 0xebebeb }),
            rgb(if dark { 0x4c5057 } else { 0xd8d8d8 }),
            rgb(if dark { 0xdfe1e5 } else { 0x1f1f1f }),
            rgb(if dark { 0x474b52 } else { 0xe0e0e0 }),
        )
    };
    button(id, label)
        .disabled(disabled)
        .h(theme::scaled_px(24.).max(px(24.)))
        .rounded(px(4.))
        .bg(bg)
        .border_color(border)
        .text_color(fg)
        .font_weight(gpui_kit::FontWeight::BOLD)
        .when(!disabled, |this| this.hover(move |style| style.bg(hover_bg)))
}

pub(super) fn sessions_table_frame(table: impl IntoElement) -> gpui_kit::Div {
    let dark = theme::is_dark();
    let border = rgb(if dark { 0x464a52 } else { 0xd4d4d4 });
    let caret_color = rgb(if dark { 0xdfe1e5 } else { 0x000000 });
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .border_1()
                .border_color(border)
                .rounded(px(0.))
                .bg(theme::surface())
                .overflow_hidden()
                .child(table),
        )
        .child(
            div()
                .ml(px(2.))
                .w(px(10.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::default()
                        .data(TABLE_CARET_RIGHT_SVG)
                        .size(px(9.))
                        .text_color(caret_color),
                ),
        )
}

pub(super) fn inline_checkbox(
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
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(2.))
        .py(px(2.))
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
