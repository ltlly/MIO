use std::rc::Rc;

use gpui_kit::assets::IconName as LucideIcon;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::{Global, RenderOnce};
use mio_core::RepeaterRedirectMode;

use super::*;

#[derive(Default)]
pub(in crate::workbench::intruder) struct IntruderAttackSettingsInputs {
    pub(super) retries: HashMap<TabId, Entity<InputState>>,
    pub(super) retry_pause: HashMap<TabId, Entity<InputState>>,
}

impl Global for IntruderAttackSettingsInputs {}

type SettingsWindowRender = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

#[derive(IntoElement)]
struct SettingsWithWindow(SettingsWindowRender);

impl RenderOnce for SettingsWithWindow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        (self.0)(window, cx)
    }
}

pub(in crate::workbench::intruder) fn ensure_attack_retries_input(
    id: TabId,
    default_retries: u8,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InputState> {
    if let Some(existing) = cx
        .try_global::<IntruderAttackSettingsInputs>()
        .and_then(|inputs| inputs.retries.get(&id).cloned())
    {
        return existing;
    }
    let initial = default_retries.to_string();
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder("0-10")
            .default_value(initial)
    });
    let mut state = cx
        .try_global::<IntruderAttackSettingsInputs>()
        .map(|s| IntruderAttackSettingsInputs {
            retries: s.retries.clone(),
            retry_pause: s.retry_pause.clone(),
        })
        .unwrap_or_default();
    state.retries.insert(id, input.clone());
    cx.set_global(state);
    input
}

pub(in crate::workbench::intruder) fn ensure_attack_retry_pause_input(
    id: TabId,
    default_pause_ms: u64,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InputState> {
    if let Some(existing) = cx
        .try_global::<IntruderAttackSettingsInputs>()
        .and_then(|inputs| inputs.retry_pause.get(&id).cloned())
    {
        return existing;
    }
    let initial = default_pause_ms.to_string();
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder("暂停毫秒 (如 2000)")
            .default_value(initial)
    });
    let mut state = cx
        .try_global::<IntruderAttackSettingsInputs>()
        .map(|s| IntruderAttackSettingsInputs {
            retries: s.retries.clone(),
            retry_pause: s.retry_pause.clone(),
        })
        .unwrap_or_default();
    state.retry_pause.insert(id, input.clone());
    cx.set_global(state);
    input
}

impl Workbench {
    #[inline(never)]
    fn render_intruder_config_save_section(
        &self,
        id: TabId,
        spec: &IntruderSpec,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let project = self.project;
        let revision = spec.revision;
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_size(theme::scaled_px(13.))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("保存攻击"),
            )
            .child(
                div()
                    .text_size(theme::scaled_px(12.))
                    .text_color(theme::muted_foreground())
                    .child("将攻击保存到当前项目文件，重开项目后可从 Dashboard 访问。"),
            )
            .child(
                Checkbox::new("intruder-config-save-to-project")
                    .label("保存攻击到项目文件")
                    .checked(spec.save_to_project)
                    .disabled(!enabled)
                    .on_click(cx.listener(move |this, value, window, cx| {
                        if this.project != project
                            || this.project_path.is_none()
                            || !this
                                .workspace
                                .intruder
                                .tabs
                                .iter()
                                .any(|tab| tab.id == id && tab.spec.revision == revision)
                        {
                            return;
                        }
                        this.change_intruder_spec(
                            id,
                            |spec| spec.save_to_project = *value,
                            window,
                            cx,
                        );
                    })),
            )
            .child(
                div()
                    .text_size(theme::scaled_px(11.5))
                    .text_color(theme::muted_foreground())
                    .child("（仅适用于磁盘项目）"),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_intruder_config_attack_results_section(
        &self,
        id: TabId,
        spec: &IntruderSpec,
        editors: &IntruderEditors,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let project = self.project;
        let revision = spec.revision;
        div()
            .border_t_1()
            .border_color(theme::border())
            .pt(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        Button::new("intruder-attack-results-defaults")
                            .small()
                            .ghost()
                            .icon(IconName::Undo2)
                            .accessibility_label("恢复 Attack results 默认设置")
                            .tooltip("恢复 Attack results 为默认配置")
                            .disabled(
                                !editable
                                    || !self.backend_available()
                                    || editors.baseline_restore.is_some(),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.restore_intruder_attack_results(id, window, cx)
                            })),
                    )
                    .child(
                        div()
                            .text_size(theme::scaled_px(13.))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child("Attack results"),
                    ),
            )
            .child(
                Checkbox::new("intruder-config-baseline")
                    .label("发送未修改的基准请求")
                    .checked(spec.baseline_request)
                    .disabled(!editable || !self.backend_available())
                    .on_click(cx.listener(move |this, value, window, cx| {
                        if this.project != project
                            || !this
                                .workspace
                                .intruder
                                .tabs
                                .iter()
                                .any(|tab| tab.id == id && tab.spec.revision == revision)
                        {
                            return;
                        }
                        this.change_intruder_spec(
                            id,
                            |spec| spec.baseline_request = *value,
                            window,
                            cx,
                        );
                    })),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_intruder_config_grep_match_section(
        &self,
        id: TabId,
        match_expressions: &[String],
        match_case_sensitive: bool,
        editors: &IntruderEditors,
        grep_editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut match_rows = div().flex().flex_col().gap(px(4.));
        for (idx, expr) in match_expressions.iter().enumerate() {
            match_rows = match_rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(3.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::toolbar_surface())
                    .child(div().flex_1().min_w_0().truncate().child(expr.clone()))
                    .child(
                        button(("intruder-grep-match-remove", idx), "删除")
                            .icon(LucideIcon::Trash)
                            .tooltip("移除此 Grep 匹配规则")
                            .disabled(!grep_editable)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if !this.flush_workspace(window, cx) {
                                    return;
                                }
                                if let Some(cfg) = this.workspace.intruder.grep_configs.get_mut(&id)
                                    && idx < cfg.match_expressions.len()
                                {
                                    cfg.match_expressions.remove(idx);
                                    if cfg.is_empty() {
                                        this.workspace.intruder.grep_configs.remove(&id);
                                    }
                                    this.refresh_intruder_grep_summaries(cx);
                                    this.save_workspace(cx);
                                    cx.notify();
                                }
                            })),
                    ),
            );
        }
        div()
            .border_t_1()
            .border_color(theme::border())
            .pt(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .text_size(theme::scaled_px(13.))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("Grep - Match"),
            )
            .child(
                div()
                    .text_size(theme::scaled_px(12.))
                    .text_color(theme::muted_foreground())
                    .child(
                        "支持字面匹配或以 re: 前缀编写正则表达式；响应详情同步对比载荷回显 (Grep - Payloads)。",
                    ),
            )
            .child(
                Checkbox::new("intruder-grep-match-case")
                    .label("匹配区分大小写")
                    .checked(match_case_sensitive)
                    .disabled(!grep_editable)
                    .on_click(cx.listener(move |this, value, window, cx| {
                        if !this.flush_workspace(window, cx) {
                            return;
                        }
                        let cfg = this
                            .workspace
                            .intruder
                            .grep_configs
                            .entry(id)
                            .or_default();
                        cfg.match_case_sensitive = *value;
                        if cfg.is_empty() {
                            this.workspace.intruder.grep_configs.remove(&id);
                        }
                        this.refresh_intruder_grep_summaries(cx);
                        this.save_workspace(cx);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&editors.grep_match_input)
                                .id("intruder-grep-match-input")
                                .aria_label("Grep Match 表达式")
                                .small()
                                .disabled(!grep_editable),
                        ),
                    )
                    .child(
                        button("intruder-grep-match-add", "添加匹配")
                            .icon(LucideIcon::Plus)
                            .tooltip("添加响应匹配表达式（支持 re: 正则前缀）")
                            .disabled(!grep_editable)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.add_intruder_grep_match(id, window, cx)
                            })),
                    )
                    .child(
                        button("intruder-grep-match-clear", "清空匹配")
                            .icon(LucideIcon::Eraser)
                            .tooltip("清空所有 Grep - Match 匹配规则")
                            .disabled(!grep_editable)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if !this.flush_workspace(window, cx) {
                                    return;
                                }
                                if let Some(cfg) =
                                    this.workspace.intruder.grep_configs.get_mut(&id)
                                {
                                    cfg.match_expressions.clear();
                                    if cfg.is_empty() {
                                        this.workspace.intruder.grep_configs.remove(&id);
                                    }
                                    this.refresh_intruder_grep_summaries(cx);
                                    this.save_workspace(cx);
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .child(match_rows)
            .into_any_element()
    }

    #[inline(never)]
    fn render_intruder_config_grep_extract_section(
        &self,
        id: TabId,
        extract_labels: &[String],
        editors: &IntruderEditors,
        grep_editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut extract_rows = div().flex().flex_col().gap(px(4.));
        for (idx, rule_label) in extract_labels.iter().enumerate() {
            extract_rows = extract_rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(3.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::toolbar_surface())
                    .child(div().flex_1().min_w_0().truncate().child(rule_label.clone()))
                    .child(
                        button(("intruder-grep-extract-remove", idx), "删除")
                            .icon(LucideIcon::Trash)
                            .tooltip("移除此 Grep 提取规则")
                            .disabled(!grep_editable)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if !this.flush_workspace(window, cx) {
                                    return;
                                }
                                if let Some(cfg) = this.workspace.intruder.grep_configs.get_mut(&id)
                                    && idx < cfg.extract_rules.len()
                                {
                                    cfg.extract_rules.remove(idx);
                                    if cfg.is_empty() {
                                        this.workspace.intruder.grep_configs.remove(&id);
                                    }
                                    this.refresh_intruder_grep_summaries(cx);
                                    this.save_workspace(cx);
                                    cx.notify();
                                }
                            })),
                    ),
            );
        }
        div()
            .border_t_1()
            .border_color(theme::border())
            .pt(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(theme::scaled_px(13.))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child("Grep - Extract"),
                    )
                    .child(
                        button(
                            "intruder-grep-extract-pick-selection",
                            "从选区拾取分隔符",
                        )
                        .icon(LucideIcon::ScanSearch)
                        .tooltip(
                            "从请求/响应选区自动拾取前后起止分隔符（支持 re: 正则捕获组提取）",
                        )
                        .disabled(!grep_editable)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.pick_intruder_grep_extract_delimiters(id, window, cx)
                        })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&editors.grep_extract_start)
                                .id("intruder-grep-extract-start")
                                .aria_label("起始分隔符")
                                .small()
                                .disabled(!grep_editable),
                        ),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&editors.grep_extract_end)
                                .id("intruder-grep-extract-end")
                                .aria_label("结束分隔符")
                                .small()
                                .disabled(!grep_editable),
                        ),
                    )
                    .child(
                        button("intruder-grep-extract-add", "添加提取")
                            .icon(LucideIcon::Plus)
                            .tooltip("根据起止分隔符添加响应字段提取规则")
                            .disabled(!grep_editable)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.add_intruder_grep_extract(id, window, cx)
                            })),
                    )
                    .child(
                        button("intruder-grep-extract-clear", "清空提取")
                            .icon(LucideIcon::Eraser)
                            .tooltip("清空所有 Grep - Extract 提取规则")
                            .disabled(!grep_editable)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if !this.flush_workspace(window, cx) {
                                    return;
                                }
                                if let Some(cfg) =
                                    this.workspace.intruder.grep_configs.get_mut(&id)
                                {
                                    cfg.extract_rules.clear();
                                    if cfg.is_empty() {
                                        this.workspace.intruder.grep_configs.remove(&id);
                                    }
                                    this.refresh_intruder_grep_summaries(cx);
                                    this.save_workspace(cx);
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .child(extract_rows)
            .into_any_element()
    }

    #[inline(never)]
    fn render_intruder_config_grep_payloads_section(
        &self,
        id: TabId,
        grep_payloads: bool,
        grep_payloads_case_sensitive: bool,
        grep_editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .border_t_1()
            .border_color(theme::border())
            .pt(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .text_size(theme::scaled_px(13.))
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("Grep - Payloads"),
            )
            .child(
                div()
                    .text_size(theme::scaled_px(12.))
                    .text_color(theme::muted_foreground())
                    .child("自动搜索响应报文中的当前请求载荷回显，并在结果列表中标记命中列。"),
            )
            .child(
                Checkbox::new("intruder-grep-payloads-enabled")
                    .label("搜索响应中的载荷回显 (Grep - Payloads)")
                    .checked(grep_payloads)
                    .disabled(!grep_editable)
                    .on_click(cx.listener(move |this, value, window, cx| {
                        if !this.flush_workspace(window, cx) {
                            return;
                        }
                        let cfg = this
                            .workspace
                            .intruder
                            .grep_configs
                            .entry(id)
                            .or_default();
                        cfg.grep_payloads = *value;
                        if cfg.is_empty() {
                            this.workspace.intruder.grep_configs.remove(&id);
                        }
                        this.refresh_intruder_grep_summaries(cx);
                        this.save_workspace(cx);
                        cx.notify();
                    })),
            )
            .child(
                Checkbox::new("intruder-grep-payloads-case")
                    .label("载荷回显区分大小写")
                    .checked(grep_payloads_case_sensitive)
                    .disabled(!grep_editable)
                    .on_click(cx.listener(move |this, value, window, cx| {
                        if !this.flush_workspace(window, cx) {
                            return;
                        }
                        let cfg = this
                            .workspace
                            .intruder
                            .grep_configs
                            .entry(id)
                            .or_default();
                        cfg.grep_payloads_case_sensitive = *value;
                        if cfg.is_empty() {
                            this.workspace.intruder.grep_configs.remove(&id);
                        }
                        this.refresh_intruder_grep_summaries(cx);
                        this.save_workspace(cx);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_intruder_config_settings(
        &self,
        id: TabId,
        spec: &IntruderSpec,
        editors: &IntruderEditors,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let enabled = editable && self.backend_available() && self.project_path.is_some();
        let grep_editable = editable && self.backend_available();
        let grep_config = self
            .workspace
            .intruder
            .grep_configs
            .get(&id)
            .cloned()
            .unwrap_or_default();
        let attack_settings = self
            .workspace
            .intruder
            .attack_settings
            .get(&id)
            .cloned()
            .unwrap_or_default();
        let extract_labels: Vec<String> = grep_config
            .extract_rules
            .iter()
            .map(|rule| {
                if rule.regex_pattern().is_some() {
                    rule.key_label()
                } else {
                    format!("{} .. {}", rule.start_delimiter, rule.end_delimiter)
                }
            })
            .collect();
        let side_panel_left = self.intruder_user_misc().intruder_side_panel_left;
        let save_section = self.render_intruder_config_save_section(id, spec, enabled, cx);
        let attack_results_section =
            self.render_intruder_config_attack_results_section(id, spec, editors, editable, cx);
        let grep_match_section = self.render_intruder_config_grep_match_section(
            id,
            &grep_config.match_expressions,
            grep_config.match_case_sensitive,
            editors,
            grep_editable,
            cx,
        );
        let grep_extract_section = self.render_intruder_config_grep_extract_section(
            id,
            &extract_labels,
            editors,
            grep_editable,
            cx,
        );
        let grep_payloads_section = self.render_intruder_config_grep_payloads_section(
            id,
            grep_config.grep_payloads,
            grep_config.grep_payloads_case_sensitive,
            grep_editable,
            cx,
        );
        let engine_section = self.render_intruder_request_engine_and_redirections(
            id,
            &attack_settings,
            grep_editable,
            cx,
        );
        div()
            .id("intruder-config-settings")
            .test_support()
            .w(px(420.))
            .min_w(px(280.))
            .max_w(px(480.))
            .flex_shrink(1.0)
            .h_full()
            .bg(theme::surface())
            .when(side_panel_left, |d| d.border_r_1())
            .when(!side_panel_left, |d| d.border_l_1())
            .border_color(theme::border())
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(14.))
                    .py(px(10.))
                    .flex_shrink_0()
                    .bg(theme::toolbar_surface())
                    .border_b_1()
                    .border_color(theme::border())
                    .text_size(theme::scaled_px(13.5))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .child("Settings"),
            )
            .child(
                div()
                    .id("intruder-config-settings-body")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(14.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(save_section)
                    .child(attack_results_section)
                    .child(grep_match_section)
                    .child(grep_extract_section)
                    .child(grep_payloads_section)
                    .child(engine_section),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_intruder_request_engine_and_redirections(
        &self,
        id: TabId,
        attack_settings: &mio_core::IntruderAttackSettings,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current_retries = attack_settings.network_failure_retries;
        let current_pause = attack_settings.retry_pause_ms;
        let current_redirect = attack_settings.follow_redirects;
        let retries_weak = cx.weak_entity();
        let pause_weak = cx.weak_entity();
        let retries_dropdown = button(
            "intruder-settings-retries",
            format!("网络失败重试: {current_retries} 次"),
        )
        .dropdown_caret(true)
        .disabled(!editable)
        .tooltip("配置单次请求遇到网络错误时的自动重试次数（0–10 次）")
        .dropdown_menu(move |menu, _, _| {
            (0u8..=10).fold(menu, |menu, count| {
                let weak = retries_weak.clone();
                menu.item(
                    PopupMenuItem::new(format!("{count} 次"))
                        .checked(count == current_retries)
                        .on_click(move |_, window, cx| {
                            let _ = weak.update(cx, |view, cx| {
                                view.update_intruder_attack_settings(
                                    id,
                                    |s| s.network_failure_retries = count,
                                    window,
                                    cx,
                                );
                            });
                        }),
                )
            })
        });
        let pause_dropdown = button(
            "intruder-settings-retry-pause",
            format!("重试暂停: {current_pause} ms"),
        )
        .dropdown_caret(true)
        .disabled(!editable)
        .tooltip("配置网络失败重试前的等待毫秒数")
        .dropdown_menu(move |menu, _, _| {
            [0u64, 200, 500, 1000, 2000, 5000]
                .into_iter()
                .fold(menu, |menu, pause_ms| {
                    let weak = pause_weak.clone();
                    menu.item(
                        PopupMenuItem::new(format!("{pause_ms} ms"))
                            .checked(pause_ms == current_pause)
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |view, cx| {
                                    view.update_intruder_attack_settings(
                                        id,
                                        |s| s.retry_pause_ms = pause_ms,
                                        window,
                                        cx,
                                    );
                                });
                            }),
                    )
                })
        });
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .border_t_1()
                    .border_color(theme::border())
                    .pt(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(theme::scaled_px(13.))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child("Request Engine / HTTP Headers"),
                    )
                    .child(
                        Checkbox::new("intruder-settings-update-content-length")
                            .label("Update Content-Length header")
                            .checked(attack_settings.update_content_length)
                            .disabled(!editable)
                            .on_click(cx.listener(move |this, value, window, cx| {
                                this.update_intruder_attack_settings(
                                    id,
                                    |s| s.update_content_length = *value,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        Checkbox::new("intruder-settings-connection-close")
                            .label("Set Connection: close")
                            .checked(attack_settings.set_connection_close)
                            .disabled(!editable)
                            .on_click(cx.listener(move |this, value, window, cx| {
                                this.update_intruder_attack_settings(
                                    id,
                                    |s| s.set_connection_close = *value,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.))
                            .child(retries_dropdown)
                            .child(div().w(px(72.)).child(SettingsWithWindow(Rc::new(
                                move |window, cx| {
                                    let input = ensure_attack_retries_input(
                                        id,
                                        current_retries,
                                        window,
                                        cx,
                                    );
                                    Input::new(&input)
                                        .id("intruder-settings-retries-input")
                                        .aria_label("网络失败重试次数 (0-10)")
                                        .small()
                                        .disabled(!editable)
                                        .into_any_element()
                                },
                            ))))
                            .child(pause_dropdown)
                            .child(div().w(px(96.)).child(SettingsWithWindow(Rc::new(
                                move |window, cx| {
                                    let input = ensure_attack_retry_pause_input(
                                        id,
                                        current_pause,
                                        window,
                                        cx,
                                    );
                                    Input::new(&input)
                                        .id("intruder-settings-retry-pause-input")
                                        .aria_label("网络失败重试暂停毫秒数")
                                        .small()
                                        .disabled(!editable)
                                        .into_any_element()
                                },
                            ))))
                            .child(
                                button("intruder-settings-retry-apply", "应用重试配置")
                                    .tooltip(
                                        "校验并保存输入框中的网络失败重试次数与重试暂停时间 (ms)",
                                    )
                                    .disabled(!editable)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.apply_intruder_retry_inputs(id, window, cx)
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(theme::border())
                    .pt(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(theme::scaled_px(13.))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child("Redirections"),
                    )
                    .child(
                        div().flex().flex_wrap().gap(px(6.)).children(
                            [
                                (
                                    "intruder-redirect-never",
                                    "Never",
                                    RepeaterRedirectMode::Never,
                                ),
                                (
                                    "intruder-redirect-on-site",
                                    "On-site only",
                                    RepeaterRedirectMode::OnSiteOnly,
                                ),
                                (
                                    "intruder-redirect-in-scope",
                                    "In-scope only",
                                    RepeaterRedirectMode::InScopeOnly,
                                ),
                                (
                                    "intruder-redirect-always",
                                    "Always",
                                    RepeaterRedirectMode::Always,
                                ),
                            ]
                            .into_iter()
                            .map(|(btn_id, label, mode)| {
                                button(btn_id, label)
                                    .selected(current_redirect == mode)
                                    .disabled(!editable)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.update_intruder_attack_settings(
                                            id,
                                            |s| s.follow_redirects = mode,
                                            window,
                                            cx,
                                        );
                                    }))
                            }),
                        ),
                    )
                    .child(
                        Checkbox::new("intruder-settings-process-cookies")
                            .label("Process cookies in redirections")
                            .checked(attack_settings.process_cookies_in_redirections)
                            .disabled(!editable)
                            .on_click(cx.listener(move |this, value, window, cx| {
                                this.update_intruder_attack_settings(
                                    id,
                                    |s| s.process_cookies_in_redirections = *value,
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
            .into_any_element()
    }
}
