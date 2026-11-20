use base64::{Engine, engine::general_purpose::STANDARD};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

#[tokio::test]
async fn actual_worker_has_paged_table_drag_latest_editor_commit_and_loopback_send() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/pipe", listener.local_addr().unwrap());
    let (_directory, package) = real_package();
    let (mut worker, mut calls) = start(
        package,
        &[
            Permission::TrafficRead,
            Permission::DraftWrite,
            Permission::NetworkSend,
        ],
    )
    .await;
    let handle = worker.handle();
    let flow = Uuid::new_v4();
    let record = FlowRecord {
        summary: FlowSummary {
            id: flow,
            revision: 1,
            source: TrafficSource::Repeater,
            method: "POST".into(),
            url: url.clone(),
            status: Some(200),
            state: FlowState::Complete,
            request_bytes: 8,
            response_bytes: 2,
            started_unix_ms: 1,
            elapsed_ms: 20,
        },
        request: request(url),
        response: None,
    };
    let query = next_call(&mut calls).await;
    assert!(matches!(
        query.operation,
        ServiceOperation::QueryHistory {
            offset: 0,
            limit: 64,
            ..
        }
    ));
    query
        .reply
        .send(Ok(ServiceResult::History {
            offset: 0,
            total: 1,
            rows: vec![record.summary.clone()],
        }))
        .unwrap();
    let table = panel(&mut worker, |p| matches!(root(p).find(20).and_then(|n| n.kind.as_ref()), Some(node::Kind::VirtualTable(t)) if t.rows.len() == 1)).await;
    let Some(node::Kind::Canvas(canvas)) = root(&table).find(40).and_then(|n| n.kind.as_ref())
    else {
        panic!("canvas");
    };
    let hit = canvas.hit_regions[0].clone();
    let bounds = hit.bounds.unwrap();
    handle
        .try_event(UiEvent {
            node_id: 40,
            event: Some(ui_event::Event::Pointer(PointerEvent {
                hit_id: hit.id,
                phase: PointerPhase::Down as i32,
                x: bounds.x + 2.0,
                y: bounds.y + 2.0,
            })),
            editors: vec![],
        })
        .unwrap();
    let down = panel(&mut worker, |p| p.revision > table.revision).await;
    handle
        .try_event(UiEvent {
            node_id: 40,
            event: Some(ui_event::Event::Pointer(PointerEvent {
                hit_id: hit.id,
                phase: PointerPhase::Move as i32,
                x: 402.0,
                y: 42.0,
            })),
            editors: vec![],
        })
        .unwrap();
    let dragged = panel(&mut worker, |p| p.revision > down.revision).await;
    let Some(node::Kind::Canvas(canvas)) = root(&dragged).find(40).and_then(|n| n.kind.as_ref())
    else {
        panic!("canvas");
    };
    assert_eq!(canvas.hit_regions[0].bounds.as_ref().unwrap().x, 400.0);
    handle
        .try_event(UiEvent {
            node_id: 40,
            event: Some(ui_event::Event::Pointer(PointerEvent {
                hit_id: hit.id,
                phase: PointerPhase::Up as i32,
                x: 402.0,
                y: 42.0,
            })),
            editors: vec![],
        })
        .unwrap();
    let released = panel(&mut worker, |p| p.revision > dragged.revision).await;
    handle
        .try_event(UiEvent {
            node_id: 20,
            event: Some(ui_event::Event::TableSelection(TableSelection {
                row_ids: vec![flow.to_string()],
            })),
            editors: vec![],
        })
        .unwrap();
    let detail = next_call(&mut calls).await;
    assert!(matches!(detail.operation, ServiceOperation::ReadFlow { flow_id } if flow_id == flow));
    detail
        .reply
        .send(Ok(ServiceResult::Record(Box::new(record.clone()))))
        .unwrap();
    let loaded = panel(&mut worker, |p| {
        p.revision > released.revision && editor(p, 30).contents == record.request.body
    })
    .await;
    assert_eq!(editor(&loaded, 31).contents, "基线".as_bytes());

    let draft = Uuid::new_v4();
    let old_revision = editor(&loaded, 31).revision;
    handle
        .open_request(draft, 7, false, record.request.clone())
        .await
        .unwrap();
    let opened = panel(&mut worker, |p| editor(p, 31).revision > old_revision).await;
    let fresh = EditorSnapshot {
        document_id: 101,
        revision: editor(&opened, 31).revision + 1,
        contents: "最后输入".as_bytes().to_vec(),
        selection_start: 0,
        selection_end: 12,
    };
    let committed = timeout(
        QUICK,
        handle.commit(
            CommitEditor {
                resource_id: draft.as_bytes().to_vec(),
                message_revision: 7,
                request: Some(record.request.clone().into()),
                editor: Some(fresh.clone()),
            },
            CancellationToken::new(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        STANDARD.decode(&committed.body).unwrap(),
        "最后输入".as_bytes()
    );
    assert_eq!(
        committed
            .headers
            .iter()
            .filter(|h| h.name == "X-Duplicate")
            .count(),
        2
    );

    handle
        .open_request(draft, 8, true, record.request.clone())
        .await
        .unwrap();
    let readonly = panel(&mut worker, |p| editor(p, 31).read_only).await;
    let denied = handle
        .commit(
            CommitEditor {
                resource_id: draft.as_bytes().to_vec(),
                message_revision: 8,
                request: Some(record.request.clone().into()),
                editor: Some(EditorSnapshot {
                    document_id: 101,
                    revision: editor(&readonly, 31).revision,
                    contents: editor(&readonly, 31).contents.clone(),
                    ..Default::default()
                }),
            },
            CancellationToken::new(),
        )
        .await;
    assert!(denied.is_err());
    let readonly_revision = editor(&readonly, 31).revision;
    handle
        .open_request(draft, 9, false, record.request.clone())
        .await
        .unwrap();
    let writable = panel(&mut worker, |p| {
        !editor(p, 31).read_only && editor(p, 31).revision > readonly_revision
    })
    .await;
    let mut click_editors = snapshots(&writable);
    let latest = click_editors
        .iter_mut()
        .find(|e| e.document_id == 101)
        .unwrap();
    latest.revision += 1;
    latest.contents = "点击前未去抖".as_bytes().to_vec();
    handle.try_event(click(12, click_editors)).unwrap();
    let sending = next_call(&mut calls).await;
    let ServiceOperation::NetworkSend { request } = sending.operation else {
        panic!("real send");
    };
    assert_eq!(
        STANDARD.decode(&request.body).unwrap(),
        "点击前未去抖".as_bytes()
    );
    assert_eq!(sending.identity, worker.identity);
    let origin = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0; 1024];
        let header_end = loop {
            let count = socket.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                break index + 4;
            }
            assert!(bytes.len() < 16 * 1024);
        };
        let header = std::str::from_utf8(&bytes[..header_end]).unwrap();
        assert!(header.starts_with("POST /pipe HTTP/1.1\r\n"));
        let duplicates: Vec<_> = header
            .lines()
            .filter_map(|line| {
                line.split_once(':')
                    .filter(|(name, _)| name.eq_ignore_ascii_case("x-duplicate"))
                    .map(|(_, value)| value.trim())
            })
            .collect();
        assert_eq!(duplicates, ["first", "second"]);
        let length: usize = header
            .lines()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse().unwrap())
            })
            .unwrap();
        while bytes.len() < header_end + length {
            let count = socket.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&buffer[..count]);
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
            .await
            .unwrap();
        bytes[header_end..header_end + length].to_vec()
    });
    let response = timeout(
        QUICK,
        mio_proxy::send(request, mio_proxy::SendConfig::default(), sending.cancel),
    )
    .await
    .unwrap()
    .unwrap();
    sending
        .reply
        .send(Ok(ServiceResult::Response(response)))
        .unwrap();
    let response = panel(&mut worker, |p| root(p).find(32).is_some()).await;
    assert_eq!(editor(&response, 32).contents, b"OK");
    let response_revision = editor(&response, 32).revision;
    handle
        .open_request(Uuid::new_v4(), 1, false, record.request.clone())
        .await
        .unwrap();
    let next_request = panel(&mut worker, |p| p.revision > response.revision).await;
    assert!(editor(&next_request, 32).contents.is_empty());
    assert!(editor(&next_request, 32).revision > response_revision);
    assert!(editor(&next_request, 32).read_only);
    let received = timeout(QUICK, origin).await.unwrap().unwrap();
    assert_eq!(
        STANDARD.decode(received).unwrap(),
        "点击前未去抖".as_bytes()
    );
    timeout(QUICK, worker.stop()).await.unwrap().unwrap();
}

#[tokio::test]
async fn malformed_frame_and_forged_identity_never_execute_host_services() {
    let (_directory, bad) = fixture_package("bad-frame");
    let (services, mut calls) = mpsc::channel(16);
    let trust = NativeTrust::confirmed(&bad, [Permission::TrafficRead]).unwrap();
    assert!(
        timeout(
            QUICK,
            PluginSupervisor::start(bad, project(), trust, services)
        )
        .await
        .unwrap()
        .is_err()
    );
    assert!(calls.try_recv().is_err());

    let (_directory, forged) = fixture_package("forged");
    let (mut worker, mut calls) = start(forged, &[Permission::TrafficRead]).await;
    timeout(QUICK, async {
        loop {
            if let Some(PluginEvent::Stopped { reason, .. }) = worker.events.recv().await {
                assert!(reason.contains("身份") || reason.contains("代际"));
                break;
            }
        }
    })
    .await
    .unwrap();
    assert!(calls.try_recv().is_err());
    let first = worker.stop().await.unwrap_err();
    let again = worker.stop().await;
    assert!(
        again.is_err(),
        "a failed stop must not become success on a second call"
    );
    assert_eq!(format!("{first:#}"), format!("{:#}", again.unwrap_err()));
}

#[tokio::test]
async fn correctly_identified_worker_without_a_grant_never_executes_host_services() {
    let (_directory, package) = fixture_package("denied");
    let (mut worker, mut calls) = start(package, &[]).await;
    timeout(QUICK, async {
        loop {
            match worker.events.recv().await {
                Some(PluginEvent::Error { failure, .. }) => {
                    assert_eq!(failure.code, ErrorCode::Denied as i32);
                    break;
                }
                Some(PluginEvent::Panel { .. }) => {}
                other => panic!("missing permission denial: {other:?}"),
            }
        }
    })
    .await
    .unwrap();
    assert!(calls.try_recv().is_err());
    timeout(QUICK, worker.stop()).await.unwrap().unwrap();
}

#[tokio::test]
async fn a_changed_manifest_requires_a_new_package_and_trust_confirmation() {
    for field in ["permissions", "entry"] {
        let (directory, package) = real_package();
        let trust = NativeTrust::confirmed(&package, [Permission::TrafficRead]).unwrap();
        let manifest_path = directory.path().join("manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest[field] = if field == "permissions" {
            serde_json::json!(["draft.write", "repeater.open", "network.send"])
        } else {
            serde_json::json!("bin/replaced-entry")
        };
        fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let (services, mut calls) = mpsc::channel(16);
        match timeout(
            QUICK,
            PluginSupervisor::start(package, project(), trust, services),
        )
        .await
        .unwrap()
        {
            Ok(mut worker) => {
                let _ = timeout(QUICK, worker.stop()).await;
                panic!("a changed manifest must not launch using old consent");
            }
            Err(error) => assert!(error.to_string().contains("manifest")),
        }
        assert!(calls.try_recv().is_err());
    }

    let (directory, package) = real_package();
    let trust = NativeTrust::confirmed(&package, [Permission::TrafficRead]).unwrap();
    let manifest_path = directory.path().join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["permissions"] = serde_json::json!(["traffic.read", "draft.write"]);
    fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let replacement = VerifiedPackage::read(directory.path()).unwrap();
    let (services, mut calls) = mpsc::channel(16);
    match timeout(
        QUICK,
        PluginSupervisor::start(replacement, project(), trust, services),
    )
    .await
    .unwrap()
    {
        Ok(mut worker) => {
            let _ = timeout(QUICK, worker.stop()).await;
            panic!("rereading a changed manifest must not reuse an older trust confirmation");
        }
        Err(error) => assert!(error.to_string().contains("信任确认")),
    }
    assert!(calls.try_recv().is_err());
}
