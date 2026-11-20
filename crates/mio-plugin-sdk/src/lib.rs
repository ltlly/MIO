use std::{
    collections::VecDeque,
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
        PassiveCheckCall, PassiveCheckResult, PayloadCall, PayloadResult, Register,
        ResponseCommitted, SessionActionCall, SessionActionResult, WebSocketHookCall,
        WebSocketHookResult, WorkerSession, envelope,
    },
};

pub mod context;
pub mod ui;

pub use context::{HostClient, HostContext};

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
        call: ActiveCheckCall,
    ) -> Result<ActiveCheckResult, String> {
        Ok(ActiveCheckResult {
            call_id: call.call_id,
            findings: Vec::new(),
        })
    }

    fn on_cancel(
        &mut self,
        _ctx: &mut dyn HostClient,
        _cancel: Cancel,
    ) -> Result<Option<PanelSnapshot>, String> {
        Ok(None)
    }

    fn on_resync(
        &mut self,
        _ctx: &mut dyn HostClient,
        _snapshot: PanelSnapshot,
    ) -> Result<Option<PanelSnapshot>, String> {
        Ok(None)
    }
}

pub fn run_stdio(plugin: impl MioPlugin) -> io::Result<()> {
    run_with_io(io::stdin().lock(), io::stdout().lock(), plugin)
}

fn reply_panel_or_ack<R: Read, W: Write>(
    session: &mut WorkerSession<R, W>,
    panel_revision: &mut u64,
    reply_to: u64,
    outcome: Result<Option<PanelSnapshot>, String>,
) -> io::Result<()> {
    match outcome {
        Ok(Some(snapshot)) => {
            *panel_revision = (*panel_revision).max(snapshot.revision).max(1);
            let rev = snapshot.revision;
            session.send(envelope::Body::Snapshot(snapshot), rev, reply_to)?;
        }
        Ok(None) => {
            session.send(envelope::Body::Ack(Empty {}), *panel_revision, reply_to)?;
        }
        Err(msg) => {
            session.send(
                envelope::Body::Error(Failure::new(ErrorCode::Invalid, msg)),
                *panel_revision,
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
    let mut panel_revision = initial.revision.max(1);
    let registration = Register {
        plugin_id: plugin.id().to_owned(),
        plugin_version: plugin.version().to_owned(),
        required_components: plugin
            .required_components()
            .into_iter()
            .map(|c| c as i32)
            .collect(),
        panel: Some(initial),
    };
    let mut session = WorkerSession::accept(reader, writer, registration)?;
    let mut deferred: VecDeque<Envelope> = VecDeque::new();

    loop {
        let envelope = match deferred.pop_front() {
            Some(env) => env,
            None => match session.receive() {
                Ok(env) => env,
                Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(err) => return Err(err),
            },
        };
        let reply_to = envelope.request_id;
        let body = envelope.body.ok_or_else(|| io::Error::other("空消息"))?;
        match body {
            envelope::Body::Shutdown(_) => break,
            envelope::Body::UiEvent(ev) => {
                let res = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_revision: &mut panel_revision,
                    };
                    plugin.on_ui_event(&mut ctx, ev)
                };
                reply_panel_or_ack(&mut session, &mut panel_revision, reply_to, res)?;
            }
            envelope::Body::OpenRequest(open) => {
                let res = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_revision: &mut panel_revision,
                    };
                    plugin.on_open_request(&mut ctx, open)
                };
                reply_panel_or_ack(&mut session, &mut panel_revision, reply_to, res)?;
            }
            envelope::Body::OpenResponse(open) => {
                let res = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_revision: &mut panel_revision,
                    };
                    plugin.on_open_response(&mut ctx, open)
                };
                reply_panel_or_ack(&mut session, &mut panel_revision, reply_to, res)?;
            }
            envelope::Body::CloseRequest(close) => {
                let res = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_revision: &mut panel_revision,
                    };
                    plugin.on_close_request(&mut ctx, close)
                };
                reply_panel_or_ack(&mut session, &mut panel_revision, reply_to, res)?;
            }
            envelope::Body::CommitEditor(commit) => {
                let out = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_revision: &mut panel_revision,
                    };
                    plugin.on_commit_editor(&mut ctx, commit)
                };
                let wire = match out {
                    Ok(committed) => envelope::Body::Committed(committed),
                    Err(msg) => envelope::Body::Error(Failure::new(ErrorCode::Stale, msg)),
                };
                session.send(wire, panel_revision, reply_to)?;
            }
            envelope::Body::CommitResponseEditor(commit) => {
                let out = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_revision: &mut panel_revision,
                    };
                    plugin.on_commit_response_editor(&mut ctx, commit)
                };
                let wire = match out {
                    Ok(committed) => envelope::Body::ResponseCommitted(committed),
                    Err(msg) => envelope::Body::Error(Failure::new(ErrorCode::Stale, msg)),
                };
                session.send(wire, panel_revision, reply_to)?;
            }
            envelope::Body::ContextMenuCall(call) => {
                let out = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_revision: &mut panel_revision,
                    };
                    plugin.on_context_menu(&mut ctx, call)
                };
                let wire = match out {
                    Ok(res) => envelope::Body::ContextMenuResult(res),
                    Err(msg) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, msg)),
                };
                session.send(wire, panel_revision, reply_to)?;
            }
            envelope::Body::HttpHookCall(call) => {
                let out = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_revision: &mut panel_revision,
                    };
                    plugin.on_http_hook(&mut ctx, call)
                };
                let wire = match out {
                    Ok(res) => envelope::Body::HttpHookResult(res),
                    Err(msg) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, msg)),
                };
                session.send(wire, panel_revision, reply_to)?;
            }
            envelope::Body::WebSocketHookCall(call) => {
                let out = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_revision: &mut panel_revision,
                    };
                    plugin.on_ws_hook(&mut ctx, call)
                };
                let wire = match out {
                    Ok(res) => envelope::Body::WebSocketHookResult(res),
                    Err(msg) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, msg)),
                };
                session.send(wire, panel_revision, reply_to)?;
            }
            envelope::Body::SessionActionCall(call) => {
                let out = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_revision: &mut panel_revision,
                    };
                    plugin.on_session_action(&mut ctx, call)
                };
                let wire = match out {
                    Ok(res) => envelope::Body::SessionActionResult(res),
                    Err(msg) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, msg)),
                };
                session.send(wire, panel_revision, reply_to)?;
            }
            envelope::Body::PayloadCall(call) => {
                let out = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_revision: &mut panel_revision,
                    };
                    plugin.on_payload(&mut ctx, call)
                };
                let wire = match out {
                    Ok(res) => envelope::Body::PayloadResult(res),
                    Err(msg) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, msg)),
                };
                session.send(wire, panel_revision, reply_to)?;
            }
            envelope::Body::PassiveCheckCall(call) => {
                let out = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_revision: &mut panel_revision,
                    };
                    plugin.on_passive_check(&mut ctx, call)
                };
                let wire = match out {
                    Ok(res) => envelope::Body::PassiveCheckResult(res),
                    Err(msg) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, msg)),
                };
                session.send(wire, panel_revision, reply_to)?;
            }
            envelope::Body::ActiveCheckCall(call) => {
                let out = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_revision: &mut panel_revision,
                    };
                    plugin.on_active_check(&mut ctx, call)
                };
                let wire = match out {
                    Ok(res) => envelope::Body::ActiveCheckResult(res),
                    Err(msg) => envelope::Body::Error(Failure::new(ErrorCode::Invalid, msg)),
                };
                session.send(wire, panel_revision, reply_to)?;
            }
            envelope::Body::Cancel(cancel) => {
                let res = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_revision: &mut panel_revision,
                    };
                    plugin.on_cancel(&mut ctx, cancel)
                };
                reply_panel_or_ack(&mut session, &mut panel_revision, reply_to, res)?;
            }
            envelope::Body::Resync(snapshot) => {
                panel_revision = panel_revision.max(snapshot.revision).max(1);
                let res = {
                    let mut ctx = HostContext {
                        session: &mut session,
                        deferred: &mut deferred,
                        panel_revision: &mut panel_revision,
                    };
                    plugin.on_resync(&mut ctx, snapshot)
                };
                reply_panel_or_ack(&mut session, &mut panel_revision, reply_to, res)?;
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
                "High",
                "Certain",
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
                flow_id: Uuid::new_v4().to_string(),
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
}
