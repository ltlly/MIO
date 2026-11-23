use std::{cell::RefCell, collections::BTreeMap};

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Global, Hsla, KeyBinding, Pixels, px, rgb};
use mio_core::{UserDisplaySettings, UserMessageEditorSettings, UserMiscSettings, UserThemeMode};

pub(crate) const ROW_HEIGHT: f32 = 19.;
pub(crate) const TOOLBAR_HEIGHT: f32 = 33.;
pub(crate) const TOOL_TAB_HEIGHT: f32 = 42.;
#[cfg(test)]
pub(crate) const WORK_TAB_BAR_HEIGHT: f32 = 47.;
pub(crate) const SECONDARY_TAB_HEIGHT: f32 = 36.;
pub(crate) const EDITOR_LINE_HEIGHT: f32 = 17.5 / 13.;

pub(crate) fn ui_scale() -> f32 {
    (ui_font_size().as_f32() / 13.0).max(0.75)
}

pub(crate) fn scaled_px(base: f32) -> Pixels {
    px((base * ui_scale()).round())
}

pub(crate) fn row_height() -> Pixels {
    scaled_px(ROW_HEIGHT).max(px(18.))
}

pub(crate) fn table_header_height() -> Pixels {
    scaled_px(25.).max(px(20.))
}

pub(crate) fn control_height() -> Pixels {
    scaled_px(24.).max(px(20.))
}

pub(crate) fn toolbar_height() -> Pixels {
    scaled_px(TOOLBAR_HEIGHT).max(px(26.))
}

pub(crate) fn tool_tab_height() -> Pixels {
    scaled_px(TOOL_TAB_HEIGHT).max(px(34.))
}

#[cfg(test)]
pub(crate) fn work_tab_bar_height() -> Pixels {
    scaled_px(WORK_TAB_BAR_HEIGHT).max(px(38.))
}

pub(crate) fn secondary_tab_height() -> Pixels {
    scaled_px(SECONDARY_TAB_HEIGHT).max(px(29.))
}

pub(crate) const fn ui_font_family() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        ".SystemUIFont"
    }
    #[cfg(target_os = "windows")]
    {
        "Segoe UI"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "DejaVu Sans"
    }
}

pub(crate) const fn svg_ui_font_family() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "PingFang SC, Helvetica Neue"
    }
    #[cfg(target_os = "windows")]
    {
        "Segoe UI"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "DejaVu Sans"
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UiPreferencesState {
    pub dark_mode: bool,
    pub ui_font_size_px: u8,
    pub scale_factor_percent: u16,
    pub system_dpi_scale_percent: u16,
    pub user_custom_scale_percent: u16,
    pub table_cell_preview_chars: u16,
    pub table_zebra_stripe: bool,
    pub issues_grouped_evidence: bool,
    pub use_system_dpi: bool,
    pub editor_font_size_px: u8,
    pub editor_soft_wrap: bool,
    pub pretty_print_by_default: bool,
    pub auto_expand_inspector: bool,
    pub highlight_search_case_sensitive_default: bool,
    pub search_regex_default: bool,
    pub search_autoscroll_default: bool,
    pub html_render_external_requests: bool,
    pub editor_charset_mode: u8,
    pub editor_specific_charset: String,
    pub confirm_on_close_tab_with_unsaved_draft: bool,
    pub redact_sensitive_headers_in_copy_curl: bool,
    pub hotkey_send_request: String,
    pub custom_hotkeys: BTreeMap<String, String>,
    pub hidden_request_headers: Option<Vec<String>>,
    pub hidden_response_headers: Option<Vec<String>>,
    pub hide_request_headers_by_default: bool,
    pub hide_response_headers_by_default: bool,
    pub url_encode_as_you_type: bool,
}

impl Default for UiPreferencesState {
    fn default() -> Self {
        Self {
            dark_mode: false,
            ui_font_size_px: 13,
            scale_factor_percent: 100,
            system_dpi_scale_percent: 100,
            user_custom_scale_percent: 100,
            table_cell_preview_chars: 256,
            table_zebra_stripe: true,
            issues_grouped_evidence: true,
            use_system_dpi: false,
            editor_font_size_px: 13,
            editor_soft_wrap: true,
            pretty_print_by_default: true,
            auto_expand_inspector: false,
            highlight_search_case_sensitive_default: false,
            search_regex_default: false,
            search_autoscroll_default: false,
            html_render_external_requests: true,
            editor_charset_mode: 0,
            editor_specific_charset: "UTF-8".into(),
            confirm_on_close_tab_with_unsaved_draft: true,
            redact_sensitive_headers_in_copy_curl: false,
            hotkey_send_request: "cmd-enter".into(),
            custom_hotkeys: BTreeMap::new(),
            hidden_request_headers: None,
            hidden_response_headers: None,
            hide_request_headers_by_default: false,
            hide_response_headers_by_default: false,
            url_encode_as_you_type: false,
        }
    }
}

impl Global for UiPreferencesState {}

thread_local! {
    static ACTIVE_UI_STATE: RefCell<UiPreferencesState> =
        RefCell::new(UiPreferencesState::default());
}

fn read_state<R>(f: impl FnOnce(&UiPreferencesState) -> R) -> R {
    ACTIVE_UI_STATE.with(|state| f(&state.borrow()))
}

fn update_state(cx: &mut App, f: impl FnOnce(&mut UiPreferencesState)) {
    if cx.try_global::<UiPreferencesState>().is_none() {
        cx.set_global(UiPreferencesState::default());
    }
    let snapshot = {
        let state = cx.global_mut::<UiPreferencesState>();
        f(state);
        state.clone()
    };
    ACTIVE_UI_STATE.with(|cell| *cell.borrow_mut() = snapshot);
}

pub(crate) fn is_dark() -> bool {
    read_state(|s| s.dark_mode)
}

#[cfg(test)]
pub(crate) fn mode() -> ThemeMode {
    if is_dark() {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    }
}

pub(crate) fn ui_font_size_px() -> u8 {
    read_state(|s| s.ui_font_size_px)
}

pub(crate) fn scale_factor_percent() -> u16 {
    if use_system_dpi() {
        return system_dpi_scale_percent();
    }
    read_state(|s| s.scale_factor_percent)
}

pub(crate) fn system_dpi_scale_percent() -> u16 {
    read_state(|s| s.system_dpi_scale_percent.clamp(80, 200))
}

#[allow(dead_code)]
pub(crate) fn user_custom_scale_percent() -> u16 {
    read_state(|s| s.user_custom_scale_percent)
}

fn compute_system_dpi_scale_percent(window_scale_factor: f32, is_macos: bool) -> u16 {
    if !window_scale_factor.is_finite() || window_scale_factor <= 0.0 {
        return 100;
    }
    // macOS Retina displays report 2.0 (or 3.0), whereas non-Retina external displays report 1.0
    // (or fractional HiDPI <= 1.5); treat 2.0+ on macOS as the 100% Retina baseline without
    // downscaling 1.0x displays to 80%.
    let normal_base = if is_macos && window_scale_factor >= 1.75 {
        2.0
    } else {
        1.0
    };
    let ratio = (window_scale_factor / normal_base).clamp(0.8, 2.0);
    (ratio * 100.0).round() as u16
}

pub(crate) fn sync_window_system_dpi(window_scale_factor: f32, cx: &mut App) {
    if cfg!(test) {
        let _ = (window_scale_factor, cx);
        return;
    }
    let pct = compute_system_dpi_scale_percent(window_scale_factor, cfg!(target_os = "macos"));
    if read_state(|s| s.system_dpi_scale_percent == pct)
        && cx
            .try_global::<UiPreferencesState>()
            .is_some_and(|s| s.system_dpi_scale_percent == pct)
    {
        return;
    }
    update_state(cx, |s| s.system_dpi_scale_percent = pct);
}

#[cfg(test)]
mod system_dpi_tests {
    use super::compute_system_dpi_scale_percent;

    #[test]
    fn macos_non_retina_and_retina_baselines_do_not_downscale_1x_displays() {
        assert_eq!(compute_system_dpi_scale_percent(1.0, true), 100);
        assert_eq!(compute_system_dpi_scale_percent(1.25, true), 125);
        assert_eq!(compute_system_dpi_scale_percent(1.5, true), 150);
        assert_eq!(compute_system_dpi_scale_percent(2.0, true), 100);
        assert_eq!(compute_system_dpi_scale_percent(3.0, true), 150);
        assert_eq!(compute_system_dpi_scale_percent(0.0, true), 100);
        assert_eq!(compute_system_dpi_scale_percent(f32::NAN, true), 100);

        assert_eq!(compute_system_dpi_scale_percent(1.0, false), 100);
        assert_eq!(compute_system_dpi_scale_percent(1.25, false), 125);
        assert_eq!(compute_system_dpi_scale_percent(1.5, false), 150);
        assert_eq!(compute_system_dpi_scale_percent(2.0, false), 200);
    }
}

pub(crate) fn table_cell_preview_chars() -> usize {
    read_state(|s| usize::from(s.table_cell_preview_chars))
}

pub(crate) fn truncate_cell_preview(text: &str) -> String {
    let max_chars = table_cell_preview_chars();
    if let Some((byte_idx, _)) = text.char_indices().nth(max_chars) {
        let mut out = String::with_capacity(byte_idx + 3);
        out.push_str(&text[..byte_idx]);
        out.push('…');
        out
    } else {
        text.to_owned()
    }
}

pub(crate) fn ui_font_size() -> Pixels {
    let scale = f32::from(scale_factor_percent()) / 100.0;
    px((f32::from(ui_font_size_px()) * scale * 10.0).round() / 10.0)
}

pub(crate) fn editor_font_size_px() -> u8 {
    read_state(|s| s.editor_font_size_px)
}

pub(crate) fn editor_font_size() -> Pixels {
    let scale = f32::from(scale_factor_percent()) / 100.0;
    px((f32::from(editor_font_size_px()) * scale * 10.0).round() / 10.0)
}

pub(crate) fn editor_soft_wrap() -> bool {
    read_state(|s| s.editor_soft_wrap)
}

pub(crate) fn pretty_print_by_default() -> bool {
    read_state(|s| s.pretty_print_by_default)
}

pub(crate) fn auto_expand_inspector() -> bool {
    read_state(|s| s.auto_expand_inspector)
}

pub(crate) fn highlight_search_case_sensitive_default() -> bool {
    read_state(|s| s.highlight_search_case_sensitive_default)
}

pub(crate) fn table_zebra_stripe() -> bool {
    read_state(|s| s.table_zebra_stripe)
}

pub(crate) fn issues_grouped_evidence() -> bool {
    read_state(|s| s.issues_grouped_evidence)
}

pub(crate) fn use_system_dpi() -> bool {
    read_state(|s| s.use_system_dpi)
}

#[allow(dead_code)]
pub(crate) fn html_render_external_requests() -> bool {
    read_state(|s| s.html_render_external_requests)
}

pub(crate) fn message_editor_search_regex_default() -> bool {
    read_state(|s| s.search_regex_default)
}

pub(crate) fn message_editor_search_autoscroll_default() -> bool {
    read_state(|s| s.search_autoscroll_default)
}

pub(crate) fn message_editor_charset() -> (u8, String) {
    read_state(|s| (s.editor_charset_mode.min(3), s.editor_specific_charset.clone()))
}

pub(crate) fn confirm_on_close_tab_with_unsaved_draft() -> bool {
    read_state(|s| s.confirm_on_close_tab_with_unsaved_draft)
}

pub(crate) fn redact_sensitive_headers_in_copy_curl() -> bool {
    read_state(|s| s.redact_sensitive_headers_in_copy_curl)
}

pub(crate) fn hotkey_send_request() -> String {
    read_state(|s| s.hotkey_send_request.clone())
}

#[allow(dead_code)]
pub(crate) fn custom_hotkeys() -> BTreeMap<String, String> {
    read_state(|s| s.custom_hotkeys.clone())
}

pub(crate) fn hidden_request_headers() -> Option<Vec<String>> {
    read_state(|s| s.hidden_request_headers.clone())
}

pub(crate) fn hide_headers_by_default(response: bool) -> bool {
    read_state(|s| {
        if response {
            s.hide_response_headers_by_default
        } else {
            s.hide_request_headers_by_default
        }
    })
}

pub(crate) fn hidden_response_headers() -> Option<Vec<String>> {
    read_state(|s| s.hidden_response_headers.clone())
}

pub(crate) fn url_encode_as_you_type() -> bool {
    read_state(|s| s.url_encode_as_you_type)
}

pub(crate) fn set_url_encode_as_you_type(enabled: bool, cx: &mut App) {
    if read_state(|s| s.url_encode_as_you_type == enabled)
        && cx
            .try_global::<UiPreferencesState>()
            .is_some_and(|s| s.url_encode_as_you_type == enabled)
    {
        return;
    }
    update_state(cx, |s| s.url_encode_as_you_type = enabled);
}

pub(crate) fn border() -> Hsla {
    rgb(if is_dark() { 0x3c3f41 } else { 0xdfdfdf }).into()
}
pub(crate) fn control_border() -> Hsla {
    rgb(if is_dark() { 0x4e535b } else { 0xdfdfdf }).into()
}
pub(crate) fn background() -> Hsla {
    rgb(if is_dark() { 0x1e1f22 } else { 0xfbfbfb }).into()
}
pub(crate) fn surface() -> Hsla {
    rgb(if is_dark() { 0x2b2d30 } else { 0xffffff }).into()
}
pub(crate) fn toolbar_surface() -> Hsla {
    rgb(if is_dark() { 0x25272b } else { 0xfbfbfb }).into()
}
pub(crate) fn foreground() -> Hsla {
    rgb(if is_dark() { 0xdfe1e5 } else { 0x2b2b2b }).into()
}
pub(crate) fn muted_foreground() -> Hsla {
    rgb(if is_dark() { 0x9da2a8 } else { 0x787878 }).into()
}
pub(crate) fn accent() -> Hsla {
    rgb(0xdb612f).into()
}
pub(crate) fn selected() -> Hsla {
    rgb(if is_dark() { 0x2e436e } else { 0xcadaf0 }).into()
}
pub(crate) fn danger_fg() -> Hsla {
    rgb(if is_dark() { 0xf28b82 } else { 0xb42318 }).into()
}
pub(crate) fn danger_bg() -> Hsla {
    rgb(if is_dark() { 0x4a2020 } else { 0xfef3f2 }).into()
}
pub(crate) fn danger_border() -> Hsla {
    rgb(if is_dark() { 0x6b2c2a } else { 0xfecdca }).into()
}

pub(crate) fn init(cx: &mut App) {
    cx.set_global(UiPreferencesState::default());
    ACTIVE_UI_STATE.with(|cell| *cell.borrow_mut() = UiPreferencesState::default());
    apply_user_display(
        &UserDisplaySettings::default(),
        &UserMessageEditorSettings::default(),
        cx,
    );
    apply_user_misc(&UserMiscSettings::default(), cx);
}

pub(crate) const fn mono_font_family() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "Menlo"
    }
    #[cfg(target_os = "windows")]
    {
        "Consolas"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "DejaVu Sans Mono"
    }
}

pub(crate) fn apply_user_display(
    display: &UserDisplaySettings,
    editor: &UserMessageEditorSettings,
    cx: &mut App,
) {
    editor.validate().expect("applied message editor preferences must be validated");
    let dark = matches!(display.theme, UserThemeMode::Dark);
    let ui_px = display.ui_font_size_px.clamp(11, 18);
    let custom_scale_pct = display.scale_factor_percent.clamp(80, 150);
    let scale_pct = if display.use_system_dpi {
        system_dpi_scale_percent()
    } else {
        custom_scale_pct
    };
    let preview_chars = display.max_table_cell_preview_chars.clamp(64, 1024);
    let ed_px = editor.font_size_px.clamp(10, 22);
    let charset_mode = editor.charset_mode.min(3);
    let specific_charset = if editor.specific_charset.trim().is_empty() {
        "UTF-8".to_owned()
    } else {
        editor.specific_charset.trim().to_owned()
    };
    update_state(cx, |s| {
        s.dark_mode = dark;
        s.ui_font_size_px = ui_px;
        s.scale_factor_percent = scale_pct;
        s.user_custom_scale_percent = custom_scale_pct;
        s.table_cell_preview_chars = preview_chars;
        s.table_zebra_stripe = display.table_zebra_stripe;
        s.issues_grouped_evidence = display.issues_grouped_evidence;
        s.use_system_dpi = display.use_system_dpi;
        s.editor_font_size_px = ed_px;
        s.editor_soft_wrap = editor.soft_wrap;
        s.pretty_print_by_default = editor.pretty_print_by_default;
        s.auto_expand_inspector = editor.auto_expand_inspector;
        s.highlight_search_case_sensitive_default = editor.highlight_search_case_sensitive_default;
        s.search_regex_default = editor.search_regex_default;
        s.search_autoscroll_default = editor.search_autoscroll_default;
        s.html_render_external_requests = editor.html_render_external_requests;
        s.editor_charset_mode = charset_mode;
        s.editor_specific_charset = specific_charset;
        s.hidden_request_headers = editor.hidden_request_headers.clone();
        s.hidden_response_headers = editor.hidden_response_headers.clone();
        s.hide_request_headers_by_default = editor.hide_request_headers_by_default;
        s.hide_response_headers_by_default = editor.hide_response_headers_by_default;
        s.url_encode_as_you_type = editor.url_encode_as_you_type;
    });

    let theme_mode = if dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    Theme::change(theme_mode, None, cx);
    let font_size = ui_font_size();
    let mono_font_size = editor_font_size();
    let bg = background();
    let fg = foreground();
    let bd = border();
    let ctrl_bd = control_border();
    let sel = selected();
    let surf = surface();
    let acc = accent();
    let zebra = display.table_zebra_stripe;
    let acc_hover: Hsla = rgb(if dark { 0xe56e3d } else { 0xc45122 }).into();
    let acc_active: Hsla = rgb(if dark { 0xc95526 } else { 0xad4318 }).into();
    let danger: Hsla = rgb(if dark { 0xe5534b } else { 0xd92d20 }).into();
    let danger_hover: Hsla = rgb(if dark { 0xf47067 } else { 0xb42318 }).into();
    let danger_active: Hsla = rgb(if dark { 0xc93c37 } else { 0x912018 }).into();
    Theme::update(cx, |theme| {
        theme.font_family = ui_font_family().into();
        #[cfg(target_os = "macos")]
        {
            theme.mono_font_family = "Menlo".into();
        }
        #[cfg(target_os = "windows")]
        {
            theme.mono_font_family = "Consolas".into();
        }
        #[cfg(target_os = "linux")]
        {
            theme.mono_font_family = "DejaVu Sans Mono".into();
        }
        theme.font_size = font_size;
        theme.mono_font_size = mono_font_size;
        theme.radius = px(3.);
        theme.radius_lg = px(4.);
        theme.shadow = false;
        theme.colors.background = bg;
        theme.colors.foreground = fg;
        theme.colors.muted_foreground = muted_foreground();
        theme.colors.border = bd;
        theme.colors.primary = acc;
        theme.colors.primary_hover = acc_hover;
        theme.colors.primary_active = acc_active;
        theme.colors.primary_foreground = rgb(0xffffff).into();
        theme.colors.button_primary = acc;
        theme.colors.button_primary_hover = acc_hover;
        theme.colors.button_primary_active = acc_active;
        theme.colors.button_primary_foreground = rgb(0xffffff).into();
        theme.colors.danger = danger;
        theme.colors.danger_hover = danger_hover;
        theme.colors.danger_active = danger_active;
        theme.colors.danger_foreground = rgb(0xffffff).into();
        theme.colors.button_danger = danger;
        theme.colors.button_danger_hover = danger_hover;
        theme.colors.button_danger_active = danger_active;
        theme.colors.button_danger_foreground = rgb(0xffffff).into();
        theme.colors.button = rgb(if dark { 0x33363a } else { 0xffffff }).into();
        theme.colors.button_foreground = fg;
        theme.colors.button_hover = rgb(if dark { 0x3e4247 } else { 0xeeeeee }).into();
        theme.colors.button_active = rgb(if dark { 0x4b5056 } else { 0xdfdfdf }).into();
        theme.colors.table_head = rgb(if dark { 0x25272b } else { 0xeaeaea }).into();
        theme.colors.table_head_foreground = rgb(if dark { 0xd4d4d4 } else { 0x141414 }).into();
        theme.colors.ring = acc;
        theme.colors.selection = rgb(if dark { 0x3b4d6b } else { 0xcadaf0 }).into();
        theme.colors.table = surf;
        theme.colors.table_even = if zebra {
            rgb(if dark { 0x25272b } else { 0xfafafa }).into()
        } else {
            surf
        };
        theme.colors.table_row_border = bd;
        theme.colors.table_active = sel;
        theme.colors.table_active_border = bd;
        theme.colors.table_hover = rgb(if dark { 0x26334a } else { 0xeeeeee }).into();
        theme.colors.scrollbar = rgb(if dark { 0x2b2d30 } else { 0xf7f7f7 }).into();
        theme.colors.scrollbar_thumb = rgb(if dark { 0x5c626b } else { 0xc8c8c8 }).into();
        theme.colors.scrollbar_thumb_hover = rgb(if dark { 0x707782 } else { 0xb0b0b0 }).into();
        theme.colors.input = ctrl_bd;
        theme.colors.switch = rgb(if dark { 0x4e535b } else { 0xb0afaf }).into();
        let highlight = std::sync::Arc::make_mut(&mut theme.highlight_theme);
        highlight.style.editor_background = Some(surf);
        highlight.style.editor_gutter_background = Some(if dark { bg } else { surf });
        highlight.style.editor_line_number = Some(muted_foreground());
        highlight.style.editor_active_line_number = Some(muted_foreground());
        highlight.style.editor_active_line = if dark { None } else { Some(rgb(0xf0f0f0).into()) };
    });
}

struct CustomHotkeyActionSpec {
    action_name: &'static str,
    default_key: &'static str,
    contexts: &'static [Option<&'static str>],
    make_action: fn() -> Box<dyn gpui_kit::Action>,
}

fn resolve_custom_hotkey_action(action_id: &str) -> Option<CustomHotkeyActionSpec> {
    const WORKSPACE: &[Option<&str>] = &[Some("MioWorkspace")];
    const WORKSPACE_AND_SETTINGS: &[Option<&str>] = &[Some("MioWorkspace"), Some("MioSettings")];
    const GLOBAL: &[Option<&str>] = &[None];

    match action_id {
        "cancel_request" | "CancelRequest" => Some(CustomHotkeyActionSpec {
            action_name: "mio::CancelRequest",
            default_key: "cmd-period",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::CancelRequest),
        }),
        "send_to_repeater" | "OpenInRepeater" | "SendRepeater" => Some(CustomHotkeyActionSpec {
            action_name: "mio::OpenInRepeater",
            default_key: "cmd-r",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::OpenInRepeater),
        }),
        "send_to_intruder" | "OpenInIntruder" | "SendIntruder" => Some(CustomHotkeyActionSpec {
            action_name: "mio::OpenInIntruder",
            default_key: "cmd-i",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::OpenInIntruder),
        }),
        "send_to_comparer" | "SendToComparerRequest" => Some(CustomHotkeyActionSpec {
            action_name: "mio::SendToComparerRequest",
            default_key: "",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::SendToComparerRequest),
        }),
        "send_to_decoder" | "SendToDecoder" => Some(CustomHotkeyActionSpec {
            action_name: "mio::SendToDecoder",
            default_key: "",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::SendToDecoder),
        }),
        "send_to_organiser" | "SendToOrganiser" => Some(CustomHotkeyActionSpec {
            action_name: "mio::SendToOrganiser",
            default_key: "cmd-o",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::SendToOrganiser),
        }),
        "url_encode" | "UrlEncodeKey" => Some(CustomHotkeyActionSpec {
            action_name: "mio::ConvertKind",
            default_key: "cmd-u",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::editing::ConvertKind::UrlEncodeKey),
        }),
        "url_decode" | "UrlDecode" => Some(CustomHotkeyActionSpec {
            action_name: "mio::ConvertKind",
            default_key: "cmd-shift-u",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::editing::ConvertKind::UrlDecode),
        }),
        "base64_encode" | "Base64Encode" => Some(CustomHotkeyActionSpec {
            action_name: "mio::ConvertKind",
            default_key: "cmd-b",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::editing::ConvertKind::Base64Encode),
        }),
        "base64_decode" | "Base64Decode" => Some(CustomHotkeyActionSpec {
            action_name: "mio::ConvertKind",
            default_key: "cmd-shift-b",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::editing::ConvertKind::Base64Decode),
        }),
        "base64_url_encode" | "Base64UrlSafeEncode" => Some(CustomHotkeyActionSpec {
            action_name: "mio::ConvertKind",
            default_key: "",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::editing::ConvertKind::Base64UrlSafeEncode),
        }),
        "base64_url_decode" | "Base64UrlSafeDecode" => Some(CustomHotkeyActionSpec {
            action_name: "mio::ConvertKind",
            default_key: "",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::editing::ConvertKind::Base64UrlSafeDecode),
        }),
        "html_encode" | "HtmlEncodeKey" => Some(CustomHotkeyActionSpec {
            action_name: "mio::ConvertKind",
            default_key: "",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::editing::ConvertKind::HtmlEncodeKey),
        }),
        "html_decode" | "HtmlDecode" => Some(CustomHotkeyActionSpec {
            action_name: "mio::ConvertKind",
            default_key: "",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::editing::ConvertKind::HtmlDecode),
        }),
        "change_request_method" | "ChangeRequestMethod" => Some(CustomHotkeyActionSpec {
            action_name: "mio::ChangeRequestMethod",
            default_key: "cmd-alt-m",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::ChangeRequestMethod),
        }),
        "find_in_message" | "Search" => Some(CustomHotkeyActionSpec {
            action_name: "input::Search",
            default_key: "cmd-f",
            contexts: WORKSPACE,
            make_action: || Box::new(gpui_kit::component::input::Search),
        }),
        "add_notes" | "AddHttpNotes" => Some(CustomHotkeyActionSpec {
            action_name: "mio::AddHttpNotes",
            default_key: "",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::AddHttpNotes),
        }),
        "clear_highlight" | "ClearHighlight" => Some(CustomHotkeyActionSpec {
            action_name: "mio::ClearHighlight",
            default_key: "",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::ClearHighlight),
        }),
        "add_to_scope" | "AddToScope" => Some(CustomHotkeyActionSpec {
            action_name: "mio::AddToScope",
            default_key: "",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::AddToScope),
        }),
        "add_to_site_map" | "AddToSiteMap" => Some(CustomHotkeyActionSpec {
            action_name: "mio::AddToSiteMap",
            default_key: "",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::AddToSiteMap),
        }),
        "add_payload_position" | "AutoMarkIntruderPositions" => Some(CustomHotkeyActionSpec {
            action_name: "mio::AutoMarkIntruderPositions",
            default_key: "",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::AutoMarkIntruderPositions),
        }),
        "clear_payload_positions" | "ClearIntruderPositions" => Some(CustomHotkeyActionSpec {
            action_name: "mio::ClearIntruderPositions",
            default_key: "",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::ClearIntruderPositions),
        }),
        "clear_history" | "ClearLiveProxyHistory" => Some(CustomHotkeyActionSpec {
            action_name: "mio::ClearLiveProxyHistory",
            default_key: "",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::ClearLiveProxyHistory),
        }),
        "save_project" | "SaveProject" => Some(CustomHotkeyActionSpec {
            action_name: "mio::SaveProject",
            default_key: "cmd-s",
            contexts: WORKSPACE_AND_SETTINGS,
            make_action: || Box::new(crate::SaveProject),
        }),
        "save_project_as" | "SaveProjectAs" => Some(CustomHotkeyActionSpec {
            action_name: "mio::SaveProjectAs",
            default_key: "cmd-shift-s",
            contexts: WORKSPACE_AND_SETTINGS,
            make_action: || Box::new(crate::SaveProjectAs),
        }),
        "open_settings" | "OpenSettings" => Some(CustomHotkeyActionSpec {
            action_name: "mio::OpenSettings",
            default_key: "cmd-,",
            contexts: GLOBAL,
            make_action: || Box::new(crate::OpenSettings),
        }),
        "close_tab" | "CloseTab" => Some(CustomHotkeyActionSpec {
            action_name: "mio::CloseTab",
            default_key: "cmd-w",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::CloseTab),
        }),
        "close_other_tabs" | "CloseOtherTabs" => Some(CustomHotkeyActionSpec {
            action_name: "mio::CloseOtherTabs",
            default_key: "",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::CloseOtherTabs),
        }),
        "toggle_tab_orientation" | "ToggleTabOrientation" => Some(CustomHotkeyActionSpec {
            action_name: "mio::ToggleTabOrientation",
            default_key: "",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::ToggleTabOrientation),
        }),
        "toggle_proxy_intercept" | "ToggleProxyIntercept" | "ToggleIntercept" => {
            Some(CustomHotkeyActionSpec {
                action_name: "mio::ToggleProxyIntercept",
                default_key: "",
                contexts: WORKSPACE,
                make_action: || Box::new(crate::ToggleProxyIntercept),
            })
        }
        "open_command_palette" | "OpenCommandPalette" => Some(CustomHotkeyActionSpec {
            action_name: "mio::OpenCommandPalette",
            default_key: "cmd-k",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::OpenCommandPalette),
        }),
        "open_global_search" | "OpenGlobalSearch" => Some(CustomHotkeyActionSpec {
            action_name: "mio::OpenGlobalSearch",
            default_key: "cmd-shift-f",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::OpenGlobalSearch),
        }),
        "new_window" | "NewWindow" => Some(CustomHotkeyActionSpec {
            action_name: "mio::NewWindow",
            default_key: "cmd-shift-n",
            contexts: GLOBAL,
            make_action: || Box::new(crate::NewWindow),
        }),
        _ => None,
    }
}

fn platform_default_key(macos_default: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        macos_default.to_owned()
    }
    #[cfg(not(target_os = "macos"))]
    {
        macos_default.replace("cmd-", "ctrl-")
    }
}

fn push_unbind_for_contexts(
    bindings: &mut Vec<KeyBinding>,
    key: &str,
    action_name: &'static str,
    contexts: &[Option<&'static str>],
    cx: &App,
) {
    if key.is_empty() {
        return;
    }
    for &context in contexts {
        let predicate = context.and_then(|ctx| {
            gpui_kit::KeyBindingContextPredicate::parse(ctx)
                .ok()
                .map(std::rc::Rc::new)
        });
        if let Ok(unbind) = KeyBinding::load(
            key,
            Box::new(gpui_kit::Unbind(action_name.into())),
            predicate,
            false,
            None,
            cx,
        ) {
            bindings.push(unbind);
        }
    }
}

fn push_bind_for_contexts(
    bindings: &mut Vec<KeyBinding>,
    key: &str,
    make_action: fn() -> Box<dyn gpui_kit::Action>,
    contexts: &[Option<&'static str>],
    cx: &App,
) {
    if key.is_empty() {
        return;
    }
    for &context in contexts {
        let predicate = context.and_then(|ctx| {
            gpui_kit::KeyBindingContextPredicate::parse(ctx)
                .ok()
                .map(std::rc::Rc::new)
        });
        if let Ok(binding) = KeyBinding::load(key, make_action(), predicate, false, None, cx) {
            bindings.push(binding);
        }
    }
}

pub(crate) fn apply_user_misc(misc: &UserMiscSettings, cx: &mut App) {
    let custom_send = misc
        .custom_hotkeys
        .get("send_request")
        .or_else(|| misc.custom_hotkeys.get("SendRequest"))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let hotkey = if let Some(custom) = custom_send {
        custom
    } else if misc.hotkey_send_request.trim().is_empty() {
        "cmd-enter"
    } else {
        misc.hotkey_send_request.trim()
    };
    let previous = hotkey_send_request();
    let previous_custom = custom_hotkeys();
    update_state(cx, |s| {
        s.confirm_on_close_tab_with_unsaved_draft = misc.confirm_on_close_tab_with_unsaved_draft;
        s.redact_sensitive_headers_in_copy_curl = misc.redact_sensitive_headers_in_copy_curl;
        s.hotkey_send_request = hotkey.to_owned();
        s.custom_hotkeys = misc.custom_hotkeys.clone();
    });

    let mut bindings = Vec::new();
    for old in [
        previous.as_str(),
        "cmd-enter",
        "ctrl-enter",
        "cmd-shift-enter",
    ] {
        if old == hotkey {
            continue;
        }
        #[cfg(not(target_os = "macos"))]
        if hotkey == "cmd-enter" && old == "ctrl-enter" {
            continue;
        }
        for context in ["MioWorkspace", "MioRepeater > Input"] {
            let predicate = gpui_kit::KeyBindingContextPredicate::parse(context)
                .ok()
                .map(std::rc::Rc::new);
            if let Ok(unbind) = KeyBinding::load(
                old,
                Box::new(gpui_kit::Unbind("mio::SendRequest".into())),
                predicate,
                false,
                None,
                cx,
            ) {
                bindings.push(unbind);
            }
        }
    }
    for context in ["MioWorkspace", "MioRepeater > Input"] {
        let predicate = gpui_kit::KeyBindingContextPredicate::parse(context)
            .ok()
            .map(std::rc::Rc::new);
        if let Ok(binding) = KeyBinding::load(
            hotkey,
            Box::new(crate::SendRequest),
            predicate,
            false,
            None,
            cx,
        ) {
            bindings.push(binding);
        }
        #[cfg(not(target_os = "macos"))]
        if hotkey == "cmd-enter" {
            let predicate = gpui_kit::KeyBindingContextPredicate::parse(context)
                .ok()
                .map(std::rc::Rc::new);
            if let Ok(binding) = KeyBinding::load(
                "ctrl-enter",
                Box::new(crate::SendRequest),
                predicate,
                false,
                None,
                cx,
            ) {
                bindings.push(binding);
            }
        }
    }

    // Dynamically unbind removed custom hotkeys and restore built-in defaults.
    for (action_id, prev_key) in &previous_custom {
        if misc.custom_hotkeys.contains_key(action_id) {
            continue;
        }
        let Some(spec) = resolve_custom_hotkey_action(action_id) else {
            continue;
        };
        let default_key = platform_default_key(spec.default_key);
        let prev_trimmed = prev_key.trim();
        if !prev_trimmed.is_empty() && prev_trimmed != default_key {
            push_unbind_for_contexts(&mut bindings, prev_trimmed, spec.action_name, spec.contexts, cx);
        }
        if !default_key.is_empty() {
            push_bind_for_contexts(&mut bindings, &default_key, spec.make_action, spec.contexts, cx);
        }
    }

    // Dynamically bind all active custom hotkeys.
    for (action_id, new_key_raw) in &misc.custom_hotkeys {
        let Some(spec) = resolve_custom_hotkey_action(action_id) else {
            continue;
        };
        let new_key = new_key_raw.trim();
        let default_key = platform_default_key(spec.default_key);
        if !default_key.is_empty() && default_key != new_key {
            push_unbind_for_contexts(&mut bindings, &default_key, spec.action_name, spec.contexts, cx);
        }
        if let Some(prev_key) = previous_custom.get(action_id) {
            let prev_trimmed = prev_key.trim();
            if !prev_trimmed.is_empty() && prev_trimmed != new_key && prev_trimmed != default_key {
                push_unbind_for_contexts(
                    &mut bindings,
                    prev_trimmed,
                    spec.action_name,
                    spec.contexts,
                    cx,
                );
            }
        }
        if !new_key.is_empty() {
            push_bind_for_contexts(&mut bindings, new_key, spec.make_action, spec.contexts, cx);
        }
    }

    if !bindings.is_empty() {
        cx.bind_keys(bindings);
    }
}

#[cfg(test)]
mod ui_preferences_state_tests {
    use super::*;

    #[gpui_kit::test]
    fn ui_preferences_global_stays_synced_with_theme_and_editing_accessors(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            init(cx);
            assert_eq!(
                cx.global::<UiPreferencesState>(),
                &UiPreferencesState::default()
            );
            assert!(!is_dark());
            assert!(!url_encode_as_you_type());

            let display = UserDisplaySettings {
                theme: UserThemeMode::Dark,
                ui_font_size_px: 15,
                scale_factor_percent: 125,
                max_table_cell_preview_chars: 512,
                table_zebra_stripe: false,
                issues_grouped_evidence: false,
                ..UserDisplaySettings::default()
            };
            let editor = UserMessageEditorSettings {
                font_size_px: 16,
                soft_wrap: false,
                pretty_print_by_default: false,
                auto_expand_inspector: true,
                url_encode_as_you_type: true,
                hidden_request_headers: Some(vec!["X-Test".into()]),
                hide_request_headers_by_default: true,
                ..UserMessageEditorSettings::default()
            };
            apply_user_display(&display, &editor, cx);

            let misc = UserMiscSettings {
                confirm_on_close_tab_with_unsaved_draft: false,
                redact_sensitive_headers_in_copy_curl: true,
                hotkey_send_request: "ctrl-enter".into(),
                ..UserMiscSettings::default()
            };
            apply_user_misc(&misc, cx);

            let global = cx.global::<UiPreferencesState>();
            assert!(global.dark_mode);
            assert_eq!(global.ui_font_size_px, 15);
            assert_eq!(global.scale_factor_percent, 125);
            assert_eq!(global.table_cell_preview_chars, 512);
            assert!(!global.table_zebra_stripe);
            assert!(!global.issues_grouped_evidence);
            assert_eq!(global.editor_font_size_px, 16);
            assert!(!global.editor_soft_wrap);
            assert!(!global.pretty_print_by_default);
            assert!(global.auto_expand_inspector);
            assert!(global.url_encode_as_you_type);
            assert_eq!(global.hidden_request_headers.as_deref(), Some(&["X-Test".to_string()][..]));
            assert!(global.hide_request_headers_by_default);
            assert!(!global.confirm_on_close_tab_with_unsaved_draft);
            assert!(global.redact_sensitive_headers_in_copy_curl);
            assert_eq!(global.hotkey_send_request, "ctrl-enter");

            assert!(is_dark());
            assert_eq!(ui_font_size_px(), 15);
            assert_eq!(scale_factor_percent(), 125);
            assert_eq!(table_cell_preview_chars(), 512);
            assert!(!table_zebra_stripe());
            assert!(!issues_grouped_evidence());
            assert_eq!(editor_font_size_px(), 16);
            assert!(!editor_soft_wrap());
            assert!(!pretty_print_by_default());
            assert!(auto_expand_inspector());
            assert!(url_encode_as_you_type());
            assert!(hide_headers_by_default(false));
            assert!(!confirm_on_close_tab_with_unsaved_draft());
            assert!(redact_sensitive_headers_in_copy_curl());
            assert_eq!(hotkey_send_request(), "ctrl-enter");

            set_url_encode_as_you_type(false, cx);
            assert!(!cx.global::<UiPreferencesState>().url_encode_as_you_type);
            assert!(!url_encode_as_you_type());

            init(cx);
            assert_eq!(
                cx.global::<UiPreferencesState>(),
                &UiPreferencesState::default()
            );
        });
    }

    #[test]
    fn custom_hotkey_specs_match_action_names() {
        for action_id in [
            "find_in_message",
            "clear_highlight",
            "close_other_tabs",
            "send_to_organiser",
            "change_request_method",
        ] {
            let spec = resolve_custom_hotkey_action(action_id)
                .unwrap_or_else(|| panic!("missing spec for {action_id}"));
            assert_eq!(spec.action_name, (spec.make_action)().name());
        }
    }
}
