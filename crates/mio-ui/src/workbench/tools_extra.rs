use std::{cell::Cell, rc::Rc};

use gpui_kit::WeakEntity;
use gpui_kit::assets::IconName;
use gpui_kit::base::{Checkbox, CheckboxIndicator, CheckboxState};
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::input::EditorState;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::table::{Column, DataTable, TableDelegate, TableEvent, TableState};
use mio_core::{
    FlowSummary, MatchAndReplaceRule, MatchReplaceTarget, TrafficSource, Uuid,
    WsMatchAndReplaceRule, WsReplaceDirection, extract_ws_match_and_replace_rules,
};

use super::*;

mod comparer;
mod decoder;
mod logger;
#[cfg(test)]
mod tests_decoder;
#[cfg(test)]
mod tests_extra;
mod transfer_bytes;

type ComparerSideBlockPair = (Option<std::ops::Range<usize>>, Option<std::ops::Range<usize>>);

pub(super) struct MatchReplacePane {
    pub(super) mr_editing: Option<Uuid>,
    pub(super) mr_selected: Option<Uuid>,
    pub(super) mr_editor_open: bool,
    pub(super) mr_dialog: Option<gpui_kit::AnyWindowHandle>,
    pub(super) mr_error: Option<String>,
    pub(super) mr_target_select: Entity<SelectState<Vec<&'static str>>>,
    pub(super) mr_table: Entity<TableState<WorkbenchMatchReplaceTable>>,
    pub(super) mr_target: MatchReplaceTarget,
    pub(super) mr_match: Entity<InputState>,
    pub(super) mr_replace: Entity<InputState>,
    pub(super) mr_comment: Entity<InputState>,
    pub(super) mr_regex: bool,
    pub(super) ws_mr_selected: Option<Uuid>,
    pub(super) ws_mr_editor_open: bool,
    pub(super) ws_mr_editing: Option<Uuid>,
    pub(super) ws_mr_direction: WsReplaceDirection,
    pub(super) ws_mr_direction_select: Entity<SelectState<Vec<&'static str>>>,
    pub(super) ws_mr_table: Entity<TableState<WorkbenchWsMatchReplaceTable>>,
    pub(super) ws_mr_match: Entity<InputState>,
    pub(super) ws_mr_replace: Entity<InputState>,
    pub(super) ws_mr_comment: Entity<InputState>,
    pub(super) ws_mr_regex: bool,
    pub(super) ws_mr_error: Option<String>,
    pub(super) decoder_input: Entity<EditorState>,
    pub(super) decoder_output: Entity<EditorState>,
    pub(super) decoder_input_bytes: Option<(u64, Vec<u8>)>,
    pub(super) decoder_output_bytes: Option<Vec<u8>>,
    pub(super) decoder_input_hex_view: bool,
    pub(super) decoder_output_hex_view: bool,
    pub(super) decoder_input_revision: u64,
    pub(super) decoder_chain_start: usize,
    pub(super) decoder_chain_continuing: bool,
    pub(super) decoder_mode: SharedString,
    pub(super) steps: Vec<DecoderStep>,
    pub(super) comparer_items: Vec<ComparerItem>,
    pub(super) left_index: usize,
    pub(super) right_index: usize,
    pub(super) comparer_selected: usize,
    pub(super) byte_mode: bool,
    pub(super) comparer_side_by_side: bool,
    pub(super) comparer_sync_views: bool,
    pub(super) comparer_diff_cursor: usize,
    pub(super) comparer_unified_blocks: Vec<std::ops::Range<usize>>,
    pub(super) comparer_side_blocks: Vec<ComparerSideBlockPair>,
    pub(super) comparer_view: Entity<EditorState>,
    pub(super) comparer_left_view: Entity<EditorState>,
    pub(super) comparer_right_view: Entity<EditorState>,
    pub(super) capture_enabled: bool,
    pub(super) logger_max_entries: usize,
    pub(super) logger_entries: Vec<FlowSummary>,
    pub(super) logger_source_filter: Option<TrafficSource>,
    pub(super) logger_sort: LoggerSort,
    pub(super) logger_filters: logger::LoggerFilters,
    pub(super) logger_search: Entity<InputState>,
    pub(super) logger_query_revision: u64,
    pub(super) logger_query_cancel: Option<CancellationToken>,
    pub(super) logger_selected: Option<FlowId>,
    pub(super) logger_pending_read: Option<(ProjectKey, u64, FlowId)>,
    pub(super) logger_loaded_detail: Option<(FlowRecord, OriginalMessages)>,
    pub(super) logger_request: MessageEditor,
    pub(super) logger_response: MessageEditor,
    plain_selection_seqs: [Rc<Cell<u64>>; 5],
    _selection_subscriptions: [Subscription; 5],
}

pub(super) type ExtraToolsState = MatchReplacePane;
pub(super) type ExtraTools = ExtraToolsState;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::workbench) struct DecoderStep {
    pub(super) op: Option<decoder::DecoderOp>,
    pub(super) op_label: String,
    pub(super) output_preview: String,
    pub(super) full_text: String,
    pub(super) raw_bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workbench) enum LoggerSortColumn {
    #[default]
    Number,
    Source,
    Method,
    Url,
    Status,
    Length,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) struct LoggerSort {
    pub(in crate::workbench) column: LoggerSortColumn,
    pub(in crate::workbench) ascending: bool,
}

impl Default for LoggerSort {
    fn default() -> Self {
        Self {
            column: LoggerSortColumn::Number,
            ascending: true,
        }
    }
}

pub(in crate::workbench) struct ComparerItem {
    pub(super) id: usize,
    pub(super) label: String,
    pub(super) bytes: Vec<u8>,
}

pub(in crate::workbench) const MR_TARGET_LABELS: [&str; 7] = [
    "Request header",
    "Request body",
    "Response header",
    "Response body",
    "Request param name",
    "Request param value",
    "Request first line",
];

fn mr_target_index(target: MatchReplaceTarget) -> usize {
    match target {
        MatchReplaceTarget::RequestHeader => 0,
        MatchReplaceTarget::RequestBody => 1,
        MatchReplaceTarget::ResponseHeader => 2,
        MatchReplaceTarget::ResponseBody => 3,
        MatchReplaceTarget::RequestParamName => 4,
        MatchReplaceTarget::RequestParamValue => 5,
        MatchReplaceTarget::RequestFirstLine => 6,
        MatchReplaceTarget::RequestScript => 0,
        MatchReplaceTarget::ResponseScript => 1,
    }
}

pub(in crate::workbench) fn mr_target_from_label(label: &str) -> MatchReplaceTarget {
    match label {
        "Request header" => MatchReplaceTarget::RequestHeader,
        "Request body" => MatchReplaceTarget::RequestBody,
        "Response header" => MatchReplaceTarget::ResponseHeader,
        "Response body" => MatchReplaceTarget::ResponseBody,
        "Request param name" => MatchReplaceTarget::RequestParamName,
        "Request param value" => MatchReplaceTarget::RequestParamValue,
        "Request first line" => MatchReplaceTarget::RequestFirstLine,
        "Request script" => MatchReplaceTarget::RequestScript,
        "Response script" => MatchReplaceTarget::ResponseScript,
        _ => unreachable!("validated HTTP rule type"),
    }
}

pub(in crate::workbench) fn mr_rule_cell(rule: &MatchAndReplaceRule, column: usize) -> String {
    match column {
        0 => if rule.enabled { "Yes" } else { "No" }.into(),
        1 => rule.item_type.display_name().into(),
        2 => if rule.item_type.is_script() { rule.script_name.clone() } else { String::new() },
        3 => if rule.item_type.is_script() { String::new() } else { rule.match_pattern.clone() },
        4 => if rule.item_type.is_script() {
            rule.filter_script.lines().next().unwrap_or_default().to_owned()
        } else { rule.replace_with.clone() },
        5 => if rule.item_type.is_script() { "Script" } else if rule.regex { "Regex" } else { "Literal" }.into(),
        _ => rule.comment.clone(),
    }
}

const WS_MR_DIRECTION_LABELS: [&str; 3] = ["Both", "Client to server", "Server to client"];

fn ws_mr_direction_index(direction: WsReplaceDirection) -> usize {
    match direction {
        WsReplaceDirection::Both => 0,
        WsReplaceDirection::ClientToServer => 1,
        WsReplaceDirection::ServerToClient => 2,
    }
}

fn ws_mr_direction_from_label(label: &str) -> WsReplaceDirection {
    match label {
        "Client to server" => WsReplaceDirection::ClientToServer,
        "Server to client" => WsReplaceDirection::ServerToClient,
        _ => WsReplaceDirection::Both,
    }
}

fn ws_mr_direction_label(direction: WsReplaceDirection) -> &'static str {
    WS_MR_DIRECTION_LABELS[ws_mr_direction_index(direction)]
}

fn mr_checkbox_indicator(checked: bool) -> CheckboxIndicator {
    let dark = theme::is_dark();
    let unchecked_border: gpui_kit::Hsla = if dark {
        theme::control_border()
    } else {
        rgb(0xb8b8b8).into()
    };
    CheckboxIndicator::new()
        .checked(checked)
        .size(px(14.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.))
        .border_1()
        .border_color(if checked {
            theme::accent()
        } else {
            unchecked_border
        })
        .bg(if checked {
            theme::accent()
        } else {
            theme::surface()
        })
        .when(checked, |this| {
            this.child(
                gpui_kit::component::Icon::new(IconName::Check)
                    .size(px(11.))
                    .text_color(rgb(0xffffff)),
            )
        })
}

fn mr_checkbox(
    id: impl Into<gpui_kit::ElementId>,
    label: impl Into<SharedString>,
    accessibility_label: impl Into<SharedString>,
    checked: bool,
    enabled: bool,
) -> Checkbox {
    Checkbox::new(id)
        .accessibility_label(accessibility_label)
        .checked(checked)
        .disabled(!enabled)
        .tab_stop(false)
        .flex()
        .items_center()
        .gap(px(7.))
        .py(px(2.))
        .text_size(theme::ui_font_size())
        .text_color(theme::foreground())
        .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
        .child(mr_checkbox_indicator(checked))
        .child(label.into())
}

fn mr_action_button(
    id: &'static str,
    label: &'static str,
    tip: &'static str,
    available: bool,
) -> gpui_kit::base::Button {
    let dark = theme::is_dark();
    let bg: gpui_kit::Hsla = rgb(if dark { 0x33363a } else { 0xe8e8e8 }).into();
    let hover_bg: gpui_kit::Hsla = rgb(if dark { 0x3e4247 } else { 0xdedede }).into();
    let active_bg: gpui_kit::Hsla = rgb(if dark { 0x4b5056 } else { 0xd4d4d4 }).into();
    let border: gpui_kit::Hsla = rgb(if dark { 0x4e535b } else { 0xd0d0d0 }).into();
    let fg: gpui_kit::Hsla = rgb(if dark { 0xdfe1e5 } else { 0x1a1a1a }).into();
    gpui_kit::base::Button::new(id)
        .accessibility_label(label)
        .disabled(!available)
        .w_full()
        .h(theme::scaled_px(24.).max(px(21.)))
        .rounded(px(3.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_size(theme::ui_font_size())
        .font_weight(gpui_kit::FontWeight::BOLD)
        .text_color(fg)
        .hover(move |s| if available { s.bg(hover_bg) } else { s })
        .active(move |s| if available { s.bg(active_bg) } else { s })
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
        })
        .child(label)
}

fn mr_help_popover(
    popover_id: &'static str,
    button_id: &'static str,
    title: &'static str,
    lines: &'static [&'static str],
) -> Popover {
    let dark = theme::is_dark();
    let icon_color: gpui_kit::Hsla = if dark {
        theme::foreground()
    } else {
        rgb(0x1a1a1a).into()
    };
    Popover::new(popover_id)
        .anchor(gpui_kit::Anchor::BottomLeft)
        .trigger(
            Button::new(button_id)
                .ghost()
                .compact()
                .p_0()
                .min_w_0()
                .w(theme::scaled_px(20.))
                .h(theme::scaled_px(20.))
                .tab_stop(false)
                .accessibility_label(format!("{title} 帮助"))
                .tooltip(format!("查看 {title} 说明"))
                .child(
                    gpui_kit::component::Icon::new(IconName::CircleQuestionMark)
                        .size(theme::scaled_px(19.))
                        .text_color(icon_color),
                ),
        )
        .content(move |_, _, _| {
            let mut body = div()
                .w(theme::scaled_px(320.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .text_size(theme::ui_font_size())
                .text_color(theme::foreground())
                .child(div().font_weight(gpui_kit::FontWeight::BOLD).child(title));
            for line in lines {
                body = body.child(*line);
            }
            body
        })
}

fn mr_table_width(columns: &[(&str, f32)]) -> gpui_kit::Pixels {
    let scale = theme::ui_scale();
    let inner: f32 = columns.iter().map(|(_, width)| (*width * scale).round()).sum();
    px(inner + 2.)
}

fn mr_table_right_triangle(dark: bool) -> impl IntoElement {
    let fill_color: gpui_kit::Hsla = if dark {
        rgb(0xd4d4d4).into()
    } else {
        rgb(0x141414).into()
    };
    gpui_kit::canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let mut builder = gpui_kit::PathBuilder::fill();
            builder.move_to(gpui_kit::point(b.left(), b.top()));
            builder.line_to(gpui_kit::point(b.right(), b.center().y));
            builder.line_to(gpui_kit::point(b.left(), b.bottom()));
            builder.close();
            if let Ok(path) = builder.build() {
                window.paint_path(path, fill_color);
            }
        },
    )
    .w(px(7.))
    .h(px(10.))
    .flex_shrink_0()
}

pub(in crate::workbench) const MR_TEST_REQUEST: &str = "POST /profile?token=secret HTTP/1.1\r\nHost: example.com\r\nUser-Agent: Mozilla/5.0\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 16\r\n\r\nname=demo&age=18";
pub(in crate::workbench) const MR_TEST_RESPONSE: &str = "HTTP/1.1 200 OK\r\nServer: Apache\r\nContent-Type: text/plain\r\nContent-Length: 6\r\n\r\nsecret";

pub(in crate::workbench) struct HttpMatchReplaceEditor {
    owner: WeakEntity<Workbench>,
    owner_window: gpui_kit::AnyWindowHandle,
    project: ProjectKey,
    original: Option<MatchAndReplaceRule>,
    rule: MatchAndReplaceRule,
    prefix: &'static str,
    target_select: Entity<SelectState<Vec<&'static str>>>,
    script_select: Entity<SelectState<Vec<&'static str>>>,
    match_input: Entity<InputState>,
    replace_input: Entity<InputState>,
    comment: Entity<InputState>,
    script_name: Entity<InputState>,
    pub(in crate::workbench) script: Entity<EditorState>,
    pub(in crate::workbench) test_original: Entity<EditorState>,
    pub(in crate::workbench) test_modified: Entity<EditorState>,
    error: Option<String>,
    focus: FocusHandle,
}

impl HttpMatchReplaceEditor {
    fn id(&self, suffix: &str) -> SharedString {
        format!("{}-{suffix}", self.prefix).into()
    }

    fn new(owner: WeakEntity<Workbench>, owner_window: gpui_kit::AnyWindowHandle,
        project: ProjectKey, original: Option<MatchAndReplaceRule>, prefix: &'static str,
        window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rule = original.clone().unwrap_or_else(|| MatchAndReplaceRule {
            id: Uuid::new_v4(), enabled: true, ..Default::default()
        });
        let target_select = cx.new(|cx| SelectState::new(MR_TARGET_LABELS.to_vec(),
            Some(gpui_kit::component::IndexPath::new(if rule.item_type.is_script() { 0 } else { mr_target_index(rule.item_type) })), window, cx));
        let script_select = cx.new(|cx| SelectState::new(vec!["Request script", "Response script"],
            Some(gpui_kit::component::IndexPath::new(usize::from(rule.item_type.is_response()))), window, cx));
        for selector in [&target_select, &script_select] {
            cx.subscribe_in(selector, window, |this, _, event, window, cx| {
                let SelectEvent::Confirm(Some(label)) = event else { return };
                let was_response = this.rule.item_type.is_response();
                this.rule.item_type = mr_target_from_label(label);
                this.update_placeholders(window, cx);
                if was_response != this.rule.item_type.is_response() { this.restore_test(window, cx); }
                this.error = None;
                cx.notify();
            }).detach();
        }
        let script = editing::new_plain_editor(window, cx);
        script.update(cx, |state, cx| state.set_value(rule.filter_script.clone(), window, cx));
        cx.subscribe(&script, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        let test_original = editing::new_editor(window, cx);
        let test_modified = editing::new_editor(window, cx);
        test_original.update(cx, |state, cx| state.set_value(if rule.item_type.is_response() { MR_TEST_RESPONSE } else { MR_TEST_REQUEST }, window, cx));
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        window.set_window_title(if original.is_some() { "Edit match/replace rule" } else { "Add match/replace rule" });
        let mut editor = Self {
            owner, owner_window, project, original,
            match_input: cx.new(|cx| InputState::new(window, cx).default_value(rule.match_pattern.clone())),
            replace_input: cx.new(|cx| InputState::new(window, cx).default_value(rule.replace_with.clone())),
            comment: cx.new(|cx| InputState::new(window, cx).default_value(rule.comment.clone())),
            script_name: cx.new(|cx| InputState::new(window, cx).default_value(rule.script_name.clone()).placeholder("Sign request header")),
            rule, prefix, target_select, script_select, script, test_original, test_modified, error: None, focus,
        };
        editor.update_placeholders(window, cx);
        editor
    }

    fn update_placeholders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let header = matches!(self.rule.item_type, MatchReplaceTarget::RequestHeader | MatchReplaceTarget::ResponseHeader);
        self.match_input.update(cx, |state, cx| state.set_placeholder(if header { "匹配内容或正则（留空则新增 Header）" } else { "匹配内容或正则" }, window, cx));
        self.replace_input.update(cx, |state, cx| state.set_placeholder(if header { "替换为（留空则删除匹配的 Header）" } else { "替换为" }, window, cx));
    }

    fn restore_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.test_original.update(cx, |state, cx| state.set_value(if self.rule.item_type.is_response() { MR_TEST_RESPONSE } else { MR_TEST_REQUEST }, window, cx));
        self.test_modified.update(cx, |state, cx| state.set_value("", window, cx));
        self.error = None;
        cx.notify();
    }

    fn switch_mode(&mut self, script: bool, window: &mut Window, cx: &mut Context<Self>) {
        let response = self.rule.item_type.is_response();
        self.rule.item_type = match (script, response) {
            (true, false) => MatchReplaceTarget::RequestScript,
            (true, true) => MatchReplaceTarget::ResponseScript,
            (false, false) => MatchReplaceTarget::RequestHeader,
            (false, true) => MatchReplaceTarget::ResponseHeader,
        };
        self.target_select.update(cx, |state, cx| state.set_selected_index(Some(gpui_kit::component::IndexPath::new(if response { 2 } else { 0 })), window, cx));
        self.script_select.update(cx, |state, cx| state.set_selected_index(Some(gpui_kit::component::IndexPath::new(usize::from(response))), window, cx));
        self.update_placeholders(window, cx);
        self.error = None;
        cx.notify();
    }

    fn snapshot(&self, window: &mut Window, cx: &mut App) -> Result<MatchAndReplaceRule, String> {
        let mut rule = self.rule.clone();
        rule.comment = editing::input_snapshot(&self.comment, window, cx).map_err(str::to_owned)?.to_string();
        if rule.item_type.is_script() {
            rule.script_name = editing::input_snapshot(&self.script_name, window, cx).map_err(str::to_owned)?.to_string();
            rule.filter_script = editing::snapshot(&self.script, window, cx).map_err(str::to_owned)?.to_string();
            mio_core::validate_filter_script(&rule.filter_script).map_err(|error| error.to_string())?;
            rule.match_pattern.clear(); rule.replace_with.clear(); rule.regex = false;
        } else {
            rule.match_pattern = editing::input_snapshot(&self.match_input, window, cx).map_err(str::to_owned)?.to_string();
            rule.replace_with = editing::input_snapshot(&self.replace_input, window, cx).map_err(str::to_owned)?.to_string();
            rule.script_name.clear(); rule.filter_script.clear();
        }
        Ok(rule)
    }

    fn test_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = self.snapshot(window, cx).and_then(|rule| {
            let original = editing::snapshot(&self.test_original, window, cx).map_err(str::to_owned)?;
            let bytes = mio_core::test_match_and_replace_rule(&rule, original.as_bytes(), rule.item_type.is_response()).map_err(|error| error.to_string())?;
            String::from_utf8(bytes).map_err(|error| error.to_string())
        });
        match result {
            Ok(text) => { self.test_modified.update(cx, |state, cx| state.set_value(text, window, cx)); self.error = None; }
            Err(error) => { self.test_modified.update(cx, |state, cx| state.set_value("", window, cx)); self.error = Some(error); }
        }
        cx.notify();
    }

    fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prefix == "mr" {
            let owner = self.owner.clone();
            let _ = self.owner_window.update(cx, |_, _, cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.extra_tools.mr_editor_open = false;
                    view.extra_tools.mr_editing = None;
                    view.extra_tools.mr_dialog = None;
                    view.extra_tools.mr_error = None;
                    cx.notify();
                });
            });
        }
        window.remove_window();
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rule = match self.snapshot(window, cx) {
            Ok(rule) => rule,
            Err(error) => { self.error = Some(error); cx.notify(); return; }
        };
        let owner = self.owner.clone();
        let project = self.project;
        let original = self.original.clone();
        let result = self.owner_window.update(cx, |_, _, cx| owner.update(cx, |view, cx| {
            view.save_http_match_replace_draft(project, original.as_ref(), rule, cx)
        }));
        match result {
            Ok(Ok(Ok(()))) => window.remove_window(),
            Ok(Ok(Err(error))) => self.error = Some(error),
            _ => self.error = Some("工作台已关闭；规则未保存。".into()),
        }
        cx.notify();
    }
}

impl Render for HttpMatchReplaceEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let script_mode = self.rule.item_type.is_script();
        let response = self.rule.item_type.is_response();
        let title = if self.original.is_some() { "Edit match/replace rule" } else { "Add match/replace rule" };
        let mut form = div().flex().flex_col().gap(px(8.)).child(
            div().flex().gap(px(8.))
                .child(button(self.id("mode-settings"), "Settings mode").selected(!script_mode).on_click(cx.listener(|this, _, window, cx| this.switch_mode(false, window, cx))))
                .child(button(self.id("mode-script"), "Script mode (MioScript)").selected(script_mode).on_click(cx.listener(|this, _, window, cx| this.switch_mode(true, window, cx))))
                .when(script_mode, |row| row.child(button(self.id("load-template"), "Load template").on_click(cx.listener(|this, _, window, cx| {
                    let template = if this.rule.item_type.is_response() { mio_core::DEFAULT_RESPONSE_SCRIPT_TEMPLATE } else { mio_core::DEFAULT_REQUEST_SCRIPT_TEMPLATE };
                    this.script.update(cx, |state, cx| state.set_value(template, window, cx));
                    this.error = None; cx.notify();
                }))))
        );
        if script_mode {
            form = form
                .child(div().flex().gap(px(8.)).items_center().child("Type").child(Select::new(&self.script_select).id(self.id("script-type-select")).small().w(px(220.))))
                .child(Input::new(&self.script_name).id(self.id("script-name-input")).aria_label("Script name").small())
                .child(div().id(self.id("script-editor")).test_support().h(px(140.)).child(Editor::new(&self.script).aria_label("MioScript script").size_full()))
                .child("Rhai：request / response 提供 headers、body；支持 sha256_hex、regex_replace、base64_encode。返回修改后的 request / response。")
                .child(div().id(self.id("compilation-errors")).test_support().role(gpui_kit::accesskit::Role::Status).child(
                    match mio_core::validate_filter_script(self.script.read(cx).value().as_ref()) {
                        Ok(()) => "Compilation errors: None".to_owned(),
                        Err(error) => format!("Compilation errors: {error}"),
                    }
                ));
        } else {
            form = form
                .child(div().flex().gap(px(8.)).items_center().child("Type").child(Select::new(&self.target_select).id(self.id("type-select")).small().w(px(240.))))
                .child(Input::new(&self.match_input).id(self.id("match-input")).aria_label("Match").small())
                .child(Input::new(&self.replace_input).id(self.id("replace-input")).aria_label("Replace").small())
                .child(
                    mr_checkbox(self.id("regex"), "Regex match", "Regex match", self.rule.regex, true)
                        .on_change({
                            let entity = cx.entity();
                            move |state, _, _, cx| {
                                entity.update(cx, |this, cx| {
                                    this.rule.regex = state == CheckboxState::Checked;
                                    cx.notify();
                                });
                            }
                        }),
                );
        }
        form = form.child(Input::new(&self.comment).id(self.id("comment-input")).aria_label("Comment").small());
        let test_panel = div().flex().gap(px(10.)).h(px(200.))
            .child(div().flex_1().min_w_0().flex().flex_col()
                .child(div().flex().justify_between().items_center().child(if response { "Original response" } else { "Original request" })
                    .child(button(self.id("test-restore"), "Restore default").on_click(cx.listener(|this, _, window, cx| this.restore_test(window, cx)))))
                .child(div().id(self.id("test-original")).test_support().flex_1().min_h_0().child(Editor::new(&self.test_original).aria_label("Original HTTP message").size_full())))
            .child(div().flex().items_center().child(button(self.id("test-btn"), "Test").on_click(cx.listener(|this, _, window, cx| this.test_rule(window, cx)))))
            .child(div().flex_1().min_w_0().flex().flex_col().child(if response { "Auto-modified response" } else { "Auto-modified request" })
                .child(div().id(self.id("test-modified")).test_support().flex_1().min_h_0().child(Editor::new(&self.test_modified).aria_label("Auto-modified HTTP message").readonly(true).size_full())));
        let close_owner = self.owner.clone();
        let close_owner_window = self.owner_window;
        let close_is_wb = self.prefix == "mr";
        let title_bar = crate::chrome::render_title_bar("mio-http-mr-editor", title, crate::chrome::TitleBarControls::DIALOG, move |window, cx| {
            if close_is_wb {
                let owner = close_owner.clone();
                let _ = close_owner_window.update(cx, |_, _, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        view.extra_tools.mr_editor_open = false;
                        view.extra_tools.mr_editing = None;
                        view.extra_tools.mr_dialog = None;
                        view.extra_tools.mr_error = None;
                        cx.notify();
                    });
                });
            }
            window.remove_window();
        }, window, cx);
        div().size_full().flex().flex_col().bg(theme::background()).text_color(theme::foreground())
            .child(title_bar)
            .child(div().id(self.id("editor")).test_support().key_context("MioSettings").track_focus(&self.focus)
                .on_action(cx.listener(|this, _: &crate::CloseSettings, window, cx| this.close_dialog(window, cx)))
                .flex_1().min_h_0().p(px(12.)).flex().flex_col().gap(px(12.)).overflow_y_scroll()
                .child(form).child(test_panel)
                .when_some(self.error.clone(), |this, error| this.child(div().id(self.id("error")).test_support().role(gpui_kit::accesskit::Role::Alert).aria_label(error.clone()).text_color(theme::danger_fg()).child(error)))
                .child(div().flex().justify_end().gap(px(8.))
                    .child(button(self.id("submit"), "OK").primary().on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))))
                    .child(button(self.id("cancel-edit"), "Cancel").on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))))))
    }
}

pub(in crate::workbench) fn open_http_match_replace_dialog(owner: WeakEntity<Workbench>,
    owner_window: gpui_kit::AnyWindowHandle, project: ProjectKey, rule: Option<MatchAndReplaceRule>,
    prefix: &'static str, window: &mut Window, cx: &mut App) -> anyhow::Result<gpui_kit::AnyWindowHandle> {
    let bounds = gpui_kit::Bounds::centered_at(window.window_bounds().get_bounds().center(), gpui_kit::size(px(900.), px(680.)));
    let options = crate::chrome::mio_window_options(bounds, false, false);
    crate::chrome::open_popup_window(options, cx, |window, cx| cx.new(|cx| HttpMatchReplaceEditor::new(owner, owner_window, project, rule, prefix, window, cx))).map(|(window, _)| window)
}

pub(super) struct WorkbenchMatchReplaceTable {
    owner: WeakEntity<Workbench>,
}

const WB_MR_COLUMNS: [(&str, f32); 7] = [
    ("Enabled", 68.),
    ("Item", 132.),
    ("Name", 174.),
    ("Match", 182.),
    ("Replace", 224.),
    ("Type", 68.),
    ("Comment", 206.),
];

impl WorkbenchMatchReplaceTable {
    fn http_rules(&self, cx: &App) -> Vec<MatchAndReplaceRule> {
        self.owner.upgrade().map_or_else(Vec::new, |wb| {
            wb.read(cx)
                .workspace
                .match_and_replace_rules
                .iter()
                .filter(|r| !mio_core::is_advanced_proxy_rule(r))
                .cloned()
                .collect()
        })
    }
}

impl TableDelegate for WorkbenchMatchReplaceTable {
    fn columns_count(&self, _: &App) -> usize {
        WB_MR_COLUMNS.len()
    }
    fn column(&self, index: usize, _: &App) -> Column {
        let scale = theme::ui_scale();
        Column::new(index.to_string(), WB_MR_COLUMNS[index].0)
            .width((WB_MR_COLUMNS[index].1 * scale).round())
            .movable(false)
            .resizable(index + 1 < WB_MR_COLUMNS.len())
    }
    fn rows_count(&self, cx: &App) -> usize {
        self.http_rules(cx).len()
    }
    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        div().id(("mr-row", row))
    }
    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(owner) = self.owner.upgrade() else {
            return div().into_any_element();
        };
        let (rule, editable) = {
            let wb = owner.read(cx);
            let http_rules = self.http_rules(cx);
            let Some(rule) = http_rules.get(row).cloned() else {
                return div().into_any_element();
            };
            let editable =
                wb.project.is_some() && wb.transition.is_none() && wb.backend_available();
            (rule, editable)
        };
        match column {
            0 => {
                let id = rule.id;
                let checked = rule.enabled;
                let wb_weak = self.owner.clone();
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Checkbox::new(("mr-toggle", row))
                            .accessibility_label(format!("启用替换规则 {}", rule.match_pattern))
                            .checked(checked)
                            .disabled(!editable)
                            .tab_stop(false)
                            .size(px(14.))
                            .child(mr_checkbox_indicator(checked))
                            .on_change(move |_, _, _, cx| {
                                let _ = wb_weak.update(cx, |view, cx| {
                                    view.toggle_http_match_replace_rule(id, cx);
                                });
                            }),
                    )
                    .into_any_element()
            }
            _ => div().truncate().child(mr_rule_cell(&rule, column)).into_any_element(),
        }
    }
    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        let rules = self.http_rules(cx);
        let rule = rules.get(row)?;
        Some(mr_rule_cell(rule, column).into())
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div().size_full().bg(theme::surface())
    }
    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }
}

pub(super) struct WorkbenchWsMatchReplaceTable {
    owner: WeakEntity<Workbench>,
}

const WB_WS_MR_COLUMNS: [(&str, f32); 6] = [
    ("Enabled", 68.),
    ("Direction", 132.),
    ("Match", 174.),
    ("Replace", 224.),
    ("Type", 68.),
    ("Comment", 212.),
];

impl WorkbenchWsMatchReplaceTable {
    fn ws_rules(&self, cx: &App) -> Vec<WsMatchAndReplaceRule> {
        self.owner.upgrade().map_or_else(Vec::new, |wb| {
            extract_ws_match_and_replace_rules(&wb.read(cx).workspace.match_and_replace_rules)
        })
    }
}

impl TableDelegate for WorkbenchWsMatchReplaceTable {
    fn columns_count(&self, _: &App) -> usize {
        WB_WS_MR_COLUMNS.len()
    }
    fn column(&self, index: usize, _: &App) -> Column {
        let scale = theme::ui_scale();
        Column::new(index.to_string(), WB_WS_MR_COLUMNS[index].0)
            .width((WB_WS_MR_COLUMNS[index].1 * scale).round())
            .movable(false)
            .resizable(index + 1 < WB_WS_MR_COLUMNS.len())
    }
    fn rows_count(&self, cx: &App) -> usize {
        self.ws_rules(cx).len()
    }
    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        div().id(("ws-mr-row", row))
    }
    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(owner) = self.owner.upgrade() else {
            return div().into_any_element();
        };
        let (rule, editable) = {
            let wb = owner.read(cx);
            let ws_rules = self.ws_rules(cx);
            let Some(rule) = ws_rules.get(row).cloned() else {
                return div().into_any_element();
            };
            let editable =
                wb.project.is_some() && wb.transition.is_none() && wb.backend_available();
            (rule, editable)
        };
        match column {
            0 => {
                let id = rule.id;
                let checked = rule.enabled;
                let wb_weak = self.owner.clone();
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Checkbox::new(("ws-mr-toggle", row))
                            .accessibility_label(format!(
                                "启用 WebSocket 替换规则 {}",
                                rule.match_pattern
                            ))
                            .checked(checked)
                            .disabled(!editable)
                            .tab_stop(false)
                            .size(px(14.))
                            .child(mr_checkbox_indicator(checked))
                            .on_change(move |_, _, _, cx| {
                                let _ = wb_weak.update(cx, |view, cx| {
                                    view.toggle_ws_match_replace_rule(id, cx);
                                });
                            }),
                    )
                    .into_any_element()
            }
            1 => div()
                .truncate()
                .child(ws_mr_direction_label(rule.direction))
                .into_any_element(),
            2 => div().truncate().child(rule.match_pattern).into_any_element(),
            3 => div().truncate().child(rule.replace_with).into_any_element(),
            4 => div()
                .child(if rule.is_regex { "Regex" } else { "Literal" })
                .into_any_element(),
            _ => div().truncate().child(rule.comment).into_any_element(),
        }
    }
    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        let rules = self.ws_rules(cx);
        let rule = rules.get(row)?;
        Some(match column {
            0 => if rule.enabled { "Yes" } else { "No" }.into(),
            1 => ws_mr_direction_label(rule.direction).into(),
            2 => rule.match_pattern.clone().into(),
            3 => rule.replace_with.clone().into(),
            4 => if rule.is_regex { "Regex" } else { "Literal" }.into(),
            _ => rule.comment.clone().into(),
        })
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div().size_full().bg(theme::surface())
    }
    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }
}

impl MatchReplacePane {
    pub(in crate::workbench) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let mr_target_select = cx.new(|cx| {
            SelectState::new(
                MR_TARGET_LABELS.to_vec(),
                Some(gpui_kit::component::IndexPath::new(0)),
                window,
                cx,
            )
        });
        cx.subscribe_in(&mr_target_select, window, |this, _, event, _, cx| {
            let SelectEvent::Confirm(value) = event;
            if let Some(value) = value {
                this.extra_tools.mr_target = mr_target_from_label(value);
            }
            cx.notify();
        }).detach();
        let owner = cx.weak_entity();
        let mr_table = cx.new(|cx| {
            TableState::new(WorkbenchMatchReplaceTable { owner: owner.clone() }, window, cx)
                .sortable(false).col_selectable(false)
        });
        cx.subscribe_in(&mr_table, window, |this, _, event, window, cx| {
            if let TableEvent::SelectRow(row) | TableEvent::DoubleClickedRow(row) = event {
                let rule = this.workspace.match_and_replace_rules.iter()
                    .filter(|rule| !mio_core::is_advanced_proxy_rule(rule)).nth(*row).cloned();
                this.extra_tools.mr_selected = rule.as_ref().map(|rule| rule.id);
                if matches!(event, TableEvent::DoubleClickedRow(_))
                    && let Some(rule) = rule
                {
                    this.load_http_match_replace_for_edit(&rule, window, cx);
                    if let Some(project) = this.project {
                        if let Some(prev) = this.extra_tools.mr_dialog.take() {
                            let _ = prev.update(cx, |_, window, _| window.remove_window());
                        }
                        this.extra_tools.mr_dialog = open_http_match_replace_dialog(
                            cx.weak_entity(),
                            window.window_handle(),
                            project,
                            Some(rule),
                            "mr",
                            window,
                            cx,
                        )
                        .ok();
                    }
                }
                cx.notify();
            }
        }).detach();
        let ws_mr_direction_select = cx.new(|cx| {
            SelectState::new(
                WS_MR_DIRECTION_LABELS.to_vec(),
                Some(gpui_kit::component::IndexPath::new(0)),
                window,
                cx,
            )
        });
        cx.subscribe_in(&ws_mr_direction_select, window, |this, _, event, _, cx| {
            let SelectEvent::Confirm(value) = event;
            if let Some(value) = value {
                this.extra_tools.ws_mr_direction = ws_mr_direction_from_label(value);
            }
            cx.notify();
        })
        .detach();
        let ws_mr_table = cx.new(|cx| {
            TableState::new(WorkbenchWsMatchReplaceTable { owner }, window, cx)
                .sortable(false)
                .col_selectable(false)
        });
        cx.subscribe_in(&ws_mr_table, window, |this, _, event, window, cx| {
            if let TableEvent::SelectRow(row) | TableEvent::DoubleClickedRow(row) = event {
                let rule = extract_ws_match_and_replace_rules(
                    &this.workspace.match_and_replace_rules,
                )
                .into_iter()
                .nth(*row);
                this.extra_tools.ws_mr_selected = rule.as_ref().map(|rule| rule.id);
                if matches!(event, TableEvent::DoubleClickedRow(_))
                    && let Some(rule) = rule
                {
                    this.load_ws_match_replace_for_edit(&rule, window, cx);
                }
                cx.notify();
            }
        })
        .detach();
        let decoder_input = editing::new_editor(window, cx);
        let decoder_output = editing::new_editor(window, cx);
        let comparer_view = editing::new_editor(window, cx);
        let comparer_left_view = editing::new_editor(window, cx);
        let comparer_right_view = editing::new_editor(window, cx);
        let plain_selection_seqs: [Rc<Cell<u64>>; 5] =
            std::array::from_fn(|_| Rc::new(Cell::new(0)));
        let s1 =
            editing::observe_selection_state(&decoder_input, plain_selection_seqs[0].clone(), cx);
        let s2 =
            editing::observe_selection_state(&decoder_output, plain_selection_seqs[1].clone(), cx);
        let s3 =
            editing::observe_selection_state(&comparer_view, plain_selection_seqs[2].clone(), cx);
        let s4 = editing::observe_selection_state(
            &comparer_left_view,
            plain_selection_seqs[3].clone(),
            cx,
        );
        let s5 = editing::observe_selection_state(
            &comparer_right_view,
            plain_selection_seqs[4].clone(),
            cx,
        );
        let sync_left_view = comparer_left_view.clone();
        let sync_right_view = comparer_right_view.clone();
        cx.observe(&comparer_left_view, move |this, left_ed, cx| {
            if this.extra_tools.comparer_side_by_side && this.extra_tools.comparer_sync_views {
                let offset = left_ed.read(cx).scroll_offset();
                if sync_right_view.read(cx).scroll_offset() != offset {
                    sync_right_view.update(cx, |right_ed, cx| right_ed.set_scroll_offset(offset, cx));
                }
            }
        })
        .detach();
        cx.observe(&comparer_right_view, move |this, right_ed, cx| {
            if this.extra_tools.comparer_side_by_side && this.extra_tools.comparer_sync_views {
                let offset = right_ed.read(cx).scroll_offset();
                if sync_left_view.read(cx).scroll_offset() != offset {
                    sync_left_view.update(cx, |left_ed, cx| left_ed.set_scroll_offset(offset, cx));
                }
            }
        })
        .detach();
        cx.subscribe_in(&decoder_input, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                this.on_decoder_input_edited(window, cx);
            }
        })
        .detach();
        Self {
            mr_editing: None,
            mr_selected: None,
            mr_editor_open: false,
            mr_dialog: None,
            mr_error: None,
            mr_target_select,
            mr_table,
            mr_target: MatchReplaceTarget::RequestHeader,
            mr_match: cx.new(|cx| InputState::new(window, cx).placeholder("匹配内容或正则（留空则新增 Header）")),
            mr_replace: cx.new(|cx| InputState::new(window, cx).placeholder("替换为（留空则删除匹配的 Header）")),
            mr_comment: cx.new(|cx| InputState::new(window, cx).placeholder("备注")),
            mr_regex: false,
            ws_mr_selected: None,
            ws_mr_editor_open: false,
            ws_mr_editing: None,
            ws_mr_direction: WsReplaceDirection::Both,
            ws_mr_direction_select,
            ws_mr_table,
            ws_mr_match: cx.new(|cx| InputState::new(window, cx).placeholder("匹配文本或正则表达式")),
            ws_mr_replace: cx.new(|cx| InputState::new(window, cx).placeholder("替换为（可留空）")),
            ws_mr_comment: cx.new(|cx| InputState::new(window, cx).placeholder("备注（可选）")),
            ws_mr_regex: false,
            ws_mr_error: None,
            decoder_input,
            decoder_output,
            decoder_input_bytes: None,
            decoder_output_bytes: None,
            decoder_input_hex_view: false,
            decoder_output_hex_view: false,
            decoder_input_revision: 0,
            decoder_chain_start: 0,
            decoder_chain_continuing: false,
            decoder_mode: "".into(),
            steps: Vec::new(),
            comparer_items: Vec::new(),
            left_index: 0,
            right_index: 0,
            comparer_selected: 0,
            byte_mode: false,
            comparer_side_by_side: false,
            comparer_sync_views: true,
            comparer_diff_cursor: 0,
            comparer_unified_blocks: Vec::new(),
            comparer_side_blocks: Vec::new(),
            comparer_view,
            comparer_left_view,
            comparer_right_view,
            capture_enabled: true,
            logger_max_entries: 500,
            logger_entries: Vec::new(),
            logger_source_filter: None,
            logger_sort: LoggerSort::default(),
            logger_filters: logger::LoggerFilters::default(),
            logger_search: cx
                .new(|cx| InputState::new(window, cx).placeholder("筛选 URL / 方法 / 状态码")),
            logger_query_revision: 0,
            logger_query_cancel: None,
            logger_selected: None,
            logger_pending_read: None,
            logger_loaded_detail: None,
            logger_request: MessageEditor::new(window, cx),
            logger_response: MessageEditor::new(window, cx),
            plain_selection_seqs,
            _selection_subscriptions: [s1, s2, s3, s4, s5],
        }
    }

    pub(in crate::workbench) fn decoder_selection_candidate(
        &self,
        window: Option<&Window>,
        cx: &App,
    ) -> Option<editing::SelectionCandidate> {
        let mut candidates = Vec::with_capacity(2);
        if let Some(mut input) = editing::plain_editor_selection_candidate(
            &self.decoder_input,
            self.plain_selection_seqs[0].get(),
            window,
            cx,
        ) {
            let state = self.decoder_input.read(cx);
            if let Some((saved_rev, _)) = &self.decoder_input_bytes
                && *saved_rev == state.document_revision()
                && let Some(bytes) = editing::decode_selected_hex_digits(&state.selected_value())
            {
                input.raw_bytes = bytes;
                input.identity.source = editing::SelectionSource::Hex;
            }
            candidates.push(input);
        }
        if let Some(mut output) = editing::plain_editor_selection_candidate(
            &self.decoder_output,
            self.plain_selection_seqs[1].get(),
            window,
            cx,
        ) {
            if self
                .decoder_output_bytes
                .as_ref()
                .is_some_and(|raw| std::str::from_utf8(raw).is_err())
                && let Some(bytes) = editing::decode_selected_hex_digits(
                    &self.decoder_output.read(cx).selected_value(),
                )
            {
                output.raw_bytes = bytes;
                output.identity.source = editing::SelectionSource::Hex;
            }
            candidates.push(output);
        }
        editing::pick_selection_candidate(candidates)
    }

    pub(in crate::workbench) fn comparer_selection_candidate(
        &self,
        window: Option<&Window>,
        cx: &App,
    ) -> Option<editing::SelectionCandidate> {
        if self.comparer_side_by_side {
            editing::pick_selection_candidate(
                [
                    editing::plain_editor_selection_candidate(
                        &self.comparer_left_view,
                        self.plain_selection_seqs[3].get(),
                        window,
                        cx,
                    ),
                    editing::plain_editor_selection_candidate(
                        &self.comparer_right_view,
                        self.plain_selection_seqs[4].get(),
                        window,
                        cx,
                    ),
                ]
                .into_iter()
                .flatten(),
            )
        } else {
            editing::plain_editor_selection_candidate(
                &self.comparer_view,
                self.plain_selection_seqs[2].get(),
                window,
                cx,
            )
        }
    }
}

impl Workbench {
    pub(in crate::workbench) fn reset_extra_tools(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(dlg) = self.extra_tools.mr_dialog.take() {
            let _ = dlg.update(cx, |_, window, _| window.remove_window());
        }
        self.extra_tools.mr_editing = None;
        self.extra_tools.mr_selected = None;
        self.extra_tools.mr_editor_open = false;
        self.extra_tools.mr_error = None;
        self.extra_tools.mr_target = MatchReplaceTarget::RequestHeader;
        self.extra_tools.mr_target_select.update(cx, |state, cx| {
            state.set_selected_index(Some(gpui_kit::component::IndexPath::new(0)), window, cx);
        });
        self.extra_tools.mr_table.update(cx, |table, cx| {
            table.clear_selection(cx);
            table.refresh(cx);
        });
        self.extra_tools.mr_regex = false;
        self.extra_tools.ws_mr_selected = None;
        self.extra_tools.ws_mr_editor_open = false;
        self.extra_tools.ws_mr_editing = None;
        self.extra_tools.ws_mr_direction = WsReplaceDirection::Both;
        self.extra_tools.ws_mr_direction_select.update(cx, |state, cx| {
            state.set_selected_index(Some(gpui_kit::component::IndexPath::new(0)), window, cx);
        });
        self.extra_tools.ws_mr_table.update(cx, |table, cx| {
            table.clear_selection(cx);
            table.refresh(cx);
        });
        self.extra_tools.ws_mr_regex = false;
        self.extra_tools.ws_mr_error = None;
        for input in [
            &self.extra_tools.mr_match,
            &self.extra_tools.mr_replace,
            &self.extra_tools.mr_comment,
            &self.extra_tools.ws_mr_match,
            &self.extra_tools.ws_mr_replace,
            &self.extra_tools.ws_mr_comment,
            &self.extra_tools.logger_search,
        ] {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        for editor in [
            &self.extra_tools.decoder_input,
            &self.extra_tools.decoder_output,
            &self.extra_tools.comparer_view,
            &self.extra_tools.comparer_left_view,
            &self.extra_tools.comparer_right_view,
        ] {
            editor.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.extra_tools.decoder_input_bytes = None;
        self.extra_tools.decoder_output_bytes = None;
        self.extra_tools.decoder_input_hex_view = false;
        self.extra_tools.decoder_output_hex_view = false;
        self.extra_tools.decoder_input_revision =
            self.extra_tools.decoder_input.read(cx).document_revision();
        self.extra_tools.decoder_chain_start = 0;
        self.extra_tools.decoder_chain_continuing = false;
        self.extra_tools.decoder_mode = "".into();
        self.extra_tools.steps.clear();
        self.extra_tools.comparer_items.clear();
        self.extra_tools.left_index = 0;
        self.extra_tools.right_index = 0;
        self.extra_tools.comparer_selected = 0;
        self.extra_tools.byte_mode = false;
        self.extra_tools.comparer_side_by_side = false;
        self.extra_tools.comparer_sync_views = true;
        self.extra_tools.comparer_diff_cursor = 0;
        self.extra_tools.comparer_unified_blocks.clear();
        self.extra_tools.comparer_side_blocks.clear();
        self.extra_tools.capture_enabled = true;
        self.extra_tools.logger_max_entries = 500;
        if let Some(cancel) = self.extra_tools.logger_query_cancel.take() {
            cancel.cancel();
        }
        self.extra_tools.logger_query_revision =
            self.extra_tools.logger_query_revision.saturating_add(1);
        self.extra_tools.logger_entries.clear();
        self.extra_tools.logger_source_filter = None;
        self.extra_tools.logger_sort = LoggerSort::default();
        self.extra_tools.logger_filters = Default::default();
        self.extra_tools.logger_selected = None;
        self.extra_tools.logger_pending_read = None;
        self.extra_tools.logger_loaded_detail = None;
        self.clear_logger_selection_state(cx);
        self.extra_tools.logger_request.load(b"", window, cx);
        self.extra_tools.logger_response.load(b"", window, cx);
        self.clear_context_viewer("logger", cx);
    }

    pub(in crate::workbench) fn validate_extra_tools_composition(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), &'static str> {
        for input in [
            &self.extra_tools.mr_match,
            &self.extra_tools.mr_replace,
            &self.extra_tools.mr_comment,
            &self.extra_tools.ws_mr_match,
            &self.extra_tools.ws_mr_replace,
            &self.extra_tools.ws_mr_comment,
            &self.extra_tools.logger_search,
        ] {
            editing::input_snapshot(input, window, cx)?;
        }
        editing::snapshot(&self.extra_tools.decoder_input, window, cx)?;
        Ok(())
    }

    pub(in crate::workbench) fn switch_proxy_page(
        &mut self,
        page: ProxyPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.proxy_page = page;
        if page == ProxyPage::MatchAndReplace {
            self.sync_mr_table_selection(cx);
            self.sync_ws_mr_table_selection(cx);
        }
        self.refresh_websocket_history(window, cx);
        cx.notify();
    }

    fn add_match_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let snapshots = (|| {
            Ok::<_, &'static str>((
                editing::input_snapshot(&self.extra_tools.mr_match, window, cx)?.to_string(),
                editing::input_snapshot(&self.extra_tools.mr_replace, window, cx)?.to_string(),
                editing::input_snapshot(&self.extra_tools.mr_comment, window, cx)?.to_string(),
            ))
        })();
        let (match_pattern, replace_with, comment) = match snapshots {
            Ok(values) => values,
            Err(error) => {
                self.extra_tools.mr_error = Some(error.into());
                self.fail(error, cx);
                return;
            }
        };
        let is_header = matches!(
            self.extra_tools.mr_target,
            MatchReplaceTarget::RequestHeader | MatchReplaceTarget::ResponseHeader
        );
        if match_pattern.is_empty() && !is_header {
            self.extra_tools.mr_error = Some("请填写 HTTP 匹配模式（不能为空）。".into());
            cx.notify();
            return;
        }
        let prev_rules = self.workspace.match_and_replace_rules.clone();
        if let Some(edit_id) = self.extra_tools.mr_editing
            && let Some(existing) = self
                .workspace
                .match_and_replace_rules
                .iter_mut()
                .find(|r| r.id == edit_id)
        {
            existing.item_type = self.extra_tools.mr_target;
            existing.match_pattern = match_pattern;
            existing.replace_with = replace_with;
            existing.regex = self.extra_tools.mr_regex;
            existing.comment = comment;
            existing.script_name.clear();
            existing.filter_script.clear();
        } else {
            self.workspace
                .match_and_replace_rules
                .push(MatchAndReplaceRule {
                    id: Uuid::new_v4(),
                    enabled: true,
                    item_type: self.extra_tools.mr_target,
                    match_pattern,
                    replace_with,
                    regex: self.extra_tools.mr_regex,
                    comment,
                    script_name: String::new(),
                    filter_script: String::new(),
                });
        }
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.match_and_replace_rules = prev_rules;
            let message = format!("未保存规则：{error}");
            self.extra_tools.mr_error = Some(message.clone());
            self.fail(message, cx);
            return;
        }
        if let Some(dlg) = self.extra_tools.mr_dialog.take() {
            let _ = dlg.update(cx, |_, window, _| window.remove_window());
        }
        self.extra_tools.mr_editing = None;
        self.extra_tools.mr_editor_open = false;
        self.extra_tools.mr_error = None;
        self.extra_tools.mr_regex = false;
        self.extra_tools.mr_target = MatchReplaceTarget::RequestHeader;
        self.extra_tools.mr_target_select.update(cx, |state, cx| {
            state.set_selected_index(Some(gpui_kit::component::IndexPath::new(0)), window, cx);
        });
        for input in [
            &self.extra_tools.mr_match,
            &self.extra_tools.mr_replace,
            &self.extra_tools.mr_comment,
        ] {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.sync_mr_table_selection(cx);
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn sync_mr_table_selection(&mut self, cx: &mut Context<Self>) {
        let sel = self.extra_tools.mr_selected.and_then(|id| {
            self.workspace
                .match_and_replace_rules
                .iter()
                .filter(|r| !mio_core::is_advanced_proxy_rule(r))
                .position(|r| r.id == id)
        });
        if sel.is_none() {
            self.extra_tools.mr_selected = None;
        }
        self.extra_tools.mr_table.update(cx, |table, cx| {
            if let Some(row) = sel {
                table.set_selected_row(row, cx);
            } else {
                table.clear_selection(cx);
            }
            table.refresh(cx);
            cx.notify();
        });
    }

    pub(in crate::workbench) fn load_http_match_replace_for_edit(
        &mut self,
        rule: &MatchAndReplaceRule,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let any_composing = editing::input_snapshot(&self.extra_tools.mr_match, window, cx).is_err()
            || editing::input_snapshot(&self.extra_tools.mr_replace, window, cx).is_err()
            || editing::input_snapshot(&self.extra_tools.mr_comment, window, cx).is_err();
        if any_composing {
            self.extra_tools.mr_error = Some(editing::COMPOSITION_PENDING.into());
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        self.extra_tools.mr_editing = Some(rule.id);
        self.extra_tools.mr_editor_open = true;
        self.extra_tools.mr_error = None;
        self.extra_tools.mr_target = rule.item_type;
        self.extra_tools.mr_target_select.update(cx, |state, cx| {
            state.set_selected_index(
                Some(gpui_kit::component::IndexPath::new(mr_target_index(rule.item_type))),
                window,
                cx,
            );
        });
        self.extra_tools.mr_regex = rule.regex;
        self.extra_tools
            .mr_match
            .update(cx, |s, cx| s.set_value(rule.match_pattern.clone(), window, cx));
        self.extra_tools
            .mr_replace
            .update(cx, |s, cx| s.set_value(rule.replace_with.clone(), window, cx));
        self.extra_tools
            .mr_comment
            .update(cx, |s, cx| s.set_value(rule.comment.clone(), window, cx));
        cx.notify();
    }

    pub(in crate::workbench) fn move_http_match_replace_rule(
        &mut self,
        id: Uuid,
        down: bool,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let http_indices: Vec<usize> = self
            .workspace
            .match_and_replace_rules
            .iter()
            .enumerate()
            .filter_map(|(i, r)| (!mio_core::is_advanced_proxy_rule(r)).then_some(i))
            .collect();
        let Some(pos) = http_indices
            .iter()
            .position(|&idx| self.workspace.match_and_replace_rules[idx].id == id)
        else {
            return;
        };
        let target_pos = if down {
            pos.checked_add(1).filter(|&p| p < http_indices.len())
        } else {
            pos.checked_sub(1)
        };
        if let Some(target_pos) = target_pos {
            self.workspace
                .match_and_replace_rules
                .swap(http_indices[pos], http_indices[target_pos]);
            self.sync_mr_table_selection(cx);
            self.save_workspace(cx);
            cx.notify();
        }
    }

    pub(in crate::workbench) fn toggle_http_match_replace_rule(
        &mut self,
        id: Uuid,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        if let Some(rule) = self
            .workspace
            .match_and_replace_rules
            .iter_mut()
            .find(|r| r.id == id)
        {
            rule.enabled = !rule.enabled;
            self.sync_mr_table_selection(cx);
            self.save_workspace(cx);
            cx.notify();
        }
    }

    pub(in crate::workbench) fn remove_http_match_replace_rule(
        &mut self,
        id: Uuid,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        if self.extra_tools.mr_editing == Some(id) {
            self.extra_tools.mr_editing = None;
            self.extra_tools.mr_editor_open = false;
            self.extra_tools.mr_error = None;
        }
        self.workspace
            .match_and_replace_rules
            .retain(|rule| rule.id != id);
        self.sync_mr_table_selection(cx);
        self.save_workspace(cx);
        cx.notify();
    }

    fn set_all_http_match_replace_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let mut changed = false;
        for rule in self
            .workspace
            .match_and_replace_rules
            .iter_mut()
            .filter(|r| !mio_core::is_advanced_proxy_rule(r))
        {
            if rule.enabled != enabled {
                rule.enabled = enabled;
                changed = true;
            }
        }
        if changed {
            self.sync_mr_table_selection(cx);
            self.save_workspace(cx);
            cx.notify();
        }
    }

    fn clear_http_match_replace_rules(&mut self, cx: &mut Context<Self>) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let before = self.workspace.match_and_replace_rules.len();
        self.workspace
            .match_and_replace_rules
            .retain(mio_core::is_advanced_proxy_rule);
        if self.workspace.match_and_replace_rules.len() != before {
            if let Some(dlg) = self.extra_tools.mr_dialog.take() {
                let _ = dlg.update(cx, |_, window, _| window.remove_window());
            }
            self.extra_tools.mr_editing = None;
            self.extra_tools.mr_selected = None;
            self.extra_tools.mr_editor_open = false;
            self.extra_tools.mr_error = None;
            self.sync_mr_table_selection(cx);
            self.save_workspace(cx);
            cx.notify();
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::workbench) fn upsert_http_match_replace_rule(
        &mut self,
        project: Option<ProjectKey>,
        editing_id: Option<Uuid>,
        item_type: MatchReplaceTarget,
        match_pattern: String,
        replace_with: String,
        regex: bool,
        comment: String,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return Err("当前项目尚未就绪或后台不可用；规则未修改。".into());
        }
        let prev = self.workspace.match_and_replace_rules.clone();
        if let Some(edit_id) = editing_id
            && let Some(existing) = self
                .workspace
                .match_and_replace_rules
                .iter_mut()
                .find(|r| r.id == edit_id && !mio_core::is_advanced_proxy_rule(r))
        {
            existing.item_type = item_type;
            existing.match_pattern = match_pattern;
            existing.replace_with = replace_with;
            existing.regex = regex;
            existing.comment = comment;
        } else {
            self.workspace
                .match_and_replace_rules
                .push(MatchAndReplaceRule {
                    id: Uuid::new_v4(),
                    enabled: true,
                    item_type,
                    match_pattern,
                    replace_with,
                    regex,
                    comment,
                    script_name: String::new(),
                    filter_script: String::new(),
                });
        }
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.match_and_replace_rules = prev;
            return Err(format!("未保存规则：{error}"));
        }
        self.sync_mr_table_selection(cx);
        self.save_workspace(cx);
        cx.notify();
        Ok(())
    }

    pub(in crate::workbench) fn save_http_match_replace_draft(
        &mut self,
        project: ProjectKey,
        original: Option<&MatchAndReplaceRule>,
        rule: MatchAndReplaceRule,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.project != Some(project) || !self.backend_available() || self.transition.is_some() {
            return Err("当前项目尚未就绪或后台不可用；规则未保存。".into());
        }
        let prev = self.workspace.match_and_replace_rules.clone();
        if let Some(orig) = original {
            let Some(existing) = self
                .workspace
                .match_and_replace_rules
                .iter_mut()
                .find(|r| r.id == orig.id && !mio_core::is_advanced_proxy_rule(r))
            else {
                return Err("待修改的规则已不存在。".into());
            };
            *existing = rule;
        } else {
            self.workspace.match_and_replace_rules.push(rule);
        }
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.match_and_replace_rules = prev;
            return Err(format!("未保存规则：{error}"));
        }
        self.extra_tools.mr_editor_open = false;
        self.extra_tools.mr_editing = None;
        self.extra_tools.mr_dialog = None;
        self.extra_tools.mr_error = None;
        self.sync_mr_table_selection(cx);
        self.save_workspace(cx);
        cx.notify();
        Ok(())
    }

    pub(in crate::workbench) fn sync_ws_mr_table_selection(&mut self, cx: &mut Context<Self>) {
        let ws_rules = extract_ws_match_and_replace_rules(&self.workspace.match_and_replace_rules);
        let sel = self
            .extra_tools
            .ws_mr_selected
            .and_then(|id| ws_rules.iter().position(|r| r.id == id));
        if sel.is_none() {
            self.extra_tools.ws_mr_selected = None;
        }
        self.extra_tools.ws_mr_table.update(cx, |table, cx| {
            if let Some(row) = sel {
                table.set_selected_row(row, cx);
            } else {
                table.clear_selection(cx);
            }
            table.refresh(cx);
            cx.notify();
        });
    }

    fn submit_ws_match_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let snapshots = [
            &self.extra_tools.ws_mr_match,
            &self.extra_tools.ws_mr_replace,
            &self.extra_tools.ws_mr_comment,
        ]
        .map(|input| editing::input_snapshot(input, window, cx));
        let [Ok(match_raw), Ok(replace_raw), Ok(comment_raw)] = snapshots else {
            self.extra_tools.ws_mr_error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        };
        if match_raw.is_empty() {
            self.extra_tools.ws_mr_error = Some("请填写 WebSocket 匹配模式（不能为空）。".into());
            cx.notify();
            return;
        }
        let match_pattern = match_raw.to_string();
        let replace_with = replace_raw.to_string();
        let comment = comment_raw.trim().to_owned();
        let direction = self.extra_tools.ws_mr_direction;
        let is_regex = self.extra_tools.ws_mr_regex;
        let editing_id = self.extra_tools.ws_mr_editing;
        let outcome = self.update_proxy_advanced(
            self.project,
            |_, ws, _| {
                if let Some(id) = editing_id
                    && let Some(existing) = ws.iter_mut().find(|r| r.id == id)
                {
                    existing.direction = direction;
                    existing.match_pattern = match_pattern;
                    existing.replace_with = replace_with;
                    existing.is_regex = is_regex;
                    existing.comment = comment;
                } else {
                    ws.push(WsMatchAndReplaceRule {
                        id: Uuid::new_v4(),
                        enabled: true,
                        direction,
                        match_pattern,
                        replace_with,
                        is_regex,
                        comment,
                    });
                }
            },
            cx,
        );
        match outcome {
            Ok(()) => {
                self.extra_tools.ws_mr_editing = None;
                self.extra_tools.ws_mr_editor_open = false;
                self.extra_tools.ws_mr_error = None;
                self.extra_tools.ws_mr_regex = false;
                self.extra_tools.ws_mr_direction = WsReplaceDirection::Both;
                self.extra_tools
                    .ws_mr_direction_select
                    .update(cx, |state, cx| {
                        state.set_selected_index(
                            Some(gpui_kit::component::IndexPath::new(0)),
                            window,
                            cx,
                        );
                    });
                for input in [
                    &self.extra_tools.ws_mr_match,
                    &self.extra_tools.ws_mr_replace,
                    &self.extra_tools.ws_mr_comment,
                ] {
                    input.update(cx, |s, cx| s.set_value("", window, cx));
                }
                self.sync_ws_mr_table_selection(cx);
            }
            Err(err) => self.extra_tools.ws_mr_error = Some(err),
        }
        cx.notify();
    }

    fn load_ws_match_replace_for_edit(
        &mut self,
        rule: &WsMatchAndReplaceRule,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let any_composing = editing::input_snapshot(&self.extra_tools.ws_mr_match, window, cx).is_err()
            || editing::input_snapshot(&self.extra_tools.ws_mr_replace, window, cx).is_err()
            || editing::input_snapshot(&self.extra_tools.ws_mr_comment, window, cx).is_err();
        if any_composing {
            self.extra_tools.ws_mr_error = Some(editing::COMPOSITION_PENDING.into());
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        self.extra_tools.ws_mr_editing = Some(rule.id);
        self.extra_tools.ws_mr_editor_open = true;
        self.extra_tools.ws_mr_error = None;
        self.extra_tools.ws_mr_direction = rule.direction;
        self.extra_tools
            .ws_mr_direction_select
            .update(cx, |state, cx| {
                state.set_selected_index(
                    Some(gpui_kit::component::IndexPath::new(ws_mr_direction_index(
                        rule.direction,
                    ))),
                    window,
                    cx,
                );
            });
        self.extra_tools.ws_mr_regex = rule.is_regex;
        self.extra_tools
            .ws_mr_match
            .update(cx, |s, cx| s.set_value(rule.match_pattern.clone(), window, cx));
        self.extra_tools
            .ws_mr_replace
            .update(cx, |s, cx| s.set_value(rule.replace_with.clone(), window, cx));
        self.extra_tools
            .ws_mr_comment
            .update(cx, |s, cx| s.set_value(rule.comment.clone(), window, cx));
        cx.notify();
    }

    fn toggle_ws_match_replace_rule(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if self
            .update_proxy_advanced(
                self.project,
                |_, ws, _| {
                    if let Some(rule) = ws.iter_mut().find(|r| r.id == id) {
                        rule.enabled = !rule.enabled;
                    }
                },
                cx,
            )
            .is_ok()
        {
            self.sync_ws_mr_table_selection(cx);
        }
    }

    fn remove_ws_match_replace_rule(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if self.extra_tools.ws_mr_editing == Some(id) {
            self.extra_tools.ws_mr_editing = None;
            self.extra_tools.ws_mr_editor_open = false;
            self.extra_tools.ws_mr_error = None;
        }
        if self
            .update_proxy_advanced(self.project, |_, ws, _| ws.retain(|r| r.id != id), cx)
            .is_ok()
        {
            self.sync_ws_mr_table_selection(cx);
        }
    }

    fn move_ws_match_replace_rule(&mut self, id: Uuid, down: bool, cx: &mut Context<Self>) {
        if self
            .update_proxy_advanced(
                self.project,
                |_, ws, _| {
                    let Some(pos) = ws.iter().position(|r| r.id == id) else {
                        return;
                    };
                    let target = if down {
                        pos.checked_add(1).filter(|&p| p < ws.len())
                    } else {
                        pos.checked_sub(1)
                    };
                    if let Some(target) = target {
                        ws.swap(pos, target);
                    }
                },
                cx,
            )
            .is_ok()
        {
            self.sync_ws_mr_table_selection(cx);
        }
    }

    fn set_all_ws_match_replace_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self
            .update_proxy_advanced(
                self.project,
                |_, ws, _| {
                    for rule in ws.iter_mut() {
                        rule.enabled = enabled;
                    }
                },
                cx,
            )
            .is_ok()
        {
            self.sync_ws_mr_table_selection(cx);
        }
    }

    fn clear_ws_match_replace_rules(&mut self, cx: &mut Context<Self>) {
        if self
            .update_proxy_advanced(self.project, |_, ws, _| ws.clear(), cx)
            .is_ok()
        {
            self.extra_tools.ws_mr_editing = None;
            self.extra_tools.ws_mr_selected = None;
            self.extra_tools.ws_mr_editor_open = false;
            self.extra_tools.ws_mr_error = None;
            self.sync_ws_mr_table_selection(cx);
        }
    }

    pub(in crate::workbench) fn render_match_and_replace(
        &self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let page_bg: gpui_kit::Hsla = if dark {
            theme::background()
        } else {
            rgb(0xffffff).into()
        };
        let title_color: gpui_kit::Hsla = if dark {
            theme::foreground()
        } else {
            rgb(0x000000).into()
        };
        let divider_color: gpui_kit::Hsla = if dark {
            theme::border()
        } else {
            rgb(0xe0e0e0).into()
        };
        let table_border: gpui_kit::Hsla = if dark {
            theme::border()
        } else {
            rgb(0xd4d4d4).into()
        };
        let editor_bg: gpui_kit::Hsla = if dark {
            theme::toolbar_surface()
        } else {
            rgb(0xfafafa).into()
        };
        let icon_color: gpui_kit::Hsla = if dark {
            theme::foreground()
        } else {
            rgb(0x1a1a1a).into()
        };
        let gutter_indent = theme::scaled_px(28.);
        let button_col_w = theme::scaled_px(84.).max(px(80.));
        let table_h = theme::scaled_px(150.).max(px(138.));
        let http_table_w = mr_table_width(&WB_MR_COLUMNS);
        let ws_table_w = mr_table_width(&WB_WS_MR_COLUMNS);
        let editable =
            self.project.is_some() && self.transition.is_none() && self.backend_available();
        let project = self.project;

        // Section 1: HTTP match and replace rules
        let http_rules: Vec<MatchAndReplaceRule> = self
            .workspace
            .match_and_replace_rules
            .iter()
            .filter(|r| !mio_core::is_advanced_proxy_rule(r))
            .cloned()
            .collect();
        let http_len = http_rules.len();
        let http_has_disabled = http_rules.iter().any(|r| !r.enabled);
        let http_has_enabled = http_rules.iter().any(|r| r.enabled);
        let selected_id = self.extra_tools.mr_selected;
        let selected = selected_id.and_then(|id| http_rules.iter().position(|r| r.id == id));
        let is_editing = self.extra_tools.mr_editing.is_some();
        let http_editor_open = self.extra_tools.mr_editor_open;
        let http_regex = self.extra_tools.mr_regex;
        let http_error = self.extra_tools.mr_error.clone();
        let in_scope_only = self.workspace.proxy_messages.match_replace_in_scope_only;

        let mut http_actions = div()
            .w(button_col_w)
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(5.));
        for (id, label, tip, available, action) in [
            ("mr-side-add", "添加", "添加新的 HTTP 匹配与替换规则", editable, 0u8),
            (
                "mr-edit",
                "编辑",
                "编辑选中的 HTTP 匹配与替换规则",
                editable && selected.is_some(),
                1u8,
            ),
            (
                "mr-remove",
                "移除",
                "移除选中的 HTTP 匹配与替换规则",
                editable && selected.is_some(),
                2u8,
            ),
            (
                "mr-up",
                "上移",
                "上移选中的规则以提高优先级",
                editable && selected.is_some_and(|i| i > 0),
                3u8,
            ),
            (
                "mr-down",
                "下移",
                "下移选中的规则以降低优先级",
                editable && selected.is_some_and(|i| i + 1 < http_len),
                4u8,
            ),
        ] {
            let rule = selected.and_then(|i| http_rules.get(i)).cloned();
            http_actions = http_actions.child(mr_action_button(id, label, tip, available).on_click(
                cx.listener(move |this, _, window, cx| {
                    if action == 0 {
                        this.extra_tools.mr_editing = None;
                        this.extra_tools.mr_editor_open = true;
                        this.extra_tools.mr_error = None;
                        this.extra_tools.mr_regex = false;
                        this.extra_tools.mr_target = MatchReplaceTarget::RequestHeader;
                        this.extra_tools.mr_target_select.update(cx, |s, cx| {
                            s.set_selected_index(
                                Some(gpui_kit::component::IndexPath::new(0)),
                                window,
                                cx,
                            );
                        });
                        for input in [
                            &this.extra_tools.mr_match,
                            &this.extra_tools.mr_replace,
                            &this.extra_tools.mr_comment,
                        ] {
                            input.update(cx, |s, cx| s.set_value("", window, cx));
                        }
                        this.extra_tools
                            .mr_match
                            .update(cx, |s, cx| s.focus(window, cx));
                        if let Some(project) = this.project {
                            if let Some(prev) = this.extra_tools.mr_dialog.take() {
                                let _ = prev.update(cx, |_, window, _| window.remove_window());
                            }
                            this.extra_tools.mr_dialog = open_http_match_replace_dialog(
                                cx.weak_entity(),
                                window.window_handle(),
                                project,
                                None,
                                "mr",
                                window,
                                cx,
                            )
                            .ok();
                        }
                        cx.notify();
                        return;
                    }
                    let Some(id) = selected_id else {
                        return;
                    };
                    match action {
                        1 => {
                            if let Some(rule) = &rule {
                                this.load_http_match_replace_for_edit(rule, window, cx);
                                if let Some(project) = this.project {
                                    if let Some(prev) = this.extra_tools.mr_dialog.take() {
                                        let _ =
                                            prev.update(cx, |_, window, _| window.remove_window());
                                    }
                                    this.extra_tools.mr_dialog = open_http_match_replace_dialog(
                                        cx.weak_entity(),
                                        window.window_handle(),
                                        project,
                                        Some(rule.clone()),
                                        "mr",
                                        window,
                                        cx,
                                    )
                                    .ok();
                                }
                            }
                        }
                        2 => this.remove_http_match_replace_rule(id, cx),
                        3 => this.move_http_match_replace_rule(id, false, cx),
                        _ => this.move_http_match_replace_rule(id, true, cx),
                    }
                }),
            ));
        }

        let http_owner = cx.weak_entity();
        let http_gear = Button::new("mr-http-settings")
            .ghost()
            .compact()
            .p_0()
            .min_w_0()
            .w(theme::scaled_px(20.))
            .h(theme::scaled_px(20.))
            .tab_stop(false)
            .accessibility_label("HTTP match and replace rules 设置")
            .tooltip("HTTP 匹配与替换规则选项")
            .child(
                gpui_kit::component::Icon::new(IconName::Settings)
                    .size(theme::scaled_px(19.))
                    .text_color(icon_color),
            )
            .dropdown_menu(move |menu, _, _| {
                let scope_owner = http_owner.clone();
                let enable_owner = http_owner.clone();
                let disable_owner = http_owner.clone();
                let clear_owner = http_owner.clone();
                let settings_owner = http_owner.clone();
                menu.item(
                    PopupMenuItem::new("仅应用于范围内的项目")
                        .disabled(!editable)
                        .checked(in_scope_only)
                        .on_click(move |_, _, cx| {
                            let _ = scope_owner.update(cx, |this, cx| {
                                this.set_proxy_message_flag(
                                    project,
                                    |m| m.match_replace_in_scope_only = !in_scope_only,
                                    cx,
                                );
                            });
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("全部启用 HTTP 规则")
                        .disabled(!editable || !http_has_disabled)
                        .on_click(move |_, _, cx| {
                            let _ = enable_owner.update(cx, |this, cx| {
                                this.set_all_http_match_replace_enabled(true, cx);
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("全部禁用 HTTP 规则")
                        .disabled(!editable || !http_has_enabled)
                        .on_click(move |_, _, cx| {
                            let _ = disable_owner.update(cx, |this, cx| {
                                this.set_all_http_match_replace_enabled(false, cx);
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("清空全部 HTTP 规则")
                        .disabled(!editable || http_len == 0)
                        .on_click(move |_, _, cx| {
                            let _ = clear_owner.update(cx, |this, cx| {
                                this.clear_http_match_replace_rules(cx);
                            });
                        }),
                )
                .separator()
                .item(PopupMenuItem::new("打开 Proxy 设置…").on_click(move |_, window, cx| {
                    let _ = settings_owner.update(cx, |this, cx| {
                        this.open_settings_filtered(
                            Some(crate::workbench::settings::SettingsPage::Proxy),
                            None,
                            false,
                            window,
                            cx,
                        );
                    });
                }))
            });

        let http_inline_editor = if http_editor_open {
            let regex_entity = cx.entity();
            Some(
                div()
                    .mt(px(10.))
                    .pl(gutter_indent)
                    .child(
                        div()
                            .id("mr-inline-editor")
                            .test_support()
                            .max_w(button_col_w + px(6.) + http_table_w)
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .p(px(10.))
                            .border_1()
                            .border_color(table_border)
                            .rounded(px(3.))
                            .bg(editor_bg)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .w(theme::scaled_px(72.).max(px(72.)))
                                            .flex_shrink_0()
                                            .child("项目类型:"),
                                    )
                                    .child(
                                        div().w(theme::scaled_px(220.).max(px(200.))).child(
                                            Select::new(&self.extra_tools.mr_target_select)
                                                .id("mr-target-select")
                                                .small()
                                                .w_full()
                                                .disabled(!editable),
                                        ),
                                    )
                                    .child(
                                        mr_checkbox(
                                            "mr-regex-toggle",
                                            "Regex match",
                                            "Regex match",
                                            http_regex,
                                            editable,
                                        )
                                        .on_change(move |state, _, _, cx| {
                                            regex_entity.update(cx, |this, cx| {
                                                this.extra_tools.mr_regex =
                                                    state == CheckboxState::Checked;
                                                cx.notify();
                                            });
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .w(theme::scaled_px(72.).max(px(72.)))
                                            .flex_shrink_0()
                                            .child("匹配内容:"),
                                    )
                                    .child(
                                        div().flex_1().min_w_0().child(
                                            Input::new(&self.extra_tools.mr_match)
                                                .id("mr-match-input")
                                                .aria_label("匹配内容")
                                                .small()
                                                .disabled(!editable),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .w(theme::scaled_px(72.).max(px(72.)))
                                            .flex_shrink_0()
                                            .child("替换为:"),
                                    )
                                    .child(
                                        div().flex_1().min_w_0().child(
                                            Input::new(&self.extra_tools.mr_replace)
                                                .id("mr-replace-input")
                                                .aria_label("替换为")
                                                .small()
                                                .disabled(!editable),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .w(theme::scaled_px(72.).max(px(72.)))
                                            .flex_shrink_0()
                                            .child("备注:"),
                                    )
                                    .child(
                                        div().flex_1().min_w_0().child(
                                            Input::new(&self.extra_tools.mr_comment)
                                                .id("mr-comment-input")
                                                .aria_label("备注")
                                                .small()
                                                .disabled(!editable),
                                        ),
                                    ),
                            )
                            .when_some(http_error, |card, error| {
                                card.child(
                                    div()
                                        .id("mr-inline-error")
                                        .test_support()
                                        .role(gpui_kit::accesskit::Role::Alert)
                                        .aria_label(error.clone())
                                        .text_color(theme::danger_fg())
                                        .child(error),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap(px(8.))
                                    .child(
                                        button("mr-add-button", if is_editing { "确定" } else { "添加" })
                                            .primary()
                                            .tooltip("保存并启用 HTTP 匹配与替换规则")
                                            .disabled(!editable)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.add_match_replace(window, cx)
                                            })),
                                    )
                                    .child(
                                        button("mr-cancel-edit", "取消")
                                            .tooltip("关闭编辑表单并放弃修改")
                                            .disabled(!editable)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                if let Some(dlg) = this.extra_tools.mr_dialog.take()
                                                {
                                                    let _ = dlg.update(cx, |_, window, _| {
                                                        window.remove_window()
                                                    });
                                                }
                                                this.extra_tools.mr_editing = None;
                                                this.extra_tools.mr_editor_open = false;
                                                this.extra_tools.mr_error = None;
                                                for input in [
                                                    &this.extra_tools.mr_match,
                                                    &this.extra_tools.mr_replace,
                                                    &this.extra_tools.mr_comment,
                                                ] {
                                                    input.update(cx, |s, cx| {
                                                        s.set_value("", window, cx)
                                                    });
                                                }
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    ),
            )
        } else {
            None
        };

        let http_scope_entity = cx.entity();
        let http_section = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .pl(px(12.))
            .pr(px(18.))
            .pt(px(14.))
            .pb(px(16.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(mr_help_popover(
                        "mr-http-help-popover",
                        "mr-http-help",
                        "HTTP match and replace rules",
                        &[
                            "按列表顺序自动匹配并替换流经 Proxy 的 HTTP 请求头、请求体、响应头、响应体与参数内容。",
                            "点击左侧“添加”或“编辑”可配置普通文本、正则表达式或 脚本规则。",
                        ],
                    ))
                    .child(
                        div()
                            .id("mr-http-heading")
                            .test_support()
                            .role(gpui_kit::accesskit::Role::Heading)
                            .aria_label("HTTP match and replace rules")
                            .text_size(theme::scaled_px(15.))
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(title_color)
                            .child("HTTP match and replace rules"),
                    ),
            )
            .child(
                div()
                    .mt(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(http_gear)
                    .child(
                        div()
                            .text_size(theme::ui_font_size())
                            .text_color(theme::foreground())
                            .child("使用这些设置自动替换通过 Proxy 的 HTTP 请求和响应中的内容。"),
                    ),
            )
            .child(
                div().mt(px(10.)).pl(gutter_indent).child(
                    mr_checkbox(
                        "proxy-http-mr-in-scope-only",
                        "仅应用于范围内的项目",
                        "Only apply to in-scope items",
                        in_scope_only,
                        editable,
                    )
                    .on_change(move |state, _, _, cx| {
                        http_scope_entity.update(cx, |this, cx| {
                            this.set_proxy_message_flag(
                                project,
                                |m| m.match_replace_in_scope_only = state == CheckboxState::Checked,
                                cx,
                            );
                        });
                    }),
                ),
            )
            .when_some(http_inline_editor, |section, editor| section.child(editor))
            .child(
                div()
                    .id("mr-rules")
                    .mt(px(10.))
                    .pl(gutter_indent)
                    .flex()
                    .items_start()
                    .gap(px(6.))
                    .child(http_actions)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .flex_shrink(1.0)
                            .min_w_0()
                            .child(
                                div()
                                    .w(http_table_w)
                                    .flex_shrink(1.0)
                                    .min_w_0()
                                    .h(table_h)
                                    .border_1()
                                    .border_color(table_border)
                                    .bg(theme::surface())
                                    .overflow_hidden()
                                    .child(
                                        DataTable::new(&self.extra_tools.mr_table)
                                            .with_size(theme::scaled_px(22.).max(px(18.)))
                                            .header_row_height(theme::table_header_height())
                                            .bordered(false)
                                            .stripe(true)
                                            .scroll_bounce(false)
                                            .scrollbar_visible(true, false)
                                            .accessibility_label("HTTP match and replace rules"),
                                    ),
                            )
                            .child(mr_table_right_triangle(dark)),
                    ),
            );

        // Section 2: WebSocket match and replace rules
        let ws_rules = extract_ws_match_and_replace_rules(&self.workspace.match_and_replace_rules);
        let ws_len = ws_rules.len();
        let ws_has_disabled = ws_rules.iter().any(|r| !r.enabled);
        let ws_has_enabled = ws_rules.iter().any(|r| r.enabled);
        let ws_selected_id = self.extra_tools.ws_mr_selected;
        let ws_selected = ws_selected_id.and_then(|id| ws_rules.iter().position(|r| r.id == id));
        let ws_is_editing = self.extra_tools.ws_mr_editing.is_some();
        let ws_editor_open = self.extra_tools.ws_mr_editor_open;
        let ws_regex = self.extra_tools.ws_mr_regex;
        let ws_error = self.extra_tools.ws_mr_error.clone();
        let ws_in_scope_only = self.workspace.proxy_messages.ws_match_replace_in_scope_only;

        let mut ws_actions = div()
            .w(button_col_w)
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(5.));
        for (id, label, tip, available, action) in [
            ("ws-mr-add", "添加", "添加新的 WebSocket 匹配与替换规则", editable, 0u8),
            (
                "ws-mr-edit",
                "编辑",
                "编辑选中的 WebSocket 匹配与替换规则",
                editable && ws_selected.is_some(),
                1u8,
            ),
            (
                "ws-mr-remove",
                "移除",
                "移除选中的 WebSocket 匹配与替换规则",
                editable && ws_selected.is_some(),
                2u8,
            ),
            (
                "ws-mr-up",
                "上移",
                "上移选中的 WebSocket 规则以提高优先级",
                editable && ws_selected.is_some_and(|i| i > 0),
                3u8,
            ),
            (
                "ws-mr-down",
                "下移",
                "下移选中的 WebSocket 规则以降低优先级",
                editable && ws_selected.is_some_and(|i| i + 1 < ws_len),
                4u8,
            ),
        ] {
            let rule = ws_selected.and_then(|i| ws_rules.get(i)).cloned();
            ws_actions = ws_actions.child(mr_action_button(id, label, tip, available).on_click(
                cx.listener(move |this, _, window, cx| {
                    if action == 0 {
                        this.extra_tools.ws_mr_editing = None;
                        this.extra_tools.ws_mr_editor_open = true;
                        this.extra_tools.ws_mr_error = None;
                        this.extra_tools.ws_mr_regex = false;
                        this.extra_tools.ws_mr_direction = WsReplaceDirection::Both;
                        this.extra_tools
                            .ws_mr_direction_select
                            .update(cx, |s, cx| {
                                s.set_selected_index(
                                    Some(gpui_kit::component::IndexPath::new(0)),
                                    window,
                                    cx,
                                );
                            });
                        for input in [
                            &this.extra_tools.ws_mr_match,
                            &this.extra_tools.ws_mr_replace,
                            &this.extra_tools.ws_mr_comment,
                        ] {
                            input.update(cx, |s, cx| s.set_value("", window, cx));
                        }
                        this.extra_tools
                            .ws_mr_match
                            .update(cx, |s, cx| s.focus(window, cx));
                        cx.notify();
                        return;
                    }
                    let Some(id) = ws_selected_id else {
                        return;
                    };
                    match action {
                        1 => {
                            if let Some(rule) = &rule {
                                this.load_ws_match_replace_for_edit(rule, window, cx);
                            }
                        }
                        2 => this.remove_ws_match_replace_rule(id, cx),
                        3 => this.move_ws_match_replace_rule(id, false, cx),
                        _ => this.move_ws_match_replace_rule(id, true, cx),
                    }
                }),
            ));
        }

        let ws_owner = cx.weak_entity();
        let ws_gear = Button::new("mr-ws-settings")
            .ghost()
            .compact()
            .p_0()
            .min_w_0()
            .w(theme::scaled_px(20.))
            .h(theme::scaled_px(20.))
            .tab_stop(false)
            .accessibility_label("WebSocket match and replace rules 设置")
            .tooltip("WebSocket 匹配与替换规则选项")
            .child(
                gpui_kit::component::Icon::new(IconName::Settings)
                    .size(theme::scaled_px(19.))
                    .text_color(icon_color),
            )
            .dropdown_menu(move |menu, _, _| {
                let scope_owner = ws_owner.clone();
                let enable_owner = ws_owner.clone();
                let disable_owner = ws_owner.clone();
                let clear_owner = ws_owner.clone();
                let settings_owner = ws_owner.clone();
                menu.item(
                    PopupMenuItem::new("仅应用于范围内的项目")
                        .disabled(!editable)
                        .checked(ws_in_scope_only)
                        .on_click(move |_, _, cx| {
                            let _ = scope_owner.update(cx, |this, cx| {
                                this.set_proxy_message_flag(
                                    project,
                                    |m| m.ws_match_replace_in_scope_only = !ws_in_scope_only,
                                    cx,
                                );
                            });
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("全部启用 WebSocket 规则")
                        .disabled(!editable || !ws_has_disabled)
                        .on_click(move |_, _, cx| {
                            let _ = enable_owner.update(cx, |this, cx| {
                                this.set_all_ws_match_replace_enabled(true, cx);
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("全部禁用 WebSocket 规则")
                        .disabled(!editable || !ws_has_enabled)
                        .on_click(move |_, _, cx| {
                            let _ = disable_owner.update(cx, |this, cx| {
                                this.set_all_ws_match_replace_enabled(false, cx);
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("清空全部 WebSocket 规则")
                        .disabled(!editable || ws_len == 0)
                        .on_click(move |_, _, cx| {
                            let _ = clear_owner.update(cx, |this, cx| {
                                this.clear_ws_match_replace_rules(cx);
                            });
                        }),
                )
                .separator()
                .item(PopupMenuItem::new("打开 Proxy 设置…").on_click(move |_, window, cx| {
                    let _ = settings_owner.update(cx, |this, cx| {
                        this.open_settings_filtered(
                            Some(crate::workbench::settings::SettingsPage::Proxy),
                            None,
                            false,
                            window,
                            cx,
                        );
                    });
                }))
            });

        let ws_inline_editor = if ws_editor_open {
            let ws_regex_entity = cx.entity();
            Some(
                div()
                    .mt(px(10.))
                    .pl(gutter_indent)
                    .child(
                        div()
                            .id("ws-mr-inline-editor")
                            .test_support()
                            .max_w(button_col_w + px(6.) + ws_table_w)
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .p(px(10.))
                            .border_1()
                            .border_color(table_border)
                            .rounded(px(3.))
                            .bg(editor_bg)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .w(theme::scaled_px(72.).max(px(72.)))
                                            .flex_shrink_0()
                                            .child("消息方向:"),
                                    )
                                    .child(
                                        div().w(theme::scaled_px(220.).max(px(200.))).child(
                                            Select::new(&self.extra_tools.ws_mr_direction_select)
                                                .id("ws-mr-direction-select")
                                                .small()
                                                .w_full()
                                                .disabled(!editable),
                                        ),
                                    )
                                    .child(
                                        mr_checkbox(
                                            "ws-mr-regex-toggle",
                                            "Regex match",
                                            "Regex match",
                                            ws_regex,
                                            editable,
                                        )
                                        .on_change(move |state, _, _, cx| {
                                            ws_regex_entity.update(cx, |this, cx| {
                                                this.extra_tools.ws_mr_regex =
                                                    state == CheckboxState::Checked;
                                                cx.notify();
                                            });
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .w(theme::scaled_px(72.).max(px(72.)))
                                            .flex_shrink_0()
                                            .child("匹配内容:"),
                                    )
                                    .child(
                                        div().flex_1().min_w_0().child(
                                            Input::new(&self.extra_tools.ws_mr_match)
                                                .id("ws-mr-match-input")
                                                .aria_label("WebSocket 匹配模式")
                                                .small()
                                                .disabled(!editable),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .w(theme::scaled_px(72.).max(px(72.)))
                                            .flex_shrink_0()
                                            .child("替换为:"),
                                    )
                                    .child(
                                        div().flex_1().min_w_0().child(
                                            Input::new(&self.extra_tools.ws_mr_replace)
                                                .id("ws-mr-replace-input")
                                                .aria_label("WebSocket 替换内容")
                                                .small()
                                                .disabled(!editable),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .w(theme::scaled_px(72.).max(px(72.)))
                                            .flex_shrink_0()
                                            .child("备注:"),
                                    )
                                    .child(
                                        div().flex_1().min_w_0().child(
                                            Input::new(&self.extra_tools.ws_mr_comment)
                                                .id("ws-mr-comment-input")
                                                .aria_label("WebSocket 规则备注")
                                                .small()
                                                .disabled(!editable),
                                        ),
                                    ),
                            )
                            .when_some(ws_error, |card, error| {
                                card.child(
                                    div()
                                        .id("ws-mr-error")
                                        .test_support()
                                        .role(gpui_kit::accesskit::Role::Alert)
                                        .aria_label(error.clone())
                                        .text_color(theme::danger_fg())
                                        .child(error),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap(px(8.))
                                    .child(
                                        button(
                                            "ws-mr-submit",
                                            if ws_is_editing { "确定" } else { "添加" },
                                        )
                                        .primary()
                                        .tooltip("保存并启用 WebSocket 匹配与替换规则")
                                        .disabled(!editable)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.submit_ws_match_replace(window, cx)
                                        })),
                                    )
                                    .child(
                                        button("ws-mr-cancel", "取消")
                                            .tooltip("关闭 WebSocket 规则编辑表单")
                                            .disabled(!editable)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.extra_tools.ws_mr_editing = None;
                                                this.extra_tools.ws_mr_editor_open = false;
                                                this.extra_tools.ws_mr_error = None;
                                                this.extra_tools.ws_mr_regex = false;
                                                for input in [
                                                    &this.extra_tools.ws_mr_match,
                                                    &this.extra_tools.ws_mr_replace,
                                                    &this.extra_tools.ws_mr_comment,
                                                ] {
                                                    input.update(cx, |s, cx| {
                                                        s.set_value("", window, cx)
                                                    });
                                                }
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    ),
            )
        } else {
            None
        };

        let ws_scope_entity = cx.entity();
        let ws_section = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .pl(px(12.))
            .pr(px(18.))
            .pt(px(14.))
            .pb(px(18.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(mr_help_popover(
                        "mr-ws-help-popover",
                        "mr-ws-help",
                        "WebSocket match and replace rules",
                        &[
                            "按列表顺序自动匹配并替换流经 Proxy 的 WebSocket 文本消息内容（自动跳过二进制帧）。",
                            "支持限定双向、客户端到服务端或服务端到客户端方向，以及正则表达式替换。",
                        ],
                    ))
                    .child(
                        div()
                            .id("mr-ws-heading")
                            .test_support()
                            .role(gpui_kit::accesskit::Role::Heading)
                            .aria_label("WebSocket match and replace rules")
                            .text_size(theme::scaled_px(15.))
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(title_color)
                            .child("WebSocket match and replace rules"),
                    ),
            )
            .child(
                div()
                    .mt(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(ws_gear)
                    .child(
                        div()
                            .text_size(theme::ui_font_size())
                            .text_color(theme::foreground())
                            .child("使用这些设置自动替换通过 Proxy 的 WebSocket 消息中的内容。"),
                    ),
            )
            .child(
                div().mt(px(10.)).pl(gutter_indent).child(
                    mr_checkbox(
                        "proxy-ws-mr-in-scope-only",
                        "仅应用于范围内的项目",
                        "Only apply to in-scope items",
                        ws_in_scope_only,
                        editable,
                    )
                    .on_change(move |state, _, _, cx| {
                        ws_scope_entity.update(cx, |this, cx| {
                            this.set_proxy_message_flag(
                                project,
                                |m| {
                                    m.ws_match_replace_in_scope_only =
                                        state == CheckboxState::Checked
                                },
                                cx,
                            );
                        });
                    }),
                ),
            )
            .when_some(ws_inline_editor, |section, editor| section.child(editor))
            .child(
                div()
                    .id("ws-mr-rules")
                    .mt(px(10.))
                    .pl(gutter_indent)
                    .flex()
                    .items_start()
                    .gap(px(6.))
                    .child(ws_actions)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .flex_shrink(1.0)
                            .min_w_0()
                            .child(
                                div()
                                    .w(ws_table_w)
                                    .flex_shrink(1.0)
                                    .min_w_0()
                                    .h(table_h)
                                    .border_1()
                                    .border_color(table_border)
                                    .bg(theme::surface())
                                    .overflow_hidden()
                                    .child(
                                        DataTable::new(&self.extra_tools.ws_mr_table)
                                            .with_size(theme::scaled_px(22.).max(px(18.)))
                                            .header_row_height(theme::table_header_height())
                                            .bordered(false)
                                            .stripe(true)
                                            .scroll_bounce(false)
                                            .scrollbar_visible(true, false)
                                            .accessibility_label("WebSocket match and replace rules"),
                                    ),
                            )
                            .child(mr_table_right_triangle(dark)),
                    ),
            );

        div()
            .id("mr-scroll")
            .size_full()
            .overflow_y_scroll()
            .bg(page_bg)
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(http_section)
                    .child(div().w_full().h(px(1.)).flex_shrink_0().bg(divider_color))
                    .child(ws_section),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests_match_replace {
    use gpui_kit::TestAppContext;
    use gpui_kit::test::TestWindowExt;

    use super::*;
    use crate::workbench::tests::settings::{acknowledge, saved};
    use crate::workbench::tests::{drain, fixture, tab};

    #[gpui_kit::test]
    fn match_and_replace_two_sections_collapsible_editors_and_ws_rules_work(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx, vec![tab("MatchReplace")], 64);
        drain(&mut f.commands);

        cx.update_window(f.window, |_, window, cx| {
            window.click("tool-proxy", cx);
            window.click("proxy-match-replace", cx);
            window.render_frame(cx);

            assert!(window.find("mr-http-heading").visible());
            assert!(window.find("mr-ws-heading").visible());
            assert!(window.find("proxy-http-mr-in-scope-only").visible());
            assert!(window.find("proxy-ws-mr-in-scope-only").visible());
            assert!(window.find("mr-side-add").visible());
            assert!(window.find("ws-mr-add").visible());
            assert!(window.try_find("mr-target-select").is_none());
            assert!(window.try_find("ws-mr-direction-select").is_none());
            assert_eq!(window.find("mr-edit").disabled(), Some(true));
            assert_eq!(window.find("mr-remove").disabled(), Some(true));
            assert_eq!(window.find("ws-mr-edit").disabled(), Some(true));
            assert_eq!(window.find("ws-mr-remove").disabled(), Some(true));

            window.click("proxy-http-mr-in-scope-only", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let ws = saved(&mut f.commands);
        assert!(ws.proxy_messages.match_replace_in_scope_only);
        acknowledge(&f, ws, cx);

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click("proxy-ws-mr-in-scope-only", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let ws = saved(&mut f.commands);
        assert!(ws.proxy_messages.ws_match_replace_in_scope_only);
        acknowledge(&f, ws, cx);

        // Open HTTP editor via Add, submit inline, verify dialog + inline editor close cleanly
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click("mr-side-add", cx);
            window.render_frame(cx);
            assert!(window.find("mr-target-select").visible());
            window.click("mr-match-input", cx);
            window.input("User-Agent: old", cx);
            window.click("mr-replace-input", cx);
            window.input("User-Agent: mio", cx);
            window.click("mr-comment-input", cx);
            window.input("swap ua", cx);
            window.click("mr-add-button", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let ws = saved(&mut f.commands);
        assert_eq!(ws.match_and_replace_rules.len(), 1);
        assert_eq!(ws.match_and_replace_rules[0].match_pattern, "User-Agent: old");
        assert_eq!(ws.match_and_replace_rules[0].replace_with, "User-Agent: mio");
        assert_eq!(ws.match_and_replace_rules[0].comment, "swap ua");
        acknowledge(&f, ws, cx);
        assert_eq!(cx.windows().len(), 1);

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("mr-target-select").is_none());
            assert!(window.find(("mr-row", 0usize)).visible());
            window.click(("mr-toggle", 0usize), cx);
        })
        .unwrap();
        cx.run_until_parked();
        let ws = saved(&mut f.commands);
        assert!(!ws.match_and_replace_rules[0].enabled);
        acknowledge(&f, ws, cx);

        // Add two WebSocket match-and-replace rules, reorder, toggle, and remove
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click("ws-mr-add", cx);
            window.render_frame(cx);
            assert!(window.find("ws-mr-direction-select").visible());
            window.click("ws-mr-match-input", cx);
            window.input("token=([0-9]+)", cx);
            window.click("ws-mr-replace-input", cx);
            window.input("token=masked", cx);
            window.click("ws-mr-comment-input", cx);
            window.input("mask token", cx);
            window.click("ws-mr-regex-toggle", cx);
            window.click("ws-mr-submit", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let ws = saved(&mut f.commands);
        let ws_rules = extract_ws_match_and_replace_rules(&ws.match_and_replace_rules);
        assert_eq!(ws_rules.len(), 1);
        assert!(ws_rules[0].is_regex);
        assert_eq!(ws_rules[0].comment, "mask token");
        acknowledge(&f, ws, cx);

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("ws-mr-direction-select").is_none());
            window.click("ws-mr-add", cx);
            window.render_frame(cx);
            window.click("ws-mr-match-input", cx);
            window.input("ping", cx);
            window.click("ws-mr-replace-input", cx);
            window.input("pong", cx);
            window.click("ws-mr-submit", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let ws = saved(&mut f.commands);
        assert_eq!(
            extract_ws_match_and_replace_rules(&ws.match_and_replace_rules).len(),
            2
        );
        acknowledge(&f, ws, cx);

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click(("ws-mr-row", 1usize), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("ws-mr-up").disabled(), None);
            window.click("ws-mr-up", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let ws = saved(&mut f.commands);
        let ws_rules = extract_ws_match_and_replace_rules(&ws.match_and_replace_rules);
        assert_eq!(ws_rules[0].match_pattern, "ping");
        acknowledge(&f, ws, cx);

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click(("ws-mr-toggle", 0usize), cx);
        })
        .unwrap();
        cx.run_until_parked();
        let ws = saved(&mut f.commands);
        assert!(!extract_ws_match_and_replace_rules(&ws.match_and_replace_rules)[0].enabled);
        acknowledge(&f, ws, cx);

        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click(("ws-mr-row", 0usize), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click("ws-mr-remove", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let ws = saved(&mut f.commands);
        let ws_rules = extract_ws_match_and_replace_rules(&ws.match_and_replace_rules);
        assert_eq!(ws_rules.len(), 1);
        assert_eq!(ws_rules[0].match_pattern, "token=([0-9]+)");
        acknowledge(&f, ws, cx);
    }
}
