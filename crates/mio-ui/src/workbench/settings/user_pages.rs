use gpui_kit::accesskit::Role;
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::component::{
    Icon, IndexPath,
    button::ButtonVariants,
    select::{SelectEvent, SelectState},
};
use mio_core::{UserDisplaySettings, UserMessageEditorSettings, UserMiscSettings, UserThemeMode};

use super::*;

mod display;
mod editor_misc;
#[cfg(test)]
mod tests;

pub(super) const UI_FONT_SIZE_OPTIONS: [&str; 8] = ["11", "12", "13", "14", "15", "16", "17", "18"];
pub(super) const SCALE_FACTOR_OPTIONS: [&str; 9] = [
    "0.8", "0.9", "1.0", "1.1", "1.2", "1.25", "1.3", "1.4", "1.5",
];
pub(super) const SCALE_FACTOR_PERCENTS: [u16; 9] = [80, 90, 100, 110, 120, 125, 130, 140, 150];
pub(super) const SEND_HOTKEY_LABELS: [&str; 3] = ["⌘Enter", "⌃Enter", "⌘⇧Enter"];
pub(super) const CHARSET_OPTIONS: [&str; 8] = [
    "UTF-8",
    "Big5",
    "GBK",
    "GB18030",
    "ISO-8859-1",
    "Shift_JIS",
    "EUC-KR",
    "US-ASCII",
];
pub(super) const USER_TABLE_CARET_RIGHT_SVG: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><path fill="currentColor" d="M2 1.5 L8.5 5 L2 8.5 Z"/></svg>"#;

pub(super) fn font_size_to_index(px: u8) -> usize {
    usize::from(px.clamp(11, 18) - 11)
}

pub(super) fn scale_percent_to_index(percent: u16) -> usize {
    SCALE_FACTOR_PERCENTS
        .iter()
        .enumerate()
        .min_by_key(|(_, p)| (**p).abs_diff(percent))
        .map_or(2, |(idx, _)| idx)
}

pub(super) fn scale_label_to_percent(label: &str) -> u16 {
    SCALE_FACTOR_OPTIONS
        .iter()
        .position(|opt| *opt == label)
        .map_or(100, |idx| SCALE_FACTOR_PERCENTS[idx])
}

pub(super) fn hotkey_key_to_index(key: &str) -> usize {
    match key.trim() {
        "ctrl-enter" => 1,
        "cmd-shift-enter" => 2,
        _ => 0,
    }
}

pub(super) struct UserPagesUi {
    pub(super) ui_font_size_select: Entity<SelectState<Vec<&'static str>>>,
    pub(super) scale_factor_select: Entity<SelectState<Vec<&'static str>>>,
    pub(super) configure_scaling: bool,
    pub(super) use_system_dpi: bool,
    pub(super) configure_scaling_focus: FocusHandle,
    pub(super) system_dpi_focus: FocusHandle,
    pub(super) table_zebra_stripe: bool,
    pub(super) issues_grouped_evidence: bool,
    pub(super) editor_scroll: ScrollHandle,
    pub(super) misc_scroll: ScrollHandle,
    pub(super) charset_mode: u8,
    pub(super) charset_select: Entity<SelectState<Vec<&'static str>>>,
    pub(super) html_render_external_requests: bool,
    pub(super) html_render_focus: FocusHandle,
    pub(super) search_regex_default: bool,
    pub(super) search_regex_focus: FocusHandle,
    pub(super) search_autoscroll_default: bool,
    pub(super) search_autoscroll_focus: FocusHandle,
    pub(super) editor_font_size_input: Entity<InputState>,
    pub(super) soft_wrap_focus: FocusHandle,
    pub(super) pretty_default_focus: FocusHandle,
    pub(super) auto_inspector_focus: FocusHandle,
    pub(super) search_case_focus: FocusHandle,
    pub(super) confirm_close_focus: FocusHandle,
    pub(super) redact_curl_focus: FocusHandle,
    pub(super) auto_check_updates_focus: FocusHandle,
    pub(super) send_hotkey_select: Entity<SelectState<Vec<&'static str>>>,
    pub(super) hotkey_search_input: Entity<InputState>,
    pub(super) hidden_req_header_input: Entity<InputState>,
    pub(super) hidden_resp_header_input: Entity<InputState>,
    pub(super) hide_headers_default_focus: [FocusHandle; 2],
    pub(super) selected_hidden_req_header: Option<usize>,
    pub(super) selected_hidden_resp_header: Option<usize>,
    pub(super) local_error: Option<String>,
}

impl UserPagesUi {
    pub(super) fn new(
        preferences: &user_preferences::UserPreferencesUi,
        window: &mut Window,
        cx: &mut Context<Settings>,
    ) -> Self {
        let display = &preferences.display;
        let editor = &preferences.message_editor;
        let misc = &preferences.misc;

        let ui_font_size_select = cx.new(|scx| {
            SelectState::new(
                UI_FONT_SIZE_OPTIONS.to_vec(),
                Some(IndexPath::new(font_size_to_index(display.ui_font_size_px))),
                window,
                scx,
            )
        });
        cx.subscribe_in(&ui_font_size_select, window, |this, _, event, _, cx| {
            let SelectEvent::Confirm(Some(value)) = event else {
                return;
            };
            if let Ok(px) = value.parse::<u8>() {
                this.set_user_display_font_size(px, cx);
            }
        })
        .detach();

        let scale_factor_select = cx.new(|scx| {
            SelectState::new(
                SCALE_FACTOR_OPTIONS.to_vec(),
                Some(IndexPath::new(scale_percent_to_index(
                    display.scale_factor_percent,
                ))),
                window,
                scx,
            )
        });
        cx.subscribe_in(&scale_factor_select, window, |this, _, event, _, cx| {
            let SelectEvent::Confirm(Some(value)) = event else {
                return;
            };
            let percent = scale_label_to_percent(value);
            this.set_user_display_scale_percent(percent, cx);
        })
        .detach();

        let charset_select = cx.new(|scx| {
            SelectState::new(
                CHARSET_OPTIONS.to_vec(),
                Some(IndexPath::new(0)),
                window,
                scx,
            )
        });

        let send_hotkey_select = cx.new(|scx| {
            SelectState::new(
                SEND_HOTKEY_LABELS.to_vec(),
                Some(IndexPath::new(hotkey_key_to_index(
                    &misc.hotkey_send_request,
                ))),
                window,
                scx,
            )
        });
        cx.subscribe_in(
            &send_hotkey_select,
            window,
            |this, _, event, window, cx| {
                let SelectEvent::Confirm(Some(label)) = event else {
                    return;
                };
                let key = match *label {
                    "⌃Enter" => "ctrl-enter",
                    "⌘⇧Enter" => "cmd-shift-enter",
                    _ => "cmd-enter",
                };
                this.toggle_user_misc(|m| m.hotkey_send_request = key.into(), window, cx);
            },
        )
        .detach();

        let hotkey_search_input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索"));
        cx.subscribe_in(&hotkey_search_input, window, |_, _, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();

        Self {
            ui_font_size_select,
            scale_factor_select,
            configure_scaling: display.scale_factor_percent != 100,
            use_system_dpi: false,
            configure_scaling_focus: cx.focus_handle(),
            system_dpi_focus: cx.focus_handle(),
            table_zebra_stripe: true,
            issues_grouped_evidence: true,
            editor_scroll: ScrollHandle::new(),
            misc_scroll: ScrollHandle::new(),
            charset_mode: 0,
            charset_select,
            html_render_external_requests: true,
            html_render_focus: cx.focus_handle(),
            search_regex_default: false,
            search_regex_focus: cx.focus_handle(),
            search_autoscroll_default: false,
            search_autoscroll_focus: cx.focus_handle(),
            editor_font_size_input: cx.new(|cx| {
                InputState::new(window, cx).default_value(editor.font_size_px.to_string())
            }),
            soft_wrap_focus: cx.focus_handle(),
            pretty_default_focus: cx.focus_handle(),
            auto_inspector_focus: cx.focus_handle(),
            search_case_focus: cx.focus_handle(),
            confirm_close_focus: cx.focus_handle(),
            redact_curl_focus: cx.focus_handle(),
            auto_check_updates_focus: cx.focus_handle(),
            send_hotkey_select,
            hotkey_search_input,
            hidden_req_header_input: cx
                .new(|cx| InputState::new(window, cx).placeholder("输入新的标头名称")),
            hidden_resp_header_input: cx
                .new(|cx| InputState::new(window, cx).placeholder("输入新的标头名称")),
            hide_headers_default_focus: [cx.focus_handle(), cx.focus_handle()],
            selected_hidden_req_header: None,
            selected_hidden_resp_header: None,
            local_error: None,
        }
    }
}

pub(super) fn user_mio_badge(id: Option<&'static str>) -> AnyElement {
    let dark = theme::is_dark();
    let pill = div()
        .flex_shrink_0()
        .px(px(10.))
        .py(px(2.))
        .rounded_full()
        .bg(rgb(if dark { 0x273b54 } else { 0xc8d7ec }))
        .text_size(theme::scaled_px(12.))
        .text_color(rgb(if dark { 0xd4e6fa } else { 0x1f2d3d }))
        .whitespace_nowrap()
        .child("用户设置");
    if let Some(id) = id {
        pill.id(id)
            .test_support()
            .aria_label("用户设置")
            .into_any_element()
    } else {
        pill.into_any_element()
    }
}

pub(super) fn user_section_heading(
    id: Option<&'static str>,
    scope_id: Option<&'static str>,
    title: &'static str,
) -> gpui_kit::Div {
    let title_el = if let Some(id) = id {
        div()
            .id(id)
            .test_support()
            .role(Role::Heading)
            .aria_label(title)
            .text_size(theme::scaled_px(15.))
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .text_color(theme::foreground())
            .whitespace_nowrap()
            .child(title)
            .into_any_element()
    } else {
        div()
            .text_size(theme::scaled_px(15.))
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .text_color(theme::foreground())
            .whitespace_nowrap()
            .child(title)
            .into_any_element()
    };
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(9.))
                .min_w_0()
                .child(
                    div()
                        .w(px(20.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(AssetIconName::CircleQuestionMark)
                                .size(px(19.))
                                .text_color(theme::foreground()),
                        ),
                )
                .child(title_el),
        )
        .child(user_mio_badge(scope_id))
}

pub(super) fn user_desc_row_with_icon(
    icon: impl IntoElement,
    desc: &'static str,
) -> gpui_kit::Div {
    div()
        .flex()
        .items_start()
        .gap(px(9.))
        .child(
            div()
                .w(px(20.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(icon),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(theme::foreground())
                .child(desc),
        )
}

pub(super) fn user_gear_static_icon() -> impl IntoElement {
    div().pt(px(1.)).child(
        Icon::new(AssetIconName::Settings)
            .size(px(19.))
            .text_color(theme::foreground()),
    )
}

pub(super) fn user_gear_menu_button(
    id: impl Into<gpui_kit::ElementId>,
    tooltip: &'static str,
    disabled: bool,
) -> Button {
    Button::new(id)
        .ghost()
        .size(px(22.))
        .p_0()
        .tab_stop(false)
        .disabled(disabled)
        .accessibility_label("设置")
        .tooltip(tooltip)
        .child(
            Icon::new(AssetIconName::Settings)
                .size(px(19.))
                .text_color(theme::foreground()),
        )
}

pub(super) fn user_reset_icon_button(
    id: impl Into<gpui_kit::ElementId>,
    tooltip: &'static str,
    disabled: bool,
) -> Button {
    Button::new(id)
        .ghost()
        .size(px(22.))
        .p_0()
        .tab_stop(false)
        .disabled(disabled)
        .accessibility_label("恢复默认设置")
        .tooltip(tooltip)
        .child(
            Icon::new(AssetIconName::RotateCcw)
                .size(px(19.))
                .text_color(theme::foreground()),
        )
}

pub(super) fn user_mio_button(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    disabled: bool,
) -> Button {
    let dark = theme::is_dark();
    let (bg, border, fg, hover_bg) = if disabled {
        (
            rgb(if dark { 0x32353a } else { 0xebebeb }),
            rgb(if dark { 0x41454d } else { 0xdfdfdf }),
            rgb(if dark { 0x6f737a } else { 0x7c7c7c }),
            rgb(if dark { 0x32353a } else { 0xebebeb }),
        )
    } else {
        (
            rgb(if dark { 0x3c3f44 } else { 0xebebeb }),
            rgb(if dark { 0x4c5057 } else { 0xd8d8d8 }),
            rgb(if dark { 0xdfe1e5 } else { 0x1f1f1f }),
            rgb(if dark { 0x474b52 } else { 0xe0e0e0 }),
        )
    };
    button(id, label)
        .disabled(disabled)
        .h(theme::scaled_px(24.).max(px(24.)))
        .rounded(px(4.))
        .bg(bg)
        .border_color(border)
        .text_color(fg)
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .when(!disabled, |this| this.hover(move |style| style.bg(hover_bg)))
}

pub(super) fn send_hotkey_display_label(hotkey: &str) -> String {
    match hotkey.trim() {
        "cmd-enter" | "" => "⌘Enter".into(),
        "ctrl-enter" => "⌃Enter".into(),
        "cmd-shift-enter" => "⌘⇧Enter".into(),
        other => other.to_owned(),
    }
}

impl Settings {
    pub(super) fn toggle_user_misc(
        &mut self,
        change: impl FnOnce(&mut UserMiscSettings),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if editing::input_snapshot(&self.user_pages.editor_font_size_input, window, cx).is_err() {
            self.user_pages.local_error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        }
        if self.has_hidden_header_composition(window, cx) {
            self.user_pages.local_error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        }
        let mut misc = self.state.preferences.misc.clone();
        change(&mut misc);
        if misc == self.state.preferences.misc {
            return;
        }
        let display = self.state.preferences.display.clone();
        let editor = self.state.preferences.message_editor.clone();
        self.submit_user_workbench(display, editor, misc, cx);
    }

    pub(super) fn reset_user_misc_defaults(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if editing::input_snapshot(&self.user_pages.editor_font_size_input, window, cx).is_err() {
            self.user_pages.local_error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        }
        if self.has_hidden_header_composition(window, cx) {
            self.user_pages.local_error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        }
        let defaults = UserMiscSettings::default();
        let display = self.state.preferences.display.clone();
        let editor = self.state.preferences.message_editor.clone();
        if self.submit_user_workbench(display, editor, defaults.clone(), cx) {
            self.user_pages.local_error = None;
            let idx = IndexPath::new(hotkey_key_to_index(&defaults.hotkey_send_request));
            self.user_pages
                .send_hotkey_select
                .update(cx, |s, cx| s.set_selected_index(Some(idx), window, cx));
        }
    }

    pub(super) fn user_settings_enabled(&self) -> bool {
        self.state.preferences_available && self.state.preferences.can_save()
    }

    pub(super) fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        self.user_pages.local_error = None;
        let _ = self.owner.update(cx, |view, cx| {
            view.update_status = mio_core::UpdateStatus::Checking;
            view.enqueue(mio_core::Command::CheckForUpdates { manual: true }, cx);
            cx.notify();
        });
        cx.notify();
    }

    pub(super) fn install_available_update(
        &mut self,
        info: mio_core::UpdateInfo,
        cx: &mut Context<Self>,
    ) {
        self.user_pages.local_error = None;
        let version = info.latest_version.clone();
        let _ = self.owner.update(cx, |view, cx| {
            view.update_status = mio_core::UpdateStatus::Downloading {
                version,
                received_bytes: 0,
                total_bytes: (info.asset_size > 0).then_some(info.asset_size),
            };
            view.enqueue(mio_core::Command::InstallUpdate { info }, cx);
            cx.notify();
        });
        cx.notify();
    }

    pub(super) fn sync_user_pages_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.user_pages.local_error.is_some() {
            return;
        }
        let display = self.state.preferences.display.clone();
        let editor = self.state.preferences.message_editor.clone();
        let misc = self.state.preferences.misc.clone();

        let font_idx = IndexPath::new(font_size_to_index(display.ui_font_size_px));
        if self
            .user_pages
            .ui_font_size_select
            .read(cx)
            .selected_index(cx)
            != Some(font_idx)
        {
            self.user_pages.ui_font_size_select.update(cx, |s, cx| {
                s.set_selected_index(Some(font_idx), window, cx)
            });
        }
        let scale_idx = IndexPath::new(scale_percent_to_index(display.scale_factor_percent));
        if self
            .user_pages
            .scale_factor_select
            .read(cx)
            .selected_index(cx)
            != Some(scale_idx)
        {
            self.user_pages.scale_factor_select.update(cx, |s, cx| {
                s.set_selected_index(Some(scale_idx), window, cx)
            });
        }
        if display.scale_factor_percent != 100 {
            self.user_pages.configure_scaling = true;
        }
        let hotkey_idx = IndexPath::new(hotkey_key_to_index(&misc.hotkey_send_request));
        if self
            .user_pages
            .send_hotkey_select
            .read(cx)
            .selected_index(cx)
            != Some(hotkey_idx)
        {
            self.user_pages.send_hotkey_select.update(cx, |s, cx| {
                s.set_selected_index(Some(hotkey_idx), window, cx)
            });
        }

        let editor_font_text = editor.font_size_px.to_string();
        self.user_pages.editor_font_size_input.update(cx, |state, cx| {
            if state.marked_text_range(window, cx).is_none()
                && state.value().as_ref() != editor_font_text.as_str()
            {
                state.set_value(editor_font_text, window, cx);
            }
        });

        let req_len = self.hidden_header_names(false).len();
        if self
            .user_pages
            .selected_hidden_req_header
            .is_some_and(|index| index >= req_len)
        {
            self.user_pages.selected_hidden_req_header = None;
        }
        let resp_len = self.hidden_header_names(true).len();
        if self
            .user_pages
            .selected_hidden_resp_header
            .is_some_and(|index| index >= resp_len)
        {
            self.user_pages.selected_hidden_resp_header = None;
        }
    }

    pub(super) fn submit_user_workbench(
        &mut self,
        display: UserDisplaySettings,
        message_editor: UserMessageEditorSettings,
        misc: UserMiscSettings,
        cx: &mut Context<Self>,
    ) -> bool {
        let result = self.owner.update(cx, |view, cx| {
            view.set_user_workbench_preferences(display, message_editor, misc, cx)
        });
        match result {
            Ok(Ok(_)) => {
                self.user_pages.local_error = None;
                cx.notify();
                true
            }
            Ok(Err(error)) => {
                self.user_pages.local_error = Some(error);
                cx.notify();
                false
            }
            Err(_) => {
                self.user_pages.local_error = Some("工作台已关闭；用户设置未修改。".into());
                cx.notify();
                false
            }
        }
    }

    pub(super) fn append_user_pages_feedback(
        &self,
        mut body: gpui_kit::Stateful<gpui_kit::Div>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let prefs = &self.state.preferences;
        for (id, error) in [
            ("settings-user-error", &self.user_pages.local_error),
            ("settings-user-load-error", &prefs.load_error),
            ("settings-user-save-error", &prefs.error),
        ] {
            if let Some(error) = error {
                body = body.child(
                    div()
                        .id(id)
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .ml(px(29.))
                        .p(px(8.))
                        .rounded(px(4.))
                        .bg(theme::danger_bg())
                        .text_color(theme::danger_fg())
                        .child(error.clone()),
                );
            }
        }
        if prefs.load_error.is_some() || prefs.error.is_some() {
            body = body.child(
                div().pl(px(29.)).child(
                    user_mio_button(
                        "settings-user-retry",
                        "重试",
                        !self.state.preferences_available || prefs.busy(),
                    )
                    .tooltip("重新读取并同步用户偏好设置")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let _ = this
                            .owner
                            .update(cx, |view, cx| view.retry_user_preferences(cx));
                    })),
                ),
            );
        }
        body
    }
}
