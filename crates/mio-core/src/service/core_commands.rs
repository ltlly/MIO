use super::*;

impl State {
    pub(super) async fn command(&mut self, command: Command) -> Result<()> {
        match command {
            Command::QueryGlobalSearch { query, cancel } => {
                self.query_global_search(query, cancel).await;
            }
            Command::QueryUserPreferences { .. }
            | Command::SetUserPreferences { .. }
            | Command::SetUserWorkbenchPreferences { .. }
            | Command::SetUserStartupPreferences { .. }
            | Command::SetUserTlsCertificates { .. }
            | Command::InspectExternal { .. }
            | Command::ConvertExternal { .. }
            | Command::ReleaseExtInspection { .. }
            | Command::QueryExtReport { .. }
            | Command::QueryExtItems { .. }
            | Command::ReadExtItem { .. }
            | Command::ReadHistoryTransfer { .. }
            | Command::OpenExtRepeater { .. }
            | Command::QueryExtMessages { .. }
            | Command::ReadExtMessage { .. }
            | Command::ReadExtResponse { .. }
            | Command::Plugin { .. }
            | Command::WebSocketRepeater(_)
            | Command::Sequencer(_)
            | Command::Audit(_)
            | Command::Collaborator(_)
            | Command::QuerySiteMap { .. }
            | Command::AddSiteMapEntry { .. }
            | Command::ReadSiteMapEntry { .. } => self.command_prefs_ext_tools(command).await?,
            Command::Quiesce { .. }
            | Command::Resume { .. }
            | Command::StartProject { .. }
            | Command::NewProject { .. }
            | Command::OpenProject { .. }
            | Command::SaveProject { .. }
            | Command::SlimSaveProjectAs { .. }
            | Command::SaveWorkspace { .. }
            | Command::ExportCa { .. }
            | Command::RegenerateCa
            | Command::ImportCa { .. }
            | Command::CheckForUpdates { .. }
            | Command::InstallUpdate { .. } => self.command_project_workspace(command).await?,
            Command::StartProxy { .. }
            | Command::StopProxy { .. }
            | Command::StopProxyListener { .. }
            | Command::SetIntercept { .. }
            | Command::OpenConfiguredBrowser { .. }
            | Command::Decide { .. }
            | Command::DecideWebSocket { .. }
            | Command::SetLiveFlowAnnotated { .. } => self.command_proxy_intercept(command).await?,
            Command::QueryHttpHistory { .. }
            | Command::DeleteHttpHistory { .. }
            | Command::ClearLiveProxyHistory { .. }
            | Command::QueryHistory { .. }
            | Command::QueryLoggerHistory { .. }
            | Command::QueryRepeaterHistory { .. }
            | Command::SelectRepeaterHistory { .. }
            | Command::ReadFlow { .. }
            | Command::QueryWebSocketHistory { .. }
            | Command::ReadWebSocket { .. }
            | Command::ReadWebSocketNotes { .. } => self.command_history_queries(command).await?,
            Command::SetWebSocketNotes { .. }
            | Command::ReadExtHttpNotes { .. }
            | Command::SetExtHttpNotes { .. }
            | Command::ReadExtHttpNotesBatch { .. }
            | Command::SetExtHttpNotesBatch { .. }
            | Command::SetExtHttpHighlights { .. }
            | Command::NewRepeater { .. }
            | Command::OpenRepeater { .. }
            | Command::Send { .. }
            | Command::CancelSend { .. } => self.command_notes_and_repeater(command).await?,
            Command::AutoIntruderPositions { .. }
            | Command::StartIntruder { .. }
            | Command::StopIntruder { .. }
            | Command::SetIntruderPaused { .. }
            | Command::SetIntruderRetention { .. }
            | Command::RequestIntruderBaseline { .. }
            | Command::QueryIntruderResourcePools { .. }
            | Command::QueryIntruderPoolUsage { .. }
            | Command::DeleteIntruderResourcePool { .. }
            | Command::EnsureIntruderDefaultPool { .. }
            | Command::SetIntruderResourcePools { .. }
            | Command::SetIntruderResourcePool { .. }
            | Command::RenameIntruder { .. }
            | Command::DiscardIntruder { .. }
            | Command::QueryIntruderResults { .. }
            | Command::QueryIntruderResultView { .. }
            | Command::SetIntruderResultNotes { .. }
            | Command::SetIntruderResultHighlight { .. }
            | Command::QueryIntruderRuns { .. }
            | Command::ReadIntruderSpec { .. } => self.command_intruder(command).await?,
        }
        Ok(())
    }
}

pub(super) fn command_context(
    command: &Command,
    current: ProjectKey,
) -> (&'static str, Option<ProjectKey>) {
    match command {
        Command::QueryUserPreferences { .. } => ("query_user_preferences", None),
        Command::SetUserPreferences { .. } => ("set_user_preferences", None),
        Command::SetUserWorkbenchPreferences { .. } => ("set_user_workbench_preferences", None),
        Command::SetUserStartupPreferences { .. } => ("set_user_startup_preferences", None),
        Command::SetUserTlsCertificates { .. } => ("set_user_tls_certificates", None),
        Command::InspectExternal { project, .. } => ("inspect_ext", Some(*project)),
        Command::ConvertExternal { project, .. } => ("convert_ext", Some(*project)),
        Command::ReleaseExtInspection { project, .. } => {
            ("release_ext_archive_inspection", Some(*project))
        }
        Command::QueryExtReport { project, .. } => ("query_ext_report", Some(*project)),
        Command::QueryExtItems { project, .. } => ("query_ext_items", Some(*project)),
        Command::ReadExtItem { project, .. } => ("read_ext_item", Some(*project)),
        Command::ReadHistoryTransfer { project, .. } => ("read_history_transfer", Some(*project)),
        Command::OpenExtRepeater { project, .. } => ("open_ext_repeater", Some(*project)),
        Command::QueryExtMessages { project, .. } => ("query_ext_messages", Some(*project)),
        Command::ReadExtMessage { project, .. } => ("read_ext_message", Some(*project)),
        Command::ReadExtResponse { project, .. } => ("read_ext_response", Some(*project)),
        Command::Plugin { project, .. } => ("plugin", Some(*project)),
        Command::WebSocketRepeater(cmd) => ("websocket_repeater", Some(cmd.project())),
        Command::Sequencer(cmd) => ("sequencer", Some(cmd.project())),
        Command::Audit(cmd) => ("audit", Some(cmd.project())),
        Command::Collaborator(cmd) => ("collaborator", Some(cmd.project())),
        Command::QuerySiteMap { project, .. } => ("query_site_map", Some(*project)),
        Command::AddSiteMapEntry { project, .. } => ("add_site_map_entry", Some(*project)),
        Command::ReadSiteMapEntry { project, .. } => ("read_site_map_entry", Some(*project)),
        Command::StartProject { .. } => ("start_project", Some(current)),
        Command::NewProject { .. } => ("new_project", Some(current)),
        Command::OpenProject { .. } => ("open_project", Some(current)),
        Command::SaveProject { project, .. } => ("save_project", Some(*project)),
        Command::SlimSaveProjectAs { project, .. } => ("slim_save_project_as", Some(*project)),
        Command::StartProxy { project, .. } => ("start_proxy", Some(*project)),
        Command::Quiesce { project } => ("quiesce", Some(*project)),
        Command::Resume { project } => ("resume", Some(*project)),
        Command::StopProxy { project } => ("stop_proxy", Some(*project)),
        Command::StopProxyListener { project, .. } => ("stop_proxy_listener", Some(*project)),
        Command::SetIntercept { project, .. } => ("set_intercept", Some(*project)),
        Command::OpenConfiguredBrowser { project, .. } => {
            ("open_configured_browser", Some(*project))
        }
        Command::Decide { project, .. } => ("decide", Some(*project)),
        Command::DecideWebSocket { project, .. } => ("decide_websocket", Some(*project)),
        Command::QueryHistory { project, .. } => ("query_history", Some(*project)),
        Command::QueryLoggerHistory { project, .. } => ("query_logger_history", Some(*project)),
        Command::QueryHttpHistory { query } => ("query_http_history", Some(query.project)),
        Command::QueryGlobalSearch { query, .. } => ("query_global_search", Some(query.project)),
        Command::DeleteHttpHistory { project, .. } => ("delete_http_history", Some(*project)),
        Command::ClearLiveProxyHistory { project, .. } => {
            ("clear_live_proxy_history", Some(*project))
        }
        Command::QueryRepeaterHistory { project, .. } => ("query_repeater_history", Some(*project)),
        Command::SelectRepeaterHistory { project, .. } => {
            ("select_repeater_history", Some(*project))
        }
        Command::ReadFlow { project, .. } => ("read_flow", Some(*project)),
        Command::QueryWebSocketHistory { project, .. } => {
            ("query_websocket_history", Some(*project))
        }
        Command::ReadWebSocket { project, .. } => ("read_websocket", Some(*project)),
        Command::ReadWebSocketNotes { project, .. } => ("read_websocket_notes", Some(*project)),
        Command::ReadExtHttpNotes { project, .. } => ("read_ext_http_notes", Some(*project)),
        Command::SetExtHttpNotes { project, .. } => ("set_ext_http_notes", Some(*project)),
        Command::ReadExtHttpNotesBatch { project, .. } => {
            ("read_ext_http_notes_batch", Some(*project))
        }
        Command::SetExtHttpNotesBatch { project, .. } => {
            ("set_ext_http_notes_batch", Some(*project))
        }
        Command::SetExtHttpHighlights { project, .. } => {
            ("set_ext_http_highlights", Some(*project))
        }
        Command::SetWebSocketNotes { project, .. } => ("set_websocket_notes", Some(*project)),
        Command::NewRepeater { project } => ("new_repeater", Some(*project)),
        Command::OpenRepeater { project, .. } => ("open_repeater", Some(*project)),
        Command::SaveWorkspace { project, .. } => ("save_workspace", Some(*project)),
        Command::Send { project, .. } => ("send", Some(*project)),
        Command::CancelSend { project, .. } => ("cancel_send", Some(*project)),
        Command::AutoIntruderPositions { project, .. } => {
            ("auto_intruder_positions", Some(*project))
        }
        Command::StartIntruder { project, .. } => ("start_intruder", Some(*project)),
        Command::StopIntruder { project, .. } => ("stop_intruder", Some(*project)),
        Command::SetIntruderPaused { project, .. } => ("set_intruder_paused", Some(*project)),
        Command::SetIntruderRetention { project, .. } => ("set_intruder_retention", Some(*project)),
        Command::RenameIntruder { project, .. } => ("rename_intruder", Some(*project)),
        Command::RequestIntruderBaseline { project, .. } => {
            ("request_intruder_baseline", Some(*project))
        }
        Command::QueryIntruderResourcePools { project, .. } => {
            ("query_intruder_resource_pools", Some(*project))
        }
        Command::QueryIntruderPoolUsage { project, .. } => {
            ("query_intruder_pool_usage", Some(*project))
        }
        Command::DeleteIntruderResourcePool { project, .. } => {
            ("delete_intruder_resource_pool", Some(*project))
        }
        Command::EnsureIntruderDefaultPool { project, .. } => {
            ("ensure_intruder_default_pool", Some(*project))
        }
        Command::SetIntruderResourcePools { project, .. } => {
            ("set_intruder_resource_pools", Some(*project))
        }
        Command::SetIntruderResourcePool { project, .. } => {
            ("set_intruder_resource_pool", Some(*project))
        }
        Command::DiscardIntruder { project, .. } => ("discard_intruder", Some(*project)),
        Command::QueryIntruderResults { project, .. } => ("query_intruder_results", Some(*project)),
        Command::QueryIntruderResultView { query, .. } => {
            ("query_intruder_result_view", Some(query.project))
        }
        Command::SetIntruderResultNotes { project, .. } => {
            ("set_intruder_result_notes", Some(*project))
        }
        Command::SetIntruderResultHighlight { project, .. } => {
            ("set_intruder_result_highlight", Some(*project))
        }
        Command::SetLiveFlowAnnotated { project, .. } => {
            ("set_live_flow_annotated", Some(*project))
        }
        Command::QueryIntruderRuns { project, .. } => ("query_intruder_runs", Some(*project)),
        Command::ReadIntruderSpec { project, .. } => ("read_intruder_spec", Some(*project)),
        Command::ExportCa { .. } => ("export_ca", None),
        Command::RegenerateCa => ("regenerate_ca", None),
        Command::ImportCa { .. } => ("import_ca", None),
        Command::CheckForUpdates { .. } => ("check_for_updates", None),
        Command::InstallUpdate { .. } => ("install_update", None),
    }
}
