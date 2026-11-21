use std::{
    collections::{HashMap, VecDeque},
    io::{self, Read, Write},
};

pub use mio_plugin_api::{
    self, FlowRecord, FlowSummary, Header, HttpRequest, HttpResponse, HttpVersion, Uuid, panel,
    protocol,
};
use mio_plugin_api::{
    panel::{Empty, PanelSnapshot},
    protocol::{
        ActiveCheckCall, ActiveCheckResult, Cancel, CloseRequest, CommitEditor,
        CommitResponseEditor, Committed, Component, ContextMenuCall, ContextMenuResult, Envelope,
        ErrorCode, Failure, HookAction, HttpHookCall, HttpHookResult, OpenRequest, OpenResponse,
        PANEL_ID, PassiveCheckCall, PassiveCheckResult, PayloadCall, PayloadResult, Register,
        ResponseCommitted, SessionActionCall, SessionActionResult, WebSocketHookCall,
        WebSocketHookResult, WorkerSession, envelope,
    },
};

pub mod context;
pub mod ui;

pub use context::{HostClient, HostContext};
use context::PanelRevisionState;

pub trait MioPlugin {
    fn id(&self) -> &str;

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn required_components(&self) -> Vec<Component> {
        vec![Component::Text]
    }

    fn initial_panel(&self) -> PanelSnapshot {
        ui::snapshot(
            1,
            self.id(),
            ui::text(1, "Status", "Ready", panel::Tone::Foreground),
        )
    }

    fn on_ui_event(
        &mut self,
        _ctx: &mut dyn HostClient,
        _event: panel::UiEvent,
    ) -> Result<Option<PanelSnapshot>, String> {
        Ok(None)
    }

    fn on_open_request(
        &mut self,
        _ctx: &mut dyn HostClient,
        _open: OpenRequest,
    ) -> Result<Option<PanelSnapshot>, String> {
        Ok(None)
    }

    fn on_open_response(
        &mut self,
        _ctx: &mut dyn HostClient,
        _open: OpenResponse,
    ) -> Result<Option<PanelSnapshot>, String> {
        Ok(None)
    }

    fn on_close_request(
        &mut self,
        _ctx: &mut dyn HostClient,
        _close: CloseRequest,
    ) -> Result<Option<PanelSnapshot>, String> {
        Ok(None)
    }

    fn on_commit_editor(
        &mut self,
        _ctx: &mut dyn HostClient,
        _commit: CommitEditor,
    ) -> Result<Committed, String> {
        Err("plugin does not handle CommitEditor".into())
    }

    fn on_commit_response_editor(
        &mut self,
        _ctx: &mut dyn HostClient,
        _commit: CommitResponseEditor,
    ) -> Result<ResponseCommitted, String> {
        Err("plugin does not handle CommitResponseEditor".into())
    }

    fn on_context_menu(
        &mut self,
        _ctx: &mut dyn HostClient,
        call: ContextMenuCall,
    ) -> Result<ContextMenuResult, String> {
        Ok(ContextMenuResult {
            status_message: format!("handled context action {}", call.action_id),
            log_line: String::new(),
            open_repeater: None,
            replace_request: None,
        })
    }

    fn on_http_hook(
        &mut self,
        _ctx: &mut dyn HostClient,
        _call: HttpHookCall,
    ) -> Result<HttpHookResult, String> {
        Ok(HttpHookResult {
            action: HookAction::Pass as i32,
            request: None,
            response: None,
        })
    }

    fn on_ws_hook(
        &mut self,
        _ctx: &mut dyn HostClient,
        _call: WebSocketHookCall,
    ) -> Result<WebSocketHookResult, String> {
        Ok(WebSocketHookResult {
            action: HookAction::Pass as i32,
            payload: Vec::new(),
        })
    }

    fn on_session_action(
        &mut self,
        _ctx: &mut dyn HostClient,
        _call: SessionActionCall,
    ) -> Result<SessionActionResult, String> {
        Ok(SessionActionResult {
            modified: false,
            request: None,
        })
    }

    fn on_payload(
        &mut self,
        _ctx: &mut dyn HostClient,
        _call: PayloadCall,
    ) -> Result<PayloadResult, String> {
        Ok(PayloadResult {
            outputs: Vec::new(),
        })
    }

    fn on_passive_check(
        &mut self,
        _ctx: &mut dyn HostClient,
        _call: PassiveCheckCall,
    ) -> Result<PassiveCheckResult, String> {
        Ok(PassiveCheckResult {
            findings: Vec::new(),
            log_line: String::new(),
        })
    }

    fn on_active_check(
        &mut self,
        _ctx: &mut dyn HostClient,
        _call: ActiveCheckCall,
    ) -> Result<ActiveCheckResult, String> {
        Ok(ActiveCheckResult {
            findings: Vec::new(),
        })
    }

    /// Invoked when a host-initiated panel or hook operation is cancelled or times out.
    ///
    /// Derived `HostCall`s triggered inside `on_cancel` run inside a bounded cleanup window tied to
    /// the host's `Cancel` notification on a **live** panel (closing/torn-down panels reject
    /// `HostCall`s as `Stale`). Within that window, allowlisted read/KV cleanup operations
    /// (`QueryHistory`, `ReadFlow`, `ScopeCheck`, `QuerySiteMap`, `ReadAnnotations`, `StorageGet`,
    /// `StorageSet`, `PreferencesGet`, `PreferencesSet`) are permitted even if local panel
    /// revision has transiently drifted from the host (scheduling `Resync` only if the active
    /// `Cancel` is for a non-`Resync` request and the callback's envelope revision exceeds the
    /// host revision), whereas all
    /// mutating or side-effect calls (including `UpdateDraft`, `OpenRepeater`, `OpenIntruder`,
    /// `NetworkSend`, `ScopeUpdate`, `OpenOrganiser`, `UpdateAnnotations`, `SubmitAuditIssue`, and
    /// `Collaborator*`) are rejected with `ErrorCode::Cancelled`. The runtime always replies `Ack`
    /// (ignoring any returned `PanelSnapshot`) or `Error`, which ends the cleanup window.
    fn on_cancel(
        &mut self,
        _ctx: &mut dyn HostClient,
        _cancel: Cancel,
    ) -> Result<Option<PanelSnapshot>, String> {
        Ok(None)
    }

    /// Invoked when the host requests panel resynchronization to its authoritative `snapshot`.
    ///
    /// Plugins that support resync recovery should override `on_resync` (or `on_panel_resync`),
    /// adopt `snapshot`, and return `Ok(Some(confirmed_snapshot))` at `ctx.next_panel_revision()`
    /// to confirm authoritative UI semantics after drift (returning `Ok(None)` confirms only the
    /// revision without clearing semantic-drift protection). The default returns `Err(...)` so
    /// plugins that do not apply `snapshot` never falsely acknowledge the host's view.
    fn on_resync(
        &mut self,
        _ctx: &mut dyn HostClient,
        _snapshot: PanelSnapshot,
    ) -> Result<Option<PanelSnapshot>, String> {
        Err("plugin does not implement on_resync".into())
    }

    fn supports_panel_multiplexing(&self) -> bool {
        false
    }

    fn on_panel_ui_event(
        &mut self,
        ctx: &mut dyn HostClient,
        _panel_id: u64,
        event: panel::UiEvent,
    ) -> Result<Option<PanelSnapshot>, String> {
        self.on_ui_event(ctx, event)
    }

    fn on_panel_open_request(
        &mut self,
        ctx: &mut dyn HostClient,
        _panel_id: u64,
        open: OpenRequest,
    ) -> Result<Option<PanelSnapshot>, String> {
        self.on_open_request(ctx, open)
    }

    fn on_panel_open_response(
        &mut self,
        ctx: &mut dyn HostClient,
        _panel_id: u64,
        open: OpenResponse,
    ) -> Result<Option<PanelSnapshot>, String> {
        self.on_open_response(ctx, open)
    }

    fn on_panel_close_request(
        &mut self,
        ctx: &mut dyn HostClient,
        _panel_id: u64,
        close: CloseRequest,
    ) -> Result<Option<PanelSnapshot>, String> {
        self.on_close_request(ctx, close)
    }

    fn on_panel_destroy(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        close: CloseRequest,
    ) -> Result<Option<PanelSnapshot>, String> {
        self.on_panel_close_request(ctx, panel_id, close)
    }

    fn on_panel_commit_editor(
        &mut self,
        ctx: &mut dyn HostClient,
        _panel_id: u64,
        commit: CommitEditor,
    ) -> Result<Committed, String> {
        self.on_commit_editor(ctx, commit)
    }

    fn on_panel_commit_response_editor(
        &mut self,
        ctx: &mut dyn HostClient,
        _panel_id: u64,
        commit: CommitResponseEditor,
    ) -> Result<ResponseCommitted, String> {
        self.on_commit_response_editor(ctx, commit)
    }

    fn on_panel_resync(
        &mut self,
        ctx: &mut dyn HostClient,
        _panel_id: u64,
        snapshot: PanelSnapshot,
    ) -> Result<Option<PanelSnapshot>, String> {
        self.on_resync(ctx, snapshot)
    }
}

pub fn run_stdio(plugin: impl MioPlugin) -> io::Result<()> {
    run_with_io(io::stdin().lock(), io::stdout().lock(), plugin)
}

fn reply_panel_or_ack<R: Read, W: Write>(
    session: &mut WorkerSession<R, W>,
    panel_id: u64,
    panel_revision: &mut PanelRevisionState,
    reply_to: u64,
    outcome: Result<Option<PanelSnapshot>, String>,
) -> io::Result<()> {
    match outcome {
        Ok(Some(snapshot)) => {
            panel_revision.published = snapshot.revision;
            panel_revision.pending = panel_revision.pending.max(snapshot.revision);
            let rev = snapshot.revision;
            session.send_on_panel(panel_id, envelope::Body::Snapshot(snapshot), rev, reply_to)?;
        }
        Ok(None) => {
            session.send_on_panel(
                panel_id,
                envelope::Body::Ack(Empty {}),
                panel_revision.published,
                reply_to,
            )?;
        }
        Err(msg) => {
            session.send_on_panel(
                panel_id,
                envelope::Body::Error(Failure::new(ErrorCode::Invalid, msg)),
                panel_revision.published,
                reply_to,
            )?;
        }
    }
    Ok(())
}

pub fn run_with_io<R: Read, W: Write>(
    reader: R,
    writer: W,
    mut plugin: impl MioPlugin,
) -> io::Result<()> {
    let initial = plugin.initial_panel();
    let initial_rev = initial.revision.max(1);
    let registration = Register {
        plugin_id: plugin.id().to_owned(),
        plugin_version: plugin.version().to_owned(),
        required_components: plugin
            .required_components()
            .into_iter()
            .map(|c| c as i32)
            .collect(),
        panel: Some(initial),
        supports_panel_multiplexing: plugin.supports_panel_multiplexing(),
    };
    let mut session = WorkerSession::accept(reader, writer, registration)?;
    let mut deferred: VecDeque<Envelope> = VecDeque::new();
    let mut panel_revisions: HashMap<u64, PanelRevisionState> =
        HashMap::from([(PANEL_ID, PanelRevisionState::new(initial_rev))]);

    loop {
        let envelope = match deferred.pop_front() {
            Some(env) => env,
            None => match session.receive() {
                Ok(env) => env,
                Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(err) => return Err(err),
            },
        };
        let expired = envelope.is_expired();
        let reply_to = envelope.request_id;
        let panel_id = envelope.panel_id.max(PANEL_ID);
        let incoming_rev = envelope.revision;
        let body = envelope.body.ok_or_else(|| io::Error::other("空消息"))?;
        if expired
            && !matches!(
                body,
                envelope::Body::Shutdown(_)
                    | envelope::Body::Cancel(_)
                    | envelope::Body::CloseRequest(_)
            )
        {
            if reply_to != 0 {
                let msg = if panel_id != PANEL_ID {
                    "子面板请求已超时"
                } else {
                    "插件请求已超时"
                };
                let rev = panel_revisions
                    .get(&panel_id)
                    .map_or(incoming_rev, |s| s.published);
                session.send_on_panel(
                    panel_id,
                    envelope::Body::Error(Failure::new(ErrorCode::Deadline, msg)),
                    rev,
                    reply_to,
                )?;
            }
            continue;
        }
        match body {
            envelope::Body::Shutdown(_) => break,
            envelope::Body::UiEvent(ev) => {
                let panel_revision = panel_revisions
                    .entry(panel_id)
                    .or_insert_with(|| PanelRevisionState::new(incoming_rev));
                let res = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_id,
                        parent_request_id: reply_to,
                        panel_revision,
                    };
                    plugin.on_panel_ui_event(&mut ctx, panel_id, ev)
                };
                reply_panel_or_ack(&mut session, panel_id, panel_revision, reply_to, res)?;
            }
            envelope::Body::OpenRequest(open) => {
                let panel_revision = panel_revisions
                    .entry(panel_id)
                    .or_insert_with(|| PanelRevisionState::new(incoming_rev));
                let res = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_id,
                        parent_request_id: reply_to,
                        panel_revision,
                    };
                    plugin.on_panel_open_request(&mut ctx, panel_id, open)
                };
                reply_panel_or_ack(&mut session, panel_id, panel_revision, reply_to, res)?;
            }
            envelope::Body::OpenResponse(open) => {
                let panel_revision = panel_revisions
                    .entry(panel_id)
                    .or_insert_with(|| PanelRevisionState::new(incoming_rev));
                let res = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_id,
                        parent_request_id: reply_to,
                        panel_revision,
                    };
                    plugin.on_panel_open_response(&mut ctx, panel_id, open)
                };
                reply_panel_or_ack(&mut session, panel_id, panel_revision, reply_to, res)?;
            }
            envelope::Body::CloseRequest(close) => {
                let destroy_panel = close.destroy_panel && panel_id != PANEL_ID;
                let panel_revision = panel_revisions
                    .entry(panel_id)
                    .or_insert_with(|| PanelRevisionState::new(incoming_rev));
                let res = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_id,
                        parent_request_id: reply_to,
                        panel_revision,
                    };
                    if destroy_panel {
                        plugin.on_panel_destroy(&mut ctx, panel_id, close)
                    } else {
                        plugin.on_panel_close_request(&mut ctx, panel_id, close)
                    }
                };
                reply_panel_or_ack(&mut session, panel_id, panel_revision, reply_to, res)?;
                if destroy_panel {
                    panel_revisions.remove(&panel_id);
                }
            }
            envelope::Body::CommitEditor(commit) => {
                let panel_revision = panel_revisions
                    .entry(panel_id)
                    .or_insert_with(|| PanelRevisionState::new(incoming_rev));
                let out = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_id,
                        parent_request_id: reply_to,
                        panel_revision,
                    };
                    plugin.on_panel_commit_editor(&mut ctx, panel_id, commit)
                };
                let wire = match out {
                    Ok(committed) => envelope::Body::Committed(committed),
                    Err(msg) => envelope::Body::Error(Failure::new(ErrorCode::Stale, msg)),
                };
                session.send_on_panel(panel_id, wire, panel_revision.published, reply_to)?;
            }
            envelope::Body::CommitResponseEditor(commit) => {
                let panel_revision = panel_revisions
                    .entry(panel_id)
                    .or_insert_with(|| PanelRevisionState::new(incoming_rev));
                let out = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_id,
                        parent_request_id: reply_to,
                        panel_revision,
                    };
                    plugin.on_panel_commit_response_editor(&mut ctx, panel_id, commit)
                };
                let wire = match out {
                    Ok(committed) => envelope::Body::ResponseCommitted(committed),
                    Err(msg) => envelope::Body::Error(Failure::new(ErrorCode::Stale, msg)),
                };
                session.send_on_panel(panel_id, wire, panel_revision.published, reply_to)?;
            }
            envelope::Body::ContextMenuCall(call) => {
                let panel_revision = panel_revisions
                    .entry(PANEL_ID)
                    .or_insert_with(|| PanelRevisionState::new(initial_rev));
                let out = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_id: PANEL_ID,
                        parent_request_id: reply_to,
                        panel_revision,
                    };
                    plugin.on_context_menu(&mut ctx, call)
                };
                let wire = match out {
                    Ok(res) => envelope::Body::ContextMenuResult(res),
                    Err(msg) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, msg)),
                };
                session.send_on_panel(PANEL_ID, wire, panel_revision.published, reply_to)?;
            }
            envelope::Body::HttpHookCall(call) => {
                let panel_revision = panel_revisions
                    .entry(PANEL_ID)
                    .or_insert_with(|| PanelRevisionState::new(initial_rev));
                let out = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_id: PANEL_ID,
                        parent_request_id: reply_to,
                        panel_revision,
                    };
                    plugin.on_http_hook(&mut ctx, call)
                };
                let wire = match out {
                    Ok(res) => envelope::Body::HttpHookResult(res),
                    Err(msg) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, msg)),
                };
                session.send_on_panel(PANEL_ID, wire, panel_revision.published, reply_to)?;
            }
            envelope::Body::WebSocketHookCall(call) => {
                let panel_revision = panel_revisions
                    .entry(PANEL_ID)
                    .or_insert_with(|| PanelRevisionState::new(initial_rev));
                let out = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_id: PANEL_ID,
                        parent_request_id: reply_to,
                        panel_revision,
                    };
                    plugin.on_ws_hook(&mut ctx, call)
                };
                let wire = match out {
                    Ok(res) => envelope::Body::WebSocketHookResult(res),
                    Err(msg) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, msg)),
                };
                session.send_on_panel(PANEL_ID, wire, panel_revision.published, reply_to)?;
            }
            envelope::Body::SessionActionCall(call) => {
                let panel_revision = panel_revisions
                    .entry(PANEL_ID)
                    .or_insert_with(|| PanelRevisionState::new(initial_rev));
                let out = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_id: PANEL_ID,
                        parent_request_id: reply_to,
                        panel_revision,
                    };
                    plugin.on_session_action(&mut ctx, call)
                };
                let wire = match out {
                    Ok(res) => envelope::Body::SessionActionResult(res),
                    Err(msg) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, msg)),
                };
                session.send_on_panel(PANEL_ID, wire, panel_revision.published, reply_to)?;
            }
            envelope::Body::PayloadCall(call) => {
                let panel_revision = panel_revisions
                    .entry(PANEL_ID)
                    .or_insert_with(|| PanelRevisionState::new(initial_rev));
                let out = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_id: PANEL_ID,
                        parent_request_id: reply_to,
                        panel_revision,
                    };
                    plugin.on_payload(&mut ctx, call)
                };
                let wire = match out {
                    Ok(res) => envelope::Body::PayloadResult(res),
                    Err(msg) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, msg)),
                };
                session.send_on_panel(PANEL_ID, wire, panel_revision.published, reply_to)?;
            }
            envelope::Body::PassiveCheckCall(call) => {
                let panel_revision = panel_revisions
                    .entry(PANEL_ID)
                    .or_insert_with(|| PanelRevisionState::new(initial_rev));
                let out = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_id: PANEL_ID,
                        parent_request_id: reply_to,
                        panel_revision,
                    };
                    plugin.on_passive_check(&mut ctx, call)
                };
                let wire = match out {
                    Ok(res) => envelope::Body::PassiveCheckResult(res),
                    Err(msg) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, msg)),
                };
                session.send_on_panel(PANEL_ID, wire, panel_revision.published, reply_to)?;
            }
            envelope::Body::ActiveCheckCall(call) => {
                let panel_revision = panel_revisions
                    .entry(PANEL_ID)
                    .or_insert_with(|| PanelRevisionState::new(initial_rev));
                let out = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_id: PANEL_ID,
                        parent_request_id: reply_to,
                        panel_revision,
                    };
                    plugin.on_active_check(&mut ctx, call)
                };
                let wire = match out {
                    Ok(res) => envelope::Body::ActiveCheckResult(res),
                    Err(msg) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, msg)),
                };
                session.send_on_panel(PANEL_ID, wire, panel_revision.published, reply_to)?;
            }
            envelope::Body::Cancel(cancel) => {
                let panel_revision = if panel_id == PANEL_ID {
                    panel_revisions
                        .entry(panel_id)
                        .or_insert_with(|| PanelRevisionState::new(incoming_rev))
                } else if let Some(existing) = panel_revisions.get_mut(&panel_id) {
                    existing
                } else {
                    session.send_on_panel(
                        panel_id,
                        envelope::Body::Ack(Empty {}),
                        incoming_rev,
                        reply_to,
                    )?;
                    continue;
                };
                let res = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_id,
                        parent_request_id: reply_to,
                        panel_revision,
                    };
                    plugin.on_cancel(&mut ctx, cancel)
                };
                match res {
                    Ok(_) => {
                        session.send_on_panel(
                            panel_id,
                            envelope::Body::Ack(Empty {}),
                            panel_revision.published,
                            reply_to,
                        )?;
                    }
                    Err(msg) => {
                        session.send_on_panel(
                            panel_id,
                            envelope::Body::Error(Failure::new(ErrorCode::Invalid, msg)),
                            panel_revision.published,
                            reply_to,
                        )?;
                    }
                }
            }
            envelope::Body::Resync(snapshot) => {
                let panel_revision = panel_revisions
                    .entry(panel_id)
                    .or_insert_with(|| PanelRevisionState::new(snapshot.revision));
                let prev_published = panel_revision.published;
                panel_revision.pending = panel_revision
                    .pending
                    .max(panel_revision.published)
                    .max(snapshot.revision);
                panel_revision.published = snapshot.revision;
                let res = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_id,
                        parent_request_id: reply_to,
                        panel_revision,
                    };
                    plugin.on_panel_resync(&mut ctx, panel_id, snapshot)
                };
                if res.is_err() {
                    panel_revision.published = prev_published;
                }
                reply_panel_or_ack(&mut session, panel_id, panel_revision, reply_to, res)?;
            }
            _ => return Err(io::Error::other("worker 收到方向无效的消息")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use mio_plugin_api::{
        ProjectKey,
        protocol::{
            ChannelIdentity, CollaboratorPayloadDto, Hello, HttpRequestDto, HttpResponseDto,
            HttpVersionDto, MAX_FRAME_BYTES, Permission, ScopeStatusDto, encode_frame,
            host_result, read_frame,
        },
    };

    use super::*;

    struct DemoPlugin {
        passive_hits: usize,
        deferred_passive_seen: usize,
    }

    impl MioPlugin for DemoPlugin {
        fn id(&self) -> &str {
            "org.mio.sdk-demo"
        }

        fn on_ui_event(
            &mut self,
            ctx: &mut dyn HostClient,
            _event: panel::UiEvent,
        ) -> Result<Option<PanelSnapshot>, String> {
            let in_scope = ctx.is_in_scope("https://example.com/")?;
            assert!(in_scope);
            let oast = ctx.collaborator_generate("sdk")?;
            assert_eq!(oast.token, "tok-sdk");
            let rev = ctx.next_panel_revision();
            Ok(Some(ui::snapshot(
                rev,
                self.id(),
                ui::column(
                    1,
                    "root",
                    vec![
                        ui::badge(2, &oast.token, panel::Tone::Accent),
                        ui::text(3, "status", "ok", panel::Tone::Foreground),
                    ],
                    6.0,
                ),
            )))
        }

        fn on_http_hook(
            &mut self,
            _ctx: &mut dyn HostClient,
            call: HttpHookCall,
        ) -> Result<HttpHookResult, String> {
            let mut req = call.request.ok_or("missing request")?;
            req.headers.push(protocol::HeaderDto {
                name: "x-sdk-hook".into(),
                value: b"1".to_vec(),
            });
            Ok(HttpHookResult {
                action: HookAction::Modify as i32,
                request: Some(req),
                response: None,
            })
        }

        fn on_passive_check(
            &mut self,
            _ctx: &mut dyn HostClient,
            call: PassiveCheckCall,
        ) -> Result<PassiveCheckResult, String> {
            self.passive_hits += 1;
            if call.url.contains("deferred") {
                self.deferred_passive_seen += 1;
            }
            let findings = vec![ui::finding(
                "SDK Demo Finding",
                protocol::FindingSeverity::High,
                protocol::FindingConfidence::Certain,
                "Matched test marker",
                "Fix it",
                "body",
                "DEMO",
            )];
            Ok(PassiveCheckResult {
                findings,
                log_line: format!("checked {}", call.url),
            })
        }
    }

    #[test]
    fn sdk_handles_handshake_hooks_and_deferred_envelopes_during_host_calls() {
        let identity = ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: ProjectKey {
                id: Uuid::new_v4(),
                generation: 1,
            },
        };
        let timeout = Duration::from_secs(5);
        let req = HttpRequestDto {
            method: "GET".into(),
            url: "https://example.com/deferred".into(),
            version: HttpVersionDto::Http11 as i32,
            ..Default::default()
        };
        let resp = HttpResponseDto {
            status: 200,
            version: HttpVersionDto::Http11 as i32,
            body: b"DEMO".to_vec(),
            ..Default::default()
        };

        // Pre-script incoming stream with Hello, Ready, UiEvent, interleaved PassiveCheckCall (before HostResult!),
        // HostResult(ScopeStatus), HostResult(CollaboratorPayload), HttpHookCall, Shutdown.
        // Expected outgoing request IDs from worker:
        // 1 = Register
        // 2 = HostCall(ScopeCheck)
        // 3 = HostCall(CollaboratorGenerate)
        // 4 = Snapshot(from UiEvent)
        // 5 = PassiveCheckResult(from deferred PassiveCheckCall)
        // 6 = HttpHookResult
        let mut incoming_bytes = Vec::new();
        let mut push_host = |req_id: u64, in_reply_to: u64, body: envelope::Body| {
            let mut env = Envelope::new(identity, req_id, 1, body, timeout);
            env.in_reply_to = in_reply_to;
            incoming_bytes.extend_from_slice(&encode_frame(&env).unwrap());
        };

        push_host(
            1,
            0,
            envelope::Body::Hello(Hello {
                max_frame_bytes: MAX_FRAME_BYTES as u32,
                max_operation_ms: 5000,
                granted_permissions: vec![
                    Permission::TrafficRead as i32,
                    Permission::NetworkSend as i32,
                ],
                host_minor: mio_plugin_api::protocol::PROTOCOL_MINOR,
            }),
        );
        push_host(2, 1, envelope::Body::Ready(Empty {}));
        push_host(
            3,
            0,
            envelope::Body::UiEvent(panel::UiEvent {
                node_id: 1,
                event: Some(panel::ui_event::Event::Click(Empty {})),
                editors: vec![],
            }),
        );
        // Interleaved PassiveCheckCall arrives BEFORE HostResult for req_id=2!
        push_host(
            4,
            0,
            envelope::Body::PassiveCheckCall(PassiveCheckCall {
                flow_id: Uuid::new_v4().as_bytes().to_vec(),
                url: req.url.clone(),
                request: Some(req.clone()),
                response: Some(resp),
            }),
        );
        push_host(
            5,
            2,
            envelope::Body::HostResult(protocol::HostResult {
                outcome: Some(host_result::Outcome::ScopeStatus(ScopeStatusDto {
                    in_scope: true,
                })),
            }),
        );
        push_host(
            6,
            3,
            envelope::Body::HostResult(protocol::HostResult {
                outcome: Some(host_result::Outcome::CollaboratorPayload(
                    CollaboratorPayloadDto {
                        token: "tok-sdk".into(),
                        label: "sdk".into(),
                        http_callback_url: "http://127.0.0.1:9000/tok-sdk".into(),
                        dns_qname: "tok-sdk.oast.local".into(),
                        created_unix_ms: 1,
                    },
                )),
            }),
        );
        push_host(
            7,
            0,
            envelope::Body::HttpHookCall(HttpHookCall {
                flow_id: Uuid::new_v4().as_bytes().to_vec(),
                source: protocol::SourceDto::Proxy as i32,
                plugin_id: "org.mio.sdk-demo".into(),
                response_stage: false,
                response_head_only: false,
                request: Some(req),
                response: None,
            }),
        );
        push_host(8, 0, envelope::Body::Shutdown(Empty {}));

        let mut outgoing_bytes = Vec::new();
        let mut plugin = DemoPlugin {
            passive_hits: 0,
            deferred_passive_seen: 0,
        };
        run_with_io(incoming_bytes.as_slice(), &mut outgoing_bytes, &mut plugin).unwrap();
        assert_eq!(plugin.passive_hits, 1);
        assert_eq!(plugin.deferred_passive_seen, 1);

        let mut cursor = outgoing_bytes.as_slice();
        let mut replies = Vec::new();
        while !cursor.is_empty() {
            replies.push(read_frame(&mut cursor).unwrap());
        }
        assert_eq!(replies.len(), 6);
        assert!(matches!(replies[0].body, Some(envelope::Body::Register(_))));
        assert!(matches!(replies[1].body, Some(envelope::Body::HostCall(_))));
        assert!(matches!(replies[2].body, Some(envelope::Body::HostCall(_))));
        assert!(matches!(
            replies[3].body,
            Some(envelope::Body::Snapshot(ref s)) if s.revision == 2
        ));
        assert_eq!(replies[3].in_reply_to, 3);
        assert!(matches!(
            replies[4].body,
            Some(envelope::Body::PassiveCheckResult(ref r)) if r.findings.len() == 1
        ));
        assert_eq!(replies[4].in_reply_to, 4);
        assert!(matches!(
            replies[5].body,
            Some(envelope::Body::HttpHookResult(ref r)) if r.action == HookAction::Modify as i32
        ));
        assert_eq!(replies[5].in_reply_to, 7);
    }

    struct MultiplexPlugin {
        panels: HashMap<u64, u64>,
    }

    impl MioPlugin for MultiplexPlugin {
        fn id(&self) -> &str {
            "org.mio.sdk-multiplex"
        }

        fn supports_panel_multiplexing(&self) -> bool {
            true
        }

        fn on_panel_open_request(
            &mut self,
            ctx: &mut dyn HostClient,
            panel_id: u64,
            _open: OpenRequest,
        ) -> Result<Option<PanelSnapshot>, String> {
            assert_eq!(ctx.panel_id(), panel_id);
            let rev = ctx.next_panel_revision();
            self.panels.insert(panel_id, rev);
            Ok(Some(ui::snapshot(
                rev,
                self.id(),
                ui::text(
                    1,
                    "panel",
                    format!("panel-{panel_id}-rev-{rev}"),
                    panel::Tone::Foreground,
                ),
            )))
        }

        fn on_panel_ui_event(
            &mut self,
            ctx: &mut dyn HostClient,
            panel_id: u64,
            _event: panel::UiEvent,
        ) -> Result<Option<PanelSnapshot>, String> {
            let rev = ctx.next_panel_revision();
            self.panels.insert(panel_id, rev);
            Ok(Some(ui::snapshot(
                rev,
                self.id(),
                ui::text(
                    1,
                    "panel",
                    format!("panel-{panel_id}-event-{rev}"),
                    panel::Tone::Accent,
                ),
            )))
        }

        fn on_panel_close_request(
            &mut self,
            _ctx: &mut dyn HostClient,
            panel_id: u64,
            _close: CloseRequest,
        ) -> Result<Option<PanelSnapshot>, String> {
            self.panels.remove(&panel_id);
            Ok(None)
        }
    }

    #[test]
    fn sdk_multiplexes_multiple_panels_with_isolated_revisions_and_close() {
        let identity = ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: ProjectKey {
                id: Uuid::new_v4(),
                generation: 1,
            },
        };
        let timeout = Duration::from_secs(5);
        let req = HttpRequestDto {
            method: "GET".into(),
            url: "https://example.invalid/".into(),
            version: HttpVersionDto::Http11 as i32,
            ..Default::default()
        };

        let mut incoming_bytes = Vec::new();
        let mut push_host = |panel_id: u64, req_id: u64, in_reply_to: u64, body: envelope::Body| {
            let mut env = Envelope::new(identity, req_id, 1, body, timeout).with_panel_id(panel_id);
            env.in_reply_to = in_reply_to;
            incoming_bytes.extend_from_slice(&encode_frame(&env).unwrap());
        };

        push_host(
            PANEL_ID,
            1,
            0,
            envelope::Body::Hello(Hello {
                max_frame_bytes: MAX_FRAME_BYTES as u32,
                max_operation_ms: 5000,
                granted_permissions: vec![Permission::TrafficRead as i32],
                host_minor: mio_plugin_api::protocol::PROTOCOL_MINOR,
            }),
        );
        push_host(PANEL_ID, 2, 1, envelope::Body::Ready(Empty {}));
        push_host(
            2,
            3,
            0,
            envelope::Body::OpenRequest(OpenRequest {
                resource_id: Uuid::new_v4().as_bytes().to_vec(),
                message_revision: 1,
                read_only: false,
                request: Some(req.clone()),
            }),
        );
        push_host(
            3,
            4,
            0,
            envelope::Body::OpenRequest(OpenRequest {
                resource_id: Uuid::new_v4().as_bytes().to_vec(),
                message_revision: 1,
                read_only: false,
                request: Some(req),
            }),
        );
        push_host(
            2,
            5,
            0,
            envelope::Body::UiEvent(panel::UiEvent {
                node_id: 1,
                event: Some(panel::ui_event::Event::Click(Empty {})),
                editors: vec![],
            }),
        );
        push_host(
            2,
            6,
            0,
            envelope::Body::CloseRequest(CloseRequest {
                resource_id: Uuid::new_v4().as_bytes().to_vec(),
                destroy_panel: true,
            }),
        );
        push_host(PANEL_ID, 7, 0, envelope::Body::Shutdown(Empty {}));

        let mut outgoing_bytes = Vec::new();
        let mut plugin = MultiplexPlugin {
            panels: HashMap::new(),
        };
        run_with_io(incoming_bytes.as_slice(), &mut outgoing_bytes, &mut plugin).unwrap();
        assert!(!plugin.panels.contains_key(&2));
        assert!(plugin.panels.contains_key(&3));

        let mut cursor = outgoing_bytes.as_slice();
        let mut replies = Vec::new();
        while !cursor.is_empty() {
            replies.push(read_frame(&mut cursor).unwrap());
        }
        assert_eq!(replies.len(), 5);
        assert!(matches!(
            replies[0].body,
            Some(envelope::Body::Register(ref r)) if r.supports_panel_multiplexing
        ));
        assert_eq!(replies[1].panel_id, 2);
        assert!(matches!(
            replies[1].body,
            Some(envelope::Body::Snapshot(ref s)) if s.revision == 2
        ));
        assert_eq!(replies[2].panel_id, 3);
        assert!(matches!(
            replies[2].body,
            Some(envelope::Body::Snapshot(ref s)) if s.revision == 2
        ));
        assert_eq!(replies[3].panel_id, 2);
        assert!(matches!(
            replies[3].body,
            Some(envelope::Body::Snapshot(ref s)) if s.revision == 3
        ));
        assert_eq!(replies[4].panel_id, 2);
        assert!(matches!(replies[4].body, Some(envelope::Body::Ack(_))));
    }

    struct HostCallOnInitialOpenPlugin;

    impl MioPlugin for HostCallOnInitialOpenPlugin {
        fn id(&self) -> &str {
            "org.mio.sdk-open-hostcall"
        }

        fn supports_panel_multiplexing(&self) -> bool {
            true
        }

        fn on_panel_open_request(
            &mut self,
            ctx: &mut dyn HostClient,
            panel_id: u64,
            _open: OpenRequest,
        ) -> Result<Option<PanelSnapshot>, String> {
            let next_rev = ctx.next_panel_revision();
            let pref = ctx.preferences_get("selected_profile")?;
            let text = String::from_utf8_lossy(&pref.unwrap_or_default()).into_owned();
            Ok(Some(ui::snapshot(
                next_rev,
                self.id(),
                ui::text(
                    1,
                    "panel",
                    format!("panel-{panel_id}:{text}"),
                    panel::Tone::Foreground,
                ),
            )))
        }
    }

    #[test]
    fn sdk_child_panel_host_call_during_initial_open_uses_published_revision_zero() {
        let identity = ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: ProjectKey {
                id: Uuid::new_v4(),
                generation: 1,
            },
        };
        let timeout = Duration::from_secs(5);
        let req = HttpRequestDto {
            method: "GET".into(),
            url: "https://example.invalid/".into(),
            version: HttpVersionDto::Http11 as i32,
            ..Default::default()
        };

        let mut incoming_bytes = Vec::new();
        let mut push_host =
            |panel_id: u64, req_id: u64, rev: u64, in_reply_to: u64, body: envelope::Body| {
                let mut env =
                    Envelope::new(identity, req_id, rev, body, timeout).with_panel_id(panel_id);
                env.in_reply_to = in_reply_to;
                incoming_bytes.extend_from_slice(&encode_frame(&env).unwrap());
            };

        push_host(
            PANEL_ID,
            1,
            0,
            0,
            envelope::Body::Hello(Hello {
                max_frame_bytes: MAX_FRAME_BYTES as u32,
                max_operation_ms: 5000,
                granted_permissions: vec![Permission::TrafficRead as i32],
                host_minor: mio_plugin_api::protocol::PROTOCOL_MINOR,
            }),
        );
        push_host(PANEL_ID, 2, 1, 1, envelope::Body::Ready(Empty {}));
        // Host sends OpenRequest on child panel 2 with revision = 0 (no snapshot yet).
        push_host(
            2,
            3,
            0,
            0,
            envelope::Body::OpenRequest(OpenRequest {
                resource_id: Uuid::new_v4().as_bytes().to_vec(),
                message_revision: 1,
                read_only: false,
                request: Some(req),
            }),
        );
        // HostResult for the PreferencesGet HostCall (request_id = 2 from worker).
        push_host(
            2,
            4,
            0,
            2,
            envelope::Body::HostResult(mio_plugin_api::protocol::HostResult {
                outcome: Some(host_result::Outcome::StorageValue(
                    mio_plugin_api::protocol::StorageValue {
                        present: true,
                        value: b"preset-a".to_vec(),
                    },
                )),
            }),
        );
        push_host(PANEL_ID, 5, 1, 0, envelope::Body::Shutdown(Empty {}));

        let mut outgoing_bytes = Vec::new();
        let mut plugin = HostCallOnInitialOpenPlugin;
        run_with_io(incoming_bytes.as_slice(), &mut outgoing_bytes, &mut plugin).unwrap();

        let mut cursor = outgoing_bytes.as_slice();
        let mut replies = Vec::new();
        while !cursor.is_empty() {
            replies.push(read_frame(&mut cursor).unwrap());
        }
        assert_eq!(replies.len(), 3);
        // Frame 1: Register
        assert!(matches!(replies[0].body, Some(envelope::Body::Register(_))));
        // Frame 2: HostCall on panel 2 MUST carry published revision 0 and parent_request_id == 3
        assert_eq!(replies[1].panel_id, 2);
        assert_eq!(replies[1].revision, 0);
        assert!(matches!(
            replies[1].body,
            Some(envelope::Body::HostCall(ref hc)) if hc.parent_request_id == 3
        ));
        // Frame 3: Snapshot on panel 2 publishes revision 1
        assert_eq!(replies[2].panel_id, 2);
        assert_eq!(replies[2].revision, 1);
        assert!(matches!(
            replies[2].body,
            Some(envelope::Body::Snapshot(ref s)) if s.revision == 1
        ));
    }

    #[test]
    fn sdk_expired_queued_child_frame_reports_deadline_without_exiting_worker() {
        let identity = ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: ProjectKey {
                id: Uuid::new_v4(),
                generation: 1,
            },
        };
        let timeout = Duration::from_secs(5);
        let req = HttpRequestDto {
            method: "GET".into(),
            url: "https://example.invalid/".into(),
            version: HttpVersionDto::Http11 as i32,
            ..Default::default()
        };

        let mut incoming_bytes = Vec::new();
        let hello = Envelope::new(
            identity,
            1,
            0,
            envelope::Body::Hello(Hello {
                max_frame_bytes: MAX_FRAME_BYTES as u32,
                max_operation_ms: 5000,
                granted_permissions: vec![],
                host_minor: mio_plugin_api::protocol::PROTOCOL_MINOR,
            }),
            timeout,
        );
        let mut ready = Envelope::new(identity, 2, 1, envelope::Body::Ready(Empty {}), timeout);
        ready.in_reply_to = 1;
        // Queued expired OpenRequest on panel 2
        let mut expired_open2 = Envelope::new(
            identity,
            3,
            0,
            envelope::Body::OpenRequest(OpenRequest {
                resource_id: Uuid::new_v4().as_bytes().to_vec(),
                message_revision: 1,
                read_only: false,
                request: Some(req.clone()),
            }),
            timeout,
        )
        .with_panel_id(2);
        expired_open2.deadline_unix_ms = 1;
        // Subsequent valid OpenRequest on panel 3 (must still be handled!)
        let valid_open3 = Envelope::new(
            identity,
            4,
            0,
            envelope::Body::OpenRequest(OpenRequest {
                resource_id: Uuid::new_v4().as_bytes().to_vec(),
                message_revision: 1,
                read_only: false,
                request: Some(req),
            }),
            timeout,
        )
        .with_panel_id(3);
        let shutdown = Envelope::new(identity, 5, 1, envelope::Body::Shutdown(Empty {}), timeout);
        for frame in [&hello, &ready, &expired_open2, &valid_open3, &shutdown] {
            incoming_bytes.extend_from_slice(&encode_frame(frame).unwrap());
        }

        let mut outgoing_bytes = Vec::new();
        let mut plugin = MultiplexPlugin {
            panels: HashMap::new(),
        };
        run_with_io(incoming_bytes.as_slice(), &mut outgoing_bytes, &mut plugin).unwrap();

        let mut cursor = outgoing_bytes.as_slice();
        let mut replies = Vec::new();
        while !cursor.is_empty() {
            replies.push(read_frame(&mut cursor).unwrap());
        }
        assert_eq!(replies.len(), 3);
        assert!(matches!(replies[0].body, Some(envelope::Body::Register(_))));
        assert_eq!(replies[1].panel_id, 2);
        assert_eq!(replies[1].in_reply_to, 3);
        assert!(matches!(
            replies[1].body,
            Some(envelope::Body::Error(ref f)) if f.code == ErrorCode::Deadline as i32
        ));
        assert_eq!(replies[2].panel_id, 3);
        assert_eq!(replies[2].in_reply_to, 4);
        assert!(matches!(replies[2].body, Some(envelope::Body::Snapshot(_))));
    }

    struct CloseDestroyPlugin {
        closed: Vec<(u64, u64)>,
        destroyed: Vec<(u64, u64)>,
    }

    impl MioPlugin for CloseDestroyPlugin {
        fn id(&self) -> &str {
            "org.mio.sdk-close-destroy"
        }

        fn supports_panel_multiplexing(&self) -> bool {
            true
        }

        fn on_panel_open_request(
            &mut self,
            ctx: &mut dyn HostClient,
            panel_id: u64,
            _open: OpenRequest,
        ) -> Result<Option<PanelSnapshot>, String> {
            let rev = ctx.next_panel_revision();
            Ok(Some(ui::snapshot(
                rev,
                self.id(),
                ui::text(1, "p", format!("p-{panel_id}"), panel::Tone::Foreground),
            )))
        }

        fn on_panel_close_request(
            &mut self,
            ctx: &mut dyn HostClient,
            panel_id: u64,
            _close: CloseRequest,
        ) -> Result<Option<PanelSnapshot>, String> {
            self.closed.push((panel_id, ctx.current_panel_revision()));
            Ok(None)
        }

        fn on_panel_destroy(
            &mut self,
            ctx: &mut dyn HostClient,
            panel_id: u64,
            _close: CloseRequest,
        ) -> Result<Option<PanelSnapshot>, String> {
            self.destroyed.push((panel_id, ctx.current_panel_revision()));
            Ok(None)
        }
    }

    #[test]
    fn sdk_close_scope_preserves_panel_on_resource_revoke_and_destroys_on_expired_destroy() {
        let identity = ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: ProjectKey {
                id: Uuid::new_v4(),
                generation: 1,
            },
        };
        let timeout = Duration::from_secs(5);
        let req = HttpRequestDto {
            method: "GET".into(),
            url: "https://example.invalid/".into(),
            version: HttpVersionDto::Http11 as i32,
            ..Default::default()
        };

        let mut incoming_bytes = Vec::new();
        let hello = Envelope::new(
            identity,
            1,
            0,
            envelope::Body::Hello(Hello {
                max_frame_bytes: MAX_FRAME_BYTES as u32,
                max_operation_ms: 5000,
                granted_permissions: vec![],
                host_minor: mio_plugin_api::protocol::PROTOCOL_MINOR,
            }),
            timeout,
        );
        let mut ready = Envelope::new(identity, 2, 1, envelope::Body::Ready(Empty {}), timeout);
        ready.in_reply_to = 1;
        let open2 = Envelope::new(
            identity,
            3,
            0,
            envelope::Body::OpenRequest(OpenRequest {
                resource_id: Uuid::new_v4().as_bytes().to_vec(),
                message_revision: 1,
                read_only: false,
                request: Some(req),
            }),
            timeout,
        )
        .with_panel_id(2);
        // Resource-only close (destroy_panel = false) keeps panel revision 1 intact.
        let close_resource = Envelope::new(
            identity,
            4,
            1,
            envelope::Body::CloseRequest(CloseRequest {
                resource_id: Uuid::new_v4().as_bytes().to_vec(),
                destroy_panel: false,
            }),
            timeout,
        )
        .with_panel_id(2);
        // Expired CloseRequest with destroy_panel = true STILL executes on_panel_destroy and replies Ack!
        let mut destroy_expired = Envelope::new(
            identity,
            5,
            1,
            envelope::Body::CloseRequest(CloseRequest {
                resource_id: Uuid::new_v4().as_bytes().to_vec(),
                destroy_panel: true,
            }),
            timeout,
        )
        .with_panel_id(2);
        destroy_expired.deadline_unix_ms = 1;
        let shutdown = Envelope::new(identity, 6, 1, envelope::Body::Shutdown(Empty {}), timeout);
        for frame in [
            &hello,
            &ready,
            &open2,
            &close_resource,
            &destroy_expired,
            &shutdown,
        ] {
            incoming_bytes.extend_from_slice(&encode_frame(frame).unwrap());
        }

        let mut outgoing_bytes = Vec::new();
        let mut plugin = CloseDestroyPlugin {
            closed: Vec::new(),
            destroyed: Vec::new(),
        };
        run_with_io(incoming_bytes.as_slice(), &mut outgoing_bytes, &mut plugin).unwrap();
        assert_eq!(plugin.closed, vec![(2, 1)]);
        // Panel 2 preserved rev=1 across close_resource, then ran on_panel_destroy with rev=1 even when expired!
        assert_eq!(plugin.destroyed, vec![(2, 1)]);

        let mut cursor = outgoing_bytes.as_slice();
        let mut replies = Vec::new();
        while !cursor.is_empty() {
            replies.push(read_frame(&mut cursor).unwrap());
        }
        assert_eq!(replies.len(), 4);
        assert!(matches!(replies[1].body, Some(envelope::Body::Snapshot(_))));
        assert!(matches!(replies[2].body, Some(envelope::Body::Ack(_))));
        assert!(matches!(replies[3].body, Some(envelope::Body::Ack(_))));
        assert_eq!(replies[3].in_reply_to, 5);
    }

    struct ResyncRevisionPlugin {
        seen_revs: Vec<u64>,
    }

    impl MioPlugin for ResyncRevisionPlugin {
        fn id(&self) -> &str {
            "org.mio.sdk-resync-rev"
        }

        fn supports_panel_multiplexing(&self) -> bool {
            true
        }

        fn on_panel_open_request(
            &mut self,
            ctx: &mut dyn HostClient,
            _panel_id: u64,
            _open: OpenRequest,
        ) -> Result<Option<PanelSnapshot>, String> {
            let rev = ctx.next_panel_revision();
            Ok(Some(ui::snapshot(
                rev,
                self.id(),
                ui::text(1, "p", format!("open-{rev}"), panel::Tone::Foreground),
            )))
        }

        fn on_panel_ui_event(
            &mut self,
            ctx: &mut dyn HostClient,
            _panel_id: u64,
            _event: panel::UiEvent,
        ) -> Result<Option<PanelSnapshot>, String> {
            if ctx.current_panel_revision() >= 3 {
                let _ = ctx.is_in_scope("https://example.invalid/after")?;
                return Ok(None);
            }
            let rev = ctx.next_panel_revision();
            Ok(Some(ui::snapshot(
                rev,
                self.id(),
                ui::text(1, "p", format!("ui-{rev}"), panel::Tone::Foreground),
            )))
        }

        fn on_panel_resync(
            &mut self,
            ctx: &mut dyn HostClient,
            _panel_id: u64,
            snapshot: PanelSnapshot,
        ) -> Result<Option<PanelSnapshot>, String> {
            self.seen_revs.push(ctx.current_panel_revision());
            let _ = ctx.is_in_scope("https://example.invalid/resync")?;
            let rev = ctx.next_panel_revision();
            assert_eq!(snapshot.revision, 1);
            assert!(rev > 2, "pending must stay monotonic above discarded rev=2");
            Ok(Some(ui::snapshot(
                rev,
                self.id(),
                ui::text(1, "p", format!("resync-{rev}"), panel::Tone::Foreground),
            )))
        }
    }

    #[test]
    fn sdk_resync_resets_published_revision_while_preserving_monotonic_pending() {
        let identity = ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: ProjectKey {
                id: Uuid::new_v4(),
                generation: 1,
            },
        };
        let timeout = Duration::from_secs(5);
        let req = HttpRequestDto {
            method: "GET".into(),
            url: "https://example.invalid/".into(),
            version: HttpVersionDto::Http11 as i32,
            ..Default::default()
        };

        let mut incoming_bytes = Vec::new();
        let mut push_host =
            |panel_id: u64, req_id: u64, rev: u64, in_reply_to: u64, body: envelope::Body| {
                let mut env =
                    Envelope::new(identity, req_id, rev, body, timeout).with_panel_id(panel_id);
                env.in_reply_to = in_reply_to;
                incoming_bytes.extend_from_slice(&encode_frame(&env).unwrap());
            };

        push_host(
            PANEL_ID,
            1,
            0,
            0,
            envelope::Body::Hello(Hello {
                max_frame_bytes: MAX_FRAME_BYTES as u32,
                max_operation_ms: 5000,
                granted_permissions: vec![Permission::TrafficRead as i32],
                host_minor: mio_plugin_api::protocol::PROTOCOL_MINOR,
            }),
        );
        push_host(PANEL_ID, 2, 1, 1, envelope::Body::Ready(Empty {}));
        // 1. Open on child panel 2 -> publishes rev=1.
        push_host(
            2,
            3,
            0,
            0,
            envelope::Body::OpenRequest(OpenRequest {
                resource_id: Uuid::new_v4().as_bytes().to_vec(),
                message_revision: 1,
                read_only: false,
                request: Some(req),
            }),
        );
        // 2. UiEvent -> plugin emits Snapshot(rev=2), host rejects/discards it staying at rev=1.
        push_host(
            2,
            4,
            1,
            0,
            envelope::Body::UiEvent(panel::UiEvent {
                node_id: 1,
                editors: vec![],
                event: None,
            }),
        );
        // 3. Host sends Resync(snapshot rev=1) on child panel 2 -> plugin calls is_in_scope & emits rev=3.
        push_host(
            2,
            5,
            1,
            0,
            envelope::Body::Resync(ui::snapshot(
                1,
                "host",
                ui::text(1, "p", "host-confirmed", panel::Tone::Foreground),
            )),
        );
        push_host(
            2,
            6,
            1,
            4,
            envelope::Body::HostResult(mio_plugin_api::protocol::HostResult {
                outcome: Some(host_result::Outcome::ScopeStatus(ScopeStatusDto {
                    in_scope: true,
                })),
            }),
        );
        // 4. Subsequent UiEvent at rev=3 calls is_in_scope -> HostCall carries rev=3.
        push_host(
            2,
            7,
            3,
            0,
            envelope::Body::UiEvent(panel::UiEvent {
                node_id: 1,
                editors: vec![],
                event: None,
            }),
        );
        push_host(
            2,
            8,
            3,
            6,
            envelope::Body::HostResult(mio_plugin_api::protocol::HostResult {
                outcome: Some(host_result::Outcome::ScopeStatus(ScopeStatusDto {
                    in_scope: true,
                })),
            }),
        );
        push_host(PANEL_ID, 9, 1, 0, envelope::Body::Shutdown(Empty {}));

        let mut outgoing_bytes = Vec::new();
        let mut plugin = ResyncRevisionPlugin {
            seen_revs: Vec::new(),
        };
        run_with_io(incoming_bytes.as_slice(), &mut outgoing_bytes, &mut plugin).unwrap();
        assert_eq!(plugin.seen_revs, vec![1]);

        let mut cursor = outgoing_bytes.as_slice();
        let mut replies = Vec::new();
        while !cursor.is_empty() {
            replies.push(read_frame(&mut cursor).unwrap());
        }
        // Register, Snap(1), Snap(2), HostCall(resync), Snap(3), HostCall(after), Ack
        assert_eq!(replies.len(), 7);
        assert_eq!(replies[3].panel_id, 2);
        assert_eq!(
            replies[3].revision, 1,
            "HostCall inside on_panel_resync MUST carry host-confirmed revision=1, not discarded 2"
        );
        assert!(matches!(
            replies[3].body,
            Some(envelope::Body::HostCall(ref hc)) if hc.parent_request_id == 5
        ));
        assert_eq!(replies[4].panel_id, 2);
        assert_eq!(replies[4].revision, 3);
        assert_eq!(replies[5].panel_id, 2);
        assert_eq!(replies[5].revision, 3);
        assert!(matches!(replies[6].body, Some(envelope::Body::Ack(_))));
    }

    struct CancelAckAndDestroyedGuardPlugin {
        cancel_panels: Vec<u64>,
    }

    impl MioPlugin for CancelAckAndDestroyedGuardPlugin {
        fn id(&self) -> &str {
            "org.mio.sdk-cancel-guard"
        }

        fn supports_panel_multiplexing(&self) -> bool {
            true
        }

        fn on_panel_open_request(
            &mut self,
            ctx: &mut dyn HostClient,
            panel_id: u64,
            _open: OpenRequest,
        ) -> Result<Option<PanelSnapshot>, String> {
            let rev = ctx.next_panel_revision();
            Ok(Some(ui::snapshot(
                rev,
                self.id(),
                ui::text(1, "p", format!("p-{panel_id}"), panel::Tone::Foreground),
            )))
        }

        fn on_cancel(
            &mut self,
            ctx: &mut dyn HostClient,
            _cancel: Cancel,
        ) -> Result<Option<PanelSnapshot>, String> {
            let panel_id = ctx.panel_id();
            self.cancel_panels.push(panel_id);
            let next = ctx.next_panel_revision();
            // Even if a plugin returns a Snapshot from on_cancel, the runtime MUST send Ack without bumping published!
            Ok(Some(ui::snapshot(
                next,
                self.id(),
                ui::text(1, "p", "ignored-cancel-snap", panel::Tone::Foreground),
            )))
        }

        fn on_panel_ui_event(
            &mut self,
            ctx: &mut dyn HostClient,
            _panel_id: u64,
            _event: panel::UiEvent,
        ) -> Result<Option<PanelSnapshot>, String> {
            let _ = ctx.is_in_scope("https://example.invalid/after-cancel")?;
            Ok(None)
        }
    }

    #[test]
    fn sdk_cancel_ignores_destroyed_panel_and_replies_ack_without_bumping_revision() {
        let identity = ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: ProjectKey {
                id: Uuid::new_v4(),
                generation: 1,
            },
        };
        let timeout = Duration::from_secs(5);
        let req = HttpRequestDto {
            method: "GET".into(),
            url: "https://example.invalid/".into(),
            version: HttpVersionDto::Http11 as i32,
            ..Default::default()
        };

        let mut incoming_bytes = Vec::new();
        let mut push_host =
            |panel_id: u64, req_id: u64, rev: u64, in_reply_to: u64, body: envelope::Body| {
                let mut env =
                    Envelope::new(identity, req_id, rev, body, timeout).with_panel_id(panel_id);
                env.in_reply_to = in_reply_to;
                incoming_bytes.extend_from_slice(&encode_frame(&env).unwrap());
            };

        push_host(
            PANEL_ID,
            1,
            0,
            0,
            envelope::Body::Hello(Hello {
                max_frame_bytes: MAX_FRAME_BYTES as u32,
                max_operation_ms: 5000,
                granted_permissions: vec![Permission::TrafficRead as i32],
                host_minor: mio_plugin_api::protocol::PROTOCOL_MINOR,
            }),
        );
        push_host(PANEL_ID, 2, 1, 1, envelope::Body::Ready(Empty {}));
        // Open child panel 2 -> rev = 1.
        push_host(
            2,
            3,
            0,
            0,
            envelope::Body::OpenRequest(OpenRequest {
                resource_id: Uuid::new_v4().as_bytes().to_vec(),
                message_revision: 1,
                read_only: false,
                request: Some(req),
            }),
        );
        // Live Cancel on panel 2: on_cancel returns Some(snap(2)), SDK MUST emit Ack(rev=1) only.
        push_host(2, 4, 1, 0, envelope::Body::Cancel(Cancel { request_id: 99 }));
        // Subsequent UiEvent on panel 2 triggers is_in_scope HostCall -> revision MUST still be 1!
        push_host(
            2,
            5,
            1,
            0,
            envelope::Body::UiEvent(panel::UiEvent {
                node_id: 1,
                editors: vec![],
                event: None,
            }),
        );
        push_host(
            2,
            6,
            1,
            4,
            envelope::Body::HostResult(mio_plugin_api::protocol::HostResult {
                outcome: Some(host_result::Outcome::ScopeStatus(ScopeStatusDto {
                    in_scope: true,
                })),
            }),
        );
        // Destroy child panel 2, then send trailing Cancel on panel 2 -> on_cancel must NOT run!
        push_host(
            2,
            7,
            1,
            0,
            envelope::Body::CloseRequest(CloseRequest {
                resource_id: Uuid::new_v4().as_bytes().to_vec(),
                destroy_panel: true,
            }),
        );
        push_host(2, 8, 1, 0, envelope::Body::Cancel(Cancel { request_id: 7 }));
        push_host(PANEL_ID, 9, 1, 0, envelope::Body::Shutdown(Empty {}));

        let mut outgoing_bytes = Vec::new();
        let mut plugin = CancelAckAndDestroyedGuardPlugin {
            cancel_panels: Vec::new(),
        };
        run_with_io(incoming_bytes.as_slice(), &mut outgoing_bytes, &mut plugin).unwrap();
        assert_eq!(plugin.cancel_panels, vec![2]);

        let mut cursor = outgoing_bytes.as_slice();
        let mut replies = Vec::new();
        while !cursor.is_empty() {
            replies.push(read_frame(&mut cursor).unwrap());
        }
        // Register(1), Snap(2), CancelAck(3), HostCall(4), UiAck(5), DestroyAck(6), StaleCancelAck(7)
        assert_eq!(replies.len(), 7);
        assert!(matches!(replies[2].body, Some(envelope::Body::Ack(_))));
        assert_eq!(replies[2].revision, 1);
        assert!(matches!(replies[3].body, Some(envelope::Body::HostCall(_))));
        assert_eq!(
            replies[3].revision, 1,
            "on_cancel returning a Snapshot must not advance published revision"
        );
        assert!(matches!(replies[6].body, Some(envelope::Body::Ack(_))));
        assert_eq!(replies[6].in_reply_to, 8);
    }

    #[test]
    fn sdk_default_on_resync_returns_error_without_forging_host_revision_ack() {
        struct NoResyncPlugin {
            bumped: bool,
        }

        impl MioPlugin for NoResyncPlugin {
            fn id(&self) -> &str {
                "org.mio.sdk-no-resync"
            }

            fn on_panel_ui_event(
                &mut self,
                ctx: &mut dyn HostClient,
                _panel_id: u64,
                _event: panel::UiEvent,
            ) -> Result<Option<PanelSnapshot>, String> {
                if !self.bumped {
                    self.bumped = true;
                    let rev = ctx.next_panel_revision();
                    return Ok(Some(ui::snapshot(
                        rev,
                        self.id(),
                        ui::text(1, "p", format!("v-{rev}"), panel::Tone::Foreground),
                    )));
                }
                Ok(None)
            }
        }

        let identity = ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: ProjectKey {
                id: Uuid::new_v4(),
                generation: 1,
            },
        };
        let timeout = Duration::from_secs(5);
        let mut incoming_bytes = Vec::new();
        let mut push_host =
            |panel_id: u64, req_id: u64, rev: u64, in_reply_to: u64, body: envelope::Body| {
                let mut env = Envelope::new(identity, req_id, rev, body, timeout);
                env.panel_id = panel_id;
                env.in_reply_to = in_reply_to;
                incoming_bytes.extend_from_slice(&encode_frame(&env).unwrap());
            };
        push_host(
            PANEL_ID,
            1,
            0,
            0,
            envelope::Body::Hello(Hello {
                max_frame_bytes: MAX_FRAME_BYTES as u32,
                max_operation_ms: 5000,
                granted_permissions: Vec::new(),
                host_minor: mio_plugin_api::protocol::PROTOCOL_MINOR,
            }),
        );
        push_host(PANEL_ID, 2, 1, 1, envelope::Body::Ready(Empty {}));
        push_host(
            PANEL_ID,
            3,
            1,
            0,
            envelope::Body::UiEvent(panel::UiEvent {
                node_id: 1,
                editors: vec![],
                event: Some(panel::ui_event::Event::Click(Empty {})),
            }),
        );
        push_host(
            PANEL_ID,
            4,
            1,
            0,
            envelope::Body::Resync(ui::snapshot(
                1,
                "org.mio.sdk-no-resync",
                ui::text(1, "p", "host-v1", panel::Tone::Foreground),
            )),
        );
        push_host(
            PANEL_ID,
            5,
            2,
            0,
            envelope::Body::UiEvent(panel::UiEvent {
                node_id: 1,
                editors: vec![],
                event: Some(panel::ui_event::Event::Click(Empty {})),
            }),
        );
        push_host(PANEL_ID, 6, 2, 0, envelope::Body::Shutdown(Empty {}));

        let mut outgoing_bytes = Vec::new();
        let mut plugin = NoResyncPlugin { bumped: false };
        run_with_io(incoming_bytes.as_slice(), &mut outgoing_bytes, &mut plugin).unwrap();

        let mut cursor = outgoing_bytes.as_slice();
        let mut replies = Vec::new();
        while !cursor.is_empty() {
            replies.push(read_frame(&mut cursor).unwrap());
        }
        // Register(1), Snap v2(2), Resync Error(3), UiEvent Ack v2(4)
        assert_eq!(replies.len(), 4);
        assert!(matches!(replies[1].body, Some(envelope::Body::Snapshot(_))));
        assert_eq!(replies[1].revision, 2);
        assert!(matches!(
            replies[2].body,
            Some(envelope::Body::Error(ref f)) if f.code == ErrorCode::Invalid as i32
        ));
        assert_eq!(
            replies[2].revision, 2,
            "failed default on_resync must preserve previous published revision=2, never claim host rev=1"
        );
        assert!(matches!(replies[3].body, Some(envelope::Body::Ack(_))));
        assert_eq!(replies[3].revision, 2);
    }
}

impl<T: MioPlugin + ?Sized> MioPlugin for &mut T {
    fn id(&self) -> &str {
        (**self).id()
    }
    fn version(&self) -> &str {
        (**self).version()
    }
    fn required_components(&self) -> Vec<Component> {
        (**self).required_components()
    }
    fn initial_panel(&self) -> PanelSnapshot {
        (**self).initial_panel()
    }
    fn on_ui_event(
        &mut self,
        ctx: &mut dyn HostClient,
        event: panel::UiEvent,
    ) -> Result<Option<PanelSnapshot>, String> {
        (**self).on_ui_event(ctx, event)
    }
    fn on_open_request(
        &mut self,
        ctx: &mut dyn HostClient,
        open: OpenRequest,
    ) -> Result<Option<PanelSnapshot>, String> {
        (**self).on_open_request(ctx, open)
    }
    fn on_open_response(
        &mut self,
        ctx: &mut dyn HostClient,
        open: OpenResponse,
    ) -> Result<Option<PanelSnapshot>, String> {
        (**self).on_open_response(ctx, open)
    }
    fn on_close_request(
        &mut self,
        ctx: &mut dyn HostClient,
        close: CloseRequest,
    ) -> Result<Option<PanelSnapshot>, String> {
        (**self).on_close_request(ctx, close)
    }
    fn on_commit_editor(
        &mut self,
        ctx: &mut dyn HostClient,
        commit: CommitEditor,
    ) -> Result<Committed, String> {
        (**self).on_commit_editor(ctx, commit)
    }
    fn on_commit_response_editor(
        &mut self,
        ctx: &mut dyn HostClient,
        commit: CommitResponseEditor,
    ) -> Result<ResponseCommitted, String> {
        (**self).on_commit_response_editor(ctx, commit)
    }
    fn on_context_menu(
        &mut self,
        ctx: &mut dyn HostClient,
        call: ContextMenuCall,
    ) -> Result<ContextMenuResult, String> {
        (**self).on_context_menu(ctx, call)
    }
    fn on_http_hook(
        &mut self,
        ctx: &mut dyn HostClient,
        call: HttpHookCall,
    ) -> Result<HttpHookResult, String> {
        (**self).on_http_hook(ctx, call)
    }
    fn on_ws_hook(
        &mut self,
        ctx: &mut dyn HostClient,
        call: WebSocketHookCall,
    ) -> Result<WebSocketHookResult, String> {
        (**self).on_ws_hook(ctx, call)
    }
    fn on_session_action(
        &mut self,
        ctx: &mut dyn HostClient,
        call: SessionActionCall,
    ) -> Result<SessionActionResult, String> {
        (**self).on_session_action(ctx, call)
    }
    fn on_payload(
        &mut self,
        ctx: &mut dyn HostClient,
        call: PayloadCall,
    ) -> Result<PayloadResult, String> {
        (**self).on_payload(ctx, call)
    }
    fn on_passive_check(
        &mut self,
        ctx: &mut dyn HostClient,
        call: PassiveCheckCall,
    ) -> Result<PassiveCheckResult, String> {
        (**self).on_passive_check(ctx, call)
    }
    fn on_active_check(
        &mut self,
        ctx: &mut dyn HostClient,
        call: ActiveCheckCall,
    ) -> Result<ActiveCheckResult, String> {
        (**self).on_active_check(ctx, call)
    }
    fn on_cancel(
        &mut self,
        ctx: &mut dyn HostClient,
        cancel: Cancel,
    ) -> Result<Option<PanelSnapshot>, String> {
        (**self).on_cancel(ctx, cancel)
    }
    fn on_resync(
        &mut self,
        ctx: &mut dyn HostClient,
        snapshot: PanelSnapshot,
    ) -> Result<Option<PanelSnapshot>, String> {
        (**self).on_resync(ctx, snapshot)
    }
    fn supports_panel_multiplexing(&self) -> bool {
        (**self).supports_panel_multiplexing()
    }
    fn on_panel_ui_event(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        event: panel::UiEvent,
    ) -> Result<Option<PanelSnapshot>, String> {
        (**self).on_panel_ui_event(ctx, panel_id, event)
    }
    fn on_panel_open_request(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        open: OpenRequest,
    ) -> Result<Option<PanelSnapshot>, String> {
        (**self).on_panel_open_request(ctx, panel_id, open)
    }
    fn on_panel_open_response(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        open: OpenResponse,
    ) -> Result<Option<PanelSnapshot>, String> {
        (**self).on_panel_open_response(ctx, panel_id, open)
    }
    fn on_panel_close_request(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        close: CloseRequest,
    ) -> Result<Option<PanelSnapshot>, String> {
        (**self).on_panel_close_request(ctx, panel_id, close)
    }
    fn on_panel_destroy(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        close: CloseRequest,
    ) -> Result<Option<PanelSnapshot>, String> {
        (**self).on_panel_destroy(ctx, panel_id, close)
    }
    fn on_panel_commit_editor(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        commit: CommitEditor,
    ) -> Result<Committed, String> {
        (**self).on_panel_commit_editor(ctx, panel_id, commit)
    }
    fn on_panel_commit_response_editor(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        commit: CommitResponseEditor,
    ) -> Result<ResponseCommitted, String> {
        (**self).on_panel_commit_response_editor(ctx, panel_id, commit)
    }
    fn on_panel_resync(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        snapshot: PanelSnapshot,
    ) -> Result<Option<PanelSnapshot>, String> {
        (**self).on_panel_resync(ctx, panel_id, snapshot)
    }
}
