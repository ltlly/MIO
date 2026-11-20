use gpui_kit::accesskit::Role;
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::base::CheckboxState;
use gpui_kit::component::{
    Icon, IndexPath,
    menu::{DropdownMenu, PopupMenuItem},
    scroll::{Scrollbar, ScrollbarMode},
    select::{Select, SelectEvent},
};

use super::*;

struct CharsetSelectSync {
    last_synced: String,
    _sub: Subscription,
}

struct HotkeyEditorUi {
    selected_row: Option<usize>,
    editing_row: Option<usize>,
    capturing: bool,
    capture_input: Entity<InputState>,
    capture_focus: FocusHandle,
    _sub: Subscription,
}

struct HotkeyRowSpec {
    action_id: &'static str,
    icon: AssetIconName,
    action: &'static str,
    category: &'static str,
    default_key: &'static str,
}

const HOTKEY_TABLE: [HotkeyRowSpec; 33] = [
    HotkeyRowSpec {
        action_id: "send_request",
        icon: AssetIconName::SendHorizontal,
        action: "发送请求",
        category: "请求",
        default_key: "cmd-enter",
    },
    HotkeyRowSpec {
        action_id: "cancel_request",
        icon: AssetIconName::CircleX,
        action: "取消请求",
        category: "请求",
        default_key: "cmd-period",
    },
    HotkeyRowSpec {
        action_id: "send_to_repeater",
        icon: AssetIconName::Repeat,
        action: "发送到 Repeater",
        category: "工具",
        default_key: "cmd-r",
    },
    HotkeyRowSpec {
        action_id: "send_to_intruder",
        icon: AssetIconName::Crosshair,
        action: "发送到 Intruder",
        category: "工具",
        default_key: "cmd-i",
    },
    HotkeyRowSpec {
        action_id: "send_to_comparer",
        icon: AssetIconName::Diff,
        action: "发送到 Comparer",
        category: "工具",
        default_key: "",
    },
    HotkeyRowSpec {
        action_id: "send_to_decoder",
        icon: AssetIconName::Binary,
        action: "发送到 Decoder",
        category: "工具",
        default_key: "",
    },
    HotkeyRowSpec {
        action_id: "send_to_organiser",
        icon: AssetIconName::FolderPlus,
        action: "发送到 Organiser",
        category: "工具",
        default_key: "",
    },
    HotkeyRowSpec {
        action_id: "find_in_message",
        icon: AssetIconName::Search,
        action: "消息内查找",
        category: "编辑",
        default_key: "cmd-f",
    },
    HotkeyRowSpec {
        action_id: "url_encode",
        icon: AssetIconName::Code,
        action: "URL 编码",
        category: "编辑",
        default_key: "cmd-u",
    },
    HotkeyRowSpec {
        action_id: "url_decode",
        icon: AssetIconName::Code,
        action: "URL 解码",
        category: "编辑",
        default_key: "cmd-shift-u",
    },
    HotkeyRowSpec {
        action_id: "base64_encode",
        icon: AssetIconName::Binary,
        action: "Base64 编码",
        category: "编辑",
        default_key: "cmd-b",
    },
    HotkeyRowSpec {
        action_id: "base64_decode",
        icon: AssetIconName::Binary,
        action: "Base64 解码",
        category: "编辑",
        default_key: "cmd-shift-b",
    },
    HotkeyRowSpec {
        action_id: "base64_url_encode",
        icon: AssetIconName::Binary,
        action: "Base64 URL 编码",
        category: "编辑",
        default_key: "",
    },
    HotkeyRowSpec {
        action_id: "base64_url_decode",
        icon: AssetIconName::Binary,
        action: "Base64 URL 解码",
        category: "编辑",
        default_key: "",
    },
    HotkeyRowSpec {
        action_id: "html_encode",
        icon: AssetIconName::FileCode,
        action: "HTML 编码",
        category: "编辑",
        default_key: "",
    },
    HotkeyRowSpec {
        action_id: "html_decode",
        icon: AssetIconName::FileCode,
        action: "HTML 解码",
        category: "编辑",
        default_key: "",
    },
    HotkeyRowSpec {
        action_id: "change_request_method",
        icon: AssetIconName::RefreshCw,
        action: "修改请求方法",
        category: "编辑",
        default_key: "",
    },
    HotkeyRowSpec {
        action_id: "add_notes",
        icon: AssetIconName::StickyNote,
        action: "添加批注",
        category: "批注",
        default_key: "",
    },
    HotkeyRowSpec {
        action_id: "clear_highlight",
        icon: AssetIconName::Eraser,
        action: "清除高亮",
        category: "批注",
        default_key: "",
    },
    HotkeyRowSpec {
        action_id: "add_to_scope",
        icon: AssetIconName::CirclePlus,
        action: "添加到目标范围",
        category: "Scope",
        default_key: "",
    },
    HotkeyRowSpec {
        action_id: "add_to_site_map",
        icon: AssetIconName::Bookmark,
        action: "添加到 Site map",
        category: "Site map",
        default_key: "",
    },
    HotkeyRowSpec {
        action_id: "add_payload_position",
        icon: AssetIconName::Hash,
        action: "添加 Payload 位置",
        category: "Payload 位置",
        default_key: "",
    },
    HotkeyRowSpec {
        action_id: "clear_payload_positions",
        icon: AssetIconName::CircleMinus,
        action: "清除 Payload 位置",
        category: "Payload 位置",
        default_key: "",
    },
    HotkeyRowSpec {
        action_id: "clear_history",
        icon: AssetIconName::Eraser,
        action: "清除历史记录",
        category: "历史记录",
        default_key: "",
    },
    HotkeyRowSpec {
        action_id: "save_project",
        icon: AssetIconName::Save,
        action: "保存项目",
        category: "项目",
        default_key: "cmd-s",
    },
    HotkeyRowSpec {
        action_id: "save_project_as",
        icon: AssetIconName::Save,
        action: "另存为项目",
        category: "项目",
        default_key: "cmd-shift-s",
    },
    HotkeyRowSpec {
        action_id: "open_settings",
        icon: AssetIconName::Settings,
        action: "打开设置",
        category: "界面",
        default_key: "cmd-,",
    },
    HotkeyRowSpec {
        action_id: "close_tab",
        icon: AssetIconName::X,
        action: "关闭当前标签",
        category: "标签管理",
        default_key: "cmd-w",
    },
    HotkeyRowSpec {
        action_id: "close_other_tabs",
        icon: AssetIconName::Layers,
        action: "关闭其他标签",
        category: "标签管理",
        default_key: "",
    },
    HotkeyRowSpec {
        action_id: "toggle_tab_orientation",
        icon: AssetIconName::Columns2,
        action: "切换布局方向",
        category: "界面",
        default_key: "",
    },
    HotkeyRowSpec {
        action_id: "toggle_proxy_intercept",
        icon: AssetIconName::ShieldCheck,
        action: "切换 Intercept 开关",
        category: "Proxy",
        default_key: "",
    },
    HotkeyRowSpec {
        action_id: "open_command_palette",
        icon: AssetIconName::Search,
        action: "命令面板",
        category: "界面",
        default_key: "cmd-k",
    },
    HotkeyRowSpec {
        action_id: "open_global_search",
        icon: AssetIconName::Search,
        action: "全局搜索",
        category: "界面",
        default_key: "cmd-shift-f",
    },
];

fn charset_to_index(charset: &str) -> usize {
    CHARSET_OPTIONS
        .iter()
        .position(|opt| opt.eq_ignore_ascii_case(charset.trim()))
        .unwrap_or(0)
}

fn format_hotkey_display(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if matches!(trimmed, "cmd-enter" | "ctrl-enter" | "cmd-shift-enter") {
        return send_hotkey_display_label(trimmed);
    }
    let mut out = String::new();
    let mut key_part = String::new();
    for token in trimmed.split('-').filter(|t| !t.is_empty()) {
        match token.to_ascii_lowercase().as_str() {
            "cmd" | "command" | "platform" | "meta" => out.push('⌘'),
            "ctrl" | "control" => out.push('⌃'),
            "alt" | "option" => out.push('⌥'),
            "shift" => out.push('⇧'),
            "enter" | "return" => key_part = "Enter".into(),
            "period" | "." => key_part = ".".into(),
            "comma" | "," => key_part = ",".into(),
            "space" => key_part = "Space".into(),
            "escape" | "esc" => key_part = "Esc".into(),
            "backspace" => key_part = "Backspace".into(),
            "tab" => key_part = "Tab".into(),
            other if other.len() == 1 && other.is_ascii() => {
                key_part = other.to_ascii_uppercase();
            }
            other => key_part = other.to_owned(),
        }
    }
    out.push_str(&key_part);
    if out.is_empty() {
        trimmed.to_owned()
    } else {
        out
    }
}

fn normalize_hotkey_input(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    let mut expanded = String::with_capacity(trimmed.len() + 16);
    for ch in trimmed.chars() {
        match ch {
            '⌘' => expanded.push_str("cmd-"),
            '⌃' => expanded.push_str("ctrl-"),
            '⌥' => expanded.push_str("alt-"),
            '⇧' => expanded.push_str("shift-"),
            '+' | ' ' => expanded.push('-'),
            other => expanded.push(other),
        }
    }
    let mut has_cmd = false;
    let mut has_ctrl = false;
    let mut has_alt = false;
    let mut has_shift = false;
    let mut key: Option<String> = None;
    for token in expanded.split('-').filter(|t| !t.is_empty()) {
        match token.to_ascii_lowercase().as_str() {
            "cmd" | "command" | "platform" | "meta" => has_cmd = true,
            "ctrl" | "control" => has_ctrl = true,
            "alt" | "option" => has_alt = true,
            "shift" => has_shift = true,
            "return" => {
                if key.is_some() {
                    return Err("快捷键只能包含一个主键（例如 cmd-shift-r）。".into());
                }
                key = Some("enter".into());
            }
            "." => {
                if key.is_some() {
                    return Err("快捷键只能包含一个主键（例如 cmd-shift-r）。".into());
                }
                key = Some("period".into());
            }
            "comma" => {
                if key.is_some() {
                    return Err("快捷键只能包含一个主键（例如 cmd-shift-r）。".into());
                }
                key = Some(",".into());
            }
            other => {
                if key.is_some() {
                    return Err("快捷键只能包含一个主键（例如 cmd-shift-r）。".into());
                }
                key = Some(other.to_owned());
            }
        }
    }
    if !has_cmd && !has_ctrl {
        return Err("所有快捷键都必须包含 Command (cmd) 或 Control (ctrl) 修饰键。".into());
    }
    let Some(key_name) = key else {
        return Err("请输入完整的快捷键组合（例如 cmd-shift-r 或 ctrl-k）。".into());
    };
    let mut parts: Vec<&str> = Vec::new();
    if has_cmd {
        parts.push("cmd");
    }
    if has_ctrl {
        parts.push("ctrl");
    }
    if has_alt {
        parts.push("alt");
    }
    if has_shift {
        parts.push("shift");
    }
    parts.push(&key_name);
    if !is_valid_hotkey_key_name(&key_name) {
        return Err(format!(
            "无法识别快捷键组合“{trimmed}”，请使用如 cmd-shift-r 格式。"
        ));
    }
    Ok(parts.join("-"))
}

fn is_valid_hotkey_key_name(key: &str) -> bool {
    if key.len() == 1 {
        let b = key.as_bytes()[0];
        return b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'.' | b',' | b'/' | b'\\' | b'-' | b'=' | b'[' | b']' | b';' | b'\'' | b'`'
            );
    }
    matches!(
        key,
        "enter"
            | "return"
            | "tab"
            | "space"
            | "escape"
            | "backspace"
            | "delete"
            | "up"
            | "down"
            | "left"
            | "right"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
            | "period"
            | "comma"
            | "slash"
            | "backslash"
            | "minus"
            | "equal"
            | "f1"
            | "f2"
            | "f3"
            | "f4"
            | "f5"
            | "f6"
            | "f7"
            | "f8"
            | "f9"
            | "f10"
            | "f11"
            | "f12"
    )
}

fn keystroke_to_canonical_string(keystroke: &gpui_kit::Keystroke) -> Option<String> {
    let key_lower = keystroke.key.trim().to_ascii_lowercase();
    if key_lower.is_empty()
        || matches!(
            key_lower.as_str(),
            "shift"
                | "control"
                | "ctrl"
                | "alt"
                | "option"
                | "command"
                | "cmd"
                | "meta"
                | "super"
                | "platform"
                | "capslock"
        )
    {
        return None;
    }
    let m = keystroke.modifiers;
    if !m.platform && !m.control {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    if m.platform {
        parts.push("cmd");
    }
    if m.control {
        parts.push("ctrl");
    }
    if m.alt {
        parts.push("alt");
    }
    if m.shift {
        parts.push("shift");
    }
    let key_name = match key_lower.as_str() {
        "." => "period",
        "return" => "enter",
        other => other,
    };
    if !is_valid_hotkey_key_name(key_name) {
        return None;
    }
    parts.push(key_name);
    Some(parts.join("-"))
}

fn effective_hotkey_raw(index: usize, spec: &HotkeyRowSpec, misc: &UserMiscSettings) -> String {
    if index == 0 {
        if let Some(custom) = misc
            .custom_hotkeys
            .get("send_request")
            .filter(|v| !v.trim().is_empty())
        {
            return custom.trim().to_owned();
        }
        let configured = misc.hotkey_send_request.trim();
        return if configured.is_empty() {
            spec.default_key.to_owned()
        } else {
            configured.to_owned()
        };
    }
    if let Some(custom) = misc.custom_hotkeys.get(spec.action_id) {
        custom.trim().to_owned()
    } else {
        spec.default_key.to_owned()
    }
}

fn is_hotkey_customized(index: usize, spec: &HotkeyRowSpec, misc: &UserMiscSettings) -> bool {
    if index == 0 {
        misc.hotkey_send_request.trim() != spec.default_key
            || misc.custom_hotkeys.contains_key("send_request")
    } else {
        misc.custom_hotkeys.contains_key(spec.action_id)
    }
}

fn uniform_editor_button(
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

        let charset_select = self.user_pages.charset_select.clone();
        let settings_weak = cx.weak_entity();
        let charset_sync = window.use_keyed_state(
            "settings-user-editor-charset-sync",
            cx,
            |window, cx| {
                let _sub = cx.subscribe_in(
                    &charset_select,
                    window,
                    move |_: &mut CharsetSelectSync, _, event, window, cx| {
                        let SelectEvent::Confirm(Some(value)) = event else {
                            return;
                        };
                        let chosen = (*value).to_owned();
                        let _ = settings_weak.update(cx, |this, cx| {
                            this.set_user_editor_specific_charset(chosen, window, cx);
                        });
                    },
                );
                CharsetSelectSync {
                    last_synced: String::new(),
                    _sub,
                }
            },
        );
        if charset_sync.read(cx).last_synced != editor.specific_charset {
            let desired = editor.specific_charset.clone();
            let idx = IndexPath::new(charset_to_index(&desired));
            let cs = charset_select.clone();
            let sync_handle = charset_sync.clone();
            let win_handle = window.window_handle();
            cx.defer(move |cx| {
                sync_handle.update(cx, |s, _| s.last_synced = desired);
                let _ = win_handle.update(cx, |_, window, cx| {
                    cs.update(cx, |s, cx| {
                        if s.selected_index(cx) != Some(idx) {
                            s.set_selected_index(Some(idx), window, cx);
                        }
                    });
                });
            });
        }

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

    fn save_hotkey_binding(
        &mut self,
        row_index: usize,
        raw_input: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(spec) = HOTKEY_TABLE.get(row_index) else {
            return false;
        };
        if editing::input_snapshot(&self.user_pages.editor_font_size_input, window, cx).is_err()
            || self.has_hidden_header_composition(window, cx)
        {
            self.user_pages.local_error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return false;
        }
        let normalized = match normalize_hotkey_input(raw_input) {
            Ok(val) => val,
            Err(err) => {
                self.user_pages.local_error = Some(err);
                cx.notify();
                return false;
            }
        };
        let mut misc = self.state.preferences.misc.clone();
        if row_index == 0 {
            if normalized.is_empty() {
                self.user_pages.local_error =
                    Some("发送请求快捷键不能为空，请至少包含 Command 或 Control 修饰键。".into());
                cx.notify();
                return false;
            }
            misc.hotkey_send_request = normalized;
            misc.custom_hotkeys.remove("send_request");
        } else if normalized == spec.default_key {
            misc.custom_hotkeys.remove(spec.action_id);
        } else {
            misc.custom_hotkeys
                .insert(spec.action_id.to_owned(), normalized);
        }
        if misc == self.state.preferences.misc {
            self.user_pages.local_error = None;
            cx.notify();
            return true;
        }
        let display = self.state.preferences.display.clone();
        let editor = self.state.preferences.message_editor.clone();
        self.submit_user_workbench(display, editor, misc, cx)
    }

    fn reset_hotkey_binding(
        &mut self,
        row_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(spec) = HOTKEY_TABLE.get(row_index) else {
            return false;
        };
        if editing::input_snapshot(&self.user_pages.editor_font_size_input, window, cx).is_err()
            || self.has_hidden_header_composition(window, cx)
        {
            self.user_pages.local_error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return false;
        }
        let mut misc = self.state.preferences.misc.clone();
        if row_index == 0 {
            misc.hotkey_send_request = spec.default_key.to_owned();
            misc.custom_hotkeys.remove("send_request");
        } else {
            misc.custom_hotkeys.remove(spec.action_id);
        }
        if misc == self.state.preferences.misc {
            self.user_pages.local_error = None;
            cx.notify();
            return true;
        }
        let display = self.state.preferences.display.clone();
        let editor = self.state.preferences.message_editor.clone();
        self.submit_user_workbench(display, editor, misc, cx)
    }

    pub(in crate::workbench) fn render_user_misc(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let _ = (
            &self.user_pages.confirm_close_focus,
            &self.user_pages.redact_curl_focus,
            &self.user_pages.send_hotkey_select,
        );
        let enabled = self.user_settings_enabled();
        let misc = &self.state.preferences.misc;

        let settings_weak_sub = cx.weak_entity();
        let hotkey_ui = window.use_keyed_state("settings-user-hotkey-editor-ui", cx, |window, cx| {
            let capture_input =
                cx.new(|icx| InputState::new(window, icx).placeholder("如 cmd-shift-r"));
            let capture_focus = cx.focus_handle();
            let _sub = cx.subscribe_in(
                &capture_input,
                window,
                move |state: &mut HotkeyEditorUi, input, event, window, cx| {
                    if !matches!(event, InputEvent::PressEnter { .. }) {
                        return;
                    }
                    let Some(row_idx) = state.editing_row else {
                        return;
                    };
                    if input.read(cx).has_marked_text() {
                        let _ = settings_weak_sub.update(cx, |this, cx| {
                            this.user_pages.local_error =
                                Some(editing::COMPOSITION_PENDING.into());
                            cx.notify();
                        });
                        return;
                    }
                    let raw = input.read(cx).value().to_string();
                    let saved = settings_weak_sub
                        .update(cx, |this, cx| {
                            this.save_hotkey_binding(row_idx, &raw, window, cx)
                        })
                        .unwrap_or(false);
                    if saved {
                        state.editing_row = None;
                        state.capturing = false;
                        cx.notify();
                    }
                },
            );
            HotkeyEditorUi {
                selected_row: None,
                editing_row: None,
                capturing: false,
                capture_input,
                capture_focus,
                _sub,
            }
        });
        let (selected_row, editing_row, capturing, capture_input, capture_focus) = {
            let ui = hotkey_ui.read(cx);
            (
                ui.selected_row,
                ui.editing_row,
                ui.capturing,
                ui.capture_input.clone(),
                ui.capture_focus.clone(),
            )
        };

        let owner_gear = cx.weak_entity();
        let ui_gear = hotkey_ui.clone();
        let hotkeys_gear = user_gear_menu_button(
            "settings-user-misc-settings",
            "快捷键设置菜单",
            !enabled,
        )
        .dropdown_menu(move |menu, _, _| {
            let owner = owner_gear.clone();
            let ui_gear = ui_gear.clone();
            menu.item(PopupMenuItem::new("恢复默认设置").on_click(move |_, window, cx| {
                ui_gear.update(cx, |ui, cx| {
                    ui.editing_row = None;
                    ui.capturing = false;
                    cx.notify();
                });
                let _ = owner.update(cx, |this, cx| this.reset_user_misc_defaults(window, cx));
            }))
        });

        let dark = theme::is_dark();
        let table_border = rgb(if dark { 0x464a52 } else { 0xe0e0e0 });
        let table_head_bg = rgb(if dark { 0x2b2e33 } else { 0xeaeaea });
        let table_head_sep = rgb(if dark { 0x3e424a } else { 0xd8d8d8 });
        let table_zebra_bg = rgb(if dark { 0x282b30 } else { 0xf7f7f7 });
        let row_hover = rgb(if dark { 0x2f3540 } else { 0xedf2fa });
        let track_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xf5f5f5 }).into();
        let thumb_bg: gpui_kit::Hsla = rgb(if dark { 0x5c626b } else { 0xcbcbcb }).into();
        let thumb_hover: gpui_kit::Hsla = rgb(if dark { 0x707782 } else { 0xb0b0b0 }).into();
        let thumb_active: gpui_kit::Hsla = rgb(if dark { 0x828a96 } else { 0x999999 }).into();

        let query = self
            .user_pages
            .hotkey_search_input
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let row_h = theme::scaled_px(22.).max(px(22.));
        let table_scroll = window
            .use_keyed_state("settings-user-hotkey-table-scroll", cx, |_, _| {
                ScrollHandle::new()
            })
            .read(cx)
            .clone();

        let mut rows_list = div()
            .id("settings-user-hotkey-rows")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&table_scroll)
            .flex()
            .flex_col();

        let mut visible_idx = 0usize;
        for (index, row) in HOTKEY_TABLE.iter().enumerate() {
            let effective_raw = effective_hotkey_raw(index, row, misc);
            let shortcut = format_hotkey_display(&effective_raw);
            if !query.is_empty()
                && !row.action.to_lowercase().contains(&query)
                && !row.category.to_lowercase().contains(&query)
                && !shortcut.to_lowercase().contains(&query)
                && !effective_raw.to_lowercase().contains(&query)
            {
                continue;
            }
            let is_selected = selected_row == Some(index) || editing_row == Some(index);
            let ui_click = hotkey_ui.clone();
            let input_click = capture_input.clone();
            let initial_raw = effective_raw.clone();
            let row_bg: gpui_kit::Hsla = if is_selected {
                theme::selected()
            } else if theme::table_zebra_stripe() && visible_idx % 2 == 1 {
                table_zebra_bg.into()
            } else {
                theme::surface()
            };
            rows_list = rows_list.child(
                div()
                    .id(("settings-user-hotkey-row", index))
                    .test_support()
                    .role(Role::Row)
                    .aria_label(format!("{} {} {}", row.action, row.category, shortcut))
                    .aria_selected(is_selected)
                    .flex()
                    .items_center()
                    .h(row_h)
                    .flex_shrink_0()
                    .bg(row_bg)
                    .cursor_pointer()
                    .when(!is_selected, |this| {
                        this.hover(move |s| s.bg(row_hover))
                    })
                    .on_click(cx.listener(move |this, event: &gpui_kit::ClickEvent, window, cx| {
                        let open_editor =
                            event.click_count() >= 2 && this.user_settings_enabled();
                        ui_click.update(cx, |ui, cx| {
                            ui.selected_row = Some(index);
                            if open_editor {
                                ui.editing_row = Some(index);
                                ui.capturing = false;
                            } else if ui.editing_row != Some(index) {
                                ui.editing_row = None;
                                ui.capturing = false;
                            }
                            cx.notify();
                        });
                        if open_editor {
                            this.user_pages.local_error = None;
                            input_click.update(cx, |input, cx| {
                                input.set_value(initial_raw.clone(), window, cx);
                                input.focus(window, cx);
                                input.select_all(window, cx);
                            });
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                Icon::new(row.icon)
                                    .size(px(14.))
                                    .text_color(theme::foreground()),
                            )
                            .child(div().truncate().child(row.action)),
                    )
                    .child(
                        div()
                            .w(theme::scaled_px(230.).max(px(230.)))
                            .flex_shrink_0()
                            .px(px(8.))
                            .truncate()
                            .child(row.category),
                    )
                    .child(
                        div()
                            .w(theme::scaled_px(200.).max(px(200.)))
                            .flex_shrink_0()
                            .px(px(8.))
                            .truncate()
                            .child(shortcut),
                    ),
            );
            visible_idx += 1;
        }

        let hotkey_table = div()
            .id("settings-user-hotkey-table")
            .test_support()
            .role(Role::Table)
            .aria_label("macOS 全局快捷键参考")
            .w_full()
            .h(theme::scaled_px(488.).max(px(488.)))
            .border_1()
            .border_color(table_border)
            .rounded(px(0.))
            .bg(theme::surface())
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(theme::scaled_px(24.).max(px(24.)))
                    .flex_shrink_0()
                    .bg(table_head_bg)
                    .border_b_1()
                    .border_color(table_head_sep)
                    .text_color(theme::foreground())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .border_r_1()
                            .border_color(table_head_sep)
                            .child("操作"),
                    )
                    .child(
                        div()
                            .w(theme::scaled_px(230.).max(px(230.)))
                            .flex_shrink_0()
                            .h_full()
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .border_r_1()
                            .border_color(table_head_sep)
                            .child("分类"),
                    )
                    .child(
                        div()
                            .w(theme::scaled_px(200.).max(px(200.)))
                            .flex_shrink_0()
                            .h_full()
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .child("快捷键"),
                    )
                    .child(div().w(px(10.)).h_full().flex_shrink_0()),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(rows_list)
                    .child(
                        div()
                            .relative()
                            .w(px(10.))
                            .h_full()
                            .flex_shrink_0()
                            .bg(track_bg)
                            .child(
                                Scrollbar::vertical(&table_scroll)
                                    .id("settings-user-hotkey-vscroll")
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
                                                    .min_length(px(28.))
                                                    .bg(thumb_bg)
                                            })
                                            .thumb_hover(|s| {
                                                s.width(px(6.))
                                                    .inset(px(2.))
                                                    .radius(px(3.))
                                                    .min_length(px(28.))
                                                    .bg(thumb_hover)
                                            })
                                            .thumb_active(|s| {
                                                s.width(px(6.))
                                                    .inset(px(2.))
                                                    .radius(px(3.))
                                                    .min_length(px(28.))
                                                    .bg(thumb_active)
                                            })
                                    }),
                            ),
                    ),
            );

        let search_box = div()
            .w(theme::scaled_px(230.).max(px(230.)))
            .flex_shrink_0()
            .child(
                Input::new(&self.user_pages.hotkey_search_input)
                    .id("settings-user-hotkey-search")
                    .aria_label("搜索快捷键")
                    .small()
                    .cleanable(true)
                    .suffix(
                        Icon::new(AssetIconName::Search)
                            .size(px(14.))
                            .text_color(theme::muted_foreground()),
                    ),
            );

        let toolbar_row = if let Some(edit_idx) = editing_row
            && let Some(edit_spec) = HOTKEY_TABLE.get(edit_idx)
        {
            let ui_cap_key = hotkey_ui.clone();
            let input_cap_key = capture_input.clone();
            let ui_rec = hotkey_ui.clone();
            let focus_rec = capture_focus.clone();
            let ui_save = hotkey_ui.clone();
            let input_save = capture_input.clone();
            let ui_clear = hotkey_ui.clone();
            let ui_cancel = hotkey_ui.clone();
            let capture_field: AnyElement = if capturing {
                div()
                    .id("settings-user-hotkey-capture-box")
                    .test_support()
                    .aria_label("按下快捷键组合进行录制")
                    .track_focus(&capture_focus)
                    .w(theme::scaled_px(176.).max(px(176.)))
                    .h(theme::scaled_px(24.).max(px(24.)))
                    .px(px(8.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(settings_blue())
                    .bg(theme::selected())
                    .flex()
                    .items_center()
                    .text_size(theme::scaled_px(12.))
                    .text_color(theme::foreground())
                    .on_key_down(cx.listener(move |this, event: &gpui_kit::KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            ui_cap_key.update(cx, |ui, cx| {
                                ui.capturing = false;
                                cx.notify();
                            });
                            cx.stop_propagation();
                            cx.notify();
                            return;
                        }
                        if let Some(canonical) = keystroke_to_canonical_string(&event.keystroke) {
                            input_cap_key.update(cx, |input, cx| {
                                input.set_value(canonical, window, cx);
                                input.focus(window, cx);
                            });
                            ui_cap_key.update(cx, |ui, cx| {
                                ui.capturing = false;
                                cx.notify();
                            });
                            this.user_pages.local_error = None;
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
                    .child("请按下组合键… (Esc)")
                    .into_any_element()
            } else {
                div()
                    .w(theme::scaled_px(160.).max(px(160.)))
                    .flex_shrink_0()
                    .child(
                        Input::new(&capture_input)
                            .id("settings-user-hotkey-input")
                            .aria_label("快捷键组合")
                            .small()
                            .disabled(!enabled),
                    )
                    .into_any_element()
            };
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .child(search_box)
                .child(
                    div()
                        .id("settings-user-hotkey-capture-bar")
                        .test_support()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(px(6.))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(theme::scaled_px(12.))
                                .text_color(theme::muted_foreground())
                                .child(format!("{}：", edit_spec.action)),
                        )
                        .child(capture_field)
                        .child(
                            uniform_editor_button(
                                "settings-user-hotkey-record",
                                if capturing { "停止录制" } else { "按键录制" },
                                !enabled,
                            )
                            .px(px(8.))
                            .tooltip("按下真实键盘组合键自动填入快捷键")
                            .on_click(cx.listener(move |_, _, window, cx| {
                                let next_cap = !capturing;
                                ui_rec.update(cx, |ui, cx| {
                                    ui.capturing = next_cap;
                                    cx.notify();
                                });
                                if next_cap {
                                    focus_rec.focus(window, cx);
                                }
                                cx.notify();
                            })),
                        )
                        .child(
                            uniform_editor_button("settings-user-hotkey-save", "保存", !enabled)
                                .px(px(10.))
                                .tooltip("保存当前操作的自定义快捷键")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let raw = match editing::input_snapshot(
                                        &input_save,
                                        window,
                                        cx,
                                    ) {
                                        Ok(v) => v.to_string(),
                                        Err(err) => {
                                            this.user_pages.local_error = Some(err.into());
                                            cx.notify();
                                            return;
                                        }
                                    };
                                    if this.save_hotkey_binding(edit_idx, &raw, window, cx) {
                                        ui_save.update(cx, |ui, cx| {
                                            ui.editing_row = None;
                                            ui.capturing = false;
                                            cx.notify();
                                        });
                                    }
                                })),
                        )
                        .when(edit_idx > 0, |this| {
                            this.child(
                                uniform_editor_button(
                                    "settings-user-hotkey-clear",
                                    "清除",
                                    !enabled,
                                )
                                .px(px(8.))
                                .tooltip("取消该操作的快捷键绑定")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if this.save_hotkey_binding(edit_idx, "", window, cx) {
                                        ui_clear.update(cx, |ui, cx| {
                                            ui.editing_row = None;
                                            ui.capturing = false;
                                            cx.notify();
                                        });
                                    }
                                })),
                            )
                        })
                        .child(
                            uniform_editor_button("settings-user-hotkey-cancel", "取消", false)
                                .px(px(8.))
                                .tooltip("取消编辑快捷键")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.user_pages.local_error = None;
                                    ui_cancel.update(cx, |ui, cx| {
                                        ui.editing_row = None;
                                        ui.capturing = false;
                                        cx.notify();
                                    });
                                    cx.notify();
                                })),
                        ),
                )
        } else if let Some(sel_idx) = selected_row
            && let Some(sel_spec) = HOTKEY_TABLE.get(sel_idx)
        {
            let customized = is_hotkey_customized(sel_idx, sel_spec, misc);
            let sel_raw = effective_hotkey_raw(sel_idx, sel_spec, misc);
            let ui_edit = hotkey_ui.clone();
            let input_edit = capture_input.clone();
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .child(search_box)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            uniform_editor_button(
                                "settings-user-hotkey-edit",
                                "编辑快捷键",
                                !enabled,
                            )
                            .px(px(10.))
                            .tooltip("修改当前选中操作的快捷键绑定")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.user_pages.local_error = None;
                                ui_edit.update(cx, |ui, cx| {
                                    ui.editing_row = Some(sel_idx);
                                    ui.capturing = false;
                                    cx.notify();
                                });
                                input_edit.update(cx, |input, cx| {
                                    input.set_value(sel_raw.clone(), window, cx);
                                    input.focus(window, cx);
                                    input.select_all(window, cx);
                                });
                                cx.notify();
                            })),
                        )
                        .child(
                            uniform_editor_button(
                                "settings-user-hotkey-reset-one",
                                "恢复默认",
                                !enabled || !customized,
                            )
                            .px(px(10.))
                            .tooltip("将当前选中操作的快捷键恢复为默认值")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let _ = this.reset_hotkey_binding(sel_idx, window, cx);
                            })),
                        ),
                )
        } else {
            div().child(search_box)
        };

        let update_status_text = match &self.state.update_status {
            mio_core::UpdateStatus::Idle => {
                format!("当前版本 v{} · 尚未检查更新", env!("CARGO_PKG_VERSION"))
            }
            mio_core::UpdateStatus::Checking => "正在连接 GitHub Releases 检查更新…".into(),
            mio_core::UpdateStatus::UpToDate { current_version } => {
                format!("当前已是最新版本 v{current_version}")
            }
            mio_core::UpdateStatus::Available(info) => {
                format!(
                    "发现新版本 v{}（{}）",
                    info.latest_version, info.asset_name
                )
            }
            mio_core::UpdateStatus::Downloading {
                version,
                received_bytes,
                total_bytes,
            } => match total_bytes {
                Some(total) if *total > 0 => format!(
                    "正在下载 v{version}（{:.1} MB / {:.1} MB）…",
                    *received_bytes as f64 / (1024.0 * 1024.0),
                    *total as f64 / (1024.0 * 1024.0)
                ),
                _ => format!("正在下载 v{version}…"),
            },
            mio_core::UpdateStatus::Verifying { version } => {
                format!("正在校验 v{version} 安装包 SHA-256…")
            }
            mio_core::UpdateStatus::Installing { version } => {
                format!("正在安装 v{version} 并准备重启…")
            }
            mio_core::UpdateStatus::RestartRequired {
                version,
                relaunched,
            } => {
                if *relaunched {
                    format!("v{version} 安装完成，正在重启 MIO…")
                } else {
                    format!("v{version} 已就绪，请重启 MIO 完成更新")
                }
            }
            mio_core::UpdateStatus::Error(err) => format!("更新失败：{err}"),
        };
        let available_update = match &self.state.update_status {
            mio_core::UpdateStatus::Available(info) => Some(info.clone()),
            _ => None,
        };
        let update_busy = matches!(
            self.state.update_status,
            mio_core::UpdateStatus::Checking
                | mio_core::UpdateStatus::Downloading { .. }
                | mio_core::UpdateStatus::Verifying { .. }
                | mio_core::UpdateStatus::Installing { .. }
        );
        let weak_auto_upd = cx.weak_entity();
        let auto_check_cb = settings_checkbox(
            "settings-user-auto-check-updates",
            "启动时自动检查 GitHub Releases 新版本",
            misc.auto_check_updates,
            enabled,
            &self.user_pages.auto_check_updates_focus,
            window,
        )
        .on_change(move |state, _, window, cx| {
            let checked = state == CheckboxState::Checked;
            let _ = weak_auto_upd.update(cx, |this, cx| {
                this.toggle_user_misc(|m| m.auto_check_updates = checked, window, cx);
            });
        });

        let update_section = div()
            .id("settings-user-updates-section")
            .test_support()
            .mt(px(12.))
            .pt(px(14.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(user_section_heading(
                Some("settings-user-updates-heading"),
                Some("settings-user-updates-scope"),
                "软件更新",
            ))
            .child(user_desc_row_with_icon(
                user_gear_static_icon(),
                "检查 GitHub Releases 中的 MIO 新版本，校验 SHA-256 后自动下载、替换并重启。",
            ))
            .child(
                div()
                    .pl(px(29.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(auto_check_cb)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                uniform_editor_button(
                                    "settings-user-check-updates",
                                    "检查更新",
                                    update_busy,
                                )
                                .px(px(12.))
                                .tooltip("立即查询 GitHub Releases 最新版本")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.check_for_updates(cx);
                                })),
                            )
                            .when_some(available_update, |row, info| {
                                let label = format!("立即下载并更新 (v{})", info.latest_version);
                                row.child(
                                    button("settings-user-install-update", label)
                                        .primary()
                                        .disabled(update_busy)
                                        .h(theme::scaled_px(24.).max(px(24.)))
                                        .px(px(12.))
                                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                        .tooltip("下载最新安装包、校验 SHA-256 并自动替换重启")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.install_available_update(info.clone(), cx);
                                        })),
                                )
                            })
                            .child(
                                div()
                                    .id("settings-user-update-status")
                                    .test_support()
                                    .aria_label(update_status_text.clone())
                                    .text_size(theme::scaled_px(12.))
                                    .text_color(theme::muted_foreground())
                                    .child(update_status_text),
                            ),
                    ),
            );

        let body = div()
            .id("settings-user-misc-inner")
            .px(px(22.))
            .py(px(16.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(user_section_heading(
                Some("settings-user-misc-heading"),
                Some("settings-user-misc-scope"),
                "Hotkeys",
            ))
            .child(user_desc_row_with_icon(
                hotkeys_gear,
                "使用以下设置配置常用操作的快捷键。",
            ))
            .child(
                div()
                    .pl(px(29.))
                    .pt(px(2.))
                    .text_color(theme::foreground())
                    .child("选择一个操作以更改其快捷键。所有快捷键都必须包含 Command 键（Mac）或 Control 键（Windows），也可组合 Shift 等修饰键。"),
            );
        let body = self
            .append_user_pages_feedback(body, cx)
            .child(
                div()
                    .pl(px(29.))
                    .pt(px(4.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(toolbar_row)
                    .child(hotkey_table),
            )
            .child(update_section);

        settings_scrollable_pane(
            "settings-user-misc-page",
            &self.user_pages.misc_scroll,
            body,
        )
        .into_any_element()
    }
}
