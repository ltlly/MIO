use gpui_kit::component::{
    IndexPath,
    select::{Select, SelectState},
    table::{Column, TableDelegate},
};
use mio_core::{AdvancedScopeRule, ScopeProtocol, Uuid};

use super::*;

const ADV_INC_FIELDS: [(&str, &str); 3] = [
    ("settings-scope-adv-host", "Host / IP range"),
    ("settings-scope-adv-port", "Port"),
    ("settings-scope-adv-file", "File"),
];
const ADV_EXC_FIELDS: [(&str, &str); 3] = [
    ("settings-scope-adv-exc-host", "Host / IP range"),
    ("settings-scope-adv-exc-port", "Port"),
    ("settings-scope-adv-exc-file", "File"),
];
const ADV_META: [(&str, &str, &str, &str, &str, &str, &str); 2] = [
    (
        "Include in scope",
        "settings-scope-adv-include-add",
        "settings-scope-adv-include-cancel",
        "settings-scope-adv-include-toggle",
        "settings-scope-adv-include-edit",
        "settings-scope-adv-include-remove",
        "settings-scope-adv-include-paste",
    ),
    (
        "Exclude from scope",
        "settings-scope-adv-exclude-add",
        "settings-scope-adv-exclude-cancel",
        "settings-scope-adv-exclude-toggle",
        "settings-scope-adv-exclude-edit",
        "settings-scope-adv-exclude-remove",
        "settings-scope-adv-exclude-paste",
    ),
];

pub(super) struct AdvScopeTable {
    owner: WeakEntity<Settings>,
    include: bool,
}

const ADV_COLUMNS: [(&str, f32); 5] = [
    ("Enabled", 68.),
    ("Protocol", 80.),
    ("Host / IP range", 214.),
    ("Port", 84.),
    ("File", 186.),
];

impl TableDelegate for AdvScopeTable {
    fn columns_count(&self, _: &App) -> usize {
        ADV_COLUMNS.len()
    }

    fn column(&self, index: usize, _: &App) -> Column {
        Column::new(index.to_string(), ADV_COLUMNS[index].0)
            .width((ADV_COLUMNS[index].1 * theme::ui_scale()).round())
            .p_0()
            .movable(false)
    }

    fn rows_count(&self, cx: &App) -> usize {
        self.owner.upgrade().map_or(0, |owner| {
            let scope = &owner.read(cx).state.scope;
            if self.include {
                scope.advanced_include.len()
            } else {
                scope.advanced_exclude.len()
            }
        })
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        settings_table_th(ADV_COLUMNS[col_ix].0, ADV_COLUMNS[col_ix].1)
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
            .id((
                if self.include {
                    "settings-scope-adv-include-row"
                } else {
                    "settings-scope-adv-exclude-row"
                },
                row,
            ))
            .border_b_0()
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
        let (rule, enabled, wb) = {
            let settings = owner.read(cx);
            let rules = if self.include {
                &settings.state.scope.advanced_include
            } else {
                &settings.state.scope.advanced_exclude
            };
            let Some(rule) = rules.get(row).cloned() else {
                return div().into_any_element();
            };
            (rule, settings.state.enabled, settings.owner.clone())
        };
        if column == 0 {
            let include = self.include;
            let id = rule.id;
            let toggle_id = ADV_META[usize::from(!include)].3;
            return settings_table_checkbox_cell(
                listeners_tls::indexed_checkbox((toggle_id, row), "", rule.enabled, enabled)
                    .p_0()
                    .gap_0()
                    .accessibility_label(format!("启用高级范围规则 {}", rule.host))
                    .on_change(move |_, _, _, cx| {
                        let _ = wb.update(cx, |view, cx| {
                            view.toggle_advanced_scope_rule(include, id, cx);
                        });
                    }),
            )
            .into_any_element();
        }
        div()
            .px(px(6.))
            .truncate()
            .child(adv_scope_cell(&rule, column))
            .into_any_element()
    }

    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        let owner = self.owner.upgrade()?;
        let settings = owner.read(cx);
        let rules = if self.include {
            &settings.state.scope.advanced_include
        } else {
            &settings.state.scope.advanced_exclude
        };
        Some(adv_scope_cell(rules.get(row)?, column).into())
    }
}

fn adv_scope_cell(rule: &AdvancedScopeRule, column: usize) -> String {
    match column {
        0 => if rule.enabled { "Yes" } else { "No" }.into(),
        1 => match rule.protocol {
            ScopeProtocol::Any => "Any",
            ScopeProtocol::Http => "HTTP",
            ScopeProtocol::Https => "HTTPS",
        }
        .into(),
        2 => rule.host.clone(),
        3 => rule.port.clone(),
        _ => rule.file.clone(),
    }
}

pub(super) struct SettingsAdvScopeState {
    pub(super) protocols: [ScopeProtocol; 2],
    pub(super) protocol_selects: [Entity<SelectState<Vec<&'static str>>>; 2],
    pub(in crate::workbench::settings) tables: [Entity<TableState<AdvScopeTable>>; 2],
    pub(super) include: [Entity<InputState>; 3],
    pub(super) exclude: [Entity<InputState>; 3],
    pub(in crate::workbench::settings) editing: [Option<Uuid>; 2],
}

impl SettingsAdvScopeState {
    pub(super) fn new(
        owner: WeakEntity<Settings>,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let mk = |window: &mut Window, cx: &mut App| {
            [
                cx.new(|cx| InputState::new(window, cx).placeholder("Host / IP 段 / 正则")),
                cx.new(|cx| InputState::new(window, cx).placeholder("Port 正则")),
                cx.new(|cx| InputState::new(window, cx).placeholder("File 正则")),
            ]
        };
        let make_select = |window: &mut Window, cx: &mut App| {
            cx.new(|scx| {
                SelectState::new(
                    vec!["Any", "HTTP", "HTTPS"],
                    Some(IndexPath::new(0)),
                    window,
                    scx,
                )
            })
        };
        let protocol_selects = [make_select(window, cx), make_select(window, cx)];
        let inc_owner = owner.clone();
        let inc_table = cx.new(|tcx| {
            TableState::new(
                AdvScopeTable {
                    owner: inc_owner,
                    include: true,
                },
                window,
                tcx,
            )
            .sortable(false)
            .col_selectable(false)
        });
        let exc_table = cx.new(|tcx| {
            TableState::new(
                AdvScopeTable {
                    owner,
                    include: false,
                },
                window,
                tcx,
            )
            .sortable(false)
            .col_selectable(false)
        });
        Self {
            protocols: [ScopeProtocol::Any, ScopeProtocol::Any],
            protocol_selects,
            tables: [inc_table, exc_table],
            include: mk(window, cx),
            exclude: mk(window, cx),
            editing: [None, None],
        }
    }
}

impl Workbench {
    pub(in crate::workbench) fn upsert_advanced_scope_rule(
        &mut self,
        project: Option<ProjectKey>,
        include: bool,
        editing_id: Option<Uuid>,
        rule: AdvancedScopeRule,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.can_mutate_scope(project) {
            return Err("当前项目状态不可修改目标范围。".into());
        }
        let mut next = self.workspace.scope.clone();
        let previous = self.workspace.scope.clone();
        let rules = if include {
            &mut next.advanced_include
        } else {
            &mut next.advanced_exclude
        };
        if let Some(id) = editing_id
            && let Some(existing) = rules.iter_mut().find(|item| item.id == id)
        {
            existing.protocol = rule.protocol;
            existing.host = rule.host;
            existing.port = rule.port;
            existing.file = rule.file;
        } else {
            rules.push(rule);
        }
        if let Err(error) = next.validate() {
            return Err(error.to_string());
        }
        self.workspace.scope = next;
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.scope = previous;
            return Err(format!("未添加高级 Scope 规则：{error}"));
        }
        self.save_workspace(cx);
        self.error = None;
        cx.notify();
        Ok(())
    }
}

impl Settings {
    pub(super) fn submit_advanced_scope_rule(
        &mut self,
        include: bool,
        adv: &Entity<SettingsAdvScopeState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (protocol, inputs, edit_id) = {
            let st = adv.read(cx);
            let side = usize::from(!include);
            let inputs = if include {
                st.include.clone()
            } else {
                st.exclude.clone()
            };
            let proto = match st.protocol_selects[side]
                .read(cx)
                .selected_value()
                .copied()
            {
                Some("HTTP") => ScopeProtocol::Http,
                Some("HTTPS") => ScopeProtocol::Https,
                _ => ScopeProtocol::Any,
            };
            (proto, inputs, st.editing[side])
        };
        let mut values: [String; 3] = std::array::from_fn(|_| String::new());
        for (idx, input) in inputs.iter().enumerate() {
            match editing::input_snapshot(input, window, cx) {
                Ok(val) => values[idx] = val.trim().to_owned(),
                Err(err) => {
                    self.settings_error(err, cx);
                    return;
                }
            }
        }
        let rule = AdvancedScopeRule {
            id: Uuid::new_v4(),
            enabled: true,
            protocol,
            host: values[0].clone(),
            port: values[1].clone(),
            file: values[2].clone(),
        };
        let project = self.state.project;
        let res = self
            .owner
            .update(cx, |view, cx| {
                view.upsert_advanced_scope_rule(project, include, edit_id, rule, cx)
            })
            .unwrap_or_else(|_| Err("主窗口已关闭。".into()));
        match res {
            Ok(()) => {
                adv.update(cx, |st, _| st.editing[usize::from(!include)] = None);
                for input in &inputs {
                    input.update(cx, |s, cx| s.set_value("", window, cx));
                }
            }
            Err(err) => self.settings_error(err, cx),
        }
    }

    fn split_settings_advanced_url(
        &mut self,
        include: bool,
        adv: &Entity<SettingsAdvScopeState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let side = usize::from(!include);
        let inputs = {
            let st = adv.read(cx);
            if include {
                st.include.clone()
            } else {
                st.exclude.clone()
            }
        };
        let url = match editing::input_snapshot(&inputs[0], window, cx) {
            Ok(val) => val.trim().to_owned(),
            Err(err) => {
                self.settings_error(err, cx);
                return;
            }
        };
        let (protocol, host, port, file) = AdvancedScopeRule::split_url_patterns(&url);
        let idx = match protocol {
            ScopeProtocol::Any => 0,
            ScopeProtocol::Http => 1,
            ScopeProtocol::Https => 2,
        };
        let sel = adv.read(cx).protocol_selects[side].clone();
        sel.update(cx, |s, cx| {
            s.set_selected_index(Some(IndexPath::new(idx)), window, cx)
        });
        adv.update(cx, |st, _| st.protocols[side] = protocol);
        for (input, value) in inputs.iter().zip([host, port, file]) {
            input.update(cx, |s, cx| s.set_value(value, window, cx));
        }
        cx.notify();
    }

    fn load_advanced_scope_rules_from_file(
        &mut self,
        include: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.enabled {
            return;
        }
        let project = self.state.project;
        let paths = crate::native_terminate::prompt_for_paths(
            window,
            cx,
            gpui_kit::PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some(if include {
                    "加载高级包含范围规则（URL、主机或网段列表）".into()
                } else {
                    "加载高级排除范围规则（URL、主机或网段列表）".into()
                }),
            },
        );
        cx.spawn_in(window, async move |this, cx| {
            let picked = match paths.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) => return,
                Ok(Err(err)) => {
                    let _ = this.update_in(cx, |this, _, cx| {
                        this.settings_error(format!("打开文件选择器失败：{err}"), cx);
                    });
                    return;
                }
                Err(_) => {
                    let _ = this.update_in(cx, |this, _, cx| {
                        this.settings_error("文件选择器通道意外关闭。", cx);
                    });
                    return;
                }
            };
            let Some(path) = picked else {
                return;
            };
            let result: Result<Vec<String>, String> = cx
                .background_executor()
                .spawn(async move { mio_core::ui_files::read_utf8_rule_lines(&path, 256 * 1024, 2048) })
                .await;
            let _ = this.update_in(cx, |this, _, cx| {
                match result {
                    Ok(lines) => {
                        for line in lines {
                            let (protocol, host, port, file) = if line.contains("://") {
                                AdvancedScopeRule::split_url_patterns(&line)
                            } else {
                                (ScopeProtocol::Any, line, String::new(), String::new())
                            };
                            let rule = AdvancedScopeRule {
                                id: Uuid::new_v4(),
                                enabled: true,
                                protocol,
                                host,
                                port,
                                file,
                            };
                            let res = this
                                .owner
                                .update(cx, |view, cx| {
                                    view.upsert_advanced_scope_rule(project, include, None, rule, cx)
                                })
                                .unwrap_or_else(|_| Err("主窗口已关闭。".into()));
                            if let Err(err) = res {
                                this.settings_error(err, cx);
                                break;
                            }
                        }
                    }
                    Err(err) => this.settings_error(err, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_settings_advanced_scope(
        &self,
        enabled: bool,
        adv: &Entity<SettingsAdvScopeState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(self.render_settings_adv_card(true, enabled, adv, cx))
            .child(self.render_settings_adv_card(false, enabled, adv, cx))
            .into_any_element()
    }

    fn render_settings_adv_card(
        &self,
        include: bool,
        enabled: bool,
        adv: &Entity<SettingsAdvScopeState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let side = usize::from(!include);
        let (inputs, edit_id, table, proto_select) = {
            let st = adv.read(cx);
            (
                if include {
                    st.include.clone()
                } else {
                    st.exclude.clone()
                },
                st.editing[side],
                st.tables[side].clone(),
                st.protocol_selects[side].clone(),
            )
        };
        let fields = if include {
            ADV_INC_FIELDS
        } else {
            ADV_EXC_FIELDS
        };
        let rules = if include {
            &self.state.scope.advanced_include
        } else {
            &self.state.scope.advanced_exclude
        };
        let selected_row = table
            .read(cx)
            .selected_row()
            .filter(|&row| row < rules.len());
        let selected_rule = selected_row.and_then(|row| rules.get(row).cloned());
        let (title, add_id, cancel_id, _, edit_pfx, rm_pfx, paste_id) = ADV_META[side];
        let adv_add = adv.clone();
        let adv_cancel = adv.clone();
        let inputs_cancel = inputs.clone();
        let adv_edit = adv.clone();
        let adv_rm = adv.clone();
        let inputs_edit = inputs.clone();
        let selected_for_edit = selected_rule.clone();
        let selected_for_remove = selected_rule.map(|rule| rule.id);
        let project = self.state.project;
        let proto_select_bar = proto_select.clone();
        let side_buttons = div()
            .w(theme::scaled_px(88.).max(px(84.)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(5.))
            .child(
                settings_action_button(
                    add_id,
                    if edit_id.is_some() { "保存" } else { "添加" },
                    !enabled,
                )
                .w_full()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.submit_advanced_scope_rule(include, &adv_add, window, cx);
                })),
            )
            .child(
                settings_action_button(
                    (edit_pfx, selected_row.unwrap_or(0)),
                    "编辑",
                    !enabled || selected_for_edit.is_none(),
                )
                .w_full()
                .on_click(cx.listener(move |this, _, window, cx| {
                    let Some(cloned) = &selected_for_edit else {
                        return;
                    };
                    for inp in &inputs_edit {
                        if let Err(err) = editing::input_snapshot(inp, window, cx) {
                            this.settings_error(err, cx);
                            return;
                        }
                    }
                    let idx = match cloned.protocol {
                        ScopeProtocol::Any => 0,
                        ScopeProtocol::Http => 1,
                        ScopeProtocol::Https => 2,
                    };
                    proto_select.update(cx, |s, cx| {
                        s.set_selected_index(Some(IndexPath::new(idx)), window, cx)
                    });
                    adv_edit.update(cx, |st, _| {
                        st.editing[side] = Some(cloned.id);
                        st.protocols[side] = cloned.protocol;
                    });
                    for (inp, val) in inputs_edit.iter().zip([
                        cloned.host.clone(),
                        cloned.port.clone(),
                        cloned.file.clone(),
                    ]) {
                        inp.update(cx, |s, cx| s.set_value(val, window, cx));
                    }
                    inputs_edit[0].update(cx, |s, cx| s.focus(window, cx));
                    cx.notify();
                })),
            )
            .child(
                settings_action_button(
                    (rm_pfx, selected_row.unwrap_or(0)),
                    "移除",
                    !enabled || selected_for_remove.is_none(),
                )
                .w_full()
                .on_click(cx.listener(move |this, _, _, cx| {
                    let Some(id) = selected_for_remove else {
                        return;
                    };
                    adv_rm.update(cx, |st, _| {
                        if st.editing[side] == Some(id) {
                            st.editing[side] = None;
                        }
                    });
                    let _ = this.owner.update(cx, |view, cx| {
                        view.remove_advanced_scope_rule(include, id, cx);
                    });
                })),
            )
            .child(
                settings_action_button(paste_id, "粘贴 URL", !enabled)
                    .w_full()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(raw) = cx.read_from_clipboard().and_then(|item| item.text()) {
                            let trimmed = raw.trim();
                            if !trimmed.is_empty() {
                                let (protocol, host, port, file) =
                                    AdvancedScopeRule::split_url_patterns(trimmed);
                                let rule = AdvancedScopeRule {
                                    id: Uuid::new_v4(),
                                    enabled: true,
                                    protocol,
                                    host,
                                    port,
                                    file,
                                };
                                let res = this
                                    .owner
                                    .update(cx, |view, cx| {
                                        view.upsert_advanced_scope_rule(
                                            project, include, None, rule, cx,
                                        )
                                    })
                                    .unwrap_or_else(|_| Err("主窗口已关闭。".into()));
                                if let Err(err) = res {
                                    this.settings_error(err, cx);
                                }
                            }
                        }
                    })),
            )
            .child(
                settings_action_button(
                    if include {
                        "settings-scope-adv-include-load"
                    } else {
                        "settings-scope-adv-exclude-load"
                    },
                    "加载...",
                    !enabled,
                )
                .w_full()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.load_advanced_scope_rules_from_file(include, window, cx)
                })),
            );
        let adv_split = adv.clone();
        let proto_id = if include {
            "settings-scope-protocol-select"
        } else {
            "settings-scope-exclude-protocol-select"
        };
        let paste_url_id = if include {
            "settings-scope-advanced-paste"
        } else {
            "settings-scope-exclude-advanced-paste"
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(div().font_weight(gpui_kit::FontWeight::MEDIUM).child(title))
            .child(
                toolbar()
                    .child("Protocol:")
                    .child(
                        Select::new(&proto_select_bar)
                            .id(proto_id)
                            .small()
                            .w(theme::scaled_px(160.).max(px(160.)))
                            .disabled(!enabled),
                    )
                    .child(
                        settings_action_button(paste_url_id, "解析 URL 为正则", !enabled)
                            .tooltip("将 Host 输入框中的完整 URL 自动拆分为协议、主机、端口与路径正则")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.split_settings_advanced_url(include, &adv_split, window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(9.))
                    .h(theme::scaled_px(148.).max(px(148.)))
                    .child(side_buttons)
                    .child(
                        settings_table_frame(
                            DataTable::new(&table)
                                .with_size(px(22.))
                                .header_row_height(px(24.))
                                .bordered(false)
                                .stripe(false)
                                .scrollbar_visible(true, false)
                                .scroll_bounce(false)
                                .accessibility_label(title),
                            true,
                        )
                        .max_w(theme::scaled_px(645.).max(px(645.))),
                    ),
            )
            .child(
                toolbar()
                    .children(inputs.iter().zip(fields).map(|(inp, (id, aria))| {
                        div().flex_1().min_w_0().child(
                            Input::new(inp)
                                .id(id)
                                .aria_label(aria)
                                .small()
                                .disabled(!enabled),
                        )
                    }))
                    .when(edit_id.is_some(), |bar| {
                        bar.child(
                            settings_action_button(cancel_id, "取消", !enabled)
                                .tooltip("放弃编辑并清空输入框")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    for inp in &inputs_cancel {
                                        if let Err(err) = editing::input_snapshot(inp, window, cx) {
                                            this.settings_error(err, cx);
                                            return;
                                        }
                                    }
                                    adv_cancel.update(cx, |st, _| {
                                        st.editing[usize::from(!include)] = None;
                                    });
                                    for inp in &inputs_cancel {
                                        inp.update(cx, |s, cx| s.set_value("", window, cx));
                                    }
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{AppContext, TestAppContext, test::TestWindowExt};
    use mio_core::ScopeProtocol;

    use crate::workbench::tests::{
        drain, fixture,
        settings::{acknowledge, open_settings, saved},
        tab,
    };

    #[gpui_kit::test]
    fn settings_project_scope_toggles_advanced_mode_and_edits_regex_rules(cx: &mut TestAppContext) {
        let mut fixture = fixture(cx, vec![tab("1")], 64);
        drain(&mut fixture.commands);
        let settings = open_settings(&fixture, cx);

        cx.update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            window.click("settings-project-scope-nav", cx);
            window.render_frame(cx);
            window.click("settings-scope-advanced-toggle", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let ws_adv = saved(&mut fixture.commands);
        assert!(ws_adv.scope.advanced_mode);
        acknowledge(&fixture, ws_adv, cx);

        cx.update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            window.click("settings-scope-protocol-select", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let open = cx
            .update_window(settings, |_, window, cx| {
                window.render_frame(cx);
                window.find("settings-scope-protocol-select").expanded() == Some(true)
            })
            .unwrap();
        if !open {
            cx.simulate_keystrokes(settings, "enter");
            cx.run_until_parked();
        }
        cx.simulate_keystrokes(settings, "down down enter");
        cx.run_until_parked();

        cx.update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            window.click("settings-scope-adv-host", cx);
            window.input("^api\\.example\\.com$", cx);
            window.click("settings-scope-adv-port", cx);
            window.input("^8443$", cx);
            window.click("settings-scope-adv-file", cx);
            window.input("^/v1/.*", cx);
            window.render_frame(cx);
            window.click("settings-scope-adv-include-add", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let ws_inc = saved(&mut fixture.commands);
        assert_eq!(ws_inc.scope.advanced_include.len(), 1);
        assert_eq!(
            ws_inc.scope.advanced_include[0].protocol,
            ScopeProtocol::Https
        );
        assert_eq!(
            ws_inc.scope.advanced_include[0].host,
            "^api\\.example\\.com$"
        );
        assert_eq!(ws_inc.scope.advanced_include[0].port, "^8443$");
        assert_eq!(ws_inc.scope.advanced_include[0].file, "^/v1/.*");
        acknowledge(&fixture, ws_inc, cx);

        cx.update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            window.click("settings-scope-adv-exc-file", cx);
            window.input("^/v1/logout$", cx);
            window.render_frame(cx);
            window.click("settings-scope-adv-exclude-add", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let ws_exc = saved(&mut fixture.commands);
        assert_eq!(ws_exc.scope.advanced_exclude.len(), 1);
        assert_eq!(ws_exc.scope.advanced_exclude[0].file, "^/v1/logout$");
        acknowledge(&fixture, ws_exc, cx);

        cx.update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            window.click(("settings-scope-adv-include-row", 0usize), cx);
            window.render_frame(cx);
            window.click(("settings-scope-adv-include-edit", 0usize), cx);
            window.render_frame(cx);
            window.click("settings-scope-adv-port", cx);
            window.press(crate::kbd!("a"), cx);
            window.input("^443$", cx);
            window.render_frame(cx);
            window.click("settings-scope-adv-include-add", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let ws_edited = saved(&mut fixture.commands);
        assert_eq!(ws_edited.scope.advanced_include.len(), 1);
        assert_eq!(ws_edited.scope.advanced_include[0].port, "^443$");
    }
}
