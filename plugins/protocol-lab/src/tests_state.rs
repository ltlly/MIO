use base64::{Engine, engine::general_purpose::STANDARD};
use mio_plugin_api::Header;

use super::*;

pub(super) fn session(lab: &Lab) -> WorkerSession<io::Cursor<Vec<u8>>, io::Sink> {
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: mio_plugin_api::ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
    };
    let duration = std::time::Duration::from_secs(5);
    let hello = Envelope::new(
        identity,
        1,
        0,
        envelope::Body::Hello(Hello {
            max_frame_bytes: MAX_FRAME_BYTES as u32,
            max_operation_ms: 5000,
            granted_permissions: vec![],
            host_minor: PROTOCOL_MINOR,
        }),
        duration,
    );
    let mut ready = Envelope::new(
        identity,
        2,
        lab.revision,
        envelope::Body::Ready(Empty {}),
        duration,
    );
    ready.in_reply_to = 1;
    let mut input = encode_frame(&hello).unwrap();
    input.extend(encode_frame(&ready).unwrap());
    WorkerSession::accept(
        io::Cursor::new(input),
        io::sink(),
        Register {
            plugin_id: "lab.protocol".into(),
            plugin_version: "0.1.0".into(),
            required_components: vec![],
            panel: Some(lab.snapshot()),
            supports_panel_multiplexing: false,
        },
    )
    .unwrap()
}

pub(super) fn request() -> HttpRequest {
    HttpRequest {
        method: "POST".into(),
        url: "http://127.0.0.1/".into(),
        version: mio_plugin_api::HttpVersion::Http11,
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
    let mut session = session(&lab);
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
            mio_plugin_api::HttpResponse {
                status: 200,
                version: mio_plugin_api::HttpVersion::Http11,
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
    lab.host_result(&mut session, 7, response.clone()).unwrap();
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
    lab.host_result(&mut session, 8, response).unwrap();
    assert_eq!(lab.response.as_ref(), Some(&cleared));
    assert_eq!(lab.context.as_ref().unwrap().resource, next.resource);
    lab.snapshot().validate().unwrap();
}

#[test]
fn closing_a_request_preserves_edits_and_discards_a_late_writeback_after_reopen() {
    let mut lab = Lab::new();
    let mut session = session(&lab);
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
    assert!(lab.close(&mut session, context.resource).unwrap());
    assert!(!lab.close(&mut session, context.resource).unwrap());
    assert!(lab.context.is_none());
    assert_eq!(lab.decoded, retained);
    lab.load(context.clone()).unwrap();
    lab.host_result(
        &mut session,
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
    let mut session = session(&lab);
    lab.event(
        &mut session,
        UiEvent {
            node_id: 3,
            event: Some(ui_event::Event::SplitResized(0.6)),
            editors: vec![],
        },
        3,
    )
    .unwrap();
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
    let mut session = session(&lab);
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
    let old_record = mio_plugin_api::FlowRecord {
        summary: mio_plugin_api::FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: mio_plugin_api::TrafficSource::Proxy,
            method: "POST".into(),
            url: request().url,
            status: None,
            state: mio_plugin_api::FlowState::Complete,
            request_bytes: 8,
            response_bytes: 0,
            started_unix_ms: 1,
            elapsed_ms: 1,
        },
        request: request(),
        response: None,
    };
    lab.host_result(
        &mut session,
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
    let mut session = session(&lab);
    lab.viewport_revision = 5;
    lab.pending
        .insert(9, PendingService::History { viewport_revision: 5 });
    lab.host_result(
        &mut session,
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
