use std::ops::ControlFlow;
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::PathBuf,
};

use gpui_kit::base::TestTextExt;
use gpui_kit::component::{
    Disableable, Selectable, Sizable, ThemeStyled,
    button::Button,
    command::CommandState,
    input::{Editor, Input, InputEvent, InputState, Search},
    table::{DataTable, TableEvent, TableState},
};
use gpui_kit::{
    AnyElement, App, AppContext, Context, Entity, EntityInputHandler, FocusHandle, Focusable,
    IntoElement, PathPromptOptions, PromptLevel, Render, ScrollHandle, SharedString, Subscription,
    Task, TestSupportExt, Window, div, prelude::*, px, rgb,
};
use mio_core::{
    Client, Command, Decision, Draft, Event, FlowId, FlowRecord, HistoryEntryId, HistoryRow,
    HttpResponse, HttpVersion, InterceptStage, Orientation, OriginalMessages, ProjectKey,
    RepeaterRedirectMode, RepeaterSettings, TabId, WorkTab, Workspace, parse_repeater_request,
    parse_request, parse_response, request_to_raw, response_to_raw,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    AddHttpNotes, CancelRequest, ChangeRequestMethod, CloseTab, CopyCurlCommand, CopyPrettified,
    CopyUrl, ExportCa, ImportExtProject, MoveTabDown, MoveTabUp, NewProject, NewTab, NextTab,
    OpenInIntruder, OpenInRepeater, OpenProject, PasteUrlAsRequest, PreviousTab, RestoreTab,
    SaveProject, SaveProjectAs, SelectToolCollaborator, SelectToolComparer, SelectToolDashboard,
    SelectToolDecoder, SelectToolDiscover, SelectToolExtensions, SelectToolIntruder,
    SelectToolLogger, SelectToolOrganiser, SelectToolProxy, SelectToolRepeater, SelectToolSequencer,
    SelectToolTarget, SendRequest, ShowExtReport, ToggleTabOrientation,
    editing::{self, MessageEditor, MessageView, SearchBarAction},
    history::{HTTP_HISTORY_BATCH_SIZE, HistoryDelegate, PAGE_SIZE},
    theme,
};

#[cfg(test)]
use crate::Quit;

mod archive_response;
mod audit_dashboard;
mod ext_import;
mod collaborator;
mod flow_detail;
mod har_export;
mod history_selection;
mod history_transfer;
mod http_filter;
mod http_highlights;
mod http_notes;
mod imported_history;
mod inspector;
mod intruder;
mod organiser;
mod plugins;
mod repeater_history;
mod sequencer;
mod settings;
mod shutdown;
mod startup_wizard;
mod tab_groups;
mod tabs;
mod target;
#[cfg(test)]
pub(crate) mod tests;
mod tools_extra;
mod user_preferences;
mod wb_editors;
mod wb_events;
mod wb_events_history;
mod wb_events_send;
mod wb_events_traffic;
mod wb_history;
mod wb_init;
mod wb_project;
mod wb_proxy;
mod wb_render_chrome;
mod wb_render_history;
mod wb_render_intercept;
mod wb_render_top;
mod wb_repeater;
mod wb_shortcuts;
#[cfg(test)]
pub(crate) use wb_shortcuts::{
    BodyEncodingTarget, change_http_body_encoding_bytes, set_http_request_method_bytes,
    toggle_http_request_method_bytes,
};
mod wb_top_menus;
mod wb_transition;
mod wb_workspace;
mod websocket;
mod websocket_repeater;

pub(in crate::workbench) use tabs::RepeaterCombinedPane;
pub(in crate::workbench) use wb_render_top::{
    FooterDrawerState, FooterLogSeverity,
};
pub(in crate::workbench) use wb_render_chrome::{
    button, flat_button, sidebar_rail, sidebar_rail_tab, sidebar_rail_with_dock, toolbar,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum Tool {
    Dashboard,
    Target,
    Proxy,
    Intruder,
    Repeater,
    Collaborator,
    Sequencer,
    Extensions,
    Decoder,
    Comparer,
    Logger,
    Organiser,
    Discover,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum ProxyPage {
    Intercept,
    History,
    WebSocketHistory,
    MatchAndReplace,
}

pub(in crate::workbench) type HistoryMatches<'a> =
    (&'a [(HistoryEntryId, bool)], &'a [(HistoryEntryId, usize)]);
pub(in crate::workbench) type HistorySelectionPage<'a> = (HistoryMatches<'a>, Vec<HistoryRow>);

pub(in crate::workbench) struct TabEditors {
    pub(in crate::workbench) request: MessageEditor,
    pub(in crate::workbench) response: MessageEditor,
    pub(in crate::workbench) target: Entity<InputState>,
    pub(in crate::workbench) combined_pane: std::cell::Cell<RepeaterCombinedPane>,
    pub(in crate::workbench) dirty: bool,
    pub(in crate::workbench) loaded_revision: u64,
    pub(in crate::workbench) response_revision: Option<u64>,
    pub(in crate::workbench) loaded_flow: Option<Box<FlowRecord>>,
    pub(in crate::workbench) loaded_original: Box<OriginalMessages>,
    pub(in crate::workbench) result: Option<String>,
    pub(in crate::workbench) _subscriptions: Vec<Subscription>,
}

pub(in crate::workbench) struct RepeaterSend {
    pub(in crate::workbench) run_id: FlowId,
    pub(in crate::workbench) draft_revision: u64,
    pub(in crate::workbench) history_cursor: Option<mio_core::RepeaterHistoryCursor>,
    pub(in crate::workbench) cancel: CancellationToken,
}

#[derive(Clone)]
pub(in crate::workbench) struct PendingSend {
    pub(in crate::workbench) draft: Draft,
    pub(in crate::workbench) settings: RepeaterSettings,
    pub(in crate::workbench) group_send: Option<mio_core::RepeaterGroupSendTicket>,
    pub(in crate::workbench) required_workspace_revision: u64,
}

impl PendingSend {
    pub(in crate::workbench) fn acknowledged_by(&self, tab_id: TabId, saved: &Workspace) -> bool {
        saved.revision >= self.required_workspace_revision
            && saved
                .tabs
                .iter()
                .any(|tab| tab.id == tab_id && tab.draft == self.draft)
    }
}

pub(super) enum InterceptTarget {
    Http {
        stage: InterceptStage,
        response_head_only: bool,
        record: Box<FlowRecord>,
    },
    WebSocket {
        url: String,
        record: Box<mio_core::WebSocketMessage>,
    },
}

pub(super) struct PendingIntercept {
    pub(super) target: InterceptTarget,
    pub(in crate::workbench) editor: MessageEditor,
    pub(in crate::workbench) deciding: bool,
}

impl PendingIntercept {
    pub(super) fn id(&self) -> mio_core::Uuid {
        match &self.target {
            InterceptTarget::Http { record, .. } => record.summary.id,
            InterceptTarget::WebSocket { record, .. } => record.id,
        }
    }

    pub(super) fn is_http(&self) -> bool {
        matches!(self.target, InterceptTarget::Http { .. })
    }

    pub(in crate::workbench) fn queue_label(&self) -> String {
        match &self.target {
            InterceptTarget::Http { record, .. } => {
                format!("{} {}", record.request.method, record.request.url)
            }
            InterceptTarget::WebSocket { url, record } => {
                let arrow = if record.direction == mio_core::WebSocketDirection::ClientToServer {
                    "→"
                } else {
                    "←"
                };
                format!("WS {arrow} {url}")
            }
        }
    }

    pub(in crate::workbench) fn header_label(&self) -> String {
        match &self.target {
            InterceptTarget::Http {
                stage,
                response_head_only,
                record,
            } => format!(
                "{} {} · {}",
                record.request.method,
                record.request.url,
                match stage {
                    InterceptStage::Request => "请求",
                    InterceptStage::Response if *response_head_only => {
                        "响应（SSE；仅编辑响应头，正文将流式转发）"
                    }
                    InterceptStage::Response => "响应",
                }
            ),
            InterceptTarget::WebSocket { url, record } => {
                let preposition =
                    if record.direction == mio_core::WebSocketDirection::ClientToServer {
                        "to"
                    } else {
                        "from"
                    };
                let kind = match record.kind {
                    mio_core::WebSocketKind::Text => "文本",
                    mio_core::WebSocketKind::Binary => "二进制",
                    _ => "消息",
                };
                format!("WebSocket 消息（{preposition} {url} · {kind}）")
            }
        }
    }
}

#[derive(Clone)]
pub(in crate::workbench) enum AfterSave {
    NewProject,
    OpenProject(PathBuf),
    Quit,
}

#[derive(PartialEq, Eq)]
pub(in crate::workbench) enum TransitionPhase {
    Confirming,
    Quiescing,
    Ready,
    Saving(PathBuf),
    Switching,
    Resuming,
    ShuttingDown,
}

pub(crate) struct Workbench {
    pub(in crate::workbench) owner_window: gpui_kit::AnyWindowHandle,
    pub(in crate::workbench) startup_wizard: Option<Box<startup_wizard::StartupWizard>>,
    pub(in crate::workbench) intruder_result_close:
        Option<Box<intruder::results_close::ResultClose>>,
    pub(in crate::workbench) commands: mpsc::Sender<Command>,
    pub(in crate::workbench) shutdown: CancellationToken,
    pub(in crate::workbench) ipc_socket_path: Option<PathBuf>,
    pub(in crate::workbench) backend_disconnected: bool,
    pub(crate) project: Option<ProjectKey>,
    pub(in crate::workbench) project_path: Option<PathBuf>,
    pub(in crate::workbench) project_disk_bytes: Option<u64>,
    pub(in crate::workbench) settings_window: Option<gpui_kit::AnyWindowHandle>,
    pub(in crate::workbench) tab_group_dialog: Option<gpui_kit::AnyWindowHandle>,
    pub(in crate::workbench) user_preferences: user_preferences::UserPreferencesUi,
    pub(in crate::workbench) ext_import: Box<ext_import::ExtImport>,
    pub(in crate::workbench) workspace: Box<Workspace>,
    pub(in crate::workbench) workspace_dirty: bool,
    pub(in crate::workbench) pending_workspace: Option<u64>,
    pub(in crate::workbench) pending_proxy_interception: Option<(
        u64,
        mio_core::ProxyInterceptionSettings,
        mio_core::ProxyMessageSettings,
        mio_core::ProxyWebSocketInterceptionSettings,
    )>,
    pub(in crate::workbench) pending_project_save: Option<Option<PathBuf>>,
    pub(in crate::workbench) project_saves_in_flight: usize,
    pub(in crate::workbench) notes_save_wait: Option<bool>,
    pub(in crate::workbench) project_notes_save: Option<Vec<mio_core::WebSocketNotesChange>>,
    pub(in crate::workbench) project_http_notes_save: Option<Vec<mio_core::ExtHttpNotesChange>>,
    pub(in crate::workbench) http_notes: http_notes::HttpNotes,
    pub(in crate::workbench) http_highlights: http_highlights::HttpHighlights,
    pub(in crate::workbench) project_http_highlights_save:
        Option<Vec<mio_core::ExtHttpHighlightChange>>,
    pub(super) inspector: Box<inspector::InspectorPane>,
    pub(in crate::workbench) extra_tools: Box<tools_extra::ExtraTools>,
    pub(in crate::workbench) project_save_status: String,
    pub(in crate::workbench) choosing_save_as: bool,
    pub(in crate::workbench) file_picker: Option<mio_core::Uuid>,
    #[cfg(test)]
    pub(in crate::workbench) save_picker_override:
        Option<tokio::sync::oneshot::Receiver<anyhow::Result<Option<PathBuf>>>>,
    pub(in crate::workbench) saving_project_snapshot: bool,
    pub(in crate::workbench) tool: Tool,
    pub(in crate::workbench) proxy_page: ProxyPage,
    pub(in crate::workbench) proxy_address: Option<SocketAddr>,
    pub(super) proxy_listeners_bound: Vec<(mio_core::Uuid, SocketAddr)>,
    pub(in crate::workbench) observed_server_certs: std::collections::VecDeque<mio_core::ObservedServerCert>,
    pub(in crate::workbench) session_traces: std::collections::VecDeque<mio_core::SessionTraceEntry>,
    pub(in crate::workbench) session_tracer_paused: bool,
    pub(in crate::workbench) last_ca_fingerprint: Option<String>,
    pub(in crate::workbench) mio_ai_context: Option<String>,
    pub(in crate::workbench) mio_ai_panel_open: bool,
    pub(in crate::workbench) csrf_poc_modal: Option<(String, String)>,
    pub(in crate::workbench) repeater_custom_actions_open: bool,
    pub(in crate::workbench) repeater_explanations_open: bool,
    pub(in crate::workbench) annotated_live_flows: HashSet<FlowId>,
    pub(in crate::workbench) live_http_annotations: Vec<mio_core::LiveHttpAnnotationEntry>,
    pub(in crate::workbench) proxy_cancel: Option<CancellationToken>,
    pub(in crate::workbench) proxy_busy: bool,
    pub(in crate::workbench) intercept_requests: bool,
    pub(in crate::workbench) intercept_responses: bool,
    pub(in crate::workbench) listener_input: Entity<InputState>,
    pub(in crate::workbench) history_filter: Entity<InputState>,
    pub(in crate::workbench) http_filter: Box<http_filter::HttpFilterState>,
    pub(crate) history: Entity<TableState<HistoryDelegate>>,
    pub(in crate::workbench) websocket: Box<websocket::WebSocketHistory>,
    pub(in crate::workbench) query_revision: u64,
    pub(in crate::workbench) history_loading: bool,
    pub(in crate::workbench) history_dirty: bool,
    pub(super) pending_history_delete: Option<(ProjectKey, mio_core::Uuid)>,
    pub(in crate::workbench) has_traffic: bool,
    pub(in crate::workbench) selection_revision: u64,
    pub(in crate::workbench) history_detail_revision: u64,
    pub(in crate::workbench) repeater_reads: HashMap<u64, TabId>,
    pub(in crate::workbench) archive_responses: archive_response::ArchiveResponses,
    pub(in crate::workbench) repeater_history: repeater_history::HistoryNavigation,
    pub(in crate::workbench) selected_flow: Option<FlowId>,
    pub(in crate::workbench) selected_history: Option<HistoryEntryId>,
    pub(in crate::workbench) history_transfer: Option<history_transfer::PendingHistoryTransfer>,
    pub(in crate::workbench) imported_history: imported_history::ImportedHistory,
    pub(in crate::workbench) detail: Option<Box<FlowRecord>>,
    pub(in crate::workbench) detail_error: Option<String>,
    pub(in crate::workbench) detail_original: Box<OriginalMessages>,
    pub(in crate::workbench) history_original_view: bool,
    pub(in crate::workbench) history_request_original_view: bool,
    pub(in crate::workbench) history_response_original_view: bool,
    pub(in crate::workbench) message_pair_layouts:
        HashMap<&'static str, mio_core::RepeaterLayoutMode>,
    pub(in crate::workbench) message_pair_combined_panes:
        HashMap<&'static str, tabs::RepeaterCombinedPane>,
    pub(in crate::workbench) history_request: Box<MessageEditor>,
    pub(in crate::workbench) history_response: Box<MessageEditor>,
    pub(in crate::workbench) intercepts: Vec<PendingIntercept>,
    pub(in crate::workbench) selected_intercept: Option<FlowId>,
    pub(in crate::workbench) tab_editors: HashMap<TabId, TabEditors>,
    pub(in crate::workbench) tab_editor_lru: Vec<TabId>,
    pub(in crate::workbench) closed_repeater_order: Vec<(Option<ProjectKey>, TabId, bool)>,
    pub(in crate::workbench) repeater_ui_caches: tabs::RepeaterUiCaches,
    pub(in crate::workbench) sequential_group_queue: Option<wb_repeater::SequentialGroupQueue>,
    pub(in crate::workbench) repeater_target_sync_host: bool,
    pub(in crate::workbench) editor_ui_caches: wb_editors::EditorUiCaches,
    pub(in crate::workbench) collaborator_insert_sub: Option<(u64, Subscription)>,
    pub(in crate::workbench) websocket_repeater: websocket_repeater::WebSocketRepeaterState,
    pub(in crate::workbench) target_pane: Box<target::TargetPane>,
    pub(in crate::workbench) target_view: Entity<target::TargetView>,
    pub(in crate::workbench) sequencer_pane: Box<sequencer::SequencerPane>,
    pub(in crate::workbench) sequencer_view: Entity<sequencer::SequencerView>,
    pub(in crate::workbench) audit_dashboard: Box<audit_dashboard::AuditDashboardPane>,
    pub(in crate::workbench) audit_dashboard_view: Entity<audit_dashboard::AuditDashboardView>,
    pub(in crate::workbench) footer_drawer: FooterDrawerState,
    pub(in crate::workbench) collaborator_pane: Box<collaborator::CollaboratorPane>,
    pub(in crate::workbench) collaborator_view: Entity<collaborator::CollaboratorView>,
    pub(in crate::workbench) command_palette: Entity<CommandState>,
    pub(in crate::workbench) command_palette_open: bool,
    pub(in crate::workbench) global_search: wb_render_chrome::GlobalSearchUi,
    pub(in crate::workbench) intruder: Box<intruder::Intruder>,
    pub(in crate::workbench) organiser: Box<organiser::OrganiserUi>,
    pub(in crate::workbench) extensions: Box<plugins::Extensions>,
    pub(in crate::workbench) sending: HashMap<TabId, RepeaterSend>,
    pub(in crate::workbench) pending_sends: HashMap<TabId, PendingSend>,
    pub(in crate::workbench) close_after_send: HashSet<TabId>,
    pub(in crate::workbench) rename: Option<tabs::Rename>,
    pub(in crate::workbench) tab_width: f32,
    pub(in crate::workbench) tabs_scroll: ScrollHandle,
    pub(in crate::workbench) resize_original: Option<f32>,
    pub(in crate::workbench) drop_target: Option<TabId>,
    pub(in crate::workbench) drop_after: bool,
    pub(in crate::workbench) focus: FocusHandle,
    pub(in crate::workbench) status: String,
    pub(in crate::workbench) error: Option<String>,
    pub(in crate::workbench) storage_error: Option<String>,
    pub(in crate::workbench) pending_records: usize,
    pub(in crate::workbench) after_save: Option<AfterSave>,
    pub(in crate::workbench) transition: Option<TransitionPhase>,
    pub(in crate::workbench) quit_attempt: Option<mio_core::Uuid>,
    pub(in crate::workbench) shutdown_requested: bool,
    pub(in crate::workbench) runtime_result: Option<Result<(), String>>,
    pub(in crate::workbench) quitting: bool,
    pub(in crate::workbench) update_status: mio_core::UpdateStatus,
    pub(in crate::workbench) last_available_update: Option<mio_core::UpdateInfo>,
    #[cfg(not(test))]
    pub(in crate::workbench) startup_update_checked: bool,
    pub(in crate::workbench) _subscriptions: Vec<Subscription>,
    pub(in crate::workbench) _events: Task<()>,
}

impl Workbench {
    pub(in crate::workbench) fn open_mio_ai_with_context(
        &mut self,
        title: impl Into<String>,
        context: String,
        cx: &mut Context<Self>,
    ) {
        let title = title.into();
        let trimmed = context.trim().to_owned();
        if !trimmed.is_empty() {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(trimmed.clone()));
            self.mio_ai_context = Some(trimmed);
        }
        self.mio_ai_panel_open = true;
        self.status = format!("已发送到 MioAT：{title}");
        cx.notify();
    }

    pub(in crate::workbench) fn open_csrf_poc_modal(
        &mut self,
        url: &str,
        raw_request: &[u8],
        cx: &mut Context<Self>,
    ) {
        let html = generate_csrf_poc_html(url, raw_request);
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(html.clone()));
        let title = if url.trim().is_empty() {
            "CSRF PoC 生成器".to_owned()
        } else {
            format!("CSRF PoC - {}", url.trim())
        };
        self.csrf_poc_modal = Some((title, html));
        self.status = "已生成 CSRF PoC 并复制到剪贴板".into();
        cx.notify();
    }
}

pub(in crate::workbench) fn generate_csrf_poc_html(url: &str, raw_request: &[u8]) -> String {
    fn esc_attr(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('\'', "&#39;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }
    fn percent_decode_form_component(s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'+' => {
                    out.push(b' ');
                    i += 1;
                }
                b'%' if i + 2 < bytes.len() => {
                    let hi = (bytes[i + 1] as char).to_digit(16);
                    let lo = (bytes[i + 2] as char).to_digit(16);
                    if let (Some(hi), Some(lo)) = (hi, lo) {
                        out.push(((hi << 4) | lo) as u8);
                        i += 3;
                    } else {
                        out.push(b'%');
                        i += 1;
                    }
                }
                b => {
                    out.push(b);
                    i += 1;
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }
    fn extract_multipart_boundary(raw_ct: &str) -> Option<String> {
        for segment in raw_ct.split(';').skip(1) {
            let trimmed = segment.trim();
            if let Some((key, value)) = trimmed.split_once('=')
                && key.trim().eq_ignore_ascii_case("boundary")
            {
                let v = value.trim();
                let unquoted = v
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                    .unwrap_or(v);
                if !unquoted.is_empty() {
                    return Some(unquoted.to_owned());
                }
            }
        }
        None
    }
    fn extract_disposition_name(header_val: &str) -> Option<String> {
        for segment in header_val.split(';') {
            let trimmed = segment.trim();
            if let Some((key, value)) = trimmed.split_once('=')
                && key.trim().eq_ignore_ascii_case("name")
            {
                let v = value.trim();
                let unquoted = v
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                    .unwrap_or(v);
                return Some(unquoted.to_owned());
            }
        }
        None
    }
    fn parse_multipart_fields(body: &str, boundary: &str) -> Vec<(String, String)> {
        let delimiter = format!("--{boundary}");
        let mut fields = Vec::new();
        for part in body.split(&delimiter).skip(1) {
            if part.starts_with("--") {
                break;
            }
            let part = part
                .strip_prefix("\r\n")
                .or_else(|| part.strip_prefix('\n'))
                .unwrap_or(part);
            let Some((part_head, part_body)) =
                part.split_once("\r\n\r\n").or_else(|| part.split_once("\n\n"))
            else {
                continue;
            };
            let mut field_name = None;
            for line in part_head.lines() {
                if let Some((hk, hv)) = line.split_once(':')
                    && hk.trim().eq_ignore_ascii_case("content-disposition")
                {
                    field_name = extract_disposition_name(hv);
                    break;
                }
            }
            if let Some(name) = field_name {
                let value = part_body
                    .strip_suffix("\r\n")
                    .or_else(|| part_body.strip_suffix('\n'))
                    .unwrap_or(part_body);
                fields.push((name, value.to_owned()));
            }
        }
        fields
    }
    let text = String::from_utf8_lossy(raw_request);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .or_else(|| text.split_once("\n\n"))
        .unwrap_or((text.as_ref(), ""));
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or("POST / HTTP/1.1");
    let mut parts = request_line.split_whitespace();
    let raw_method = parts.next().unwrap_or("POST").to_ascii_uppercase();
    let form_method = if raw_method == "GET" { "GET" } else { "POST" };
    let target_path = parts.next().unwrap_or("/");
    let mut host = String::new();
    let mut raw_content_type = String::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("host") {
                host = v.trim().to_owned();
            } else if k.trim().eq_ignore_ascii_case("content-type") {
                raw_content_type = v.trim().to_owned();
            }
        }
    }
    let content_type = raw_content_type.to_ascii_lowercase();
    let action_url = if !url.trim().is_empty() {
        url.trim().to_owned()
    } else if target_path.starts_with("http://") || target_path.starts_with("https://") {
        target_path.to_owned()
    } else if !host.is_empty() {
        format!("https://{host}{target_path}")
    } else {
        target_path.to_owned()
    };
    let (enctype, inputs_html) = if form_method == "GET" {
        let mut inputs = String::new();
        if let Some((_, query)) = action_url.split_once('?') {
            for pair in query.split('&').filter(|p| !p.is_empty()) {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                let dec_k = percent_decode_form_component(k);
                let dec_v = percent_decode_form_component(v);
                inputs.push_str(&format!(
                    "      <input type=\"hidden\" name=\"{}\" value=\"{}\" />\n",
                    esc_attr(&dec_k),
                    esc_attr(&dec_v)
                ));
            }
        }
        ("application/x-www-form-urlencoded", inputs)
    } else if content_type.contains("application/json") {
        let escaped_body = esc_attr(body.trim());
        (
            "text/plain",
            format!(
                "      <input type=\"hidden\" name=\"{}\" value=\"\" />\n",
                escaped_body
            ),
        )
    } else if content_type.contains("multipart/form-data") {
        let parsed = extract_multipart_boundary(&raw_content_type)
            .map(|boundary| parse_multipart_fields(body, &boundary))
            .unwrap_or_default();
        let inputs = if parsed.is_empty() {
            format!(
                "      <input type=\"hidden\" name=\"payload\" value=\"{}\" />\n",
                esc_attr(body.trim())
            )
        } else {
            let mut buf = String::new();
            for (name, val) in parsed {
                buf.push_str(&format!(
                    "      <input type=\"hidden\" name=\"{}\" value=\"{}\" />\n",
                    esc_attr(&name),
                    esc_attr(&val)
                ));
            }
            buf
        };
        ("multipart/form-data", inputs)
    } else {
        let mut inputs = String::new();
        for pair in body.trim().split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let dec_k = percent_decode_form_component(k);
            let dec_v = percent_decode_form_component(v);
            inputs.push_str(&format!(
                "      <input type=\"hidden\" name=\"{}\" value=\"{}\" />\n",
                esc_attr(&dec_k),
                esc_attr(&dec_v)
            ));
        }
        ("application/x-www-form-urlencoded", inputs)
    };
    let clean_action = if form_method == "GET" {
        action_url
            .split_once('?')
            .map_or(action_url.as_str(), |(base, _)| base)
    } else {
        action_url.as_str()
    };
    format!(
        "<html>\n  <!-- MIO CSRF PoC - generated for {method} {url} -->\n  <body>\n    <form action=\"{action}\" method=\"{form_method}\" enctype=\"{enctype}\">\n{inputs}      <input type=\"submit\" value=\"Submit request\" />\n    </form>\n    <script>\n      history.pushState('', '', '/');\n      document.forms[0].submit();\n    </script>\n  </body>\n</html>\n",
        method = raw_method,
        url = esc_attr(&action_url),
        action = esc_attr(clean_action),
        form_method = form_method,
        enctype = enctype,
        inputs = inputs_html
    )
}

#[cfg(test)]
mod csrf_poc_tests {
    use super::generate_csrf_poc_html;

    #[test]
    fn generates_percent_decoded_get_and_urlencoded_and_multipart_csrf_pocs() {
        let get_req = b"GET /transfer?user%5Bname%5D=alice+smith&amount=100%25 HTTP/1.1\r\nHost: example.invalid\r\n\r\n";
        let get_html = generate_csrf_poc_html("https://example.invalid/transfer?user%5Bname%5D=alice+smith&amount=100%25", get_req);
        assert!(get_html.contains("name=\"user[name]\" value=\"alice smith\""));
        assert!(get_html.contains("name=\"amount\" value=\"100%\""));

        let post_req = b"POST /update HTTP/1.1\r\nHost: example.invalid\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\nemail=a%40example.invalid&note=hello+world%21";
        let post_html = generate_csrf_poc_html("https://example.invalid/update", post_req);
        assert!(post_html.contains("name=\"email\" value=\"a@example.invalid\""));
        assert!(post_html.contains("name=\"note\" value=\"hello world!\""));

        let mp_req = b"POST /profile HTTP/1.1\r\nHost: example.invalid\r\nContent-Type: multipart/form-data; boundary=----MioBoundABC\r\n\r\n------MioBoundABC\r\nContent-Disposition: form-data; name=\"role\"\r\n\r\nadmin\r\n------MioBoundABC\r\nContent-Disposition: form-data; name=\"note\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nline1\r\nline2\r\n------MioBoundABC--\r\n";
        let mp_html = generate_csrf_poc_html("https://example.invalid/profile", mp_req);
        assert!(mp_html.contains("enctype=\"multipart/form-data\""));
        assert!(mp_html.contains("name=\"role\" value=\"admin\""));
        assert!(mp_html.contains("name=\"note\" value=\"line1\r\nline2\""));
    }
}
