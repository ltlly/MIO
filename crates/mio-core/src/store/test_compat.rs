//! Test-only compatibility shims forwarding legacy `ProjectStore` method calls
//! to the domain repository accessors. Production code calls the repositories
//! directly.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::Result;
use mio_proxy::StreamedResponse;
use rusqlite::Connection;
use tokio_util::sync::CancellationToken;

use super::{CapturedWebSocket, IntruderMutation, ProjectStore, startup_config::StartupProjectConfig};
use crate::{
    AuditStateSnapshot, Draft, ExtCoverageReport, ExtHttpHighlight, ExtHttpHighlightChange,
    ExtHttpHighlightError, ExtHttpNotes, ExtHttpNotesBatch, ExtHttpNotesBatchError,
    ExtHttpNotesError, ExtImportReport, ExtItem, ExtItemDetail, ExtItemPage, ExtMessageFragment,
    ExtMessagePage, ExtMessageSummary, ExtResponseRef, ExtSourceInfo, ExtWebSocketFragment,
    ExtWebSocketPage, FlowId, FlowProvenance, FlowRecord, FlowSummary, HistoryRow,
    HttpHistoryPending, HttpHistoryQuery, HttpHistoryResult, HttpResponse, IntruderDiscardReceipt,
    IntruderPoolDeleteError, IntruderPoolDeleteReceipt, IntruderPoolUsage, IntruderResourcePool,
    IntruderResourcePools, IntruderResultPage, IntruderResultSummary, IntruderResultViewPage,
    IntruderResultViewQuery, IntruderRetention, IntruderRetentionReceipt, IntruderRunPage,
    IntruderRunState, IntruderRunSummary, IntruderSpec, OriginalMessages, ProjectKey,
    RepeaterHistoryEntryId, RepeaterHistoryPage, SiteMapEntryDetail, SiteMapEntryReceipt,
    SiteMapQuery, SiteMapSnapshot, StartupDefaultConfig, TabId, TargetScope, TrafficSource, Uuid,
    WebSocketHistoryFilter, WebSocketHistoryQuery, WebSocketHistoryResult, WebSocketMessage,
    WebSocketNotes, WebSocketNotesError, WebSocketSummary, WorkTab, Workspace,
};

impl ProjectStore {
    // --- MetadataRepo ---
    pub(crate) fn workspace(&self) -> Result<Workspace> {
        self.metadata().workspace()
    }

    pub(crate) fn save_workspace(&mut self, workspace: &Workspace) -> Result<()> {
        self.metadata_mut().save_workspace(workspace)
    }

    pub(crate) fn apply_startup_options(
        &mut self,
        project_name: Option<String>,
        config_mode: &StartupDefaultConfig,
        loaded_config: Option<&StartupProjectConfig>,
        cancel: &CancellationToken,
    ) -> Result<()> {
        self.metadata_mut()
            .apply_startup_options(project_name, config_mode, loaded_config, cancel)
    }

    // --- FlowsRepo ---
    pub(crate) fn put(&mut self, record: &FlowRecord) -> Result<()> {
        self.flows_mut().put(record)
    }

    pub(crate) fn put_repeater_completion(
        &mut self,
        received: &FlowRecord,
        record: &FlowRecord,
    ) -> Result<()> {
        self.flows_mut().put_repeater_completion(received, record)
    }

    pub(crate) fn put_capture(
        &mut self,
        record: &FlowRecord,
        response_stream: Option<&StreamedResponse>,
    ) -> Result<()> {
        self.flows_mut().put_capture(record, response_stream)
    }

    pub(crate) fn put_captured_flow(
        &mut self,
        record: &FlowRecord,
        response_stream: Option<&StreamedResponse>,
        listener_port: Option<u16>,
    ) -> Result<()> {
        self.flows_mut()
            .put_captured_flow(record, response_stream, listener_port)
    }

    pub(crate) fn put_captured_flow_with_metrics(
        &mut self,
        record: &FlowRecord,
        response_stream: Option<&StreamedResponse>,
        listener_port: Option<u16>,
        metrics: &mio_proxy::CapturedFlowMetrics,
    ) -> Result<()> {
        self.flows_mut()
            .put_captured_flow_with_metrics(record, response_stream, listener_port, metrics)
    }

    pub(crate) fn put_flow_provenance(
        &mut self,
        flow_id: FlowId,
        provenance: &FlowProvenance,
    ) -> Result<()> {
        self.flows_mut().put_provenance(flow_id, provenance)
    }

    pub(crate) fn read(&self, id: FlowId) -> Result<FlowRecord> {
        self.flows().read(id)
    }

    pub(crate) fn read_optional(&self, id: FlowId) -> Result<Option<FlowRecord>> {
        self.flows().read_optional(id)
    }

    pub(crate) fn original(&self, id: FlowId) -> Result<OriginalMessages> {
        self.flows().original(id)
    }

    pub(crate) fn page(
        &self,
        offset: usize,
        limit: usize,
        filter: &str,
    ) -> Result<(usize, Vec<FlowSummary>)> {
        self.flows().page(offset, limit, filter)
    }

    pub(crate) fn history_page(
        &self,
        offset: usize,
        limit: usize,
        filter: &str,
    ) -> Result<(usize, Vec<HistoryRow>)> {
        self.flows().history_page(offset, limit, filter)
    }

    pub(crate) fn logger_history_page(
        &self,
        offset: usize,
        limit: usize,
        filter: &str,
        source: Option<&TrafficSource>,
        cancel: &CancellationToken,
    ) -> Result<(usize, Vec<FlowSummary>)> {
        self.flows()
            .logger_history_page(offset, limit, filter, source, cancel)
    }

    pub(crate) fn delete_live_proxy_flows(&mut self, flow_ids: &[FlowId]) -> Result<Vec<FlowId>> {
        self.flows_mut().delete_live_proxy_flows(flow_ids)
    }

    pub(crate) fn clear_live_proxy_history(&mut self) -> Result<Vec<FlowId>> {
        self.flows_mut().clear_live_proxy_history()
    }

    pub(crate) fn query_http_history(
        &self,
        query: &HttpHistoryQuery,
        cancel: &CancellationToken,
    ) -> Result<HttpHistoryResult> {
        self.flows().query_http_history(query, cancel)
    }

    pub(crate) fn http_history_snapshot(
        path: &Path,
        query: &HttpHistoryQuery,
        pending: &[HttpHistoryPending],
        cancel: &CancellationToken,
    ) -> Result<HttpHistoryResult> {
        super::http_history_snapshot(path, query, pending, cancel)
    }

    // --- AuditRepo ---
    pub(crate) fn read_audit_state(&self) -> Result<AuditStateSnapshot> {
        self.audit().read_state()
    }

    pub(crate) fn write_audit_state(
        &mut self,
        next: &AuditStateSnapshot,
        cancel: &CancellationToken,
    ) -> Result<()> {
        self.audit_mut().write_state(next, cancel)
    }

    pub(crate) fn recover_audit_tasks_on_open(
        &mut self,
        pause_automated_tasks: bool,
        cancel: &CancellationToken,
    ) -> Result<()> {
        self.audit_mut()
            .recover_tasks_on_open(pause_automated_tasks, cancel)
    }

    // --- ExtArchiveRepo ---
    pub(crate) fn begin_source(&mut self, source_id: Uuid, report: &ExtImportReport) -> Result<()> {
        self.ext_archive_mut().begin_source(source_id, report)
    }

    pub(crate) fn append_source_chunk(
        &mut self,
        source_id: Uuid,
        ordinal: u64,
        raw: &[u8],
    ) -> Result<()> {
        self.ext_archive_mut()
            .append_source_chunk(source_id, ordinal, raw)
    }

    pub(crate) fn append_source_chunks(
        &mut self,
        source_id: Uuid,
        first: u64,
        chunks: &[Vec<u8>],
    ) -> Result<()> {
        self.ext_archive_mut()
            .append_source_chunks(source_id, first, chunks)
    }

    pub(crate) fn put_ext_item(&mut self, item: &ExtItem) -> Result<()> {
        self.ext_archive_mut().put_item(item)
    }

    pub(crate) fn put_ext_message(&mut self, message: &ExtMessageSummary) -> Result<()> {
        self.ext_archive_mut().put_message(message)
    }

    pub(crate) fn put_ext_batch(
        &mut self,
        items: &[ExtItem],
        messages: &[ExtMessageSummary],
    ) -> Result<()> {
        self.ext_archive_mut().put_batch(items, messages)
    }

    pub(crate) fn finish_source(
        &mut self,
        source_id: Uuid,
        report: &ExtImportReport,
    ) -> Result<()> {
        self.ext_archive_mut().finish_source(source_id, report)
    }

    pub(crate) fn finish_source_with_repeater_workspace(
        &mut self,
        source_id: Uuid,
        report: &ExtImportReport,
        cancel: &CancellationToken,
    ) -> Result<ExtImportReport> {
        self.ext_archive_mut()
            .finish_source_with_repeater_workspace(source_id, report, cancel)
    }

    pub(crate) fn ext_sources(&self) -> Result<Vec<ExtSourceInfo>> {
        self.ext_archive().sources()
    }

    pub(crate) fn read_direct_ext_source(&self) -> Result<Option<ExtSourceInfo>> {
        self.ext_archive().read_direct_source()
    }

    pub(crate) fn ext_report(&self, source_id: Uuid) -> Result<ExtImportReport> {
        self.ext_archive().report(source_id)
    }

    pub(crate) fn ext_item(&self, id: Uuid) -> Result<ExtItem> {
        self.ext_archive().item(id)
    }

    pub(crate) fn ext_item_page(
        &self,
        source_id: Uuid,
        collection: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<ExtItemPage> {
        self.ext_archive()
            .item_page(source_id, collection, offset, limit)
    }

    pub(crate) fn ext_item_detail(
        &self,
        source_id: Uuid,
        item_id: Uuid,
        message_offset: usize,
        message_limit: usize,
    ) -> Result<ExtItemDetail> {
        self.ext_archive()
            .item_detail(source_id, item_id, message_offset, message_limit)
    }

    pub(crate) fn ext_coverage_report(&self, source_id: Uuid) -> Result<ExtCoverageReport> {
        self.ext_archive().coverage_report(source_id)
    }

    pub(crate) fn ext_message_page(
        &self,
        source_id: Uuid,
        collection: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<ExtMessagePage> {
        self.ext_archive()
            .message_page(source_id, collection, offset, limit)
    }

    pub(crate) fn read_source_range(
        &self,
        source_id: Uuid,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>> {
        self.ext_archive()
            .read_source_range(source_id, offset, length)
    }

    pub(crate) fn ext_message_fragment(
        &self,
        id: Uuid,
        offset: u64,
        length: usize,
    ) -> Result<ExtMessageFragment> {
        self.ext_archive().message_fragment(id, offset, length)
    }

    pub(crate) fn ext_repeater_tab(
        &self,
        source_id: Uuid,
        item_id: Uuid,
        message_id: Uuid,
        workspace: &Workspace,
        cancel: &CancellationToken,
    ) -> Result<WorkTab> {
        self.ext_archive()
            .repeater_tab(source_id, item_id, message_id, workspace, cancel)
    }

    pub(crate) fn ext_response_fragment(
        &self,
        reference: &ExtResponseRef,
        offset: u64,
        length: usize,
    ) -> Result<ExtMessageFragment> {
        self.ext_archive()
            .response_fragment(reference, offset, length)
    }

    pub(crate) fn ext_websocket_page(
        &self,
        source_id: Uuid,
        offset: usize,
        limit: usize,
    ) -> Result<ExtWebSocketPage> {
        self.ext_archive().websocket_page(source_id, offset, limit)
    }

    pub(crate) fn ext_websocket_fragment(
        &self,
        source_id: Uuid,
        item_id: Uuid,
        edited: bool,
        offset: u64,
        length: usize,
    ) -> Result<ExtWebSocketFragment> {
        self.ext_archive()
            .websocket_fragment(source_id, item_id, edited, offset, length)
    }

    pub(crate) fn ext_http_notes(
        &self,
        project: ProjectKey,
        source: Uuid,
        item: Uuid,
    ) -> Result<ExtHttpNotes> {
        self.ext_archive().http_notes(project, source, item)
    }

    pub(crate) fn set_ext_http_notes(
        &mut self,
        project: ProjectKey,
        source: Uuid,
        item: Uuid,
        expected: u64,
        text: &str,
        cancel: &CancellationToken,
    ) -> std::result::Result<ExtHttpNotes, ExtHttpNotesError> {
        self.ext_archive_mut()
            .set_http_notes(project, source, item, expected, text, cancel)
    }

    pub(crate) fn ext_http_notes_batch(
        &self,
        project: ProjectKey,
        ids: &[(Uuid, Uuid)],
        cancel: &CancellationToken,
    ) -> Result<ExtHttpNotesBatch> {
        self.ext_archive().http_notes_batch(project, ids, cancel)
    }

    pub(crate) fn set_ext_http_notes_batch(
        &mut self,
        project: ProjectKey,
        changes: &[(Uuid, Uuid, u64)],
        text: &str,
        cancel: &CancellationToken,
    ) -> std::result::Result<Vec<(Uuid, Uuid, u64)>, ExtHttpNotesBatchError> {
        self.ext_archive_mut()
            .set_http_notes_batch(project, changes, text, cancel)
    }

    pub(crate) fn ext_http_highlights(
        &self,
        project: ProjectKey,
        changes: &[ExtHttpHighlightChange],
    ) -> Result<Vec<(Uuid, Uuid, ExtHttpHighlight)>> {
        self.ext_archive().http_highlights(project, changes)
    }

    pub(crate) fn set_ext_http_highlights(
        &mut self,
        project: ProjectKey,
        changes: &[ExtHttpHighlightChange],
        cancel: &CancellationToken,
    ) -> std::result::Result<Vec<(Uuid, Uuid, ExtHttpHighlight)>, ExtHttpHighlightError> {
        self.ext_archive_mut()
            .set_http_highlights(project, changes, cancel)
    }

    // --- WebSocketRepo ---
    pub(crate) fn put_websocket(&mut self, record: &WebSocketMessage) -> Result<()> {
        self.websocket_mut().put(record)
    }

    pub(crate) fn put_captured_websocket(&mut self, record: &CapturedWebSocket) -> Result<()> {
        self.websocket_mut().put_captured(record)
    }

    pub(crate) fn read_websocket(&self, id: Uuid) -> Result<WebSocketMessage> {
        self.websocket().read(id)
    }

    pub(crate) fn read_websocket_detail(
        &self,
        id: Uuid,
    ) -> Result<(WebSocketMessage, Option<Vec<u8>>)> {
        self.websocket().read_detail(id)
    }

    pub(crate) fn flow_url(&self, id: FlowId) -> Result<String> {
        self.websocket().flow_url(id)
    }

    pub(crate) fn websocket_numbers(&self) -> Result<(u64, u64)> {
        self.websocket().numbers()
    }

    pub(crate) fn websocket_connection(&self, flow: FlowId) -> Result<Option<(u64, Option<u16>)>> {
        self.websocket().connection(flow)
    }

    pub(crate) fn websocket_page(
        &self,
        flow: Option<FlowId>,
        offset: usize,
        limit: usize,
        excluded: &[Uuid],
    ) -> Result<(usize, Vec<WebSocketSummary>)> {
        self.websocket().page(flow, offset, limit, excluded)
    }

    pub(crate) fn websocket_page_with_recovery(
        &self,
        flow: Option<FlowId>,
        offset: usize,
        limit: usize,
        excluded: &[Uuid],
        pending: &[WebSocketSummary],
        history_filter: &WebSocketHistoryFilter,
    ) -> Result<(usize, Vec<WebSocketSummary>)> {
        self.websocket().page_with_recovery(
            flow,
            offset,
            limit,
            excluded,
            pending,
            history_filter,
        )
    }

    pub(crate) fn websocket_query_path(&self) -> PathBuf {
        self.query_path()
    }

    pub(crate) fn websocket_query_snapshot(
        path: &Path,
        query: &WebSocketHistoryQuery,
        pending: &[(WebSocketSummary, CapturedWebSocket)],
        cancel: &CancellationToken,
    ) -> Result<WebSocketHistoryResult> {
        super::websocket_query_snapshot(path, query, pending, cancel)
    }

    pub(crate) fn websocket_notes(&self, project: ProjectKey, id: Uuid) -> Result<WebSocketNotes> {
        self.websocket().notes(project, id)
    }

    pub(crate) fn set_websocket_notes(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        expected: u64,
        text: &str,
        cancel: &CancellationToken,
    ) -> std::result::Result<WebSocketNotes, WebSocketNotesError> {
        self.websocket_mut()
            .set_notes(project, id, expected, text, cancel)
    }

    // --- SiteMapRepo ---
    pub(crate) fn target_site_map_snapshot(
        &self,
        scope: &TargetScope,
        query: &SiteMapQuery,
        pending: &[HttpHistoryPending],
    ) -> Result<SiteMapSnapshot> {
        self.site_map().snapshot(scope, query, pending)
    }

    pub(crate) fn add_site_map_entry(
        &mut self,
        workspace_revision: u64,
        owner: &str,
        row: HistoryRow,
        detail: SiteMapEntryDetail,
        draft: bool,
        cancel: &CancellationToken,
    ) -> Result<SiteMapEntryReceipt> {
        self.site_map_mut()
            .add_entry(workspace_revision, owner, row, detail, draft, cancel)
    }

    pub(crate) fn read_site_map_entry(
        &self,
        entry_id: Uuid,
        revision: u64,
    ) -> Result<SiteMapEntryDetail> {
        self.site_map().read_entry(entry_id, revision)
    }

    // --- RepeaterHistoryRepo ---
    pub(crate) fn begin_repeater_send(
        &mut self,
        tab_id: TabId,
        draft: &Draft,
        pending: &FlowRecord,
        cancel: &CancellationToken,
    ) -> Result<()> {
        self.repeater_history_mut()
            .begin_send(tab_id, draft, pending, cancel)
    }

    pub(crate) fn repeater_history_page(
        &self,
        tab_id: TabId,
        start: u64,
        limit: usize,
    ) -> Result<RepeaterHistoryPage> {
        self.repeater_history().page(tab_id, start, limit)
    }

    pub(crate) fn load_repeater_history(
        &self,
        tab_id: TabId,
        entry: RepeaterHistoryEntryId,
        workspace_revision: u64,
        draft_revision: u64,
        cancel: &CancellationToken,
    ) -> Result<WorkTab> {
        self.repeater_history().load(
            tab_id,
            entry,
            workspace_revision,
            draft_revision,
            cancel,
        )
    }

    // --- PluginStorageRepo ---
    pub(crate) fn plugin_storage_get(
        &self,
        project: ProjectKey,
        plugin_id: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>> {
        self.plugin_storage().get(project, plugin_id, key)
    }

    pub(crate) fn plugin_storage_set(
        &mut self,
        project: ProjectKey,
        plugin_id: &str,
        key: &str,
        value: &[u8],
        cancel: &CancellationToken,
    ) -> Result<()> {
        self.plugin_storage_mut()
            .set(project, plugin_id, key, value, cancel)
    }

    // --- ResourcePoolsRepo ---
    pub(crate) fn intruder_resource_pools(&self) -> Result<IntruderResourcePools> {
        self.resource_pools().list()
    }

    pub(crate) fn ensure_intruder_default_pool(
        &mut self,
        project: ProjectKey,
        cancel: &CancellationToken,
    ) -> Result<IntruderResourcePools> {
        self.resource_pools_mut().ensure_default(project, cancel)
    }

    pub(crate) fn set_intruder_resource_pools(
        &mut self,
        project: ProjectKey,
        expected_revision: u64,
        pools: Vec<IntruderResourcePool>,
        cancel: &CancellationToken,
    ) -> Result<IntruderResourcePools> {
        self.resource_pools_mut()
            .set(project, expected_revision, pools, cancel)
    }

    pub(crate) fn intruder_pool_usage(
        &self,
        cancel: &CancellationToken,
    ) -> Result<IntruderPoolUsage> {
        self.resource_pools().usage(cancel)
    }

    pub(crate) fn delete_intruder_resource_pool(
        &mut self,
        project: ProjectKey,
        pool_id: Uuid,
        expected_revisions: (u64, u64),
        allow_reassign: bool,
        has_in_flight: bool,
        cancel: &CancellationToken,
    ) -> std::result::Result<IntruderPoolDeleteReceipt, IntruderPoolDeleteError> {
        self.resource_pools_mut().delete(
            project,
            pool_id,
            expected_revisions,
            allow_reassign,
            has_in_flight,
            cancel,
        )
    }

    // --- IntruderRepo ---
    pub(crate) fn create_intruder_run(
        &mut self,
        id: Uuid,
        tab_id: TabId,
        spec: &IntruderSpec,
        started_unix_ms: i64,
    ) -> Result<IntruderRunSummary> {
        self.intruder_mut()
            .create_run(id, tab_id, spec, started_unix_ms)
    }

    pub(crate) fn create_intruder_run_with_retention(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        tab_id: TabId,
        spec: &IntruderSpec,
        started: i64,
    ) -> Result<IntruderRunSummary> {
        self.intruder_mut()
            .create_run_with_retention(project, id, tab_id, spec, started)
    }

    pub(crate) fn create_intruder_run_with_pool_catalog(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        tab_id: TabId,
        spec: &IntruderSpec,
        started: i64,
        cancel: &CancellationToken,
    ) -> Result<(IntruderRunSummary, IntruderResourcePools)> {
        self.intruder_mut()
            .create_run_with_pool_catalog(project, id, tab_id, spec, started, cancel)
    }

    pub(crate) fn put_intruder_result(
        &mut self,
        id: Uuid,
        ordinal: u64,
        result: std::result::Result<&FlowRecord, &str>,
    ) -> Result<IntruderRunSummary> {
        self.intruder_mut().put_result(id, ordinal, result)
    }

    pub(crate) fn preflight_intruder_result(
        &mut self,
        id: Uuid,
        ordinal: u64,
        record: &FlowRecord,
    ) -> Result<bool> {
        self.intruder_mut().preflight_result(id, ordinal, record)
    }

    pub(crate) fn set_intruder_state(
        &mut self,
        id: Uuid,
        state: IntruderRunState,
    ) -> Result<IntruderRunSummary> {
        self.intruder_mut().set_state(id, state)
    }

    pub(crate) fn set_intruder_result_notes(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        ordinal: u64,
        expected_notes_revision: u64,
        notes: &[u8],
        cancel: &CancellationToken,
    ) -> Result<(u64, Vec<u8>)> {
        self.intruder_mut().set_result_notes(
            project,
            id,
            ordinal,
            expected_notes_revision,
            notes,
            cancel,
        )
    }

    pub(crate) fn set_intruder_result_highlight(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        entries: &[(u64, u64)],
        highlight: Option<u8>,
        cancel: &CancellationToken,
    ) -> Result<Vec<(u64, u64)>> {
        self.intruder_mut()
            .set_result_highlight(project, id, entries, highlight, cancel)
    }

    pub(crate) fn interrupt_intruder_runs(&mut self) -> Result<usize> {
        self.intruder_mut().interrupt_runs()
    }

    pub(crate) fn discard_intruder_run(
        &mut self,
        id: Uuid,
        expected_revision: u64,
        workspace: Workspace,
        cancel: &CancellationToken,
    ) -> Result<IntruderDiscardReceipt> {
        self.intruder_mut()
            .discard_run(id, expected_revision, workspace, cancel)
    }

    pub(crate) fn set_intruder_resource_pool(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        expected_pool_revision: u64,
        pool_id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<IntruderRunSummary> {
        self.intruder_mut().set_resource_pool(
            project,
            id,
            expected_pool_revision,
            pool_id,
            cancel,
        )
    }

    pub(crate) fn request_intruder_baseline(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        expected_revision: u64,
        cancel: &CancellationToken,
    ) -> Result<IntruderRunSummary> {
        self.intruder_mut()
            .request_baseline(project, id, expected_revision, cancel)
    }

    pub(crate) fn request_intruder_baseline_with<T>(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        expected_revision: u64,
        cancel: &CancellationToken,
        prepare: impl FnOnce(IntruderSpec) -> Result<T>,
    ) -> Result<(IntruderRunSummary, T)> {
        self.intruder_mut()
            .request_baseline_with(project, id, expected_revision, cancel, prepare)
    }

    pub(crate) fn rename_intruder_run(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        expected_title_revision: u64,
        title: &str,
        cancel: &CancellationToken,
    ) -> Result<IntruderRunSummary> {
        self.intruder_mut()
            .rename_run(project, id, expected_title_revision, title, cancel)
    }

    pub(crate) fn set_intruder_session_quota(&mut self, bytes: u64) {
        self.intruder_mut().set_session_quota(bytes);
    }

    pub(crate) fn set_intruder_retention(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        expected_revision: u64,
        retention: IntruderRetention,
        workspace: Workspace,
        cancel: &CancellationToken,
    ) -> Result<IntruderRetentionReceipt> {
        self.intruder_mut().set_retention(
            project,
            id,
            expected_revision,
            retention,
            workspace,
            cancel,
        )
    }

    pub(crate) fn read_intruder_run(&self, id: Uuid) -> Result<IntruderRunSummary> {
        self.intruder().read_run(id)
    }

    pub(crate) fn find_intruder_run(&self, id: Uuid) -> Result<Option<IntruderRunSummary>> {
        self.intruder().find_run(id)
    }

    pub(crate) fn intruder_runs(
        &self,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<IntruderRunPage> {
        self.intruder().runs(after, limit)
    }

    pub(crate) fn intruder_results(
        &self,
        id: Uuid,
        after: Option<u64>,
        limit: usize,
    ) -> Result<IntruderResultPage> {
        self.intruder().results(id, after, limit)
    }

    pub(crate) fn read_intruder_spec(&self, id: Uuid) -> Result<IntruderSpec> {
        self.intruder().read_spec(id)
    }

    pub(crate) fn query_intruder_result_view(
        &self,
        query: &IntruderResultViewQuery,
        latest: Option<&IntruderRunSummary>,
        recovery: &[IntruderResultSummary],
        cancel: &CancellationToken,
        shutdown: &CancellationToken,
    ) -> Result<IntruderResultViewPage> {
        self.intruder()
            .query_result_view(query, latest, recovery, cancel, shutdown)
    }

    pub(crate) fn query_intruder_result_view_with_responses(
        &self,
        query: &IntruderResultViewQuery,
        latest: Option<&IntruderRunSummary>,
        recovery: &[IntruderResultSummary],
        recovery_responses: &BTreeMap<u64, &HttpResponse>,
        cancel: &CancellationToken,
        shutdown: &CancellationToken,
    ) -> Result<IntruderResultViewPage> {
        self.intruder().query_result_view_with_responses(
            query,
            latest,
            recovery,
            recovery_responses,
            cancel,
            shutdown,
        )
    }

    pub(crate) fn intruder_payload_summaries(
        &self,
        id: Uuid,
        ordinals: &[u64],
        cancel: &CancellationToken,
    ) -> Result<Vec<super::intruder::result_view::ResultPayloads>> {
        self.intruder().payload_summaries(id, ordinals, cancel)
    }

    pub(crate) fn require_intruder_sessions_resolved(&self) -> Result<()> {
        self.intruder().require_sessions_resolved()
    }

    pub(crate) fn is_session_intruder_run(&self, id: Uuid) -> bool {
        self.intruder().is_session_run(id)
    }

    pub(crate) fn reject_session_intruder_writes(&self, id: Uuid, reject: bool) {
        self.intruder().reject_session_writes(id, reject);
    }

    pub(crate) fn set_session_intruder_write_hook(
        &self,
        id: Uuid,
        hook: Option<super::intruder::SessionWriteHook>,
    ) {
        self.intruder().set_session_write_hook(id, hook);
    }

    pub(crate) fn retry_session_intruder_mutations(
        &mut self,
        mutations: &[IntruderMutation],
        cancel: &CancellationToken,
    ) -> Result<()> {
        self.intruder_mut()
            .retry_session_mutations(mutations, cancel)
    }

    pub(crate) fn read_session_intruder_flow(&self, id: FlowId) -> Result<Option<FlowRecord>> {
        self.intruder().read_session_flow(id)
    }

    pub(crate) fn test_read_session_flow_raw(
        &self,
        run_id: Uuid,
        id: FlowId,
    ) -> Result<Option<FlowRecord>> {
        self.intruder().test_read_session_flow_raw(run_id, id)
    }

    pub(crate) fn test_intruder_connection(&self, id: Uuid) -> Result<&Connection> {
        self.intruder().test_connection(id)
    }

    pub(crate) fn fail_session_pool_delete_commit(&mut self, id: Uuid) {
        self.intruder_mut().fail_session_pool_delete_commit(id);
    }
}
