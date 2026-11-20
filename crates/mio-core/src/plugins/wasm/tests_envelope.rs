use std::time::Duration;

use anyhow::{Context, Result, bail};
use mio_plugin_api::{
    HttpRequest, HttpResponse, ProjectKey, Uuid,
    panel::{EditorFormat, EditorSnapshot, Empty, PanelState, UiEvent, node, ui_event},
    protocol::{
        self, ChannelIdentity, CommitEditor, CommitResponseEditor, Envelope, HeaderDto, Hello,
        HostCall, HostResult, HttpVersionDto, OpenRequest, OpenResponse, Permission, RequestDto,
        ResponseDto, envelope, host_call, host_result,
    },
};
use tokio_util::sync::CancellationToken;

use super::*;

fn test_identity() -> ChannelIdentity {
    ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
    }
}

fn env_body(identity: ChannelIdentity, req_id: u64, rev: u64, body: envelope::Body) -> Envelope {
    Envelope::new(identity, req_id, rev, body, Duration::from_secs(5))
}

fn noop_host(
    _req: &[u8],
    _out: &mut [u8],
    _env_abi: bool,
    _cancel: &CancellationToken,
) -> Result<usize> {
    bail!("unexpected host_call during hook or local operation")
}

fn sample_request(body: &[u8]) -> RequestDto {
    RequestDto {
        method: "POST".into(),
        url: "https://example.test/api?v=1".into(),
        version: HttpVersionDto::Http11 as i32,
        headers: vec![
            HeaderDto {
                name: "Host".into(),
                value: b"example.test".to_vec(),
            },
            HeaderDto {
                name: "X-Dup".into(),
                value: b"one".to_vec(),
            },
            HeaderDto {
                name: "X-Dup".into(),
                value: b"two".to_vec(),
            },
        ],
        body: body.to_vec(),
        trailers: vec![HeaderDto {
            name: "X-Trail".into(),
            value: b"t1".to_vec(),
        }],
    }
}

fn sample_response(body: &[u8]) -> ResponseDto {
    ResponseDto {
        status: 201,
        version: HttpVersionDto::Http11 as i32,
        headers: vec![
            HeaderDto {
                name: "Set-Cookie".into(),
                value: b"a=1".to_vec(),
            },
            HeaderDto {
                name: "Set-Cookie".into(),
                value: b"b=2".to_vec(),
            },
        ],
        body: body.to_vec(),
        trailers: vec![HeaderDto {
            name: "X-Resp-Trail".into(),
            value: b"rt".to_vec(),
        }],
    }
}

fn apply_panel_reply(panel: &mut PanelState, reply: Option<envelope::Body>) -> Result<()> {
    match reply.context("expected panel reply")? {
        envelope::Body::Snapshot(snapshot) => panel.replace(snapshot).map_err(anyhow::Error::msg),
        envelope::Body::Patch(patch) => panel.apply(&patch).map_err(anyhow::Error::msg),
        other => bail!("expected Snapshot or Patch, got {other:?}"),
    }
}

fn is_rejected(outcome: Result<Option<envelope::Body>>) -> bool {
    matches!(outcome, Err(_) | Ok(Some(envelope::Body::Error(_))))
}

#[test]
fn hello_and_ui_events_handle_open_panel_preferences_and_u64_revisions() -> Result<()> {
    let module = WasmModule::parse(&build_envelope_wasm_module())?;
    assert!(module.is_envelope_abi());
    let mut vm = WasmInstance::new(module)?;
    let tok = CancellationToken::new();
    let id = test_identity();

    let hello = env_body(
        id,
        1,
        0,
        envelope::Body::Hello(Hello {
            max_frame_bytes: protocol::MAX_FRAME_BYTES as u32,
            max_operation_ms: 5_000,
            granted_permissions: vec![Permission::Storage as i32, Permission::TrafficRead as i32],
        }),
    );
    let Some(envelope::Body::Snapshot(initial)) =
        dispatch_wasm_envelope(&mut vm, &hello, WASM_CALL_FUEL, &tok, &mut noop_host)?
    else {
        bail!("Hello must return initial PanelSnapshot");
    };
    let mut panel = PanelState::new(initial).map_err(anyhow::Error::msg)?;
    let Some(node::Kind::Editor(ed)) = panel.node(30).and_then(|n| n.kind.as_ref()) else {
        bail!("initial panel must expose editor node 30");
    };
    assert_eq!(ed.document_id, 100);
    assert_eq!(ed.format, EditorFormat::Binary as i32);

    let high_rev = (1u64 << 35) + 41;
    let mut high_snap = panel.snapshot().clone();
    high_snap.revision = high_rev;
    panel = PanelState::new(high_snap.clone()).map_err(anyhow::Error::msg)?;
    let _ = dispatch_wasm_envelope(
        &mut vm,
        &env_body(id, 2, high_rev, envelope::Body::Resync(high_snap)),
        WASM_CALL_FUEL,
        &tok,
        &mut noop_host,
    )?;

    let mut seen_pref = None;
    let mut pref_host =
        |req: &[u8], out: &mut [u8], env_abi: bool, _: &CancellationToken| -> Result<usize> {
            assert!(env_abi);
            let call = HostCall::decode_bytes(req).map_err(anyhow::Error::msg)?;
            if let Some(host_call::Operation::PreferencesSet(entry)) = call.operation {
                seen_pref = Some((entry.key, entry.value));
            }
            let encoded = HostResult {
                outcome: Some(host_result::Outcome::Done(Empty {})),
            }
            .encode_bytes();
            out[..encoded.len()].copy_from_slice(&encoded);
            Ok(encoded.len())
        };
    let click = env_body(
        id,
        3,
        high_rev,
        envelope::Body::UiEvent(UiEvent {
            node_id: 10,
            event: Some(ui_event::Event::Click(Empty {})),
            editors: vec![],
        }),
    );
    let Some(envelope::Body::Patch(patch)) =
        dispatch_wasm_envelope(&mut vm, &click, WASM_CALL_FUEL, &tok, &mut pref_host)?
    else {
        bail!("clicking node 10 must return PanelPatch");
    };
    assert_eq!(patch.base_revision, high_rev);
    assert_eq!(patch.revision, high_rev + 1);
    panel.apply(&patch).map_err(anyhow::Error::msg)?;
    assert_eq!(
        seen_pref,
        Some(("wasm_tag".to_owned(), b"envelope-v13".to_vec()))
    );

    let focus = env_body(
        id,
        4,
        panel.snapshot().revision,
        envelope::Body::UiEvent(UiEvent {
            node_id: 30,
            event: Some(ui_event::Event::Focus(true)),
            editors: vec![],
        }),
    );
    let ack = dispatch_wasm_envelope(&mut vm, &focus, WASM_CALL_FUEL, &tok, &mut noop_host)?;
    assert!(matches!(ack, Some(envelope::Body::Ack(_))));
    Ok(())
}

#[test]
fn open_commit_and_close_editors_preserve_messages_and_check_u64_revisions() -> Result<()> {
    let mut vm = WasmInstance::new(WasmModule::parse(&build_envelope_wasm_module())?)?;
    let tok = CancellationToken::new();
    let id = test_identity();

    let Some(envelope::Body::Snapshot(initial)) = dispatch_wasm_envelope(
        &mut vm,
        &env_body(
            id,
            1,
            0,
            envelope::Body::Hello(Hello {
                max_frame_bytes: protocol::MAX_FRAME_BYTES as u32,
                max_operation_ms: 5_000,
                granted_permissions: vec![
                    Permission::TrafficRead as i32,
                    Permission::DraftWrite as i32,
                ],
            }),
        ),
        WASM_CALL_FUEL,
        &tok,
        &mut noop_host,
    )?
    else {
        bail!("expected initial snapshot");
    };
    let mut panel = PanelState::new(initial).map_err(anyhow::Error::msg)?;

    let req_res = Uuid::new_v4();
    let req_msg_rev = (1u64 << 36) + 7;
    let base_req = sample_request(b"orig\x00\xffreq");
    let open_req = env_body(
        id,
        2,
        panel.snapshot().revision,
        envelope::Body::OpenRequest(OpenRequest {
            resource_id: req_res.as_bytes().to_vec(),
            message_revision: req_msg_rev,
            read_only: false,
            request: Some(base_req.clone()),
        }),
    );
    let reply = dispatch_wasm_envelope(&mut vm, &open_req, WASM_CALL_FUEL, &tok, &mut noop_host)?;
    apply_panel_reply(&mut panel, reply)?;
    let ed = panel.editor(100).context("editor 100 missing")?;
    assert_eq!(ed.contents, b"orig\x00\xffreq");
    assert!(!ed.read_only);

    let truncated_rev = u64::from(req_msg_rev as u32);
    let bad_rev_commit = env_body(
        id,
        3,
        panel.snapshot().revision,
        envelope::Body::CommitEditor(CommitEditor {
            resource_id: req_res.as_bytes().to_vec(),
            message_revision: truncated_rev,
            request: Some(base_req.clone()),
            editor: Some(EditorSnapshot {
                document_id: 100,
                revision: ed.revision + 1,
                contents: b"tampered".to_vec(),
                ..Default::default()
            }),
        }),
    );
    assert!(is_rejected(dispatch_wasm_envelope(
        &mut vm,
        &bad_rev_commit,
        WASM_CALL_FUEL,
        &tok,
        &mut noop_host,
    )));

    let doc_rev = (1u64 << 34) + 9;
    let good_commit = env_body(
        id,
        4,
        panel.snapshot().revision,
        envelope::Body::CommitEditor(CommitEditor {
            resource_id: req_res.as_bytes().to_vec(),
            message_revision: req_msg_rev,
            request: Some(base_req),
            editor: Some(EditorSnapshot {
                document_id: 100,
                revision: doc_rev,
                contents: b"edited\x00\xfebody".to_vec(),
                ..Default::default()
            }),
        }),
    );
    let Some(envelope::Body::Committed(committed)) =
        dispatch_wasm_envelope(&mut vm, &good_commit, WASM_CALL_FUEL, &tok, &mut noop_host)?
    else {
        bail!("expected Committed");
    };
    assert_eq!(committed.resource_id, req_res.as_bytes());
    assert_eq!(committed.message_revision, req_msg_rev);
    assert_eq!(committed.document_id, 100);
    assert_eq!(committed.document_revision, doc_rev);
    let updated_req: HttpRequest = committed
        .request
        .context("missing committed request")?
        .try_into()
        .map_err(anyhow::Error::msg)?;
    assert_eq!(updated_req.body, b"edited\x00\xfebody");
    assert_eq!(
        updated_req
            .headers
            .iter()
            .filter(|h| h.name == "X-Dup")
            .count(),
        2
    );
    assert_eq!(updated_req.trailers.len(), 1);

    let ro_open = env_body(
        id,
        5,
        panel.snapshot().revision,
        envelope::Body::OpenRequest(OpenRequest {
            resource_id: req_res.as_bytes().to_vec(),
            message_revision: req_msg_rev + 1,
            read_only: true,
            request: Some(sample_request(b"ro")),
        }),
    );
    let ro_reply = dispatch_wasm_envelope(&mut vm, &ro_open, WASM_CALL_FUEL, &tok, &mut noop_host)?;
    apply_panel_reply(&mut panel, ro_reply)?;
    let ro_commit = env_body(
        id,
        6,
        panel.snapshot().revision,
        envelope::Body::CommitEditor(CommitEditor {
            resource_id: req_res.as_bytes().to_vec(),
            message_revision: req_msg_rev + 1,
            request: Some(sample_request(b"ro")),
            editor: Some(EditorSnapshot {
                document_id: 100,
                revision: panel.editor(100).unwrap().revision + 1,
                contents: b"illegal".to_vec(),
                ..Default::default()
            }),
        }),
    );
    assert!(is_rejected(dispatch_wasm_envelope(
        &mut vm,
        &ro_commit,
        WASM_CALL_FUEL,
        &tok,
        &mut noop_host,
    )));

    let resp_res = Uuid::new_v4();
    let resp_rev = (1u64 << 37) + 13;
    let base_resp = sample_response(b"\x89PNG\r\n\x1a\n");
    let open_resp = env_body(
        id,
        7,
        panel.snapshot().revision,
        envelope::Body::OpenResponse(OpenResponse {
            resource_id: resp_res.as_bytes().to_vec(),
            message_revision: resp_rev,
            read_only: false,
            response: Some(base_resp.clone()),
        }),
    );
    let resp_reply =
        dispatch_wasm_envelope(&mut vm, &open_resp, WASM_CALL_FUEL, &tok, &mut noop_host)?;
    apply_panel_reply(&mut panel, resp_reply)?;
    assert_eq!(panel.editor(100).unwrap().contents, b"\x89PNG\r\n\x1a\n");

    let resp_doc_rev = panel.editor(100).unwrap().revision + 1;
    let commit_resp = env_body(
        id,
        8,
        panel.snapshot().revision,
        envelope::Body::CommitResponseEditor(CommitResponseEditor {
            resource_id: resp_res.as_bytes().to_vec(),
            message_revision: resp_rev,
            response: Some(base_resp.clone()),
            editor: Some(EditorSnapshot {
                document_id: 100,
                revision: resp_doc_rev,
                contents: b"resp\x00\x80ok".to_vec(),
                ..Default::default()
            }),
        }),
    );
    let Some(envelope::Body::ResponseCommitted(rc)) =
        dispatch_wasm_envelope(&mut vm, &commit_resp, WASM_CALL_FUEL, &tok, &mut noop_host)?
    else {
        bail!("expected ResponseCommitted");
    };
    assert_eq!(rc.message_revision, resp_rev);
    assert_eq!(rc.document_revision, resp_doc_rev);
    let updated_resp: HttpResponse = rc
        .response
        .context("missing committed response")?
        .try_into()
        .map_err(anyhow::Error::msg)?;
    assert_eq!(updated_resp.status, 201);
    assert_eq!(updated_resp.body, b"resp\x00\x80ok");
    assert_eq!(updated_resp.headers.len(), 2);
    assert_eq!(updated_resp.trailers.len(), 1);

    let close = env_body(
        id,
        9,
        panel.snapshot().revision,
        envelope::Body::CloseRequest(protocol::CloseRequest {
            resource_id: resp_res.as_bytes().to_vec(),
        }),
    );
    let close_reply =
        dispatch_wasm_envelope(&mut vm, &close, WASM_CALL_FUEL, &tok, &mut noop_host)?;
    assert!(matches!(
        close_reply,
        Some(envelope::Body::Ack(_) | envelope::Body::Snapshot(_) | envelope::Body::Patch(_))
    ));
    let after_close = env_body(
        id,
        10,
        panel.snapshot().revision,
        envelope::Body::CommitResponseEditor(CommitResponseEditor {
            resource_id: resp_res.as_bytes().to_vec(),
            message_revision: resp_rev,
            response: Some(base_resp),
            editor: Some(EditorSnapshot {
                document_id: 100,
                revision: resp_doc_rev + 1,
                contents: b"stale".to_vec(),
                ..Default::default()
            }),
        }),
    );
    assert!(is_rejected(dispatch_wasm_envelope(
        &mut vm,
        &after_close,
        WASM_CALL_FUEL,
        &tok,
        &mut noop_host,
    )));
    Ok(())
}
