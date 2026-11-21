use base64::{Engine, engine::general_purpose::STANDARD};
use tokio::io::AsyncReadExt;

use super::*;

#[tokio::test]
async fn application_independent_editors_publish_opened_content_and_commit_latest_bytes() {
    let directory = package();
    let mut app = App::start().await;
    let first = app.add_tab("http://127.0.0.1:9/plugin", "第一份").await;
    let second = app.add_tab("http://127.0.0.1:9/plugin", "第二份").await;
    let source = app.load(&directory, None).await;
    let mut editors = vec![];
    for (tab_id, text) in [(first, "第一份"), (second, "第二份")] {
        let revision = app
            .workspace
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .unwrap()
            .draft
            .revision;
        let op = app
            .action(PluginAction::CreateEditor {
                source_instance: source.identity.instance_id,
                tab_id,
                draft_revision: revision,
            })
            .await;
        let PluginOutcome::Loaded { info, panel } = app.outcome(op).await.unwrap() else {
            panic!("editor loaded");
        };
        assert_eq!(
            editor(&panel, 31).contents,
            text.as_bytes(),
            "Loaded must publish the opened request, never the empty handshake panel"
        );
        editors.push(info.identity.instance_id);
    }
    assert_ne!(editors[0], editors[1]);
    let panel = app.panels[&editors[0]].clone();
    let document = editor(&panel, 31);
    let revision = app.workspace.tabs[0].draft.revision;
    let op = app
        .action(PluginAction::CommitEditor {
            instance_id: editors[0],
            tab_id: first,
            draft_revision: revision,
            editor: EditorSnapshot {
                document_id: document.document_id,
                revision: document.revision + 1,
                contents: "最后输入🙂".as_bytes().to_vec(),
                selection_start: 0,
                selection_end: 0,
            },
            expected_panel: None,
        })
        .await;
    let PluginOutcome::Committed { request, .. } = app.outcome(op).await.unwrap() else {
        panic!("commit");
    };
    assert_eq!(
        STANDARD.decode(&request.body).unwrap(),
        "最后输入🙂".as_bytes()
    );
    assert_eq!(
        request
            .headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case("X-Duplicate"))
            .count(),
        2
    );
    assert_eq!(
        editor(&app.panels[&editors[1]], 31).contents,
        "第二份".as_bytes()
    );

    let mut closed = app.workspace.clone();
    let index = closed.tabs.iter().position(|tab| tab.id == second).unwrap();
    closed.closed.push(closed.tabs.remove(index));
    closed.active = Some(first);
    app.save(closed).await;
    while !app.stopped.contains(&editors[1]) {
        app.next().await;
    }
    let op = app
        .action(PluginAction::OpenRequest {
            instance_id: editors[1],
            tab_id: second,
            draft_revision: revision,
        })
        .await;
    assert!(app.outcome(op).await.is_err());
    let op = app
        .action(PluginAction::Disable {
            instance_id: source.identity.instance_id,
        })
        .await;
    assert!(matches!(
        app.outcome(op).await.unwrap(),
        PluginOutcome::Disabled { .. }
    ));
    assert!(app.stopped.contains(&editors[0]));
    app.shutdown();
}

#[tokio::test]
async fn application_multi_native_hooks_payloads_kv_and_reorder_end_to_end() {
    use tokio::io::AsyncWriteExt;

    let native_dir = tempfile::tempdir().unwrap();
    let bin = std::env::var("MIO_TEST_PROTOCOL_LAB_BIN")
        .unwrap_or_else(|_| env!("CARGO_BIN_EXE_protocol-lab").into());
    let entry = if cfg!(windows) {
        "protocol-lab.exe"
    } else {
        "protocol-lab"
    };
    fs::copy(&bin, native_dir.path().join(entry)).unwrap();
    fs::write(
        native_dir.path().join("manifest.json"),
        serde_json::to_vec(&serde_json::json!({
            "id": "lab.protocol",
            "version": "0.1.0",
            "api_version": 1,
            "engine": "native",
            "entry": entry,
            "permissions": [
                "traffic.read", "draft.write", "repeater.open", "network.send",
                "traffic.modify", "session.hook", "payload.process", "storage.kv"
            ],
            "contributions": [
                "suite_panel", "message_editor", "http_hook", "ws_hook",
                "session_action", "payload_generator", "payload_processor"
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let peer_dir = tempfile::tempdir().unwrap();
    let peer_bin = std::env::var("MIO_TEST_PROTOCOL_LAB_WORKER_BIN")
        .unwrap_or_else(|_| env!("CARGO_BIN_EXE_protocol-lab-test-worker").into());
    let peer_entry = if cfg!(windows) {
        "hook-peer.exe"
    } else {
        "hook-peer"
    };
    fs::copy(&peer_bin, peer_dir.path().join(peer_entry)).unwrap();
    fs::write(
        peer_dir.path().join("manifest.json"),
        serde_json::to_vec(&serde_json::json!({
            "id": "lab.hook-peer",
            "version": "0.1.0",
            "api_version": 1,
            "engine": "native",
            "entry": peer_entry,
            "permissions": ["traffic.read", "traffic.modify", "session.hook", "payload.process", "storage.kv"],
            "contributions": ["suite_panel", "http_hook", "ws_hook", "session_action", "payload_generator", "payload_processor"]
        }))
        .unwrap(),
    )
    .unwrap();

    let mut app = App::start().await;
    let all_native_grants = vec![
        Permission::TrafficRead,
        Permission::DraftWrite,
        Permission::RepeaterOpen,
        Permission::NetworkSend,
        Permission::TrafficModify,
        Permission::SessionHook,
        Permission::PayloadProcess,
        Permission::Storage,
    ];
    let insp_n = app
        .action(PluginAction::Inspect {
            directory: native_dir.path().to_owned(),
        })
        .await;
    let PluginOutcome::Inspected(insp_n) = app.outcome(insp_n).await.unwrap() else {
        panic!("native inspect");
    };
    let load_n = app
        .action(PluginAction::Load {
            inspection_id: insp_n.id,
            native_trusted: true,
            grants: all_native_grants,
            replace_instance: None,
        })
        .await;
    let PluginOutcome::Loaded {
        info: native_info, ..
    } = app.outcome(load_n).await.unwrap()
    else {
        panic!("native load");
    };
    let p_native = app
        .panel(native_info.identity.instance_id, |panel| {
            matches!(panel.root.as_ref().and_then(|root| root.find(4)).and_then(|node| node.kind.as_ref()),
                Some(node::Kind::Text(text)) if text.text.contains("条实际流量"))
        })
        .await;
    // Enable auto_tag_hooks (button 17) and test KV StorageSet (button 19) -> StorageGet (button 21).
    let mut kv_snaps = snapshots(&p_native);
    kv_snaps
        .iter_mut()
        .find(|s| s.document_id == 100)
        .unwrap()
        .contents = b"persisted-secret-kv".to_vec();
    kv_snaps
        .iter_mut()
        .find(|s| s.document_id == 100)
        .unwrap()
        .revision += 1;
    app.click(native_info.identity.instance_id, &p_native, 19, kv_snaps)
        .await;
    let p_after_save = app
        .panel(native_info.identity.instance_id, |panel| {
            matches!(panel.root.as_ref().and_then(|root| root.find(4)).and_then(|node| node.kind.as_ref()),
                Some(node::Kind::Text(text)) if text.text.contains("已存入插件 KV（scratch）"))
        })
        .await;
    app.click(native_info.identity.instance_id, &p_after_save, 21, vec![])
        .await;
    let p_after_load = app
        .panel(native_info.identity.instance_id, |panel| {
            matches!(panel.root.as_ref().and_then(|root| root.find(4)).and_then(|node| node.kind.as_ref()),
                Some(node::Kind::Text(text)) if text.text.contains("已从 KV 读取 19 字节到 Decoded"))
                && editor(panel, 31).contents == b"persisted-secret-kv"
        })
        .await;
    app.click(native_info.identity.instance_id, &p_after_load, 17, vec![])
        .await;
    let _p_after_toggle = app
        .panel(native_info.identity.instance_id, |panel| {
            matches!(panel.root.as_ref().and_then(|root| root.find(4)).and_then(|node| node.kind.as_ref()),
                Some(node::Kind::Text(text)) if text.text.contains("已开启自动流量与会话 Hook 标记"))
        })
        .await;

    // Load second native plugin and reorder it above the first.
    let insp_p = app
        .action(PluginAction::Inspect {
            directory: peer_dir.path().to_owned(),
        })
        .await;
    let PluginOutcome::Inspected(insp_p) = app.outcome(insp_p).await.unwrap() else {
        panic!("peer inspect");
    };
    let load_p = app
        .action(PluginAction::Load {
            inspection_id: insp_p.id,
            native_trusted: true,
            grants: vec![
                Permission::TrafficRead,
                Permission::TrafficModify,
                Permission::SessionHook,
                Permission::PayloadProcess,
                Permission::Storage,
            ],
            replace_instance: None,
        })
        .await;
    let PluginOutcome::Loaded {
        info: peer_info, ..
    } = app.outcome(load_p).await.unwrap()
    else {
        panic!("peer load");
    };
    let reorder_op = app
        .action(PluginAction::Reorder {
            instance_id: peer_info.identity.instance_id,
            move_up: true,
        })
        .await;
    let PluginOutcome::Reordered { order } = app.outcome(reorder_op).await.unwrap() else {
        panic!("reorder");
    };
    assert_eq!(
        order,
        vec![
            peer_info.identity.instance_id,
            native_info.identity.instance_id
        ]
    );

    // Test Payload Generate & Process across both native plugins.
    let tab_id = Uuid::new_v4();
    let gen_op = app
        .action(PluginAction::GeneratePayloads {
            instance_id: peer_info.identity.instance_id,
            tab_id,
            config: String::new(),
            max_items: 10,
        })
        .await;
    let PluginOutcome::PayloadsGenerated { payloads, .. } = app.outcome(gen_op).await.unwrap()
    else {
        panic!("gen");
    };
    assert_eq!(
        payloads,
        vec![
            b"peer-gen-1".to_vec(),
            b"peer-gen-2".to_vec(),
            b"peer-gen-3".to_vec()
        ]
    );
    let proc_op = app
        .action(PluginAction::ProcessPayloads {
            instance_id: native_info.identity.instance_id,
            tab_id,
            position: Some(0),
            config: "p:".into(),
            inputs: payloads,
        })
        .await;
    let PluginOutcome::PayloadsProcessed {
        payloads: proc_out, ..
    } = app.outcome(proc_op).await.unwrap()
    else {
        panic!("proc");
    };
    assert_eq!(proc_out[0], b"p:peer-gen-1");

    // Test Proxy HTTP Hook chain across both native plugins against real loopback origin.
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_addr = origin.local_addr().unwrap();
    let origin_task = tokio::spawn(async move {
        let (mut sock, _) = origin.accept().await.unwrap();
        let mut buf = vec![0u8; 4096];
        let n = sock.read(&mut buf).await.unwrap();
        let req_str = String::from_utf8_lossy(&buf[..n]).into_owned();
        sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .await
            .unwrap();
        req_str
    });
    app.client
        .commands
        .send(Command::StartProxy {
            project: app.project,
            listen: "127.0.0.1:0".parse().unwrap(),
        })
        .await
        .unwrap();
    let proxy_addr = loop {
        if let Event::ProxyState {
            address: Some(addr),
            ..
        } = app.next().await
        {
            break addr;
        }
    };
    let mut client_sock = tokio::net::TcpStream::connect(proxy_addr).await.unwrap();
    client_sock
        .write_all(
            format!("GET http://{origin_addr}/hook HTTP/1.1\r\nHost: {origin_addr}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut resp_buf = Vec::new();
    client_sock.read_to_end(&mut resp_buf).await.unwrap();
    let resp_text = String::from_utf8_lossy(&resp_buf);
    let seen_by_origin = timeout(BOUND, origin_task).await.unwrap().unwrap();
    assert!(
        seen_by_origin
            .to_ascii_lowercase()
            .contains("x-mio-peer-hook: chained")
    );
    assert!(
        seen_by_origin
            .to_ascii_lowercase()
            .contains("x-protocol-lab: hooked")
    );
    assert!(
        resp_text
            .to_ascii_lowercase()
            .contains("x-mio-peer-resp: verified")
    );
    assert!(
        resp_text
            .to_ascii_lowercase()
            .contains("x-protocol-lab-resp: observed")
    );
    app.shutdown();
}

#[tokio::test]
async fn application_plugin_active_check_contributes_scanner_findings_end_to_end() {
    use mio_core::{AuditCommand, AuditEvent, AuditPhase, AuditTaskMode, AuditTaskState, Draft};
    use tokio::io::AsyncWriteExt;
    use tokio_util::sync::CancellationToken;

    let native_dir = tempfile::tempdir().unwrap();
    let bin = std::env::var("MIO_TEST_PROTOCOL_LAB_BIN")
        .unwrap_or_else(|_| env!("CARGO_BIN_EXE_protocol-lab").into());
    let entry = if cfg!(windows) {
        "protocol-lab.exe"
    } else {
        "protocol-lab"
    };
    fs::copy(&bin, native_dir.path().join(entry)).unwrap();
    fs::write(
        native_dir.path().join("manifest.json"),
        serde_json::to_vec(&serde_json::json!({
            "id": "lab.protocol",
            "version": "0.1.0",
            "api_version": 1,
            "engine": "native",
            "entry": entry,
            "permissions": ["traffic.read"],
            "contributions": ["suite_panel", "active_check"]
        }))
        .unwrap(),
    )
    .unwrap();

    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_addr = origin.local_addr().unwrap();
    let origin_task = tokio::spawn(async move {
        while let Ok((mut sock, _)) = origin.accept().await {
            let mut buf = vec![0u8; 2048];
            let _ = sock.read(&mut buf).await;
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nclean-page")
                .await;
        }
    });

    let mut app = App::start().await;
    let insp = app
        .action(PluginAction::Inspect {
            directory: native_dir.path().to_owned(),
        })
        .await;
    let PluginOutcome::Inspected(inspected) = app.outcome(insp).await.unwrap() else {
        panic!("inspect");
    };
    let load = app
        .action(PluginAction::Load {
            inspection_id: inspected.id,
            native_trusted: true,
            grants: vec![Permission::TrafficRead],
            replace_instance: None,
        })
        .await;
    assert!(matches!(
        app.outcome(load).await.unwrap(),
        PluginOutcome::Loaded { .. }
    ));

    let task_id = Uuid::new_v4();
    let target = format!("http://{origin_addr}");
    let draft = Draft {
        revision: 1,
        version: mio_core::HttpVersion::Http11,
        target: target.clone(),
        raw: format!("GET /item?q=PLUGIN_ACTIVE_VULN HTTP/1.1\r\nHost: {origin_addr}\r\n\r\n")
            .into_bytes(),
    };
    app.client
        .commands
        .send(Command::Audit(AuditCommand::StartScan {
            project: app.project,
            task_id,
            title: "plugin-active".into(),
            draft: Box::new(draft),
            mode: AuditTaskMode::PassiveAndActive,
            crawl_same_origin: false,
            cancel: CancellationToken::new(),
        }))
        .await
        .unwrap();

    let mut plugin_finding_seen = false;
    loop {
        if let Event::Audit(AuditEvent::TaskUpdated {
            task, new_findings, ..
        }) = app.next().await
            && task.id == task_id
        {
            if new_findings
                .iter()
                .any(|f| f.issue_id == "0x00900101" && f.phase == AuditPhase::Active)
            {
                plugin_finding_seen = true;
            }
            if matches!(
                task.state,
                AuditTaskState::Completed | AuditTaskState::Failed | AuditTaskState::Cancelled
            ) {
                assert_eq!(task.state, AuditTaskState::Completed);
                break;
            }
        }
    }
    assert!(plugin_finding_seen, "expected active_check plugin finding");
    origin_task.abort();
    app.shutdown();
}
