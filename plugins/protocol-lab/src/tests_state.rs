use base64::{Engine, engine::general_purpose::STANDARD};
use mio_plugin_sdk::Header;

use super::*;

pub(super) struct TestHostClient {
    pub(super) panel_id: u64,
    pub(super) grants: Vec<Permission>,
    pub(super) published: u64,
    pub(super) pending: u64,
    pub(super) next_req_id: u64,
    pub(super) cancelled: Vec<u64>,
    pub(super) published_updates: Vec<PanelUpdate>,
    pub(super) enqueued_updates: Vec<PanelUpdate>,
}

impl HostClient for TestHostClient {
    fn grants(&self) -> &[Permission] {
        &self.grants
    }

    fn current_panel_revision(&self) -> u64 {
        self.published
    }

    fn next_panel_revision(&mut self) -> u64 {
        self.pending = self
            .pending
            .max(self.published)
            .saturating_add(1)
            .max(1);
        self.pending
    }

    fn panel_id(&self) -> u64 {
        self.panel_id
    }

    fn call_host(
        &mut self,
        _operation: host_call::Operation,
    ) -> Result<host_result::Outcome, String> {
        Err("TestHostClient does not execute synchronous HostCalls".into())
    }

    fn start_host_call(&mut self, _operation: host_call::Operation) -> Result<u64, String> {
        self.next_req_id += 1;
        Ok(self.next_req_id)
    }

    fn cancel_host_call(&mut self, request_id: u64) -> Result<(), String> {
        self.cancelled.push(request_id);
        Ok(())
    }

    fn publish_panel(&mut self, update: PanelUpdate) -> Result<(), String> {
        match &update {
            PanelUpdate::Snapshot(s) => {
                self.published = s.revision;
                self.pending = self.pending.max(s.revision);
            }
            PanelUpdate::Patch(p) => {
                self.published = p.revision;
                self.pending = self.pending.max(p.revision);
            }
            PanelUpdate::None => {}
        }
        self.published_updates.push(update);
        Ok(())
    }

    fn enqueue_panel_update(&mut self, update: PanelUpdate) {
        self.enqueued_updates.push(update);
    }
}

pub(super) fn client(lab: &Lab) -> TestHostClient {
    TestHostClient {
        panel_id: PANEL_ID,
        grants: lab.grants.clone(),
        published: lab.revision,
        pending: lab.revision,
        next_req_id: 1,
        cancelled: Vec::new(),
        published_updates: Vec::new(),
        enqueued_updates: Vec::new(),
    }
}

pub(super) fn request() -> HttpRequest {
    HttpRequest {
        method: "POST".into(),
        url: "http://127.0.0.1/".into(),
        version: mio_plugin_sdk::HttpVersion::Http11,
        headers: vec![Header {
            name: "Content-Length".into(),
            value: b"8".to_vec(),
        }],
        body: b"5Z+657q/".to_vec(),
        trailers: vec![],
    }
}

#[test]
fn latest_decoded_snapshot_is_encoded_on_commit_and_read_only_is_rejected() {
    let mut lab = Lab::new();
    let resource = Uuid::new_v4();
    lab.load(Context {
        resource,
        revision: 7,
        read_only: false,
        is_draft: true,
        request: request(),
    })
    .unwrap();
    let commit = CommitEditor {
        resource_id: resource.as_bytes().to_vec(),
        message_revision: 7,
        request: Some(request().into()),
        editor: Some(EditorSnapshot {
            document_id: DECODED_DOCUMENT,
            revision: lab.decoded.revision + 1,
            contents: "最新编辑".as_bytes().to_vec(),
            ..Default::default()
        }),
    };
    let result = lab.commit(commit.clone()).unwrap();
    let request: HttpRequest = result.request.unwrap().try_into().unwrap();
    assert_eq!(
        STANDARD.decode(request.body).unwrap(),
        "最新编辑".as_bytes()
    );
    lab.context.as_mut().unwrap().read_only = true;
    assert!(lab.commit(commit).is_err());
    lab.snapshot().validate().unwrap();
}

#[test]
fn a_new_request_clears_response_without_reusing_its_revision() {
    let mut lab = Lab::new();
    let mut ctx = client(&lab);
    let context = Context {
        resource: Uuid::new_v4(),
        revision: 1,
        read_only: false,
        is_draft: true,
        request: request(),
    };
    lab.load(context.clone()).unwrap();
    let response = HostResult {
        outcome: Some(host_result::Outcome::Response(
            mio_plugin_sdk::HttpResponse {
                status: 200,
                version: mio_plugin_sdk::HttpVersion::Http11,
                reason_phrase: None,
                headers: vec![],
                body: b"previous response".to_vec(),
                trailers: vec![],
            }
            .into(),
        )),
    };
    for id in [7, 8] {
        lab.pending.insert(
            id,
            PendingService::Send {
                context_generation: lab.context_generation,
            },
        );
    }
    lab.host_result(&mut ctx, 7, response.clone()).unwrap();
    let old_response = lab.response.clone().unwrap();
    let mut invalid = context.clone();
    invalid.request.body = vec![0; MAX_EDITOR_BYTES + 1];
    assert!(lab.load(invalid).is_err());
    assert_eq!(lab.response.as_ref(), Some(&old_response));

    let mut next = context;
    next.resource = Uuid::new_v4();
    lab.load(next.clone()).unwrap();
    let cleared = lab.response.clone().unwrap();
    assert!(
        cleared.contents.is_empty(),
        "old response belongs to another request"
    );
    assert_eq!(cleared.document_id, old_response.document_id);
    assert!(cleared.revision > old_response.revision);
    assert!(cleared.read_only);
    lab.host_result(&mut ctx, 8, response).unwrap();
    assert_eq!(lab.response.as_ref(), Some(&cleared));
    assert_eq!(lab.context.as_ref().unwrap().resource, next.resource);
    lab.snapshot().validate().unwrap();
}

#[test]
fn closing_a_request_preserves_edits_and_discards_a_late_writeback_after_reopen() {
    let mut lab = Lab::new();
    let mut ctx = client(&lab);
    let context = Context {
        resource: Uuid::new_v4(),
        revision: 1,
        read_only: false,
        is_draft: true,
        request: request(),
    };
    lab.load(context.clone()).unwrap();
    Lab::set_editor(&mut lab.decoded, b"retained native edit".to_vec()).unwrap();
    let retained = lab.decoded.clone();
    let mut old_write = request();
    old_write.body = b"old writeback".to_vec();
    lab.pending.insert(
        7,
        PendingService::WriteBack {
            request: old_write,
            context_generation: lab.context_generation,
        },
    );
    assert!(lab.close(&mut ctx, context.resource).unwrap());
    assert_eq!(ctx.cancelled, vec![7]);
    assert!(!lab.close(&mut ctx, context.resource).unwrap());
    assert!(lab.context.is_none());
    assert_eq!(lab.decoded, retained);
    lab.load(context.clone()).unwrap();
    lab.host_result(
        &mut ctx,
        7,
        HostResult {
            outcome: Some(host_result::Outcome::Resource(ResourceRevision {
                resource_id: context.resource.as_bytes().to_vec(),
                revision: 2,
            })),
        },
    )
    .unwrap();
    assert_eq!(lab.context.as_ref().unwrap().revision, 1);
    assert_eq!(lab.context.as_ref().unwrap().request, context.request);
    lab.snapshot().validate().unwrap();
}

#[test]
fn split_resize_is_preserved_in_following_worker_snapshots() {
    let mut lab = Lab::new();
    let mut ctx = client(&lab);
    let update = lab
        .event(
            &mut ctx,
            UiEvent {
                node_id: 3,
                event: Some(ui_event::Event::SplitResized(0.6)),
                editors: vec![],
            },
        )
        .unwrap();
    assert_eq!(update, PanelUpdate::None);
    let snapshot = lab.snapshot();
    let Some(node::Kind::Split(split)) = snapshot
        .root
        .as_ref()
        .unwrap()
        .find(3)
        .unwrap()
        .kind
        .as_ref()
    else {
        panic!("split");
    };
    assert_eq!(split.fraction, 0.6);
}

#[test]
fn late_flow_detail_cannot_replace_a_new_open_request() {
    let mut lab = Lab::new();
    let mut ctx = client(&lab);
    lab.pending.insert(
        7,
        PendingService::Detail {
            selection: lab.selection_revision,
        },
    );
    let new_resource = Uuid::new_v4();
    lab.load(Context {
        resource: new_resource,
        revision: 9,
        read_only: false,
        is_draft: true,
        request: request(),
    })
    .unwrap();
    let old_record = mio_plugin_sdk::FlowRecord {
        summary: mio_plugin_sdk::FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: mio_plugin_sdk::mio_plugin_api::TrafficSource::Proxy,
            method: "POST".into(),
            url: request().url,
            status: None,
            state: mio_plugin_sdk::mio_plugin_api::FlowState::Complete,
            request_bytes: 8,
            response_bytes: 0,
            started_unix_ms: 1,
            elapsed_ms: 1,
        },
        request: request(),
        response: None,
    };
    lab.host_result(
        &mut ctx,
        7,
        HostResult {
            outcome: Some(host_result::Outcome::Record(Box::new(old_record.into()))),
        },
    )
    .unwrap();
    assert_eq!(lab.context.as_ref().unwrap().resource, new_resource);
    assert_eq!(lab.context.as_ref().unwrap().revision, 9);
}

#[test]
fn query_history_error_advances_viewport_revision_to_supersede_loading_state() {
    let mut lab = Lab::new();
    let mut ctx = client(&lab);
    lab.viewport_revision = 5;
    lab.pending
        .insert(9, PendingService::History { viewport_revision: 5 });
    lab.host_result(
        &mut ctx,
        9,
        HostResult {
            outcome: Some(host_result::Outcome::Error(Failure {
                code: ErrorCode::Deadline as i32,
                message: "history timeout".into(),
            })),
        },
    )
    .unwrap();
    assert!(lab.error);
    assert_eq!(lab.viewport_revision, 6);
}

#[test]
fn sdk_runner_multiplexes_panels_and_emits_patch_for_pointer_drag() {
    use std::time::Duration;

    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: mio_plugin_sdk::mio_plugin_api::ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
    };
    let timeout = Duration::from_secs(5);
    let flow_id = Uuid::new_v4();
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
            granted_permissions: vec![
                Permission::TrafficRead as i32,
                Permission::DraftWrite as i32,
            ],
            host_minor: PROTOCOL_MINOR,
        }),
    );
    push_host(PANEL_ID, 2, 1, 1, envelope::Body::Ready(Empty {}));
    // Register=1, on_ready Snapshot=2, QueryHistory HostCall=3.
    push_host(
        PANEL_ID,
        3,
        2,
        3,
        envelope::Body::HostResult(HostResult {
            outcome: Some(host_result::Outcome::History(HistoryPage {
                offset: 0,
                total: 1,
                rows: vec![
                    mio_plugin_sdk::FlowSummary {
                        id: flow_id,
                        revision: 1,
                        source: mio_plugin_sdk::mio_plugin_api::TrafficSource::Proxy,
                        method: "GET".into(),
                        url: "http://127.0.0.1/".into(),
                        status: Some(200),
                        state: mio_plugin_sdk::mio_plugin_api::FlowState::Complete,
                        request_bytes: 0,
                        response_bytes: 0,
                        started_unix_ms: 1,
                        elapsed_ms: 15,
                    }
                    .into(),
                ],
            })),
        }),
    );
    // Pointer Down on hit_id=1 emits PanelPatch.
    push_host(
        PANEL_ID,
        4,
        3,
        0,
        envelope::Body::UiEvent(UiEvent {
            node_id: CANVAS_NODE,
            event: Some(ui_event::Event::Pointer(PointerEvent {
                hit_id: 1,
                phase: PointerPhase::Down as i32,
                x: 14.0,
                y: 22.0,
            })),
            editors: vec![],
        }),
    );
    // Multiplexed OpenRequest + CloseRequest on child panel 2.
    let child_res = Uuid::new_v4();
    push_host(
        2,
        5,
        0,
        0,
        envelope::Body::OpenRequest(OpenRequest {
            resource_id: child_res.as_bytes().to_vec(),
            message_revision: 1,
            read_only: false,
            request: Some(request().into()),
        }),
    );
    push_host(
        2,
        6,
        1,
        0,
        envelope::Body::CloseRequest(CloseRequest {
            resource_id: child_res.as_bytes().to_vec(),
            destroy_panel: true,
        }),
    );
    push_host(PANEL_ID, 7, 4, 0, envelope::Body::Shutdown(Empty {}));

    let mut outgoing_bytes = Vec::new();
    let mut plugin = ProtocolLabPlugin::new();
    mio_plugin_sdk::run_with_io(incoming_bytes.as_slice(), &mut outgoing_bytes, &mut plugin)
        .unwrap();
    assert!(!plugin.panels.contains_key(&2));

    let mut cursor = outgoing_bytes.as_slice();
    let mut replies = Vec::new();
    while !cursor.is_empty() {
        replies.push(read_frame(&mut cursor).unwrap());
    }
    // Register(0), on_ready Snapshot(1), HostCall(2), History Snapshot(3), Pointer Patch(4), Child Open Snap(5), Child Destroy Ack(6)
    assert_eq!(replies.len(), 7);
    assert!(matches!(
        replies[1].body,
        Some(envelope::Body::Snapshot(ref s)) if s.revision == 2
    ));
    assert!(matches!(replies[2].body, Some(envelope::Body::HostCall(_))));
    assert!(matches!(
        replies[3].body,
        Some(envelope::Body::Snapshot(ref s)) if s.revision == 3
    ));
    assert!(matches!(
        replies[4].body,
        Some(envelope::Body::Patch(ref p)) if p.base_revision == 3 && p.revision == 4
    ));
    assert_eq!(replies[5].panel_id, 2);
    assert!(matches!(
        replies[5].body,
        Some(envelope::Body::Snapshot(ref s)) if s.revision == 1
    ));
    assert_eq!(replies[6].panel_id, 2);
    assert!(matches!(replies[6].body, Some(envelope::Body::Ack(_))));
}
