use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use mio_plugin_api::{
    ProjectKey, Uuid,
    panel::{Empty, UiEvent, ui_event},
    protocol::{
        self, ChannelIdentity, ContextMenuCall, Envelope, HeaderDto, HookAction, HostResult,
        HttpHookCall, HttpVersionDto, OpenRequest, PassiveCheckCall, PayloadCall, PayloadOp,
        RequestDto, ResponseDto, SessionActionCall, SourceDto, WebSocketHookCall, envelope,
        host_call, host_result,
    },
};
use tokio::sync::mpsc;
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

fn noop_host(_: &[u8], _: &mut [u8], _: bool, _: &CancellationToken) -> Result<usize> {
    bail!("unexpected host_call during hook dispatch")
}

fn call_env(
    vm: &mut WasmInstance,
    id: ChannelIdentity,
    seq: u64,
    body: envelope::Body,
) -> Result<Option<envelope::Body>> {
    let tok = CancellationToken::new();
    dispatch_wasm_envelope(
        vm,
        &env_body(id, seq, 1, body),
        WASM_CALL_FUEL,
        &tok,
        &mut noop_host,
    )
}

fn req_dto() -> RequestDto {
    RequestDto {
        method: "GET".into(),
        url: "http://127.0.0.1:8080/items".into(),
        version: HttpVersionDto::Http11 as i32,
        headers: vec![HeaderDto {
            name: "Host".into(),
            value: b"127.0.0.1:8080".to_vec(),
        }],
        body: vec![],
        trailers: vec![],
    }
}

fn resp_dto(headers: Vec<HeaderDto>, body: &[u8]) -> ResponseDto {
    ResponseDto {
        status: 200,
        version: HttpVersionDto::Http11 as i32,
        headers,
        body: body.to_vec(),
        trailers: vec![],
    }
}

#[test]
fn all_hooks_payload_limits_passive_check_and_memory_reuse_behave_exact() -> Result<()> {
    let mut vm = WasmInstance::new(WasmModule::parse(&build_envelope_wasm_module())?)?;
    let tok = CancellationToken::new();
    let id = test_identity();
    let flow_id = Uuid::new_v4().as_bytes().to_vec();

    let Some(envelope::Body::HttpHookResult(req_hook)) = call_env(
        &mut vm,
        id,
        1,
        envelope::Body::HttpHookCall(HttpHookCall {
            flow_id: flow_id.clone(),
            source: SourceDto::Proxy as i32,
            plugin_id: String::new(),
            response_stage: false,
            response_head_only: false,
            request: Some(req_dto()),
            response: None,
        }),
    )?
    else {
        bail!("HttpHookCall(request) must return HttpHookResult, never PanelPatch");
    };
    assert_eq!(req_hook.action, HookAction::Modify as i32);
    assert!(
        req_hook
            .request
            .context("modified request")?
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("x-mio-wasm-hook") && h.value == b"sandboxed")
    );

    let Some(envelope::Body::HttpHookResult(resp_hook)) = call_env(
        &mut vm,
        id,
        2,
        envelope::Body::HttpHookCall(HttpHookCall {
            flow_id: flow_id.clone(),
            source: SourceDto::Proxy as i32,
            plugin_id: String::new(),
            response_stage: true,
            response_head_only: false,
            request: Some(req_dto()),
            response: Some(resp_dto(vec![], b"ok")),
        }),
    )?
    else {
        bail!("HttpHookCall(response) must return HttpHookResult");
    };
    assert_eq!(resp_hook.action, HookAction::Modify as i32);
    assert!(
        resp_hook
            .response
            .context("modified response")?
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("x-mio-wasm-resp") && h.value == b"verified")
    );

    let Some(envelope::Body::WebSocketHookResult(ws_res)) = call_env(
        &mut vm,
        id,
        3,
        envelope::Body::WebSocketHookCall(WebSocketHookCall {
            flow_id: flow_id.clone(),
            message_id: Uuid::new_v4().as_bytes().to_vec(),
            url: "ws://127.0.0.1:8080/ws".into(),
            client_to_server: true,
            is_binary: true,
            payload: b"\x00hello".to_vec(),
        }),
    )?
    else {
        bail!("WebSocketHookCall must return WebSocketHookResult");
    };
    assert_eq!(ws_res.action, HookAction::Modify as i32);
    assert_eq!(ws_res.payload, b"[wasm] \x00hello");

    let Some(envelope::Body::SessionActionResult(sess_res)) = call_env(
        &mut vm,
        id,
        4,
        envelope::Body::SessionActionCall(SessionActionCall {
            source: SourceDto::Repeater as i32,
            request: Some(req_dto()),
        }),
    )?
    else {
        bail!("SessionActionCall must return SessionActionResult");
    };
    assert!(sess_res.modified);
    assert!(
        sess_res
            .request
            .context("session request")?
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("x-mio-wasm-session") && h.value == b"token-v1")
    );

    for (max_items, expected) in [
        (0u32, vec![]),
        (1, vec![b"wasm-gen-1".to_vec()]),
        (2, vec![b"wasm-gen-1".to_vec(), b"wasm-gen-2".to_vec()]),
        (
            9,
            vec![
                b"wasm-gen-1".to_vec(),
                b"wasm-gen-2".to_vec(),
                b"wasm-gen-3".to_vec(),
            ],
        ),
    ] {
        let Some(envelope::Body::PayloadResult(r#gen)) = call_env(
            &mut vm,
            id,
            5,
            envelope::Body::PayloadCall(PayloadCall {
                op: PayloadOp::Generate as i32,
                config: String::new(),
                inputs: vec![],
                max_items,
            }),
        )?
        else {
            bail!("PayloadCall(Generate) must return PayloadResult");
        };
        assert_eq!(r#gen.outputs, expected);
    }

    let Some(envelope::Body::PayloadResult(proc)) = call_env(
        &mut vm,
        id,
        6,
        envelope::Body::PayloadCall(PayloadCall {
            op: PayloadOp::Process as i32,
            config: String::new(),
            inputs: vec![b"a\x00".to_vec(), b"b\xff".to_vec()],
            max_items: 10,
        }),
    )?
    else {
        bail!("PayloadCall(Process) must return PayloadResult");
    };
    assert_eq!(
        proc.outputs,
        vec![b"wasm:a\x00".to_vec(), b"wasm:b\xff".to_vec()]
    );

    let near_limit = vec![vec![b'x'; protocol::MAX_INLINE_BODY / 2]; 2];
    let over_out = dispatch_wasm_envelope(
        &mut vm,
        &env_body(
            id,
            7,
            1,
            envelope::Body::PayloadCall(PayloadCall {
                op: PayloadOp::Process as i32,
                config: String::new(),
                inputs: near_limit,
                max_items: 10,
            }),
        ),
        WASM_CALL_FUEL * 4,
        &tok,
        &mut noop_host,
    );
    match over_out {
        Err(_) | Ok(Some(envelope::Body::Error(_))) => {}
        Ok(Some(envelope::Body::PayloadResult(res))) => {
            let validate_env = env_body(id, 8, 1, envelope::Body::PayloadResult(res));
            assert!(
                validate_env.validate().is_err(),
                "oversized payload output must fail validation and never silently truncate"
            );
        }
        other => bail!("unexpected oversized payload result: {other:?}"),
    }

    let Some(envelope::Body::ContextMenuResult(ctx)) = call_env(
        &mut vm,
        id,
        9,
        envelope::Body::ContextMenuCall(ContextMenuCall {
            source: SourceDto::Repeater as i32,
            action_id: "default".into(),
            selection_start: 0,
            selection_end: 0,
            selected_bytes: vec![],
            request: Some(req_dto()),
            response: None,
        }),
    )?
    else {
        bail!("ContextMenuCall must return ContextMenuResult");
    };
    assert!(!ctx.status_message.is_empty() && !ctx.log_line.is_empty());

    let mut req_only_marker = req_dto();
    req_only_marker.body = b"PLUGIN_VULN_MARKER".to_vec();
    req_only_marker.headers.push(HeaderDto {
        name: "X-Plugin-Audit-Flag".into(),
        value: b"1".to_vec(),
    });
    let Some(envelope::Body::PassiveCheckResult(clean_passive)) = call_env(
        &mut vm,
        id,
        10,
        envelope::Body::PassiveCheckCall(PassiveCheckCall {
            flow_id: Uuid::new_v4().to_string(),
            url: req_only_marker.url.clone(),
            request: Some(req_only_marker),
            response: Some(resp_dto(vec![], b"clean response body")),
        }),
    )?
    else {
        bail!("PassiveCheckCall(clean) must return PassiveCheckResult");
    };
    assert!(
        clean_passive.findings.is_empty(),
        "ordinary traffic or request-only markers must not trigger response passive findings"
    );

    for flagged_resp in [
        resp_dto(
            vec![HeaderDto {
                name: "X-Plugin-Audit-Flag".into(),
                value: b"true".to_vec(),
            }],
            b"normal",
        ),
        resp_dto(vec![], b"body has PLUGIN_VULN_MARKER inside"),
    ] {
        let Some(envelope::Body::PassiveCheckResult(hit)) = call_env(
            &mut vm,
            id,
            11,
            envelope::Body::PassiveCheckCall(PassiveCheckCall {
                flow_id: Uuid::new_v4().to_string(),
                url: "http://127.0.0.1:8080/vuln".into(),
                request: Some(req_dto()),
                response: Some(flagged_resp),
            }),
        )?
        else {
            bail!("PassiveCheckCall(flagged) must return PassiveCheckResult");
        };
        assert!(!hit.findings.is_empty());
        assert!(!hit.findings[0].title.is_empty());
    }

    let steady_mem = vm.mem.len();
    for i in 0..250u64 {
        let reply = call_env(
            &mut vm,
            id,
            100 + i,
            envelope::Body::WebSocketHookCall(WebSocketHookCall {
                flow_id: flow_id.clone(),
                message_id: Uuid::new_v4().as_bytes().to_vec(),
                url: "ws://127.0.0.1/loop".into(),
                client_to_server: false,
                is_binary: false,
                payload: vec![b'z'; 1024],
            }),
        )?;
        assert!(matches!(
            reply,
            Some(envelope::Body::WebSocketHookResult(_))
        ));
    }
    assert_eq!(
        vm.mem.len(),
        steady_mem,
        "repeated envelope dispatches must reuse linear memory without growth"
    );
    Ok(())
}

#[tokio::test]
async fn sandbox_worker_drives_envelope_hooks_and_matches_packaged_wasm_asset() -> Result<()> {
    let expected = build_envelope_wasm_module();
    let asset_path = std::env::var_os("MIO_TEST_REPO_ROOT")
        .map(|root| PathBuf::from(root).join("plugins/wasm-sandbox-lab/sandbox.wasm"))
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../plugins/wasm-sandbox-lab/sandbox.wasm")
        });
    let disk_bytes = std::fs::read(&asset_path)?;
    assert_eq!(
        disk_bytes, expected,
        "sandbox.wasm must match build_envelope_wasm_module"
    );

    let module = WasmModule::parse(&disk_bytes)?;
    assert!(module.is_envelope_abi());
    let id = test_identity();
    let (tx, writes) = mpsc::channel(16);
    let (wire_tx, mut wire_rx) = mpsc::channel(16);
    let cancel = CancellationToken::new();
    let worker = spawn_sandbox_worker(
        id,
        "lab.wasm".into(),
        module,
        writes,
        wire_tx,
        cancel.clone(),
    );

    tx.send(env_body(
        id,
        10,
        1,
        envelope::Body::HttpHookCall(HttpHookCall {
            flow_id: Uuid::new_v4().as_bytes().to_vec(),
            source: SourceDto::Repeater as i32,
            plugin_id: String::new(),
            response_stage: false,
            response_head_only: false,
            request: Some(req_dto()),
            response: None,
        }),
    ))
    .await?;
    let Some(super::Wire::Frame(hook_frame)) = wire_rx.recv().await else {
        bail!("expected hook frame");
    };
    assert_eq!(hook_frame.in_reply_to, 10);
    assert_eq!(hook_frame.revision, 1);
    assert!(matches!(
        hook_frame.body,
        Some(envelope::Body::HttpHookResult(_))
    ));

    tx.send(env_body(
        id,
        11,
        1,
        envelope::Body::OpenRequest(OpenRequest {
            resource_id: Uuid::new_v4().as_bytes().to_vec(),
            message_revision: 3,
            read_only: false,
            request: Some(req_dto()),
        }),
    ))
    .await?;
    let Some(super::Wire::Frame(open_frame)) = wire_rx.recv().await else {
        bail!("expected open frame");
    };
    assert_eq!(open_frame.in_reply_to, 11);
    assert!(open_frame.revision >= 2);
    let cur_rev = open_frame.revision;

    tx.send(env_body(
        id,
        12,
        cur_rev,
        envelope::Body::UiEvent(UiEvent {
            node_id: 10,
            event: Some(ui_event::Event::Click(Empty {})),
            editors: vec![],
        }),
    ))
    .await?;
    let Some(super::Wire::Frame(host_frame)) = wire_rx.recv().await else {
        bail!("expected HostCall frame");
    };
    let Some(envelope::Body::HostCall(call)) = host_frame.body else {
        bail!("expected PreferencesSet HostCall");
    };
    assert!(matches!(
        call.operation,
        Some(host_call::Operation::PreferencesSet(_))
    ));
    let mut host_reply = env_body(
        id,
        13,
        cur_rev,
        envelope::Body::HostResult(HostResult {
            outcome: Some(host_result::Outcome::Done(Empty {})),
        }),
    );
    host_reply.in_reply_to = host_frame.request_id;
    tx.send(host_reply).await?;
    let Some(super::Wire::Frame(patch_frame)) = wire_rx.recv().await else {
        bail!("expected Patch frame");
    };
    assert_eq!(patch_frame.in_reply_to, 12);
    assert_eq!(patch_frame.revision, cur_rev + 1);

    tx.send(env_body(
        id,
        14,
        cur_rev + 1,
        envelope::Body::Shutdown(Empty {}),
    ))
    .await?;
    tokio::time::timeout(Duration::from_secs(2), worker).await??;
    Ok(())
}
