use gpui_kit::accesskit::Role;
use gpui_kit::base::CheckboxState;
use gpui_kit::component::{
    Icon, IndexPath,
    menu::{DropdownMenu, PopupMenuItem},
    scroll::{Scrollbar, ScrollbarMode},
    select::Select,
};

use super::*;

pub(super) fn uniform_editor_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    disabled: bool,
) -> Button {
    let dark = theme::is_dark();
    let bg = rgb(if dark { 0x3c3f44 } else { 0xeaeaea });
    let border = rgb(if dark { 0x4c5057 } else { 0xd4d4d4 });
    let fg = rgb(if dark { 0xdfe1e5 } else { 0x000000 });
    user_mio_button(id, label, disabled)
        .bg(bg)
        .border_color(border)
        .text_color(fg)
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
}

const HEADER_ROW_HEIGHT_PX: f32 = 19.;
const HEADER_VISIBLE_ROWS: usize = 7;

impl Settings {
    pub(super) fn has_hidden_header_composition(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        editing::input_snapshot(&self.user_pages.hidden_req_header_input, window, cx).is_err()
            || editing::input_snapshot(&self.user_pages.hidden_resp_header_input, window, cx)
                .is_err()
    }

    pub(super) fn hidden_header_names(&self, response: bool) -> Vec<String> {
        let editor = &self.state.preferences.message_editor;
        let configured = if response {
            &editor.hidden_response_headers
        } else {
            &editor.hidden_request_headers
        };
        configured.clone().unwrap_or_else(|| {
            let defaults = if response {
                editing::DEFAULT_HIDDEN_RESPONSE_HEADERS
            } else {
                editing::DEFAULT_HIDDEN_REQUEST_HEADERS
            };
            defaults.iter().map(|name| (*name).to_owned()).collect()
        })
    }

    fn hidden_header_input(&self, response: bool) -> Entity<InputState> {
        if response {
            self.user_pages.hidden_resp_header_input.clone()
        } else {
            self.user_pages.hidden_req_header_input.clone()
        }
    }

    fn submit_hidden_header(&mut self, response: bool, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.hidden_header_input(response);
        let name = match editing::input_snapshot(&input, window, cx) {
            Ok(value) => value.to_string(),
            Err(error) => {
                self.user_pages.local_error = Some(error.into());
                cx.notify();
                return;
            }
        };
        if let Err(error) = editing::validate_http_header_names(std::slice::from_ref(&name)) {
            self.user_pages.local_error = Some(error);
            cx.notify();
            return;
        }
        let mut names = self.hidden_header_names(response);
        if names.iter().any(|existing| existing.eq_ignore_ascii_case(&name)) {
            self.user_pages.local_error = Some(format!("隐藏标头列表已包含“{name}”。"));
            cx.notify();
            return;
        }
        names.push(name);
        if self.save_hidden_headers(response, Some(names), window, cx) {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
    }

    fn save_hidden_headers(
        &mut self,
        response: bool,
        names: Option<Vec<String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        for input in [
            &self.user_pages.editor_font_size_input,
            &self.user_pages.hidden_req_header_input,
            &self.user_pages.hidden_resp_header_input,
        ] {
            if let Err(error) = editing::input_snapshot(input, window, cx) {
                self.user_pages.local_error = Some(error.into());
                cx.notify();
                return false;
            }
        }
        let mut editor = self.state.preferences.message_editor.clone();
        if response {
            editor.hidden_response_headers = names;
        } else {
            editor.hidden_request_headers = names;
        }
        if editor == self.state.preferences.message_editor {
            if response {
                self.user_pages.selected_hidden_resp_header = None;
            } else {
                self.user_pages.selected_hidden_req_header = None;
            }
            self.user_pages.local_error = None;
            cx.notify();
            return true;
        }
        let saved = self.submit_user_workbench(
            self.state.preferences.display.clone(),
            editor,
            self.state.preferences.misc.clone(),
            cx,
        );
        if saved {
            if response {
                self.user_pages.selected_hidden_resp_header = None;
            } else {
                self.user_pages.selected_hidden_req_header = None;
            }
        }
        saved
    }

    fn remove_hidden_header(&mut self, response: bool, window: &mut Window, cx: &mut Context<Self>) {
        let selected = if response {
            self.user_pages.selected_hidden_resp_header
        } else {
            self.user_pages.selected_hidden_req_header
        };
        let Some(index) = selected else { return };
        let mut names = self.hidden_header_names(response);
        if index >= names.len() {
            return;
        }
        names.remove(index);
        if self.save_hidden_headers(response, Some(names), window, cx) {
            let input = self.hidden_header_input(response);
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
    }

    #[inline(never)]
    fn render_hidden_headers_section(
        &self,
        response: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let enabled = self.user_settings_enabled();
        let dark = theme::is_dark();
        let prefix = if response {
            "settings-hidden-response"
        } else {
            "settings-hidden-request"
        };
        let selected = if response {
            self.user_pages.selected_hidden_resp_header
        } else {
            self.user_pages.selected_hidden_req_header
        };
        let checked = if response {
            self.state
                .preferences
                .message_editor
                .hide_response_headers_by_default
        } else {
            self.state
                .preferences
                .message_editor
                .hide_request_headers_by_default
        };
        let names = self.hidden_header_names(response);
        let row_h = theme::scaled_px(HEADER_ROW_HEIGHT_PX).max(px(HEADER_ROW_HEIGHT_PX));
        // Exact multiple of HEADER_VISIBLE_ROWS (7 rows) + 2px outer border so rows are never sliced.
        let list_h = row_h * (HEADER_VISIBLE_ROWS as f32) + px(2.);
        let border_col = rgb(if dark { 0x464a52 } else { 0xd4d4d4 });
        let zebra_bg = rgb(if dark { 0x282b30 } else { 0xf4f5f7 });
        let selected_bg = rgb(if dark { 0x2e436e } else { 0xd0def0 });
        let track_bg: gpui_kit::Hsla = rgb(if dark { 0x23262b } else { 0xf7f7f7 }).into();
        let thumb_bg: gpui_kit::Hsla = rgb(if dark { 0x525761 } else { 0xc9c9c9 }).into();
        let thumb_hover: gpui_kit::Hsla = rgb(if dark { 0x676d79 } else { 0xb0b0b0 }).into();
        let thumb_active: gpui_kit::Hsla = rgb(if dark { 0x7a818f } else { 0x999999 }).into();
        let caret_color = rgb(if dark { 0xdfe1e5 } else { 0x000000 });
        let list_scroll = window
            .use_keyed_state(
                SharedString::from(format!("{prefix}-list-scroll-handle")),
                cx,
                |_, _| ScrollHandle::new(),
            )
            .read(cx)
            .clone();

        let weak = cx.weak_entity();
        let default_checkbox = settings_checkbox(
            if response {
                "settings-hidden-response-default"
            } else {
                "settings-hidden-request-default"
            },
            "默认隐藏以下标头：",
            checked,
            enabled,
            &self.user_pages.hide_headers_default_focus[usize::from(response)],
            window,
        )
        .on_change(move |state, _, window, cx| {
            let _ = weak.update(cx, |this, cx| {
                this.submit_user_editor(
                    |editor| {
                        if response {
                            editor.hide_response_headers_by_default =
                                state == CheckboxState::Checked;
                        } else {
                            editor.hide_request_headers_by_default =
                                state == CheckboxState::Checked;
                        }
                    },
                    window,
                    cx,
                )
            });
        });

        let owner = cx.weak_entity();
        let gear_btn = user_gear_menu_button(
            SharedString::from(format!("{prefix}-settings")),
            "隐藏标头设置",
            !enabled,
        )
        .dropdown_menu(move |menu, _, _| {
            let owner = owner.clone();
            menu.item(PopupMenuItem::new("恢复默认设置").on_click(move |_, window, cx| {
                let _ = owner.update(cx, |this, cx| {
                    this.save_hidden_headers(response, None, window, cx);
                });
            }))
        });

        let mut list_rows = div()
            .id(SharedString::from(format!("{prefix}-list-scroll")))
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&list_scroll)
            .flex()
            .flex_col();

        for (index, name) in names.iter().enumerate() {
            let is_selected = selected == Some(index);
            let row_bg = if is_selected {
                selected_bg.into()
            } else if theme::table_zebra_stripe() && index % 2 == 1 {
                zebra_bg.into()
            } else {
                theme::surface()
            };
            list_rows = list_rows.child(
                div()
                    .id((
                        if response {
                            "settings-hidden-response-row"
                        } else {
                            "settings-hidden-request-row"
                        },
                        index,
                    ))
                    .test_support()
                    .role(Role::Row)
                    .aria_label(name.clone())
                    .aria_selected(is_selected)
                    .h(row_h)
                    .flex_shrink_0()
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .bg(row_bg)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if response {
                            this.user_pages.selected_hidden_resp_header = Some(index);
                        } else {
                            this.user_pages.selected_hidden_req_header = Some(index);
                        }
                        cx.notify();
                    }))
                    .child(div().truncate().child(name.clone())),
            );
        }

        let show_scrollbar = names.len() > HEADER_VISIBLE_ROWS;
        let list_box = div()
            .flex()
            .items_center()
            .child(
                div()
                    .id(SharedString::from(format!("{prefix}-list")))
                    .test_support()
                    .role(Role::List)
                    .aria_label(if response {
                        "隐藏响应头列表"
                    } else {
                        "隐藏请求头列表"
                    })
                    .w(theme::scaled_px(452.).max(px(452.)))
                    .h(list_h)
                    .border_1()
                    .border_color(border_col)
                    .rounded(px(0.))
                    .bg(theme::surface())
                    .overflow_hidden()
                    .flex()
                    .child(list_rows)
                    .when(show_scrollbar, |this| {
                        this.child(
                            div()
                                .relative()
                                .w(px(10.))
                                .h_full()
                                .flex_shrink_0()
                                .bg(track_bg)
                                .border_l_1()
                                .border_color(rgb(if dark { 0x353942 } else { 0xebebeb }))
                                .child(
                                    Scrollbar::vertical(&list_scroll)
                                        .id(SharedString::from(format!("{prefix}-list-vscroll")))
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
                                ),
                        )
                    }),
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
                            .data(USER_TABLE_CARET_RIGHT_SVG)
                            .size(px(9.))
                            .text_color(caret_color),
                    ),
            );

        div()
            .id(SharedString::from(format!("{prefix}-section")))
            .test_support()
            .w_full()
            .pt(px(12.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(7.))
            .child(user_section_heading(
                None,
                None,
                if response {
                    "隐藏响应头"
                } else {
                    "隐藏请求头"
                },
            ))
            .child(user_desc_row_with_icon(
                gear_btn,
                if response {
                    "使用以下设置控制是否自动隐藏指定的 HTTP 响应头。标头名称匹配忽略大小写。"
                } else {
                    "使用以下设置控制是否自动隐藏指定的 HTTP 请求头。标头名称匹配忽略大小写。"
                },
            ))
            .child(
                div()
                    .pl(px(29.))
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(6.))
                    .child(default_checkbox)
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(8.))
                            .child(
                                div()
                                    .w(px(92.))
                                    .flex_shrink_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(5.))
                                    .child(
                                        uniform_editor_button(
                                            SharedString::from(format!("{prefix}-remove")),
                                            "移除",
                                            !enabled || selected.is_none(),
                                        )
                                        .w(px(92.))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.remove_hidden_header(response, window, cx)
                                        })),
                                    )
                                    .child(
                                        uniform_editor_button(
                                            SharedString::from(format!("{prefix}-clear")),
                                            "清空",
                                            !enabled,
                                        )
                                        .w(px(92.))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.save_hidden_headers(
                                                response,
                                                Some(Vec::new()),
                                                window,
                                                cx,
                                            );
                                        })),
                                    ),
                            )
                            .child(list_box),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                uniform_editor_button(
                                    SharedString::from(format!("{prefix}-add")),
                                    "添加",
                                    !enabled,
                                )
                                .w(px(92.))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.submit_hidden_header(response, window, cx)
                                })),
                            )
                            .child(
                                div().w(theme::scaled_px(250.).max(px(250.))).child(
                                    Input::new(&self.hidden_header_input(response))
                                        .id(SharedString::from(format!("{prefix}-input")))
                                        .small()
                                        .disabled(!enabled),
                                ),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn submit_user_editor(
        &mut self,
        change: impl FnOnce(&mut UserMessageEditorSettings),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if editing::input_snapshot(&self.user_pages.editor_font_size_input, window, cx).is_err()
            || self.has_hidden_header_composition(window, cx)
        {
            self.user_pages.local_error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        }
        let mut editor = self.state.preferences.message_editor.clone();
        change(&mut editor);
        if editor == self.state.preferences.message_editor {
            return;
        }
        let display = self.state.preferences.display.clone();
        let misc = self.state.preferences.misc.clone();
        if self.submit_user_workbench(display, editor, misc, cx) {
            self.user_pages
                .editor_font_size_input
                .update(cx, |input, cx| {
                    input.set_value(
                        self.state
                            .preferences
                            .message_editor
                            .font_size_px
                            .to_string(),
                        window,
                        cx,
                    )
                });
        }
    }

    fn apply_user_editor_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = match editing::input_snapshot(&self.user_pages.editor_font_size_input, window, cx)
        {
            Ok(raw) => raw,
            Err(error) => {
                self.user_pages.local_error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let Ok(font_size_px) = raw.trim().parse::<u8>() else {
            self.user_pages.local_error =
                Some("消息编辑器等宽字号必须在 10 到 22 pt 之间。".into());
            cx.notify();
            return;
        };
        self.submit_user_editor(|editor| editor.font_size_px = font_size_px, window, cx);
    }

    fn step_user_editor_font(&mut self, delta: i16, window: &mut Window, cx: &mut Context<Self>) {
        if editing::input_snapshot(&self.user_pages.editor_font_size_input, window, cx).is_err() {
            self.user_pages.local_error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        }
        let current = i16::from(self.state.preferences.message_editor.font_size_px);
        let next = (current + delta).clamp(10, 22) as u8;
        self.submit_user_editor(|editor| editor.font_size_px = next, window, cx);
    }

    fn toggle_user_editor(
        &mut self,
        change: impl FnOnce(&mut UserMessageEditorSettings),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if editing::input_snapshot(&self.user_pages.editor_font_size_input, window, cx).is_err() {
            self.user_pages.local_error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        }
        self.submit_user_editor(change, window, cx);
    }

    pub(super) fn set_user_editor_specific_charset(
        &mut self,
        charset: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let trimmed = charset.trim().to_owned();
        if trimmed.is_empty()
            || self
                .state
                .preferences
                .message_editor
                .specific_charset
                .eq_ignore_ascii_case(&trimmed)
        {
            return;
        }
        self.toggle_user_editor(|editor| editor.specific_charset = trimmed, window, cx);
    }

    pub(super) fn reset_user_editor_defaults(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if editing::input_snapshot(&self.user_pages.editor_font_size_input, window, cx).is_err()
            || self.has_hidden_header_composition(window, cx)
        {
            self.user_pages.local_error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        }
        let defaults = UserMessageEditorSettings::default();
        let display = self.state.preferences.display.clone();
        let misc = self.state.preferences.misc.clone();
        if self.submit_user_workbench(display, defaults.clone(), misc, cx) {
            self.user_pages.charset_mode = defaults.charset_mode;
            self.user_pages.html_render_external_requests = defaults.html_render_external_requests;
            self.user_pages.search_regex_default = defaults.search_regex_default;
            self.user_pages.search_autoscroll_default = defaults.search_autoscroll_default;
            let cs_idx = IndexPath::new(charset_to_index(&defaults.specific_charset));
            self.user_pages
                .charset_select
                .update(cx, |s, cx| s.set_selected_index(Some(cs_idx), window, cx));
            self.user_pages.selected_hidden_req_header = None;
            self.user_pages.selected_hidden_resp_header = None;
            self.user_pages
                .hidden_req_header_input
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.user_pages
                .hidden_resp_header_input
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.user_pages
                .editor_font_size_input
                .update(cx, |input, cx| {
                    input.set_value(defaults.font_size_px.to_string(), window, cx)
                });
        }
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_user_message_editor(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let _ = (
            self.user_pages.charset_mode,
            self.user_pages.html_render_external_requests,
            self.user_pages.search_regex_default,
            self.user_pages.search_autoscroll_default,
        );
        let enabled = self.user_settings_enabled();
        let editor = &self.state.preferences.message_editor;
        let charset_mode = editor.charset_mode;

        // Section 1: Character sets
        let e_cs0 = cx.entity();
        let e_cs1 = cx.entity();
        let e_cs2 = cx.entity();
        let e_cs3 = cx.entity();
        let charset_section = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(user_section_heading(
                Some("settings-user-editor-heading"),
                Some("settings-user-editor-scope"),
                "字符集",
            ))
            .child(user_desc_row_with_icon(
                user_gear_static_icon(),
                "使用以下设置控制显示原始 HTTP 消息时对不同字符集的处理方式。部分字符集所需的字形并非所有字体均支持；如需使用扩展或非常见字符集，建议优先尝试等宽系统字体。",
            ))
            .child(
                div()
                    .id("settings-user-editor-charset-group")
                    .test_support()
                    .role(Role::RadioGroup)
                    .aria_label("字符集")
                    .pl(px(29.))
                    .pt(px(2.))
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(1.))
                    .child(
                        settings_radio(
                            "settings-user-editor-charset-auto",
                            "根据消息标头自动识别",
                            charset_mode == 0,
                            enabled,
                            window,
                            cx,
                        )
                        .min_h(theme::scaled_px(22.).max(px(21.)))
                        .py(px(1.))
                        .on_change(move |_, _, window, cx| {
                            e_cs0.update(cx, |this, cx| {
                                this.toggle_user_editor(|ed| ed.charset_mode = 0, window, cx);
                            });
                        }),
                    )
                    .child(
                        settings_radio(
                            "settings-user-editor-charset-default",
                            "使用平台默认字符集 (UTF-8)",
                            charset_mode == 1,
                            enabled,
                            window,
                            cx,
                        )
                        .min_h(theme::scaled_px(22.).max(px(21.)))
                        .py(px(1.))
                        .on_change(move |_, _, window, cx| {
                            e_cs1.update(cx, |this, cx| {
                                this.toggle_user_editor(|ed| ed.charset_mode = 1, window, cx);
                            });
                        }),
                    )
                    .child(
                        settings_radio(
                            "settings-user-editor-charset-raw",
                            "以原始字节显示",
                            charset_mode == 2,
                            enabled,
                            window,
                            cx,
                        )
                        .min_h(theme::scaled_px(22.).max(px(21.)))
                        .py(px(1.))
                        .on_change(move |_, _, window, cx| {
                            e_cs2.update(cx, |this, cx| {
                                this.toggle_user_editor(|ed| ed.charset_mode = 2, window, cx);
                            });
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                settings_radio(
                                    "settings-user-editor-charset-specific",
                                    "使用指定字符集：",
                                    charset_mode == 3,
                                    enabled,
                                    window,
                                    cx,
                                )
                                .min_h(theme::scaled_px(22.).max(px(21.)))
                                .py(px(1.))
                                .on_change(move |_, _, window, cx| {
                                    e_cs3.update(cx, |this, cx| {
                                        this.toggle_user_editor(
                                            |ed| ed.charset_mode = 3,
                                            window,
                                            cx,
                                        );
                                    });
                                }),
                            )
                            .child(
                                div()
                                    .w(theme::scaled_px(140.).max(px(140.)))
                                    .flex_shrink_0()
                                    .child(
                                        Select::new(&self.user_pages.charset_select)
                                            .id("settings-user-editor-charset-select")
                                            .small()
                                            .w_full()
                                            .disabled(!enabled),
                                    ),
                            ),
                    ),
            );

        // Section 2: HTML rendering
        let weak_html = cx.weak_entity();
        let html_section = div()
            .pt(px(12.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(user_section_heading(
                Some("settings-user-editor-html-heading"),
                None,
                "HTML 渲染",
            ))
            .child(user_desc_row_with_icon(
                user_gear_static_icon(),
                "使用以下设置控制工具内 HTML 内容的渲染行为。",
            ))
            .child(
                div().pl(px(29.)).child(
                    settings_checkbox(
                        "settings-user-editor-html-requests",
                        "发起额外 HTTP 请求以渲染外部资源",
                        editor.html_render_external_requests,
                        enabled,
                        &self.user_pages.html_render_focus,
                        window,
                    )
                    .on_change(move |state, _, window, cx| {
                        let _ = weak_html.update(cx, |this, cx| {
                            this.toggle_user_editor(
                                |ed| {
                                    ed.html_render_external_requests =
                                        state == CheckboxState::Checked
                                },
                                window,
                                cx,
                            );
                        });
                    }),
                ),
            );

        // Section 3: HTTP message search
        let weak_case = cx.weak_entity();
        let search_case = settings_checkbox(
            "settings-user-editor-search-case",
            "区分大小写",
            editor.highlight_search_case_sensitive_default,
            enabled,
            &self.user_pages.search_case_focus,
            window,
        )
        .on_change(move |state, _, window, cx| {
            let _ = weak_case.update(cx, |this, cx| {
                this.toggle_user_editor(
                    |editor| {
                        editor.highlight_search_case_sensitive_default =
                            state == CheckboxState::Checked
                    },
                    window,
                    cx,
                );
            });
        });
        let weak_regex = cx.weak_entity();
        let search_regex = settings_checkbox(
            "settings-user-editor-search-regex",
            "正则表达式",
            editor.search_regex_default,
            enabled,
            &self.user_pages.search_regex_focus,
            window,
        )
        .on_change(move |state, _, window, cx| {
            let _ = weak_regex.update(cx, |this, cx| {
                this.toggle_user_editor(
                    |ed| ed.search_regex_default = state == CheckboxState::Checked,
                    window,
                    cx,
                );
            });
        });
        let weak_scroll = cx.weak_entity();
        let search_autoscroll = settings_checkbox(
            "settings-user-editor-search-autoscroll",
            "文本变更时自动滚动到匹配项",
            editor.search_autoscroll_default,
            enabled,
            &self.user_pages.search_autoscroll_focus,
            window,
        )
        .on_change(move |state, _, window, cx| {
            let _ = weak_scroll.update(cx, |this, cx| {
                this.toggle_user_editor(
                    |ed| ed.search_autoscroll_default = state == CheckboxState::Checked,
                    window,
                    cx,
                );
            });
        });
        let search_section = div()
            .pt(px(9.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(user_section_heading(
                Some("settings-user-editor-search-heading"),
                None,
                "HTTP 消息搜索",
            ))
            .child(user_desc_row_with_icon(
                user_gear_static_icon(),
                "使用以下设置控制 HTTP 编辑器搜索栏的默认配置。",
            ))
            .child(
                div()
                    .pl(px(29.))
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(2.))
                    .child(search_case)
                    .child(search_regex)
                    .child(search_autoscroll),
            );

        // Section 6: HTTP message display (monospace font size & editor view defaults)
        let weak = cx.weak_entity();
        let soft_wrap = settings_checkbox(
            "settings-user-editor-soft-wrap",
            "自动软换行",
            editor.soft_wrap,
            enabled,
            &self.user_pages.soft_wrap_focus,
            window,
        )
        .on_change(move |state, _, window, cx| {
            let _ = weak.update(cx, |this, cx| {
                this.toggle_user_editor(
                    |editor| editor.soft_wrap = state == CheckboxState::Checked,
                    window,
                    cx,
                );
            });
        });
        let weak = cx.weak_entity();
        let pretty_default = settings_checkbox(
            "settings-user-editor-pretty-default",
            "新载入消息默认启用 Pretty 视图",
            editor.pretty_print_by_default,
            enabled,
            &self.user_pages.pretty_default_focus,
            window,
        )
        .on_change(move |state, _, window, cx| {
            let _ = weak.update(cx, |this, cx| {
                this.toggle_user_editor(
                    |editor| editor.pretty_print_by_default = state == CheckboxState::Checked,
                    window,
                    cx,
                );
            });
        });
        let weak = cx.weak_entity();
        let auto_inspector = settings_checkbox(
            "settings-user-editor-auto-inspector",
            "新载入消息默认展开 Inspector",
            editor.auto_expand_inspector,
            enabled,
            &self.user_pages.auto_inspector_focus,
            window,
        )
        .on_change(move |state, _, window, cx| {
            let _ = weak.update(cx, |this, cx| {
                this.toggle_user_editor(
                    |editor| editor.auto_expand_inspector = state == CheckboxState::Checked,
                    window,
                    cx,
                );
            });
        });

        let display_section = div()
            .pt(px(12.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(user_section_heading(
                Some("settings-user-editor-display-heading"),
                None,
                "HTTP 消息显示",
            ))
            .child(user_desc_row_with_icon(
                user_reset_icon_button(
                    "settings-user-editor-reset",
                    "将消息编辑器字号、自动换行、查找与隐藏标头恢复为用户级默认值",
                    !enabled,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.reset_user_editor_defaults(window, cx)
                })),
                "设置跨项目生效的消息编辑器等宽字号、软换行、Pretty 视图与 Inspector 默认行为。",
            ))
            .child(
                div()
                    .pl(px(29.))
                    .pt(px(2.))
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(4.))
                    .child(
                        div()
                            .pb(px(4.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().flex_shrink_0().child("等宽字号："))
                            .child(
                                user_mio_button(
                                    "settings-user-editor-font-dec",
                                    "−",
                                    !enabled || editor.font_size_px <= 10,
                                )
                                .tooltip("减小消息编辑器等宽字号 1 pt")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.step_user_editor_font(-1, window, cx)
                                })),
                            )
                            .child(
                                div().w(theme::scaled_px(58.).max(px(58.))).child(
                                    Input::new(&self.user_pages.editor_font_size_input)
                                        .id("settings-user-editor-font-input")
                                        .small()
                                        .disabled(!enabled),
                                ),
                            )
                            .child(
                                user_mio_button(
                                    "settings-user-editor-font-inc",
                                    "+",
                                    !enabled || editor.font_size_px >= 22,
                                )
                                .tooltip("增大消息编辑器等宽字号 1 pt")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.step_user_editor_font(1, window, cx)
                                })),
                            )
                            .child("pt")
                            .child(
                                user_mio_button(
                                    "settings-user-editor-font-apply",
                                    "应用",
                                    !enabled,
                                )
                                .tooltip("应用消息编辑器等宽字号设置")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.apply_user_editor_inputs(window, cx)
                                })),
                            ),
                    )
                    .child(soft_wrap)
                    .child(pretty_default)
                    .child(auto_inspector),
            );

        let body = div()
            .id("settings-user-editor-inner")
            .px(px(22.))
            .py(px(8.))
            .flex()
            .flex_col()
            .gap(px(6.));
        let body = self.append_user_pages_feedback(body, cx);
        let content = body
            .child(charset_section)
            .child(html_section)
            .child(search_section)
            .child(self.render_hidden_headers_section(false, window, cx))
            .child(self.render_hidden_headers_section(true, window, cx))
            .child(display_section);
        settings_scrollable_pane(
            "settings-user-editor-page",
            &self.user_pages.editor_scroll,
            content,
        )
        .into_any_element()
    }
}
