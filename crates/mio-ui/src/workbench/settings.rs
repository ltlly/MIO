use gpui_kit::{AnyWindowHandle, WeakEntity};
use mio_core::{
    ProxyInterceptionSettings, RepeaterRedirectMode, TargetScope, UpstreamProxyProtocol,
    UpstreamProxySettings,
};

use super::*;

mod connections;
mod interception_actions;
mod interception_editor;
mod interception_table;
mod listeners_tls;
mod navigation;
mod project_json;
mod proxy_advanced;
mod proxy_rules;
mod repeater_opts;
mod repeater_render;
pub(super) mod resource_pools;
mod scope;
mod sessions;
mod sessions_misc;
mod tls_certs;
mod user_pages;
mod window_init;
mod window_render;

pub(in crate::workbench) use interception_editor::InterceptionEditor;
use interception_table::{
    InterceptionTable, intercept_match_types, intercept_relation_label, intercept_relations,
    intercept_set, intercept_set_mut, intercept_type_label,
};
use navigation::{
    project_scope_badge, project_scope_badge_with_id, settings_action_button, settings_blue,
    settings_checkbox, settings_checkbox_border, settings_desc_row, settings_desc_row_plain,
    settings_desc_row_with_action, settings_entity, settings_gear_icon_button,
    settings_navigation, settings_pill_button, settings_radio, settings_scrollable_pane,
    settings_section_heading, settings_section_heading_plain, settings_table_checkbox_cell,
    settings_table_frame, settings_table_th,
};
use tls_certs::{CertificateTable, certificate_names, tls_certificate_names};

pub(super) const SNAPSHOT_ERROR: &str =
    "Proxy 消息设置或规则保存回执与实际提交快照不匹配；输入保留，未确认保存。";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum SettingsPage {
    Proxy,
    Repeater,
    Intruder,
    ProjectTasks,
    ProjectScope,
    NetworkConnections,
    NetworkHttp,
    NetworkTls,
    Sessions,
    UserDisplay,
    UserMessageEditor,
    UserMisc,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workbench) enum SettingsScopeFilter {
    #[default]
    All,
    Project,
    User,
}

// Keep Workbench reads outside auxiliary rendering to preserve its GPUI window ownership.
struct SettingsState {
    project: Option<ProjectKey>,
    revision: u64,
    proxy_interception: ProxyInterceptionSettings,
    proxy_messages: mio_core::ProxyMessageSettings,
    proxy_websocket_interception: mio_core::ProxyWebSocketInterceptionSettings,
    update_content_length: bool,
    checked: bool,
    normalize_http1_line_endings: bool,
    strip_connection_headers_over_http2: bool,
    enforce_http1_connection_close: bool,
    network_http: mio_core::NetworkHttpSettings,
    repeater_settings: mio_core::RepeaterSettings,
    tasks_auto_start: bool,
    scheduled_tasks: Vec<mio_core::ScheduledTask>,
    observed_server_certs: Vec<mio_core::ObservedServerCert>,
    session_traces: Vec<mio_core::SessionTraceEntry>,
    session_tracer_paused: bool,
    last_ca_fingerprint: Option<String>,
    follow_redirects: RepeaterRedirectMode,
    request_timeout_secs: u64,
    intercept_requests: bool,
    intercept_responses: bool,
    proxy_listener: String,
    proxy_listeners: Vec<mio_core::ProxyListenerConfig>,
    active_proxy_address: Option<std::net::SocketAddr>,
    active_proxy_listeners: Vec<(mio_core::Uuid, std::net::SocketAddr)>,
    proxy_busy: bool,
    client_certificates: Vec<mio_core::ClientCertificateRule>,
    upstream_proxy: UpstreamProxySettings,
    upstream_tls: mio_core::UpstreamTlsSettings,
    scope: TargetScope,
    sessions: mio_core::SessionSettings,
    enabled: bool,
    error: Option<String>,
    status: Option<&'static str>,
    preferences: user_preferences::UserPreferencesUi,
    preferences_available: bool,
    pools: resource_pools::PoolPageState,
    update_status: mio_core::UpdateStatus,
}

impl SettingsState {
    fn new(view: &Workbench) -> Self {
        Self {
            pools: resource_pools::PoolPageState::new(view),
            update_status: view.update_status.clone(),
            preferences: view.user_preferences.clone(),
            preferences_available: view.backend_available(),
            project: view.project,
            revision: view.workspace.revision,
            proxy_interception: view.workspace.proxy_interception.clone(),
            proxy_messages: view.workspace.proxy_messages,
            proxy_websocket_interception: view.workspace.proxy_websocket_interception,
            update_content_length: view.workspace.repeater_settings.update_content_length,
            checked: view.workspace.repeater_settings.unpack_compressed_responses,
            normalize_http1_line_endings: view
                .workspace
                .repeater_settings
                .normalize_http1_line_endings,
            strip_connection_headers_over_http2: view
                .workspace
                .repeater_settings
                .strip_connection_headers_over_http2,
            enforce_http1_connection_close: view
                .workspace
                .repeater_settings
                .enforce_http1_connection_close,
            network_http: view.workspace.network_http.clone(),
            repeater_settings: view.workspace.repeater_settings.clone(),
            tasks_auto_start: view.workspace.tasks_auto_start,
            scheduled_tasks: view.workspace.scheduled_tasks.clone(),
            observed_server_certs: view.observed_server_certs.clone(),
            session_traces: view.session_traces.clone(),
            session_tracer_paused: view.session_tracer_paused,
            last_ca_fingerprint: view.last_ca_fingerprint.clone(),
            follow_redirects: view.workspace.repeater_settings.follow_redirects,
            request_timeout_secs: view.workspace.repeater_settings.request_timeout_secs,
            intercept_requests: view.intercept_requests,
            intercept_responses: view.intercept_responses,
            proxy_listener: view.workspace.proxy_listener_or_default().to_string(),
            proxy_listeners: view.workspace.effective_proxy_listeners(),
            active_proxy_address: view.proxy_address,
            active_proxy_listeners: view.proxy_listeners_bound.clone(),
            proxy_busy: view.proxy_busy,
            client_certificates: view.workspace.client_certificates.clone(),
            upstream_proxy: view.workspace.upstream_proxy.clone(),
            upstream_tls: view.workspace.upstream_tls.clone(),
            scope: view.workspace.scope.clone(),
            sessions: view.workspace.sessions.clone(),
            enabled: view.project.is_some()
                && view.backend_available()
                && view.transition.is_none(),
            error: view.error.clone().or_else(|| view.storage_error.clone()),
            status: if view.project.is_none() {
                Some("正在打开项目…")
            } else if view.backend_disconnected {
                Some("后台已断开；保存结果未确认，当前设置保留在本窗口。")
            } else if view.workspace_dirty {
                Some("最新修改尚未保存。可通过“项目 > 保存项目”或“另存为…”重试。")
            } else if view.pending_workspace.is_some() {
                Some("修改正在保存，等待项目确认…")
            } else {
                None
            },
        }
    }

    fn refresh(&mut self, view: &Workbench) -> bool {
        self.project = view.project;
        self.revision = view.workspace.revision;
        if self.proxy_interception != view.workspace.proxy_interception {
            self.proxy_interception
                .clone_from(&view.workspace.proxy_interception);
        }
        self.proxy_messages = view.workspace.proxy_messages;
        self.proxy_websocket_interception = view.workspace.proxy_websocket_interception;
        self.update_content_length = view.workspace.repeater_settings.update_content_length;
        self.checked = view.workspace.repeater_settings.unpack_compressed_responses;
        self.normalize_http1_line_endings = view
            .workspace
            .repeater_settings
            .normalize_http1_line_endings;
        self.strip_connection_headers_over_http2 = view
            .workspace
            .repeater_settings
            .strip_connection_headers_over_http2;
        self.enforce_http1_connection_close = view
            .workspace
            .repeater_settings
            .enforce_http1_connection_close;
        self.network_http.clone_from(&view.workspace.network_http);
        self.repeater_settings
            .clone_from(&view.workspace.repeater_settings);
        self.tasks_auto_start = view.workspace.tasks_auto_start;
        if self.scheduled_tasks != view.workspace.scheduled_tasks {
            self.scheduled_tasks
                .clone_from(&view.workspace.scheduled_tasks);
        }
        if self.observed_server_certs.len() != view.observed_server_certs.len()
            || self.observed_server_certs != view.observed_server_certs
        {
            self.observed_server_certs
                .clone_from(&view.observed_server_certs);
        }
        if self.session_traces.len() != view.session_traces.len()
            || self.session_traces.last().map(|e| e.id) != view.session_traces.last().map(|e| e.id)
        {
            self.session_traces.clone_from(&view.session_traces);
        }
        self.session_tracer_paused = view.session_tracer_paused;
        self.last_ca_fingerprint
            .clone_from(&view.last_ca_fingerprint);
        self.follow_redirects = view.workspace.repeater_settings.follow_redirects;
        self.request_timeout_secs = view.workspace.repeater_settings.request_timeout_secs;
        self.intercept_requests = view.intercept_requests;
        self.intercept_responses = view.intercept_responses;
        self.proxy_listener = view.workspace.proxy_listener_or_default().to_string();
        self.proxy_listeners = view.workspace.effective_proxy_listeners();
        self.active_proxy_address = view.proxy_address;
        self.active_proxy_listeners
            .clone_from(&view.proxy_listeners_bound);
        self.proxy_busy = view.proxy_busy;
        if self.client_certificates != view.workspace.client_certificates {
            self.client_certificates
                .clone_from(&view.workspace.client_certificates);
        }
        self.upstream_proxy
            .clone_from(&view.workspace.upstream_proxy);
        if self.upstream_tls != view.workspace.upstream_tls {
            self.upstream_tls.clone_from(&view.workspace.upstream_tls);
        }
        self.scope.clone_from(&view.workspace.scope);
        self.sessions = view.workspace.sessions.clone();
        self.enabled =
            view.project.is_some() && view.backend_available() && view.transition.is_none();
        self.error = view.error.clone().or_else(|| view.storage_error.clone());
        self.status = if view.project.is_none() {
            Some("正在打开项目…")
        } else if view.backend_disconnected {
            Some("后台已断开；保存结果未确认，当前设置保留在本窗口。")
        } else if view.workspace_dirty {
            Some("最新修改尚未保存。可通过“项目 > 保存项目”或“另存为…”重试。")
        } else if view.pending_workspace.is_some() {
            Some("修改正在保存，等待项目确认…")
        } else {
            None
        };
        self.preferences_available = view.backend_available();
        self.pools = resource_pools::PoolPageState::new(view);
        self.update_status.clone_from(&view.update_status);
        self.preferences.sync_from(&view.user_preferences)
    }
}

struct Settings {
    owner: WeakEntity<Workbench>,
    owner_window: AnyWindowHandle,
    state: Box<SettingsState>,
    focus: FocusHandle,
    search_input: Entity<InputState>,
    scope_filter: SettingsScopeFilter,
    collapsed_groups: [bool; 5],
    nav_scroll: gpui_kit::ScrollHandle,
    content_scroll: gpui_kit::ScrollHandle,
    project_json_feedback: Option<(bool, String)>,
    update_content_length_focus: FocusHandle,
    checkbox_focus: FocusHandle,
    normalize_http1_focus: FocusHandle,
    repeater_timeout_input: Entity<InputState>,
    http_streaming_scope_table: Entity<TableState<window_render::HttpStreamingScopeTable>>,
    http_streaming_scope_selected: Option<usize>,
    http_streaming_scope_editing: Option<mio_core::Uuid>,
    http_streaming_scope_editor_open: bool,
    http_streaming_scope_input: Entity<InputState>,
    http_streaming_scope_subdomains: bool,
    scheduled_tasks_table: Entity<TableState<resource_pools::ScheduledTasksTable>>,
    scheduled_tasks_selected: Option<usize>,
    scheduled_task_editing: Option<mio_core::Uuid>,
    scheduled_task_editor_open: bool,
    scheduled_task_action_select:
        Entity<gpui_kit::component::select::SelectState<Vec<&'static str>>>,
    scheduled_task_time: Entity<InputState>,
    scheduled_task_repeat: Entity<InputState>,
    scheduled_task_name: Entity<InputState>,
    scope_editor_open: [bool; 2],
    scope_oos_custom_input: Entity<InputState>,
    scope_oos_custom_editor_open: bool,
    scope_oos_custom_editing: Option<mio_core::Uuid>,
    scope_oos_custom_subdomains: bool,
    proxy_listener_input: Entity<InputState>,
    listeners_tls: Box<listeners_tls::ListenersTlsUi>,
    connections: Box<connections::ConnectionsUi>,
    upstream_protocol: UpstreamProxyProtocol,
    upstream_host: Entity<InputState>,
    upstream_port: Entity<InputState>,
    upstream_username: Entity<InputState>,
    upstream_password: Entity<InputState>,
    tls_names: Vec<String>,
    tls_legacy_names: String,
    tls_selected: Option<Vec<u8>>,
    tls_table: Entity<TableState<CertificateTable>>,
    tls_import: Option<mio_core::Uuid>,
    tls_error: Option<String>,
    scope_include_input: Entity<InputState>,
    scope_exclude_input: Entity<InputState>,
    page: SettingsPage,
    pools: resource_pools::PoolPage,
    sessions: Box<sessions::SessionsUi>,
    interception_tables: [Entity<TableState<InterceptionTable>>; 2],
    interception_selected: [Option<mio_core::Uuid>; 2],
    interception_focus: [FocusHandle; 2],
    proxy_message_focus: [FocusHandle; 2],
    proxy_ws_interception_focus: [FocusHandle; 3],
    proxy_advanced: Box<proxy_advanced::ProxyAdvancedUi>,
    user_pages: Box<user_pages::UserPagesUi>,
    interception_dialog: Option<AnyWindowHandle>,
    interception_opening: bool,
    interception_restore: Option<mio_core::Uuid>,
    interception_error: Option<String>,
    _subscription: Subscription,
}

impl Workbench {
    pub(in crate::workbench) fn update_network_http_settings(
        &mut self,
        project: Option<ProjectKey>,
        mutate: impl FnOnce(&mut mio_core::NetworkHttpSettings),
        cx: &mut Context<Self>,
    ) {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return;
        }
        let mut next = self.workspace.network_http.clone();
        mutate(&mut next);
        if self.workspace.network_http == next {
            return;
        }
        self.workspace.network_http = next;
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn update_repeater_settings(
        &mut self,
        project: Option<ProjectKey>,
        mutate: impl FnOnce(&mut mio_core::RepeaterSettings),
        cx: &mut Context<Self>,
    ) {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return;
        }
        let mut next = self.workspace.repeater_settings.clone();
        mutate(&mut next);
        if self.workspace.repeater_settings == next {
            return;
        }
        self.workspace.repeater_settings = next;
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn update_tasks_settings(
        &mut self,
        project: Option<ProjectKey>,
        tasks_auto_start: Option<bool>,
        scheduled_tasks: Option<Vec<mio_core::ScheduledTask>>,
        cx: &mut Context<Self>,
    ) {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return;
        }
        let mut changed = false;
        if let Some(auto_start) = tasks_auto_start
            && self.workspace.tasks_auto_start != auto_start
        {
            self.workspace.tasks_auto_start = auto_start;
            changed = true;
        }
        if let Some(tasks) = scheduled_tasks
            && self.workspace.scheduled_tasks != tasks
        {
            self.workspace.scheduled_tasks = tasks;
            changed = true;
        }
        if changed {
            self.save_workspace(cx);
            cx.notify();
        }
    }

    pub(in crate::workbench) fn regenerate_ca(&mut self, cx: &mut Context<Self>) {
        if !self.backend_available() {
            return;
        }
        self.enqueue(Command::RegenerateCa, cx);
        self.status = "正在重新生成 CA 证书…".into();
        cx.notify();
    }

    pub(in crate::workbench) fn import_ca(
        &mut self,
        cert_pem: String,
        key_pem: String,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available() {
            return;
        }
        self.enqueue(Command::ImportCa { cert_pem, key_pem }, cx);
        self.status = "正在导入自定义 CA 证书…".into();
        cx.notify();
    }

    pub(in crate::workbench) fn stop_proxy_listener(
        &mut self,
        listener_id: mio_core::Uuid,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else {
            return;
        };
        if !self.backend_available() {
            return;
        }
        self.enqueue(
            Command::StopProxyListener {
                project,
                listener_id,
            },
            cx,
        );
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_session_tracer_paused(&mut self, cx: &mut Context<Self>) {
        self.session_tracer_paused = !self.session_tracer_paused;
        cx.notify();
    }

    pub(in crate::workbench) fn clear_session_traces(&mut self, cx: &mut Context<Self>) {
        self.session_traces.clear();
        cx.notify();
    }
}
