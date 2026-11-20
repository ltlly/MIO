use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

const MIN_UI_FONT_SIZE_PX: u8 = 11;
const MAX_UI_FONT_SIZE_PX: u8 = 18;
const MIN_SCALE_FACTOR_PERCENT: u16 = 80;
const MAX_SCALE_FACTOR_PERCENT: u16 = 150;
const MIN_TABLE_CELL_PREVIEW_CHARS: u16 = 64;
const MAX_TABLE_CELL_PREVIEW_CHARS: u16 = 1024;
const MIN_EDITOR_FONT_SIZE_PX: u8 = 10;
const MAX_EDITOR_FONT_SIZE_PX: u8 = 22;
const MAX_HOTKEY_BYTES: usize = 64;

const fn default_ui_font_size_px() -> u8 {
    13
}
const fn default_scale_factor_percent() -> u16 {
    100
}
const fn default_table_cell_preview_chars() -> u16 {
    256
}
const fn default_editor_font_size_px() -> u8 {
    13
}
const fn default_true() -> bool {
    true
}
fn default_utf8() -> String {
    "UTF-8".into()
}
fn default_hotkey_send_request() -> String {
    "cmd-enter".into()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserThemeMode {
    #[default]
    Light,
    Dark,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserDisplaySettings {
    #[serde(default)]
    pub theme: UserThemeMode,
    #[serde(default = "default_ui_font_size_px")]
    pub ui_font_size_px: u8,
    #[serde(default = "default_scale_factor_percent")]
    pub scale_factor_percent: u16,
    #[serde(default = "default_table_cell_preview_chars")]
    pub max_table_cell_preview_chars: u16,
    #[serde(default = "default_true")]
    pub table_zebra_stripe: bool,
    #[serde(default = "default_true")]
    pub issues_grouped_evidence: bool,
    #[serde(default)]
    pub use_system_dpi: bool,
}

impl Default for UserDisplaySettings {
    fn default() -> Self {
        Self {
            theme: UserThemeMode::Light,
            ui_font_size_px: default_ui_font_size_px(),
            scale_factor_percent: default_scale_factor_percent(),
            max_table_cell_preview_chars: default_table_cell_preview_chars(),
            table_zebra_stripe: true,
            issues_grouped_evidence: true,
            use_system_dpi: false,
        }
    }
}

impl UserDisplaySettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            (MIN_UI_FONT_SIZE_PX..=MAX_UI_FONT_SIZE_PX).contains(&self.ui_font_size_px),
            "界面字号必须在 {MIN_UI_FONT_SIZE_PX} 到 {MAX_UI_FONT_SIZE_PX} pt 之间"
        );
        ensure!(
            (MIN_SCALE_FACTOR_PERCENT..=MAX_SCALE_FACTOR_PERCENT)
                .contains(&self.scale_factor_percent),
            "缩放比例必须在 {MIN_SCALE_FACTOR_PERCENT}% 到 {MAX_SCALE_FACTOR_PERCENT}% 之间"
        );
        ensure!(
            (MIN_TABLE_CELL_PREVIEW_CHARS..=MAX_TABLE_CELL_PREVIEW_CHARS)
                .contains(&self.max_table_cell_preview_chars),
            "表格单元格最大预览字符数必须在 {MIN_TABLE_CELL_PREVIEW_CHARS} 到 {MAX_TABLE_CELL_PREVIEW_CHARS} 之间"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserMessageEditorSettings {
    #[serde(default = "default_editor_font_size_px")]
    pub font_size_px: u8,
    #[serde(default = "default_true")]
    pub soft_wrap: bool,
    #[serde(default = "default_true")]
    pub pretty_print_by_default: bool,
    #[serde(default)]
    pub auto_expand_inspector: bool,
    #[serde(default)]
    pub highlight_search_case_sensitive_default: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden_request_headers: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden_response_headers: Option<Vec<String>>,
    #[serde(default)]
    pub hide_request_headers_by_default: bool,
    #[serde(default)]
    pub hide_response_headers_by_default: bool,
    #[serde(default)]
    pub charset_mode: u8,
    #[serde(default = "default_utf8")]
    pub specific_charset: String,
    #[serde(default = "default_true")]
    pub html_render_external_requests: bool,
    #[serde(default)]
    pub search_regex_default: bool,
    #[serde(default)]
    pub search_autoscroll_default: bool,
    #[serde(default)]
    pub url_encode_as_you_type: bool,
}

impl Default for UserMessageEditorSettings {
    fn default() -> Self {
        Self {
            font_size_px: default_editor_font_size_px(),
            soft_wrap: true,
            pretty_print_by_default: true,
            auto_expand_inspector: false,
            highlight_search_case_sensitive_default: false,
            hidden_request_headers: None,
            hidden_response_headers: None,
            hide_request_headers_by_default: false,
            hide_response_headers_by_default: false,
            charset_mode: 0,
            specific_charset: default_utf8(),
            html_render_external_requests: true,
            search_regex_default: false,
            search_autoscroll_default: false,
            url_encode_as_you_type: false,
        }
    }
}

impl UserMessageEditorSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            (MIN_EDITOR_FONT_SIZE_PX..=MAX_EDITOR_FONT_SIZE_PX).contains(&self.font_size_px),
            "消息编辑器等宽字号必须在 {MIN_EDITOR_FONT_SIZE_PX} 到 {MAX_EDITOR_FONT_SIZE_PX} pt 之间"
        );
        ensure!(self.charset_mode <= 3, "消息编辑器字符集模式无效");
        let charset_trimmed = self.specific_charset.trim();
        ensure!(
            !charset_trimmed.is_empty()
                && self.specific_charset.len() <= 64
                && !self.specific_charset.chars().any(char::is_control),
            "消息编辑器指定字符集不能为空、包含控制字符或超过 64 字节"
        );
        for names in [&self.hidden_request_headers, &self.hidden_response_headers]
            .into_iter()
            .flatten()
        {
            Self::validate_hidden_header_names(names)?;
        }
        Ok(())
    }

    pub fn validate_hidden_header_names(names: &[String]) -> Result<()> {
        for name in names {
            ensure!(!name.is_empty(), "指定隐藏的标头名称不能为空。");
            let probe = format!("{name}: x\r\n\r\n");
            let mut headers = [httparse::EMPTY_HEADER; 2];
            ensure!(
                matches!(
                    httparse::parse_headers(probe.as_bytes(), &mut headers),
                    Ok(httparse::Status::Complete((offset, parsed)))
                        if offset == probe.len() && parsed.len() == 1 && parsed[0].name == name
                ),
                "标头名称“{name}”不是合法的 HTTP 标头 token。"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserMiscSettings {
    #[serde(default = "default_true")]
    pub confirm_on_close_tab_with_unsaved_draft: bool,
    #[serde(default)]
    pub redact_sensitive_headers_in_copy_curl: bool,
    #[serde(default = "default_hotkey_send_request")]
    pub hotkey_send_request: String,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub custom_hotkeys: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub intruder_side_panel_left: bool,
    #[serde(default = "default_true")]
    pub intruder_side_panel_auto_expand: bool,
    #[serde(default)]
    pub intruder_auto_payload_append: bool,
    #[serde(default)]
    pub intruder_new_tab_config_mode: u8,
    #[serde(default)]
    pub intruder_onboarding_dismissed: bool,
    #[serde(default)]
    pub collaborator_onboarding_dismissed: bool,
    #[serde(default = "default_true")]
    pub auto_check_updates: bool,
}

impl Default for UserMiscSettings {
    fn default() -> Self {
        Self {
            confirm_on_close_tab_with_unsaved_draft: true,
            redact_sensitive_headers_in_copy_curl: false,
            hotkey_send_request: default_hotkey_send_request(),
            custom_hotkeys: std::collections::BTreeMap::new(),
            intruder_side_panel_left: false,
            intruder_side_panel_auto_expand: true,
            intruder_auto_payload_append: false,
            intruder_new_tab_config_mode: 0,
            intruder_onboarding_dismissed: false,
            collaborator_onboarding_dismissed: false,
            auto_check_updates: true,
        }
    }
}

impl UserMiscSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn validate(&self) -> Result<()> {
        let trimmed = self.hotkey_send_request.trim();
        ensure!(
            !trimmed.is_empty()
                && self.hotkey_send_request.len() <= MAX_HOTKEY_BYTES
                && trimmed == self.hotkey_send_request
                && !self.hotkey_send_request.chars().any(char::is_control),
            "发送请求快捷键不能为空、包含首尾空白/控制字符或超过 {MAX_HOTKEY_BYTES} 字节"
        );
        ensure!(
            self.intruder_new_tab_config_mode <= 2,
            "Intruder 新建标签配置模式无效"
        );
        ensure!(
            self.custom_hotkeys.len() <= 128,
            "自定义快捷键数量超过 128 条上限"
        );
        for (action, binding) in &self.custom_hotkeys {
            ensure!(
                !action.trim().is_empty()
                    && action.len() <= 128
                    && !action.chars().any(char::is_control)
                    && binding.len() <= MAX_HOTKEY_BYTES
                    && !binding.chars().any(char::is_control),
                "自定义快捷键键名或绑定格式无效"
            );
        }
        Ok(())
    }
}
