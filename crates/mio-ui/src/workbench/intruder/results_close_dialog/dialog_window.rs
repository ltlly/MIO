use gpui_kit::accesskit::Role;
use gpui_kit::base::{Button, Checkbox, CheckboxIndicator, CheckboxState};
use gpui_kit::component::{Icon, IconName};
use gpui_kit::size;
use std::rc::Rc;

use super::*;

pub(in crate::workbench::intruder) struct CloseDialog {
    pub(super) owner: WeakEntity<Workbench>,
    pub(super) owner_window: AnyWindowHandle,
    pub(super) operation: Uuid,
    pub(super) terminal: bool,
    pub(super) can_save: bool,
    pub(super) remember: bool,
    pub(super) pending: bool,
    pub(super) error: Option<String>,
    pub(super) answer_revision: u64,
    pub(super) focus: FocusHandle,
    pub(super) remember_focus: FocusHandle,
    pub(super) button_focus: Vec<FocusHandle>,
    pub(super) on_answer: Rc<AnswerHandler>,
    pub(super) _subscriptions: Vec<Subscription>,
}

impl CloseDialog {
    pub(super) fn answer(
        &mut self,
        answer: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if answer.is_some() && self.pending {
            return;
        }
        let Some(revision) = self.answer_revision.checked_add(1) else {
            return;
        };
        self.answer_revision = revision;
        self.pending = true;
        self.error = None;
        self.focus.focus(window, cx);
        let selection = answer.map(|answer| (answer, self.remember));
        let operation = self.operation;
        let owner = self.owner.clone();
        let owner_window = self.owner_window;
        let dialog_window = window.window_handle();
        let dialog = cx.weak_entity();
        let callback = self.on_answer.clone();
        cx.defer(move |cx| {
            if !cx.windows().contains(&dialog_window)
                || !dialog.upgrade().is_some_and(|dialog| {
                    let dialog = dialog.read(cx);
                    dialog.operation == operation
                        && dialog.answer_revision == revision
                        && dialog.pending
                })
            {
                return;
            }
            let result = owner_window.update(cx, |_, _, cx| {
                owner.update(cx, |owner, cx| callback(owner, operation, selection, cx))
            });
            let answer = match result {
                Ok(Ok(result)) => result,
                _ if owner.upgrade().is_none() || !cx.windows().contains(&owner_window) => Ok(true),
                _ => Err("无法交付关闭选择；结果仍保留，请重试或取消。".into()),
            };
            let _ = dialog_window.update(cx, |_, window, cx| {
                let _ = dialog.update(cx, |dialog, cx| {
                    if dialog.operation != operation || dialog.answer_revision != revision {
                        return;
                    }
                    match answer {
                        Ok(true) => window.remove_window(),
                        Ok(false) => cx.notify(),
                        Err(error) => {
                            dialog.set_error(error, window, cx);
                            dialog.pending = false;
                            cx.notify();
                        }
                    }
                });
            });
        });
        cx.notify();
    }

    fn set_error(&mut self, error: String, window: &mut Window, cx: &mut Context<Self>) {
        let scale = theme::ui_scale();
        let base_w = if self.terminal {
            if self.can_save { 614. } else { 452. }
        } else {
            538.
        };
        let mut width = if scale > 1.0 {
            theme::scaled_px(base_w)
        } else {
            px(base_w)
        };
        if let Some(display) = window.display(cx) {
            width = width.min(display.visible_bounds().size.width);
        }
        let text: SharedString = error.clone().into();
        let run = window.text_style().to_run(text.len());
        let font_size = theme::ui_font_size();
        let line_h = theme::scaled_px(18.).max(px(18.));
        let lines = window
            .text_system()
            .shape_text(
                text,
                font_size,
                &[run],
                Some((width - theme::scaled_px(77.)).max(px(120.))),
                None,
            )
            .map(|lines| {
                lines
                    .iter()
                    .map(|line| line.wrap_boundaries.len() + 1)
                    .sum::<usize>()
            })
            .unwrap_or(2);
        let minimum = if scale > 1.0 {
            theme::scaled_px(if self.terminal { 184. } else { 194. })
        } else {
            px(if self.terminal { 166. } else { 176. })
        };
        let available = window.display(cx).map_or(minimum, |display| {
            display.visible_bounds().bottom() - window.window_bounds().get_bounds().origin.y
        });
        let base_chrome = if scale > 1.0 {
            theme::scaled_px(133.)
        } else {
            px(133.)
        };
        let height = (base_chrome + line_h * lines as f32)
            .max(minimum)
            .min(available.max(minimum));
        window.resize(size(width, height));
        self.error = Some(error);
    }

    fn choice(
        &self,
        index: usize,
        label: &'static str,
        tip: &'static str,
        primary: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Button {
        let focus = &self.button_focus[index];
        let background = if primary {
            theme::accent()
        } else {
            theme::surface()
        };
        let hover_bg: gpui_kit::Hsla = if primary {
            rgb(if theme::is_dark() { 0xe56e3d } else { 0xc45122 }).into()
        } else {
            rgb(if theme::is_dark() { 0x3e4247 } else { 0xf0f3f7 }).into()
        };
        Button::new(("intruder-close-choice", index))
            .accessibility_label(label)
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
            })
            .track_focus(focus)
            .disabled(self.pending)
            .min_h(theme::scaled_px(26.).max(px(26.)))
            .py(px(3.))
            .min_w(theme::scaled_px(if !self.terminal {
                if index == 0 { 89. } else { 234. }
            } else if index == 2 {
                89.
            } else {
                138.
            }))
            .px(px(14.))
            .rounded(px(4.))
            .border_1()
            .border_color(if focus.is_focused(window) {
                theme::accent()
            } else if primary {
                background
            } else {
                theme::control_border()
            })
            .bg(background)
            .hover(move |style| style.bg(hover_bg))
            .text_size(theme::ui_font_size())
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .text_color(if primary {
                rgb(0xffffff).into()
            } else {
                theme::foreground()
            })
            .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
            .child(label)
            .on_click(cx.listener(move |this, _, window, cx| this.answer(Some(index), window, cx)))
    }
}

impl Render for CloseDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.owner.upgrade().is_none() {
            window.remove_window();
            return div().into_any_element();
        }
        let weak = cx.weak_entity();
        let mut footer = div()
            .min_h(theme::scaled_px(53.).max(px(53.)))
            .py(px(8.))
            .flex_shrink_0()
            .px(px(17.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(10.))
            .border_t_1()
            .border_color(theme::border())
            .child(
                Checkbox::new("intruder-close-remember")
                    .accessibility_label("记住我的选择")
                    .checked(self.remember)
                    .disabled(self.pending)
                    .track_focus(&self.remember_focus)
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(4.))
                    .when(self.remember_focus.is_focused(window), |this| {
                        this.bg(theme::selected())
                    })
                    .child(
                        CheckboxIndicator::new()
                            .checked(self.remember)
                            .size(theme::scaled_px(14.).max(px(14.)))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(if self.remember {
                                theme::accent()
                            } else {
                                theme::control_border()
                            })
                            .bg(if self.remember {
                                theme::accent()
                            } else {
                                theme::surface()
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(self.remember, |this| {
                                this.child(
                                    Icon::new(IconName::Check)
                                        .size(theme::scaled_px(11.).max(px(11.)))
                                        .text_color(rgb(0xffffff)),
                                )
                            }),
                    )
                    .child("记住我的选择")
                    .on_change(move |state, _, _, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            this.remember = state == CheckboxState::Checked;
                            cx.notify();
                        });
                    }),
            )
            .child(div().flex_1());
        if self.terminal {
            if self.can_save {
                footer = footer.child(self.choice(
                    0,
                    "保存到项目文件",
                    "将此攻击的配置与响应结果持久化保存到当前磁盘项目文件并关闭窗口",
                    true,
                    window,
                    cx,
                ));
            }
            footer = footer
                .child(self.choice(
                    1,
                    "保留在内存",
                    "将此攻击保留在当前会话内存中（可从 Dashboard 继续查看，退出 MIO 后清除）",
                    !self.can_save,
                    window,
                    cx,
                ))
                .child(self.choice(
                    2,
                    "丢弃",
                    "永久丢弃此攻击任务及其独占流量结果并关闭窗口",
                    false,
                    window,
                    cx,
                ));
        } else {
            footer = footer
                .child(self.choice(
                    0,
                    "丢弃",
                    "停止正在运行的攻击并丢弃其独占流量结果",
                    false,
                    window,
                    cx,
                ))
                .child(self.choice(
                    1,
                    "在后台继续攻击",
                    "关闭结果窗口并让此攻击在后台继续执行（可随时从 Dashboard 打开查看）",
                    true,
                    window,
                    cx,
                ));
        }
        div()
            .id("intruder-close-dialog")
            .test_support()
            .track_focus(&self.focus)
            .role(Role::Dialog)
            .aria_label(if self.terminal {
                "保存攻击？"
            } else {
                "继续攻击？"
            })
            .size_full()
            .overflow_hidden()
            .rounded(px(6.))
            .flex()
            .flex_col()
            .font_family(".SystemUIFont")
            .text_size(theme::ui_font_size())
            .text_color(theme::foreground())
            .bg(theme::background())
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape"
                        || (event.keystroke.modifiers.secondary(cx) && event.keystroke.key == "w")
                    {
                        this.answer(None, window, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .child(div().h(px(4.)).flex_shrink_0().bg(theme::accent()))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px(px(20.))
                    .py(px(20.))
                    .flex()
                    .gap(px(15.))
                    .child(
                        Icon::new(IconName::Info)
                            .size(theme::scaled_px(22.))
                            .text_color(theme::accent()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(
                                div()
                                    .text_size(theme::scaled_px(19.))
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .child(if self.terminal {
                                        "保存攻击？"
                                    } else {
                                        "继续攻击？"
                                    }),
                            )
                            .child(match &self.error {
                                Some(error) => div()
                                    .id("intruder-close-dialog-error")
                                    .test_support()
                                    .role(Role::Alert)
                                    .aria_label(error.clone())
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .line_height(theme::scaled_px(18.).max(px(18.)))
                                    .text_color(theme::danger_fg())
                                    .child(error.clone())
                                    .into_any_element(),
                                None => div()
                                    .id("intruder-close-dialog-message")
                                    .test_support()
                                    .line_height(theme::scaled_px(18.).max(px(18.)))
                                    .child(if self.pending {
                                        "正在确认关闭选择…"
                                    } else if self.terminal {
                                        "保留在内存的攻击可从 Dashboard 访问，直到关闭 MIO。"
                                    } else {
                                        "是否让 Intruder 在后台继续运行此攻击？\n可从 Dashboard 查看进度和结果。"
                                    })
                                    .into_any_element(),
                            }),
                    ),
            )
            .child(footer)
            .into_any_element()
    }
}
