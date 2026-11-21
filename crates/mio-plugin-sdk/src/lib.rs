use std::{
    collections::HashMap,
    io::{self, Read, Write},
};

pub use mio_plugin_api::{
    self, FlowRecord, FlowSummary, Header, HttpRequest, HttpResponse, HttpVersion, Uuid, panel,
    protocol,
};
use mio_plugin_api::{
    panel::{Empty, PanelPatch, PanelSnapshot},
    protocol::{
        ActiveCheckCall, ActiveCheckResult, Cancel, CloseRequest, CommitEditor,
        CommitResponseEditor, Committed, Component, ContextMenuCall, ContextMenuResult,
        ErrorCode, Failure, HookAction, HostResult, HttpHookCall, HttpHookResult, OpenRequest,
        OpenResponse, PANEL_ID, PassiveCheckCall, PassiveCheckResult, PayloadCall, PayloadResult,
        Register, ResponseCommitted, SessionActionCall, SessionActionResult, WebSocketHookCall,
        WebSocketHookResult, WorkerSession, envelope,
    },
};

pub mod context;
#[cfg(test)]
mod tests;
pub mod ui;

pub use context::{HostClient, HostContext, MAX_DEFERRED_ENVELOPES};
use context::{
    DeferredQueue, PanelRevisionState, emit_panel_update, is_prunable_expired, send_deadline_error,
};
pub use ui::FindingBuilder;

/// Return value for panel and lifecycle callbacks: emit nothing (`Ack`), replace the panel (`Snapshot`), or mutate nodes incrementally (`Patch`).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum PanelUpdate {
    #[default]
    None,
    Snapshot(PanelSnapshot),
    Patch(PanelPatch),
}

impl From<PanelSnapshot> for PanelUpdate {
    fn from(snapshot: PanelSnapshot) -> Self {
        Self::Snapshot(snapshot)
    }
}

impl From<PanelPatch> for PanelUpdate {
    fn from(patch: PanelPatch) -> Self {
        Self::Patch(patch)
    }
}

impl From<Option<PanelSnapshot>> for PanelUpdate {
    fn from(snapshot: Option<PanelSnapshot>) -> Self {
        match snapshot {
            Some(snap) => Self::Snapshot(snap),
            None => Self::None,
        }
    }
}

pub trait MioPlugin {
    fn id(&self) -> &str;

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn required_components(&self) -> Vec<Component> {
        vec![Component::Text]
    }

    fn supports_panel_multiplexing(&self) -> bool {
        false
    }

    fn initial_panel(&self) -> PanelSnapshot {
        ui::snapshot(
            1,
            self.id(),
            ui::text(1, "Status", "Ready", panel::Tone::Foreground),
        )
    }

    /// Invoked once immediately after the `Hello` / `Register` / `Ready` handshake succeeds.
    fn on_ready(&mut self, _ctx: &mut dyn HostClient) -> Result<PanelUpdate, String> {
        Ok(PanelUpdate::None)
    }

    fn on_ui_event(
        &mut self,
        _ctx: &mut dyn HostClient,
        _panel_id: u64,
        _event: panel::UiEvent,
    ) -> Result<PanelUpdate, String> {
        Ok(PanelUpdate::None)
    }

    fn on_open_request(
        &mut self,
        _ctx: &mut dyn HostClient,
        _panel_id: u64,
        _open: OpenRequest,
    ) -> Result<PanelUpdate, String> {
        Ok(PanelUpdate::None)
    }

    fn on_open_response(
        &mut self,
        _ctx: &mut dyn HostClient,
        _panel_id: u64,
        _open: OpenResponse,
    ) -> Result<PanelUpdate, String> {
        Ok(PanelUpdate::None)
    }

    fn on_close_request(
        &mut self,
        _ctx: &mut dyn HostClient,
        _panel_id: u64,
        _close: CloseRequest,
    ) -> Result<PanelUpdate, String> {
        Ok(PanelUpdate::None)
    }

    fn on_panel_destroy(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        close: CloseRequest,
    ) -> Result<PanelUpdate, String> {
        self.on_close_request(ctx, panel_id, close)
    }

    fn on_commit_editor(
        &mut self,
        _ctx: &mut dyn HostClient,
        _panel_id: u64,
        _commit: CommitEditor,
    ) -> Result<Committed, String> {
        Err("plugin does not handle CommitEditor".into())
    }

    fn on_commit_response_editor(
        &mut self,
        _ctx: &mut dyn HostClient,
        _panel_id: u64,
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

    /// Invoked when an asynchronous `HostResult` arrives for a call started via `HostClient::start_host_call`.
    fn on_host_result(
        &mut self,
        _ctx: &mut dyn HostClient,
        _panel_id: u64,
        _request_id: u64,
        _result: HostResult,
    ) -> Result<PanelUpdate, String> {
        Ok(PanelUpdate::None)
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
    /// host revision), whereas all mutating or side-effect calls (including `UpdateDraft`,
    /// `OpenRepeater`, `OpenIntruder`, `NetworkSend`, `ScopeUpdate`, `OpenOrganiser`,
    /// `UpdateAnnotations`, `SubmitAuditIssue`, and `Collaborator*`) are rejected with
    /// `ErrorCode::Cancelled`. The runtime always replies `Ack` (ignoring any returned
    /// `PanelUpdate`) or `Error`, which ends the cleanup window.
    fn on_cancel(
        &mut self,
        _ctx: &mut dyn HostClient,
        _panel_id: u64,
        _cancel: Cancel,
    ) -> Result<PanelUpdate, String> {
        Ok(PanelUpdate::None)
    }

    /// Invoked when the host requests panel resynchronization to its authoritative `snapshot`.
    ///
    /// Plugins that support resync recovery should override `on_resync`, adopt `snapshot`, and
    /// return `Ok(PanelUpdate::Snapshot(confirmed_snapshot))` (or `PanelUpdate::Patch`) at
    /// `ctx.next_panel_revision()` to confirm authoritative UI semantics after drift (returning
    /// `Ok(PanelUpdate::None)` confirms only the revision without clearing semantic-drift
    /// protection). The default returns `Err(...)` so plugins that do not apply `snapshot` never
    /// falsely acknowledge the host's view.
    fn on_resync(
        &mut self,
        _ctx: &mut dyn HostClient,
        _panel_id: u64,
        _snapshot: PanelSnapshot,
    ) -> Result<PanelUpdate, String> {
        Err("plugin does not implement on_resync".into())
    }
}

pub fn run_stdio(plugin: impl MioPlugin) -> io::Result<()> {
    run_with_io(io::stdin().lock(), io::stdout().lock(), plugin)
}

pub fn run_with_io<R: Read, W: Write>(
    reader: R,
    writer: W,
    plugin: impl MioPlugin,
) -> io::Result<()> {
    run_with_io_bounded(reader, writer, plugin, MAX_DEFERRED_ENVELOPES)
}

struct Runner<R: Read, W: Write> {
    session: WorkerSession<R, W>,
    deferred: DeferredQueue,
    panel_revisions: HashMap<u64, PanelRevisionState>,
    initial_rev: u64,
}

impl<R: Read, W: Write> Runner<R, W> {
    fn with_context<T>(
        &mut self,
        panel_id: u64,
        parent_request_id: u64,
        allow_panel_reply: bool,
        f: impl FnOnce(&mut HostContext<'_, R, W>) -> T,
    ) -> (T, bool, bool, Vec<PanelUpdate>) {
        let mut post_reply_updates = Vec::new();
        let (out, replied, cancelled) = {
            let mut ctx = HostContext::new(
                &mut self.session,
                &mut self.deferred,
                panel_id,
                parent_request_id,
                allow_panel_reply,
                &mut self.panel_revisions,
                &mut post_reply_updates,
            );
            let out = f(&mut ctx);
            (out, ctx.replied, ctx.cancelled)
        };
        (out, replied, cancelled, post_reply_updates)
    }

    fn finish_panel_call(
        &mut self,
        panel_id: u64,
        reply_to: u64,
        allow_panel_reply: bool,
        replied: bool,
        cancelled: bool,
        outcome: Result<PanelUpdate, String>,
        post_reply_updates: Vec<PanelUpdate>,
    ) -> io::Result<()> {
        if self.deferred.shutdown_requested {
            return Ok(());
        }
        let primary_reply_to = if allow_panel_reply && !replied {
            reply_to
        } else {
            0
        };
        match outcome {
            Ok(update) => {
                if !matches!(update, PanelUpdate::None) || primary_reply_to != 0 {
                    emit_panel_update(
                        &mut self.session,
                        panel_id,
                        &mut self.panel_revisions,
                        primary_reply_to,
                        update,
                    )?;
                }
            }
            Err(msg) => {
                if primary_reply_to == 0 {
                    return Err(io::Error::other(msg));
                }
                let code = if cancelled {
                    ErrorCode::Cancelled
                } else {
                    ErrorCode::Invalid
                };
                let rev = self
                    .panel_revisions
                    .get(&panel_id)
                    .map_or(0, |s| s.published);
                self.session.send_on_panel(
                    panel_id,
                    envelope::Body::Error(Failure::new(code, msg)),
                    rev,
                    primary_reply_to,
                )?;
            }
        }
        for extra in post_reply_updates {
            if !matches!(extra, PanelUpdate::None) {
                emit_panel_update(
                    &mut self.session,
                    panel_id,
                    &mut self.panel_revisions,
                    0,
                    extra,
                )?;
            }
        }
        Ok(())
    }

    fn dispatch_panel(
        &mut self,
        panel_id: u64,
        incoming_rev: u64,
        reply_to: u64,
        f: impl FnOnce(&mut HostContext<'_, R, W>) -> Result<PanelUpdate, String>,
    ) -> io::Result<()> {
        self.panel_revisions
            .entry(panel_id)
            .or_insert_with(|| PanelRevisionState::new(incoming_rev));
        let (res, replied, cancelled, post_reply_updates) =
            self.with_context(panel_id, reply_to, true, f);
        self.finish_panel_call(
            panel_id,
            reply_to,
            true,
            replied,
            cancelled,
            res,
            post_reply_updates,
        )
    }

    fn dispatch_typed<T>(
        &mut self,
        panel_id: u64,
        default_rev: u64,
        reply_to: u64,
        err_code: ErrorCode,
        f: impl FnOnce(&mut HostContext<'_, R, W>) -> Result<T, String>,
        wrap: impl FnOnce(T) -> envelope::Body,
    ) -> io::Result<()> {
        self.panel_revisions
            .entry(panel_id)
            .or_insert_with(|| PanelRevisionState::new(default_rev));
        let (out, _replied, cancelled, post_reply_updates) =
            self.with_context(panel_id, reply_to, false, f);
        if self.deferred.shutdown_requested {
            return Ok(());
        }
        let wire = match out {
            Ok(val) => wrap(val),
            Err(msg) => {
                let code = if cancelled {
                    ErrorCode::Cancelled
                } else {
                    err_code
                };
                envelope::Body::Error(Failure::new(code, msg))
            }
        };
        let rev = self
            .panel_revisions
            .get(&panel_id)
            .map_or(default_rev, |s| s.published);
        self.session.send_on_panel(panel_id, wire, rev, reply_to)?;
        for extra in post_reply_updates {
            if !matches!(extra, PanelUpdate::None) {
                emit_panel_update(
                    &mut self.session,
                    panel_id,
                    &mut self.panel_revisions,
                    0,
                    extra,
                )?;
            }
        }
        Ok(())
    }
}

pub(crate) fn run_with_io_bounded<R: Read, W: Write>(
    reader: R,
    writer: W,
    mut plugin: impl MioPlugin,
    max_deferred: usize,
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
    let session = WorkerSession::accept(reader, writer, registration)?;
    let mut runner = Runner {
        session,
        deferred: DeferredQueue::new(max_deferred),
        panel_revisions: HashMap::from([(PANEL_ID, PanelRevisionState::new(initial_rev))]),
        initial_rev,
    };

    let (ready_res, ready_replied, ready_cancelled, ready_extra) =
        runner.with_context(PANEL_ID, 0, false, |ctx| plugin.on_ready(ctx));
    runner.finish_panel_call(
        PANEL_ID,
        0,
        false,
        ready_replied,
        ready_cancelled,
        ready_res,
        ready_extra,
    )?;
    if runner.deferred.shutdown_requested {
        return Ok(());
    }

    loop {
        let envelope = match runner.deferred.pop_next() {
            Some(env) => env,
            None => match runner.session.receive() {
                Ok(env) => env,
                Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(err) => return Err(err),
            },
        };
        if is_prunable_expired(&envelope) {
            if envelope.request_id != 0 {
                send_deadline_error(&mut runner.session, &envelope, &runner.panel_revisions)?;
            }
            continue;
        }
        let reply_to = envelope.request_id;
        let in_reply_to = envelope.in_reply_to;
        let panel_id = envelope.panel_id.max(PANEL_ID);
        let incoming_rev = envelope.revision;
        let body = envelope.body.ok_or_else(|| io::Error::other("空消息"))?;
        match body {
            envelope::Body::Shutdown(_) => break,
            envelope::Body::UiEvent(ev) => {
                runner.dispatch_panel(panel_id, incoming_rev, reply_to, |ctx| {
                    plugin.on_ui_event(ctx, panel_id, ev)
                })?;
            }
            envelope::Body::OpenRequest(open) => {
                runner.dispatch_panel(panel_id, incoming_rev, reply_to, |ctx| {
                    plugin.on_open_request(ctx, panel_id, open)
                })?;
            }
            envelope::Body::OpenResponse(open) => {
                runner.dispatch_panel(panel_id, incoming_rev, reply_to, |ctx| {
                    plugin.on_open_response(ctx, panel_id, open)
                })?;
            }
            envelope::Body::CloseRequest(close) => {
                let destroy = close.destroy_panel && panel_id != PANEL_ID;
                runner.dispatch_panel(panel_id, incoming_rev, reply_to, |ctx| {
                    if destroy {
                        plugin.on_panel_destroy(ctx, panel_id, close)
                    } else {
                        plugin.on_close_request(ctx, panel_id, close)
                    }
                })?;
                if destroy {
                    runner.panel_revisions.remove(&panel_id);
                }
            }
            envelope::Body::CommitEditor(commit) => {
                runner.dispatch_typed(
                    panel_id,
                    incoming_rev,
                    reply_to,
                    ErrorCode::Stale,
                    |ctx| plugin.on_commit_editor(ctx, panel_id, commit),
                    envelope::Body::Committed,
                )?;
            }
            envelope::Body::CommitResponseEditor(commit) => {
                runner.dispatch_typed(
                    panel_id,
                    incoming_rev,
                    reply_to,
                    ErrorCode::Stale,
                    |ctx| plugin.on_commit_response_editor(ctx, panel_id, commit),
                    envelope::Body::ResponseCommitted,
                )?;
            }
            envelope::Body::ContextMenuCall(call) => {
                runner.dispatch_typed(
                    PANEL_ID,
                    runner.initial_rev,
                    reply_to,
                    ErrorCode::Invalid,
                    |ctx| plugin.on_context_menu(ctx, call),
                    envelope::Body::ContextMenuResult,
                )?;
            }
            envelope::Body::HttpHookCall(call) => {
                runner.dispatch_typed(
                    PANEL_ID,
                    runner.initial_rev,
                    reply_to,
                    ErrorCode::Invalid,
                    |ctx| plugin.on_http_hook(ctx, call),
                    envelope::Body::HttpHookResult,
                )?;
            }
            envelope::Body::WebSocketHookCall(call) => {
                runner.dispatch_typed(
                    PANEL_ID,
                    runner.initial_rev,
                    reply_to,
                    ErrorCode::Invalid,
                    |ctx| plugin.on_ws_hook(ctx, call),
                    envelope::Body::WebSocketHookResult,
                )?;
            }
            envelope::Body::SessionActionCall(call) => {
                runner.dispatch_typed(
                    PANEL_ID,
                    runner.initial_rev,
                    reply_to,
                    ErrorCode::Invalid,
                    |ctx| plugin.on_session_action(ctx, call),
                    envelope::Body::SessionActionResult,
                )?;
            }
            envelope::Body::PayloadCall(call) => {
                runner.dispatch_typed(
                    PANEL_ID,
                    runner.initial_rev,
                    reply_to,
                    ErrorCode::Invalid,
                    |ctx| plugin.on_payload(ctx, call),
                    envelope::Body::PayloadResult,
                )?;
            }
            envelope::Body::PassiveCheckCall(call) => {
                runner.dispatch_typed(
                    PANEL_ID,
                    runner.initial_rev,
                    reply_to,
                    ErrorCode::Invalid,
                    |ctx| plugin.on_passive_check(ctx, call),
                    envelope::Body::PassiveCheckResult,
                )?;
            }
            envelope::Body::ActiveCheckCall(call) => {
                runner.dispatch_typed(
                    PANEL_ID,
                    runner.initial_rev,
                    reply_to,
                    ErrorCode::Invalid,
                    |ctx| plugin.on_active_check(ctx, call),
                    envelope::Body::ActiveCheckResult,
                )?;
            }
            envelope::Body::HostResult(result) => {
                if runner.deferred.take_abandoned(in_reply_to)
                    || (panel_id != PANEL_ID && !runner.panel_revisions.contains_key(&panel_id))
                {
                    continue;
                }
                let (res, replied, cancelled, post_reply_updates) =
                    runner.with_context(panel_id, 0, false, |ctx| {
                        plugin.on_host_result(ctx, panel_id, in_reply_to, result)
                    });
                runner.finish_panel_call(
                    panel_id,
                    0,
                    false,
                    replied,
                    cancelled,
                    res,
                    post_reply_updates,
                )?;
            }
            envelope::Body::Cancel(cancel) => {
                if panel_id != PANEL_ID && !runner.panel_revisions.contains_key(&panel_id) {
                    runner.session.send_on_panel(
                        panel_id,
                        envelope::Body::Ack(Empty {}),
                        incoming_rev,
                        reply_to,
                    )?;
                    continue;
                }
                runner
                    .panel_revisions
                    .entry(panel_id)
                    .or_insert_with(|| PanelRevisionState::new(incoming_rev));
                let (res, _replied, _cancelled, _post_reply_updates) = runner
                    .with_context(panel_id, reply_to, false, |ctx| {
                        plugin.on_cancel(ctx, panel_id, cancel)
                    });
                if runner.deferred.shutdown_requested {
                    break;
                }
                let rev = runner.panel_revisions[&panel_id].published;
                let wire = match res {
                    Ok(_) => envelope::Body::Ack(Empty {}),
                    Err(msg) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, msg)),
                };
                runner
                    .session
                    .send_on_panel(panel_id, wire, rev, reply_to)?;
            }
            envelope::Body::Resync(snapshot) => {
                let state = runner
                    .panel_revisions
                    .entry(panel_id)
                    .or_insert_with(|| PanelRevisionState::new(snapshot.revision));
                let prev_published = state.published;
                state.pending = state.pending.max(state.published).max(snapshot.revision);
                state.published = snapshot.revision;
                let (res, replied, cancelled, post_reply_updates) = runner
                    .with_context(panel_id, reply_to, true, |ctx| {
                        plugin.on_resync(ctx, panel_id, snapshot)
                    });
                if res.is_err()
                    && !replied
                    && let Some(state) = runner.panel_revisions.get_mut(&panel_id)
                {
                    state.published = prev_published;
                }
                runner.finish_panel_call(
                    panel_id,
                    reply_to,
                    true,
                    replied,
                    cancelled,
                    res,
                    post_reply_updates,
                )?;
            }
            _ => return Err(io::Error::other("worker 收到方向无效的消息")),
        }
        if runner.deferred.shutdown_requested {
            break;
        }
    }
    Ok(())
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
    fn supports_panel_multiplexing(&self) -> bool {
        (**self).supports_panel_multiplexing()
    }
    fn initial_panel(&self) -> PanelSnapshot {
        (**self).initial_panel()
    }
    fn on_ready(&mut self, ctx: &mut dyn HostClient) -> Result<PanelUpdate, String> {
        (**self).on_ready(ctx)
    }
    fn on_ui_event(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        event: panel::UiEvent,
    ) -> Result<PanelUpdate, String> {
        (**self).on_ui_event(ctx, panel_id, event)
    }
    fn on_open_request(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        open: OpenRequest,
    ) -> Result<PanelUpdate, String> {
        (**self).on_open_request(ctx, panel_id, open)
    }
    fn on_open_response(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        open: OpenResponse,
    ) -> Result<PanelUpdate, String> {
        (**self).on_open_response(ctx, panel_id, open)
    }
    fn on_close_request(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        close: CloseRequest,
    ) -> Result<PanelUpdate, String> {
        (**self).on_close_request(ctx, panel_id, close)
    }
    fn on_panel_destroy(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        close: CloseRequest,
    ) -> Result<PanelUpdate, String> {
        (**self).on_panel_destroy(ctx, panel_id, close)
    }
    fn on_commit_editor(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        commit: CommitEditor,
    ) -> Result<Committed, String> {
        (**self).on_commit_editor(ctx, panel_id, commit)
    }
    fn on_commit_response_editor(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        commit: CommitResponseEditor,
    ) -> Result<ResponseCommitted, String> {
        (**self).on_commit_response_editor(ctx, panel_id, commit)
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
    fn on_host_result(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        request_id: u64,
        result: HostResult,
    ) -> Result<PanelUpdate, String> {
        (**self).on_host_result(ctx, panel_id, request_id, result)
    }
    fn on_cancel(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        cancel: Cancel,
    ) -> Result<PanelUpdate, String> {
        (**self).on_cancel(ctx, panel_id, cancel)
    }
    fn on_resync(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        snapshot: PanelSnapshot,
    ) -> Result<PanelUpdate, String> {
        (**self).on_resync(ctx, panel_id, snapshot)
    }
}
