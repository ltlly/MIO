#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::{AiIpcPayload, AiIpcRequest, SlimSaveOptions, call_ai_ipc, default_ipc_socket_path};

async fn recv_until<T>(
    events: &mut mpsc::Receiver<Event>,
    mut f: impl FnMut(Event) -> Option<T>,
) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("timed out waiting for event")
            .expect("event stream closed");
        if let Some(out) = f(event) {
            return out;
        }
    }
}

#[tokio::test]
async fn ai_ipc_status_flows_repeater_open_and_slim_save_end_to_end() {
    assert!(
        default_ipc_socket_path()
            .as_os_str()
            .as_encoded_bytes()
            .len()
            <= if cfg!(unix) { 96 } else { 4096 }
    );

    #[cfg(unix)]
    let dir = tempfile::Builder::new()
        .prefix("mio-ipc-")
        .tempdir_in("/tmp")
        .unwrap();
    #[cfg(not(unix))]
    let dir = tempfile::Builder::new()
        .prefix("mio-ipc-")
        .tempdir()
        .unwrap();
    let project_path = dir.path().join("seed.mio");
    let slim_path = dir.path().join("slim.mio");
    let sock_path = dir.path().join("ai.sock");

    let mut seed_store = ProjectStore::create(Some(&project_path)).unwrap();
    let mut f1 = tests::completed_flow();
    f1.summary.source = TrafficSource::Proxy;
    f1.summary.method = "POST".into();
    f1.summary.url = "http://127.0.0.1:18080/api/login".into();
    f1.summary.status = Some(200);
    f1.request.method = "POST".into();
    f1.request.url = f1.summary.url.clone();
    let orig_req = f1.request.clone();
    f1.request.headers.push(Header {
        name: "X-Rewritten".into(),
        value: b"yes".to_vec(),
    });
    seed_store.put(&f1).unwrap();
    let mut prov = FlowProvenance::default();
    prov.push_request_step(
        &orig_req,
        ProvenanceActor::SessionRule {
            rule_name: "RewriteRule".into(),
        },
        &f1.request,
    );
    seed_store
        .put_flow_provenance(f1.summary.id, &prov)
        .unwrap();

    let mut f2 = tests::completed_flow();
    f2.summary.id = Uuid::new_v4();
    f2.summary.source = TrafficSource::Scanner;
    f2.summary.method = "GET".into();
    f2.summary.url = "http://127.0.0.1:18080/scan/noise".into();
    f2.summary.status = Some(404);
    f2.response.as_mut().unwrap().status = 404;
    f2.response.as_mut().unwrap().body = vec![b'N'; 128 * 1024];
    f2.summary.response_bytes = 128 * 1024;
    seed_store.put(&f2).unwrap();
    seed_store.save(None).unwrap();
    seed_store.close().unwrap();

    let (runtime, mut client) = Runtime::start_with_ipc_socket(sock_path.clone()).unwrap();
    let meta = std::fs::symlink_metadata(&sock_path).unwrap();
    #[cfg(unix)]
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    #[cfg(not(unix))]
    assert!(meta.is_file());

    let _initial_project = recv_until(&mut client.events, |ev| match ev {
        Event::ProjectOpened { project, .. } => Some(project),
        _ => None,
    })
    .await;

    client
        .commands
        .send(Command::OpenProject {
            path: project_path.clone(),
        })
        .await
        .unwrap();
    let (opened_project, mut ui_workspace) = recv_until(&mut client.events, |ev| match ev {
        Event::ProjectOpened {
            project,
            path: Some(p),
            workspace,
        } if p == project_path => Some((project, workspace)),
        _ => None,
    })
    .await;

    let sock_for_blocking = sock_path.clone();
    let f1_id = f1.summary.id;
    let slim_dest = slim_path.to_string_lossy().into_owned();

    // 1. status
    let s1 = sock_for_blocking.clone();
    let (status_env, _) = tokio::task::spawn_blocking(move || {
        call_ai_ipc(&s1, &AiIpcRequest::Status, Duration::from_secs(5)).unwrap()
    })
    .await
    .unwrap();
    assert!(status_env.ok);
    match status_env.payload.unwrap() {
        AiIpcPayload::Status {
            project_id,
            total_flows,
            project_disk_bytes,
            process_memory_bytes,
            ..
        } => {
            assert_eq!(project_id, opened_project.id);
            assert_eq!(total_flows, 2);
            assert!(project_disk_bytes > 0);
            assert!(process_memory_bytes > 0);
        }
        other => panic!("unexpected payload: {other:?}"),
    }

    // 2. flows_list with filter + method + status
    let s2 = sock_for_blocking.clone();
    let (list_env, _) = tokio::task::spawn_blocking(move || {
        call_ai_ipc(
            &s2,
            &AiIpcRequest::FlowsList {
                offset: 0,
                limit: 10,
                filter: "login".into(),
                method: Some("POST".into()),
                status: Some(200),
                hide_connect: false,
                host: None,
                source: None,
                state: None,
                order: None,
            },
            Duration::from_secs(5),
        )
        .unwrap()
    })
    .await
    .unwrap();
    match list_env.payload.unwrap() {
        AiIpcPayload::FlowsList { total, flows } => {
            assert_eq!(total, 1);
            assert_eq!(flows[0].id, f1_id);
        }
        other => panic!("unexpected payload: {other:?}"),
    }

    // 3. flows_get with provenance
    let s3 = sock_for_blocking.clone();
    let (get_env, _) = tokio::task::spawn_blocking(move || {
        call_ai_ipc(
            &s3,
            &AiIpcRequest::FlowsGet {
                flow_id: Some(f1_id),
                flow_number: None,
            },
            Duration::from_secs(5),
        )
        .unwrap()
    })
    .await
    .unwrap();
    match get_env.payload.unwrap() {
        AiIpcPayload::FlowsGet(detail) => {
            assert_eq!(detail.summary.id, f1_id);
            assert!(
                detail
                    .request
                    .headers
                    .iter()
                    .any(|h| h.name == "X-Rewritten")
            );
            assert_eq!(detail.provenance.request_steps.len(), 2);
        }
        other => panic!("unexpected payload: {other:?}"),
    }

    // 4. repeater_open emits Event::RepeaterOpened and reconciles cleanly with UI SaveWorkspace
    let s4 = sock_for_blocking.clone();
    let (open_env, _) = tokio::task::spawn_blocking(move || {
        call_ai_ipc(
            &s4,
            &AiIpcRequest::RepeaterOpen {
                flow_id: Some(f1_id),
                flow_number: None,
                use_original: false,
                target: None,
                raw_request: None,
                title: Some("AI Login Replay".into()),
                blank: false,
            },
            Duration::from_secs(5),
        )
        .unwrap()
    })
    .await
    .unwrap();
    let opened_tab_id = match open_env.payload.unwrap() {
        AiIpcPayload::RepeaterOpen { tab_id, title, .. } => {
            assert_eq!(title, "AI Login Replay");
            tab_id
        }
        other => panic!("unexpected payload: {other:?}"),
    };
    let emitted_tab = recv_until(&mut client.events, |ev| match ev {
        Event::RepeaterOpened { project, tab } if project == opened_project => Some(tab),
        _ => None,
    })
    .await;
    assert_eq!(emitted_tab.id, opened_tab_id);

    // Simulate UI receiving Event::RepeaterOpened and saving its workspace at old_rev + 1
    ui_workspace.revision += 1;
    ui_workspace.active = Some(emitted_tab.id);
    ui_workspace.tabs.push(emitted_tab);
    client
        .commands
        .send(Command::SaveWorkspace {
            project: opened_project,
            workspace: ui_workspace.clone(),
        })
        .await
        .unwrap();
    let acked_ws = recv_until(&mut client.events, |ev| match ev {
        Event::WorkspaceChanged { project, workspace } if project == opened_project => {
            Some(workspace)
        }
        Event::WorkspaceRejected { message, .. } => {
            panic!("UI workspace save after IPC repeater_open was rejected: {message}")
        }
        _ => None,
    })
    .await;
    assert_eq!(acked_ws.revision, ui_workspace.revision);

    // 5. slim_save via IPC
    let s5 = sock_for_blocking.clone();
    let (slim_env, _) = tokio::task::spawn_blocking(move || {
        call_ai_ipc(
            &s5,
            &AiIpcRequest::SlimSave {
                path: slim_dest,
                options: Some(SlimSaveOptions::default()),
            },
            Duration::from_secs(5),
        )
        .unwrap()
    })
    .await
    .unwrap();
    match slim_env.payload.unwrap() {
        AiIpcPayload::SlimSave { report } => {
            assert_eq!(report.removed_flows, 1);
            assert!(report.new_bytes < report.old_bytes);
        }
        other => panic!("unexpected payload: {other:?}"),
    }

    drop(client);
    runtime.shutdown().unwrap();
    assert!(
        !sock_path.exists(),
        "owned IPC socket file must be removed on shutdown"
    );
}

#[tokio::test]
async fn ipc_repeater_send_does_not_block_concurrent_status_and_workspace_patch_rejects_bad_revision()
 {
    #[cfg(unix)]
    let dir = tempfile::Builder::new()
        .prefix("mio-ipc-")
        .tempdir_in("/tmp")
        .unwrap();
    #[cfg(not(unix))]
    let dir = tempfile::Builder::new()
        .prefix("mio-ipc-")
        .tempdir()
        .unwrap();
    let sock_path = dir.path().join("mio.sock");
    let (runtime, mut client) = Runtime::start_with_ipc_socket(sock_path.clone()).unwrap();
    let _opened = recv_until(&mut client.events, |ev| match ev {
        Event::ProjectOpened { project, .. } => Some(project),
        _ => None,
    })
    .await;

    // Reject invalid workspace revision (same revision 0 instead of 1) with ok=false.
    let s_ws = sock_path.clone();
    let (ws_env, _) = tokio::task::spawn_blocking(move || {
        let bad_json = serde_json::to_string(&crate::Workspace::default()).unwrap();
        call_ai_ipc(
            &s_ws,
            &AiIpcRequest::WorkspacePatch {
                workspace_json: bad_json,
            },
            Duration::from_secs(5),
        )
        .unwrap()
    })
    .await
    .unwrap();
    assert!(!ws_env.ok, "stale workspace revision must return ok=false");

    // Slow upstream holds connection for 600ms while concurrent Status completes immediately.
    let slow_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let slow_addr = slow_listener.local_addr().unwrap();
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (mut stream, _) = slow_listener.accept().await.unwrap();
        let mut buf = [0u8; 512];
        let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;
        let _ = accepted_tx.send(());
        tokio::time::sleep(Duration::from_millis(500)).await;
        use std::io::Write as _;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(b"unpacked-ok").unwrap();
        let gz_body = encoder.finish().unwrap();
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            gz_body.len()
        );
        let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, head.as_bytes()).await;
        let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, &gz_body).await;
    });

    let s_send = sock_path.clone();
    let send_task = tokio::task::spawn_blocking(move || {
        call_ai_ipc(
            &s_send,
            &AiIpcRequest::RepeaterSend {
                tab_id: None,
                flow_id: None,
                flow_number: None,
                use_original: false,
                target: Some(format!("http://{slow_addr}")),
                raw_request: Some(format!("GET /slow HTTP/1.1\r\nHost: {slow_addr}\r\n\r\n")),
                add_headers: vec![("X-AI-Probe".into(), "1".into())],
                remove_headers: Vec::new(),
                override_body: None,
                override_body_base64: None,
                insecure: false,
            },
            Duration::from_secs(5),
        )
        .unwrap()
    });

    accepted_rx.await.unwrap();
    let s_status = sock_path.clone();
    let status_start = std::time::Instant::now();
    let (status_env, _) = tokio::task::spawn_blocking(move || {
        call_ai_ipc(&s_status, &AiIpcRequest::Status, Duration::from_secs(2)).unwrap()
    })
    .await
    .unwrap();
    assert!(status_env.ok);
    assert!(
        status_start.elapsed() < Duration::from_millis(300),
        "Status IPC must not wait behind in-flight RepeaterSend"
    );

    let (send_env, _) = send_task.await.unwrap();
    assert!(send_env.ok);
    let AiIpcPayload::FlowsGet(detail) = send_env.payload.unwrap() else {
        panic!("expected FlowsGet payload");
    };
    assert_eq!(detail.response.as_ref().unwrap().body, b"unpacked-ok");
    assert!(detail.original_request.is_some());
    assert!(detail.original_response.is_some());

    let s_get = sock_path.clone();
    let flow_id = detail.summary.id;
    let (get_env, _) = tokio::task::spawn_blocking(move || {
        call_ai_ipc(
            &s_get,
            &AiIpcRequest::FlowsGet {
                flow_id: Some(flow_id),
                flow_number: None,
            },
            Duration::from_secs(2),
        )
        .unwrap()
    })
    .await
    .unwrap();
    assert!(get_env.ok);
    let AiIpcPayload::FlowsGet(persisted) = get_env.payload.unwrap() else {
        panic!("expected persisted FlowsGet");
    };
    assert_eq!(persisted.response.as_ref().unwrap().body, b"unpacked-ok");
    assert!(
        persisted.original_request.is_some(),
        "persisted side=2 original request must be readable"
    );
    assert!(
        persisted
            .original_response
            .as_ref()
            .is_some_and(|orig| orig.body != b"unpacked-ok"),
        "persisted side=3 original compressed response must be readable"
    );
    server.await.unwrap();
    drop(client);
    runtime.shutdown().unwrap();
}

#[tokio::test]
async fn ai_ipc_repeater_send_obeys_monitor_mio_ai_and_session_rule_scope() {
    #[cfg(unix)]
    let dir = tempfile::Builder::new()
        .prefix("mio-ipc-ai-jar-")
        .tempdir_in("/tmp")
        .unwrap();
    #[cfg(not(unix))]
    let dir = tempfile::Builder::new()
        .prefix("mio-ipc-ai-jar-")
        .tempdir()
        .unwrap();
    let sock_path = dir.path().join("mio.sock");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let observed_cookies = Arc::new(std::sync::Mutex::new(Vec::<Option<String>>::new()));
    let observed_for_srv = observed_cookies.clone();
    let server = tokio::spawn(async move {
        for _ in 0..3 {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            let mut chunk = [0u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
                let n = tokio::io::AsyncReadExt::read(&mut sock, &mut chunk)
                    .await
                    .unwrap();
                if n == 0 {
                    break;
                }
                head.extend_from_slice(&chunk[..n]);
            }
            let req_text = String::from_utf8_lossy(&head);
            let cookie_line = req_text
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("cookie:"))
                .map(|l| l.trim().to_owned());
            observed_for_srv.lock().unwrap().push(cookie_line);
            let resp = "HTTP/1.1 200 OK\r\nSet-Cookie: ai_sid=42; Path=/\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
            tokio::io::AsyncWriteExt::write_all(&mut sock, resp.as_bytes())
                .await
                .unwrap();
        }
    });

    let (runtime, mut client) = Runtime::start_with_ipc_socket(sock_path.clone()).unwrap();
    let _ = recv_until(&mut client.events, |ev| match ev {
        Event::ProjectOpened { project, .. } => Some(project),
        _ => None,
    })
    .await;

    let patch_sessions = |sock: &std::path::Path, monitor_mio_ai: bool| {
        let sock = sock.to_path_buf();
        async move {
            tokio::task::spawn_blocking(move || {
                let (get_env, _) =
                    call_ai_ipc(&sock, &AiIpcRequest::WorkspaceGet, Duration::from_secs(3))
                        .unwrap();
                let AiIpcPayload::WorkspaceState { workspace: val } =
                    get_env.payload.unwrap()
                else {
                    panic!("expected WorkspaceState");
                };
                let mut ws: crate::Workspace = serde_json::from_value(val).unwrap();
                ws.revision += 1;
                ws.sessions.cookie_jar.monitor_mio_ai = monitor_mio_ai;
                if ws.sessions.rules.is_empty() {
                    ws.sessions.rules.push(crate::SessionRule {
                        id: Uuid::new_v4(),
                        name: "AI Cookie Rule".into(),
                        enabled: true,
                        tools: crate::SessionToolScope {
                            proxy: false,
                            repeater: false,
                            intruder: false,
                            scanner: false,
                            sequencer: false,
                            extensions: false,
                            mio_ai: true,
                        },
                        condition: crate::SessionRuleCondition::default(),
                        actions: vec![crate::SessionRuleAction::UseCookieJar {
                            add_new_cookies: true,
                            update_existing_cookies: true,
                        }],
                    });
                }
                let json = serde_json::to_string(&ws).unwrap();
                let (patch_env, _) = call_ai_ipc(
                    &sock,
                    &AiIpcRequest::WorkspacePatch {
                        workspace_json: json,
                    },
                    Duration::from_secs(3),
                )
                .unwrap();
                assert!(patch_env.ok, "patch failed: {:?}", patch_env.error);
            })
            .await
            .unwrap();
        }
    };

    let send_once = |sock: &std::path::Path, path: &str| {
        let sock = sock.to_path_buf();
        let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\n\r\n");
        let target = format!("http://{addr}");
        async move {
            tokio::task::spawn_blocking(move || {
                let (env, _) = call_ai_ipc(
                    &sock,
                    &AiIpcRequest::RepeaterSend {
                        tab_id: None,
                        flow_id: None,
                        flow_number: None,
                        use_original: false,
                        target: Some(target),
                        raw_request: Some(req),
                        add_headers: Vec::new(),
                        remove_headers: Vec::new(),
                        override_body: None,
                        override_body_base64: None,
                        insecure: false,
                    },
                    Duration::from_secs(5),
                )
                .unwrap();
                assert!(env.ok, "send failed: {:?}", env.error);
            })
            .await
            .unwrap();
        }
    };

    // 1. Default monitor_mio_ai = false: Set-Cookie must NOT populate Cookie Jar for next AI send.
    patch_sessions(&sock_path, false).await;
    send_once(&sock_path, "/first").await;
    // 2. Enable monitor_mio_ai = true, send again -> Cookie Jar is populated with ai_sid=42.
    patch_sessions(&sock_path, true).await;
    send_once(&sock_path, "/second").await;
    // 3. Third AI send must automatically inject Cookie: ai_sid=42 via UseCookieJar (mio_ai=true).
    send_once(&sock_path, "/third").await;

    let seen = observed_cookies.lock().unwrap().clone();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[0], None);
    assert_eq!(seen[1], None, "disabled monitor_mio_ai must not ingest ai_sid from /first");
    assert_eq!(
        seen[2].as_deref(),
        Some("Cookie: ai_sid=42"),
        "enabled monitor_mio_ai + mio_ai rule must ingest and inject ai_sid"
    );

    server.await.unwrap();
    drop(client);
    runtime.shutdown().unwrap();
}

#[tokio::test]
async fn ai_ipc_ca_presets_and_use_apply_preset_and_reject_invalid_ids() {
    let dir = tempfile::tempdir().unwrap();
    let prefs_path = dir.path().canonicalize().unwrap().join("preferences.json");
    let (events, _rx) = mpsc::channel(64);
    let mut state = State::with_preference_paths(
        events,
        CancellationToken::new(),
        crate::preferences::PreferencesStore::new(Some(prefs_path.clone())),
        Some(prefs_path),
    )
    .unwrap();

    let list_env = state.handle_ai_ipc(AiIpcRequest::CaPresets).await;
    assert!(list_env.ok, "CaPresets failed: {:?}", list_env.error);
    let Some(AiIpcPayload::CaPresets { presets }) = list_env.payload else {
        panic!("expected CaPresets payload");
    };
    assert_eq!(presets.len(), crate::BUILTIN_CA_PRESETS.len());
    assert!(presets.iter().any(|p| p.id == "emudhra-emsign-dv-g3b"));
    assert!(presets.iter().any(|p| p.id == "emudhra-aaa-dv-g3b"));

    let expected = crate::ca_preset("emudhra-aaa-dv-g3b").unwrap();
    let use_env = state
        .handle_ai_ipc(AiIpcRequest::CaPresetUse {
            preset_id: " emudhra-aaa-dv-g3b ".into(),
        })
        .await;
    assert!(use_env.ok, "CaPresetUse failed: {:?}", use_env.error);
    let Some(AiIpcPayload::CaPresetApplied {
        preset_id,
        ca_fingerprint,
        ca_subject,
    }) = use_env.payload
    else {
        panic!("expected CaPresetApplied payload");
    };
    assert_eq!(preset_id, expected.id);
    assert_eq!(ca_fingerprint, expected.fingerprint_sha256);
    assert!(!ca_subject.trim().is_empty());

    let authority = state.authority().await.unwrap();
    assert_eq!(authority.certificate_chain_der().len(), expected.chain_len);
    assert_eq!(
        authority::fingerprint_sha256(&authority.certificate_der()),
        expected.fingerprint_sha256
    );

    let blank_env = state
        .handle_ai_ipc(AiIpcRequest::CaPresetUse {
            preset_id: "   ".into(),
        })
        .await;
    assert!(!blank_env.ok);
    assert!(blank_env.error.as_deref().unwrap_or("").contains("不能为空"));

    let unknown_env = state
        .handle_ai_ipc(AiIpcRequest::CaPresetUse {
            preset_id: "missing-preset".into(),
        })
        .await;
    assert!(!unknown_env.ok);
    let err = unknown_env.error.unwrap_or_default();
    assert!(err.contains("missing-preset"));
    assert!(err.contains("emudhra-emsign-dv-g3b"));
    assert!(err.contains("emudhra-aaa-dv-g3b"));
}

#[tokio::test]
async fn ai_ipc_roadmap4_flows_numbering_filters_repeater_tabs_provenance_and_workspace_revision() {
    #[cfg(unix)]
    let dir = tempfile::Builder::new()
        .prefix("mio-ipc-r4-")
        .tempdir_in("/tmp")
        .unwrap();
    #[cfg(not(unix))]
    let dir = tempfile::Builder::new()
        .prefix("mio-ipc-r4-")
        .tempdir()
        .unwrap();
    let project_path = dir.path().join("r4.mio");
    let sock_path = dir.path().join("r4.sock");

    let mut seed = ProjectStore::create(Some(&project_path)).unwrap();
    let mut f_connect = tests::completed_flow();
    f_connect.summary.id = Uuid::new_v4();
    f_connect.summary.source = TrafficSource::Proxy;
    f_connect.summary.method = "CONNECT".into();
    f_connect.summary.url = "https://tunnel.example.invalid:443".into();
    f_connect.summary.started_unix_ms = 1000;
    f_connect.request.method = "CONNECT".into();
    f_connect.request.url = f_connect.summary.url.clone();
    seed.put(&f_connect).unwrap();

    let mut f_ok = tests::completed_flow();
    f_ok.summary.id = Uuid::new_v4();
    f_ok.summary.source = TrafficSource::Proxy;
    f_ok.summary.method = "GET".into();
    f_ok.summary.url = "https://api.example.invalid/v1/orders".into();
    f_ok.summary.status = Some(200);
    f_ok.summary.started_unix_ms = 2000;
    f_ok.request.method = "GET".into();
    f_ok.request.url = f_ok.summary.url.clone();
    seed.put(&f_ok).unwrap();

    let mut f_fail = tests::completed_flow();
    f_fail.summary.id = Uuid::new_v4();
    f_fail.summary.source = TrafficSource::Repeater;
    f_fail.summary.method = "POST".into();
    f_fail.summary.url = "https://other.example.invalid:8443/submit".into();
    f_fail.summary.status = None;
    f_fail.summary.state = FlowState::Failed("upstream timeout".into());
    f_fail.summary.started_unix_ms = 3000;
    f_fail.request.method = "POST".into();
    f_fail.request.url = f_fail.summary.url.clone();
    seed.put(&f_fail).unwrap();
    seed.save(None).unwrap();
    seed.close().unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_addr = listener.local_addr().unwrap();
    let origin = tokio::spawn(async move {
        for _ in 0..2 {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let mut buf = [0u8; 2048];
            let _ = tokio::io::AsyncReadExt::read(&mut sock, &mut buf).await;
            let body = r#"{"ok":true}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut sock, resp.as_bytes()).await;
        }
    });

    let (runtime, mut client) = Runtime::start_with_ipc_socket(sock_path.clone()).unwrap();
    let _ = recv_until(&mut client.events, |ev| match ev {
        Event::ProjectOpened { project, .. } => Some(project),
        _ => None,
    })
    .await;
    client
        .commands
        .send(Command::OpenProject {
            path: project_path.clone(),
        })
        .await
        .unwrap();
    let _ = recv_until(&mut client.events, |ev| match ev {
        Event::ProjectOpened { path: Some(p), .. } if p == project_path => Some(()),
        _ => None,
    })
    .await;

    let call = |req: AiIpcRequest| {
        let sock = sock_path.clone();
        async move {
            tokio::task::spawn_blocking(move || call_ai_ipc(&sock, &req, Duration::from_secs(5)))
                .await
                .unwrap()
                .unwrap()
                .0
        }
    };

    // 1. Hide CONNECT + asc ordering + monotonic numbers
    let list_env = call(AiIpcRequest::FlowsList {
        offset: 0,
        limit: 50,
        filter: String::new(),
        method: None,
        status: None,
        hide_connect: true,
        host: None,
        source: None,
        state: None,
        order: Some("asc".into()),
    })
    .await;
    assert!(list_env.ok);
    let AiIpcPayload::FlowsList { total, flows } = list_env.payload.unwrap() else {
        panic!("expected FlowsList");
    };
    assert_eq!(total, 2);
    assert_eq!(flows[0].id, f_ok.summary.id);
    assert_eq!(flows[1].id, f_fail.summary.id);
    let ok_num = flows[0].number.expect("flow number must be present");
    assert!(ok_num > 0);

    // 2. Host + source + abnormal state filters
    let abnormal_env = call(AiIpcRequest::FlowsList {
        offset: 0,
        limit: 50,
        filter: String::new(),
        method: None,
        status: None,
        hide_connect: false,
        host: Some("other.example.invalid:8443".into()),
        source: Some("repeater".into()),
        state: Some("abnormal".into()),
        order: None,
    })
    .await;
    let AiIpcPayload::FlowsList { total, flows } = abnormal_env.payload.unwrap() else {
        panic!("expected FlowsList");
    };
    assert_eq!(total, 1);
    assert_eq!(flows[0].id, f_fail.summary.id);

    // 3. Address flow by sequence number
    let get_by_num = call(AiIpcRequest::FlowsGet {
        flow_id: None,
        flow_number: Some(ok_num),
    })
    .await;
    assert!(get_by_num.ok);
    let AiIpcPayload::FlowsGet(detail) = get_by_num.payload.unwrap() else {
        panic!("expected FlowsGet");
    };
    assert_eq!(detail.summary.id, f_ok.summary.id);
    assert_eq!(detail.number, Some(ok_num));

    // 4. Repeater blank open, template open, list, rename, send --tab with override (CliRepeaterOverride), close
    let blank_env = call(AiIpcRequest::RepeaterOpen {
        flow_id: None,
        flow_number: None,
        use_original: false,
        target: None,
        raw_request: None,
        title: Some("Blank".into()),
        blank: true,
    })
    .await;
    assert!(blank_env.ok);
    let AiIpcPayload::RepeaterOpen { tab_id: blank_id, .. } = blank_env.payload.unwrap() else {
        panic!("expected RepeaterOpen");
    };

    let tpl_env = call(AiIpcRequest::RepeaterOpen {
        flow_id: None,
        flow_number: None,
        use_original: false,
        target: Some(format!("http://{origin_addr}/ping?a=1")),
        raw_request: None,
        title: Some("Tpl".into()),
        blank: false,
    })
    .await;
    assert!(tpl_env.ok);
    let AiIpcPayload::RepeaterOpen { tab_id: tpl_id, .. } = tpl_env.payload.unwrap() else {
        panic!("expected RepeaterOpen");
    };

    let ren_env = call(AiIpcRequest::RepeaterRename {
        tab_id: tpl_id,
        title: "Renamed Tab".into(),
    })
    .await;
    assert!(ren_env.ok);

    let send_tab_env = call(AiIpcRequest::RepeaterSend {
        tab_id: Some(tpl_id),
        flow_id: None,
        flow_number: None,
        use_original: false,
        target: None,
        raw_request: None,
        add_headers: vec![("X-Cli-Check".into(), "yes".into())],
        remove_headers: Vec::new(),
        override_body: None,
        override_body_base64: Some(
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"\x00\x01bin"),
        ),
        insecure: false,
    })
    .await;
    assert!(send_tab_env.ok, "send --tab failed: {:?}", send_tab_env.error);
    let AiIpcPayload::FlowsGet(sent_detail) = send_tab_env.payload.unwrap() else {
        panic!("expected FlowsGet");
    };
    assert_eq!(sent_detail.tab_id, Some(tpl_id));
    assert_eq!(sent_detail.request.body, b"\x00\x01bin");
    assert!(
        sent_detail
            .provenance
            .request_steps
            .iter()
            .any(|s| s.actor == ProvenanceActor::CliRepeaterOverride),
        "CLI override must use CliRepeaterOverride, got {:?}",
        sent_detail.provenance.request_steps
    );
    assert!(
        !sent_detail
            .provenance
            .request_steps
            .iter()
            .any(|s| s.actor == ProvenanceActor::ManualInterceptEdit)
    );

    let rlist = call(AiIpcRequest::RepeaterList).await;
    let AiIpcPayload::RepeaterList { repeater_tabs, .. } = rlist.payload.unwrap() else {
        panic!("expected RepeaterList");
    };
    let tpl_summary = repeater_tabs.iter().find(|t| t.tab_id == tpl_id).unwrap();
    assert_eq!(tpl_summary.title, "Renamed Tab");
    assert_eq!(tpl_summary.last_flow, Some(sent_detail.summary.id));

    let close_env = call(AiIpcRequest::RepeaterClose { tab_id: blank_id }).await;
    assert!(close_env.ok);

    // 5. WorkspacePatch rejects stale/zero revision with descriptive conflict message & succeeds on rev+1
    let ws_get = call(AiIpcRequest::WorkspaceGet).await;
    let AiIpcPayload::WorkspaceState { workspace } = ws_get.payload.unwrap() else {
        panic!("expected WorkspaceState");
    };
    let mut ws: crate::Workspace = serde_json::from_value(workspace).unwrap();
    let stale_rev = ws.revision;
    let bad_patch = call(AiIpcRequest::WorkspacePatch {
        workspace_json: serde_json::to_string(&ws).unwrap(),
    })
    .await;
    assert!(!bad_patch.ok);
    let err_text = bad_patch.error.unwrap_or_default();
    assert!(err_text.contains(&format!("revision={stale_rev}")));
    assert!(err_text.contains(&format!("递增为 {}", stale_rev + 1)));

    ws.revision += 1;
    let ok_patch = call(AiIpcRequest::WorkspacePatch {
        workspace_json: serde_json::to_string(&ws).unwrap(),
    })
    .await;
    assert!(ok_patch.ok);

    origin.abort();
    drop(client);
    runtime.shutdown().unwrap();
}

#[tokio::test]
async fn ai_ipc_headless_response_interception_captures_ordinary_json_response_in_pending() {
    #[cfg(unix)]
    let dir = tempfile::Builder::new()
        .prefix("mio-ipc-resp-")
        .tempdir_in("/tmp")
        .unwrap();
    #[cfg(not(unix))]
    let dir = tempfile::Builder::new()
        .prefix("mio-ipc-resp-")
        .tempdir()
        .unwrap();
    let sock_path = dir.path().join("resp.sock");

    let origin_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_addr = origin_listener.local_addr().unwrap();
    let origin = tokio::spawn(async move {
        if let Ok((mut sock, _)) = origin_listener.accept().await {
            let mut buf = [0u8; 2048];
            let _ = tokio::io::AsyncReadExt::read(&mut sock, &mut buf).await;
            let body = r#"{"secret":"before"}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut sock, resp.as_bytes()).await;
        }
    });

    let (runtime, mut client) = Runtime::start_with_ipc_socket(sock_path.clone()).unwrap();
    let _ = recv_until(&mut client.events, |ev| match ev {
        Event::ProjectOpened { project, .. } => Some(project),
        _ => None,
    })
    .await;

    let call = |req: AiIpcRequest| {
        let sock = sock_path.clone();
        async move {
            tokio::task::spawn_blocking(move || call_ai_ipc(&sock, &req, Duration::from_secs(5)))
                .await
                .unwrap()
                .unwrap()
                .0
        }
    };

    let start_env = call(AiIpcRequest::ProxyStart {
        bind_address: Some("127.0.0.1:0".into()),
    })
    .await;
    assert!(start_env.ok, "ProxyStart failed: {:?}", start_env.error);
    let AiIpcPayload::ProxyControl {
        proxy_address: Some(proxy_addr),
        ..
    } = start_env.payload.unwrap()
    else {
        panic!("expected ProxyControl with bound address");
    };

    let intercept_env = call(AiIpcRequest::ProxyIntercept {
        requests: Some(false),
        responses: Some(true),
    })
    .await;
    assert!(
        intercept_env.ok,
        "ProxyIntercept failed: {:?}",
        intercept_env.error
    );

    let proxy_addr_clone = proxy_addr.clone();
    let client_req_task = tokio::spawn(async move {
        let mut stream = tokio::net::TcpStream::connect(&proxy_addr_clone)
            .await
            .unwrap();
        let raw = format!(
            "GET http://{origin_addr}/api/item HTTP/1.1\r\nHost: {origin_addr}\r\nConnection: close\r\n\r\n"
        );
        tokio::io::AsyncWriteExt::write_all(&mut stream, raw.as_bytes())
            .await
            .unwrap();
        let mut resp_buf = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut resp_buf).await;
        String::from_utf8_lossy(&resp_buf).into_owned()
    });

    let mut pending_item = None;
    for _ in 0..80 {
        let env = call(AiIpcRequest::ProxyPending).await;
        if let Some(AiIpcPayload::ProxyPending { pending_items }) = env.payload
            && let Some(item) = pending_items.into_iter().find(|i| i.stage == "response")
        {
            pending_item = Some(item);
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let item = pending_item.expect("ordinary JSON response must enter proxy pending");
    assert_eq!(item.status, Some(200));

    let decide_env = call(AiIpcRequest::ProxyDecide {
        flow_id: item.flow_id,
        revision: item.revision,
        drop_flow: false,
        add_headers: vec![("X-Intercepted".into(), "yes".into())],
        remove_headers: Vec::new(),
        override_body: Some(r#"{"secret":"after"}"#.into()),
        override_status: None,
    })
    .await;
    assert!(decide_env.ok, "ProxyDecide failed: {:?}", decide_env.error);

    let received = tokio::time::timeout(Duration::from_secs(5), client_req_task)
        .await
        .unwrap()
        .unwrap();
    assert!(received.contains("X-Intercepted: yes"));
    assert!(received.contains(r#"{"secret":"after"}"#));

    let _ = origin.await;
    drop(client);
    runtime.shutdown().unwrap();
}
