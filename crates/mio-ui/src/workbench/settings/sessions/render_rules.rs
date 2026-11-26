use gpui_kit::accesskit::Role;
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use mio_core::SessionTool;

use super::*;

struct SessionsTracerUi {
    selected_id: Option<Uuid>,
    list_scroll: ScrollHandle,
    before_scroll: ScrollHandle,
    after_scroll: ScrollHandle,
}

impl Default for SessionsTracerUi {
    fn default() -> Self {
        Self {
            selected_id: None,
            list_scroll: ScrollHandle::new(),
            before_scroll: ScrollHandle::new(),
            after_scroll: ScrollHandle::new(),
        }
    }
}

fn format_trace_timestamp(timestamp_ms: i64) -> String {
    let secs = timestamp_ms.div_euclid(1000);
    let millis = timestamp_ms.rem_euclid(1000);
    if let Some(cal) = crate::local_time::from_unix_seconds(secs) {
        format!("{:02}:{:02}:{:02}.{:03}", cal.hour, cal.min, cal.sec, millis)
    } else {
        format!("{timestamp_ms}ms")
    }
}

fn format_trace_tool(tool: SessionTool) -> &'static str {
    match tool {
        SessionTool::Proxy => "Proxy",
        SessionTool::Repeater => "Repeater",
        SessionTool::Intruder => "Intruder",
        SessionTool::Scanner => "Scanner",
        SessionTool::Sequencer => "Sequencer",
        SessionTool::Extensions => "Extensions",
        SessionTool::MioAi => "MioAI",
    }
}

fn tracer_vertical_scrollbar(
    id: impl Into<gpui_kit::ElementId>,
    handle: &ScrollHandle,
    dark: bool,
) -> gpui_kit::Div {
    let track_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xf5f5f5 }).into();
    let thumb_bg: gpui_kit::Hsla = rgb(if dark { 0x5c626b } else { 0xcbcbcb }).into();
    let thumb_hover: gpui_kit::Hsla = rgb(if dark { 0x707782 } else { 0xb0b0b0 }).into();
    let thumb_active: gpui_kit::Hsla = rgb(if dark { 0x828a96 } else { 0x999999 }).into();
    div()
        .relative()
        .w(px(10.))
        .h_full()
        .flex_shrink_0()
        .bg(track_bg)
        .child(
            Scrollbar::vertical(handle)
                .id(id)
                .viewport_from_layout()
                .mode(ScrollbarMode::Always)
                .styles(move |styles| {
                    styles
                        .track(|s| s.width(px(10.)).bg(track_bg))
                        .track_hover(|s| s.width(px(10.)).bg(track_bg))
                        .track_active(|s| s.width(px(10.)).bg(track_bg))
                        .thumb(|s| {
                            s.width(px(6.))
                                .inset(px(2.))
                                .radius(px(3.))
                                .min_length(px(24.))
                                .bg(thumb_bg)
                        })
                        .thumb_hover(|s| {
                            s.width(px(6.))
                                .inset(px(2.))
                                .radius(px(3.))
                                .min_length(px(24.))
                                .bg(thumb_hover)
                        })
                        .thumb_active(|s| {
                            s.width(px(6.))
                                .inset(px(2.))
                                .radius(px(3.))
                                .min_length(px(24.))
                                .bg(thumb_active)
                        })
                }),
        )
}

fn render_trace_preview_pane(
    id: &'static str,
    title: &'static str,
    content: &str,
    scroll: &ScrollHandle,
    dark: bool,
) -> AnyElement {
    let head_bg = rgb(if dark { 0x2b2e33 } else { 0xeaeaea });
    let text = if content.is_empty() {
        "（无请求预览）".to_owned()
    } else {
        content.to_owned()
    };
    div()
        .id(id)
        .test_support()
        .aria_label(format!("{title}: {text}"))
        .flex_1()
        .min_w_0()
        .h_full()
        .border_1()
        .border_color(theme::border())
        .rounded(px(3.))
        .bg(theme::surface())
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .h(theme::scaled_px(22.).max(px(22.)))
                .px(px(8.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .bg(head_bg)
                .border_b_1()
                .border_color(theme::border())
                .text_size(theme::scaled_px(11.5))
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(theme::foreground())
                .child(title),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-scroll")))
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_y_scroll()
                        .track_scroll(scroll)
                        .p(px(6.))
                        .font_family(theme::mono_font_family())
                        .text_size(theme::scaled_px(11.5))
                        .text_color(if content.is_empty() {
                            theme::muted_foreground()
                        } else {
                            theme::foreground()
                        })
                        .child(text),
                )
                .child(tracer_vertical_scrollbar(
                    SharedString::from(format!("{id}-vscroll")),
                    scroll,
                    dark,
                )),
        )
        .into_any_element()
}

pub(super) fn uniform_sessions_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    disabled: bool,
) -> Button {
    let dark = theme::is_dark();
    let bg = rgb(if dark { 0x3c3f44 } else { 0xeaeaea });
    let border = rgb(if dark { 0x4c5057 } else { 0xd4d4d4 });
    let fg = rgb(if dark { 0xdfe1e5 } else { 0x000000 });
    sessions_mio_button(id, label, disabled)
        .bg(bg)
        .border_color(border)
        .text_color(fg)
        .font_weight(gpui_kit::FontWeight::BOLD)
}

pub(super) fn describe_action(
    action: &SessionRuleAction,
    macros: &[mio_core::SessionMacro],
) -> String {
    match action {
        SessionRuleAction::UseCookieJar {
            add_new_cookies,
            update_existing_cookies,
        } => match (*add_new_cookies, *update_existing_cookies) {
            (true, true) => "使用 MIO Cookie Jar 中的 Cookie".into(),
            (true, false) => "使用 MIO Cookie Jar 中的 Cookie (仅新增)".into(),
            (false, true) => "使用 MIO Cookie Jar 中的 Cookie (仅更新)".into(),
            (false, false) => "使用 MIO Cookie Jar 中的 Cookie (不修改)".into(),
        },
        SessionRuleAction::SetHeader { name, value, .. } => format!("设置标头({name}: {value})"),
        SessionRuleAction::SetCookie { name, value, .. } => format!("设置 Cookie({name}={value})"),
        SessionRuleAction::RewriteUrlParam {
            name,
            value,
            remove,
            ..
        } => {
            if *remove {
                format!("移除参数({name})")
            } else {
                format!("设置参数({name}={value})")
            }
        }
        SessionRuleAction::ReplaceRequestText {
            match_pattern,
            replace_with,
            regex,
        } => {
            if *regex {
                format!("正则替换({match_pattern} -> {replace_with})")
            } else {
                format!("文本替换({match_pattern} -> {replace_with})")
            }
        }
        SessionRuleAction::RunMacro { macro_id, .. } => {
            let m_name = macros
                .iter()
                .find(|m| m.id == *macro_id)
                .map_or("未知", |m| m.name.as_str());
            format!("运行宏({m_name})")
        }
    }
}

impl Settings {
    pub(in crate::workbench) fn render_sessions(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut body = div()
            .id(SharedString::from(format!(
                "settings-sessions-project-{:?}",
                self.state.project
            )))
            .px(px(22.))
            .py(px(14.))
            .flex()
            .flex_col()
            .gap(px(14.));

        if let Some(status) = self.state.status {
            body = body.child(
                div()
                    .id("settings-sessions-status")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(status)
                    .p(px(8.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::toolbar_surface())
                    .child(status),
            );
        }
        let alert = self
            .sessions
            .local_error
            .clone()
            .or_else(|| self.state.error.clone());
        if let Some(error) = alert {
            body = body.child(
                div()
                    .id("settings-sessions-error")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(error.clone())
                    .p(px(8.))
                    .rounded(px(4.))
                    .bg(theme::danger_bg())
                    .text_color(theme::danger_fg())
                    .child(error),
            );
        }

        let content = body
            .child(self.render_session_rules_card(window, cx))
            .child(self.render_cookie_jar_card(cx))
            .child(self.render_session_macros_card(window, cx));
        settings_scrollable_pane("settings-sessions-page", &self.sessions.scroll, content)
            .into_any_element()
    }

    fn render_session_rules_card(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.state.enabled;
        let selected_idx = self
            .sessions
            .selected_rule
            .and_then(|id| self.state.sessions.rules.iter().position(|r| r.id == id));
        let has_sel = selected_idx.is_some()
            || (self.state.sessions.rules.is_empty() && self.sessions.default_rule_selected);
        let can_up = selected_idx.is_some_and(|i| i > 0);
        let can_down = selected_idx.is_some_and(|i| i + 1 < self.state.sessions.rules.len());
        let active_rule_count = if self.state.sessions.rules.is_empty() {
            usize::from(!self.sessions.default_rule_dismissed && self.sessions.default_rule_enabled)
        } else {
            self.state
                .sessions
                .rules
                .iter()
                .filter(|r| r.enabled)
                .count()
        };

        div()
            .pb(px(14.))
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(sessions_section_heading(
                "settings-sessions-rules-heading",
                "会话处理规则",
            ))
            .child(sessions_desc_row(
                "您可以定义会话处理规则，使系统在发出 HTTP 请求时执行特定操作。每条规则均可指定作用范围（工具、URL 或参数），并执行注入 Cookie、登录应用或校验会话有效性等动作。发送每个请求前，将按顺序应用所有匹配范围内的规则。",
            ))
            .child(
                div()
                    .pl(px(29.))
                    .pt(px(2.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .items_stretch()
                            .gap(px(8.))
                            .h(theme::scaled_px(170.).max(px(170.)))
                            .child(
                                div()
                                    .w(theme::scaled_px(88.).max(px(84.)))
                                    .flex_shrink_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(5.))
                                    .child(
                                        uniform_sessions_button(
                                            "settings-sessions-rule-add",
                                            "添加",
                                            !enabled,
                                        )
                                        .w_full()
                                        .tooltip("添加新的会话处理规则")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_new_session_rule_editor(window, cx)
                                        })),
                                    )
                                    .child(
                                        uniform_sessions_button(
                                            "settings-sessions-rule-edit",
                                            "编辑",
                                            !enabled || !has_sel,
                                        )
                                        .w_full()
                                        .tooltip("编辑当前选中的会话处理规则")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.load_selected_rule_for_edit(window, cx)
                                        })),
                                    )
                                    .child(
                                        uniform_sessions_button(
                                            "settings-sessions-rule-remove",
                                            "移除",
                                            !enabled || !has_sel,
                                        )
                                        .w_full()
                                        .tooltip("移除当前选中的会话处理规则")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.remove_selected_session_rule(cx)
                                        })),
                                    )
                                    .child(
                                        uniform_sessions_button(
                                            "settings-sessions-rule-duplicate",
                                            "复制",
                                            !enabled || !has_sel,
                                        )
                                        .w_full()
                                        .tooltip("复制当前选中的会话处理规则")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.duplicate_selected_session_rule(cx)
                                        })),
                                    )
                                    .child(
                                        uniform_sessions_button(
                                            "settings-sessions-rule-up",
                                            "上移",
                                            !enabled || !can_up,
                                        )
                                        .w_full()
                                        .tooltip("上移选中的会话处理规则以提高执行顺序")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.move_selected_session_rule(false, cx)
                                        })),
                                    )
                                    .child(
                                        uniform_sessions_button(
                                            "settings-sessions-rule-down",
                                            "下移",
                                            !enabled || !can_down,
                                        )
                                        .w_full()
                                        .tooltip("下移选中的会话处理规则以延后执行顺序")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.move_selected_session_rule(true, cx)
                                        })),
                                    ),
                            )
                            .child(
                                div()
                                    .id("sessions-rules-table-wrap")
                                    .w(theme::scaled_px(704.).max(px(704.)))
                                    .max_w_full()
                                    .h_full()
                                    .child(sessions_table_frame(
                                        DataTable::new(&self.sessions.rules_table)
                                            .with_size(theme::scaled_px(22.).max(px(20.)))
                                            .header_row_height(theme::scaled_px(24.).max(px(22.)))
                                            .bordered(false)
                                            .scroll_bounce(false)
                                            .accessibility_label("会话处理规则"),
                                    )),
                            ),
                    )
                    .when(self.sessions.rule_editor_open, |this| {
                        this.child(self.render_rule_editor_panel(enabled, window, cx))
                    })
                    .child(
                        div()
                            .pt(px(2.))
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap(px(8.))
                            .child("使用会话追踪器监控或排查会话处理规则的执行行为。")
                            .child(
                                uniform_sessions_button(
                                    "settings-sessions-open-tracer",
                                    "打开会话追踪器",
                                    !enabled,
                                )
                                .px(px(14.))
                                .tooltip("打开或关闭会话追踪器状态视图")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.sessions.tracer_open = !this.sessions.tracer_open;
                                    cx.notify();
                                })),
                            ),
                    )
                    .when(self.sessions.tracer_open, |this| {
                        this.child(self.render_sessions_tracer_panel(
                            active_rule_count,
                            window,
                            cx,
                        ))
                    }),
            )
            .into_any_element()
    }

    fn render_sessions_tracer_panel(
        &self,
        active_rule_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let tracer_ui = window.use_keyed_state("settings-sessions-tracer-ui", cx, |_, _| {
            SessionsTracerUi::default()
        });
        let (selected_id, list_scroll, before_scroll, after_scroll) = {
            let ui = tracer_ui.read(cx);
            (
                ui.selected_id,
                ui.list_scroll.clone(),
                ui.before_scroll.clone(),
                ui.after_scroll.clone(),
            )
        };
        let traces = &self.state.session_traces;
        let paused = self.state.session_tracer_paused;
        let selected_entry = selected_id.and_then(|id| traces.iter().find(|e| e.id == id));

        let head_bg = rgb(if dark { 0x2b2e33 } else { 0xeaeaea });
        let head_sep = rgb(if dark { 0x3e424a } else { 0xd8d8d8 });
        let zebra_bg = rgb(if dark { 0x282b30 } else { 0xf4f5f7 });
        let row_hover = rgb(if dark { 0x2f3540 } else { 0xedf2fa });
        let row_h = theme::scaled_px(22.).max(px(22.));

        let mut rows = div()
            .id("settings-sessions-tracer-rows")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&list_scroll)
            .flex()
            .flex_col();

        if traces.is_empty() {
            rows = rows.child(
                div()
                    .id("settings-sessions-tracer-empty")
                    .test_support()
                    .p(px(12.))
                    .text_color(theme::muted_foreground())
                    .child("尚无会话追踪记录。当 Repeater、Intruder、Proxy 等工具触发会话处理规则时，执行日志将实时显示在此处。"),
            );
        } else {
            for (idx, entry) in traces.iter().enumerate() {
                let is_selected = selected_entry.is_some_and(|sel| sel.id == entry.id);
                let time_text = format_trace_timestamp(entry.timestamp_ms);
                let tool_text = format_trace_tool(entry.tool);
                let ui_click = tracer_ui.clone();
                let entry_id = entry.id;
                let row_bg: gpui_kit::Hsla = if is_selected {
                    theme::selected()
                } else if theme::table_zebra_stripe() && idx % 2 == 1 {
                    zebra_bg.into()
                } else {
                    theme::surface()
                };
                rows = rows.child(
                    div()
                        .id(("settings-sessions-tracer-row", idx))
                        .test_support()
                        .role(Role::Row)
                        .aria_label(format!(
                            "{} {} {} {} {}",
                            time_text, tool_text, entry.url, entry.rule_name, entry.action_summary
                        ))
                        .aria_selected(is_selected)
                        .h(row_h)
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .bg(row_bg)
                        .cursor_pointer()
                        .when(!is_selected, |this| {
                            this.hover(move |s| s.bg(row_hover))
                        })
                        .on_click(cx.listener(move |_, _, _, cx| {
                            ui_click.update(cx, |ui, cx| {
                                ui.selected_id = Some(entry_id);
                                cx.notify();
                            });
                            cx.notify();
                        }))
                        .child(
                            div()
                                .w(theme::scaled_px(88.).max(px(88.)))
                                .flex_shrink_0()
                                .px(px(6.))
                                .font_family(theme::mono_font_family())
                                .text_size(theme::scaled_px(11.5))
                                .truncate()
                                .child(time_text),
                        )
                        .child(
                            div()
                                .w(theme::scaled_px(76.).max(px(76.)))
                                .flex_shrink_0()
                                .px(px(6.))
                                .truncate()
                                .child(tool_text),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(150.))
                                .px(px(6.))
                                .truncate()
                                .child(entry.url.clone()),
                        )
                        .child(
                            div()
                                .w(theme::scaled_px(136.).max(px(136.)))
                                .flex_shrink_0()
                                .px(px(6.))
                                .truncate()
                                .child(entry.rule_name.clone()),
                        )
                        .child(
                            div()
                                .w(theme::scaled_px(176.).max(px(176.)))
                                .flex_shrink_0()
                                .px(px(6.))
                                .truncate()
                                .child(entry.action_summary.clone()),
                        ),
                );
            }
        }

        let tracer_table = div()
            .id("settings-sessions-tracer-list")
            .test_support()
            .role(Role::Table)
            .aria_label("会话追踪记录列表")
            .w_full()
            .h(theme::scaled_px(184.).max(px(184.)))
            .border_1()
            .border_color(theme::border())
            .rounded(px(3.))
            .bg(theme::surface())
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(theme::scaled_px(24.).max(px(24.)))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .bg(head_bg)
                    .border_b_1()
                    .border_color(head_sep)
                    .text_color(theme::foreground())
                    .child(
                        div()
                            .w(theme::scaled_px(88.).max(px(88.)))
                            .h_full()
                            .flex_shrink_0()
                            .px(px(6.))
                            .flex()
                            .items_center()
                            .border_r_1()
                            .border_color(head_sep)
                            .child("时间"),
                    )
                    .child(
                        div()
                            .w(theme::scaled_px(76.).max(px(76.)))
                            .h_full()
                            .flex_shrink_0()
                            .px(px(6.))
                            .flex()
                            .items_center()
                            .border_r_1()
                            .border_color(head_sep)
                            .child("工具"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(150.))
                            .h_full()
                            .px(px(6.))
                            .flex()
                            .items_center()
                            .border_r_1()
                            .border_color(head_sep)
                            .child("URL"),
                    )
                    .child(
                        div()
                            .w(theme::scaled_px(136.).max(px(136.)))
                            .h_full()
                            .flex_shrink_0()
                            .px(px(6.))
                            .flex()
                            .items_center()
                            .border_r_1()
                            .border_color(head_sep)
                            .child("规则"),
                    )
                    .child(
                        div()
                            .w(theme::scaled_px(176.).max(px(176.)))
                            .h_full()
                            .flex_shrink_0()
                            .px(px(6.))
                            .flex()
                            .items_center()
                            .child("动作摘要"),
                    )
                    .child(div().w(px(10.)).h_full().flex_shrink_0()),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(rows)
                    .child(tracer_vertical_scrollbar(
                        "settings-sessions-tracer-vscroll",
                        &list_scroll,
                        dark,
                    )),
            );

        let ui_clear = tracer_ui.clone();
        let mut panel = div()
            .id("settings-sessions-tracer-panel")
            .test_support()
            .w_full()
            .p(px(10.))
            .rounded(px(4.))
            .border_1()
            .border_color(theme::border())
            .bg(theme::background())
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_color(theme::foreground())
                                    .child("会话追踪器"),
                            )
                            .when(paused, |this| {
                                this.child(
                                    div()
                                        .id("settings-sessions-tracer-paused-badge")
                                        .test_support()
                                        .px(px(6.))
                                        .py(px(1.))
                                        .rounded_full()
                                        .bg(rgb(if dark { 0x4b3c20 } else { 0xfef0c7 }))
                                        .text_size(theme::scaled_px(11.5))
                                        .text_color(rgb(if dark { 0xfed26b } else { 0x93370d }))
                                        .child("已暂停"),
                                )
                            })
                            .child(
                                div()
                                    .text_size(theme::scaled_px(12.))
                                    .text_color(theme::muted_foreground())
                                    .child(format!(
                                        "记录：{} 条 ｜ 启用规则：{} 条 ｜ Cookie Jar：{} 条 ｜ Macro：{} 个",
                                        traces.len(),
                                        active_rule_count,
                                        self.state.sessions.cookie_jar.entries.len(),
                                        self.state.sessions.macros.len()
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                uniform_sessions_button(
                                    "settings-sessions-tracer-pause",
                                    if paused { "继续追踪" } else { "暂停追踪" },
                                    false,
                                )
                                .px(px(10.))
                                .tooltip(if paused {
                                    "恢复接收新的会话规则追踪记录"
                                } else {
                                    "暂停记录新的会话规则执行日志"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.session_tracer_paused =
                                        !this.state.session_tracer_paused;
                                    let _ = this.owner.update(cx, |view, cx| {
                                        view.toggle_session_tracer_paused(cx);
                                    });
                                    cx.notify();
                                })),
                            )
                            .child(
                                uniform_sessions_button(
                                    "settings-sessions-tracer-clear",
                                    "清空记录",
                                    traces.is_empty(),
                                )
                                .px(px(10.))
                                .tooltip("清空当前会话追踪记录列表")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.state.session_traces.clear();
                                    ui_clear.update(cx, |ui, cx| {
                                        ui.selected_id = None;
                                        cx.notify();
                                    });
                                    let _ = this.owner.update(cx, |view, cx| {
                                        view.clear_session_traces(cx);
                                    });
                                    cx.notify();
                                })),
                            ),
                    ),
            )
            .child(tracer_table);

        if let Some(entry) = selected_entry {
            panel = panel
                .child(
                    div()
                        .id("settings-sessions-tracer-detail")
                        .test_support()
                        .aria_label(format!(
                            "{} {} {}",
                            entry.rule_name, entry.action_summary, entry.detail
                        ))
                        .p(px(8.))
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::surface())
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            div()
                                .text_size(theme::scaled_px(12.))
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(theme::foreground())
                                .child(format!(
                                    "规则：{} ｜ 工具：{} ｜ 动作：{}",
                                    entry.rule_name,
                                    format_trace_tool(entry.tool),
                                    entry.action_summary
                                )),
                        )
                        .child(
                            div()
                                .text_size(theme::scaled_px(12.))
                                .text_color(theme::muted_foreground())
                                .child(format!("URL：{}", entry.url)),
                        )
                        .child(
                            div()
                                .text_size(theme::scaled_px(12.))
                                .text_color(theme::foreground())
                                .child(format!("详情：{}", entry.detail)),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .h(theme::scaled_px(152.).max(px(152.)))
                        .flex()
                        .items_stretch()
                        .gap(px(8.))
                        .child(render_trace_preview_pane(
                            "settings-sessions-tracer-before",
                            "修改前请求预览",
                            &entry.before_preview,
                            &before_scroll,
                            dark,
                        ))
                        .child(render_trace_preview_pane(
                            "settings-sessions-tracer-after",
                            "修改后请求预览",
                            &entry.after_preview,
                            &after_scroll,
                            dark,
                        )),
                );
        } else {
            panel = panel.child(
                div()
                    .text_size(theme::scaled_px(12.))
                    .text_color(theme::muted_foreground())
                    .child("点击上方任意追踪记录以查看规则执行详情及修改前/后请求预览。"),
            );
        }

        panel.into_any_element()
    }
}
