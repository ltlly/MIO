use gpui_kit::accesskit::Role;
use gpui_kit::base::CheckboxState;
use gpui_kit::component::table::{Column, TableDelegate};
use mio_core::{TargetScope, Uuid};

use super::*;

mod advanced;
use advanced::SettingsAdvScopeState;

type SimpleScopeMeta = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);
const SIMPLE_META: [SimpleScopeMeta; 2] = [
    (
        "Include in scope",
        "settings-scope-include-input",
        "包含规则前缀",
        "settings-scope-include-add",
        "添加",
        "settings-scope-include-edit",
        "settings-scope-include-remove",
        "settings-scope-include-paste",
        "settings-scope-include-load",
    ),
    (
        "Exclude from scope",
        "settings-scope-exclude-input",
        "排除规则前缀",
        "settings-scope-exclude-add",
        "添加",
        "settings-scope-exclude-edit",
        "settings-scope-exclude-remove",
        "settings-scope-exclude-paste",
        "settings-scope-exclude-load",
    ),
];

const SIMPLE_SCOPE_COLUMNS: [(&str, f32); 3] = [
    ("Enabled", 74.),
    ("Prefix", 410.),
    ("Include subdomains", 148.),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScopeRuleList {
    Include,
    Exclude,
    OutOfScopeCustom,
}

struct SimpleScopeTable {
    owner: WeakEntity<Settings>,
    kind: ScopeRuleList,
}

impl TableDelegate for SimpleScopeTable {
    fn columns_count(&self, _: &App) -> usize {
        SIMPLE_SCOPE_COLUMNS.len()
    }

    fn column(&self, index: usize, _: &App) -> Column {
        let (name, width) = SIMPLE_SCOPE_COLUMNS[index];
        Column::new(index.to_string(), name)
            .width((width * theme::ui_scale()).round())
            .p_0()
            .movable(false)
    }

    fn rows_count(&self, cx: &App) -> usize {
        self.owner.upgrade().map_or(0, |owner| {
            let scope = &owner.read(cx).state.scope;
            match self.kind {
                ScopeRuleList::Include => scope.include.len(),
                ScopeRuleList::Exclude => scope.exclude.len(),
                ScopeRuleList::OutOfScopeCustom => scope.out_of_scope_custom_rules.len(),
            }
        })
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        settings_table_th(
            SIMPLE_SCOPE_COLUMNS[col_ix].0,
            SIMPLE_SCOPE_COLUMNS[col_ix].1,
        )
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
                match self.kind {
                    ScopeRuleList::Include => "settings-scope-include-row",
                    ScopeRuleList::Exclude => "settings-scope-exclude-row",
                    ScopeRuleList::OutOfScopeCustom => "settings-scope-oos-custom-row",
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
        let (rule, enabled, project, wb) = {
            let settings = owner.read(cx);
            let rules = match self.kind {
                ScopeRuleList::Include => &settings.state.scope.include,
                ScopeRuleList::Exclude => &settings.state.scope.exclude,
                ScopeRuleList::OutOfScopeCustom => &settings.state.scope.out_of_scope_custom_rules,
            };
            let Some(rule) = rules.get(row).cloned() else {
                return div().into_any_element();
            };
            (
                rule,
                settings.state.enabled,
                settings.state.project,
                settings.owner.clone(),
            )
        };
        match column {
            0 => {
                let id = rule.id;
                let toggle_id = match self.kind {
                    ScopeRuleList::Include => "settings-scope-toggle",
                    ScopeRuleList::Exclude => "settings-scope-exclude-toggle",
                    ScopeRuleList::OutOfScopeCustom => "settings-scope-oos-custom-toggle",
                };
                settings_table_checkbox_cell(
                    listeners_tls::indexed_checkbox((toggle_id, row), "", rule.enabled, enabled)
                        .p_0()
                        .gap_0()
                        .accessibility_label(format!("启用范围规则 {}", rule.prefix))
                        .on_change(move |_, _, _, cx| {
                            let _ = wb.update(cx, |view, cx| view.toggle_scope_rule(project, id, cx));
                        }),
                )
                .into_any_element()
            }
            1 => div().px(px(6.)).truncate().child(rule.prefix).into_any_element(),
            _ => {
                let id = rule.id;
                let sub_id = match self.kind {
                    ScopeRuleList::Include => "settings-scope-subdomains",
                    ScopeRuleList::Exclude => "settings-scope-exclude-subdomains",
                    ScopeRuleList::OutOfScopeCustom => "settings-scope-oos-custom-subdomains",
                };
                settings_table_checkbox_cell(
                    listeners_tls::indexed_checkbox(
                        (sub_id, row),
                        "",
                        rule.include_subdomains,
                        enabled,
                    )
                    .p_0()
                    .gap_0()
                    .accessibility_label(format!("包含子域 {}", rule.prefix))
                    .on_change(move |st, _, _, cx| {
                        let checked = st == CheckboxState::Checked;
                        let _ = wb.update(cx, |view, cx| {
                            view.set_scope_rule_subdomains(project, id, checked, cx)
                        });
                    }),
                )
                .into_any_element()
            }
        }
    }

    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        let owner = self.owner.upgrade()?;
        let settings = owner.read(cx);
        let rules = match self.kind {
            ScopeRuleList::Include => &settings.state.scope.include,
            ScopeRuleList::Exclude => &settings.state.scope.exclude,
            ScopeRuleList::OutOfScopeCustom => &settings.state.scope.out_of_scope_custom_rules,
        };
        let rule = rules.get(row)?;
        Some(match column {
            0 => if rule.enabled { "Yes" } else { "No" }.into(),
            1 => rule.prefix.clone().into(),
            _ => if rule.include_subdomains { "Yes" } else { "No" }.into(),
        })
    }
}

struct SimpleScopeTablesState {
    include: Entity<TableState<SimpleScopeTable>>,
    exclude: Entity<TableState<SimpleScopeTable>>,
    oos_custom: Entity<TableState<SimpleScopeTable>>,
}

impl Workbench {
    pub(in crate::workbench) fn add_scope_rule(
        &mut self,
        project: Option<ProjectKey>,
        include: bool,
        prefix: String,
        cx: &mut Context<Self>,
    ) {
        let _ = self.upsert_simple_scope_rule(project, include, None, prefix, cx);
    }

    pub(super) fn can_mutate_scope(&self, project: Option<ProjectKey>) -> bool {
        project.is_some()
            && self.project == project
            && self.backend_available()
            && self.transition.is_none()
    }

    pub(in crate::workbench) fn toggle_scope_rule(
        &mut self,
        project: Option<ProjectKey>,
        id: Uuid,
        cx: &mut Context<Self>,
    ) {
        if !self.can_mutate_scope(project) {
            return;
        }
        if let Some(rule) = self
            .workspace
            .scope
            .include
            .iter_mut()
            .chain(&mut self.workspace.scope.exclude)
            .chain(&mut self.workspace.scope.out_of_scope_custom_rules)
            .find(|rule| rule.id == id)
        {
            rule.enabled = !rule.enabled;
            self.save_workspace(cx);
            cx.notify();
        }
    }

    pub(in crate::workbench) fn set_scope_rule_subdomains(
        &mut self,
        project: Option<ProjectKey>,
        id: Uuid,
        include_subdomains: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.can_mutate_scope(project) {
            return;
        }
        if let Some(rule) = self
            .workspace
            .scope
            .include
            .iter_mut()
            .chain(&mut self.workspace.scope.exclude)
            .chain(&mut self.workspace.scope.out_of_scope_custom_rules)
            .find(|rule| rule.id == id)
        {
            if rule.include_subdomains == include_subdomains {
                return;
            }
            rule.include_subdomains = include_subdomains;
            self.save_workspace(cx);
            cx.notify();
        }
    }

    pub(in crate::workbench) fn update_scope_out_of_scope(
        &mut self,
        project: Option<ProjectKey>,
        mutate: impl FnOnce(&mut TargetScope),
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.can_mutate_scope(project) {
            return false;
        }
        let mut next = self.workspace.scope.clone();
        mutate(&mut next);
        if next == self.workspace.scope {
            return true;
        }
        if let Err(error) = next.validate() {
            self.fail(error.to_string(), cx);
            return false;
        }
        let previous = std::mem::replace(&mut self.workspace.scope, next);
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.scope = previous;
            self.fail(error.to_string(), cx);
            return false;
        }
        self.save_workspace(cx);
        self.error = None;
        cx.notify();
        true
    }

    pub(in crate::workbench) fn remove_scope_rule(
        &mut self,
        project: Option<ProjectKey>,
        id: Uuid,
        cx: &mut Context<Self>,
    ) {
        if !self.can_mutate_scope(project) {
            return;
        }
        let before = self.workspace.scope.include.len()
            + self.workspace.scope.exclude.len()
            + self.workspace.scope.out_of_scope_custom_rules.len();
        self.workspace.scope.include.retain(|rule| rule.id != id);
        self.workspace.scope.exclude.retain(|rule| rule.id != id);
        self.workspace
            .scope
            .out_of_scope_custom_rules
            .retain(|rule| rule.id != id);
        for slot in self
            .target_pane
            .editing_simple_scope
            .iter_mut()
            .chain(&mut self.target_pane.settings_editing_scope)
        {
            if *slot == Some(id) {
                *slot = None;
            }
        }
        if self.workspace.scope.include.len()
            + self.workspace.scope.exclude.len()
            + self.workspace.scope.out_of_scope_custom_rules.len()
            != before
        {
            self.save_workspace(cx);
            cx.notify();
        }
    }

    pub(in crate::workbench) fn reset_scope_rules(
        &mut self,
        project: Option<ProjectKey>,
        cx: &mut Context<Self>,
    ) {
        if !self.can_mutate_scope(project) || self.workspace.scope.is_empty() {
            return;
        }
        self.workspace.scope = TargetScope::default();
        self.target_pane.advanced_mode_input = false;
        self.target_pane.editing_simple_scope = [None, None];
        self.target_pane.editing_advanced_scope = [None, None];
        self.target_pane.settings_editing_scope = [None, None];
        self.save_workspace(cx);
        cx.notify();
    }
}

impl Settings {
    #[allow(dead_code)]
    fn validate_scope_ime(
        &self,
        adv: &Entity<SettingsAdvScopeState>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), &'static str> {
        editing::input_snapshot(&self.scope_include_input, window, cx)?;
        editing::input_snapshot(&self.scope_exclude_input, window, cx)?;
        let (inc, exc) = {
            let st = adv.read(cx);
            (st.include.clone(), st.exclude.clone())
        };
        for input in inc.iter().chain(&exc) {
            editing::input_snapshot(input, window, cx)?;
        }
        Ok(())
    }

    fn edit_scope_rule(
        &mut self,
        include: bool,
        id: Uuid,
        prefix: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = if include {
            self.scope_include_input.clone()
        } else {
            self.scope_exclude_input.clone()
        };
        if let Err(error) = editing::input_snapshot(&input, window, cx) {
            self.settings_error(error, cx);
            return;
        }
        let side = usize::from(!include);
        self.scope_editor_open[side] = true;
        let _ = self.owner.update(cx, |view, _| {
            view.target_pane.settings_editing_scope[side] = Some(id);
        });
        input.update(cx, |state, cx| {
            state.set_value(prefix, window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    #[allow(dead_code)]
    fn copy_scope_json(
        &mut self,
        adv: &Entity<SettingsAdvScopeState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.validate_scope_ime(adv, window, cx) {
            self.settings_error(error, cx);
            return;
        }
        match super::super::target::export_target_scope_json(&self.state.scope) {
            Ok(json) => {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(json));
                let _ = self.owner.update(cx, |view, cx| {
                    view.error = None;
                    view.status = "已复制 Target Scope (JSON) 到剪贴板。".into();
                    cx.notify();
                });
            }
            Err(error) => self.settings_error(error, cx),
        }
    }

    #[allow(dead_code)]
    fn paste_scope_json(
        &mut self,
        adv: &Entity<SettingsAdvScopeState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.enabled {
            return;
        }
        if let Err(error) = self.validate_scope_ime(adv, window, cx) {
            self.settings_error(error, cx);
            return;
        }
        let Some(raw) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.settings_error("剪贴板中没有可导入的 Scope JSON 文本。", cx);
            return;
        };
        let next = match super::super::target::parse_target_scope_json(&raw) {
            Ok(scope) => scope,
            Err(error) => {
                self.settings_error(error, cx);
                return;
            }
        };
        let project = self.state.project;
        let applied = self
            .owner
            .update(cx, |view, cx| {
                view.apply_imported_target_scope(project, next, cx)
            })
            .unwrap_or(false);
        if applied {
            self.scope_include_input
                .update(cx, |state, cx| state.set_value("", window, cx));
            self.scope_exclude_input
                .update(cx, |state, cx| state.set_value("", window, cx));
            adv.update(cx, |st, _| st.editing = [None, None]);
            let (inc, exc) = {
                let st = adv.read(cx);
                (st.include.clone(), st.exclude.clone())
            };
            for input in inc.iter().chain(&exc) {
                input.update(cx, |s, cx| s.set_value("", window, cx));
            }
        }
    }

    fn load_scope_rules_from_file(
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
                    "加载包含范围 URL 前缀列表".into()
                } else {
                    "加载排除范围 URL 前缀列表".into()
                }),
            },
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.into_iter().next()
                && let Ok(text) = std::fs::read_to_string(path)
            {
                let _ = this.update_in(cx, |this, _, cx| {
                    for line in text.lines() {
                        let trimmed = line.trim().to_owned();
                        if !trimmed.is_empty() {
                            let _ = this.owner.update(cx, |view, cx| {
                                view.add_scope_rule(project, include, trimmed, cx);
                            });
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn submit_scope_rule(&mut self, include: bool, window: &mut Window, cx: &mut Context<Self>) {
        let input = if include {
            self.scope_include_input.clone()
        } else {
            self.scope_exclude_input.clone()
        };
        let prefix = match editing::input_snapshot(&input, window, cx) {
            Ok(value) => value.trim().to_owned(),
            Err(error) => {
                self.settings_error(error, cx);
                return;
            }
        };
        if prefix.is_empty() {
            self.settings_error("Scope 规则前缀不能为空。", cx);
            return;
        }
        let project = self.state.project;
        let side = usize::from(!include);
        let saved = self
            .owner
            .update(cx, |view, cx| {
                let edit_id = view.target_pane.settings_editing_scope[side];
                let ok = view.upsert_simple_scope_rule(project, include, edit_id, prefix, cx);
                if ok {
                    view.target_pane.settings_editing_scope[side] = None;
                }
                ok
            })
            .unwrap_or(false);
        if saved {
            self.scope_editor_open[side] = false;
            input.update(cx, |state, cx| state.set_value("", window, cx));
            cx.notify();
        }
    }

    fn simple_scope_tables(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<SimpleScopeTablesState> {
        let weak = cx.weak_entity();
        window.use_keyed_state("settings-simple-scope-tables", cx, |window, cx| {
            let inc = cx.new(|tcx| {
                TableState::new(
                    SimpleScopeTable {
                        owner: weak.clone(),
                        kind: ScopeRuleList::Include,
                    },
                    window,
                    tcx,
                )
                .sortable(false)
                .col_selectable(false)
            });
            let exc = cx.new(|tcx| {
                TableState::new(
                    SimpleScopeTable {
                        owner: weak.clone(),
                        kind: ScopeRuleList::Exclude,
                    },
                    window,
                    tcx,
                )
                .sortable(false)
                .col_selectable(false)
            });
            let oos_custom = cx.new(|tcx| {
                TableState::new(
                    SimpleScopeTable {
                        owner: weak,
                        kind: ScopeRuleList::OutOfScopeCustom,
                    },
                    window,
                    tcx,
                )
                .sortable(false)
                .col_selectable(false)
            });
            SimpleScopeTablesState {
                include: inc,
                exclude: exc,
                oos_custom,
            }
        })
    }

    fn submit_oos_custom_scope_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prefix = match editing::input_snapshot(&self.scope_oos_custom_input, window, cx) {
            Ok(value) => value.trim().to_owned(),
            Err(error) => {
                self.settings_error(error, cx);
                return;
            }
        };
        if prefix.is_empty() || prefix.len() > 2048 || prefix.chars().any(char::is_control) {
            self.settings_error("自定义范围规则前缀不能为空、包含控制字符或超过 2048 字节。", cx);
            return;
        }
        let project = self.state.project;
        let edit_id = self.scope_oos_custom_editing;
        let include_subdomains = self.scope_oos_custom_subdomains;
        let saved = self
            .owner
            .update(cx, |view, cx| {
                view.update_scope_out_of_scope(
                    project,
                    |scope| {
                        if let Some(id) = edit_id
                            && let Some(existing) = scope
                                .out_of_scope_custom_rules
                                .iter_mut()
                                .find(|r| r.id == id)
                        {
                            existing.prefix = prefix;
                            existing.include_subdomains = include_subdomains;
                        } else if scope.out_of_scope_custom_rules.len() < 256 {
                            scope.out_of_scope_custom_rules.push(mio_core::ScopeRule {
                                id: Uuid::new_v4(),
                                enabled: true,
                                prefix,
                                include_subdomains,
                            });
                        }
                    },
                    cx,
                )
            })
            .unwrap_or(false);
        if saved {
            self.scope_oos_custom_editor_open = false;
            self.scope_oos_custom_editing = None;
            self.scope_oos_custom_subdomains = false;
            self.scope_oos_custom_input
                .update(cx, |state, cx| state.set_value("", window, cx));
            cx.notify();
        }
    }

    fn load_oos_custom_scope_rules_from_file(
        &mut self,
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
                prompt: Some("加载自定义范围外判定 URL 前缀列表".into()),
            },
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.into_iter().next()
            {
                let entries = cx
                    .background_executor()
                    .spawn(async move {
                        let meta = std::fs::metadata(&path).ok()?;
                        if !meta.is_file() || meta.len() > 256 * 1024 {
                            return None;
                        }
                        let text = std::fs::read_to_string(path).ok()?;
                        Some(
                            text.lines()
                                .map(str::trim)
                                .filter(|line| {
                                    !line.is_empty()
                                        && line.len() <= 2048
                                        && !line.chars().any(char::is_control)
                                })
                                .map(str::to_owned)
                                .collect::<Vec<_>>(),
                        )
                    })
                    .await;
                if let Some(entries) = entries
                    && !entries.is_empty()
                {
                    let _ = this.update_in(cx, |this, _, cx| {
                        let _ = this.owner.update(cx, |view, cx| {
                            view.update_scope_out_of_scope(
                                project,
                                |scope| {
                                    for prefix in entries {
                                        if scope.out_of_scope_custom_rules.len() >= 256 {
                                            break;
                                        }
                                        scope.out_of_scope_custom_rules.push(mio_core::ScopeRule {
                                            id: Uuid::new_v4(),
                                            enabled: true,
                                            prefix,
                                            include_subdomains: false,
                                        });
                                    }
                                },
                                cx,
                            );
                        });
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    pub(in crate::workbench) fn render_scope(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let enabled = self.state.enabled;
        let project = self.state.project;
        let advanced = self.state.scope.advanced_mode;
        let weak = cx.weak_entity();
        let adv_state = window.use_keyed_state("settings-adv-scope-state", cx, |window, cx| {
            SettingsAdvScopeState::new(weak, window, cx)
        });
        let adv_reset = adv_state.clone();
        let header = settings_section_heading_plain("目标范围", project_scope_badge());
        let desc_row = settings_desc_row_with_action(
            settings_gear_icon_button(
                "settings-scope-reset",
                "清空全部包含与排除范围规则并恢复为简单前缀模式",
                !enabled,
            )
            .tab_stop(false)
            .on_click(cx.listener(move |this, _, _, cx| {
                adv_reset.update(cx, |st, _| st.editing = [None, None]);
                let _ = this
                    .owner
                    .update(cx, |view, cx| view.reset_scope_rules(project, cx));
            })),
            "定义当前工作构成目标的主机与 URL 规则，该配置会影响整个套件中工具的目标范围判断。",
        );
        let owner_adv = self.owner.clone();
        let actions = div().pl(px(31.)).child(
            listeners_tls::indexed_checkbox(
                "settings-scope-advanced-toggle",
                "使用高级正则范围控制",
                advanced,
                enabled,
            )
            .on_change(move |_, _, _, cx| {
                let _ = owner_adv.update(cx, |view, cx| view.toggle_target_advanced_mode(cx));
            }),
        );

        let drop_oos = self.state.scope.drop_out_of_scope;
        let use_custom_oos = self.state.scope.out_of_scope_use_custom;
        let o_drop = self.owner.clone();
        let o_suite = self.owner.clone();
        let o_custom = self.owner.clone();
        let oos_enabled = enabled && drop_oos;
        let out_of_scope_section = div()
            .mt(px(4.))
            .pt(px(16.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(9.))
            .child(settings_section_heading(
                "settings-scope-oos-heading",
                "范围外请求处理",
                project_scope_badge(),
            ))
            .child(settings_desc_row("定义如何处理任意超出目标范围的请求。"))
            .child(
                div()
                    .pl(px(31.))
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(4.))
                    .child(
                        listeners_tls::indexed_checkbox(
                            "settings-scope-drop-oos",
                            "丢弃所有范围外请求",
                            drop_oos,
                            enabled,
                        )
                        .tab_stop(false)
                        .on_change(move |st, _, _, cx| {
                            let checked = st == CheckboxState::Checked;
                            let _ = o_drop.update(cx, |view, cx| {
                                view.update_scope_out_of_scope(
                                    project,
                                    |s| s.drop_out_of_scope = checked,
                                    cx,
                                );
                            });
                        }),
                    )
                    .child(
                        div()
                            .id("settings-scope-oos-radios")
                            .test_support()
                            .role(Role::RadioGroup)
                            .aria_label("范围外请求判定范围")
                            .pl(px(22.))
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap(px(2.))
                            .child(
                                settings_radio(
                                    "settings-scope-oos-suite",
                                    "使用套件范围 [上方定义]",
                                    !use_custom_oos,
                                    oos_enabled,
                                    window,
                                    cx,
                                )
                                .tab_stop(false)
                                .on_change(move |_, _, _, cx| {
                                    let _ = o_suite.update(cx, |view, cx| {
                                        view.update_scope_out_of_scope(
                                            project,
                                            |s| s.out_of_scope_use_custom = false,
                                            cx,
                                        );
                                    });
                                }),
                            )
                            .child(
                                settings_radio(
                                    "settings-scope-oos-custom",
                                    "使用自定义范围",
                                    use_custom_oos,
                                    oos_enabled,
                                    window,
                                    cx,
                                )
                                .tab_stop(false)
                                .on_change(move |_, _, _, cx| {
                                    let _ = o_custom.update(cx, |view, cx| {
                                        view.update_scope_out_of_scope(
                                            project,
                                            |s| s.out_of_scope_use_custom = true,
                                            cx,
                                        );
                                    });
                                }),
                            ),
                    )
                    .when(drop_oos && use_custom_oos, |this| {
                        this.child(
                            div()
                                .w_full()
                                .pl(px(22.))
                                .pt(px(6.))
                                .child(self.render_oos_custom_scope_card(
                                    enabled, project, window, cx,
                                )),
                        )
                    }),
            );

        let content = if advanced {
            div()
                .px(px(22.))
                .py(px(18.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(header)
                .child(desc_row)
                .child(actions)
                .child(
                    div()
                        .pl(px(31.))
                        .child(self.render_settings_advanced_scope(enabled, &adv_state, cx)),
                )
                .child(out_of_scope_section)
        } else {
            div()
                .px(px(22.))
                .py(px(18.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(header)
                .child(desc_row)
                .child(actions)
                .child(
                    div()
                        .pl(px(31.))
                        .flex()
                        .flex_col()
                        .gap(px(14.))
                        .child(self.render_simple_scope_card(true, enabled, project, window, cx))
                        .child(self.render_simple_scope_card(false, enabled, project, window, cx)),
                )
                .child(out_of_scope_section)
        };
        settings_scrollable_pane("settings-scope-page", &self.content_scroll, content)
            .into_any_element()
    }

    fn render_oos_custom_scope_card(
        &self,
        enabled: bool,
        project: Option<ProjectKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tables_state = self.simple_scope_tables(window, cx);
        let table = tables_state.read(cx).oos_custom.clone();
        let rules = &self.state.scope.out_of_scope_custom_rules;
        let count = rules.len();
        let table_refresh = table.clone();
        cx.defer(move |cx| {
            table_refresh.update(cx, |state, cx| {
                if state.selected_row().is_some_and(|row| row >= count) {
                    state.clear_selection(cx);
                }
                state.refresh(cx);
            });
        });
        let selected_row = table
            .read(cx)
            .selected_row()
            .filter(|&row| row < rules.len());
        let selected_rule = selected_row.and_then(|row| rules.get(row).cloned());
        let selected_for_edit = selected_rule.clone();
        let selected_for_remove = selected_rule.map(|rule| rule.id);
        let e_sub = cx.entity();

        let side_buttons = div()
            .w(theme::scaled_px(88.).max(px(84.)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(5.))
            .child(
                settings_action_button("settings-scope-oos-custom-add", "添加", !enabled)
                    .w_full()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.scope_oos_custom_editor_open = true;
                        this.scope_oos_custom_editing = None;
                        this.scope_oos_custom_subdomains = false;
                        this.scope_oos_custom_input.update(cx, |s, cx| {
                            s.set_value("", window, cx);
                            s.focus(window, cx);
                        });
                        cx.notify();
                    })),
            )
            .child(
                settings_action_button(
                    "settings-scope-oos-custom-edit",
                    "编辑",
                    !enabled || selected_for_edit.is_none(),
                )
                .w_full()
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(rule) = &selected_for_edit {
                        this.scope_oos_custom_editor_open = true;
                        this.scope_oos_custom_editing = Some(rule.id);
                        this.scope_oos_custom_subdomains = rule.include_subdomains;
                        this.scope_oos_custom_input.update(cx, |s, cx| {
                            s.set_value(rule.prefix.clone(), window, cx);
                            s.focus(window, cx);
                        });
                        cx.notify();
                    }
                })),
            )
            .child(
                settings_action_button(
                    "settings-scope-oos-custom-remove",
                    "移除",
                    !enabled || selected_for_remove.is_none(),
                )
                .w_full()
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(id) = selected_for_remove {
                        if this.scope_oos_custom_editing == Some(id) {
                            this.scope_oos_custom_editing = None;
                            this.scope_oos_custom_editor_open = false;
                        }
                        let _ = this.owner.update(cx, |view, cx| {
                            view.update_scope_out_of_scope(
                                project,
                                |s| s.out_of_scope_custom_rules.retain(|r| r.id != id),
                                cx,
                            );
                        });
                    }
                })),
            )
            .child(
                settings_action_button("settings-scope-oos-custom-paste", "粘贴 URL", !enabled)
                    .w_full()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                            let trimmed = text.trim().to_owned();
                            if !trimmed.is_empty()
                                && trimmed.len() <= 2048
                                && !trimmed.chars().any(char::is_control)
                            {
                                let _ = this.owner.update(cx, |view, cx| {
                                    view.update_scope_out_of_scope(
                                        project,
                                        |s| {
                                            if s.out_of_scope_custom_rules.len() < 256 {
                                                s.out_of_scope_custom_rules.push(
                                                    mio_core::ScopeRule {
                                                        id: Uuid::new_v4(),
                                                        enabled: true,
                                                        prefix: trimmed,
                                                        include_subdomains: false,
                                                    },
                                                );
                                            }
                                        },
                                        cx,
                                    );
                                });
                            }
                        }
                    })),
            )
            .child(
                settings_action_button("settings-scope-oos-custom-load", "加载...", !enabled)
                    .w_full()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.load_oos_custom_scope_rules_from_file(window, cx)
                    })),
            );

        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .gap(px(9.))
                    .h(theme::scaled_px(136.).max(px(136.)))
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
                                .accessibility_label("自定义范围外判定规则"),
                            true,
                        )
                        .max_w(theme::scaled_px(645.).max(px(645.))),
                    ),
            )
            .when(self.scope_oos_custom_editor_open, |this| {
                this.child(
                    div()
                        .pl(theme::scaled_px(97.).max(px(93.)))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div().w(px(320.)).child(
                                Input::new(&self.scope_oos_custom_input)
                                    .id("settings-scope-oos-custom-input")
                                    .aria_label("自定义范围规则前缀")
                                    .small()
                                    .disabled(!enabled),
                            ),
                        )
                        .child(
                            listeners_tls::indexed_checkbox(
                                "settings-scope-oos-custom-sub-cb",
                                "Include subdomains",
                                self.scope_oos_custom_subdomains,
                                enabled,
                            )
                            .on_change(move |st, _, _, cx| {
                                e_sub.update(cx, |this, cx| {
                                    this.scope_oos_custom_subdomains =
                                        st == CheckboxState::Checked;
                                    cx.notify();
                                });
                            }),
                        )
                        .child(
                            settings_action_button(
                                "settings-scope-oos-custom-confirm",
                                "确定",
                                !enabled,
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_oos_custom_scope_rule(window, cx)
                            })),
                        )
                        .child(
                            settings_action_button(
                                "settings-scope-oos-custom-cancel",
                                "取消",
                                false,
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.scope_oos_custom_editor_open = false;
                                this.scope_oos_custom_editing = None;
                                this.scope_oos_custom_subdomains = false;
                                this.scope_oos_custom_input
                                    .update(cx, |s, cx| s.set_value("", window, cx));
                                cx.notify();
                            })),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_simple_scope_card(
        &self,
        include: bool,
        enabled: bool,
        project: Option<ProjectKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tables_state = self.simple_scope_tables(window, cx);
        let table = if include {
            tables_state.read(cx).include.clone()
        } else {
            tables_state.read(cx).exclude.clone()
        };
        let side = usize::from(!include);
        let (title, input_id, aria, add_id, add_lbl, edit_pfx, rm_pfx, paste_id, load_id) =
            SIMPLE_META[side];
        let (input, rules) = if include {
            (&self.scope_include_input, &self.state.scope.include)
        } else {
            (&self.scope_exclude_input, &self.state.scope.exclude)
        };
        let count = rules.len();
        let table_refresh = table.clone();
        cx.defer(move |cx| {
            table_refresh.update(cx, |state, cx| {
                if state.selected_row().is_some_and(|row| row >= count) {
                    state.clear_selection(cx);
                }
                state.refresh(cx);
            });
        });
        let selected_row = table
            .read(cx)
            .selected_row()
            .filter(|&row| row < rules.len());
        let selected_rule = selected_row.and_then(|row| rules.get(row).cloned());
        let selected_for_edit = selected_rule.clone();
        let selected_for_remove = selected_rule.map(|rule| rule.id);
        let editor_open = self.scope_editor_open[side];
        let side_buttons = div()
            .w(theme::scaled_px(88.).max(px(84.)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(5.))
            .child(
                settings_action_button(add_id, add_lbl, !enabled)
                    .w_full()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let input = if include {
                            this.scope_include_input.clone()
                        } else {
                            this.scope_exclude_input.clone()
                        };
                        if !this.scope_editor_open[side] {
                            this.scope_editor_open[side] = true;
                            let _ = this.owner.update(cx, |view, _| {
                                view.target_pane.settings_editing_scope[side] = None;
                            });
                            input.update(cx, |state, cx| {
                                state.set_value("", window, cx);
                                state.focus(window, cx);
                            });
                            cx.notify();
                        } else {
                            this.submit_scope_rule(include, window, cx);
                        }
                    })),
            )
            .child(
                settings_action_button(
                    edit_pfx,
                    "编辑",
                    !enabled || selected_for_edit.is_none(),
                )
                .w_full()
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(rule) = &selected_for_edit {
                        this.edit_scope_rule(
                            include,
                            rule.id,
                            rule.prefix.clone(),
                            window,
                            cx,
                        );
                    }
                })),
            )
            .child(
                settings_action_button(
                    rm_pfx,
                    "移除",
                    !enabled || selected_for_remove.is_none(),
                )
                .w_full()
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(id) = selected_for_remove {
                        let _ = this
                            .owner
                            .update(cx, |view, cx| view.remove_scope_rule(project, id, cx));
                    }
                })),
            )
            .child(
                settings_action_button(paste_id, "粘贴 URL", !enabled)
                    .w_full()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                            let trimmed = text.trim().to_owned();
                            if !trimmed.is_empty() {
                                let _ = this.owner.update(cx, |view, cx| {
                                    view.add_scope_rule(project, include, trimmed, cx);
                                });
                            }
                        }
                    })),
            )
            .child(
                settings_action_button(load_id, "加载...", !enabled)
                    .w_full()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.load_scope_rules_from_file(include, window, cx)
                    })),
            );
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(div().font_weight(gpui_kit::FontWeight::MEDIUM).child(title))
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
            .when(editor_open, |this| {
                this.child(
                    div()
                        .pl(theme::scaled_px(97.).max(px(93.)))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div().flex_1().max_w(px(420.)).child(
                                Input::new(input)
                                    .id(input_id)
                                    .aria_label(aria)
                                    .small()
                                    .disabled(!enabled),
                            ),
                        )
                        .child(
                            settings_action_button(
                                if include {
                                    "settings-scope-include-confirm"
                                } else {
                                    "settings-scope-exclude-confirm"
                                },
                                "确定",
                                !enabled,
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.submit_scope_rule(include, window, cx)
                            })),
                        )
                        .child(
                            settings_action_button(
                                if include {
                                    "settings-scope-include-cancel"
                                } else {
                                    "settings-scope-exclude-cancel"
                                },
                                "取消",
                                false,
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.scope_editor_open[side] = false;
                                let _ = this.owner.update(cx, |view, _| {
                                    view.target_pane.settings_editing_scope[side] = None;
                                });
                                let inp = if include {
                                    &this.scope_include_input
                                } else {
                                    &this.scope_exclude_input
                                };
                                inp.update(cx, |s, cx| s.set_value("", window, cx));
                                cx.notify();
                            })),
                        ),
                )
            })
            .into_any_element()
    }
}
