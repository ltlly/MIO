use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{
    tests_r4::{drain_until_plugin_op, write_wasm_package},
    *,
};
use crate::plugins::wasm::build_demo_wasm_module;

#[tokio::test]
async fn plugin_send_increments_depth_header_and_passive_findings_persist_to_audit() {
    let origin = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_addr = origin.local_addr().unwrap();
    let seen_task = tokio::spawn(async move {
        let mut headers = Vec::new();
        for _ in 0..2 {
            let (mut sock, _) = origin.accept().await.unwrap();
            let mut buf = vec![0u8; 2048];
            let n = sock.read(&mut buf).await.unwrap();
            headers.push(String::from_utf8_lossy(&buf[..n]).into_owned());
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await
                .unwrap();
        }
        headers
    });

    let dir = tempfile::tempdir().unwrap();
    write_wasm_package(
        dir.path(),
        "lab.net",
        "0.1.0",
        &build_demo_wasm_module(),
        &["network.send", "traffic.read"],
        &["suite_panel"],
    );
    let (tx, mut rx) = mpsc::channel(128);
    let mut state = State::new(tx, CancellationToken::new()).unwrap();
    let insp_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            insp_op,
            PluginAction::Inspect {
                directory: dir.path().to_owned(),
            },
        )
        .await
        .unwrap();
    let PluginOutcome::Inspected(insp) = drain_until_plugin_op(&mut state, &mut rx, insp_op)
        .await
        .unwrap()
    else {
        panic!("inspect");
    };
    let load_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            load_op,
            PluginAction::Load {
                inspection_id: insp.id,
                native_trusted: true,
                grants: vec![Permission::NetworkSend, Permission::TrafficRead],
                replace_instance: None,
            },
        )
        .await
        .unwrap();
    let PluginOutcome::Loaded { info, .. } = drain_until_plugin_op(&mut state, &mut rx, load_op)
        .await
        .unwrap()
    else {
        panic!("load");
    };

    for (req_id, initial_headers) in [
        (1u64, vec![]),
        (
            2u64,
            vec![Header {
                name: "X-Mio-Plugin-Depth".into(),
                value: b"1".to_vec(),
            }],
        ),
    ] {
        let (reply, reply_rx) = oneshot::channel();
        state
            .plugin_service(ServiceRequest {
                identity: info.identity,
                plugin_id: info.manifest.id.clone(),
                request_id: req_id,
                deadline: tokio::time::Instant::now() + Duration::from_secs(5),
                operation: ServiceOperation::NetworkSend {
                    request: HttpRequest {
                        method: "GET".into(),
                        url: format!("http://{origin_addr}/d{req_id}"),
                        version: HttpVersion::Http11,
                        headers: initial_headers,
                        body: vec![],
                        trailers: vec![],
                    },
                },
                cancel: CancellationToken::new(),
                reply,
            })
            .await;
        loop {
            let sig = state.plugins.next_signal().await;
            let done = matches!(sig, Signal::Network(k, _) if k.1 == req_id);
            state.plugin_signal(sig).await;
            if done {
                break;
            }
        }
        assert!(reply_rx.await.unwrap().is_ok());
    }
    let captured = seen_task.await.unwrap();
    assert!(
        captured[0]
            .to_ascii_lowercase()
            .contains("x-mio-plugin-depth: 1")
    );
    assert!(
        captured[1]
            .to_ascii_lowercase()
            .contains("x-mio-plugin-depth: 2")
    );

    let flow_id = Uuid::new_v4();
    let resp = HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![],
        body: b"body PLUGIN_VULN_MARKER end".to_vec(),
        trailers: vec![],
    };
    state
        .finish_passive_checked(
            state.project,
            flow_id,
            "http://127.0.0.1/vuln".into(),
            resp,
            vec![protocol::PluginFindingDto {
                title: "插件被动发现".into(),
                detail: "详情".into(),
                remediation: "修复".into(),
                severity: "High".into(),
                confidence: "Certain".into(),
                parameter: "body".into(),
                evidence: "PLUGIN_VULN_MARKER".into(),
            }],
        )
        .await;
    let audit_snap = state.store.read_audit_state().unwrap();
    assert_eq!(audit_snap.findings.len(), 1);
    assert_eq!(audit_snap.findings[0].issue_id, "0x00900100");
    assert_eq!(audit_snap.findings[0].flow_id, Some(flow_id));
    assert!(!audit_snap.findings[0].response_highlights.is_empty());
    let mut saw_audit_event = false;
    while let Ok(ev) = rx.try_recv() {
        if let Event::Audit(AuditEvent::State {
            result: Ok(snap), ..
        }) = ev
            && snap.findings.len() == 1
        {
            saw_audit_event = true;
        }
    }
    assert!(saw_audit_event);
    state.stop_plugins().await.unwrap();
}
