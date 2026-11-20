use gpui_kit::component::{
    button::{ButtonRounded, ButtonVariants},
    input::EditorState,
    native_menu::NativeMenu,
    resizable::{h_resizable, resizable_panel},
};
use mio_core::RepeaterLayoutMode;

use super::*;

fn message_layout_icon(mode: RepeaterLayoutMode, selected: bool) -> AnyElement {
    let color = if selected {
        rgb(0xffffff).into()
    } else {
        theme::foreground()
    };
    match mode {
        RepeaterLayoutMode::SideBySide => div()
            .w(px(12.))
            .h(px(8.))
            .flex()
            .gap(px(2.))
            .child(div().w(px(5.)).h_full().bg(color))
            .child(div().w(px(5.)).h_full().bg(color)),
        RepeaterLayoutMode::TopBottom => div()
            .w(px(11.))
            .h(px(9.))
            .flex()
            .flex_col()
            .gap(px(1.))
            .child(div().w_full().h(px(4.)).bg(color))
            .child(div().w_full().h(px(4.)).bg(color)),
        RepeaterLayoutMode::Combined => div().w(px(11.)).h(px(8.)).bg(color),
    }
    .into_any_element()
}

fn save_repeater_entire_history_action() -> Box<dyn gpui_kit::Action> {
    Box::new(SaveRepeaterEntireHistory)
}

impl Workbench {
    pub(in crate::workbench) fn set_repeater_layout_mode(
        &mut self,
        project: Option<ProjectKey>,
        mode: RepeaterLayoutMode,
        cx: &mut Context<Self>,
    ) {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
            || self.workspace.repeater_settings.layout == mode
        {
            return;
        }
        self.workspace.repeater_settings.layout = mode;
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn render_message_pair_layout_controls(
        &self,
        owner: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let project = self.project;
        let is_repeater = owner == "repeater";
        let current = if is_repeater {
            self.workspace.repeater_settings.layout
        } else {
            self.message_pair_layout_for_owner(owner)
        };
        let disabled = is_repeater
            && (project.is_none() || !self.backend_available() || self.transition.is_some());
        let inactive_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x35383d } else { 0xdfdfdf }).into();
        div()
            .w(px(75.))
            .h(px(19.))
            .flex()
            .items_center()
            .gap_0()
            .children(
                [
                    (
                        SharedString::from(format!("{owner}-layout-side")),
                        "Horizontal layout",
                        "左右并排同时展示请求编辑器与响应视图（Side-by-side）",
                        RepeaterLayoutMode::SideBySide,
                    ),
                    (
                        SharedString::from(format!("{owner}-layout-stack")),
                        "Vertical layout",
                        "上下分栏堆叠展示请求编辑器与响应视图（Top / bottom）",
                        RepeaterLayoutMode::TopBottom,
                    ),
                    (
                        SharedString::from(format!("{owner}-layout-combined")),
                        "Combined view",
                        "单栏全宽视图，通过顶部子标签切换请求与响应（Tabs）",
                        RepeaterLayoutMode::Combined,
                    ),
                ]
                .into_iter()
                .map(|(id, label, tip, mode)| {
                    let is_cur = current == mode;
                    Button::new(id)
                        .ghost()
                        .small()
                        .rounded(ButtonRounded::None)
                        .when(mode == RepeaterLayoutMode::SideBySide, |btn| {
                            btn.rounded_tl(px(3.)).rounded_bl(px(3.))
                        })
                        .when(mode == RepeaterLayoutMode::Combined, |btn| {
                            btn.rounded_tr(px(3.)).rounded_br(px(3.))
                        })
                        .w(px(25.))
                        .h(px(19.))
                        .p_0()
                        .bg(if is_cur { rgb(0x26649d).into() } else { inactive_bg })
                        .text_color(if is_cur { rgb(0xffffff).into() } else { theme::foreground() })
                        .child(message_layout_icon(mode, is_cur))
                        .accessibility_label(label)
                        .tooltip(tip)
                        .tab_stop(false)
                        .selected(is_cur)
                        .toggled(is_cur)
                        .disabled(disabled)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if is_repeater {
                                this.set_repeater_layout_mode(project, mode, cx);
                            } else {
                                this.set_message_pair_layout_for_owner(owner, mode);
                                cx.notify();
                            }
                        }))
                }),
            )
            .into_any_element()
    }

    fn repeater_editor_context_menu(
        &self,
        input: Entity<EditorState>,
        readonly: bool,
    ) -> impl Fn(NativeMenu, &mut Window, &mut App) -> NativeMenu + 'static {
        self.request_context_menu_with_options(
            input,
            readonly,
            Some(save_repeater_entire_history_action),
        )
    }

    fn render_repeater_response_editor_body(
        &self,
        response: &MessageEditor,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if response.render_active {
            return self.render_response_visual_card("repeater-response", response, cx);
        }
        editing::message_editor(response.state())
            .readonly(true)
            .size_full()
            .context_menu(self.repeater_editor_context_menu(response.state().clone(), true))
            .aria_label("响应消息编辑器")
            .into_any_element()
    }

    pub(super) fn render_repeater_native_pane(
        &self,
        tab_id: TabId,
        editors: &TabEditors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.workspace.repeater_settings.layout {
            RepeaterLayoutMode::SideBySide => self.render_repeater_side_by_side(editors, cx),
            RepeaterLayoutMode::TopBottom => self.render_repeater_top_bottom(editors, cx),
            RepeaterLayoutMode::Combined => {
                self.render_repeater_combined(tab_id, editors, window, cx)
            }
        }
    }

    fn render_repeater_side_by_side(
        &self,
        editors: &TabEditors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let request = &editors.request;
        let response = &editors.response;
        let replace_readonly = self.is_message_replace_readonly("repeater-request");
        let req_readonly = request.is_readonly_preview();
        let request_pane = div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(self.message_header("repeater-request", "Request", request.view, cx))
            .child(
                div().flex_1().min_h_0().child(
                    editing::message_editor(request.state())
                        .readonly(req_readonly)
                        .size_full()
                        .context_menu(
                            self.repeater_editor_context_menu(request.state().clone(), req_readonly),
                        )
                        .aria_label("请求消息编辑器"),
                ),
            )
            .child(request.render_search_bar_editable(
                "repeater-request",
                replace_readonly || request.show_non_printable,
                cx,
                |this, action, window, cx| {
                    this.handle_message_search_action("repeater-request", action, window, cx);
                },
            ));
        let response_pane = div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(self.message_header("repeater-response", "Response", response.view, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_repeater_response_editor_body(response, cx)),
            )
            .child(response.render_search_bar(
                "repeater-response",
                cx,
                |this, action, window, cx| {
                    this.handle_message_search_action("repeater-response", action, window, cx);
                },
            ));
        div()
            .size_full()
            .flex()
            .child(
                div().flex_1().min_w_0().child(
                    h_resizable("repeater-hsplit")
                        .child(
                            resizable_panel()
                                .size_range(px(160.)..px(10000.))
                                .child(request_pane),
                        )
                        .child(
                            resizable_panel()
                                .size_range(px(160.)..px(10000.))
                                .child(response_pane),
                        ),
                ),
            )
            .into_any_element()
    }

    fn render_repeater_top_bottom(
        &self,
        editors: &TabEditors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let request = &editors.request;
        let response = &editors.response;
        let replace_readonly = self.is_message_replace_readonly("repeater-request");
        let req_readonly = request.is_readonly_preview();
        div()
            .size_full()
            .flex()
            .child(
                div()
                    .id("repeater-stack-container")
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .border_b_1()
                            .border_color(theme::border())
                            .child(self.message_header(
                                "repeater-request",
                                "Request",
                                request.view,
                                cx,
                            ))
                            .child(
                                div().flex_1().min_h_0().child(
                                    editing::message_editor(request.state())
                                        .readonly(req_readonly)
                                        .size_full()
                                        .context_menu(self.repeater_editor_context_menu(
                                            request.state().clone(),
                                            req_readonly,
                                        ))
                                        .aria_label("请求消息编辑器"),
                                ),
                            )
                            .child(request.render_search_bar_editable(
                                "repeater-request",
                                replace_readonly || request.show_non_printable,
                                cx,
                                |this, action, window, cx| {
                                    this.handle_message_search_action(
                                        "repeater-request",
                                        action,
                                        window,
                                        cx,
                                    );
                                },
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .child(self.message_header(
                                "repeater-response",
                                "Response",
                                response.view,
                                cx,
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .child(self.render_repeater_response_editor_body(
                                        response,
                                        cx,
                                    )),
                            )
                            .child(response.render_search_bar(
                                "repeater-response",
                                cx,
                                |this, action, window, cx| {
                                    this.handle_message_search_action(
                                        "repeater-response",
                                        action,
                                        window,
                                        cx,
                                    );
                                },
                            )),
                    ),
            )
            .into_any_element()
    }

    pub(in crate::workbench) fn sync_repeater_combined_state(
        &self,
        tab_id: TabId,
        editors: &TabEditors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<CombinedTabState> {
        let state = window.use_keyed_state(
            SharedString::from(format!("repeater-combined-state-{tab_id}")),
            cx,
            |_, _| CombinedTabState {
                pane: RepeaterCombinedPane::Request,
                observed_response_revision: editors.response_revision,
            },
        );
        if state.read(cx).observed_response_revision != editors.response_revision {
            let next_rev = editors.response_revision;
            state.update(cx, |s, _| {
                if next_rev.is_some() {
                    s.pane = RepeaterCombinedPane::Response;
                    editors.combined_pane.set(RepeaterCombinedPane::Response);
                }
                s.observed_response_revision = next_rev;
            });
        } else if state.read(cx).pane != editors.combined_pane.get() {
            let current = editors.combined_pane.get();
            state.update(cx, |s, _| {
                s.pane = current;
            });
        }
        editors.combined_pane.set(state.read(cx).pane);
        state
    }

    fn render_repeater_combined(
        &self,
        tab_id: TabId,
        editors: &TabEditors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let _state = self.sync_repeater_combined_state(tab_id, editors, window, cx);
        let active_pane = editors.combined_pane.get();
        let request = &editors.request;
        let response = &editors.response;
        let replace_readonly = self.is_message_replace_readonly("repeater-request");
        let req_readonly = request.is_readonly_preview();
        let body = match active_pane {
            RepeaterCombinedPane::Request => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.message_header("repeater-request", "Request", request.view, cx))
                .child(
                    div().flex_1().min_h_0().child(
                        editing::message_editor(request.state())
                            .readonly(req_readonly)
                            .size_full()
                            .context_menu(self.repeater_editor_context_menu(
                                request.state().clone(),
                                req_readonly,
                            ))
                            .aria_label("请求消息编辑器"),
                    ),
                )
                .child(request.render_search_bar_editable(
                    "repeater-request",
                    replace_readonly || request.show_non_printable,
                    cx,
                    |this, action, window, cx| {
                        this.handle_message_search_action("repeater-request", action, window, cx);
                    },
                ))
                .into_any_element(),
            RepeaterCombinedPane::Response => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(self.message_header("repeater-response", "Response", response.view, cx))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(self.render_repeater_response_editor_body(response, cx)),
                )
                .child(response.render_search_bar(
                    "repeater-response",
                    cx,
                    |this, action, window, cx| {
                        this.handle_message_search_action("repeater-response", action, window, cx);
                    },
                ))
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .child(
                div()
                    .id("repeater-combined-container")
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(body),
            )
            .into_any_element()
    }
}
