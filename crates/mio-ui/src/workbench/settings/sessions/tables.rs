use gpui_kit::base::{Checkbox, CheckboxIndicator, CheckboxState};
use gpui_kit::component::table::{Column, TableDelegate};

use super::*;

pub(super) const DEFAULT_RULE_DESCRIPTION: &str = "使用 External Cookie Jar 中的 Cookie";

fn mio_table_zebra_bg() -> gpui_kit::Rgba {
    rgb(if theme::is_dark() { 0x282b30 } else { 0xf4f5f7 })
}

fn format_rule_tools(tools: &SessionToolScope) -> String {
    let mut parts = Vec::new();
    if tools.proxy {
        parts.push("Proxy");
    }
    if tools.repeater {
        parts.push("Repeater");
    }
    if tools.intruder {
        parts.push("Intruder");
    }
    if tools.scanner {
        parts.push("Scanner");
    }
    if tools.sequencer {
        parts.push("Sequencer");
    }
    if tools.extensions {
        parts.push("Extensions");
    }
    if tools.mio_ai {
        parts.push("MioAI");
    }
    parts.join(", ")
}

fn table_cell_checkbox(
    id: impl Into<gpui_kit::ElementId>,
    aria_label: &'static str,
    checked: bool,
    enabled: bool,
) -> Checkbox {
    Checkbox::new(id)
        .accessibility_label(aria_label)
        .checked(checked)
        .disabled(!enabled)
        .flex()
        .items_center()
        .justify_center()
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
}

pub(in crate::workbench::settings) struct SessionRulesTable {
    pub(super) owner: WeakEntity<Settings>,
}

impl SessionRulesTable {
    fn showing_default_rule(&self, cx: &App) -> bool {
        self.owner.upgrade().is_some_and(|o| {
            let s = o.read(cx);
            s.state.sessions.rules.is_empty() && !s.sessions.default_rule_dismissed
        })
    }
}

impl TableDelegate for SessionRulesTable {
    fn columns_count(&self, _: &App) -> usize {
        3
    }

    fn column(&self, column: usize, _: &App) -> Column {
        let scale = theme::ui_scale();
        match column {
            0 => Column::new("enabled", "Enabled")
                .width((76. * scale).round())
                .movable(false),
            1 => Column::new("description", "Description")
                .width((316. * scale).round())
                .movable(false),
            _ => Column::new("tools", "Tools")
                .width((298. * scale).round())
                .movable(false),
        }
    }

    fn rows_count(&self, cx: &App) -> usize {
        if self.showing_default_rule(cx) {
            return 1;
        }
        self.owner
            .upgrade()
            .map_or(0, |o| o.read(cx).state.sessions.rules.len())
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }

    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }

    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        div()
            .id(("settings-sessions-rule-row", row))
            .border_b_0()
            .when(theme::table_zebra_stripe() && row % 2 == 1, |this| {
                this.bg(mio_table_zebra_bg())
            })
    }

    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(owner) = self.owner.upgrade() else {
            return div().into_any_element();
        };
        let (enabled, showing_default, default_enabled, rule_opt) = {
            let s = owner.read(cx);
            let showing_default =
                s.state.sessions.rules.is_empty() && !s.sessions.default_rule_dismissed;
            (
                s.state.enabled,
                showing_default,
                s.sessions.default_rule_enabled,
                s.state.sessions.rules.get(row).cloned(),
            )
        };
        if showing_default && row == 0 {
            return match column {
                0 => {
                    let owner_weak = self.owner.clone();
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            table_cell_checkbox(
                                "settings-sessions-rule-enable-default",
                                "启用默认会话处理规则",
                                default_enabled,
                                enabled,
                            )
                            .on_change(move |st, _, _, cx| {
                                let _ = owner_weak.update(cx, |this, cx| {
                                    this.toggle_default_cookie_jar_rule(
                                        st == CheckboxState::Checked,
                                        cx,
                                    );
                                });
                            }),
                        )
                        .into_any_element()
                }
                1 => div()
                    .truncate()
                    .child(DEFAULT_RULE_DESCRIPTION)
                    .into_any_element(),
                _ => div().truncate().child("Scanner").into_any_element(),
            };
        }
        let Some(rule) = rule_opt else {
            return div().into_any_element();
        };
        match column {
            0 => {
                let owner_weak = self.owner.clone();
                let rule_id = rule.id;
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        table_cell_checkbox(
                            ("settings-sessions-rule-enable", row),
                            "启用或禁用会话处理规则",
                            rule.enabled,
                            enabled,
                        )
                        .on_change(move |st, _, _, cx| {
                            let _ = owner_weak.update(cx, |this, cx| {
                                this.set_session_rule_enabled(
                                    rule_id,
                                    st == CheckboxState::Checked,
                                    cx,
                                );
                            });
                        }),
                    )
                    .into_any_element()
            }
            1 => div().truncate().child(rule.name).into_any_element(),
            _ => div()
                .truncate()
                .child(format_rule_tools(&rule.tools))
                .into_any_element(),
        }
    }

    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        let owner = self.owner.upgrade()?;
        let s = owner.read(cx);
        if s.state.sessions.rules.is_empty() && !s.sessions.default_rule_dismissed && row == 0 {
            let text = match column {
                0 => if s.sessions.default_rule_enabled {
                    "Yes"
                } else {
                    "No"
                }
                .to_owned(),
                1 => DEFAULT_RULE_DESCRIPTION.to_owned(),
                _ => "Scanner".to_owned(),
            };
            return Some(text.into());
        }
        let rule = s.state.sessions.rules.get(row)?;
        let text = match column {
            0 => if rule.enabled { "Yes" } else { "No" }.to_owned(),
            1 => rule.name.clone(),
            _ => format_rule_tools(&rule.tools),
        };
        Some(text.into())
    }
}

pub(in crate::workbench::settings) struct CookieJarTable {
    pub(super) owner: WeakEntity<Settings>,
}

impl TableDelegate for CookieJarTable {
    fn columns_count(&self, _: &App) -> usize {
        6
    }

    fn column(&self, column: usize, _: &App) -> Column {
        let scale = theme::ui_scale();
        match column {
            0 => Column::new("domain", "Domain")
                .width((150. * scale).round())
                .movable(false),
            1 => Column::new("path", "Path")
                .width((100. * scale).round())
                .movable(false),
            2 => Column::new("name", "Name")
                .width((130. * scale).round())
                .movable(false),
            3 => Column::new("value", "Value")
                .width((160. * scale).round())
                .movable(false),
            4 => Column::new("flags", "Flags")
                .width((110. * scale).round())
                .movable(false),
            _ => Column::new("expires", "Expires")
                .width((110. * scale).round())
                .movable(false),
        }
    }

    fn rows_count(&self, cx: &App) -> usize {
        self.owner
            .upgrade()
            .map_or(0, |o| o.read(cx).state.sessions.cookie_jar.entries.len())
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }

    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }

    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        div()
            .id(("settings-cookie-jar-row", row))
            .border_b_0()
            .when(theme::table_zebra_stripe() && row % 2 == 1, |this| {
                this.bg(mio_table_zebra_bg())
            })
    }

    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(owner) = self.owner.upgrade() else {
            return div().into_any_element();
        };
        let entry = match owner.read(cx).state.sessions.cookie_jar.entries.get(row) {
            Some(entry) => entry.clone(),
            None => return div().into_any_element(),
        };
        drop(owner);
        let text = match column {
            0 => {
                if entry.host_only {
                    entry.domain.clone()
                } else {
                    format!(".{}", entry.domain)
                }
            }
            1 => entry.path.clone(),
            2 => entry.name.clone(),
            3 => entry.value.clone(),
            4 => {
                let mut parts = Vec::new();
                if entry.secure {
                    parts.push("Secure");
                }
                if entry.http_only {
                    parts.push("HttpOnly");
                }
                parts.join("+")
            }
            _ => entry
                .expires_unix_ms
                .map_or_else(|| "Session".into(), |ms| ms.to_string()),
        };
        div().truncate().child(text).into_any_element()
    }

    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        let owner = self.owner.upgrade()?;
        let entry = owner.read(cx).state.sessions.cookie_jar.entries.get(row)?;
        let text = match column {
            0 => {
                if entry.host_only {
                    entry.domain.clone()
                } else {
                    format!(".{}", entry.domain)
                }
            }
            1 => entry.path.clone(),
            2 => entry.name.clone(),
            3 => entry.value.clone(),
            4 => {
                let mut parts = Vec::new();
                if entry.secure {
                    parts.push("Secure");
                }
                if entry.http_only {
                    parts.push("HttpOnly");
                }
                parts.join("+")
            }
            _ => entry
                .expires_unix_ms
                .map_or_else(|| "Session".into(), |ms| ms.to_string()),
        };
        Some(text.into())
    }
}

pub(in crate::workbench::settings) struct SessionMacrosTable {
    pub(super) owner: WeakEntity<Settings>,
}

impl TableDelegate for SessionMacrosTable {
    fn columns_count(&self, _: &App) -> usize {
        4
    }

    fn column(&self, column: usize, _: &App) -> Column {
        let scale = theme::ui_scale();
        match column {
            0 => Column::new("name", "Description")
                .width((260. * scale).round())
                .movable(false),
            1 => Column::new("steps", "Steps")
                .width((100. * scale).round())
                .movable(false),
            2 => Column::new("policy", "Failure")
                .width((165. * scale).round())
                .movable(false),
            _ => Column::new("timeout", "Timeout")
                .width((165. * scale).round())
                .movable(false),
        }
    }

    fn rows_count(&self, cx: &App) -> usize {
        self.owner
            .upgrade()
            .map_or(0, |o| o.read(cx).state.sessions.macros.len())
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }

    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }

    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        div()
            .id(("settings-session-macro-row", row))
            .border_b_0()
            .when(theme::table_zebra_stripe() && row % 2 == 1, |this| {
                this.bg(mio_table_zebra_bg())
            })
    }

    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(owner) = self.owner.upgrade() else {
            return div().into_any_element();
        };
        let mac = match owner.read(cx).state.sessions.macros.get(row) {
            Some(mac) => mac.clone(),
            None => return div().into_any_element(),
        };
        drop(owner);
        let text = match column {
            0 => mac.name.clone(),
            1 => format!("{}", mac.steps.len()),
            2 => match mac.failure_policy {
                MacroFailurePolicy::Abort => "Abort".into(),
                MacroFailurePolicy::Retry { max_retries } => {
                    format!("Retry({max_retries})")
                }
            },
            _ => format!("{} ms", mac.timeout_ms),
        };
        div().truncate().child(text).into_any_element()
    }

    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        let owner = self.owner.upgrade()?;
        let mac = owner.read(cx).state.sessions.macros.get(row)?;
        let text = match column {
            0 => mac.name.clone(),
            1 => format!("{}", mac.steps.len()),
            2 => match mac.failure_policy {
                MacroFailurePolicy::Abort => "Abort".into(),
                MacroFailurePolicy::Retry { max_retries } => format!("Retry({max_retries})"),
            },
            _ => format!("{} ms", mac.timeout_ms),
        };
        Some(text.into())
    }
}
