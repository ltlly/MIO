use std::collections::BTreeMap;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Hsla, KeyBinding, Pixels, px, rgb};
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

#[cfg(not(test))]
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU8, AtomicU16, Ordering},
};

#[cfg(not(test))]
static DARK_MODE: AtomicBool = AtomicBool::new(false);
#[cfg(not(test))]
static UI_FONT_SIZE_PX: AtomicU8 = AtomicU8::new(13);
#[cfg(not(test))]
static SCALE_FACTOR_PERCENT: AtomicU16 = AtomicU16::new(100);
#[cfg(not(test))]
static SYSTEM_DPI_SCALE_PERCENT: AtomicU16 = AtomicU16::new(100);
#[cfg(not(test))]
static USER_CUSTOM_SCALE_PERCENT: AtomicU16 = AtomicU16::new(100);
#[cfg(not(test))]
static TABLE_CELL_PREVIEW_CHARS: AtomicU16 = AtomicU16::new(256);
#[cfg(not(test))]
static TABLE_ZEBRA_STRIPE: AtomicBool = AtomicBool::new(true);
#[cfg(not(test))]
static ISSUES_GROUPED_EVIDENCE: AtomicBool = AtomicBool::new(true);
#[cfg(not(test))]
static USE_SYSTEM_DPI: AtomicBool = AtomicBool::new(false);
#[cfg(not(test))]
static EDITOR_FONT_SIZE_PX: AtomicU8 = AtomicU8::new(13);
#[cfg(not(test))]
static EDITOR_SOFT_WRAP: AtomicBool = AtomicBool::new(true);
#[cfg(not(test))]
static PRETTY_PRINT_BY_DEFAULT: AtomicBool = AtomicBool::new(true);
#[cfg(not(test))]
static AUTO_EXPAND_INSPECTOR: AtomicBool = AtomicBool::new(false);
#[cfg(not(test))]
static HIGHLIGHT_SEARCH_CASE_SENSITIVE_DEFAULT: AtomicBool = AtomicBool::new(false);
#[cfg(not(test))]
static SEARCH_REGEX_DEFAULT: AtomicBool = AtomicBool::new(false);
#[cfg(not(test))]
static SEARCH_AUTOSCROLL_DEFAULT: AtomicBool = AtomicBool::new(false);
#[cfg(not(test))]
static HTML_RENDER_EXTERNAL_REQUESTS: AtomicBool = AtomicBool::new(true);
#[cfg(not(test))]
static EDITOR_CHARSET_MODE: AtomicU8 = AtomicU8::new(0);
#[cfg(not(test))]
static EDITOR_SPECIFIC_CHARSET: Mutex<Option<String>> = Mutex::new(None);
#[cfg(not(test))]
static CONFIRM_ON_CLOSE_TAB_WITH_UNSAVED_DRAFT: AtomicBool = AtomicBool::new(true);
#[cfg(not(test))]
static REDACT_SENSITIVE_HEADERS_IN_COPY_CURL: AtomicBool = AtomicBool::new(false);
#[cfg(not(test))]
static HOTKEY_SEND_REQUEST: Mutex<Option<String>> = Mutex::new(None);
#[cfg(not(test))]
static CUSTOM_HOTKEYS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
#[cfg(not(test))]
static HIDDEN_REQUEST_HEADERS: Mutex<Option<Vec<String>>> = Mutex::new(None);
#[cfg(not(test))]
static HIDDEN_RESPONSE_HEADERS: Mutex<Option<Vec<String>>> = Mutex::new(None);
#[cfg(not(test))]
static HIDE_REQUEST_HEADERS_BY_DEFAULT: AtomicBool = AtomicBool::new(false);
#[cfg(not(test))]
static HIDE_RESPONSE_HEADERS_BY_DEFAULT: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
use std::cell::{Cell, RefCell};

#[cfg(test)]
thread_local! {
    static TEST_DARK_MODE: Cell<bool> = const { Cell::new(false) };
    static TEST_UI_FONT_SIZE_PX: Cell<u8> = const { Cell::new(13) };
    static TEST_SCALE_FACTOR_PERCENT: Cell<u16> = const { Cell::new(100) };
    static TEST_SYSTEM_DPI_SCALE_PERCENT: Cell<u16> = const { Cell::new(100) };
    static TEST_USER_CUSTOM_SCALE_PERCENT: Cell<u16> = const { Cell::new(100) };
    static TEST_TABLE_CELL_PREVIEW_CHARS: Cell<u16> = const { Cell::new(256) };
    static TEST_TABLE_ZEBRA_STRIPE: Cell<bool> = const { Cell::new(true) };
    static TEST_ISSUES_GROUPED_EVIDENCE: Cell<bool> = const { Cell::new(true) };
    static TEST_USE_SYSTEM_DPI: Cell<bool> = const { Cell::new(false) };
    static TEST_EDITOR_FONT_SIZE_PX: Cell<u8> = const { Cell::new(13) };
    static TEST_EDITOR_SOFT_WRAP: Cell<bool> = const { Cell::new(true) };
    static TEST_PRETTY_PRINT_BY_DEFAULT: Cell<bool> = const { Cell::new(true) };
    static TEST_AUTO_EXPAND_INSPECTOR: Cell<bool> = const { Cell::new(false) };
    static TEST_HIGHLIGHT_SEARCH_CASE_SENSITIVE_DEFAULT: Cell<bool> = const { Cell::new(false) };
    static TEST_SEARCH_REGEX_DEFAULT: Cell<bool> = const { Cell::new(false) };
    static TEST_SEARCH_AUTOSCROLL_DEFAULT: Cell<bool> = const { Cell::new(false) };
    static TEST_HTML_RENDER_EXTERNAL_REQUESTS: Cell<bool> = const { Cell::new(true) };
    static TEST_EDITOR_CHARSET_MODE: Cell<u8> = const { Cell::new(0) };
    static TEST_EDITOR_SPECIFIC_CHARSET: RefCell<String> = RefCell::new("UTF-8".into());
    static TEST_CONFIRM_ON_CLOSE_TAB_WITH_UNSAVED_DRAFT: Cell<bool> = const { Cell::new(true) };
    static TEST_REDACT_SENSITIVE_HEADERS_IN_COPY_CURL: Cell<bool> = const { Cell::new(false) };
    static TEST_HOTKEY_SEND_REQUEST: RefCell<String> = RefCell::new("cmd-enter".into());
    static TEST_CUSTOM_HOTKEYS: RefCell<BTreeMap<String, String>> = const { RefCell::new(BTreeMap::new()) };
    static TEST_HIDDEN_REQUEST_HEADERS: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
    static TEST_HIDDEN_RESPONSE_HEADERS: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
    static TEST_HIDE_REQUEST_HEADERS_BY_DEFAULT: Cell<bool> = const { Cell::new(false) };
    static TEST_HIDE_RESPONSE_HEADERS_BY_DEFAULT: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn is_dark() -> bool {
    #[cfg(not(test))]
    {
        DARK_MODE.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_DARK_MODE.with(Cell::get)
    }
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
    #[cfg(not(test))]
    {
        UI_FONT_SIZE_PX.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_UI_FONT_SIZE_PX.with(Cell::get)
    }
}

pub(crate) fn scale_factor_percent() -> u16 {
    if use_system_dpi() {
        return system_dpi_scale_percent();
    }
    #[cfg(not(test))]
    {
        SCALE_FACTOR_PERCENT.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_SCALE_FACTOR_PERCENT.with(Cell::get)
    }
}

pub(crate) fn system_dpi_scale_percent() -> u16 {
    #[cfg(not(test))]
    {
        SYSTEM_DPI_SCALE_PERCENT.load(Ordering::Relaxed).clamp(80, 200)
    }
    #[cfg(test)]
    {
        TEST_SYSTEM_DPI_SCALE_PERCENT.with(Cell::get).clamp(80, 200)
    }
}

#[allow(dead_code)]
pub(crate) fn user_custom_scale_percent() -> u16 {
    #[cfg(not(test))]
    {
        USER_CUSTOM_SCALE_PERCENT.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_USER_CUSTOM_SCALE_PERCENT.with(Cell::get)
    }
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

pub(crate) fn sync_window_system_dpi(window_scale_factor: f32) {
    #[cfg(not(test))]
    {
        let pct = compute_system_dpi_scale_percent(window_scale_factor, cfg!(target_os = "macos"));
        SYSTEM_DPI_SCALE_PERCENT.store(pct, Ordering::Relaxed);
    }
    #[cfg(test)]
    {
        let _ = window_scale_factor;
    }
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
    #[cfg(not(test))]
    {
        usize::from(TABLE_CELL_PREVIEW_CHARS.load(Ordering::Relaxed))
    }
    #[cfg(test)]
    {
        usize::from(TEST_TABLE_CELL_PREVIEW_CHARS.with(Cell::get))
    }
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
    #[cfg(not(test))]
    {
        EDITOR_FONT_SIZE_PX.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_EDITOR_FONT_SIZE_PX.with(Cell::get)
    }
}

pub(crate) fn editor_font_size() -> Pixels {
    let scale = f32::from(scale_factor_percent()) / 100.0;
    px((f32::from(editor_font_size_px()) * scale * 10.0).round() / 10.0)
}

pub(crate) fn editor_soft_wrap() -> bool {
    #[cfg(not(test))]
    {
        EDITOR_SOFT_WRAP.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_EDITOR_SOFT_WRAP.with(Cell::get)
    }
}

pub(crate) fn pretty_print_by_default() -> bool {
    #[cfg(not(test))]
    {
        PRETTY_PRINT_BY_DEFAULT.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_PRETTY_PRINT_BY_DEFAULT.with(Cell::get)
    }
}

pub(crate) fn auto_expand_inspector() -> bool {
    #[cfg(not(test))]
    {
        AUTO_EXPAND_INSPECTOR.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_AUTO_EXPAND_INSPECTOR.with(Cell::get)
    }
}

pub(crate) fn highlight_search_case_sensitive_default() -> bool {
    #[cfg(not(test))]
    {
        HIGHLIGHT_SEARCH_CASE_SENSITIVE_DEFAULT.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_HIGHLIGHT_SEARCH_CASE_SENSITIVE_DEFAULT.with(Cell::get)
    }
}

pub(crate) fn table_zebra_stripe() -> bool {
    #[cfg(not(test))]
    {
        TABLE_ZEBRA_STRIPE.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_TABLE_ZEBRA_STRIPE.with(Cell::get)
    }
}

pub(crate) fn issues_grouped_evidence() -> bool {
    #[cfg(not(test))]
    {
        ISSUES_GROUPED_EVIDENCE.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_ISSUES_GROUPED_EVIDENCE.with(Cell::get)
    }
}

pub(crate) fn use_system_dpi() -> bool {
    #[cfg(not(test))]
    {
        USE_SYSTEM_DPI.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_USE_SYSTEM_DPI.with(Cell::get)
    }
}

#[allow(dead_code)]
pub(crate) fn html_render_external_requests() -> bool {
    #[cfg(not(test))]
    {
        HTML_RENDER_EXTERNAL_REQUESTS.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_HTML_RENDER_EXTERNAL_REQUESTS.with(Cell::get)
    }
}

pub(crate) fn message_editor_search_regex_default() -> bool {
    #[cfg(not(test))]
    {
        SEARCH_REGEX_DEFAULT.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_SEARCH_REGEX_DEFAULT.with(Cell::get)
    }
}

pub(crate) fn message_editor_search_autoscroll_default() -> bool {
    #[cfg(not(test))]
    {
        SEARCH_AUTOSCROLL_DEFAULT.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_SEARCH_AUTOSCROLL_DEFAULT.with(Cell::get)
    }
}

pub(crate) fn message_editor_charset() -> (u8, String) {
    #[cfg(not(test))]
    {
        let mode = EDITOR_CHARSET_MODE.load(Ordering::Relaxed).min(3);
        let specific = EDITOR_SPECIFIC_CHARSET
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_else(|| "UTF-8".into());
        (mode, specific)
    }
    #[cfg(test)]
    {
        let mode = TEST_EDITOR_CHARSET_MODE.with(Cell::get).min(3);
        let specific = TEST_EDITOR_SPECIFIC_CHARSET.with(|c| c.borrow().clone());
        (mode, specific)
    }
}

pub(crate) fn confirm_on_close_tab_with_unsaved_draft() -> bool {
    #[cfg(not(test))]
    {
        CONFIRM_ON_CLOSE_TAB_WITH_UNSAVED_DRAFT.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_CONFIRM_ON_CLOSE_TAB_WITH_UNSAVED_DRAFT.with(Cell::get)
    }
}

pub(crate) fn redact_sensitive_headers_in_copy_curl() -> bool {
    #[cfg(not(test))]
    {
        REDACT_SENSITIVE_HEADERS_IN_COPY_CURL.load(Ordering::Relaxed)
    }
    #[cfg(test)]
    {
        TEST_REDACT_SENSITIVE_HEADERS_IN_COPY_CURL.with(Cell::get)
    }
}

pub(crate) fn hotkey_send_request() -> String {
    #[cfg(not(test))]
    {
        HOTKEY_SEND_REQUEST
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_else(|| "cmd-enter".into())
    }
    #[cfg(test)]
    {
        TEST_HOTKEY_SEND_REQUEST.with(|c| c.borrow().clone())
    }
}

#[allow(dead_code)]
pub(crate) fn custom_hotkeys() -> BTreeMap<String, String> {
    #[cfg(not(test))]
    {
        CUSTOM_HOTKEYS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    #[cfg(test)]
    {
        TEST_CUSTOM_HOTKEYS.with(|c| c.borrow().clone())
    }
}

pub(crate) fn hidden_request_headers() -> Option<Vec<String>> {
    #[cfg(not(test))]
    {
        HIDDEN_REQUEST_HEADERS
            .lock()
            .expect("hidden request headers mutex must remain available")
            .clone()
    }
    #[cfg(test)]
    {
        TEST_HIDDEN_REQUEST_HEADERS.with(|c| c.borrow().clone())
    }
}

pub(crate) fn hide_headers_by_default(response: bool) -> bool {
    #[cfg(not(test))]
    {
        if response {
            HIDE_RESPONSE_HEADERS_BY_DEFAULT.load(Ordering::Relaxed)
        } else {
            HIDE_REQUEST_HEADERS_BY_DEFAULT.load(Ordering::Relaxed)
        }
    }
    #[cfg(test)]
    {
        if response {
            TEST_HIDE_RESPONSE_HEADERS_BY_DEFAULT.with(Cell::get)
        } else {
            TEST_HIDE_REQUEST_HEADERS_BY_DEFAULT.with(Cell::get)
        }
    }
}

pub(crate) fn set_hidden_request_headers(headers: Option<Vec<String>>) {
    #[cfg(not(test))]
    {
        *HIDDEN_REQUEST_HEADERS
            .lock()
            .expect("hidden request headers mutex must remain available") = headers;
    }
    #[cfg(test)]
    {
        TEST_HIDDEN_REQUEST_HEADERS.with(|c| *c.borrow_mut() = headers);
    }
}

pub(crate) fn hidden_response_headers() -> Option<Vec<String>> {
    #[cfg(not(test))]
    {
        HIDDEN_RESPONSE_HEADERS
            .lock()
            .expect("hidden response headers mutex must remain available")
            .clone()
    }
    #[cfg(test)]
    {
        TEST_HIDDEN_RESPONSE_HEADERS.with(|c| c.borrow().clone())
    }
}

pub(crate) fn set_hidden_response_headers(headers: Option<Vec<String>>) {
    #[cfg(not(test))]
    {
        *HIDDEN_RESPONSE_HEADERS
            .lock()
            .expect("hidden response headers mutex must remain available") = headers;
    }
    #[cfg(test)]
    {
        TEST_HIDDEN_RESPONSE_HEADERS.with(|c| *c.borrow_mut() = headers);
    }
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
    set_hidden_request_headers(editor.hidden_request_headers.clone());
    set_hidden_response_headers(editor.hidden_response_headers.clone());
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
    #[cfg(not(test))]
    {
        DARK_MODE.store(dark, Ordering::Relaxed);
        UI_FONT_SIZE_PX.store(ui_px, Ordering::Relaxed);
        SCALE_FACTOR_PERCENT.store(scale_pct, Ordering::Relaxed);
        USER_CUSTOM_SCALE_PERCENT.store(custom_scale_pct, Ordering::Relaxed);
        TABLE_CELL_PREVIEW_CHARS.store(preview_chars, Ordering::Relaxed);
        TABLE_ZEBRA_STRIPE.store(display.table_zebra_stripe, Ordering::Relaxed);
        ISSUES_GROUPED_EVIDENCE.store(display.issues_grouped_evidence, Ordering::Relaxed);
        USE_SYSTEM_DPI.store(display.use_system_dpi, Ordering::Relaxed);
        EDITOR_FONT_SIZE_PX.store(ed_px, Ordering::Relaxed);
        EDITOR_SOFT_WRAP.store(editor.soft_wrap, Ordering::Relaxed);
        PRETTY_PRINT_BY_DEFAULT.store(editor.pretty_print_by_default, Ordering::Relaxed);
        AUTO_EXPAND_INSPECTOR.store(editor.auto_expand_inspector, Ordering::Relaxed);
        HIGHLIGHT_SEARCH_CASE_SENSITIVE_DEFAULT.store(
            editor.highlight_search_case_sensitive_default,
            Ordering::Relaxed,
        );
        SEARCH_REGEX_DEFAULT.store(editor.search_regex_default, Ordering::Relaxed);
        SEARCH_AUTOSCROLL_DEFAULT.store(editor.search_autoscroll_default, Ordering::Relaxed);
        HTML_RENDER_EXTERNAL_REQUESTS
            .store(editor.html_render_external_requests, Ordering::Relaxed);
        EDITOR_CHARSET_MODE.store(charset_mode, Ordering::Relaxed);
        *EDITOR_SPECIFIC_CHARSET
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(specific_charset);
        HIDE_REQUEST_HEADERS_BY_DEFAULT.store(editor.hide_request_headers_by_default, Ordering::Relaxed);
        HIDE_RESPONSE_HEADERS_BY_DEFAULT.store(editor.hide_response_headers_by_default, Ordering::Relaxed);
    }
    #[cfg(test)]
    {
        TEST_DARK_MODE.with(|c| c.set(dark));
        TEST_UI_FONT_SIZE_PX.with(|c| c.set(ui_px));
        TEST_SCALE_FACTOR_PERCENT.with(|c| c.set(scale_pct));
        TEST_USER_CUSTOM_SCALE_PERCENT.with(|c| c.set(custom_scale_pct));
        TEST_TABLE_CELL_PREVIEW_CHARS.with(|c| c.set(preview_chars));
        TEST_TABLE_ZEBRA_STRIPE.with(|c| c.set(display.table_zebra_stripe));
        TEST_ISSUES_GROUPED_EVIDENCE.with(|c| c.set(display.issues_grouped_evidence));
        TEST_USE_SYSTEM_DPI.with(|c| c.set(display.use_system_dpi));
        TEST_EDITOR_FONT_SIZE_PX.with(|c| c.set(ed_px));
        TEST_EDITOR_SOFT_WRAP.with(|c| c.set(editor.soft_wrap));
        TEST_PRETTY_PRINT_BY_DEFAULT.with(|c| c.set(editor.pretty_print_by_default));
        TEST_AUTO_EXPAND_INSPECTOR.with(|c| c.set(editor.auto_expand_inspector));
        TEST_HIGHLIGHT_SEARCH_CASE_SENSITIVE_DEFAULT
            .with(|c| c.set(editor.highlight_search_case_sensitive_default));
        TEST_SEARCH_REGEX_DEFAULT.with(|c| c.set(editor.search_regex_default));
        TEST_SEARCH_AUTOSCROLL_DEFAULT.with(|c| c.set(editor.search_autoscroll_default));
        TEST_HTML_RENDER_EXTERNAL_REQUESTS.with(|c| c.set(editor.html_render_external_requests));
        TEST_EDITOR_CHARSET_MODE.with(|c| c.set(charset_mode));
        TEST_EDITOR_SPECIFIC_CHARSET.with(|c| *c.borrow_mut() = specific_charset);
        TEST_HIDE_REQUEST_HEADERS_BY_DEFAULT.with(|c| c.set(editor.hide_request_headers_by_default));
        TEST_HIDE_RESPONSE_HEADERS_BY_DEFAULT.with(|c| c.set(editor.hide_response_headers_by_default));
    }

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
        "add_notes" | "AddHttpNotes" => Some(CustomHotkeyActionSpec {
            action_name: "mio::AddHttpNotes",
            default_key: "",
            contexts: WORKSPACE,
            make_action: || Box::new(crate::AddHttpNotes),
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
    #[cfg(not(test))]
    {
        CONFIRM_ON_CLOSE_TAB_WITH_UNSAVED_DRAFT.store(
            misc.confirm_on_close_tab_with_unsaved_draft,
            Ordering::Relaxed,
        );
        REDACT_SENSITIVE_HEADERS_IN_COPY_CURL.store(
            misc.redact_sensitive_headers_in_copy_curl,
            Ordering::Relaxed,
        );
        *HOTKEY_SEND_REQUEST
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(hotkey.to_owned());
        *CUSTOM_HOTKEYS.lock().unwrap_or_else(|e| e.into_inner()) = misc.custom_hotkeys.clone();
    }
    #[cfg(test)]
    {
        TEST_CONFIRM_ON_CLOSE_TAB_WITH_UNSAVED_DRAFT
            .with(|c| c.set(misc.confirm_on_close_tab_with_unsaved_draft));
        TEST_REDACT_SENSITIVE_HEADERS_IN_COPY_CURL
            .with(|c| c.set(misc.redact_sensitive_headers_in_copy_curl));
        TEST_HOTKEY_SEND_REQUEST.with(|c| *c.borrow_mut() = hotkey.to_owned());
        TEST_CUSTOM_HOTKEYS.with(|c| *c.borrow_mut() = misc.custom_hotkeys.clone());
    }

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
