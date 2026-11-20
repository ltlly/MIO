use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonVariants;
use mio_core::IntruderResultFilter;

use super::*;

impl Render for ResultFilterWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title_bar = crate::chrome::render_title_bar(
            "mio-intruder-filter",
            "Intruder 结果筛选",
            crate::chrome::TitleBarControls::DIALOG,
            |window, _| window.remove_window(),
            window,
            cx,
        );
        let enabled = !self.submitting && self.pending.is_none();
        let content = div()
            .id("intruder-result-filter-dialog")
            .test_support()
            .track_focus(&self.focus)
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .border_1()
                    .border_color(theme::border())
                    .p(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child("按状态码筛选")
                    .child(
                        div().flex().flex_wrap().gap(px(20.)).children(
                            [
                                (0, "intruder-result-filter-2xx", "2xx [成功]"),
                                (1, "intruder-result-filter-3xx", "3xx [重定向]"),
                                (2, "intruder-result-filter-4xx", "4xx [请求错误]"),
                                (3, "intruder-result-filter-5xx", "5xx [服务器错误]"),
                            ]
                            .into_iter()
                            .map(|(index, id, label)| {
                                let owner = cx.weak_entity();
                                resource_pools::pool_checkbox(
                                    id,
                                    label,
                                    self.draft.status_classes[index],
                                    enabled,
                                )
                                .on_change(
                                    move |value, _, _, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.draft.status_classes[index] =
                                                value == gpui_kit::base::CheckboxState::Checked;
                                            cx.notify();
                                        });
                                    },
                                )
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .border_1()
                    .border_color(theme::border())
                    .p(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child("按注解筛选")
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(20.))
                            .child({
                                let owner = cx.weak_entity();
                                resource_pools::pool_checkbox(
                                    "intruder-result-filter-notes",
                                    "有备注",
                                    self.draft.only_with_notes,
                                    enabled,
                                )
                                .on_change(
                                    move |value, _, _, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.draft.only_with_notes =
                                                value == gpui_kit::base::CheckboxState::Checked;
                                            cx.notify();
                                        });
                                    },
                                )
                            })
                            .child({
                                let owner = cx.weak_entity();
                                resource_pools::pool_checkbox(
                                    "intruder-result-filter-highlighted",
                                    "已高亮",
                                    self.draft.only_highlighted,
                                    enabled,
                                )
                                .on_change(
                                    move |value, _, _, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.draft.only_highlighted =
                                                value == gpui_kit::base::CheckboxState::Checked;
                                            cx.notify();
                                        });
                                    },
                                )
                            })
                            .child({
                                let owner = cx.weak_entity();
                                resource_pools::pool_checkbox(
                                    "intruder-result-filter-grep-matched",
                                    "仅显示 Grep 命中项",
                                    self.draft.only_grep_matched,
                                    enabled,
                                )
                                .on_change(
                                    move |value, _, _, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.draft.only_grep_matched =
                                                value == gpui_kit::base::CheckboxState::Checked;
                                            cx.notify();
                                        });
                                    },
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .border_1()
                    .border_color(theme::border())
                    .p(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child("按搜索词筛选")
                    .child(Input::new(&self.term).small().disabled(!enabled))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(20.))
                            .child({
                                let owner = cx.weak_entity();
                                resource_pools::pool_checkbox(
                                    "intruder-result-filter-regex",
                                    "正则表达式",
                                    self.draft.regex,
                                    enabled,
                                )
                                .on_change(
                                    move |value, _, _, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.draft.regex =
                                                value == gpui_kit::base::CheckboxState::Checked;
                                            cx.notify();
                                        });
                                    },
                                )
                            })
                            .child({
                                let owner = cx.weak_entity();
                                resource_pools::pool_checkbox(
                                    "intruder-result-filter-case-sensitive",
                                    "区分大小写",
                                    self.draft.case_sensitive,
                                    enabled,
                                )
                                .on_change(
                                    move |value, _, _, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.draft.case_sensitive =
                                                value == gpui_kit::base::CheckboxState::Checked;
                                            cx.notify();
                                        });
                                    },
                                )
                            })
                            .child({
                                let owner = cx.weak_entity();
                                resource_pools::pool_checkbox(
                                    "intruder-result-filter-negative",
                                    "反向搜索",
                                    self.draft.negative_search,
                                    enabled,
                                )
                                .on_change(
                                    move |value, _, _, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.draft.negative_search =
                                                value == gpui_kit::base::CheckboxState::Checked;
                                            cx.notify();
                                        });
                                    },
                                )
                            }),
                    ),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("intruder-result-filter-error")
                        .test_support()
                        .role(gpui_kit::accesskit::Role::Alert)
                        .aria_label(error.clone())
                        .min_h(theme::scaled_px(34.))
                        .flex_shrink_0()
                        .line_height(theme::scaled_px(17.))
                        .line_clamp(2)
                        .overflow_hidden()
                        .text_color(rgb(0xb42318))
                        .child(error),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .flex_shrink_0()
                    .gap(px(8.))
                    .child(
                        button("intruder-result-filter-all", "显示全部")
                            .icon(IconName::Eye)
                            .tooltip("重置所有筛选条件并显示全部攻击结果")
                            .disabled(!enabled)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.restore(IntruderResultFilter::default(), window, cx);
                            })),
                    )
                    .child(
                        button("intruder-result-filter-hide-all", "隐藏全部")
                            .icon(IconName::EyeOff)
                            .tooltip("取消勾选全部 HTTP 状态码分类")
                            .disabled(!enabled)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.draft.status_classes = [false; 4];
                                this.error = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        button("intruder-result-filter-revert", "还原更改")
                            .icon(IconName::Undo2)
                            .tooltip("还原为当前已生效的筛选配置")
                            .disabled(!enabled)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.restore(this.applied.clone(), window, cx);
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        button("intruder-result-filter-cancel", "取消")
                            .tooltip("放弃筛选更改并关闭窗口")
                            .on_click(cx.listener(|this, _, window, cx| {
                                match editing::input_snapshot(&this.term, window, cx) {
                                    Ok(_) => window.remove_window(),
                                    Err(error) => {
                                        this.error = Some(error.into());
                                        cx.notify();
                                    }
                                }
                            })),
                    )
                    .child(
                        button("intruder-result-filter-apply", "应用")
                            .primary()
                            .icon(IconName::Check)
                            .tooltip("应用当前筛选条件并刷新结果列表")
                            .disabled(!enabled)
                            .on_click(cx.listener(|this, _, window, cx| this.apply(window, cx))),
                    ),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_size(theme::ui_font_size())
            .bg(theme::background())
            .text_color(theme::foreground())
            .child(title_bar)
            .child(div().flex_1().min_h_0().child(content))
    }
}
