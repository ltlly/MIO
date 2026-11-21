use std::{net::SocketAddr, path::PathBuf};

use super::*;


#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum BrowserRequestSession {
    #[default]
    Default,
    OriginalSession {
        raw_request: Vec<u8>,
    },
    CurrentSession,
}

pub enum Command {
    QueryRepeaterHistory {
        project: ProjectKey,
        query_revision: u64,
        tab_id: TabId,
        start: u64,
        limit: usize,
    },
    SelectRepeaterHistory {
        project: ProjectKey,
        operation_id: Uuid,
        tab_id: TabId,
        entry: RepeaterHistoryEntryId,
        workspace_revision: u64,
        draft_revision: u64,
        cancel: tokio_util::sync::CancellationToken,
    },
    QueryUserPreferences {
        operation_id: Uuid,
    },
    SetUserPreferences {
        operation_id: Uuid,
        expected_revision: u64,
        running: crate::RunningAttackClose,
        finished: crate::FinishedAttackClose,
    },
    SetUserWorkbenchPreferences {
        operation_id: Uuid,
        expected_revision: u64,
        display: crate::UserDisplaySettings,
        message_editor: crate::UserMessageEditorSettings,
        misc: crate::UserMiscSettings,
    },
    SetUserStartupPreferences {
        operation_id: Uuid,
        expected_revision: u64,
        default_config: crate::StartupDefaultConfig,
        disable_extensions: bool,
    },
    RemoveRecentProject {
        operation_id: Uuid,
        expected_revision: u64,
        path: PathBuf,
    },
    PruneMissingRecentProjects {
        operation_id: Uuid,
        expected_revision: u64,
    },
    SetUserTlsCertificates {
        operation_id: Uuid,
        expected_revision: u64,
        certificates: Vec<Vec<u8>>,
    },
    InspectExternal {
        project: ProjectKey,
        job_id: Uuid,
        path: PathBuf,
        cancel: tokio_util::sync::CancellationToken,
    },
    ConvertExternal {
        project: ProjectKey,
        job_id: Uuid,
        inspection_id: Uuid,
        destination: PathBuf,
        cancel: tokio_util::sync::CancellationToken,
    },
    ReleaseExtInspection {
        project: ProjectKey,
        job_id: Uuid,
        inspection_id: Uuid,
    },
    QueryExtReport {
        project: ProjectKey,
        query_revision: u64,
    },
    QueryExtItems {
        project: ProjectKey,
        query_revision: u64,
        source_id: Uuid,
        collection: Option<String>,
        offset: usize,
        limit: usize,
    },
    ReadExtItem {
        project: ProjectKey,
        selection_revision: u64,
        source_id: Uuid,
        item_id: Uuid,
        message_offset: usize,
        message_limit: usize,
    },
    ReadHistoryTransfer {
        project: ProjectKey,
        operation_id: Uuid,
        workspace_revision: u64,
        entries: Vec<HistoryTransferEntry>,
        cancel: tokio_util::sync::CancellationToken,
    },
    OpenExtRepeater {
        project: ProjectKey,
        operation_id: Uuid,
        source_id: Uuid,
        item_id: Uuid,
        message_id: Uuid,
        workspace_revision: u64,
    },
    QueryExtMessages {
        project: ProjectKey,
        query_revision: u64,
        source_id: Uuid,
        collection: Option<String>,
        offset: usize,
        limit: usize,
    },
    ReadExtMessage {
        project: ProjectKey,
        selection_revision: u64,
        message_id: Uuid,
        offset: u64,
        limit: usize,
    },
    ReadExtResponse {
        project: ProjectKey,
        selection_revision: u64,
        reference: ExtResponseRef,
        offset: u64,
        limit: usize,
    },
    Plugin {
        project: ProjectKey,
        operation_id: Uuid,
        action: PluginAction,
    },
    WebSocketRepeater(WebSocketRepeaterCommand),
    Sequencer(SequencerCommand),
    Audit(AuditCommand),
    Collaborator(CollaboratorCommand),
    QuerySiteMap {
        project: ProjectKey,
        query_revision: u64,
        query: SiteMapQuery,
        cancel: tokio_util::sync::CancellationToken,
    },
    AddSiteMapEntry {
        project: ProjectKey,
        operation_id: Uuid,
        workspace_revision: u64,
        entry: SiteMapEntryInput,
        cancel: tokio_util::sync::CancellationToken,
    },
    ReadSiteMapEntry {
        project: ProjectKey,
        selection_revision: u64,
        entry_id: Uuid,
        revision: u64,
        cancel: tokio_util::sync::CancellationToken,
    },
    StartProject {
        path: Option<PathBuf>,
        create_new: bool,
        options: ProjectStartOptions,
    },
    NewProject {
        path: Option<PathBuf>,
    },
    OpenProject {
        path: PathBuf,
    },
    SaveProject {
        project: ProjectKey,
        path: Option<PathBuf>,
        workspace: Option<Workspace>,
        websocket_notes: Vec<WebSocketNotesChange>,
        ext_http_notes: Vec<ExtHttpNotesChange>,
        ext_http_highlights: Vec<ExtHttpHighlightChange>,
    },
    SlimSaveProjectAs {
        project: ProjectKey,
        path: PathBuf,
        options: SlimSaveOptions,
        cancel: tokio_util::sync::CancellationToken,
    },
    Quiesce {
        project: ProjectKey,
    },
    Resume {
        project: ProjectKey,
    },
    StartProxy {
        project: ProjectKey,
        listen: SocketAddr,
    },
    StopProxy {
        project: ProjectKey,
    },
    StopProxyListener {
        project: ProjectKey,
        listener_id: Uuid,
    },
    SetIntercept {
        project: ProjectKey,
        requests: bool,
        responses: bool,
    },
    OpenConfiguredBrowser {
        project: ProjectKey,
        url: Option<String>,
        session: BrowserRequestSession,
    },
    Decide {
        project: ProjectKey,
        flow_id: FlowId,
        revision: u64,
        decision: Decision,
    },
    DecideWebSocket {
        project: ProjectKey,
        message_id: Uuid,
        decision: WebSocketDecision,
    },
    QueryHistory {
        project: ProjectKey,
        query_revision: u64,
        offset: usize,
        limit: usize,
        filter: String,
    },
    QueryLoggerHistory {
        project: ProjectKey,
        query_revision: u64,
        offset: usize,
        limit: usize,
        filter: String,
        source: Option<TrafficSource>,
        cancel: tokio_util::sync::CancellationToken,
    },
    QueryHttpHistory {
        query: HttpHistoryQuery,
    },
    QueryGlobalSearch {
        query: GlobalSearchQuery,
        cancel: tokio_util::sync::CancellationToken,
    },
    DeleteHttpHistory {
        project: ProjectKey,
        operation_id: Uuid,
        flow_ids: Vec<FlowId>,
    },
    ClearLiveProxyHistory {
        project: ProjectKey,
        operation_id: Uuid,
    },
    ReadFlow {
        project: ProjectKey,
        selection_revision: u64,
        flow_id: FlowId,
    },
    QueryWebSocketHistory {
        project: ProjectKey,
        query_revision: u64,
        filter: WebSocketHistoryFilter,
        flow_id: Option<FlowId>,
        offset: usize,
        limit: usize,
        selected_message_id: Option<Uuid>,
    },
    ReadWebSocket {
        project: ProjectKey,
        selection_revision: u64,
        message_id: Uuid,
    },
    ReadWebSocketNotes {
        project: ProjectKey,
        selection_revision: u64,
        message_id: Uuid,
    },
    ReadExtHttpNotes {
        project: ProjectKey,
        selection_revision: u64,
        source_id: Uuid,
        item_id: Uuid,
    },
    SetExtHttpNotes {
        project: ProjectKey,
        operation_id: Uuid,
        change: ExtHttpNotesChange,
        cancel: tokio_util::sync::CancellationToken,
    },
    ReadExtHttpNotesBatch {
        project: ProjectKey,
        operation_id: Uuid,
        ids: Vec<(Uuid, Uuid)>,
        cancel: tokio_util::sync::CancellationToken,
    },
    SetExtHttpNotesBatch {
        project: ProjectKey,
        operation_id: Uuid,
        changes: Vec<(Uuid, Uuid, u64)>,
        text: String,
        cancel: tokio_util::sync::CancellationToken,
    },
    SetExtHttpHighlights {
        project: ProjectKey,
        operation_id: Uuid,
        changes: Vec<ExtHttpHighlightChange>,
        cancel: tokio_util::sync::CancellationToken,
    },
    SetWebSocketNotes {
        project: ProjectKey,
        operation_id: Uuid,
        message_id: Uuid,
        expected_notes_revision: u64,
        text: String,
        cancel: tokio_util::sync::CancellationToken,
    },
    NewRepeater {
        project: ProjectKey,
    },
    OpenRepeater {
        project: ProjectKey,
        flow_id: FlowId,
        original: bool,
    },
    SaveWorkspace {
        project: ProjectKey,
        workspace: Workspace,
    },
    Send {
        project: ProjectKey,
        tab_id: TabId,
        /// A send owns one flow; this ID also identifies its run before admission.
        run_id: FlowId,
        draft: Draft,
        /// Frozen at Send activation, independently of later project defaults.
        settings: RepeaterSettings,
        /// Out-of-band cancellation remains effective before SendStarted is delivered.
        cancel: tokio_util::sync::CancellationToken,
    },
    CancelSend {
        project: ProjectKey,
        tab_id: TabId,
        run_id: FlowId,
        draft_revision: u64,
    },
    AutoIntruderPositions {
        project: ProjectKey,
        operation_id: Uuid,
        raw: Vec<u8>,
        selection: Option<std::ops::Range<usize>>,
        placement: crate::intruder::AutomaticPlacement,
    },
    /// Freezes spec.save_to_project for the initial backing; Session data is never implicitly saved.
    StartIntruder {
        project: ProjectKey,
        run_id: Uuid,
        tab_id: TabId,
        spec: Box<IntruderSpec>,
        /// Out-of-band stop remains effective while UI event delivery is backpressured.
        cancel: tokio_util::sync::CancellationToken,
    },
    StopIntruder {
        project: ProjectKey,
        run_id: Uuid,
    },
    SetIntruderPaused {
        project: ProjectKey,
        run_id: Uuid,
        operation_id: Uuid,
        paused: bool,
    },
    /// Explicitly adds one baseline slot to the same run; existing payloads are not replayed.
    RequestIntruderBaseline {
        project: ProjectKey,
        run_id: Uuid,
        operation_id: Uuid,
        expected_revision: u64,
        /// Cancels admission before commit, independently of the accepted run's lifetime.
        cancel: tokio_util::sync::CancellationToken,
    },
    QueryIntruderResourcePools {
        project: ProjectKey,
        query_revision: u64,
    },
    QueryIntruderPoolUsage {
        project: ProjectKey,
        query_revision: u64,
    },
    DeleteIntruderResourcePool {
        project: ProjectKey,
        operation_id: Uuid,
        pool_id: Uuid,
        expected_catalog_revision: u64,
        expected_workspace_revision: u64,
        allow_reassign: bool,
        cancel: tokio_util::sync::CancellationToken,
    },
    /// Initializes the project's default pool once without changing legacy run bindings.
    EnsureIntruderDefaultPool {
        project: ProjectKey,
        operation_id: Uuid,
        cancel: tokio_util::sync::CancellationToken,
    },
    SetIntruderResourcePools {
        project: ProjectKey,
        operation_id: Uuid,
        expected_revision: u64,
        pools: Vec<IntruderResourcePool>,
        cancel: tokio_util::sync::CancellationToken,
    },
    SetIntruderResourcePool {
        project: ProjectKey,
        run_id: Uuid,
        operation_id: Uuid,
        expected_pool_revision: u64,
        resource_pool: Uuid,
        cancel: tokio_util::sync::CancellationToken,
    },
    RenameIntruder {
        project: ProjectKey,
        run_id: Uuid,
        operation_id: Uuid,
        expected_title_revision: u64,
        /// At most 4096 UTF-8 bytes; empty restores the generated task title.
        title: String,
    },
    /// Session can be saved while Running/Paused; other changes require a terminal, joined run.
    /// Pending recovery or stale project/run revisions always refuse the change.
    /// Repeated or same-retention requests are rejected without replaying workspace edits.
    SetIntruderRetention {
        project: ProjectKey,
        run_id: Uuid,
        operation_id: Uuid,
        expected_revision: u64,
        retention: IntruderRetention,
        workspace: Box<Workspace>,
    },
    /// Terminal Project or Session datasets whose senders have joined can be discarded.
    /// Repeated discard is rejected without reapplying its workspace proposal.
    DiscardIntruder {
        project: ProjectKey,
        run_id: Uuid,
        operation_id: Uuid,
        expected_revision: u64,
        /// The next workspace revision; only last_run references to this run are cleared.
        workspace: Box<Workspace>,
    },
    QueryIntruderResults {
        project: ProjectKey,
        run_id: Uuid,
        query_revision: u64,
        after: Option<u64>,
        limit: usize,
    },
    QueryIntruderResultView {
        query: IntruderResultViewQuery,
        cancel: tokio_util::sync::CancellationToken,
    },
    SetIntruderResultNotes {
        project: ProjectKey,
        run_id: Uuid,
        ordinal: u64,
        expected_notes_revision: u64,
        notes: Vec<u8>,
        operation_id: Uuid,
    },
    SetIntruderResultHighlight {
        project: ProjectKey,
        run_id: Uuid,
        entries: Vec<(u64, u64)>,
        highlight: Option<u8>,
        operation_id: Uuid,
    },
    SetLiveFlowAnnotated {
        project: ProjectKey,
        flow_id: FlowId,
        annotated: bool,
    },
    QueryIntruderRuns {
        project: ProjectKey,
        query_revision: u64,
        after: Option<Uuid>,
        limit: usize,
    },
    ReadIntruderSpec {
        project: ProjectKey,
        run_id: Uuid,
        selection_revision: u64,
    },
    ExportCa {
        path: PathBuf,
    },
    RegenerateCa,
    ImportCa {
        cert_pem: String,
        key_pem: String,
    },
    CheckForUpdates {
        manual: bool,
    },
    InstallUpdate {
        info: crate::UpdateInfo,
    },
}

#[derive(Clone, Debug)]
pub enum Decision {
    ForwardRequest(HttpRequest),
    ForwardResponse(HttpResponse),
    Drop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterceptStage {
    Request,
    Response,
}
