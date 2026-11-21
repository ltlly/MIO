use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::base::{Checkbox, CheckboxIndicator, CheckboxState};
use gpui_kit::component::Icon;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::resizable::{resizable_panel, v_resizable};
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::component::switch::Switch;
use gpui_kit::{ClickEvent, DragMoveEvent, FontWeight, MouseButton, MouseDownEvent, uniform_list};
use mio_core::TrafficSource;

use super::*;

struct LoggerScriptPopoverState {
    editor: Entity<TextareaState>,
    synced: String,
    feedback: Option<(bool, String)>,
}

impl LoggerScriptPopoverState {
    fn new(
        initial: String,
        placeholder: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| {
            let mut st = TextareaState::new(window, cx)
                .placeholder(placeholder)
                .soft_wrap(true)
                .autocorrect(false);
            if !initial.is_empty() {
                st.set_value(initial.clone(), window, cx);
            }
            st
        });
        Self {
            editor,
            synced: initial,
            feedback: None,
        }
    }
}

#[derive(Clone, Copy)]
enum LoggerHeaderSort {
    Primary(LoggerSortColumn),
    Extended(LoggerExtendedSortColumn),
}

struct LoggerRowsCacheKey {
    entries_len: usize,
    first_id: Option<FlowId>,
    last_id: Option<FlowId>,
    entries_sig: u64,
    source_filter: Option<TrafficSource>,
    search_term: String,
    sort: LoggerSort,
    extended_sort: Option<(LoggerExtendedSortColumn, bool)>,
    status_classes: [bool; 4],
    in_scope_only: bool,
    scope: Option<mio_core::TargetScope>,
    script_enabled: bool,
    view_filter_script: String,
    param_counts_sig: (usize, usize),
    live_notes_sig: (usize, u64),
}

pub(in crate::workbench::tools_extra) struct LoggerRowsCacheEntry {
    key: LoggerRowsCacheKey,
    rows: Rc<Vec<(usize, FlowId)>>,
}

fn logger_entries_fingerprint(entries: &[mio_core::FlowSummary]) -> u64 {
    let mut acc = entries.len() as u64;
    for entry in entries {
        let id_raw = entry.id.as_u128();
        let mut x = (id_raw as u64)
            ^ ((id_raw >> 64) as u64)
            ^ entry.revision.rotate_left(13)
            ^ (u64::from(entry.status.unwrap_or(0)) << 48)
            ^ entry.response_bytes.rotate_left(7)
            ^ entry.elapsed_ms.rotate_left(29)
            ^ (entry.started_unix_ms as u64).rotate_left(3)
            ^ (entry.url.len() as u64).rotate_left(19)
            ^ (entry.method.len() as u64).rotate_left(41);
        x ^= x >> 30;
        x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        acc = acc.rotate_left(5).wrapping_add(x);
    }
    acc
}

fn logger_live_notes_fingerprint(
    live_notes: &std::collections::HashMap<FlowId, (u64, String)>,
) -> (usize, u64) {
    use std::hash::{Hash, Hasher};
    let mut sum = 0u64;
    for (id, (rev, text)) in live_notes {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        id.hash(&mut h);
        rev.hash(&mut h);
        text.hash(&mut h);
        sum = sum.wrapping_add(h.finish());
    }
    (live_notes.len(), sum)
}

fn logger_checkbox_indicator(checked: bool) -> CheckboxIndicator {
    let dark = theme::is_dark();
    let unchecked_border: gpui_kit::Hsla = if dark {
        theme::control_border()
    } else {
        rgb(0xb8b8b8).into()
    };
    let checked_bg: gpui_kit::Hsla = rgb(0x1558b0).into();
    CheckboxIndicator::new()
        .checked(checked)
        .size(px(14.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.))
        .border_1()
        .border_color(if checked {
            checked_bg
        } else {
            unchecked_border
        })
        .bg(if checked {
            checked_bg
        } else {
            theme::surface()
        })
        .when(checked, |this| {
            this.child(
                Icon::new(IconName::Check)
                    .size(px(11.))
                    .text_color(rgb(0xffffff)),
            )
        })
}

fn logger_checkbox(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    checked: bool,
) -> Checkbox {
    let label: SharedString = label.into();
    Checkbox::new(id)
        .accessibility_label(label.clone())
        .checked(checked)
        .tab_stop(false)
        .flex()
        .items_center()
        .gap(px(7.))
        .py(px(2.))
        .text_size(theme::ui_font_size())
        .text_color(theme::foreground())
        .child(logger_checkbox_indicator(checked))
        .child(label)
}

impl Workbench {
    #[inline(never)]
    fn render_logger_capture_filter_bar(
        &self,
        capture_summary: String,
        filter_bg: gpui_kit::Hsla,
        filter_hover_bg: gpui_kit::Hsla,
        filter_border: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let capture_owner = cx.weak_entity();
        div()
            .flex_1()
            .min_w_0()
            .child(
                Popover::new("logger-capture-filter-popover")
                    .anchor(gpui_kit::Anchor::TopLeft)
                    .trigger_style(gpui_kit::StyleRefinement::default().w_full())
                    .trigger(
                        Button::new("logger-capture-filter-bar")
                            .ghost()
                            .small()
                            .compact()
                            .w_full()
                            .h(theme::scaled_px(24.))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(filter_border)
                            .px(theme::scaled_px(8.))
                            .bg(filter_bg)
                            .hover(move |s| s.bg(filter_hover_bg))
                            .text_color(theme::foreground())
                            .justify_start()
                            .accessibility_label(capture_summary.clone())
                            .tooltip("点击配置捕获筛选（内存记录上限、捕获工具来源与捕获条件）")
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap(theme::scaled_px(6.))
                                    .child(
                                        Icon::new(IconName::Funnel)
                                            .size(theme::scaled_px(13.))
                                            .text_color(theme::foreground())
                                            .flex_shrink_0(),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_left()
                                            .text_size(theme::ui_font_size())
                                            .child(
                                                capture_summary
                                                    .clone()
                                                    .test_text(capture_summary),
                                            ),
                                    ),
                            ),
                    )
                    .content(move |_, window, cx| {
                        let Some(wb) = capture_owner.upgrade() else {
                            return div().into_any_element();
                        };
                        let (
                            max_entries,
                            capture_sources,
                            capture_in_scope,
                            capture_status,
                            capture_script,
                        ) = {
                            let view = wb.read(cx);
                            (
                                view.extra_tools.logger_max_entries,
                                view.extra_tools.logger_filters.capture_sources,
                                view.extra_tools.logger_filters.capture_in_scope_only,
                                view.extra_tools.logger_filters.capture_status_classes,
                                view.extra_tools
                                    .logger_filters
                                    .capture_filter_script
                                    .clone(),
                            )
                        };
                        let script_state = window.use_keyed_state(
                            "logger-capture-script-editor-state",
                            cx,
                            |window, cx| {
                                LoggerScriptPopoverState::new(
                                    capture_script.clone(),
                                    "例如：status >= 400 || method == \"POST\"",
                                    window,
                                    cx,
                                )
                            },
                        );
                        if script_state.read(cx).synced != capture_script {
                            let synced_next = capture_script.clone();
                            script_state.update(cx, |st, cx| {
                                st.synced = synced_next.clone();
                                st.editor
                                    .update(cx, |ed, cx| ed.set_value(synced_next, window, cx));
                            });
                        }
                        let (capture_editor, capture_feedback) = {
                            let st = script_state.read(cx);
                            (st.editor.clone(), st.feedback.clone())
                        };
                        let o_scope = capture_owner.clone();
                        let o_reset = capture_owner.clone();
                        let o_apply = capture_owner.clone();
                        let o_clear = capture_owner.clone();
                        let st_val = script_state.clone();
                        let st_apply = script_state.clone();
                        let st_clear = script_state.clone();
                        let st_reset = script_state.clone();
                        div()
                            .id("logger-capture-filter-content")
                            .test_support()
                            .w(px(400.))
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(
                                div()
                                    .text_size(theme::scaled_px(13.5))
                                    .font_weight(FontWeight::BOLD)
                                    .child("捕获筛选设置 (Capture filter)"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_size(theme::scaled_px(12.))
                                            .text_color(theme::muted_foreground())
                                            .child("内存记录上限"),
                                    )
                                    .child(div().flex().flex_wrap().gap(px(6.)).children(
                                        LOGGER_MAX_LIMITS.map(|limit| {
                                            let o_lim = capture_owner.clone();
                                            button(
                                                SharedString::from(format!("logger-max-{limit}")),
                                                format!("上限 {limit}"),
                                            )
                                            .tooltip(format!("设置内存日志保留上限为 {limit} 条"))
                                            .selected(max_entries == limit)
                                            .on_click(move |_, _, cx| {
                                                let _ = o_lim.update(cx, |this, cx| {
                                                    this.set_logger_max_entries(limit, cx);
                                                });
                                            })
                                        }),
                                    )),
                            )
                            .child(
                                div()
                                    .pt(px(6.))
                                    .border_t_1()
                                    .border_color(theme::border())
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .text_size(theme::scaled_px(12.))
                                            .text_color(theme::muted_foreground())
                                            .child("实时捕获工具来源"),
                                    )
                                    .child(div().flex().flex_wrap().gap(px(10.)).children(
                                        [
                                            (0usize, "logger-capture-source-proxy", "Proxy"),
                                            (1usize, "logger-capture-source-repeater", "Repeater"),
                                            (2usize, "logger-capture-source-intruder", "Intruder"),
                                            (3usize, "logger-capture-source-scanner", "Scanner"),
                                            (4usize, "logger-capture-source-plugin", "Extensions"),
                                        ]
                                        .into_iter()
                                        .map(|(idx, cb_id, lbl)| {
                                            let o_src = capture_owner.clone();
                                            div().w(px(110.)).child(
                                                logger_checkbox(cb_id, lbl, capture_sources[idx])
                                                    .on_change(move |state, _, _, cx| {
                                                        let checked =
                                                            state == CheckboxState::Checked;
                                                        let _ = o_src.update(cx, |this, cx| {
                                                            this.extra_tools
                                                                .logger_filters
                                                                .capture_sources[idx] = checked;
                                                            cx.notify();
                                                        });
                                                    }),
                                            )
                                        }),
                                    )),
                            )
                            .child(
                                div()
                                    .pt(px(6.))
                                    .border_t_1()
                                    .border_color(theme::border())
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .text_size(theme::scaled_px(12.))
                                            .text_color(theme::muted_foreground())
                                            .child("捕获范围与状态码"),
                                    )
                                    .child(
                                        logger_checkbox(
                                            "logger-capture-in-scope-only",
                                            "仅捕获目标范围内 (In scope only)",
                                            capture_in_scope,
                                        )
                                        .on_change(move |state, _, _, cx| {
                                            let checked = state == CheckboxState::Checked;
                                            let _ = o_scope.update(cx, |this, cx| {
                                                this.extra_tools
                                                    .logger_filters
                                                    .capture_in_scope_only = checked;
                                                cx.notify();
                                            });
                                        }),
                                    )
                                    .child(div().flex().flex_wrap().gap(px(10.)).children(
                                        [
                                            (0usize, "logger-capture-status-2xx", "2xx [成功]"),
                                            (1usize, "logger-capture-status-3xx", "3xx [重定向]"),
                                            (2usize, "logger-capture-status-4xx", "4xx [客户端错误]"),
                                            (3usize, "logger-capture-status-5xx", "5xx [服务端错误]"),
                                        ]
                                        .into_iter()
                                        .map(|(idx, cb_id, lbl)| {
                                            let o_st = capture_owner.clone();
                                            div().w(px(156.)).child(
                                                logger_checkbox(cb_id, lbl, capture_status[idx])
                                                    .on_change(move |state, _, _, cx| {
                                                        let checked =
                                                            state == CheckboxState::Checked;
                                                        let _ = o_st.update(cx, |this, cx| {
                                                            this.extra_tools
                                                                .logger_filters
                                                                .capture_status_classes[idx] =
                                                                checked;
                                                            cx.notify();
                                                        });
                                                    }),
                                            )
                                        }),
                                    )),
                            )
                            .child(
                                div()
                                    .pt(px(6.))
                                    .border_t_1()
                                    .border_color(theme::border())
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_size(theme::scaled_px(12.))
                                            .text_color(theme::muted_foreground())
                                            .child("捕获脚本 (Capture MioScript)"),
                                    )
                                    .child(
                                        div()
                                            .id("logger-capture-script-input")
                                            .test_support()
                                            .w_full()
                                            .h(px(68.))
                                            .font_family(theme::mono_font_family())
                                            .text_size(theme::editor_font_size())
                                            .child(
                                                Textarea::new(&capture_editor)
                                                    .size_full()
                                                    .bordered(true)
                                                    .aria_label("捕获脚本编辑器"),
                                            ),
                                    )
                                    .when_some(capture_feedback, |box_el, (ok, msg)| {
                                        box_el.child(
                                            div()
                                                .id("logger-capture-script-feedback")
                                                .test_support()
                                                .text_size(theme::scaled_px(11.5))
                                                .text_color(if ok {
                                                    rgb(0x2e8b57).into()
                                                } else {
                                                    theme::danger_fg()
                                                })
                                                .child(msg),
                                        )
                                    })
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.))
                                            .child(
                                                button("logger-capture-script-validate", "验证脚本")
                                                    .on_click(move |_, _, cx| {
                                                        let code = st_val
                                                            .read(cx)
                                                            .editor
                                                            .read(cx)
                                                            .content()
                                                            .text()
                                                            .to_string();
                                                        let feedback = match table::validate_logger_script(&code) {
                                                            Ok(()) if code.trim().is_empty() => {
                                                                (true, "脚本为空：将捕获全部流量。".to_owned())
                                                            }
                                                            Ok(()) => {
                                                                (true, "编译通过：捕获脚本语法有效。".to_owned())
                                                            }
                                                            Err(err) => (false, format!("语法错误：{err}")),
                                                        };
                                                        st_val.update(cx, |st, cx| {
                                                            st.feedback = Some(feedback);
                                                            cx.notify();
                                                        });
                                                    }),
                                            )
                                            .child(
                                                button("logger-capture-script-apply", "应用脚本")
                                                    .primary()
                                                    .on_click(move |_, _, cx| {
                                                        let code = st_apply
                                                            .read(cx)
                                                            .editor
                                                            .read(cx)
                                                            .content()
                                                            .text()
                                                            .to_string();
                                                        match table::validate_logger_script(&code) {
                                                            Ok(()) => {
                                                                let trimmed = code.trim().to_owned();
                                                                st_apply.update(cx, |st, cx| {
                                                                    st.synced = trimmed.clone();
                                                                    st.feedback = Some((
                                                                        true,
                                                                        if trimmed.is_empty() {
                                                                            "已清除捕获 脚本。".to_owned()
                                                                        } else {
                                                                            "已应用捕获 脚本。".to_owned()
                                                                        },
                                                                    ));
                                                                    cx.notify();
                                                                });
                                                                let _ = o_apply.update(cx, |this, cx| {
                                                                    this.extra_tools
                                                                        .logger_filters
                                                                        .capture_filter_script = trimmed;
                                                                    cx.notify();
                                                                });
                                                            }
                                                            Err(err) => {
                                                                st_apply.update(cx, |st, cx| {
                                                                    st.feedback = Some((false, format!("未应用，语法错误：{err}")));
                                                                    cx.notify();
                                                                });
                                                            }
                                                        }
                                                    }),
                                            )
                                            .child(
                                                button("logger-capture-script-clear", "清空")
                                                    .ghost()
                                                    .on_click(move |_, window, cx| {
                                                        st_clear.update(cx, |st, cx| {
                                                            st.synced.clear();
                                                            st.feedback = Some((true, "已清空捕获 脚本。".to_owned()));
                                                            st.editor.update(cx, |ed, cx| {
                                                                ed.set_value("", window, cx);
                                                            });
                                                            cx.notify();
                                                        });
                                                        let _ = o_clear.update(cx, |this, cx| {
                                                            this.extra_tools
                                                                .logger_filters
                                                                .capture_filter_script
                                                                .clear();
                                                            cx.notify();
                                                        });
                                                    }),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .pt(px(8.))
                                    .border_t_1()
                                    .border_color(theme::border())
                                    .flex()
                                    .justify_end()
                                    .child(
                                        button("logger-capture-filter-reset", "恢复默认").on_click(
                                            move |_, window, cx| {
                                                st_reset.update(cx, |st, cx| {
                                                    st.synced.clear();
                                                    st.feedback = None;
                                                    st.editor.update(cx, |ed, cx| {
                                                        ed.set_value("", window, cx);
                                                    });
                                                    cx.notify();
                                                });
                                                let _ = o_reset.update(cx, |this, cx| {
                                                    this.reset_logger_capture_filters(cx);
                                                });
                                            },
                                        ),
                                    ),
                            )
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_logger_filter_row_1(
        &self,
        capture_summary: String,
        filter_bg: gpui_kit::Hsla,
        filter_hover_bg: gpui_kit::Hsla,
        filter_border: gpui_kit::Hsla,
        toolbar_icon_fg: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let capture_filter_bar = self.render_logger_capture_filter_bar(
            capture_summary,
            filter_bg,
            filter_hover_bg,
            filter_border,
            cx,
        );
        let capture_enabled = self.extra_tools.capture_enabled;
        let capture_switch = Switch::new("logger-capture-toggle")
            .small()
            .color(rgb(0x1558b0))
            .checked(capture_enabled)
            .label(if capture_enabled {
                "记录开启"
            } else {
                "记录暂停"
            })
            .tooltip(if capture_enabled {
                "记录开启 (Logging on)：正在实时捕获各工具模块的 HTTP 流量，点击暂停"
            } else {
                "记录暂停 (Logging off)：已暂停实时捕获流量，点击恢复"
            })
            .on_change(cx.listener(|this, checked, _, cx| {
                this.extra_tools.capture_enabled = *checked;
                cx.notify();
            }));

        let help_popover = Popover::new("logger-help-popover")
            .anchor(gpui_kit::Anchor::TopRight)
            .trigger(
                Button::new("logger-help")
                    .ghost()
                    .compact()
                    .p_0()
                    .size(theme::scaled_px(22.))
                    .rounded(px(11.))
                    .accessibility_label("Logger 帮助")
                    .tooltip("查看 Logger 捕获与视图筛选说明")
                    .child(
                        Icon::new(IconName::CircleQuestionMark)
                            .size(px(18.))
                            .text_color(toolbar_icon_fg),
                    ),
            )
            .content(|_, _, _| {
                div()
                    .id("logger-help-content")
                    .test_support()
                    .w(px(310.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .text_size(theme::ui_font_size())
                    .text_color(theme::foreground())
                    .child(div().font_weight(FontWeight::BOLD).child("Logger"))
                    .child("第一行捕获筛选栏控制内存记录上限、捕获工具来源与捕获条件，右侧开关可即时暂停或恢复记录。")
                    .child("第二行视图筛选栏可按工具来源、目标范围、状态码分类与 脚本表达式过滤视图条目，右侧搜索框支持关键字或 脚本表达式实时过滤。")
                    .child("点击表格任意行可展开下方 Request / Response 与 Inspector 面板，右键行可发送至其他工具或导出 HAR。")
            });

        div()
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(theme::scaled_px(10.))
            .child(capture_filter_bar)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::scaled_px(8.))
                    .flex_shrink_0()
                    .child(capture_switch)
                    .child(help_popover),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_logger_view_filter_bar(
        &self,
        view_summary: String,
        filter_bg: gpui_kit::Hsla,
        filter_hover_bg: gpui_kit::Hsla,
        filter_border: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view_owner = cx.weak_entity();
        div()
            .flex_1()
            .min_w_0()
            .child(
                Popover::new("logger-view-filter-popover")
                    .anchor(gpui_kit::Anchor::TopLeft)
                    .trigger_style(gpui_kit::StyleRefinement::default().w_full())
                    .trigger(
                        Button::new("logger-view-filter-bar")
                            .ghost()
                            .small()
                            .compact()
                            .w_full()
                            .h(theme::scaled_px(24.))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(filter_border)
                            .px(theme::scaled_px(8.))
                            .bg(filter_bg)
                            .hover(move |s| s.bg(filter_hover_bg))
                            .text_color(theme::foreground())
                            .justify_start()
                            .accessibility_label(view_summary.clone())
                            .tooltip("点击配置视图筛选（工具来源、目标范围、状态码与 MioScript）")
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap(theme::scaled_px(6.))
                                    .child(
                                        Icon::new(IconName::Funnel)
                                            .size(theme::scaled_px(13.))
                                            .text_color(theme::foreground())
                                            .flex_shrink_0(),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_left()
                                            .text_size(theme::ui_font_size())
                                            .child(view_summary.clone().test_text(view_summary)),
                                    ),
                            ),
                    )
                    .content(move |_, window, cx| {
                        let Some(wb) = view_owner.upgrade() else {
                            return div().into_any_element();
                        };
                        let (
                            source_filter,
                            in_scope_only,
                            status_classes,
                            script_enabled,
                            view_script,
                        ) = {
                            let view = wb.read(cx);
                            (
                                view.extra_tools.logger_source_filter.clone(),
                                view.extra_tools.logger_filters.in_scope_only,
                                view.extra_tools.logger_filters.status_classes,
                                view.extra_tools.logger_filters.script_enabled,
                                view.extra_tools.logger_filters.view_filter_script.clone(),
                            )
                        };
                        let view_script_state = window.use_keyed_state(
                            "logger-view-script-editor-state",
                            cx,
                            |window, cx| {
                                LoggerScriptPopoverState::new(
                                    view_script.clone(),
                                    "例如：status == 200 && method == \"POST\"",
                                    window,
                                    cx,
                                )
                            },
                        );
                        if view_script_state.read(cx).synced != view_script {
                            let synced_next = view_script.clone();
                            view_script_state.update(cx, |st, cx| {
                                st.synced = synced_next.clone();
                                st.editor
                                    .update(cx, |ed, cx| ed.set_value(synced_next, window, cx));
                            });
                        }
                        let (view_editor, view_feedback) = {
                            let st = view_script_state.read(cx);
                            (st.editor.clone(), st.feedback.clone())
                        };
                        let o_all = view_owner.clone();
                        let o_proxy = view_owner.clone();
                        let o_repeater = view_owner.clone();
                        let o_intruder = view_owner.clone();
                        let o_scanner = view_owner.clone();
                        let o_plugin = view_owner.clone();
                        let o_scope = view_owner.clone();
                        let o_script = view_owner.clone();
                        let o_apply = view_owner.clone();
                        let o_clear = view_owner.clone();
                        let o_reset = view_owner.clone();
                        let vst_val = view_script_state.clone();
                        let vst_apply = view_script_state.clone();
                        let vst_clear = view_script_state.clone();
                        let vst_reset = view_script_state.clone();
                        div()
                            .id("logger-view-filter-content")
                            .test_support()
                            .w(px(410.))
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(
                                div()
                                    .text_size(theme::scaled_px(13.5))
                                    .font_weight(FontWeight::BOLD)
                                    .child("视图筛选设置 (View filter)"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_size(theme::scaled_px(12.))
                                            .text_color(theme::muted_foreground())
                                            .child("按工具来源筛选"),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .gap(px(6.))
                                            .child(
                                                button("logger-source-all", "全部来源")
                                                    .tooltip("显示所有工具模块产生的 HTTP 流量日志")
                                                    .selected(source_filter.is_none())
                                                    .on_click(move |_, window, cx| {
                                                        let _ = o_all.update(cx, |this, cx| {
                                                            this.set_logger_source_filter(
                                                                None, window, cx,
                                                            );
                                                        });
                                                    }),
                                            )
                                            .child(
                                                button("logger-source-proxy", "Proxy")
                                                    .tooltip("仅显示流经 Proxy 代理监听器的流量日志")
                                                    .selected(
                                                        source_filter == Some(TrafficSource::Proxy),
                                                    )
                                                    .on_click(move |_, window, cx| {
                                                        let _ = o_proxy.update(cx, |this, cx| {
                                                            this.set_logger_source_filter(
                                                                Some(TrafficSource::Proxy),
                                                                window,
                                                                cx,
                                                            );
                                                        });
                                                    }),
                                            )
                                            .child(
                                                button("logger-source-repeater", "Repeater")
                                                    .tooltip("仅显示由 Repeater 重放发出的流量日志")
                                                    .selected(
                                                        source_filter
                                                            == Some(TrafficSource::Repeater),
                                                    )
                                                    .on_click(move |_, window, cx| {
                                                        let _ = o_repeater.update(cx, |this, cx| {
                                                            this.set_logger_source_filter(
                                                                Some(TrafficSource::Repeater),
                                                                window,
                                                                cx,
                                                            );
                                                        });
                                                    }),
                                            )
                                            .child(
                                                button("logger-source-intruder", "Intruder")
                                                    .tooltip("仅显示由 Intruder 攻击任务发出的流量日志")
                                                    .selected(
                                                        source_filter
                                                            == Some(TrafficSource::Intruder),
                                                    )
                                                    .on_click(move |_, window, cx| {
                                                        let _ = o_intruder.update(cx, |this, cx| {
                                                            this.set_logger_source_filter(
                                                                Some(TrafficSource::Intruder),
                                                                window,
                                                                cx,
                                                            );
                                                        });
                                                    }),
                                            )
                                            .child(
                                                button("logger-source-scanner", "Scanner")
                                                    .tooltip(
                                                        "仅显示由主动扫描器 (Scanner) 发出的流量日志",
                                                    )
                                                    .selected(
                                                        source_filter
                                                            == Some(TrafficSource::Scanner),
                                                    )
                                                    .on_click(move |_, window, cx| {
                                                        let _ = o_scanner.update(cx, |this, cx| {
                                                            this.set_logger_source_filter(
                                                                Some(TrafficSource::Scanner),
                                                                window,
                                                                cx,
                                                            );
                                                        });
                                                    }),
                                            )
                                            .child(
                                                button("logger-source-plugin", "Extensions")
                                                    .tooltip("仅显示由扩展插件主动发起的 HTTP 流量日志")
                                                    .selected(matches!(
                                                        source_filter,
                                                        Some(TrafficSource::Plugin(_))
                                                    ))
                                                    .on_click(move |_, window, cx| {
                                                        let _ = o_plugin.update(cx, |this, cx| {
                                                            this.set_logger_source_filter(
                                                                Some(TrafficSource::Plugin(
                                                                    String::new(),
                                                                )),
                                                                window,
                                                                cx,
                                                            );
                                                        });
                                                    }),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .pt(px(6.))
                                    .border_t_1()
                                    .border_color(theme::border())
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .text_size(theme::scaled_px(12.))
                                            .text_color(theme::muted_foreground())
                                            .child("范围与脚本过滤"),
                                    )
                                    .child(
                                        logger_checkbox(
                                            "logger-in-scope-only",
                                            "仅显示目标范围内 (In scope only)",
                                            in_scope_only,
                                        )
                                        .on_change(move |state, _, _, cx| {
                                            let checked = state == CheckboxState::Checked;
                                            let _ = o_scope.update(cx, |this, cx| {
                                                this.extra_tools.logger_filters.in_scope_only =
                                                    checked;
                                                cx.notify();
                                            });
                                        }),
                                    )
                                    .child(
                                        logger_checkbox(
                                            "logger-script-toggle",
                                            "启用 脚本表达式筛选 (MioScript)",
                                            script_enabled,
                                        )
                                        .on_change(move |state, _, _, cx| {
                                            let checked = state == CheckboxState::Checked;
                                            let _ = o_script.update(cx, |this, cx| {
                                                this.extra_tools.logger_filters.script_enabled =
                                                    checked;
                                                cx.notify();
                                            });
                                        }),
                                    )
                                    .child(
                                        div()
                                            .text_size(theme::scaled_px(11.5))
                                            .text_color(theme::muted_foreground())
                                            .child("视图脚本（变量：status、method、url、host、path、query、length、request、response）"),
                                    )
                                    .child(
                                        div()
                                            .id("logger-view-script-input")
                                            .test_support()
                                            .w_full()
                                            .h(px(68.))
                                            .font_family(theme::mono_font_family())
                                            .text_size(theme::editor_font_size())
                                            .child(
                                                Textarea::new(&view_editor)
                                                    .size_full()
                                                    .bordered(true)
                                                    .aria_label("视图脚本编辑器"),
                                            ),
                                    )
                                    .when_some(view_feedback, |box_el, (ok, msg)| {
                                        box_el.child(
                                            div()
                                                .id("logger-view-script-feedback")
                                                .test_support()
                                                .text_size(theme::scaled_px(11.5))
                                                .text_color(if ok {
                                                    rgb(0x2e8b57).into()
                                                } else {
                                                    theme::danger_fg()
                                                })
                                                .child(msg),
                                        )
                                    })
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.))
                                            .child(
                                                button("logger-view-script-validate", "验证脚本")
                                                    .on_click(move |_, _, cx| {
                                                        let code = vst_val
                                                            .read(cx)
                                                            .editor
                                                            .read(cx)
                                                            .content()
                                                            .text()
                                                            .to_string();
                                                        let feedback = match table::validate_logger_script(&code) {
                                                            Ok(()) if code.trim().is_empty() => {
                                                                (true, "脚本为空：将显示全部视图条目。".to_owned())
                                                            }
                                                            Ok(()) => {
                                                                (true, "编译通过：视图脚本语法有效。".to_owned())
                                                            }
                                                            Err(err) => (false, format!("语法错误：{err}")),
                                                        };
                                                        vst_val.update(cx, |st, cx| {
                                                            st.feedback = Some(feedback);
                                                            cx.notify();
                                                        });
                                                    }),
                                            )
                                            .child(
                                                button("logger-view-script-apply", "应用脚本")
                                                    .primary()
                                                    .on_click(move |_, _, cx| {
                                                        let code = vst_apply
                                                            .read(cx)
                                                            .editor
                                                            .read(cx)
                                                            .content()
                                                            .text()
                                                            .to_string();
                                                        match table::validate_logger_script(&code) {
                                                            Ok(()) => {
                                                                let trimmed = code.trim().to_owned();
                                                                vst_apply.update(cx, |st, cx| {
                                                                    st.synced = trimmed.clone();
                                                                    st.feedback = Some((
                                                                        true,
                                                                        if trimmed.is_empty() {
                                                                            "已清除视图 脚本。".to_owned()
                                                                        } else {
                                                                            "已应用视图 脚本。".to_owned()
                                                                        },
                                                                    ));
                                                                    cx.notify();
                                                                });
                                                                let _ = o_apply.update(cx, |this, cx| {
                                                                    let non_empty = !trimmed.is_empty();
                                                                    this.extra_tools
                                                                        .logger_filters
                                                                        .view_filter_script = trimmed;
                                                                    if non_empty {
                                                                        this.extra_tools
                                                                            .logger_filters
                                                                            .script_enabled = true;
                                                                    }
                                                                    cx.notify();
                                                                });
                                                            }
                                                            Err(err) => {
                                                                vst_apply.update(cx, |st, cx| {
                                                                    st.feedback = Some((false, format!("未应用，语法错误：{err}")));
                                                                    cx.notify();
                                                                });
                                                            }
                                                        }
                                                    }),
                                            )
                                            .child(
                                                button("logger-view-script-clear", "清空")
                                                    .ghost()
                                                    .on_click(move |_, window, cx| {
                                                        vst_clear.update(cx, |st, cx| {
                                                            st.synced.clear();
                                                            st.feedback = Some((true, "已清空视图 脚本。".to_owned()));
                                                            st.editor.update(cx, |ed, cx| {
                                                                ed.set_value("", window, cx);
                                                            });
                                                            cx.notify();
                                                        });
                                                        let _ = o_clear.update(cx, |this, cx| {
                                                            this.extra_tools
                                                                .logger_filters
                                                                .view_filter_script
                                                                .clear();
                                                            this.extra_tools
                                                                .logger_filters
                                                                .script_enabled = false;
                                                            cx.notify();
                                                        });
                                                    }),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .pt(px(6.))
                                    .border_t_1()
                                    .border_color(theme::border())
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .text_size(theme::scaled_px(12.))
                                            .text_color(theme::muted_foreground())
                                            .child("按状态码显示"),
                                    )
                                    .child(div().flex().flex_wrap().gap(px(10.)).children(
                                        [
                                            (0usize, "logger-status-2xx", "2xx [成功]"),
                                            (1usize, "logger-status-3xx", "3xx [重定向]"),
                                            (2usize, "logger-status-4xx", "4xx [客户端错误]"),
                                            (3usize, "logger-status-5xx", "5xx [服务端错误]"),
                                        ]
                                        .into_iter()
                                        .map(|(idx, cb_id, lbl)| {
                                            let o_st = view_owner.clone();
                                            div().w(px(160.)).child(
                                                logger_checkbox(cb_id, lbl, status_classes[idx])
                                                    .on_change(move |state, _, _, cx| {
                                                        let checked =
                                                            state == CheckboxState::Checked;
                                                        let _ = o_st.update(cx, |this, cx| {
                                                            this.extra_tools
                                                                .logger_filters
                                                                .status_classes[idx] = checked;
                                                            cx.notify();
                                                        });
                                                    }),
                                            )
                                        }),
                                    )),
                            )
                            .child(
                                div()
                                    .pt(px(8.))
                                    .border_t_1()
                                    .border_color(theme::border())
                                    .flex()
                                    .justify_end()
                                    .child(
                                        button("logger-view-filter-reset", "重置筛选").on_click(
                                            move |_, window, cx| {
                                                vst_reset.update(cx, |st, cx| {
                                                    st.synced.clear();
                                                    st.feedback = None;
                                                    st.editor.update(cx, |ed, cx| {
                                                        ed.set_value("", window, cx);
                                                    });
                                                    cx.notify();
                                                });
                                                let _ = o_reset.update(cx, |this, cx| {
                                                    this.reset_logger_view_filters(window, cx);
                                                });
                                            },
                                        ),
                                    ),
                            )
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_logger_filter_row_2(
        &self,
        search_term: &str,
        view_summary: String,
        filter_bg: gpui_kit::Hsla,
        filter_hover_bg: gpui_kit::Hsla,
        filter_border: gpui_kit::Hsla,
        toolbar_icon_fg: gpui_kit::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view_filter_bar = self.render_logger_view_filter_bar(
            view_summary,
            filter_bg,
            filter_hover_bg,
            filter_border,
            cx,
        );
        let has_search = !search_term.is_empty();
        let search_box = div()
            .w(theme::scaled_px(244.))
            .min_w(theme::scaled_px(140.))
            .flex_shrink(1.0)
            .child(
                Input::new(&self.extra_tools.logger_search)
                    .id("logger-search-input")
                    .aria_label("筛选日志")
                    .small()
                    .h(theme::scaled_px(24.))
                    .bg(theme::surface())
                    .focus_bordered(false)
                    .suffix(
                        Button::new("logger-search-action")
                            .ghost()
                            .compact()
                            .p_0()
                            .size(theme::scaled_px(20.))
                            .accessibility_label(if has_search {
                                "清除搜索"
                            } else {
                                "搜索日志"
                            })
                            .tooltip(if has_search {
                                "清除当前 Logger 搜索条件"
                            } else {
                                "按关键字或 脚本表达式实时过滤日志"
                            })
                            .child(
                                Icon::new(if has_search {
                                    IconName::Close
                                } else {
                                    IconName::Search
                                })
                                .size(px(14.))
                                .text_color(toolbar_icon_fg),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if has_search {
                                    this.extra_tools
                                        .logger_search
                                        .update(cx, |input, cx| input.set_value("", window, cx));
                                }
                                cx.notify();
                            })),
                    ),
            );

        let options_owner = cx.weak_entity();
        let options_btn = Button::new("logger-options-menu")
            .ghost()
            .compact()
            .p_0()
            .size(theme::scaled_px(22.))
            .rounded(px(3.))
            .accessibility_label("Logger 选项")
            .tooltip("Logger 操作选项")
            .child(
                Icon::new(IconName::EllipsisVertical)
                    .size(px(16.))
                    .text_color(toolbar_icon_fg),
            )
            .dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, move |menu, window, cx| {
                let Some(entity) = options_owner.upgrade() else {
                    return menu;
                };
                let continuous_scroll = entity
                    .read(cx)
                    .extra_tools
                    .logger_filters
                    .continuous_scroll;
                let col_visible = entity
                    .read(cx)
                    .extra_tools
                    .logger_ui_table
                    .borrow()
                    .col_visible;
                let visible_count = col_visible.iter().filter(|&&v| v).count();
                let o_csv = options_owner.clone();
                let o_refresh = options_owner.clone();
                let o_scroll = options_owner.clone();
                let o_cols = options_owner.clone();
                let o_reset_layout = options_owner.clone();
                let o_clear = options_owner.clone();
                let layout_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
                    let sub = (0..table::LOGGER_COL_COUNT).fold(
                        sub.compact().min_w(px(180.)),
                        |sub, col_ix| {
                            let owner = o_cols.clone();
                            let checked = col_visible[col_ix];
                            let disabled = col_ix == 0 || (checked && visible_count <= 1);
                            sub.item(
                                PopupMenuItem::new(table::LOGGER_COL_NAMES[col_ix])
                                    .checked(checked)
                                    .disabled(disabled)
                                    .on_click(move |_, _, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.toggle_logger_column_visibility(col_ix, cx);
                                        });
                                    }),
                            )
                        },
                    );
                    sub.separator().item(
                        PopupMenuItem::new("恢复默认列布局 (Reset layout)").on_click(
                            move |_, _, cx| {
                                let _ = o_reset_layout.update(cx, |this, cx| {
                                    this.reset_logger_column_layout(cx);
                                });
                            },
                        ),
                    )
                });
                menu.compact()
                    .min_w(px(176.))
                    .item(PopupMenuItem::new("导出 CSV").on_click(move |_, window, cx| {
                        let _ = o_csv.update(cx, |this, cx| this.export_logger_csv(window, cx));
                    }))
                    .item(PopupMenuItem::new("刷新历史").on_click(move |_, window, cx| {
                        let _ =
                            o_refresh.update(cx, |this, cx| this.refresh_logger_history(window, cx));
                    }))
                    .item(
                        PopupMenuItem::new("连续自动滚屏")
                            .checked(continuous_scroll)
                            .on_click(move |_, _, cx| {
                                let _ = o_scroll.update(cx, |this, cx| {
                                    this.toggle_logger_continuous_scroll(cx);
                                });
                            }),
                    )
                    .separator()
                    .item(PopupMenuItem::submenu("表格列布局 (Table layout)", layout_submenu))
                    .separator()
                    .item(PopupMenuItem::new("清空日志").on_click(move |_, window, cx| {
                        let _ =
                            o_clear.update(cx, |this, cx| this.clear_logger_entries(window, cx));
                    }))
            });

        div()
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(theme::scaled_px(8.))
            .child(view_filter_bar)
            .child(search_box)
            .child(options_btn)
            .into_any_element()
    }

    #[inline(never)]
    fn render_logger_filter_header(&self, search_term: &str, cx: &mut Context<Self>) -> AnyElement {
        let dark = theme::is_dark();
        let filter_bg: gpui_kit::Hsla = rgb(if dark { 0x35383d } else { 0xeeeeee }).into();
        let filter_hover_bg: gpui_kit::Hsla = rgb(if dark { 0x3f434a } else { 0xe4e4e4 }).into();
        let filter_border: gpui_kit::Hsla = rgb(if dark { 0x454950 } else { 0xdfdfdf }).into();
        let toolbar_icon_fg: gpui_kit::Hsla = rgb(if dark { 0xdfe1e5 } else { 0x222222 }).into();

        let capture_summary = table::capture_filter_summary(
            self.extra_tools.logger_max_entries,
            &self.extra_tools.logger_filters,
        );
        let view_summary = table::view_filter_summary(
            &self.extra_tools.logger_source_filter,
            &self.extra_tools.logger_filters,
        );

        let row_1 = self.render_logger_filter_row_1(
            capture_summary,
            filter_bg,
            filter_hover_bg,
            filter_border,
            toolbar_icon_fg,
            cx,
        );
        let row_2 = self.render_logger_filter_row_2(
            search_term,
            view_summary,
            filter_bg,
            filter_hover_bg,
            filter_border,
            toolbar_icon_fg,
            cx,
        );

        div()
            .id("logger-filter-header")
            .test_support()
            .w_full()
            .flex_shrink_0()
            .bg(theme::surface())
            .px(theme::scaled_px(10.))
            .pt(theme::scaled_px(8.))
            .pb(theme::scaled_px(14.))
            .flex()
            .flex_col()
            .gap(theme::scaled_px(6.))
            .child(row_1)
            .child(row_2)
            .into_any_element()
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_logger(
        &self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let source_filter = self.extra_tools.logger_source_filter.clone();
        let search_term = self
            .extra_tools
            .logger_search
            .read(cx)
            .value()
            .trim()
            .to_string();

        let entries = &self.extra_tools.logger_entries;
        let entries_len = entries.len();
        let first_id = entries.first().map(|e| e.id);
        let last_id = entries.last().map(|e| e.id);
        let entries_sig = logger_entries_fingerprint(entries);
        let workbench_id = cx.entity_id();
        let filters = &self.extra_tools.logger_filters;
        let sort = self.extra_tools.logger_sort;
        let extended_sort = filters.extended_sort;
        let use_search_script = !search_term.is_empty()
            && (filters.script_enabled || table::is_script_expression(&search_term));
        let needs_notes_sig = (!search_term.is_empty() && !use_search_script)
            || matches!(extended_sort, Some((LoggerExtendedSortColumn::Comment, _)));
        let param_counts_sig = if matches!(
            extended_sort,
            Some((LoggerExtendedSortColumn::ParamCount, _))
        ) {
            (
                filters.param_counts.len(),
                filters
                    .param_counts
                    .values()
                    .copied()
                    .fold(0usize, usize::wrapping_add),
            )
        } else {
            (0, 0)
        };
        let live_notes_sig = if needs_notes_sig {
            let live_notes = self.history.read(cx).delegate().live_notes.borrow();
            logger_live_notes_fingerprint(&live_notes)
        } else {
            (0, 0)
        };

        let cached_rows = self
            .extra_tools
            .logger_visible_rows_cache
            .borrow()
            .as_ref()
            .filter(|entry| {
                let k = &entry.key;
                k.entries_len == entries_len
                    && k.first_id == first_id
                    && k.last_id == last_id
                    && k.entries_sig == entries_sig
                    && k.source_filter == source_filter
                    && k.search_term == search_term
                    && k.sort == sort
                    && k.extended_sort == extended_sort
                    && k.status_classes == filters.status_classes
                    && k.in_scope_only == filters.in_scope_only
                    && k.script_enabled == filters.script_enabled
                    && k.view_filter_script == filters.view_filter_script
                    && k.param_counts_sig == param_counts_sig
                    && k.live_notes_sig == live_notes_sig
                    && (!filters.in_scope_only || k.scope.as_ref() == Some(&self.workspace.scope))
            })
            .map(|entry| Rc::clone(&entry.rows));

        let rows_rc = if let Some(hit) = cached_rows {
            hit
        } else {
            let computed = {
                let live_notes = self.history.read(cx).delegate().live_notes.borrow();
                let visible_rows = table::filter_and_sort_logger(
                    entries,
                    &source_filter,
                    &search_term,
                    sort,
                    filters,
                    &self.workspace.scope,
                    &live_notes,
                );
                Rc::new(
                    visible_rows
                        .into_iter()
                        .map(|(number, entry)| (number, entry.id))
                        .collect::<Vec<_>>(),
                )
            };
            *self.extra_tools.logger_visible_rows_cache.borrow_mut() = Some(LoggerRowsCacheEntry {
                key: LoggerRowsCacheKey {
                    entries_len,
                    first_id,
                    last_id,
                    entries_sig,
                    source_filter: source_filter.clone(),
                    search_term: search_term.clone(),
                    sort,
                    extended_sort,
                    status_classes: filters.status_classes,
                    in_scope_only: filters.in_scope_only,
                    scope: filters.in_scope_only.then(|| self.workspace.scope.clone()),
                    script_enabled: filters.script_enabled,
                    view_filter_script: filters.view_filter_script.clone(),
                    param_counts_sig,
                    live_notes_sig,
                },
                rows: Rc::clone(&computed),
            });
            computed
        };
        let table_head_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xf1f1f1 }).into();
        let table_head_fg: gpui_kit::Hsla = rgb(if dark { 0xd4d4d4 } else { 0x141414 }).into();
        let table_head_hover: gpui_kit::Hsla = rgb(if dark { 0x2f3237 } else { 0xe6e6e6 }).into();
        let cell_px = theme::scaled_px(6.);
        let (selected_set, col_widths, col_visible) = self
            .extra_tools
            .logger_ui_table
            .borrow_mut()
            .snapshot(self.extra_tools.logger_selected, entries);
        let visible_cols: Vec<usize> = (0..table::LOGGER_COL_COUNT)
            .filter(|&col_ix| col_visible[col_ix])
            .collect();
        let last_visible_col = visible_cols.last().copied().unwrap_or(0);
        let total_table_w = theme::scaled_px(
            visible_cols
                .iter()
                .map(|&col_ix| col_widths[col_ix])
                .sum::<f32>()
                .max(260.),
        );
        let visible_ids: Rc<Vec<FlowId>> =
            Rc::new(rows_rc.iter().map(|&(_, flow_id)| flow_id).collect());
        let selected_set_rc = Rc::new(selected_set);

        let header_defs: [(&'static str, &'static str, &'static str, LoggerHeaderSort);
            table::LOGGER_COL_COUNT] = [
            (
                "logger-sort-number",
                "#",
                "按请求序号升序或降序排列",
                LoggerHeaderSort::Primary(LoggerSortColumn::Number),
            ),
            (
                "logger-sort-time",
                "Time",
                "按请求发起时间排序",
                LoggerHeaderSort::Extended(LoggerExtendedSortColumn::Time),
            ),
            (
                "logger-sort-source",
                "Tool",
                "按发起请求的工具来源模块排序",
                LoggerHeaderSort::Primary(LoggerSortColumn::Source),
            ),
            (
                "logger-sort-method",
                "Method",
                "按 HTTP 请求方法排序",
                LoggerHeaderSort::Primary(LoggerSortColumn::Method),
            ),
            (
                "logger-sort-host",
                "Host",
                "按请求主机名排序",
                LoggerHeaderSort::Extended(LoggerExtendedSortColumn::Host),
            ),
            (
                "logger-sort-path",
                "Path",
                "按请求路径排序",
                LoggerHeaderSort::Extended(LoggerExtendedSortColumn::Path),
            ),
            (
                "logger-sort-query",
                "Query",
                "按查询参数字符串排序",
                LoggerHeaderSort::Extended(LoggerExtendedSortColumn::Query),
            ),
            (
                "logger-sort-params",
                "Param count",
                "按请求参数数量排序",
                LoggerHeaderSort::Extended(LoggerExtendedSortColumn::ParamCount),
            ),
            (
                "logger-sort-status",
                "Status code",
                "按 HTTP 响应状态码排序",
                LoggerHeaderSort::Primary(LoggerSortColumn::Status),
            ),
            (
                "logger-sort-length",
                "Length",
                "按响应字节长度排序",
                LoggerHeaderSort::Primary(LoggerSortColumn::Length),
            ),
            (
                "logger-sort-timer",
                "Start response timer",
                "按响应耗时 (ms) 排序",
                LoggerHeaderSort::Extended(LoggerExtendedSortColumn::StartResponseTimer),
            ),
            (
                "logger-sort-comment",
                "Comment",
                "按备注内容排序",
                LoggerHeaderSort::Extended(LoggerExtendedSortColumn::Comment),
            ),
        ];

        let weak = cx.weak_entity();
        let header_cells: Vec<AnyElement> = visible_cols
            .iter()
            .map(|&col_ix| {
                let (id, title, tip, target) = header_defs[col_ix];
                let width = col_widths[col_ix];
                let is_last = col_ix == last_visible_col;
                let active_asc = match (extended_sort, target) {
                    (Some((ext_col, asc)), LoggerHeaderSort::Extended(col)) if ext_col == col => {
                        Some(asc)
                    }
                    (None, LoggerHeaderSort::Primary(col)) if sort.column == col => {
                        Some(sort.ascending)
                    }
                    _ => None,
                };
                let ctx_view = weak.clone();
                let ctx_rows = Rc::clone(&rows_rc);
                let fit_rows = Rc::clone(&rows_rc);
                let resize_handle = div()
                    .id(("logger-col-resize", col_ix))
                    .occlude()
                    .cursor_col_resize()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right_0()
                    .w(px(6.))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if event.click_count == 2 {
                                this.fit_logger_column_width(col_ix, &fit_rows, cx);
                            } else {
                                this.begin_logger_column_resize(
                                    col_ix,
                                    event.position.x.as_f32(),
                                    cx,
                                );
                            }
                        }),
                    )
                    .on_drag(
                        table::ResizeLoggerCol {
                            entity_id: workbench_id,
                            col_ix,
                        },
                        |drag, _, _, cx| {
                            cx.stop_propagation();
                            cx.new(|_| *drag)
                        },
                    );
                let cell = div()
                    .id(id)
                    .test_support()
                    .role(gpui_kit::accesskit::Role::ColumnHeader)
                    .aria_label(title)
                    .relative()
                    .h_full()
                    .px(cell_px)
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .overflow_hidden()
                    .cursor_pointer()
                    .when(!is_last, |c| c.border_r_1().border_color(theme::border()))
                    .hover(move |s| s.bg(table_head_hover))
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| match target {
                        LoggerHeaderSort::Primary(col) => this.toggle_logger_sort(col, cx),
                        LoggerHeaderSort::Extended(col) => {
                            this.toggle_logger_extended_sort(col, cx)
                        }
                    }))
                    .context_menu(move |menu, window, cx| {
                        table::build_logger_header_context_menu(
                            ctx_view.clone(),
                            col_ix,
                            Rc::clone(&ctx_rows),
                            col_visible,
                            menu,
                            window,
                            cx,
                        )
                    })
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(theme::ui_font_size())
                            .text_color(table_head_fg)
                            .child(title),
                    )
                    .when_some(active_asc, |c, asc| {
                        c.child(
                            Icon::new(if asc {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .size(theme::scaled_px(12.))
                            .text_color(table_head_fg)
                            .flex_shrink_0(),
                        )
                    })
                    .child(resize_handle);
                if is_last {
                    cell.min_w(theme::scaled_px(width))
                        .flex_1()
                        .into_any_element()
                } else {
                    cell.w(theme::scaled_px(width))
                        .flex_shrink_0()
                        .into_any_element()
                }
            })
            .collect();

        let header_bar = div()
            .id("logger-sort-bar")
            .test_support()
            .min_w(total_table_w)
            .w_full()
            .h(theme::table_header_height())
            .flex_shrink_0()
            .flex()
            .items_center()
            .bg(table_head_bg)
            .text_color(table_head_fg)
            .border_b_1()
            .border_color(theme::border())
            .on_drag_move::<table::ResizeLoggerCol>(cx.listener(
                |this, event: &DragMoveEvent<table::ResizeLoggerCol>, _, cx| {
                    let drag = *event.drag(cx);
                    if drag.entity_id == cx.entity_id() {
                        this.update_logger_column_resize(
                            drag.col_ix,
                            event.event.position.x.as_f32(),
                            cx,
                        );
                    }
                },
            ))
            .children(header_cells);

        let selected_id = self.extra_tools.logger_selected;
        let row_count = rows_rc.len();
        let row_even_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xfafafa }).into();
        let row_hover_bg: gpui_kit::Hsla = rgb(if dark { 0x26334a } else { 0xeeeeee }).into();
        let row_h = theme::scaled_px(21.);
        let zebra = theme::table_zebra_stripe();

        let list = uniform_list("logger-entries", row_count, move |range, _, cx| {
            let Some(owner) = weak.upgrade() else {
                return Vec::new();
            };
            let workbench = owner.read(cx);
            let entries = &workbench.extra_tools.logger_entries;
            let param_counts = &workbench.extra_tools.logger_filters.param_counts;
            let delegate = workbench.history.read(cx).delegate();
            let live_notes = delegate.live_notes.borrow();
            let live_highlights = delegate.live_highlights.borrow();
            let items = range
                .filter_map(|index| {
                    let &(number, flow_id) = rows_rc.get(index)?;
                    let entry = entries
                        .get(number.checked_sub(1)?)
                        .filter(|e| e.id == flow_id)
                        .or_else(|| entries.iter().find(|e| e.id == flow_id))?;
                    Some((index, number, entry))
                })
                .map(|(index, number, entry)| {
                    let flow_id = entry.id;
                    let selected = selected_set_rc.contains(&flow_id)
                        || (selected_set_rc.is_empty() && selected_id == Some(flow_id));
                    let parsed = table::parse_logger_url(&entry.url);
                    let time_text = crate::history::local_time(entry.started_unix_ms);
                    let tool_text = table::logger_source_label(&entry.source);
                    let param_text = table::entry_param_count(entry, param_counts).to_string();
                    let status_text = entry
                        .status
                        .map(|code| code.to_string())
                        .unwrap_or_default();
                    let length_text = if entry.status.is_some() || entry.response_bytes > 0 {
                        entry.response_bytes.to_string()
                    } else {
                        String::new()
                    };
                    let timer_text = if entry.status.is_some() || entry.elapsed_ms > 0 {
                        entry.elapsed_ms.to_string()
                    } else {
                        String::new()
                    };
                    let comment_text = table::entry_comment(flow_id, &live_notes);
                    let highlight_color = live_highlights
                        .get(&flow_id)
                        .map(|h| h.color)
                        .filter(|c| (1..=9).contains(c));
                    let row_bg = if selected {
                        theme::selected()
                    } else if let Some(hl) =
                        highlight_color.and_then(crate::history::highlight_background)
                    {
                        hl
                    } else if zebra && index % 2 == 1 {
                        row_even_bg
                    } else {
                        theme::surface()
                    };
                    let row_fg = if selected {
                        theme::foreground()
                    } else if let Some(hl) = highlight_color {
                        crate::history::highlight_foreground(hl)
                    } else {
                        theme::foreground()
                    };
                    let right_view = weak.clone();
                    let click_view = weak.clone();
                    let menu_view = weak.clone();
                    let row_visible_ids = Rc::clone(&visible_ids);
                    let row_url = entry.url.clone();
                    let mut row_cells = Vec::with_capacity(visible_cols.len());
                    for &col_ix in &visible_cols {
                        let text = match col_ix {
                            0 => number.to_string(),
                            1 => time_text.clone(),
                            2 => tool_text.clone(),
                            3 => entry.method.clone(),
                            4 => theme::truncate_cell_preview(&parsed.host),
                            5 => theme::truncate_cell_preview(&parsed.path),
                            6 => theme::truncate_cell_preview(&parsed.query),
                            7 => param_text.clone(),
                            8 => status_text.clone(),
                            9 => length_text.clone(),
                            10 => timer_text.clone(),
                            11 => theme::truncate_cell_preview(&comment_text),
                            _ => String::new(),
                        };
                        let w = col_widths[col_ix];
                        let cell = if col_ix == last_visible_col {
                            div()
                                .min_w(theme::scaled_px(w))
                                .flex_1()
                                .px(cell_px)
                                .truncate()
                                .child(text)
                        } else {
                            div()
                                .w(theme::scaled_px(w))
                                .flex_shrink_0()
                                .px(cell_px)
                                .truncate()
                                .child(text)
                        };
                        row_cells.push(cell);
                    }
                    div()
                        .id(("logger-row", index))
                        .test_support()
                        .aria_selected(selected)
                        .min_w(total_table_w)
                        .w_full()
                        .h(row_h)
                        .flex()
                        .items_center()
                        .text_size(theme::ui_font_size())
                        .text_color(row_fg)
                        .bg(row_bg)
                        .when(!selected && highlight_color.is_none(), |r| {
                            r.hover(move |s| s.bg(row_hover_bg))
                        })
                        .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                            let _ = right_view.update(cx, |this, cx| {
                                this.handle_logger_row_right_down(flow_id, window, cx);
                            });
                        })
                        .on_click(move |event: &ClickEvent, window, cx| {
                            let shift = event.modifiers().shift;
                            let toggle = event.modifiers().secondary(cx);
                            let _ = click_view.update(cx, |this, cx| {
                                this.handle_logger_row_click(
                                    flow_id,
                                    &row_visible_ids,
                                    shift,
                                    toggle,
                                    window,
                                    cx,
                                );
                            });
                        })
                        .context_menu(move |menu, window, cx| {
                            table::logger_row_menu(
                                menu_view.clone(),
                                flow_id,
                                row_url.clone(),
                                menu,
                                window,
                                cx,
                            )
                        })
                        .children(row_cells)
                })
                .collect::<Vec<_>>();
            drop(live_highlights);
            drop(live_notes);
            items
        })
        .track_scroll(&self.extra_tools.logger_filters.scroll_handle)
        .min_w(total_table_w)
        .w_full()
        .flex_1()
        .min_h_0()
        .bg(theme::surface());

        let sb_size = px(10.);
        let v_sb_width = px(11.);
        let h_sb_height = px(11.);
        let sb_track_bg: gpui_kit::Hsla = rgb(if dark { 0x2b2d30 } else { 0xf5f5f5 }).into();
        let sb_thumb_bg: gpui_kit::Hsla = rgb(if dark { 0x5a5d62 } else { 0xd4d4d4 }).into();
        let sb_edge: gpui_kit::Hsla = rgb(if dark { 0x35383d } else { 0xe8e8e8 }).into();

        let top_table = div()
            .id("logger-table-container")
            .relative()
            .size_full()
            .min_h_0()
            .bg(theme::surface())
            .border_t_1()
            .border_color(theme::border())
            .child(
                div()
                    .size_full()
                    .min_h_0()
                    .pr(v_sb_width)
                    .pb(h_sb_height)
                    .child(
                        div()
                            .id("logger-table-hscroll")
                            .size_full()
                            .min_h_0()
                            .overflow_x_scroll()
                            .track_scroll(&self.extra_tools.logger_filters.h_scroll_handle)
                            .child(
                                div()
                                    .h_full()
                                    .min_h_0()
                                    .min_w(total_table_w)
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .child(header_bar)
                                    .child(list),
                            ),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .w(v_sb_width)
                    .h(theme::table_header_height())
                    .bg(table_head_bg)
                    .border_l_1()
                    .border_b_1()
                    .border_color(theme::border()),
            )
            .child(
                div()
                    .id("logger-vertical-scrollbar")
                    .test_support()
                    .absolute()
                    .top(theme::table_header_height())
                    .right_0()
                    .bottom(h_sb_height)
                    .w(v_sb_width)
                    .bg(sb_track_bg)
                    .border_l_1()
                    .border_color(sb_edge)
                    .child(
                        Scrollbar::vertical(&self.extra_tools.logger_filters.scroll_handle)
                            .viewport_from_layout()
                            .mode(ScrollbarMode::Always)
                            .styles(move |styles| {
                                styles
                                    .track(|s| s.width(sb_size).bg(sb_track_bg))
                                    .track_hover(|s| s.width(sb_size).bg(sb_track_bg))
                                    .track_active(|s| s.width(sb_size).bg(sb_track_bg))
                                    .thumb(|s| {
                                        s.width(px(6.))
                                            .inset(px(2.))
                                            .radius(px(3.))
                                            .min_length(px(24.))
                                            .bg(sb_thumb_bg)
                                    })
                                    .thumb_hover(|s| {
                                        s.width(px(6.))
                                            .inset(px(2.))
                                            .radius(px(3.))
                                            .min_length(px(24.))
                                            .bg(sb_thumb_bg)
                                    })
                                    .thumb_active(|s| {
                                        s.width(px(6.))
                                            .inset(px(2.))
                                            .radius(px(3.))
                                            .min_length(px(24.))
                                            .bg(sb_thumb_bg)
                                    })
                            })
                            .max_fps(60),
                    ),
            )
            .child(
                div()
                    .id("logger-horizontal-scrollbar")
                    .test_support()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(h_sb_height)
                    .bg(sb_track_bg)
                    .border_t_1()
                    .border_color(sb_edge)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .right(v_sb_width)
                            .child(
                                Scrollbar::horizontal(
                                    &self.extra_tools.logger_filters.h_scroll_handle,
                                )
                                .viewport_from_layout()
                                .mode(ScrollbarMode::Always)
                                .styles(move |styles| {
                                    styles
                                        .track(|s| s.width(sb_size).bg(sb_track_bg))
                                        .track_hover(|s| s.width(sb_size).bg(sb_track_bg))
                                        .track_active(|s| s.width(sb_size).bg(sb_track_bg))
                                        .thumb(|s| {
                                            s.width(px(6.))
                                                .inset(px(2.))
                                                .radius(px(3.))
                                                .min_length(px(24.))
                                                .bg(sb_thumb_bg)
                                        })
                                        .thumb_hover(|s| {
                                            s.width(px(6.))
                                                .inset(px(2.))
                                                .radius(px(3.))
                                                .min_length(px(24.))
                                                .bg(sb_thumb_bg)
                                        })
                                        .thumb_active(|s| {
                                            s.width(px(6.))
                                                .inset(px(2.))
                                                .radius(px(3.))
                                                .min_length(px(24.))
                                                .bg(sb_thumb_bg)
                                        })
                                }),
                            ),
                    ),
            );

        let bottom_pane: AnyElement = if self.extra_tools.logger_selected.is_some() {
            div()
                .size_full()
                .min_h_0()
                .child(self.render_http_notes(
                    self.render_context_message_pair(
                        "logger",
                        &self.extra_tools.logger_request,
                        &self.extra_tools.logger_response,
                        true,
                        cx,
                    ),
                    cx,
                ))
                .into_any_element()
        } else {
            div()
                .id("logger-empty-detail")
                .test_support()
                .size_full()
                .min_h_0()
                .bg(theme::surface())
                .into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .child(self.render_logger_filter_header(&search_term, cx))
            .child(
                div().flex_1().min_h_0().child(
                    v_resizable("logger-main-vsplit")
                        .child(
                            resizable_panel()
                                .size(px(270.))
                                .size_range(px(110.)..px(800.))
                                .child(top_table),
                        )
                        .child(
                            resizable_panel()
                                .size_range(px(160.)..px(10000.))
                                .child(bottom_pane),
                        ),
                ),
            )
            .into_any_element()
    }
}
