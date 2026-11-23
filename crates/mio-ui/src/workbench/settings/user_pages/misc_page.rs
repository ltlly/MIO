use gpui_kit::accesskit::Role;
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::base::CheckboxState;
use gpui_kit::component::{
    Icon,
    menu::{DropdownMenu, PopupMenuItem},
    scroll::{Scrollbar, ScrollbarMode},
};

use super::message_editor_page::uniform_editor_button;
use super::*;

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

const HOTKEY_TABLE: [HotkeyRowSpec; 34] = [
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
        default_key: "cmd-o",
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
        default_key: "cmd-alt-m",
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
    HotkeyRowSpec {
        action_id: "new_window",
        icon: AssetIconName::CirclePlus,
        action: "新建窗口",
        category: "窗口",
        default_key: "cmd-shift-n",
    },
];

fn is_function_key(key: &str) -> bool {
    matches!(
        key,
        "f1" | "f2"
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

fn format_hotkey_display(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if matches!(trimmed, "cmd-enter" | "ctrl-enter" | "cmd-shift-enter") {
        return send_hotkey_display_label(trimmed);
    }
    let (prefix_str, trailing_key) = if trimmed == "-" || trimmed.ends_with("--") {
        (&trimmed[..trimmed.len() - 1], Some("-".to_owned()))
    } else if trimmed == "+" || trimmed.ends_with("-+") {
        (&trimmed[..trimmed.len() - 1], Some("+".to_owned()))
    } else {
        (trimmed, None)
    };
    let mut out = String::new();
    let mut key_part = String::new();
    for token in prefix_str.split('-').filter(|t| !t.is_empty()) {
        match token.to_ascii_lowercase().as_str() {
            "cmd" | "command" | "platform" | "meta" => out.push('⌘'),
            "ctrl" | "control" => out.push('⌃'),
            "alt" | "option" => out.push('⌥'),
            "shift" => out.push('⇧'),
            "enter" | "return" => key_part = "Enter".into(),
            "period" | "." => key_part = ".".into(),
            "comma" | "," => key_part = ",".into(),
            "minus" => key_part = "-".into(),
            "equal" => key_part = "=".into(),
            "space" => key_part = "Space".into(),
            "escape" | "esc" => key_part = "Esc".into(),
            "backspace" => key_part = "Backspace".into(),
            "tab" => key_part = "Tab".into(),
            other if other.len() == 1 && other.is_ascii() => {
                key_part = other.to_ascii_uppercase();
            }
            other if is_function_key(&other.to_ascii_lowercase()) => {
                key_part = other.to_ascii_uppercase();
            }
            other => key_part = other.to_owned(),
        }
    }
    if let Some(tk) = trailing_key {
        key_part = tk;
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
    let chars: Vec<char> = trimmed.chars().collect();
    let mut expanded = String::with_capacity(trimmed.len() + 16);
    for (idx, &ch) in chars.iter().enumerate() {
        let is_last = idx + 1 == chars.len();
        match ch {
            '⌘' => expanded.push_str("cmd-"),
            '⌃' => expanded.push_str("ctrl-"),
            '⌥' => expanded.push_str("alt-"),
            '⇧' => expanded.push_str("shift-"),
            ' ' => expanded.push('-'),
            '+' if !is_last => expanded.push('-'),
            other => expanded.push(other),
        }
    }
    let (prefix_slice, forced_key) = if expanded == "-" {
        ("", Some("-".to_owned()))
    } else if expanded.ends_with("--") || expanded.ends_with("-+") {
        let last = expanded.chars().last().unwrap_or('-');
        (&expanded[..expanded.len() - 1], Some(last.to_string()))
    } else if expanded.ends_with('+') {
        (&expanded[..expanded.len() - 1], None)
    } else {
        (expanded.as_str(), None)
    };
    let mut has_cmd = false;
    let mut has_ctrl = false;
    let mut has_alt = false;
    let mut has_shift = false;
    let mut key: Option<String> = forced_key;
    for token in prefix_slice.split('-').filter(|t| !t.is_empty()) {
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
    let Some(key_name) = key else {
        return Err("请输入完整的快捷键组合（例如 cmd-shift-r、ctrl-k 或 f5）。".into());
    };
    if !is_valid_hotkey_key_name(&key_name) {
        return Err(format!(
            "无法识别快捷键组合“{trimmed}”，请使用如 cmd-shift-r 格式。"
        ));
    }
    if !has_cmd && !has_ctrl && !is_function_key(&key_name) {
        return Err("非功能键快捷键都必须包含 Command (cmd) 或 Control (ctrl) 修饰键。".into());
    }
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
    Ok(parts.join("-"))
}

fn is_valid_hotkey_key_name(key: &str) -> bool {
    if key.len() == 1 {
        let b = key.as_bytes()[0];
        return b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'.' | b',' | b'/' | b'\\' | b'-' | b'+' | b'=' | b'[' | b']' | b';' | b'\'' | b'`'
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
    ) || is_function_key(key)
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
    let key_name = match key_lower.as_str() {
        "." => "period",
        "return" => "enter",
        other => other,
    };
    if !is_valid_hotkey_key_name(key_name) {
        return None;
    }
    let m = keystroke.modifiers;
    if !m.platform && !m.control && !is_function_key(key_name) {
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

impl Settings {
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
        if !normalized.is_empty()
            && let Some((_, conflict)) = HOTKEY_TABLE.iter().enumerate().find(|&(idx, other)| {
                idx != row_index
                    && effective_hotkey_raw(idx, other, &misc).eq_ignore_ascii_case(&normalized)
            })
        {
            self.user_pages.local_error = Some(format!(
                "快捷键 {} 已被“{}”占用，请先清除或更换其他组合。",
                format_hotkey_display(&normalized),
                conflict.action
            ));
            cx.notify();
            return false;
        }
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

    #[inline(never)]
    fn render_user_misc_hotkey_table(
        &self,
        hotkey_ui: &Entity<HotkeyEditorUi>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let misc = &self.state.preferences.misc;
        let (selected_row, editing_row, capture_input) = {
            let ui = hotkey_ui.read(cx);
            (ui.selected_row, ui.editing_row, ui.capture_input.clone())
        };
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

        div()
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
            )
            .into_any_element()
    }

    #[inline(never)]
    fn render_user_misc_hotkey_toolbar(
        &self,
        hotkey_ui: &Entity<HotkeyEditorUi>,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let misc = &self.state.preferences.misc;
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

        if let Some(edit_idx) = editing_row
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
                .into_any_element()
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
                .into_any_element()
        } else {
            div().child(search_box).into_any_element()
        }
    }

    #[inline(never)]
    fn render_user_misc_updates_section(
        &self,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let misc = &self.state.preferences.misc;
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

        div()
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
            )
            .into_any_element()
    }

    #[inline(never)]
    pub(in crate::workbench) fn render_user_misc(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let _ = &self.user_pages.send_hotkey_select;
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
                let _ = owner.update(cx, |this, cx| this.reset_user_hotkeys_defaults(window, cx));
            }))
        });

        let weak_close = cx.weak_entity();
        let confirm_close_cb = settings_checkbox(
            "settings-user-confirm-close-unsaved",
            "关闭含未保存草稿的标签页前确认",
            misc.confirm_on_close_tab_with_unsaved_draft,
            enabled,
            &self.user_pages.confirm_close_focus,
            window,
        )
        .on_change(move |state, _, window, cx| {
            let checked = state == CheckboxState::Checked;
            let _ = weak_close.update(cx, |this, cx| {
                this.toggle_user_misc(
                    |m| m.confirm_on_close_tab_with_unsaved_draft = checked,
                    window,
                    cx,
                );
            });
        });

        let weak_redact = cx.weak_entity();
        let redact_curl_cb = settings_checkbox(
            "settings-user-redact-curl",
            "复制为 cURL 时脱敏敏感请求头",
            misc.redact_sensitive_headers_in_copy_curl,
            enabled,
            &self.user_pages.redact_curl_focus,
            window,
        )
        .on_change(move |state, _, window, cx| {
            let checked = state == CheckboxState::Checked;
            let _ = weak_redact.update(cx, |this, cx| {
                this.toggle_user_misc(
                    |m| m.redact_sensitive_headers_in_copy_curl = checked,
                    window,
                    cx,
                );
            });
        });

        let behavior_section = div()
            .id("settings-user-behavior-section")
            .test_support()
            .mt(px(12.))
            .pt(px(14.))
            .border_t_1()
            .border_color(theme::border())
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(user_section_heading(
                Some("settings-user-behavior-heading"),
                Some("settings-user-behavior-scope"),
                "通用行为",
            ))
            .child(user_desc_row_with_icon(
                user_gear_static_icon(),
                "配置跨项目生效的标签页关闭确认与导出脱敏安全选项。",
            ))
            .child(
                div()
                    .pl(px(29.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(confirm_close_cb)
                    .child(redact_curl_cb),
            );

        let hotkey_table = self.render_user_misc_hotkey_table(&hotkey_ui, window, cx);
        let toolbar_row = self.render_user_misc_hotkey_toolbar(&hotkey_ui, enabled, cx);
        let update_section = self.render_user_misc_updates_section(enabled, window, cx);

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
                    .child("选择一个操作以更改其快捷键。除 F1–F12 功能键外，快捷键须包含 Command 键（Mac）或 Control 键（Windows/Linux），也可组合 Alt/Shift 修饰键。"),
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
            .child(behavior_section)
            .child(update_section);

        settings_scrollable_pane(
            "settings-user-misc-page",
            &self.user_pages.misc_scroll,
            body,
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod hotkey_tests {
    use super::*;

    #[test]
    fn normalize_hotkey_input_handles_minus_plus_and_function_keys() {
        assert_eq!(normalize_hotkey_input("cmd--").unwrap(), "cmd--");
        assert_eq!(normalize_hotkey_input("⌘-").unwrap(), "cmd--");
        assert_eq!(format_hotkey_display("cmd--"), "⌘-");
        assert_eq!(normalize_hotkey_input("⌘+").unwrap(), "cmd-+");
        assert_eq!(normalize_hotkey_input("f5").unwrap(), "f5");
        assert_eq!(normalize_hotkey_input("F12").unwrap(), "f12");
        assert_eq!(format_hotkey_display("f5"), "F5");
        assert_eq!(normalize_hotkey_input("cmd-shift-r").unwrap(), "cmd-shift-r");
        assert!(normalize_hotkey_input("r").is_err());
        assert!(normalize_hotkey_input("cmd-").is_err());
    }
}
