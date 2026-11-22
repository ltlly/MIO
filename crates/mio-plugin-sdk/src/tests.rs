use std::time::Duration;

use mio_plugin_api::{
    ProjectKey,
    protocol::{
        ChannelIdentity, CollaboratorPayloadDto, Envelope, Hello, HttpRequestDto, HttpResponseDto,
        HttpVersionDto, MAX_FRAME_BYTES, Permission, ScopeStatusDto, encode_frame, host_result,
        read_frame, validate_finding,
    },
};

use super::*;

struct DemoPlugin {
    passive_hits: usize,
    deferred_passive_seen: usize,
}

impl MioPlugin for DemoPlugin {
    fn id(&self) -> &str {
        "lab.sdk-demo"
    }

    fn on_ui_event(
        &mut self,
        ctx: &mut dyn HostClient,
        _panel_id: u64,
        _event: panel::UiEvent,
    ) -> Result<PanelUpdate, String> {
        let in_scope = ctx.is_in_scope("https://example.invalid/")?;
        assert!(in_scope);
        let oast = ctx.collaborator_generate("sdk")?;
        assert_eq!(oast.token, "tok-sdk");
        let rev = ctx.next_panel_revision();
        Ok(ui::snapshot(
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
        )
        .into())
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
        let findings = vec![
            ui::finding("SDK Demo Finding")
                .severity(protocol::FindingSeverity::High)
                .confidence(protocol::FindingConfidence::Certain)
                .detail("Matched test marker")
                .remediation("Fix it")
                .parameter("body")
                .evidence("DEMO")
                .build(),
        ];
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
        url: "https://example.invalid/deferred".into(),
        version: HttpVersionDto::Http11 as i32,
        ..Default::default()
    };
    let resp = HttpResponseDto {
        status: 200,
        version: HttpVersionDto::Http11 as i32,
        body: b"DEMO".to_vec(),
        ..Default::default()
    };

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
                    dns_qname: "tok-sdk.oast.invalid".into(),
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
            plugin_id: "lab.sdk-demo".into(),
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
        "lab.sdk-multiplex"
    }

    fn supports_panel_multiplexing(&self) -> bool {
        true
    }

    fn on_open_request(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        _open: OpenRequest,
    ) -> Result<PanelUpdate, String> {
        assert_eq!(ctx.panel_id(), panel_id);
        let rev = ctx.next_panel_revision();
        self.panels.insert(panel_id, rev);
        Ok(ui::snapshot(
            rev,
            self.id(),
            ui::text(
                1,
                "panel",
                format!("panel-{panel_id}-rev-{rev}"),
                panel::Tone::Foreground,
            ),
        )
        .into())
    }

    fn on_ui_event(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        _event: panel::UiEvent,
    ) -> Result<PanelUpdate, String> {
        let rev = ctx.next_panel_revision();
        self.panels.insert(panel_id, rev);
        Ok(ui::snapshot(
            rev,
            self.id(),
            ui::text(
                1,
                "panel",
                format!("panel-{panel_id}-event-{rev}"),
                panel::Tone::Accent,
            ),
        )
        .into())
    }

    fn on_close_request(
        &mut self,
        _ctx: &mut dyn HostClient,
        panel_id: u64,
        _close: CloseRequest,
    ) -> Result<PanelUpdate, String> {
        self.panels.remove(&panel_id);
        Ok(PanelUpdate::None)
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
        "lab.sdk-open-hostcall"
    }

    fn supports_panel_multiplexing(&self) -> bool {
        true
    }

    fn on_open_request(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        _open: OpenRequest,
    ) -> Result<PanelUpdate, String> {
        let next_rev = ctx.next_panel_revision();
        let pref = ctx.preferences_get("selected_profile")?;
        let text = String::from_utf8_lossy(&pref.unwrap_or_default()).into_owned();
        Ok(ui::snapshot(
            next_rev,
            self.id(),
            ui::text(
                1,
                "panel",
                format!("panel-{panel_id}:{text}"),
                panel::Tone::Foreground,
            ),
        )
        .into())
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
    assert!(matches!(replies[0].body, Some(envelope::Body::Register(_))));
    assert_eq!(replies[1].panel_id, 2);
    assert_eq!(replies[1].revision, 0);
    assert!(matches!(
        replies[1].body,
        Some(envelope::Body::HostCall(ref hc)) if hc.parent_request_id == 3
    ));
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
        "lab.sdk-close-destroy"
    }

    fn supports_panel_multiplexing(&self) -> bool {
        true
    }

    fn on_open_request(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        _open: OpenRequest,
    ) -> Result<PanelUpdate, String> {
        let rev = ctx.next_panel_revision();
        Ok(ui::snapshot(
            rev,
            self.id(),
            ui::text(1, "p", format!("p-{panel_id}"), panel::Tone::Foreground),
        )
        .into())
    }

    fn on_close_request(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        _close: CloseRequest,
    ) -> Result<PanelUpdate, String> {
        self.closed.push((panel_id, ctx.current_panel_revision()));
        Ok(PanelUpdate::None)
    }

    fn on_panel_destroy(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        _close: CloseRequest,
    ) -> Result<PanelUpdate, String> {
        self.destroyed.push((panel_id, ctx.current_panel_revision()));
        Ok(PanelUpdate::None)
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
        "lab.sdk-resync-rev"
    }

    fn supports_panel_multiplexing(&self) -> bool {
        true
    }

    fn on_open_request(
        &mut self,
        ctx: &mut dyn HostClient,
        _panel_id: u64,
        _open: OpenRequest,
    ) -> Result<PanelUpdate, String> {
        let rev = ctx.next_panel_revision();
        Ok(ui::snapshot(
            rev,
            self.id(),
            ui::text(1, "p", format!("open-{rev}"), panel::Tone::Foreground),
        )
        .into())
    }

    fn on_ui_event(
        &mut self,
        ctx: &mut dyn HostClient,
        _panel_id: u64,
        _event: panel::UiEvent,
    ) -> Result<PanelUpdate, String> {
        if ctx.current_panel_revision() >= 3 {
            let _ = ctx.is_in_scope("https://example.invalid/after")?;
            return Ok(PanelUpdate::None);
        }
        let rev = ctx.next_panel_revision();
        Ok(ui::snapshot(
            rev,
            self.id(),
            ui::text(1, "p", format!("ui-{rev}"), panel::Tone::Foreground),
        )
        .into())
    }

    fn on_resync(
        &mut self,
        ctx: &mut dyn HostClient,
        _panel_id: u64,
        snapshot: PanelSnapshot,
    ) -> Result<PanelUpdate, String> {
        self.seen_revs.push(ctx.current_panel_revision());
        let _ = ctx.is_in_scope("https://example.invalid/resync")?;
        let rev = ctx.next_panel_revision();
        assert_eq!(snapshot.revision, 1);
        assert!(rev > 2, "pending must stay monotonic above discarded rev=2");
        Ok(ui::snapshot(
            rev,
            self.id(),
            ui::text(1, "p", format!("resync-{rev}"), panel::Tone::Foreground),
        )
        .into())
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
    assert_eq!(replies.len(), 7);
    assert_eq!(replies[3].panel_id, 2);
    assert_eq!(
        replies[3].revision, 1,
        "HostCall inside on_resync MUST carry host-confirmed revision=1, not discarded 2"
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
        "lab.sdk-cancel-guard"
    }

    fn supports_panel_multiplexing(&self) -> bool {
        true
    }

    fn on_open_request(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        _open: OpenRequest,
    ) -> Result<PanelUpdate, String> {
        let rev = ctx.next_panel_revision();
        Ok(ui::snapshot(
            rev,
            self.id(),
            ui::text(1, "p", format!("p-{panel_id}"), panel::Tone::Foreground),
        )
        .into())
    }

    fn on_cancel(
        &mut self,
        ctx: &mut dyn HostClient,
        panel_id: u64,
        _cancel: Cancel,
    ) -> Result<PanelUpdate, String> {
        assert_eq!(ctx.panel_id(), panel_id);
        self.cancel_panels.push(panel_id);
        let next = ctx.next_panel_revision();
        Ok(ui::snapshot(
            next,
            self.id(),
            ui::text(1, "p", "ignored-cancel-snap", panel::Tone::Foreground),
        )
        .into())
    }

    fn on_ui_event(
        &mut self,
        ctx: &mut dyn HostClient,
        _panel_id: u64,
        _event: panel::UiEvent,
    ) -> Result<PanelUpdate, String> {
        let _ = ctx.is_in_scope("https://example.invalid/after-cancel")?;
        Ok(PanelUpdate::None)
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
    push_host(2, 4, 1, 0, envelope::Body::Cancel(Cancel { request_id: 99 }));
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
            "lab.sdk-no-resync"
        }

        fn on_ui_event(
            &mut self,
            ctx: &mut dyn HostClient,
            _panel_id: u64,
            _event: panel::UiEvent,
        ) -> Result<PanelUpdate, String> {
            if !self.bumped {
                self.bumped = true;
                let rev = ctx.next_panel_revision();
                return Ok(ui::snapshot(
                    rev,
                    self.id(),
                    ui::text(1, "p", format!("v-{rev}"), panel::Tone::Foreground),
                )
                .into());
            }
            Ok(PanelUpdate::None)
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
            "lab.sdk-no-resync",
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

#[test]
fn sdk_emits_panel_patch_and_updates_revision() {
    struct PatchPlugin;
    impl MioPlugin for PatchPlugin {
        fn id(&self) -> &str {
            "lab.sdk-patch"
        }

        fn on_ui_event(
            &mut self,
            ctx: &mut dyn HostClient,
            _panel_id: u64,
            _event: panel::UiEvent,
        ) -> Result<PanelUpdate, String> {
            let base = ctx.current_panel_revision();
            let rev = ctx.next_panel_revision();
            Ok(PanelUpdate::Patch(ui::patch(
                base,
                rev,
                vec![panel::PatchOperation {
                    node_id: 1,
                    change: Some(panel::patch_operation::Change::Replace(ui::text(
                        1,
                        "Status",
                        "patched",
                        panel::Tone::Accent,
                    ))),
                }],
            )))
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
            granted_permissions: vec![],
            host_minor: mio_plugin_api::protocol::PROTOCOL_MINOR,
        }),
    );
    push_host(2, 1, envelope::Body::Ready(Empty {}));
    push_host(
        3,
        0,
        envelope::Body::UiEvent(panel::UiEvent {
            node_id: 1,
            editors: vec![],
            event: Some(panel::ui_event::Event::Click(Empty {})),
        }),
    );
    push_host(4, 0, envelope::Body::Shutdown(Empty {}));

    let mut outgoing_bytes = Vec::new();
    run_with_io(incoming_bytes.as_slice(), &mut outgoing_bytes, PatchPlugin).unwrap();

    let mut cursor = outgoing_bytes.as_slice();
    let mut replies = Vec::new();
    while !cursor.is_empty() {
        replies.push(read_frame(&mut cursor).unwrap());
    }
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[1].in_reply_to, 3);
    assert_eq!(replies[1].revision, 2);
    assert!(matches!(
        replies[1].body,
        Some(envelope::Body::Patch(ref p)) if p.base_revision == 1 && p.revision == 2
    ));
}

#[test]
fn sdk_call_host_aborts_immediately_on_shutdown() {
    struct ShutdownDuringCallPlugin {
        saw_err: bool,
    }
    impl MioPlugin for ShutdownDuringCallPlugin {
        fn id(&self) -> &str {
            "lab.sdk-shutdown-call"
        }

        fn on_ui_event(
            &mut self,
            ctx: &mut dyn HostClient,
            _panel_id: u64,
            _event: panel::UiEvent,
        ) -> Result<PanelUpdate, String> {
            // Even if plugin swallows the error and tries another call_host, shutdown flag stops it.
            let first = ctx.is_in_scope("https://example.invalid/1");
            self.saw_err = first.is_err();
            let second = ctx.is_in_scope("https://example.invalid/2");
            assert!(second.is_err());
            Ok(PanelUpdate::None)
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
            granted_permissions: vec![Permission::TrafficRead as i32],
            host_minor: mio_plugin_api::protocol::PROTOCOL_MINOR,
        }),
    );
    push_host(2, 1, envelope::Body::Ready(Empty {}));
    push_host(
        3,
        0,
        envelope::Body::UiEvent(panel::UiEvent {
            node_id: 1,
            editors: vec![],
            event: Some(panel::ui_event::Event::Click(Empty {})),
        }),
    );
    // Shutdown arrives BEFORE any HostResult for the in-flight ScopeCheck!
    push_host(4, 0, envelope::Body::Shutdown(Empty {}));

    let mut outgoing_bytes = Vec::new();
    let mut plugin = ShutdownDuringCallPlugin { saw_err: false };
    run_with_io(incoming_bytes.as_slice(), &mut outgoing_bytes, &mut plugin).unwrap();
    assert!(plugin.saw_err);

    let mut cursor = outgoing_bytes.as_slice();
    let mut replies = Vec::new();
    while !cursor.is_empty() {
        replies.push(read_frame(&mut cursor).unwrap());
    }
    // Register, first ScopeCheck HostCall (no second HostCall or superfluous Ack after Shutdown!)
    assert_eq!(replies.len(), 2);
    assert!(matches!(replies[1].body, Some(envelope::Body::HostCall(_))));
}

#[test]
fn sdk_call_host_aborts_immediately_on_matching_cancel_and_drops_late_host_result() {
    struct CancelDuringCallPlugin {
        cancel_calls: usize,
        host_results_seen: usize,
    }
    impl MioPlugin for CancelDuringCallPlugin {
        fn id(&self) -> &str {
            "lab.sdk-cancel-during-call"
        }

        fn on_ui_event(
            &mut self,
            ctx: &mut dyn HostClient,
            _panel_id: u64,
            _event: panel::UiEvent,
        ) -> Result<PanelUpdate, String> {
            let _ = ctx.is_in_scope("https://example.invalid/cancelled")?;
            Ok(PanelUpdate::None)
        }

        fn on_cancel(
            &mut self,
            _ctx: &mut dyn HostClient,
            _panel_id: u64,
            _cancel: Cancel,
        ) -> Result<PanelUpdate, String> {
            self.cancel_calls += 1;
            Ok(PanelUpdate::None)
        }

        fn on_host_result(
            &mut self,
            _ctx: &mut dyn HostClient,
            _panel_id: u64,
            _request_id: u64,
            _result: HostResult,
        ) -> Result<PanelUpdate, String> {
            self.host_results_seen += 1;
            Ok(PanelUpdate::None)
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
            granted_permissions: vec![Permission::TrafficRead as i32],
            host_minor: mio_plugin_api::protocol::PROTOCOL_MINOR,
        }),
    );
    push_host(2, 1, envelope::Body::Ready(Empty {}));
    push_host(
        3,
        0,
        envelope::Body::UiEvent(panel::UiEvent {
            node_id: 1,
            editors: vec![],
            event: Some(panel::ui_event::Event::Click(Empty {})),
        }),
    );
    // Cancel matching parent_request_id=3 arrives while call_host (req_id=2) is waiting!
    push_host(4, 0, envelope::Body::Cancel(Cancel { request_id: 3 }));
    // Late HostResult for abandoned req_id=2 must be ignored, never routed to on_host_result!
    push_host(
        5,
        2,
        envelope::Body::HostResult(HostResult {
            outcome: Some(host_result::Outcome::ScopeStatus(ScopeStatusDto {
                in_scope: true,
            })),
        }),
    );
    push_host(6, 0, envelope::Body::Shutdown(Empty {}));

    let mut outgoing_bytes = Vec::new();
    let mut plugin = CancelDuringCallPlugin {
        cancel_calls: 0,
        host_results_seen: 0,
    };
    run_with_io(incoming_bytes.as_slice(), &mut outgoing_bytes, &mut plugin).unwrap();
    assert_eq!(plugin.cancel_calls, 1);
    assert_eq!(plugin.host_results_seen, 0);

    let mut cursor = outgoing_bytes.as_slice();
    let mut replies = Vec::new();
    while !cursor.is_empty() {
        replies.push(read_frame(&mut cursor).unwrap());
    }
    // Register, HostCall(req_id=2), Error(Cancelled for UiEvent req=3), Ack(for Cancel req=4)
    assert_eq!(replies.len(), 4);
    assert!(matches!(replies[1].body, Some(envelope::Body::HostCall(_))));
    assert!(matches!(
        replies[2].body,
        Some(envelope::Body::Error(ref f)) if f.code == ErrorCode::Cancelled as i32
    ));
    assert_eq!(replies[2].in_reply_to, 3);
    assert!(matches!(replies[3].body, Some(envelope::Body::Ack(_))));
    assert_eq!(replies[3].in_reply_to, 4);
}

#[test]
fn sdk_call_host_handles_expired_interleaved_and_destroyed_cancel_without_queuing() {
    struct Plugin {
        events_seen: usize,
        cancel_seen: usize,
    }
    impl MioPlugin for Plugin {
        fn id(&self) -> &str {
            "lab.sdk-inline-expire"
        }

        fn supports_panel_multiplexing(&self) -> bool {
            true
        }

        fn on_ui_event(
            &mut self,
            ctx: &mut dyn HostClient,
            panel_id: u64,
            _event: panel::UiEvent,
        ) -> Result<PanelUpdate, String> {
            self.events_seen += 1;
            if panel_id == PANEL_ID {
                assert!(ctx.is_in_scope("https://example.invalid/")?);
            }
            Ok(PanelUpdate::None)
        }

        fn on_cancel(
            &mut self,
            _ctx: &mut dyn HostClient,
            _panel_id: u64,
            _cancel: Cancel,
        ) -> Result<PanelUpdate, String> {
            self.cancel_seen += 1;
            Ok(PanelUpdate::None)
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
    let push = |buf: &mut Vec<u8>, env: Envelope| {
        buf.extend_from_slice(&encode_frame(&env).unwrap());
    };
    push(
        &mut incoming_bytes,
        Envelope::new(
            identity,
            1,
            0,
            envelope::Body::Hello(Hello {
                max_frame_bytes: MAX_FRAME_BYTES as u32,
                max_operation_ms: 5000,
                granted_permissions: vec![Permission::TrafficRead as i32],
                host_minor: mio_plugin_api::protocol::PROTOCOL_MINOR,
            }),
            timeout,
        ),
    );
    let mut ready = Envelope::new(identity, 2, 1, envelope::Body::Ready(Empty {}), timeout);
    ready.in_reply_to = 1;
    push(&mut incoming_bytes, ready);
    push(
        &mut incoming_bytes,
        Envelope::new(
            identity,
            3,
            1,
            envelope::Body::UiEvent(panel::UiEvent {
                node_id: 1,
                editors: vec![],
                event: Some(panel::ui_event::Event::Click(Empty {})),
            }),
            timeout,
        ),
    );
    // While call_host waits, send 2 expired peer UiEvents and 1 Cancel for unknown panel 99,
    // with max_deferred = 2 (they must NOT overflow deferred because they are handled immediately!).
    for req_id in [4, 5] {
        let mut expired = Envelope::new(
            identity,
            req_id,
            0,
            envelope::Body::UiEvent(panel::UiEvent {
                node_id: 1,
                editors: vec![],
                event: Some(panel::ui_event::Event::Click(Empty {})),
            }),
            timeout,
        )
        .with_panel_id(2);
        expired.deadline_unix_ms = 1;
        push(&mut incoming_bytes, expired);
    }
    push(
        &mut incoming_bytes,
        Envelope::new(
            identity,
            6,
            0,
            envelope::Body::Cancel(Cancel { request_id: 10 }),
            timeout,
        )
        .with_panel_id(99),
    );
    let mut hr = Envelope::new(
        identity,
        7,
        1,
        envelope::Body::HostResult(HostResult {
            outcome: Some(host_result::Outcome::ScopeStatus(ScopeStatusDto {
                in_scope: true,
            })),
        }),
        timeout,
    );
    hr.in_reply_to = 2;
    push(&mut incoming_bytes, hr);
    push(
        &mut incoming_bytes,
        Envelope::new(identity, 8, 1, envelope::Body::Shutdown(Empty {}), timeout),
    );

    let mut outgoing_bytes = Vec::new();
    let mut plugin = Plugin {
        events_seen: 0,
        cancel_seen: 0,
    };
    run_with_io_bounded(
        incoming_bytes.as_slice(),
        &mut outgoing_bytes,
        &mut plugin,
        2,
    )
    .unwrap();
    assert_eq!(plugin.events_seen, 1);
    assert_eq!(plugin.cancel_seen, 0);

    let mut cursor = outgoing_bytes.as_slice();
    let mut replies = Vec::new();
    while !cursor.is_empty() {
        replies.push(read_frame(&mut cursor).unwrap());
    }
    // Register, HostCall(req=2), Deadline(4), Deadline(5), Ack(panel 99 req=6), Ack(panel 1 req=3)
    assert_eq!(replies.len(), 6);
    assert!(matches!(
        replies[2].body,
        Some(envelope::Body::Error(ref f)) if f.code == ErrorCode::Deadline as i32
    ));
    assert!(matches!(
        replies[3].body,
        Some(envelope::Body::Error(ref f)) if f.code == ErrorCode::Deadline as i32
    ));
    assert_eq!(replies[4].panel_id, 99);
    assert!(matches!(replies[4].body, Some(envelope::Body::Ack(_))));
    assert_eq!(replies[5].in_reply_to, 3);
    assert!(matches!(replies[5].body, Some(envelope::Body::Ack(_))));
}

#[test]
fn sdk_bounded_deferred_overflow_aborts_sync_call_and_drains_peer_events() {
    struct PeerDrainPlugin {
        seen_panels: Vec<u64>,
    }
    impl MioPlugin for PeerDrainPlugin {
        fn id(&self) -> &str {
            "lab.sdk-bounded-defer"
        }

        fn supports_panel_multiplexing(&self) -> bool {
            true
        }

        fn on_open_request(
            &mut self,
            ctx: &mut dyn HostClient,
            panel_id: u64,
            _open: OpenRequest,
        ) -> Result<PanelUpdate, String> {
            let rev = ctx.next_panel_revision();
            Ok(ui::snapshot(
                rev,
                self.id(),
                ui::text(1, "p", format!("p-{panel_id}"), panel::Tone::Foreground),
            )
            .into())
        }

        fn on_ui_event(
            &mut self,
            ctx: &mut dyn HostClient,
            panel_id: u64,
            _event: panel::UiEvent,
        ) -> Result<PanelUpdate, String> {
            self.seen_panels.push(panel_id);
            if panel_id == PANEL_ID {
                let _ = ctx.is_in_scope("https://example.invalid/overflow")?;
            }
            Ok(PanelUpdate::None)
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
            request: Some(req),
        }),
    );
    // Main panel starts a synchronous call_host; 3 peer events flood before HostResult!
    push_host(
        PANEL_ID,
        4,
        0,
        envelope::Body::UiEvent(panel::UiEvent {
            node_id: 1,
            editors: vec![],
            event: Some(panel::ui_event::Event::Click(Empty {})),
        }),
    );
    for req_id in [5, 6, 7] {
        push_host(
            2,
            req_id,
            0,
            envelope::Body::UiEvent(panel::UiEvent {
                node_id: 1,
                editors: vec![],
                event: Some(panel::ui_event::Event::Click(Empty {})),
            }),
        );
    }
    push_host(
        PANEL_ID,
        8,
        2,
        envelope::Body::HostResult(HostResult {
            outcome: Some(host_result::Outcome::ScopeStatus(ScopeStatusDto {
                in_scope: true,
            })),
        }),
    );
    push_host(PANEL_ID, 9, 0, envelope::Body::Shutdown(Empty {}));

    let mut outgoing_bytes = Vec::new();
    let mut plugin = PeerDrainPlugin {
        seen_panels: Vec::new(),
    };
    run_with_io_bounded(
        incoming_bytes.as_slice(),
        &mut outgoing_bytes,
        &mut plugin,
        3,
    )
    .unwrap();
    // Main panel entered first, aborted on deferred cap=3, then all 3 peer events drained!
    assert_eq!(plugin.seen_panels, vec![1, 2, 2, 2]);

    let mut cursor = outgoing_bytes.as_slice();
    let mut replies = Vec::new();
    while !cursor.is_empty() {
        replies.push(read_frame(&mut cursor).unwrap());
    }
    // Register, Snap(panel 2), HostCall(req=2), Error(panel 1 req=4), Ack(5), Ack(6), Ack(7)
    assert_eq!(replies.len(), 7);
    assert_eq!(replies[3].in_reply_to, 4);
    assert!(matches!(
        replies[3].body,
        Some(envelope::Body::Error(ref f)) if f.code == ErrorCode::Invalid as i32
    ));
    assert_eq!(replies[4].in_reply_to, 5);
    assert_eq!(replies[5].in_reply_to, 6);
    assert_eq!(replies[6].in_reply_to, 7);
}

#[test]
fn sdk_on_ready_and_async_start_host_call_route_to_on_host_result() {
    struct AsyncPlugin {
        pending_id: Option<u64>,
        completed: bool,
    }
    impl MioPlugin for AsyncPlugin {
        fn id(&self) -> &str {
            "lab.sdk-async-ready"
        }

        fn on_ready(&mut self, ctx: &mut dyn HostClient) -> Result<PanelUpdate, String> {
            let rev = ctx.next_panel_revision();
            ctx.publish_panel(PanelUpdate::Snapshot(ui::snapshot(
                rev,
                self.id(),
                ui::text(1, "Status", "booted", panel::Tone::Foreground),
            )))?;
            let id = ctx.start_host_call(protocol::host_call::Operation::ScopeCheck(
                "https://example.invalid/".into(),
            ))?;
            self.pending_id = Some(id);
            Ok(PanelUpdate::None)
        }

        fn on_host_result(
            &mut self,
            ctx: &mut dyn HostClient,
            panel_id: u64,
            request_id: u64,
            result: HostResult,
        ) -> Result<PanelUpdate, String> {
            assert_eq!(panel_id, PANEL_ID);
            assert_eq!(Some(request_id), self.pending_id);
            assert!(matches!(
                result.outcome,
                Some(host_result::Outcome::ScopeStatus(ref s)) if s.in_scope
            ));
            self.completed = true;
            let rev = ctx.next_panel_revision();
            Ok(ui::snapshot(
                rev,
                self.id(),
                ui::text(1, "Status", "async-done", panel::Tone::Accent),
            )
            .into())
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
            granted_permissions: vec![Permission::TrafficRead as i32],
            host_minor: mio_plugin_api::protocol::PROTOCOL_MINOR,
        }),
    );
    push_host(2, 1, envelope::Body::Ready(Empty {}));
    // Register is req 1, publish_panel Snapshot is req 2, start_host_call is req 3.
    push_host(
        3,
        3,
        envelope::Body::HostResult(HostResult {
            outcome: Some(host_result::Outcome::ScopeStatus(ScopeStatusDto {
                in_scope: true,
            })),
        }),
    );
    push_host(4, 0, envelope::Body::Shutdown(Empty {}));

    let mut outgoing_bytes = Vec::new();
    let mut plugin = AsyncPlugin {
        pending_id: None,
        completed: false,
    };
    run_with_io(incoming_bytes.as_slice(), &mut outgoing_bytes, &mut plugin).unwrap();
    assert!(plugin.completed);

    let mut cursor = outgoing_bytes.as_slice();
    let mut replies = Vec::new();
    while !cursor.is_empty() {
        replies.push(read_frame(&mut cursor).unwrap());
    }
    assert_eq!(replies.len(), 4);
    assert!(matches!(
        replies[1].body,
        Some(envelope::Body::Snapshot(ref s)) if s.revision == 2
    ));
    assert_eq!(replies[2].revision, 2);
    assert!(matches!(
        replies[2].body,
        Some(envelope::Body::HostCall(ref hc)) if hc.parent_request_id == 0
    ));
    assert!(matches!(
        replies[3].body,
        Some(envelope::Body::Snapshot(ref s)) if s.revision == 3
    ));
    assert_eq!(replies[3].in_reply_to, 0);
}

#[test]
fn ui_builders_and_finding_builder_produce_valid_protocol_dtos() {
    let left = ui::virtual_table(
        2,
        "Flows",
        vec![ui::table_column("url", "URL", 200.0)],
        1,
        0,
        1,
        vec![ui::table_row("row-1", vec!["http://127.0.0.1/".into()])],
        false,
    );
    let right = ui::tabs(
        3,
        "Editors",
        panel::Axis::Horizontal,
        4,
        vec![ui::tab(
            "Body",
            ui::editor(
                4,
                "Body Editor",
                10,
                1,
                b"hello".to_vec(),
                panel::EditorFormat::Utf8,
                false,
            ),
        )],
    );
    let canvas_node = ui::canvas(
        5,
        "Timeline",
        300.0,
        100.0,
        vec![panel::DrawCommand {
            op: Some(panel::draw_command::Op::Rect(panel::DrawRect {
                bounds: Some(panel::Rect {
                    x: 4.0,
                    y: 4.0,
                    width: 60.0,
                    height: 20.0,
                }),
                tone: panel::Tone::Accent as i32,
                radius: 2.0,
            })),
        }],
        vec![panel::HitRegion {
            id: 1,
            bounds: Some(panel::Rect {
                x: 4.0,
                y: 4.0,
                width: 60.0,
                height: 20.0,
            }),
            accessibility_label: "entry".into(),
            focusable: true,
            capture_pointer: false,
        }],
    );
    let snap = ui::snapshot(
        1,
        "Widget Suite",
        ui::column(
            1,
            "Root",
            vec![
                ui::split(6, "Workspace", panel::Axis::Horizontal, 0.5, left, right),
                canvas_node,
            ],
            4.0,
        ),
    );
    snap.validate().unwrap();

    let patch = ui::patch(
        1,
        2,
        vec![panel::PatchOperation {
            node_id: 5,
            change: Some(panel::patch_operation::Change::Canvas(panel::Canvas {
                width: 300.0,
                height: 100.0,
                commands: vec![],
                hit_regions: vec![],
            })),
        }],
    );
    assert_eq!(patch.base_revision, 1);
    assert_eq!(patch.revision, 2);

    let dto: protocol::PluginFindingDto = ui::finding("Reflected Parameter")
        .severity(protocol::FindingSeverity::Medium)
        .confidence(protocol::FindingConfidence::Firm)
        .detail("Reflected value in response")
        .remediation("Encode output")
        .parameter("q")
        .evidence("probe")
        .into();
    validate_finding(&dto).unwrap();
    assert_eq!(dto.severity, protocol::FindingSeverity::Medium as i32);
    assert_eq!(dto.confidence, protocol::FindingConfidence::Firm as i32);
}

#[test]
fn destroying_child_panel_prunes_stale_queued_open_requests() {
    struct NoPostDestroyOpenPlugin {
        open_calls: usize,
        destroyed: bool,
    }

    impl MioPlugin for NoPostDestroyOpenPlugin {
        fn id(&self) -> &str {
            "lab.sdk-destroy-prune"
        }

        fn supports_panel_multiplexing(&self) -> bool {
            true
        }

        fn on_ui_event(
            &mut self,
            ctx: &mut dyn HostClient,
            _panel_id: u64,
            _event: panel::UiEvent,
        ) -> Result<PanelUpdate, String> {
            let _ = ctx.is_in_scope("https://example.invalid/");
            Ok(PanelUpdate::None)
        }

        fn on_open_request(
            &mut self,
            _ctx: &mut dyn HostClient,
            panel_id: u64,
            _open: OpenRequest,
        ) -> Result<PanelUpdate, String> {
            assert_ne!(panel_id, 2, "pruned child panel OpenRequest must never run");
            self.open_calls += 1;
            Ok(PanelUpdate::None)
        }

        fn on_panel_destroy(
            &mut self,
            _ctx: &mut dyn HostClient,
            panel_id: u64,
            _close: CloseRequest,
        ) -> Result<PanelUpdate, String> {
            assert_eq!(panel_id, 2);
            self.destroyed = true;
            Ok(PanelUpdate::None)
        }
    }

    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
    };
    let mut incoming_bytes = Vec::new();
    let timeout = Duration::from_secs(5);
    let push_host = |buf: &mut Vec<u8>, panel_id: u64, req_id: u64, rev: u64, in_reply_to: u64, body| {
        let mut env = Envelope::new(identity, req_id, rev, body, timeout).with_panel_id(panel_id);
        env.in_reply_to = in_reply_to;
        buf.extend_from_slice(&encode_frame(&env).unwrap());
    };

    push_host(
        &mut incoming_bytes,
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
    push_host(
        &mut incoming_bytes,
        PANEL_ID,
        2,
        1,
        1,
        envelope::Body::Ready(Empty {}),
    );
    push_host(
        &mut incoming_bytes,
        PANEL_ID,
        3,
        1,
        0,
        envelope::Body::UiEvent(panel::UiEvent {
            node_id: 1,
            event: None,
            editors: vec![],
        }),
    );
    // While UiEvent waits on HostCall(req 2 from worker), host enqueues OpenRequest(req 5) + CloseRequest(destroy=true, req 6) for panel 2,
    // followed by HostResult replying to worker req 2, then Shutdown(req 7).
    push_host(
        &mut incoming_bytes,
        2,
        5,
        0,
        0,
        envelope::Body::OpenRequest(OpenRequest {
            resource_id: Uuid::new_v4().as_bytes().to_vec(),
            message_revision: 1,
            read_only: false,
            request: Some(HttpRequestDto {
                method: "GET".into(),
                url: "https://example.invalid/".into(),
                version: HttpVersionDto::Http11 as i32,
                ..Default::default()
            }),
        }),
    );
    push_host(
        &mut incoming_bytes,
        2,
        6,
        1,
        0,
        envelope::Body::CloseRequest(CloseRequest {
            resource_id: Uuid::new_v4().as_bytes().to_vec(),
            destroy_panel: true,
        }),
    );
    push_host(
        &mut incoming_bytes,
        PANEL_ID,
        4,
        1,
        2,
        envelope::Body::HostResult(mio_plugin_api::protocol::HostResult {
            outcome: Some(host_result::Outcome::ScopeStatus(ScopeStatusDto {
                in_scope: true,
            })),
        }),
    );
    push_host(&mut incoming_bytes, PANEL_ID, 7, 1, 0, envelope::Body::Shutdown(Empty {}));

    let mut outgoing_bytes = Vec::new();
    let mut plugin = NoPostDestroyOpenPlugin {
        open_calls: 0,
        destroyed: false,
    };
    run_with_io(incoming_bytes.as_slice(), &mut outgoing_bytes, &mut plugin).unwrap();
    assert!(plugin.destroyed);
    assert_eq!(plugin.open_calls, 0);

    let mut cursor = outgoing_bytes.as_slice();
    let mut replies = Vec::new();
    while !cursor.is_empty() {
        replies.push(read_frame(&mut cursor).unwrap());
    }
    assert!(
        replies.iter().any(|r| {
            r.in_reply_to == 5
                && r.panel_id == 2
                && matches!(
                    r.body,
                    Some(envelope::Body::Error(ref err)) if err.code == ErrorCode::Stale as i32
                )
        }),
        "pruned OpenRequest must receive Stale error reply, got: {replies:?}"
    );
}
