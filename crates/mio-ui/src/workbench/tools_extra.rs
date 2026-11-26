use std::{cell::Cell, rc::Rc};

use gpui_kit::WeakEntity;
use gpui_kit::assets::IconName;
use gpui_kit::base::{Checkbox, CheckboxIndicator, CheckboxState};
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::input::EditorState;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::table::{Column, TableDelegate, TableEvent, TableState};
use mio_core::{
    FlowSummary, MatchAndReplaceRule, MatchReplaceTarget, TrafficSource, Uuid,
    WsMatchAndReplaceRule, WsReplaceDirection,
};

use super::*;

mod comparer;
mod decoder;
mod logger;
#[cfg(test)]
mod tests_decoder;
#[cfg(test)]
mod tests_extra;
mod match_replace_actions;
mod match_replace_render;
mod transfer_bytes;
#[cfg(test)]
mod tests_match_replace;

type ComparerSideBlockPair = (Option<std::ops::Range<usize>>, Option<std::ops::Range<usize>>);

pub(super) struct MatchReplacePane {
    pub(super) mr_selected: Option<Uuid>,
    pub(super) mr_dialog: Option<gpui_kit::AnyWindowHandle>,
    pub(super) mr_table: Entity<TableState<WorkbenchMatchReplaceTable>>,
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
    pub(super) logger_query_filter: String,
    pub(super) logger_query_revision: u64,
    pub(super) logger_query_cancel: Option<CancellationToken>,
    pub(super) logger_selected: Option<FlowId>,
    pub(super) logger_pending_read: Option<(ProjectKey, u64, FlowId)>,
    pub(super) logger_loaded_detail: Option<Box<(FlowRecord, OriginalMessages)>>,
    pub(super) logger_request: MessageEditor,
    pub(super) logger_response: MessageEditor,
    pub(in crate::workbench::tools_extra) logger_ui_table:
        std::cell::RefCell<logger::LoggerUiTableState>,
    pub(in crate::workbench::tools_extra) logger_visible_rows_cache:
        std::cell::RefCell<Option<logger::LoggerRowsCacheEntry>>,
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
    "请求头",
    "请求体",
    "响应头",
    "响应体",
    "请求参数名",
    "请求参数值",
    "请求首行",
];

pub(super) fn mr_target_index(target: MatchReplaceTarget) -> usize {
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
        "请求头" | "Request header" => MatchReplaceTarget::RequestHeader,
        "请求体" | "Request body" => MatchReplaceTarget::RequestBody,
        "响应头" | "Response header" => MatchReplaceTarget::ResponseHeader,
        "响应体" | "Response body" => MatchReplaceTarget::ResponseBody,
        "请求参数名" | "Request param name" => MatchReplaceTarget::RequestParamName,
        "请求参数值" | "Request param value" => MatchReplaceTarget::RequestParamValue,
        "请求首行" | "Request first line" => MatchReplaceTarget::RequestFirstLine,
        "请求脚本" | "Request script" => MatchReplaceTarget::RequestScript,
        "响应脚本" | "Response script" => MatchReplaceTarget::ResponseScript,
        _ => unreachable!("validated HTTP rule type"),
    }
}

pub(in crate::workbench) fn mr_target_label(target: MatchReplaceTarget) -> &'static str {
    match target {
        MatchReplaceTarget::RequestHeader => "请求头",
        MatchReplaceTarget::RequestBody => "请求体",
        MatchReplaceTarget::ResponseHeader => "响应头",
        MatchReplaceTarget::ResponseBody => "响应体",
        MatchReplaceTarget::RequestParamName => "请求参数名",
        MatchReplaceTarget::RequestParamValue => "请求参数值",
        MatchReplaceTarget::RequestFirstLine => "请求首行",
        MatchReplaceTarget::RequestScript => "请求脚本",
        MatchReplaceTarget::ResponseScript => "响应脚本",
    }
}

pub(in crate::workbench) fn mr_rule_cell(rule: &MatchAndReplaceRule, column: usize) -> String {
    match column {
        0 => if rule.enabled { "是" } else { "否" }.into(),
        1 => mr_target_label(rule.item_type).into(),
        2 => if rule.item_type.is_script() { rule.script_name.clone() } else { String::new() },
        3 => if rule.item_type.is_script() { String::new() } else { rule.match_pattern.clone() },
        4 => if rule.item_type.is_script() {
            rule.filter_script.lines().next().unwrap_or_default().to_owned()
        } else { rule.replace_with.clone() },
        5 => if rule.item_type.is_script() { "脚本" } else if rule.regex { "正则" } else { "字面" }.into(),
        _ => rule.comment.clone(),
    }
}

pub(super) const WS_MR_DIRECTION_LABELS: [&str; 3] = ["双向", "客户端到服务器", "服务器到客户端"];
pub(super) fn ws_mr_direction_index(direction: WsReplaceDirection) -> usize {
    match direction {
        WsReplaceDirection::Both => 0,
        WsReplaceDirection::ClientToServer => 1,
        WsReplaceDirection::ServerToClient => 2,
    }
}

pub(super) fn ws_mr_direction_from_label(label: &str) -> WsReplaceDirection {
    match label {
        "客户端到服务器" | "Client to server" => WsReplaceDirection::ClientToServer,
        "服务器到客户端" | "Server to client" => WsReplaceDirection::ServerToClient,
        _ => WsReplaceDirection::Both,
    }
}

pub(super) fn ws_mr_direction_label(direction: WsReplaceDirection) -> &'static str {
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

pub(super) fn mr_checkbox(
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

pub(super) fn mr_action_button(
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

pub(super) fn mr_help_popover(
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

pub(super) fn mr_table_width(columns: &[(&str, f32)]) -> gpui_kit::Pixels {
    let scale = theme::ui_scale();
    let inner: f32 = columns.iter().map(|(_, width)| (*width * scale).round()).sum();
    px(inner + 2.)
}

pub(super) fn mr_table_right_triangle(dark: bool) -> impl IntoElement {
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
        let script_select = cx.new(|cx| SelectState::new(vec!["请求脚本", "响应脚本"],
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
        let dialog_handle = window.window_handle();
        let rel_owner = owner.clone();
        let rel_owner_window = owner_window;
        let rel_prefix = prefix;
        cx.on_release(move |_, cx| {
            if rel_prefix == "mr" {
                let owner = rel_owner.clone();
                cx.defer(move |cx| {
                    let _ = rel_owner_window.update(cx, |_, _, cx| {
                        let _ = owner.update(cx, |view, cx| {
                            if view.extra_tools.mr_dialog == Some(dialog_handle) {
                                view.extra_tools.mr_dialog = None;
                                cx.notify();
                            }
                        });
                    });
                });
            }
        })
        .detach();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        window.set_window_title(if original.is_some() { "编辑匹配与替换规则" } else { "添加匹配与替换规则" });
        let mut editor = Self {
            owner, owner_window, project, original,
            match_input: cx.new(|cx| InputState::new(window, cx).default_value(rule.match_pattern.clone())),
            replace_input: cx.new(|cx| InputState::new(window, cx).default_value(rule.replace_with.clone())),
            comment: cx.new(|cx| InputState::new(window, cx).default_value(rule.comment.clone())),
            script_name: cx.new(|cx| InputState::new(window, cx).default_value(rule.script_name.clone()).placeholder("如：签名请求头")),
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
                    view.extra_tools.mr_dialog = None;
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
        let title = if self.original.is_some() { "编辑匹配与替换规则" } else { "添加匹配与替换规则" };
        let mut form = div().flex().flex_col().gap(px(8.)).child(
            div().flex().gap(px(8.))
                .child(button(self.id("mode-settings"), "配置模式").selected(!script_mode).on_click(cx.listener(|this, _, window, cx| this.switch_mode(false, window, cx))))
                .child(button(self.id("mode-script"), "脚本模式 (MioScript)").selected(script_mode).on_click(cx.listener(|this, _, window, cx| this.switch_mode(true, window, cx))))
                .when(script_mode, |row| row.child(button(self.id("load-template"), "载入模板").on_click(cx.listener(|this, _, window, cx| {
                    let template = if this.rule.item_type.is_response() { mio_core::DEFAULT_RESPONSE_SCRIPT_TEMPLATE } else { mio_core::DEFAULT_REQUEST_SCRIPT_TEMPLATE };
                    this.script.update(cx, |state, cx| state.set_value(template, window, cx));
                    this.error = None; cx.notify();
                }))))
        );
        if script_mode {
            form = form
                .child(div().flex().gap(px(8.)).items_center().child("类型").child(Select::new(&self.script_select).id(self.id("script-type-select")).small().w(px(220.))))
                .child(Input::new(&self.script_name).id(self.id("script-name-input")).aria_label("脚本名称").small())
                .child(div().id(self.id("script-editor")).test_support().h(px(140.)).child(Editor::new(&self.script).aria_label("MioScript 脚本").size_full()))
                .child("Rhai：request / response 提供 headers、body；支持 sha256_hex、regex_replace、base64_encode。返回修改后的 request / response。")
                .child(div().id(self.id("compilation-errors")).test_support().role(gpui_kit::accesskit::Role::Status).child(
                    match mio_core::validate_filter_script(self.script.read(cx).value().as_ref()) {
                        Ok(()) => "编译错误：无".to_owned(),
                        Err(error) => format!("编译错误：{error}"),
                    }
                ));
        } else {
            form = form
                .child(div().flex().gap(px(8.)).items_center().child("类型").child(Select::new(&self.target_select).id(self.id("type-select")).small().w(px(240.))))
                .child(Input::new(&self.match_input).id(self.id("match-input")).aria_label("匹配").small())
                .child(Input::new(&self.replace_input).id(self.id("replace-input")).aria_label("替换").small())
                .child(
                    mr_checkbox(self.id("regex"), "正则匹配", "正则匹配", self.rule.regex, true)
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
        form = form.child(Input::new(&self.comment).id(self.id("comment-input")).aria_label("备注").small());
        let test_panel = div().flex().gap(px(10.)).h(px(200.))
            .child(div().flex_1().min_w_0().flex().flex_col()
                .child(div().flex().justify_between().items_center().child(if response { "原始响应" } else { "原始请求" })
                    .child(button(self.id("test-restore"), "恢复默认").on_click(cx.listener(|this, _, window, cx| this.restore_test(window, cx)))))
                .child(div().id(self.id("test-original")).test_support().flex_1().min_h_0().child(Editor::new(&self.test_original).aria_label("原始 HTTP 消息").size_full())))
            .child(div().flex().items_center().child(button(self.id("test-btn"), "测试").on_click(cx.listener(|this, _, window, cx| this.test_rule(window, cx)))))
            .child(div().flex_1().min_w_0().flex().flex_col().child(if response { "自动修改后的响应" } else { "自动修改后的请求" })
                .child(div().id(self.id("test-modified")).test_support().flex_1().min_h_0().child(Editor::new(&self.test_modified).aria_label("自动修改后的 HTTP 消息").readonly(true).size_full())));
        let close_owner = self.owner.clone();
        let close_owner_window = self.owner_window;
        let close_is_wb = self.prefix == "mr";
        let title_bar = crate::chrome::render_title_bar("mio-http-mr-editor", title, crate::chrome::TitleBarControls::DIALOG, move |window, cx| {
            if close_is_wb {
                let owner = close_owner.clone();
                let _ = close_owner_window.update(cx, |_, _, cx| {
                    let _ = owner.update(cx, |view, cx| {
                        view.extra_tools.mr_dialog = None;
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
                    .child(button(self.id("submit"), "确定").primary().on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))))
                    .child(button(self.id("cancel-edit"), "取消").on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))))))
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

pub(super) const WB_MR_COLUMNS: [(&str, f32); 7] = [
    ("启用", 68.),
    ("匹配项", 132.),
    ("名称", 174.),
    ("匹配", 182.),
    ("替换", 224.),
    ("类型", 68.),
    ("备注", 206.),
];

impl WorkbenchMatchReplaceTable {
    fn http_rules(&self, cx: &App) -> Vec<MatchAndReplaceRule> {
        self.owner
            .upgrade()
            .map_or_else(Vec::new, |wb| wb.read(cx).workspace.match_and_replace_rules.clone())
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

pub(super) const WB_WS_MR_COLUMNS: [(&str, f32); 6] = [
    ("启用", 68.),
    ("方向", 132.),
    ("匹配", 174.),
    ("替换", 224.),
    ("类型", 68.),
    ("备注", 212.),
];

impl WorkbenchWsMatchReplaceTable {
    fn ws_rules(&self, cx: &App) -> Vec<WsMatchAndReplaceRule> {
        self.owner.upgrade().map_or_else(Vec::new, |wb| {
            wb.read(cx).workspace.ws_match_and_replace_rules.clone()
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
                .child(if rule.is_regex { "正则" } else { "字面" })
                .into_any_element(),
            _ => div().truncate().child(rule.comment).into_any_element(),
        }
    }
    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        let rules = self.ws_rules(cx);
        let rule = rules.get(row)?;
        Some(match column {
            0 => if rule.enabled { "是" } else { "否" }.into(),
            1 => ws_mr_direction_label(rule.direction).into(),
            2 => rule.match_pattern.clone().into(),
            3 => rule.replace_with.clone().into(),
            4 => if rule.is_regex { "正则" } else { "字面" }.into(),
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
        let owner = cx.weak_entity();
        let mr_table = cx.new(|cx| {
            TableState::new(WorkbenchMatchReplaceTable { owner: owner.clone() }, window, cx)
                .sortable(false).col_selectable(false)
        });
        cx.subscribe_in(&mr_table, window, |this, _, event, window, cx| {
            if let TableEvent::SelectRow(row) | TableEvent::DoubleClickedRow(row) = event {
                let rule = this.workspace.match_and_replace_rules.get(*row).cloned();
                this.extra_tools.mr_selected = rule.as_ref().map(|rule| rule.id);
                if matches!(event, TableEvent::DoubleClickedRow(_))
                    && let Some(rule) = rule
                    && let Some(project) = this.project
                {
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
                let rule = this.workspace.ws_match_and_replace_rules.get(*row).cloned();
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
            mr_selected: None,
            mr_dialog: None,
            mr_table,
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
            logger_query_filter: String::new(),
            logger_query_revision: 0,
            logger_query_cancel: None,
            logger_selected: None,
            logger_pending_read: None,
            logger_loaded_detail: None,
            logger_request: MessageEditor::new(window, cx),
            logger_response: MessageEditor::new(window, cx),
            logger_ui_table: std::cell::RefCell::new(logger::LoggerUiTableState::default()),
            logger_visible_rows_cache: std::cell::RefCell::new(None),
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
        self.extra_tools.mr_selected = None;
        self.extra_tools.mr_table.update(cx, |table, cx| {
            table.clear_selection(cx);
            table.refresh(cx);
        });
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
        self.extra_tools.logger_query_filter.clear();
        self.extra_tools.logger_entries.clear();
        self.extra_tools.logger_source_filter = None;
        self.extra_tools.logger_sort = LoggerSort::default();
        self.extra_tools.logger_filters = Default::default();
        self.extra_tools.logger_selected = None;
        self.extra_tools.logger_pending_read = None;
        self.extra_tools.logger_loaded_detail = None;
        self.clear_logger_selection_state();
        *self.extra_tools.logger_visible_rows_cache.borrow_mut() = None;
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
}
