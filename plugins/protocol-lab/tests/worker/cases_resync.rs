use base64::{Engine, engine::general_purpose::STANDARD};

use super::*;

#[tokio::test]
async fn open_request_returns_its_verified_snapshot_without_consuming_events() {
    let (_directory, package) = real_package();
    let (mut worker, _calls) = start(package, &[Permission::TrafficRead]).await;
    let initial = worker.initial_panel.clone();
    let request = request("http://127.0.0.1/snapshot".into());
    let opened = timeout(
        QUICK,
        worker
            .handle()
            .open_request(Uuid::new_v4(), 1, false, request.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(opened.revision > initial.revision);
    assert_eq!(editor(&opened, 30).contents, request.body);
    assert_eq!(editor(&opened, 31).contents, "基线".as_bytes());
    let published = panel(&mut worker, |p| p.revision == opened.revision).await;
    assert_eq!(published, opened);
    timeout(QUICK, worker.stop()).await.unwrap().unwrap();
}

#[tokio::test]
async fn opening_waits_for_the_workers_correlated_panel_acceptance() {
    let (_directory, package) = fixture_package("open-ack");
    let (mut worker, mut calls) = start(package, &[Permission::TrafficRead]).await;
    let handle = worker.handle();
    let opening = handle.open_request(
        Uuid::new_v4(),
        1,
        false,
        request("http://127.0.0.1/ack".into()),
    );
    tokio::pin!(opening);
    let release = tokio::select! {
        biased;
        result = &mut opening => {
            let _ = timeout(QUICK, worker.stop()).await;
            panic!("open_request returned before the worker accepted it: {result:?}");
        },
        call = next_call(&mut calls) => call,
    };
    assert!(
        matches!(&release.operation, ServiceOperation::QueryHistory { filter, .. }
        if filter == "open-received-before-ack")
    );
    release.reply.send(Ok(ServiceResult::Done)).unwrap();
    let accepted = timeout(QUICK, &mut opening).await.unwrap().unwrap();
    assert_eq!(accepted.revision, 2);
    let published = panel(&mut worker, |p| p.revision == 2).await;
    assert_eq!(accepted, published);
    timeout(QUICK, worker.stop()).await.unwrap().unwrap();
}

#[tokio::test]
async fn stale_refresh_call_is_rejected_without_stopping_or_replaying_the_worker() {
    let (_directory, package) = real_package();
    let (mut worker, mut calls) = start(package, &[Permission::TrafficRead]).await;
    next_call(&mut calls)
        .await
        .reply
        .send(Ok(ServiceResult::History {
            offset: 0,
            total: 0,
            rows: vec![],
        }))
        .unwrap();
    let initial = panel(&mut worker, |p| p.revision >= 3).await;
    let handle = worker.handle();
    let native = EditorSnapshot {
        document_id: 100,
        revision: editor(&initial, 30).revision + 1,
        contents: b"native input after refresh".to_vec(),
        selection_start: 0,
        selection_end: 0,
    };
    // Both inputs reach the supervisor before it can accept the worker's old refresh snapshot.
    handle.try_event(click(16, vec![])).unwrap();
    handle
        .try_event(UiEvent {
            node_id: 30,
            event: Some(ui_event::Event::EditorChanged(native.clone())),
            editors: vec![],
        })
        .unwrap();
    let rejected = timeout(QUICK, async {
        loop {
            match worker.events.recv().await {
                Some(PluginEvent::Panel { snapshot, .. })
                    if matches!(root(&snapshot).find(4).and_then(|n| n.kind.as_ref()),
                        Some(node::Kind::Text(text)) if text.text.contains("服务调用来自旧面板")) =>
                {
                    break snapshot;
                }
                Some(PluginEvent::Panel { .. }) => {}
                Some(PluginEvent::Error { failure, .. }) => {
                    assert_eq!(failure.code, ErrorCode::Stale as i32)
                }
                other => panic!("stale HostCall killed a healthy worker: {other:?}"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(editor(&rejected, 30).contents, native.contents);
    assert!(
        calls.try_recv().is_err(),
        "stale service was executed or automatically replayed"
    );
    handle.try_event(click(16, vec![])).unwrap();
    next_call(&mut calls)
        .await
        .reply
        .send(Ok(ServiceResult::History {
            offset: 0,
            total: 0,
            rows: vec![],
        }))
        .unwrap();
    panel(&mut worker, |p| p.revision > rejected.revision).await;
    timeout(QUICK, worker.stop()).await.unwrap().unwrap();
}

#[tokio::test]
async fn actual_worker_recovers_when_native_input_wins_an_equal_revision_transform_race() {
    let (_directory, package) = real_package();
    let (mut worker, _calls) = start(package, &[]).await;
    let handle = worker.handle();
    let initial = panel(&mut worker, |p| p.revision >= 2).await;
    let mut fresh = snapshots(&initial);
    let raw = fresh
        .iter_mut()
        .find(|editor| editor.document_id == 100)
        .unwrap();
    raw.revision += 1;
    raw.contents = STANDARD.encode("较旧的解码结果").into_bytes();
    let native = EditorSnapshot {
        document_id: 101,
        revision: editor(&initial, 31).revision + 1,
        contents: "后来的原生输入".as_bytes().to_vec(),
        selection_start: 0,
        selection_end: 0,
    };
    // No await between these events: the host records the new native edit before processing worker output.
    handle.try_event(click(10, fresh)).unwrap();
    handle
        .try_event(UiEvent {
            node_id: 31,
            event: Some(ui_event::Event::EditorChanged(native.clone())),
            editors: vec![],
        })
        .unwrap();
    let mut stale = 0;
    let restored = timeout(QUICK, async {
        loop {
            match worker.events.recv().await {
                Some(PluginEvent::Panel { snapshot, .. })
                    if editor(&snapshot, 31).revision == native.revision
                        && editor(&snapshot, 31).contents == native.contents =>
                {
                    break snapshot;
                }
                Some(PluginEvent::Panel { .. }) => {}
                Some(PluginEvent::Error { failure, .. }) => {
                    assert_eq!(failure.code, ErrorCode::Stale as i32);
                    stale += 1;
                }
                other => panic!("worker did not recover from explicit host resync: {other:?}"),
            }
        }
    })
    .await
    .unwrap();
    assert!(
        stale > 0,
        "the test must actually exercise stale transform rejection"
    );
    handle.try_event(click(11, snapshots(&restored))).unwrap();
    let encoded = panel(&mut worker, |p| {
        editor(p, 30).contents == STANDARD.encode(&native.contents).as_bytes()
    })
    .await;
    assert_eq!(editor(&encoded, 31).contents, native.contents);
    timeout(QUICK, worker.stop()).await.unwrap().unwrap();
}
