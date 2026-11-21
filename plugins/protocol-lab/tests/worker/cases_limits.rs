use std::future::Future;

use super::*;

#[tokio::test]
async fn close_request_keeps_native_contents_and_requires_an_explicit_reopen() {
    let (_directory, package) = real_package();
    let (mut worker, mut calls) =
        start(package, &[Permission::TrafficRead, Permission::DraftWrite]).await;
    next_call(&mut calls)
        .await
        .reply
        .send(Ok(ServiceResult::History {
            offset: 0,
            total: 0,
            rows: vec![],
        }))
        .unwrap();
    panel(&mut worker, |p| p.revision >= 3).await;
    let handle = worker.handle();
    let resource = Uuid::new_v4();
    let original = request("http://127.0.0.1/closed-context".into());
    handle
        .open_request(resource, 1, false, original.clone())
        .await
        .unwrap();
    let opened = panel(&mut worker, |p| editor(p, 30).contents == original.body).await;
    let mut retained = snapshots(&opened);
    let latest = retained.iter_mut().find(|e| e.document_id == 100).unwrap();
    latest.revision += 1;
    latest.contents = b"cmV0YWluZWQ=".to_vec();
    handle
        .try_event(UiEvent {
            node_id: 30,
            event: Some(ui_event::Event::EditorChanged(latest.clone())),
            editors: vec![],
        })
        .unwrap();
    let edited = panel(&mut worker, |p| editor(p, 30).contents == b"cmV0YWluZWQ=").await;
    timeout(QUICK, handle.revoke_resource(resource))
        .await
        .unwrap()
        .unwrap();
    let closed = panel(&mut worker, |p| p.revision > edited.revision).await;
    assert_eq!(snapshots(&closed), snapshots(&edited));
    for id in [12, 13, 15] {
        assert!(
            matches!(root(&closed).find(id).and_then(|n| n.kind.as_ref()), Some(node::Kind::Button(button)) if !button.enabled)
        );
    }
    let error = handle
        .commit(
            CommitEditor {
                resource_id: resource.as_bytes().to_vec(),
                message_revision: 1,
                request: Some(original.clone().into()),
                editor: Some(latest.clone()),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("上下文已切换"));
    timeout(QUICK, handle.revoke_resource(resource))
        .await
        .unwrap()
        .unwrap();
    handle.try_event(click(10, snapshots(&closed))).unwrap();
    let local = panel(&mut worker, |p| editor(p, 31).contents == b"retained").await;
    handle
        .open_request(resource, 2, false, original.clone())
        .await
        .unwrap();
    let reopened = panel(&mut worker, |p| p.revision > local.revision).await;
    let committed = handle
        .commit(
            CommitEditor {
                resource_id: resource.as_bytes().to_vec(),
                message_revision: 2,
                request: Some(original.clone().into()),
                editor: Some(snapshots(&reopened).remove(0)),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(committed, original);
    assert!(calls.try_recv().is_err());
    timeout(QUICK, worker.stop()).await.unwrap().unwrap();
}

#[tokio::test]
async fn oversized_open_and_service_results_leave_the_actual_worker_usable() {
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
    next_call(&mut calls)
        .await
        .reply
        .send(Ok(ServiceResult::History {
            offset: 0,
            total: 0,
            rows: vec![],
        }))
        .unwrap();
    panel(&mut worker, |p| p.revision >= 3).await;
    let handle = worker.handle();
    let identity = worker.identity;
    let resource = Uuid::new_v4();
    let original = request("http://127.0.0.1/payload-limit".into());
    handle
        .open_request(resource, 1, false, original.clone())
        .await
        .unwrap();
    let mut current = panel(&mut worker, |p| editor(p, 30).contents == original.body).await;
    let mut oversized = original.clone();
    oversized.body = vec![0; mio_plugin_api::protocol::MAX_INLINE_BODY + 1];
    let failure = timeout(
        QUICK,
        handle.open_request(Uuid::new_v4(), 1, false, oversized),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(failure.to_string().contains("内联限额"));
    for size in [
        mio_plugin_api::protocol::MAX_INLINE_BODY + 1,
        mio_plugin_api::protocol::MAX_FRAME_BYTES + 1,
    ] {
        handle.try_event(click(12, snapshots(&current))).unwrap();
        let sending = next_call(&mut calls).await;
        assert!(matches!(
            sending.operation,
            ServiceOperation::NetworkSend { .. }
        ));
        sending
            .reply
            .send(Ok(ServiceResult::Response(mio_plugin_api::HttpResponse {
                status: 200,
                version: HttpVersion::Http11,
                reason_phrase: None,
                headers: vec![],
                body: vec![0; size],
                trailers: vec![],
            })))
            .unwrap();
        let revision = current.revision;
        current = panel(&mut worker, |p| p.revision > revision && matches!(root(p).find(4).and_then(|n| n.kind.as_ref()), Some(node::Kind::Text(text)) if text.text.contains("限额"))).await;
        assert!(
            root(&current).find(32).is_none(),
            "no truncated response may be presented"
        );
        assert_eq!(editor(&current, 30).contents, original.body);
    }
    let committed = handle
        .commit(
            CommitEditor {
                resource_id: resource.as_bytes().to_vec(),
                message_revision: 1,
                request: Some(original.clone().into()),
                editor: Some(snapshots(&current).remove(0)),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(committed, original);
    assert_eq!(worker.identity, identity);
    assert!(calls.try_recv().is_err());
    timeout(QUICK, worker.stop()).await.unwrap().unwrap();
}

#[tokio::test]
async fn host_pushed_request_is_denied_without_traffic_read_and_worker_stays_usable() {
    let (_directory, package) = real_package();
    let (mut worker, mut calls) = start(package, &[Permission::DraftWrite]).await;
    let handle = worker.handle();
    let initial = panel(&mut worker, |p| p.revision >= 2).await;
    let resource = Uuid::new_v4();
    let opened = timeout(
        QUICK,
        handle.open_request(
            resource,
            1,
            false,
            request("http://127.0.0.1/private-request".into()),
        ),
    )
    .await
    .unwrap();
    if opened.is_ok() {
        let _ = timeout(QUICK, worker.stop()).await;
        panic!("denied traffic.read must reject host-pushed request contents");
    }
    assert!(opened.unwrap_err().to_string().contains("traffic.read"));
    handle.try_event(click(10, snapshots(&initial))).unwrap();
    let unchanged = panel(&mut worker, |p| p.revision > initial.revision).await;
    assert!(editor(&unchanged, 30).contents.is_empty());
    assert!(editor(&unchanged, 31).contents.is_empty());
    let denied_commit = handle
        .commit(
            CommitEditor {
                resource_id: resource.as_bytes().to_vec(),
                message_revision: 1,
                editor: Some(snapshots(&unchanged).remove(0)),
                request: Some(request("http://127.0.0.1/private-request".into()).into()),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(
        denied_commit.to_string().contains("上下文已切换"),
        "denied open established a writable context"
    );
    assert!(calls.try_recv().is_err());
    timeout(QUICK, worker.stop()).await.unwrap().unwrap();
}

#[tokio::test]
async fn worker_open_error_preserves_the_previous_draft_binding() {
    let (_directory, package) = real_package();
    let (mut worker, mut calls) =
        start(package, &[Permission::TrafficRead, Permission::DraftWrite]).await;
    next_call(&mut calls)
        .await
        .reply
        .send(Ok(ServiceResult::History {
            offset: 0,
            total: 0,
            rows: vec![],
        }))
        .unwrap();
    panel(&mut worker, |p| p.revision >= 3).await;
    let handle = worker.handle();
    let original = request("http://127.0.0.1/original".into());
    let original_id = Uuid::new_v4();
    timeout(
        QUICK,
        handle.open_request(original_id, 1, false, original.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    let original_panel = panel(&mut worker, |p| editor(p, 30).contents == original.body).await;
    let rejected_id = Uuid::new_v4();
    let mut too_large = original.clone();
    too_large.body = vec![b'A'; mio_plugin_api::panel::MAX_EDITOR_BYTES + 1];
    let failure = timeout(QUICK, handle.open_request(rejected_id, 1, false, too_large))
        .await
        .unwrap()
        .unwrap_err();
    assert!(failure.to_string().contains("256 KiB"));
    timeout(QUICK, async {
        loop {
            match worker.events.recv().await.expect("worker events") {
                PluginEvent::Panel { snapshot, .. }
                    if snapshot.revision > original_panel.revision =>
                {
                    break;
                }
                PluginEvent::Error { .. } => continue,
                PluginEvent::Stopped { reason, .. } => panic!("worker stopped: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("error status panel update");
    let snapshot = snapshots(&original_panel).remove(0);
    let rejected = handle
        .commit(
            CommitEditor {
                resource_id: rejected_id.as_bytes().to_vec(),
                message_revision: 1,
                editor: Some(snapshot.clone()),
                request: Some(original.clone().into()),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(rejected.to_string().contains("上下文已切换"));
    let committed = handle
        .commit(
            CommitEditor {
                resource_id: original_id.as_bytes().to_vec(),
                message_revision: 1,
                editor: Some(snapshot),
                request: Some(original.clone().into()),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(committed, original);
    assert!(calls.try_recv().is_err());
    timeout(QUICK, worker.stop()).await.unwrap().unwrap();
}

#[tokio::test]
async fn a_new_open_invalidates_an_already_pending_editor_commit() {
    let (_directory, package) = real_package();
    let (mut worker, mut calls) =
        start(package, &[Permission::TrafficRead, Permission::DraftWrite]).await;
    next_call(&mut calls)
        .await
        .reply
        .send(Ok(ServiceResult::History {
            offset: 0,
            total: 0,
            rows: vec![],
        }))
        .unwrap();
    panel(&mut worker, |p| p.revision >= 3).await;
    let handle = worker.handle();
    let original = request("http://127.0.0.1/original".into());
    let resource = Uuid::new_v4();
    handle
        .open_request(resource, 1, false, original.clone())
        .await
        .unwrap();
    let current = panel(&mut worker, |p| editor(p, 30).contents == original.body).await;
    let committing = handle.commit(
        CommitEditor {
            resource_id: resource.as_bytes().to_vec(),
            message_revision: 1,
            editor: Some(snapshots(&current).remove(0)),
            request: Some(original.into()),
        },
        CancellationToken::new(),
    );
    tokio::pin!(committing);
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(committing.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    let opening = handle.open_request(
        Uuid::new_v4(),
        1,
        false,
        request("http://127.0.0.1/replacement".into()),
    );
    tokio::pin!(opening);
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(opening.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    let committed = timeout(QUICK, committing).await.unwrap();
    timeout(QUICK, opening).await.unwrap().unwrap();
    timeout(QUICK, worker.stop()).await.unwrap().unwrap();
    assert!(
        committed.is_err(),
        "an old commit succeeded while a replacement context was opening"
    );
    assert!(committed.unwrap_err().to_string().contains("切换"));
}
