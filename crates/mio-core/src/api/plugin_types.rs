use std::{path::PathBuf, sync::Arc, time::Instant};

use super::*;

pub type PluginInstanceId = Uuid;

#[derive(Clone, Debug)]
pub struct PluginInspection {
    pub id: Uuid,
    pub directory: PathBuf,
    pub executable: PathBuf,
    pub manifest: crate::plugins::PluginManifest,
}

#[derive(Clone, Debug)]
pub struct LoadedPlugin {
    pub identity: protocol::ChannelIdentity,
    pub directory: PathBuf,
    pub manifest: crate::plugins::PluginManifest,
    pub grants: Vec<protocol::Permission>,
    /// Each message editor owns a separate worker and its own native editor entities.
    pub tab_id: Option<TabId>,
}

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum PluginAction {
    Inspect {
        directory: PathBuf,
    },
    Load {
        inspection_id: Uuid,
        native_trusted: bool,
        grants: Vec<protocol::Permission>,
        replace_instance: Option<Uuid>,
    },
    CreateEditor {
        source_instance: Uuid,
        tab_id: TabId,
        draft_revision: u64,
    },
    Disable {
        instance_id: Uuid,
    },
    Ui {
        instance_id: Uuid,
        panel_revision: u64,
        event: panel::UiEvent,
    },
    OpenRequest {
        instance_id: Uuid,
        tab_id: TabId,
        draft_revision: u64,
    },
    CommitEditor {
        instance_id: Uuid,
        tab_id: TabId,
        draft_revision: u64,
        editor: panel::EditorSnapshot,
    },
    Cancel {
        operation_id: Uuid,
    },
    /// The UI remains the sole workspace writer; the service validates and persists this proposal.
    ApplyWorkspace {
        instance_id: Uuid,
        request_id: u64,
        workspace: Workspace,
        tab_id: TabId,
    },
    RejectWorkspace {
        instance_id: Uuid,
        request_id: u64,
        message: String,
    },
    Reorder {
        instance_id: Uuid,
        move_up: bool,
    },
    OpenContextViewer {
        source_instance: Uuid,
        context_id: Uuid,
        revision: u64,
        read_only: bool,
        request: HttpRequest,
    },
    CommitContextEditor {
        instance_id: Uuid,
        context_id: Uuid,
        revision: u64,
        request: HttpRequest,
        editor: panel::EditorSnapshot,
    },
    OpenResponseViewer {
        source_instance: PluginInstanceId,
        context_id: Uuid,
        revision: u64,
        read_only: bool,
        response: HttpResponse,
    },
    CommitResponseEditor {
        instance_id: PluginInstanceId,
        context_id: Uuid,
        revision: u64,
        response: HttpResponse,
        editor: panel::EditorSnapshot,
    },
    InvokeContextMenu {
        instance_id: PluginInstanceId,
        source: TrafficSource,
        action_id: String,
        selection_start: u64,
        selection_end: u64,
        selected_bytes: Vec<u8>,
        request: HttpRequest,
        response: Option<HttpResponse>,
    },
    Reload {
        instance_id: PluginInstanceId,
    },
    GeneratePayloads {
        instance_id: Uuid,
        tab_id: TabId,
        config: String,
        max_items: u32,
    },
    ProcessPayloads {
        instance_id: Uuid,
        tab_id: TabId,
        position: Option<usize>,
        config: String,
        inputs: Vec<Vec<u8>>,
    },
    SetAutoReload {
        instance_id: Uuid,
        enabled: bool,
    },
}

#[derive(Clone, Debug)]
pub enum PluginOutcome {
    Inspected(PluginInspection),
    Loaded {
        info: LoadedPlugin,
        panel: Arc<panel::PanelSnapshot>,
    },
    Opened {
        identity: protocol::ChannelIdentity,
        tab_id: TabId,
        draft_revision: u64,
    },
    Committed {
        identity: protocol::ChannelIdentity,
        tab_id: TabId,
        draft_revision: u64,
        request: HttpRequest,
    },
    ContextOpened {
        info: LoadedPlugin,
        context_id: Uuid,
        revision: u64,
        panel: Arc<panel::PanelSnapshot>,
    },
    ContextCommitted {
        identity: protocol::ChannelIdentity,
        context_id: Uuid,
        revision: u64,
        request: HttpRequest,
    },
    ResponseOpened {
        info: LoadedPlugin,
        context_id: Uuid,
        revision: u64,
        panel: Arc<panel::PanelSnapshot>,
    },
    ResponseCommitted {
        identity: protocol::ChannelIdentity,
        context_id: Uuid,
        revision: u64,
        response: HttpResponse,
    },
    ContextMenuInvoked {
        identity: protocol::ChannelIdentity,
        status_message: String,
        log_line: String,
        open_repeater: Option<HttpRequest>,
        replace_request: Option<Vec<u8>>,
    },
    Reloaded {
        old_instance: PluginInstanceId,
        info: LoadedPlugin,
        panel: Arc<panel::PanelSnapshot>,
    },
    PayloadsGenerated {
        identity: protocol::ChannelIdentity,
        tab_id: TabId,
        payloads: Vec<Vec<u8>>,
    },
    PayloadsProcessed {
        identity: protocol::ChannelIdentity,
        tab_id: TabId,
        position: Option<usize>,
        payloads: Vec<Vec<u8>>,
    },
    Reordered {
        order: Vec<Uuid>,
    },
    AutoReloadUpdated {
        instance_id: Uuid,
        enabled: bool,
    },
    Disabled {
        identity: protocol::ChannelIdentity,
    },
    EventQueued,
    WorkspaceApplied,
    WorkspaceRejected,
    CancellationRequested,
}

#[derive(Clone, Debug)]
pub enum PluginWorkspaceEdit {
    UpdateDraft {
        tab_id: TabId,
        expected_revision: u64,
        request: HttpRequest,
    },
    OpenRepeater {
        request: HttpRequest,
    },
    OpenIntruder {
        request: HttpRequest,
    },
}

#[derive(Clone, Debug)]
pub struct PluginWorkspaceRequest {
    pub identity: protocol::ChannelIdentity,
    pub request_id: u64,
    pub deadline: Instant,
    pub cancel: tokio_util::sync::CancellationToken,
    pub edit: PluginWorkspaceEdit,
}
