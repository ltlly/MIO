use super::tests_runtime::{request, running};
use super::*;

#[tokio::test]
async fn panel_provenance_uses_only_the_exact_validated_event() {
    let (mut running, _calls, mut updates) = running();
    let mut initial = running.panel.snapshot().clone();
    initial.root = Some(panel::Node::new(
        3,
        "layout",
        panel::node::Kind::Column(panel::Container {
            children: vec![
                initial.root.take().unwrap(),
                panel::Node::new(
                    2,
                    "transform",
                    panel::node::Kind::Button(panel::Button {
                        label: "transform".into(),
                        enabled: true,
                        commit_editors: true,
                    }),
                ),
            ],
        }),
    ));
    running.panel = PanelState::new(initial).unwrap();
    let (outgoing, mut wire) = mpsc::channel(QUEUE_CAPACITY);
    let changed = EditorSnapshot {
        document_id: 1,
        revision: 2,
        contents: "先输入".as_bytes().to_vec(),
        ..Default::default()
    };
    running
        .control(
            Control::Event {
                panel_id: protocol::PANEL_ID,
                event: UiEvent {
                    node_id: 1,
                    event: Some(panel::ui_event::Event::EditorChanged(changed.clone())),
                    editors: vec![],
                },
                expected_snapshot: None,
                event_seq: 0,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let first = wire.try_recv().unwrap().request_id;
    let clicked = EditorSnapshot {
        revision: 3,
        contents: "点击前的最新输入".as_bytes().to_vec(),
        ..changed.clone()
    };
    running
        .control(
            Control::Event {
                panel_id: protocol::PANEL_ID,
                event: UiEvent {
                    node_id: 2,
                    event: Some(panel::ui_event::Event::Click(panel::Empty {})),
                    editors: vec![clicked.clone()],
                },
                expected_snapshot: None,
                event_seq: 0,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let second = wire.try_recv().unwrap().request_id;
    for (sequence, reply, expected) in [
        (1, 0, vec![]),
        (2, first, vec![changed]),
        (3, second, vec![clicked]),
    ] {
        let mut snapshot = running.panel.snapshot().clone();
        snapshot.revision += 1;
        let mut frame = Envelope::new(
            running.identity,
            sequence,
            snapshot.revision,
            if sequence == 3 {
                envelope::Body::Patch(panel::PanelPatch {
                    base_revision: snapshot.revision - 1,
                    revision: snapshot.revision,
                    operations: vec![panel::PatchOperation {
                        node_id: 1,
                        change: Some(panel::patch_operation::Change::Replace(
                            snapshot.root.as_ref().unwrap().find(1).unwrap().clone(),
                        )),
                    }],
                })
            } else {
                envelope::Body::Snapshot(snapshot)
            },
            MAX_OPERATION,
        );
        frame.in_reply_to = reply;
        running.frame(frame, &outgoing).await.unwrap();
        let PluginEvent::Panel { editor_bases, .. } = updates.try_recv().unwrap() else {
            panic!("accepted panel must carry its native-input provenance");
        };
        assert_eq!(editor_bases, expected);
    }
    assert!(running.requests.is_empty());

    // A consumed correlation cannot authorize another panel, even at a newer revision.
    let before = running.panel.snapshot().clone();
    let mut repeated = before.clone();
    repeated.revision += 1;
    let mut frame = Envelope::new(
        running.identity,
        4,
        repeated.revision,
        envelope::Body::Snapshot(repeated),
        MAX_OPERATION,
    );
    frame.in_reply_to = first;
    running.frame(frame, &outgoing).await.unwrap();
    assert_eq!(running.panel.snapshot(), &before);
    assert!(matches!(
        updates.try_recv().unwrap(),
        PluginEvent::Error { .. }
    ));

    running
        .resync(&outgoing, 0, "fixture mismatch".into())
        .await
        .unwrap();
    let resync = wire.try_recv().unwrap();
    assert!(
        running
            .accept_panel_response(protocol::PANEL_ID, resync.request_id, Instant::now())
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn revoke_discards_pending_opens_commits_and_late_resource_results() {
    let (mut running, mut calls, _updates) = running();
    let (outgoing, mut wire) = mpsc::channel(QUEUE_CAPACITY);
    let resource = Uuid::new_v4();
    running.broker.bind_resource(resource, 1, false).unwrap();
    running.active_resource = Some((resource, 1));
    let retained = running.panel.snapshot().clone();
    let call = HostCall {
        operation: Some(host_call::Operation::UpdateDraft(protocol::DraftUpdate {
            resource_id: resource.as_bytes().to_vec(),
            expected_revision: 1,
            request: Some(request().into()),
        })),
        parent_request_id: 0,
    };
    let call_envelope = Envelope::new(
        running.identity,
        7,
        1,
        envelope::Body::HostCall(call.clone()),
        MAX_OPERATION,
    );
    running
        .host_call(&call_envelope, call, &outgoing)
        .await
        .unwrap();
    calls
        .try_recv()
        .unwrap()
        .reply
        .send(Ok(ServiceResult::Resource {
            id: resource,
            revision: 2,
        }))
        .unwrap();
    let completed = timeout(MAX_OPERATION, running.service_tasks.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (reply, mut committed) = oneshot::channel();
    running
        .control(
            Control::Commit {
                panel_id: protocol::PANEL_ID,
                op_id: Uuid::new_v4(),
                request: CommitEditor {
                    resource_id: resource.as_bytes().to_vec(),
                    message_revision: 1,
                    request: Some(request().into()),
                    editor: Some(EditorSnapshot {
                        document_id: 1,
                        revision: 1,
                        contents: b"retained".to_vec(),
                        ..Default::default()
                    }),
                },
                cancelled: CancellationToken::new(),
                expected_snapshot: None,
                reply,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let committing = wire.try_recv().unwrap();
    let (reply, mut opened) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: protocol::PANEL_ID,
                id: resource,
                revision: 2,
                read_only: false,
                request: request(),
                reply,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let opening = wire.try_recv().unwrap();
    let (reply, mut closed) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: protocol::PANEL_ID,
                id: resource,
                remove_panel: false,
                reply,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let mut closing = None;
    let mut cancelled_ids = Vec::new();
    while let Ok(frame) = wire.try_recv() {
        match frame.body {
            Some(envelope::Body::Cancel(cancel)) => cancelled_ids.push(cancel.request_id),
            Some(envelope::Body::CloseRequest(_)) => closing = Some(frame),
            other => panic!("unexpected frame during revoke: {other:?}"),
        }
    }
    assert!(cancelled_ids.contains(&committing.request_id));
    assert!(cancelled_ids.contains(&opening.request_id));
    let closing = closing.expect("revoke must emit CloseRequest");
    assert!(opened.try_recv().unwrap().is_err());
    assert!(committed.try_recv().unwrap().is_err());
    assert!(running.active_resource.is_none());
    assert!(running.opening_request.is_none());
    assert!(!running.broker.resources.contains_key(&resource));

    let mut late_panel = retained.clone();
    late_panel.revision += 1;
    let mut late = Envelope::new(
        running.identity,
        1,
        late_panel.revision,
        envelope::Body::Snapshot(late_panel),
        MAX_OPERATION,
    );
    late.in_reply_to = opening.request_id;
    running.frame(late, &outgoing).await.unwrap();
    let mut late = Envelope::new(
        running.identity,
        2,
        1,
        envelope::Body::Committed(protocol::Committed::default()),
        MAX_OPERATION,
    );
    late.in_reply_to = committing.request_id;
    running.frame(late, &outgoing).await.unwrap();
    assert_eq!(running.panel.snapshot(), &retained);
    assert!(!running.broker.resources.contains_key(&resource));

    let mut failure = Envelope::new(
        running.identity,
        3,
        1,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "close rejected")),
        MAX_OPERATION,
    );
    failure.in_reply_to = closing.request_id;
    running.frame(failure, &outgoing).await.unwrap();
    assert!(closed.try_recv().unwrap().is_err());
    assert_eq!(running.panel.snapshot(), &retained);
    assert!(!running.broker.resources.contains_key(&resource));

    let (reply, mut reopened) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: protocol::PANEL_ID,
                id: resource,
                revision: 1,
                read_only: false,
                request: request(),
                reply,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let reopening = wire.try_recv().unwrap();
    let mut next_panel = retained;
    next_panel.revision += 1;
    let mut next = Envelope::new(
        running.identity,
        4,
        next_panel.revision,
        envelope::Body::Snapshot(next_panel),
        MAX_OPERATION,
    );
    next.in_reply_to = reopening.request_id;
    running.frame(next, &outgoing).await.unwrap();
    assert!(reopened.try_recv().unwrap().is_ok());
    running.complete_service(completed, &outgoing).unwrap();
    assert_eq!(running.broker.resources[&resource].revision, 1);
    let Some(envelope::Body::HostResult(HostResult {
        outcome: Some(host_result::Outcome::Error(failure)),
    })) = wire.try_recv().unwrap().body
    else {
        panic!("cancelled service completion must not restore authority");
    };
    assert_eq!(failure.code, ErrorCode::Cancelled as i32);
}
