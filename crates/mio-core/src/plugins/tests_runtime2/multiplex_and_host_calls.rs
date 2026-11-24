use super::*;

#[tokio::test]
async fn open_repeater_capacity_is_reserved_before_dispatching_side_effects() {
    let (mut running, mut calls, _updates) = running();
    let (outgoing, mut wire) = mpsc::channel(QUEUE_CAPACITY);
    for _ in 0..63 {
        running
            .broker
            .bind_resource(Uuid::new_v4(), 1, false)
            .unwrap();
    }
    for id in [1, 2] {
        let call = HostCall {
            operation: Some(host_call::Operation::OpenRepeater(request().into())),
            parent_request_id: 0,
        };
        let envelope = Envelope::new(
            running.identity,
            id,
            1,
            envelope::Body::HostCall(call.clone()),
            MAX_OPERATION,
        );
        running.host_call(&envelope, call, &outgoing).await.unwrap();
    }
    let first = calls.try_recv().unwrap();
    assert!(
        calls.try_recv().is_err(),
        "the 65th resource must be rejected before a tab is created"
    );
    let denied = wire.try_recv().unwrap();
    assert_eq!(denied.in_reply_to, 2);
    assert!(matches!(
        denied.body,
        Some(envelope::Body::HostResult(HostResult {
            outcome: Some(host_result::Outcome::Error(_))
        }))
    ));
    first
        .reply
        .send(Err(Failure::new(ErrorCode::Cancelled, "test cancelled")))
        .unwrap();
    let completed = timeout(MAX_OPERATION, running.service_tasks.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    running.complete_service(completed, &outgoing).unwrap();
    let call = HostCall {
        operation: Some(host_call::Operation::OpenRepeater(request().into())),
        parent_request_id: 0,
    };
    let envelope = Envelope::new(
        running.identity,
        3,
        1,
        envelope::Body::HostCall(call.clone()),
        MAX_OPERATION,
    );
    running.host_call(&envelope, call, &outgoing).await.unwrap();
    assert_eq!(
        calls.try_recv().unwrap().request_id,
        3,
        "failed opens must release their reservation"
    );
}

#[tokio::test]
async fn a_closed_new_tab_cannot_be_bound_by_a_late_open_repeater_result() {
    let (mut running, mut calls, _updates) = running();
    let (outgoing, mut wire) = mpsc::channel(QUEUE_CAPACITY);
    let resource = Uuid::new_v4();
    let call = HostCall {
        operation: Some(host_call::Operation::OpenRepeater(request().into())),
        parent_request_id: 0,
    };
    let envelope = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::HostCall(call.clone()),
        MAX_OPERATION,
    );
    running.host_call(&envelope, call, &outgoing).await.unwrap();
    calls
        .try_recv()
        .unwrap()
        .reply
        .send(Ok(ServiceResult::Resource {
            id: resource,
            revision: 1,
        }))
        .unwrap();
    let completed = timeout(MAX_OPERATION, running.service_tasks.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (reply, _closed) = oneshot::channel();
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
    assert!(matches!(
        wire.try_recv().unwrap().body,
        Some(envelope::Body::CloseRequest(_))
    ));
    running.complete_service(completed, &outgoing).unwrap();
    assert!(
        !running.broker.resources.contains_key(&resource),
        "closing a just-created tab must win over the earlier service reply"
    );
    assert!(
        running.panel_revoked_resources.is_empty(),
        "finished creates must release close barriers"
    );
    let result = wire.try_recv().unwrap();
    assert_eq!(result.in_reply_to, 1);
    assert!(matches!(
        result.body,
        Some(envelope::Body::HostResult(HostResult {
            outcome: Some(host_result::Outcome::Error(_))
        }))
    ));
}

#[tokio::test]
async fn oversized_open_is_a_correlated_error_without_a_wire_write() {
    let (mut running, _calls, _updates) = running();
    let (outgoing, mut wire) = mpsc::channel(QUEUE_CAPACITY);
    let mut large = request();
    large.body = vec![0; protocol::MAX_INLINE_BODY + 1];
    let (reply, mut opened) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: protocol::PANEL_ID,
                id: Uuid::new_v4(),
                revision: 1,
                read_only: false,
                request: large,
                reply,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(
        matches!(opened.try_recv(), Ok(Err(_))),
        "an oversized open must fail its own reply, not kill the pipe writer"
    );
    assert!(wire.try_recv().is_err());
    assert!(running.main_slot().opening_request.is_none());
    assert!(running.broker.resources.is_empty());
}

#[tokio::test]
async fn oversized_host_results_fail_only_the_correlated_call() {
    let (mut running, _calls, _updates) = running();
    let (outgoing, mut wire) = mpsc::channel(QUEUE_CAPACITY);
    for size in [protocol::MAX_INLINE_BODY + 1, protocol::MAX_FRAME_BYTES + 1] {
        running
            .complete_service(
                ServiceCompletion {
                    request_id: 7,
                    panel_id: protocol::PANEL_ID,
                    resource_binding: None,
                    result: Ok(ServiceResult::Response(HttpResponse {
                        status: 200,
                        version: mio_plugin_api::HttpVersion::Http11,
                        reason_phrase: None,
                        headers: vec![],
                        body: vec![0; size],
                        trailers: vec![],
                    })),
                },
                &outgoing,
            )
            .unwrap();
        let result = wire.try_recv().unwrap();
        assert_eq!(result.in_reply_to, 7);
        assert!(
            matches!(
                result.body,
                Some(envelope::Body::HostResult(HostResult {
                    outcome: Some(host_result::Outcome::Error(_))
                }))
            ),
            "oversized results need a per-call error, not oversized pipe data"
        );
        result.validate().unwrap();
    }
}

#[tokio::test]
async fn multiplexed_panels_route_open_commit_and_revoke_independently() {
    let (mut running, _calls, mut updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(QUEUE_CAPACITY);

    let res2 = Uuid::new_v4();
    let res3 = Uuid::new_v4();
    let (reply2, mut opened2) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: 2,
                id: res2,
                revision: 1,
                read_only: false,
                request: request(),
                reply: reply2,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let open2_wire = wire.try_recv().unwrap();
    assert_eq!(open2_wire.panel_id, 2);

    let (reply3, mut opened3) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: 3,
                id: res3,
                revision: 1,
                read_only: false,
                request: request(),
                reply: reply3,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let open3_wire = wire.try_recv().unwrap();
    assert_eq!(open3_wire.panel_id, 3);

    for (seq, panel_id, reply_id, body_text) in [
        (1u64, 2u64, open2_wire.request_id, b"p2".to_vec()),
        (2u64, 3u64, open3_wire.request_id, b"p3".to_vec()),
    ] {
        let mut frame = Envelope::new(
            running.identity,
            seq,
            1,
            envelope::Body::Snapshot(PanelSnapshot {
                revision: 1,
                title: format!("panel-{panel_id}"),
                root: Some(panel::Node::new(
                    1,
                    "ed",
                    panel::node::Kind::Editor(panel::Editor {
                        document_id: 1,
                        revision: 1,
                        contents: body_text,
                        format: panel::EditorFormat::Utf8 as i32,
                        read_only: false,
                    }),
                )),
            }),
            MAX_OPERATION,
        )
        .with_panel_id(panel_id);
        frame.in_reply_to = reply_id;
        running.frame(frame, &outgoing).await.unwrap();
    }

    assert_eq!(opened2.try_recv().unwrap().unwrap().title, "panel-2");
    assert_eq!(opened3.try_recv().unwrap().unwrap().title, "panel-3");
    assert!(matches!(
        updates.try_recv().unwrap(),
        PluginEvent::Panel { panel_id: 2, .. }
    ));
    assert!(matches!(
        updates.try_recv().unwrap(),
        PluginEvent::Panel { panel_id: 3, .. }
    ));

    // Commit on panel 2 succeeds independently of panel 3.
    let (commit_reply, mut committed2) = oneshot::channel();
    running
        .control(
            Control::Commit {
                panel_id: 2,
                op_id: Uuid::new_v4(),
                request: CommitEditor {
                    resource_id: res2.as_bytes().to_vec(),
                    message_revision: 1,
                    request: Some(request().into()),
                    editor: Some(EditorSnapshot {
                        document_id: 1,
                        revision: 1,
                        contents: b"p2".to_vec(),
                        ..Default::default()
                    }),
                },
                cancelled: CancellationToken::new(),
                reply: commit_reply,
                            expected_snapshot: None,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let commit2_wire = wire.try_recv().unwrap();
    assert_eq!(commit2_wire.panel_id, 2);
    let mut commit_res = Envelope::new(
        running.identity,
        3,
        1,
        envelope::Body::Committed(protocol::Committed {
            resource_id: res2.as_bytes().to_vec(),
            message_revision: 1,
            document_id: 1,
            document_revision: 1,
            request: Some(request().into()),
        }),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    commit_res.in_reply_to = commit2_wire.request_id;
    running.frame(commit_res, &outgoing).await.unwrap();
    assert!(committed2.try_recv().unwrap().is_ok());

    // Revoke panel 2 with remove_panel=true leaves panel 3 intact.
    let (rev_reply, mut revoked2) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: 2,
                id: res2,
                remove_panel: true,
                reply: rev_reply,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let close2_wire = wire.try_recv().unwrap();
    assert_eq!(close2_wire.panel_id, 2);
    let mut ack2 = Envelope::new(
        running.identity,
        4,
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    ack2.in_reply_to = close2_wire.request_id;
    running.frame(ack2, &outgoing).await.unwrap();
    revoked2.try_recv().unwrap().unwrap();
    assert!(!running.panels.contains_key(&2));
    assert!(running.panels.contains_key(&3));
    assert!(running.broker.has_panel_resource(3, res3));
}

#[tokio::test]
async fn cross_panel_reply_mismatch_rejected_and_duplicate_revoke_is_idempotent() {
    let (mut running, _calls, mut updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(QUEUE_CAPACITY);

    let res2 = Uuid::new_v4();
    let res3 = Uuid::new_v4();
    let (reply2, mut opened2) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: 2,
                id: res2,
                revision: 1,
                read_only: false,
                request: request(),
                reply: reply2,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let open2_wire = wire.try_recv().unwrap();

    let (reply3, mut opened3) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: 3,
                id: res3,
                revision: 1,
                read_only: false,
                request: request(),
                reply: reply3,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let open3_wire = wire.try_recv().unwrap();

    // Inject cross-panel Snapshot pretending panel 2 answered panel 3's request_id.
    let mut mismatched = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(PanelSnapshot {
            revision: 1,
            title: "hijack".into(),
            root: Some(panel::Node::new(
                1,
                "hijack",
                panel::node::Kind::Text(panel::Text {
                    text: "hijack".into(),
                    tone: 0,
                }),
            )),
        }),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    mismatched.in_reply_to = open3_wire.request_id;
    let mismatch_err = running.frame(mismatched, &outgoing).await.unwrap_err();
    assert!(mismatch_err.to_string().contains("panel_id"));
    // Panel 3's pending open must NOT have been consumed or overwritten by panel 2.
    assert!(running.requests.contains_key(&open3_wire.request_id));
    assert!(opened3.try_recv().is_err());

    // Now answer panel 2 & panel 3 properly.
    for (seq, pid, rid) in [(2u64, 2u64, open2_wire.request_id), (3u64, 3u64, open3_wire.request_id)] {
        let mut frame = Envelope::new(
            running.identity,
            seq,
            1,
            envelope::Body::Snapshot(PanelSnapshot {
                revision: 1,
                title: format!("panel-{pid}"),
                root: Some(panel::Node::new(
                    1,
                    "panel",
                    panel::node::Kind::Editor(panel::Editor {
                        document_id: 1,
                        revision: 1,
                        contents: b"ok".to_vec(),
                        format: panel::EditorFormat::Utf8 as i32,
                        read_only: false,
                    }),
                )),
            }),
            MAX_OPERATION,
        )
        .with_panel_id(pid);
        frame.in_reply_to = rid;
        running.frame(frame, &outgoing).await.unwrap();
    }
    assert!(opened2.try_recv().unwrap().is_ok());
    assert!(opened3.try_recv().unwrap().is_ok());
    while updates.try_recv().is_ok() {}

    // Revoke unknown panel (99) before any Open: succeeds immediately without writing CloseRequest.
    let (rev99_tx, mut rev99_rx) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: 99,
                id: Uuid::new_v4(),
                remove_panel: true,
                reply: rev99_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    rev99_rx.try_recv().unwrap().unwrap();
    assert!(wire.try_recv().is_err());

    // Revoke panel 2 twice concurrently: first emits CloseRequest and marks closing, second is no-op Ok(()).
    let (rev2a_tx, mut rev2a_rx) = oneshot::channel();
    let (rev2b_tx, mut rev2b_rx) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: 2,
                id: res2,
                remove_panel: true,
                reply: rev2a_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let close2_wire = wire.try_recv().unwrap();
    assert_eq!(close2_wire.panel_id, 2);
    running
        .control(
            Control::Revoke {
                panel_id: 2,
                id: res2,
                remove_panel: true,
                reply: rev2b_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    // Both concurrent revokes wait until the single CloseRequest's Ack settles, then both get Ok(()).
    assert!(rev2a_rx.try_recv().is_err());
    assert!(rev2b_rx.try_recv().is_err());
    assert!(wire.try_recv().is_err(), "duplicate close must not emit a second CloseRequest");
    let mut ack2 = Envelope::new(
        running.identity,
        4,
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    ack2.in_reply_to = close2_wire.request_id;
    running.frame(ack2, &outgoing).await.unwrap();
    rev2a_rx.try_recv().unwrap().unwrap();
    rev2b_rx.try_recv().unwrap().unwrap();
    assert!(!running.panels.contains_key(&2));

    // Cancelling panel 3's commit via CancelPanelOp clears Pending and sends Cancel on panel 3.
    let (commit3_tx, mut commit3_rx) = oneshot::channel();
    let cancel_op3 = Uuid::new_v4();
    running
        .control(
            Control::Commit {
                panel_id: 3,
                op_id: cancel_op3,
                request: CommitEditor {
                    resource_id: res3.as_bytes().to_vec(),
                    message_revision: 1,
                    request: Some(request().into()),
                    editor: Some(EditorSnapshot {
                        document_id: 1,
                        revision: 1,
                        contents: b"ok".to_vec(),
                        ..Default::default()
                    }),
                },
                cancelled: CancellationToken::new(),
                reply: commit3_tx,
                            expected_snapshot: None,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let commit3_wire = wire.try_recv().unwrap();
    running
        .control(
            Control::CancelPanelOp {
                panel_id: 3,
                op_id: cancel_op3,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(commit3_rx.try_recv().unwrap().is_err());
    let cancel_wire = wire.try_recv().unwrap();
    assert_eq!(cancel_wire.panel_id, 3);
    assert!(matches!(
        cancel_wire.body,
        Some(envelope::Body::Cancel(ref c)) if c.request_id == commit3_wire.request_id
    ));
}

#[tokio::test]
async fn multiplexed_over_limit_open_fails_single_request_without_killing_worker() {
    let (mut running, _calls, _updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(QUEUE_CAPACITY * 2);

    for idx in 0..MAX_MULTIPLEXED_PANELS {
        let panel_id = protocol::PANEL_ID + 1 + idx as u64;
        let (reply, _rx) = oneshot::channel();
        running
            .control(
                Control::Open {
                    panel_id,
                    id: Uuid::new_v4(),
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let _ = wire.try_recv().unwrap();
    }
    let (over_tx, mut over_rx) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: protocol::PANEL_ID + 1 + MAX_MULTIPLEXED_PANELS as u64,
                id: Uuid::new_v4(),
                revision: 1,
                read_only: false,
                request: request(),
                reply: over_tx,
            },
            &outgoing,
        )
        .await
        .expect("over-limit open on child panel must not fail worker loop");
    assert!(over_rx.try_recv().unwrap().is_err());
}

#[tokio::test]
async fn child_panel_timeout_isolates_without_killing_shared_worker() {
    let (mut running, _calls, mut updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(QUEUE_CAPACITY);

    let res2 = Uuid::new_v4();
    let res3 = Uuid::new_v4();
    let (reply2, mut opened2) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: 2,
                id: res2,
                revision: 1,
                read_only: false,
                request: request(),
                reply: reply2,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let open2_wire = wire.try_recv().unwrap();

    let (reply3, mut opened3) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: 3,
                id: res3,
                revision: 1,
                read_only: false,
                request: request(),
                reply: reply3,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let open3_wire = wire.try_recv().unwrap();

    let mut snap3 = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(PanelSnapshot {
            revision: 1,
            title: "panel-3".into(),
            root: Some(panel::Node::new(
                1,
                "panel-3",
                panel::node::Kind::Text(panel::Text {
                    text: "panel-3".into(),
                    tone: 0,
                }),
            )),
        }),
        MAX_OPERATION,
    )
    .with_panel_id(3);
    snap3.in_reply_to = open3_wire.request_id;
    running.frame(snap3, &outgoing).await.unwrap();
    assert!(opened3.try_recv().unwrap().is_ok());
    while updates.try_recv().is_ok() {}

    // Expire panel 2's pending open: shared worker stays alive and panel 3 stays intact.
    running.requests.get_mut(&open2_wire.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();

    let cancel2_wire = wire.try_recv().unwrap();
    assert_eq!(cancel2_wire.panel_id, 2);
    assert!(matches!(
        cancel2_wire.body,
        Some(envelope::Body::Cancel(ref c)) if c.request_id == open2_wire.request_id
    ));
    assert!(opened2.try_recv().unwrap().is_err());
    assert!(matches!(
        updates.try_recv().unwrap(),
        PluginEvent::Error { panel_id: 2, .. }
    ));
    assert!(running.panels.contains_key(&3));

    // Expire a Close on child panel 2: panel 2 slot is cleaned up (no zombie closing slot).
    let (rev2_tx, mut rev2_rx) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: 2,
                id: res2,
                remove_panel: true,
                reply: rev2_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let close2_wire = wire.try_recv().unwrap();
    running.requests.get_mut(&close2_wire.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
    assert!(
        wire.try_recv().is_err(),
        "timed-out CloseRequest(destroy_panel=true) must NOT emit trailing Cancel"
    );
    assert!(rev2_rx.try_recv().unwrap().is_err());
    assert!(
        matches!(
            running.panels.get(&2),
            Some(slot) if slot.closing && slot.destroy_failed && slot.state.is_none()
        ),
        "timed-out child destroy must keep a failed tombstone slot"
    );
    assert!(running.panels.contains_key(&3));

    // Expiring a main-panel operation still terminates the worker loop.
    let (main_tx, _main_rx) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: protocol::PANEL_ID,
                id: Uuid::new_v4(),
                revision: 1,
                read_only: false,
                request: request(),
                reply: main_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let main_wire = wire.try_recv().unwrap();
    running.requests.get_mut(&main_wire.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    assert!(running.handle_timeouts(Instant::now(), &outgoing).await.is_err());
}

#[tokio::test]
async fn child_panel_expired_frame_queue_headroom_and_cancel_services() {
    let (mut running, mut calls, mut updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(QUEUE_CAPACITY * 2);

    let res2 = Uuid::new_v4();
    let res3 = Uuid::new_v4();
    let (reply2, mut opened2) = oneshot::channel();
    let (reply3, mut opened3) = oneshot::channel();
    for (pid, res, rep) in [(2u64, res2, reply2), (3u64, res3, reply3)] {
        running
            .control(
                Control::Open {
                    panel_id: pid,
                    id: res,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: rep,
                },
                &outgoing,
            )
            .await
            .unwrap();
    }
    let open2_wire = wire.try_recv().unwrap();
    let open3_wire = wire.try_recv().unwrap();
    for (seq, pid, rid) in [(1u64, 2u64, open2_wire.request_id), (2u64, 3u64, open3_wire.request_id)] {
        let mut snap = Envelope::new(
            running.identity,
            seq,
            1,
            envelope::Body::Snapshot(PanelSnapshot {
                revision: 1,
                title: format!("p-{pid}"),
                root: Some(panel::Node::new(
                    1,
                    "ed",
                    panel::node::Kind::Editor(panel::Editor {
                        document_id: 1,
                        revision: 1,
                        contents: b"ok".to_vec(),
                        format: panel::EditorFormat::Utf8 as i32,
                        read_only: false,
                    }),
                )),
            }),
            MAX_OPERATION,
        )
        .with_panel_id(pid);
        snap.in_reply_to = rid;
        running.frame(snap, &outgoing).await.unwrap();
    }
    assert!(opened2.try_recv().unwrap().is_ok());
    assert!(opened3.try_recv().unwrap().is_ok());
    while updates.try_recv().is_ok() {}

    // 1. Expired frame on child panel 2 does NOT kill shared worker and emits Deadline diagnostic.
    let mut expired_child_ev = Envelope::new(
        running.identity,
        3,
        1,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "late")),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    expired_child_ev.deadline_unix_ms = 1;
    running.frame(expired_child_ev, &outgoing).await.unwrap();
    assert!(matches!(
        updates.try_recv().unwrap(),
        PluginEvent::Error {
            panel_id: 2,
            failure: Failure { code, .. },
            ..
        } if code == ErrorCode::Deadline as i32
    ));
    assert!(running.panels.contains_key(&3));

    // 2. Per-op cancellation isolation, synchronous service cancellation inheritance & late cancelled HostCall blocking.
    let commit_cancel = CancellationToken::new();
    let commit_op = Uuid::new_v4();
    let (commit_tx, mut commit_rx) = oneshot::channel();
    running
        .control(
            Control::Commit {
                panel_id: 2,
                op_id: commit_op,
                request: CommitEditor {
                    resource_id: res2.as_bytes().to_vec(),
                    message_revision: 1,
                    request: Some(request().into()),
                    editor: Some(EditorSnapshot {
                        document_id: 1,
                        revision: 1,
                        contents: b"ok".to_vec(),
                        ..Default::default()
                    }),
                },
                cancelled: commit_cancel.clone(),
                reply: commit_tx,
                            expected_snapshot: None,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let commit_wire = wire.try_recv().unwrap();
    let hc = HostCall {
        operation: Some(host_call::Operation::UpdateDraft(protocol::DraftUpdate {
            resource_id: res2.as_bytes().to_vec(),
            expected_revision: 1,
            request: Some(request().into()),
        })),
        parent_request_id: commit_wire.request_id,
    };
    let hc_env = Envelope::new(
        running.identity,
        4,
        1,
        envelope::Body::HostCall(hc.clone()),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    running.host_call(&hc_env, hc.clone(), &outgoing).await.unwrap();
    let svc_req = calls.try_recv().unwrap();
    assert!(!svc_req.cancel.is_cancelled());

    // Cancelling commit_cancel immediately cancels inherited UpdateDraft service token synchronously!
    commit_cancel.cancel();
    assert!(
        svc_req.cancel.is_cancelled(),
        "derived service token must inherit commit cancellation immediately"
    );

    // CancelPanelOp removes commit_wire and records its request_id in cancelled_requests.
    running
        .control(
            Control::CancelPanelOp {
                panel_id: 2,
                op_id: commit_op,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(commit_rx.try_recv().unwrap().is_err());
    while wire.try_recv().is_ok() {}

    // A NEW commit on the SAME panel 2 is NOT cancelled by a delayed CancelPanelOp for the old op_id!
    let commit2_op = Uuid::new_v4();
    let (commit2_tx, mut commit2_rx) = oneshot::channel();
    running
        .control(
            Control::Commit {
                panel_id: 2,
                op_id: commit2_op,
                request: CommitEditor {
                    resource_id: res2.as_bytes().to_vec(),
                    message_revision: 1,
                    request: Some(request().into()),
                    editor: Some(EditorSnapshot {
                        document_id: 1,
                        revision: 1,
                        contents: b"ok".to_vec(),
                        ..Default::default()
                    }),
                },
                cancelled: CancellationToken::new(),
                reply: commit2_tx,
                            expected_snapshot: None,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let _commit2_wire = wire.try_recv().unwrap();
    running
        .control(
            Control::CancelPanelOp {
                panel_id: 2,
                op_id: commit_op,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(
        commit2_rx.try_recv().is_err(),
        "stale CancelPanelOp for old op_id must not cancel new commit on the same panel"
    );

    // Late HostCall referencing the cancelled parent_request_id is STILL blocked after Pending removal!
    let hc_late = Envelope::new(
        running.identity,
        5,
        1,
        envelope::Body::HostCall(hc.clone()),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    running.host_call(&hc_late, hc, &outgoing).await.unwrap();
    let denied = wire.try_recv().unwrap();
    assert_eq!(denied.in_reply_to, 5);
    assert!(matches!(
        denied.body,
        Some(envelope::Body::HostResult(HostResult {
            outcome: Some(host_result::Outcome::Error(Failure { code, .. })),
        })) if code == ErrorCode::Cancelled as i32
    ));

    // 3. Soft limit (CHILD_QUEUE_SOFT_LIMIT = 96) rejects extra child opens and child resync non-fatally while reserving
    // headroom for main panel events and child CloseRequest.
    let mut held_replies = Vec::new();
    for i in 0..CHILD_QUEUE_SOFT_LIMIT {
        let (tx, rx) = oneshot::channel();
        held_replies.push(rx);
        running
            .control(
                Control::Commit {
                    panel_id: 3,
                    op_id: Uuid::new_v4(),
                    request: CommitEditor {
                        resource_id: res3.as_bytes().to_vec(),
                        message_revision: 1,
                        request: Some(request().into()),
                        editor: Some(EditorSnapshot {
                            document_id: 1,
                            revision: 1,
                            contents: b"ok".to_vec(),
                            ..Default::default()
                        }),
                    },
                    cancelled: CancellationToken::new(),
                    reply: tx,
                                    expected_snapshot: None,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let _ = i;
        while wire.try_recv().is_ok() {}
    }
    while updates.try_recv().is_ok() {}
    running
        .resync_panel(2, &outgoing, 0, "busy resync".into())
        .await
        .expect("child resync at soft limit must degrade safely without killing shared worker");
    assert!(matches!(
        updates.try_recv().unwrap(),
        PluginEvent::Error { panel_id: 2, .. }
    ));
    let (busy_tx, mut busy_rx) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: 4,
                id: Uuid::new_v4(),
                revision: 1,
                read_only: false,
                request: request(),
                reply: busy_tx,
            },
            &outgoing,
        )
        .await
        .expect("child open hitting soft limit must not fail worker loop");
    assert!(busy_rx.try_recv().unwrap().is_err());

    let (main_tx, _main_rx) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: protocol::PANEL_ID,
                id: Uuid::new_v4(),
                revision: 1,
                read_only: false,
                request: request(),
                reply: main_tx,
            },
            &outgoing,
        )
        .await
        .expect("main panel open must retain queue headroom above child soft limit");
    let main_open_wire = wire.try_recv().unwrap();
    assert_eq!(main_open_wire.panel_id, protocol::PANEL_ID);

    let (rev3_tx, _rev3_rx) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: 3,
                id: res3,
                remove_panel: true,
                reply: rev3_tx,
            },
            &outgoing,
        )
        .await
        .expect("child CloseRequest must retain queue headroom above child soft limit");
    let mut saw_close3 = false;
    while let Ok(frame) = wire.try_recv() {
        assert_eq!(frame.panel_id, 3);
        match frame.body {
            Some(envelope::Body::CloseRequest(_)) => {
                saw_close3 = true;
                break;
            }
            Some(envelope::Body::Cancel(_)) => {}
            other => panic!("unexpected frame during panel 3 revoke: {other:?}"),
        }
    }
    assert!(saw_close3, "expected CloseRequest on panel 3");
}

#[tokio::test]
async fn pending_destroy_queues_under_full_requests_and_flushes_on_recovery() {
    let (mut running, _calls, mut updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
    let res2 = Uuid::new_v4();
    let (open_tx, open_rx) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: 2,
                id: res2,
                revision: 1,
                read_only: false,
                request: request(),
                reply: open_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let open_wire = wire.try_recv().unwrap();
    let mut open_reply = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(PanelSnapshot {
            revision: 1,
            title: "p2".into(),
            root: Some(panel::Node::new(
                1,
                "ed",
                panel::node::Kind::Editor(panel::Editor {
                    document_id: 1,
                    revision: 1,
                    contents: b"init".to_vec(),
                    format: panel::EditorFormat::Utf8 as i32,
                    read_only: false,
                }),
            )),
        }),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    open_reply.in_reply_to = open_wire.request_id;
    running.frame(open_reply, &outgoing).await.unwrap();
    assert!(open_rx.await.is_ok());

    // Fill all QUEUE_CAPACITY request slots on main panel (e.g. 128 pending UI events).
    for _ in 0..QUEUE_CAPACITY {
        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    event: panel::UiEvent {
                        node_id: 1,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Focus(true)),
                    },
                    expected_snapshot: None,
                    event_seq: 0,
                },
                &outgoing,
            )
            .await
            .unwrap();
    }
    while wire.try_recv().is_ok() {}
    assert_eq!(running.requests.len(), QUEUE_CAPACITY);

    // Revoking panel 2 with remove_panel=true while requests == QUEUE_CAPACITY MUST queue pending_destroy instead of dropping it!
    let (rev_tx, mut rev_rx) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: 2,
                id: res2,
                remove_panel: true,
                reply: rev_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert_eq!(running.pending_revokes.len(), 1);
    assert!(matches!(running.panels.get(&2), Some(slot) if slot.closing && !slot.destroy_failed));
    assert!(matches!(
        rev_rx.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert!(wire.try_recv().is_err());

    // A second concurrent revoke coalesces onto the queued pending_revoke.
    let (rev_dup_tx, mut rev_dup_rx) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: 2,
                id: res2,
                remove_panel: true,
                reply: rev_dup_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert_eq!(running.pending_revokes.len(), 1);
    assert!(matches!(
        rev_dup_rx.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));

    // Completing one main request frees capacity and automatically flushes CloseRequest(destroy_panel=true).
    let one_main_req = *running.requests.keys().next().unwrap();
    let mut main_ack =
        Envelope::new(running.identity, 2, 1, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION);
    main_ack.in_reply_to = one_main_req;
    running.frame(main_ack, &outgoing).await.unwrap();
    assert!(running.pending_revokes.is_empty());

    let destroy_wire = wire.try_recv().unwrap();
    assert_eq!(destroy_wire.panel_id, 2);
    assert!(matches!(
        destroy_wire.body,
        Some(envelope::Body::CloseRequest(ref c)) if c.destroy_panel
    ));
    assert!(matches!(
        rev_rx.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));

    let mut close_ack =
        Envelope::new(running.identity, 3, 1, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION)
            .with_panel_id(2);
    close_ack.in_reply_to = destroy_wire.request_id;
    running.frame(close_ack, &outgoing).await.unwrap();
    assert!(rev_rx.try_recv().unwrap().is_ok());
    assert!(rev_dup_rx.try_recv().unwrap().is_ok());
    assert!(!running.panels.contains_key(&2));
    running.requests.clear();

    // Also verify rejected CloseRequest(destroy_panel=true) keeps tombstone & rejects follow-up revokes.
    let res3 = Uuid::new_v4();
    let (open3_tx, open3_rx) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: 3,
                id: res3,
                revision: 1,
                read_only: false,
                request: request(),
                reply: open3_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let open3_wire = wire.try_recv().unwrap();
    let mut open3_reply = Envelope::new(
        running.identity,
        4,
        1,
        envelope::Body::Snapshot(PanelSnapshot {
            revision: 1,
            title: "p3".into(),
            root: Some(panel::Node::new(
                1,
                "ed",
                panel::node::Kind::Editor(panel::Editor {
                    document_id: 1,
                    revision: 1,
                    contents: b"p3".to_vec(),
                    format: panel::EditorFormat::Utf8 as i32,
                    read_only: false,
                }),
            )),
        }),
        MAX_OPERATION,
    )
    .with_panel_id(3);
    open3_reply.in_reply_to = open3_wire.request_id;
    running.frame(open3_reply, &outgoing).await.unwrap();
    assert!(open3_rx.await.is_ok());
    while updates.try_recv().is_ok() {}

    let (rev3_tx, mut rev3_rx) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: 3,
                id: res3,
                remove_panel: true,
                reply: rev3_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let destroy3_wire = wire.try_recv().unwrap();
    let mut close3_err = Envelope::new(
        running.identity,
        5,
        1,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "refused close")),
        MAX_OPERATION,
    )
    .with_panel_id(3);
    close3_err.in_reply_to = destroy3_wire.request_id;
    running.frame(close3_err, &outgoing).await.unwrap();
    assert!(rev3_rx.try_recv().unwrap().is_err());
    assert!(matches!(
        updates.try_recv(),
        Ok(PluginEvent::Error { panel_id: 3, .. })
    ));
    assert!(matches!(
        running.panels.get(&3),
        Some(slot) if slot.closing && slot.destroy_failed && slot.state.is_none()
    ));

    let (rev3_again_tx, mut rev3_again_rx) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: 3,
                id: res3,
                remove_panel: true,
                reply: rev3_again_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(rev3_again_rx.try_recv().unwrap().is_err());
}

#[tokio::test]
async fn host_call_validates_parent_clamps_deadline_and_expired_reply_cancels_derived_service() {
    let (mut running, mut calls, mut updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(QUEUE_CAPACITY);
    let res2 = Uuid::new_v4();
    let res3 = Uuid::new_v4();

    for (panel_id, seq, res) in [(2u64, 1u64, res2), (3u64, 2u64, res3)] {
        let (tx, rx) = oneshot::channel();
        running
            .control(
                Control::Open {
                    panel_id,
                    id: res,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let ow = wire.try_recv().unwrap();
        let mut snap = Envelope::new(
            running.identity,
            seq,
            1,
            envelope::Body::Snapshot(PanelSnapshot {
                revision: 1,
                title: format!("p-{panel_id}"),
                root: Some(panel::Node::new(
                    1,
                    "ed",
                    panel::node::Kind::Editor(panel::Editor {
                        document_id: 1,
                        revision: 1,
                        contents: b"init".to_vec(),
                        format: panel::EditorFormat::Utf8 as i32,
                        read_only: false,
                    }),
                )),
            }),
            MAX_OPERATION,
        )
        .with_panel_id(panel_id);
        snap.in_reply_to = ow.request_id;
        running.frame(snap, &outgoing).await.unwrap();
        assert!(rx.await.is_ok());
    }

    // 1. Unknown non-zero parent_request_id MUST be rejected with Stale (never reaches `calls`).
    let hc_unknown = HostCall {
        operation: Some(host_call::Operation::UpdateDraft(protocol::DraftUpdate {
            resource_id: res2.as_bytes().to_vec(),
            expected_revision: 1,
            request: Some(request().into()),
        })),
        parent_request_id: 9999,
    };
    let env_unknown = Envelope::new(
        running.identity,
        10,
        1,
        envelope::Body::HostCall(hc_unknown.clone()),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    running.host_call(&env_unknown, hc_unknown, &outgoing).await.unwrap();
    assert!(calls.try_recv().is_err());
    let err_unknown = wire.try_recv().unwrap();
    assert!(matches!(
        err_unknown.body,
        Some(envelope::Body::HostResult(HostResult {
            outcome: Some(host_result::Outcome::Error(Failure { code, .. })),
        })) if code == ErrorCode::Stale as i32
    ));

    // 2. Active Commit on panel 2 cannot authorize HostCall on panel 3 (cross-panel parent_request_id rejected).
    let (c2_tx, mut c2_rx) = oneshot::channel();
    running
        .control(
            Control::Commit {
                panel_id: 2,
                op_id: Uuid::new_v4(),
                request: CommitEditor {
                    resource_id: res2.as_bytes().to_vec(),
                    message_revision: 1,
                    request: Some(request().into()),
                    editor: Some(EditorSnapshot {
                        document_id: 1,
                        revision: 1,
                        contents: b"init".to_vec(),
                        ..Default::default()
                    }),
                },
                cancelled: CancellationToken::new(),
                reply: c2_tx,
                            expected_snapshot: None,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let c2_wire = wire.try_recv().unwrap();
    let parent_deadline = Instant::now() + Duration::from_millis(120);
    running.requests.get_mut(&c2_wire.request_id).unwrap().deadline = parent_deadline;

    let hc_cross = HostCall {
        operation: Some(host_call::Operation::UpdateDraft(protocol::DraftUpdate {
            resource_id: res3.as_bytes().to_vec(),
            expected_revision: 1,
            request: Some(request().into()),
        })),
        parent_request_id: c2_wire.request_id,
    };
    let env_cross = Envelope::new(
        running.identity,
        11,
        1,
        envelope::Body::HostCall(hc_cross.clone()),
        MAX_OPERATION,
    )
    .with_panel_id(3);
    running.host_call(&env_cross, hc_cross, &outgoing).await.unwrap();
    assert!(calls.try_recv().is_err());
    let err_cross = wire.try_recv().unwrap();
    assert!(matches!(
        err_cross.body,
        Some(envelope::Body::HostResult(HostResult {
            outcome: Some(host_result::Outcome::Error(Failure { code, .. })),
        })) if code == ErrorCode::Stale as i32
    ));

    // 3. Valid same-panel parent clamps ServiceRequest.deadline to parent.deadline (NOT full 5s).
    let hc_ok = HostCall {
        operation: Some(host_call::Operation::UpdateDraft(protocol::DraftUpdate {
            resource_id: res2.as_bytes().to_vec(),
            expected_revision: 1,
            request: Some(request().into()),
        })),
        parent_request_id: c2_wire.request_id,
    };
    let env_ok = Envelope::new(
        running.identity,
        12,
        1,
        envelope::Body::HostCall(hc_ok.clone()),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    running.host_call(&env_ok, hc_ok.clone(), &outgoing).await.unwrap();
    let svc_req = calls.try_recv().unwrap();
    assert!(
        svc_req.deadline <= parent_deadline,
        "derived service deadline must be clamped by parent deadline"
    );

    // 4. An expired reply arriving at io::frame MUST centrally record cancelled parent + cancel in-flight derived service.
    while updates.try_recv().is_ok() {}
    let mut expired_reply = Envelope::new(
        running.identity,
        3,
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    expired_reply.deadline_unix_ms = 1;
    expired_reply.in_reply_to = c2_wire.request_id;
    running.frame(expired_reply, &outgoing).await.unwrap();
    assert!(c2_rx.try_recv().unwrap().is_err());
    assert!(
        svc_req.cancel.is_cancelled(),
        "expired reply rejecting parent MUST cancel in-flight derived service"
    );

    // Subsequent late HostCall referencing that parent_request_id is rejected as Cancelled.
    let env_after = Envelope::new(
        running.identity,
        13,
        1,
        envelope::Body::HostCall(hc_ok.clone()),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    running.host_call(&env_after, hc_ok, &outgoing).await.unwrap();
    assert!(calls.try_recv().is_err());
    let mut saw_cancelled = false;
    while let Ok(msg) = wire.try_recv() {
        if msg.in_reply_to == 13 {
            saw_cancelled = matches!(
                msg.body,
                Some(envelope::Body::HostResult(HostResult {
                    outcome: Some(host_result::Outcome::Error(Failure { code, .. })),
                })) if code == ErrorCode::Cancelled as i32
            );
        }
    }
    assert!(saw_cancelled);

    // 5. Parent past deadline blocks new derived HostCall IMMEDIATELY even before handle_timeouts runs.
    let (c3_tx, mut c3_rx) = oneshot::channel();
    running
        .control(
            Control::Commit {
                panel_id: 3,
                op_id: Uuid::new_v4(),
                request: CommitEditor {
                    resource_id: res3.as_bytes().to_vec(),
                    message_revision: 1,
                    request: Some(request().into()),
                    editor: Some(EditorSnapshot {
                        document_id: 1,
                        revision: 1,
                        contents: b"init".to_vec(),
                        ..Default::default()
                    }),
                },
                cancelled: CancellationToken::new(),
                reply: c3_tx,
                            expected_snapshot: None,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let c3_wire = wire.try_recv().unwrap();
    running.requests.get_mut(&c3_wire.request_id).unwrap().deadline =
        Instant::now() - Duration::from_millis(5);
    let hc_expired_parent = HostCall {
        operation: Some(host_call::Operation::UpdateDraft(protocol::DraftUpdate {
            resource_id: res3.as_bytes().to_vec(),
            expected_revision: 1,
            request: Some(request().into()),
        })),
        parent_request_id: c3_wire.request_id,
    };
    let env_expired_parent = Envelope::new(
        running.identity,
        14,
        1,
        envelope::Body::HostCall(hc_expired_parent.clone()),
        MAX_OPERATION,
    )
    .with_panel_id(3);
    running
        .host_call(&env_expired_parent, hc_expired_parent, &outgoing)
        .await
        .unwrap();
    assert!(calls.try_recv().is_err());
    assert!(c3_rx.try_recv().unwrap().is_err());
    let mut saw_deadline = false;
    while let Ok(msg) = wire.try_recv() {
        if msg.in_reply_to == 14 {
            saw_deadline = matches!(
                msg.body,
                Some(envelope::Body::HostResult(HostResult {
                    outcome: Some(host_result::Outcome::Error(Failure { code, .. })),
                })) if code == ErrorCode::Deadline as i32
            );
        }
    }
    assert!(saw_deadline);
}

#[tokio::test]
async fn outbound_queue_absorbs_backpressure_fifo_and_detects_closed_pipe() {
    let (mut running, _calls, _updates) = running();
    let (outgoing, mut wire) = mpsc::channel(1);

    // Fill the 1-slot outgoing channel.
    let first_id = running
        .send_on_panel(
            &outgoing,
            protocol::PANEL_ID,
            envelope::Body::Ack(panel::Empty {}),
            1,
        )
        .unwrap();
    // Next complete_service and send_on_panel hit TrySendError::Full -> buffered in outbound_queue without error!
    running
        .complete_service(
            ServiceCompletion {
                request_id: 20,
                panel_id: protocol::PANEL_ID,
                resource_binding: None,
                result: Ok(ServiceResult::Done),
            },
            &outgoing,
        )
        .expect("backpressure during complete_service must buffer rather than kill worker");
    let third_id = running
        .send_on_panel(
            &outgoing,
            protocol::PANEL_ID,
            envelope::Body::Cancel(protocol::Cancel { request_id: 21 }),
            0,
        )
        .unwrap();
    assert_eq!(running.outbound_queue.len(), 2);

    // Drain channel and flush outbound_queue -> strict FIFO order preserved.
    let mut received = Vec::new();
    while let Ok(env) = wire.try_recv() {
        received.push(env);
        running.flush_outbound_nonblocking(&outgoing).unwrap();
    }
    assert!(running.outbound_queue.is_empty());
    assert_eq!(received.len(), 3);
    assert_eq!(received[0].request_id, first_id);
    assert_eq!(received[1].in_reply_to, 20);
    assert_eq!(received[2].request_id, third_id);
    assert!(received[0].request_id < received[1].request_id && received[1].request_id < received[2].request_id);

    // Dropping receiver closes pipe -> next send MUST report pipe closure Err.
    drop(wire);
    let err = running
        .send_on_panel(
            &outgoing,
            protocol::PANEL_ID,
            envelope::Body::Ack(panel::Empty {}),
            2,
        )
        .unwrap_err();
    assert!(err.to_string().contains("关闭"));
}

#[tokio::test]
async fn overload_fails_hook_non_fatally_and_defers_extra_resource_revoke() {
    let (mut running, _calls, _updates) = running();
    running.supports_panel_multiplexing = true;
    running.broker.grants.insert(Permission::TrafficRead);
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

    let res2 = Uuid::new_v4();
    let (open_tx, open_rx) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: 2,
                id: res2,
                revision: 1,
                read_only: false,
                request: request(),
                reply: open_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let open_wire = wire.try_recv().unwrap();
    let mut snap = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(PanelSnapshot {
            revision: 1,
            title: "p2".into(),
            root: Some(panel::Node::new(
                1,
                "ed",
                panel::node::Kind::Editor(panel::Editor {
                    document_id: 1,
                    revision: 1,
                    contents: b"init".to_vec(),
                    format: panel::EditorFormat::Utf8 as i32,
                    read_only: false,
                }),
            )),
        }),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    snap.in_reply_to = open_wire.request_id;
    running.frame(snap, &outgoing).await.unwrap();
    assert!(open_rx.await.is_ok());

    // Fill all QUEUE_CAPACITY slots on main panel (e.g. mixed workload hitting 128).
    for _ in 0..QUEUE_CAPACITY {
        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    event: panel::UiEvent {
                        node_id: 1,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Focus(true)),
                    },
                    expected_snapshot: None,
                    event_seq: 0,
                },
                &outgoing,
            )
            .await
            .unwrap();
    }
    while wire.try_recv().is_ok() {}
    assert_eq!(running.requests.len(), QUEUE_CAPACITY);

    // 1. Hook arriving at capacity limit MUST fail only the hook reply, returning Ok(()) to keep worker alive!
    let (hook_tx, mut hook_rx) = oneshot::channel();
    running
        .control(
            Control::Hook {
                permission: Permission::TrafficRead,
                body: envelope::Body::HttpHookCall(protocol::HttpHookCall {
                    source: protocol::SourceDto::Repeater as i32,
                    flow_id: Uuid::new_v4().as_bytes().to_vec(),
                    plugin_id: String::new(),
                    response_stage: false,
                    response_head_only: false,
                    request: Some(request().into()),
                    response: None,
                }),
                cancelled: CancellationToken::new(),
                reply: hook_tx,
            },
            &outgoing,
        )
        .await
        .expect("main hook overload must not terminate shared worker");
    assert!(hook_rx.try_recv().unwrap().is_err());

    // 2. Non-panel extra-resource Revoke (remove_panel=false) at saturation defers into pending_revokes.
    let extra_res = Uuid::new_v4();
    let (rev_tx, mut rev_rx) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: 2,
                id: extra_res,
                remove_panel: false,
                reply: rev_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert_eq!(running.pending_revokes.len(), 1);
    assert!(matches!(
        rev_rx.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert!(wire.try_recv().is_err());

    // Free one slot -> deferred resource CloseRequest(destroy_panel=false) flushes automatically & resolves Ok(()).
    let one_main_req = *running.requests.keys().next().unwrap();
    let mut main_ack =
        Envelope::new(running.identity, 2, 1, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION);
    main_ack.in_reply_to = one_main_req;
    running.frame(main_ack, &outgoing).await.unwrap();
    assert!(running.pending_revokes.is_empty());

    let close_wire = wire.try_recv().unwrap();
    assert_eq!(close_wire.panel_id, 2);
    assert!(matches!(
        close_wire.body,
        Some(envelope::Body::CloseRequest(ref c)) if !c.destroy_panel
    ));
    let mut close_ack =
        Envelope::new(running.identity, 3, 1, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION)
            .with_panel_id(2);
    close_ack.in_reply_to = close_wire.request_id;
    running.frame(close_ack, &outgoing).await.unwrap();
    assert!(rev_rx.try_recv().unwrap().is_ok());
}

#[tokio::test]
async fn late_verified_destroy_ack_reclaims_tombstone_and_emits_panel_closed() {
    let (mut running, _calls, mut updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

    for (panel_id, seq) in [(2u64, 1u64), (3u64, 2u64)] {
        let res = Uuid::new_v4();
        let (tx, rx) = oneshot::channel();
        running
            .control(
                Control::Open {
                    panel_id,
                    id: res,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let ow = wire.try_recv().unwrap();
        let mut snap = Envelope::new(
            running.identity,
            seq,
            1,
            envelope::Body::Snapshot(PanelSnapshot {
                revision: 1,
                title: format!("p-{panel_id}"),
                root: Some(panel::Node::new(
                    1,
                    "ed",
                    panel::node::Kind::Editor(panel::Editor {
                        document_id: 1,
                        revision: 1,
                        contents: b"init".to_vec(),
                        format: panel::EditorFormat::Utf8 as i32,
                        read_only: false,
                    }),
                )),
            }),
            MAX_OPERATION,
        )
        .with_panel_id(panel_id);
        snap.in_reply_to = ow.request_id;
        running.frame(snap, &outgoing).await.unwrap();
        assert!(rx.await.is_ok());
    }
    while updates.try_recv().is_ok() {}

    // 1. Panel 2 destroy times out -> tombstone keeps destroy_request_id.
    let (rev2_tx, mut rev2_rx) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: 2,
                id: Uuid::new_v4(),
                remove_panel: true,
                reply: rev2_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let close2_wire = wire.try_recv().unwrap();
    running.requests.get_mut(&close2_wire.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
    assert!(
        wire.try_recv().is_err(),
        "timed-out CloseRequest(destroy_panel=true) must NOT emit trailing Cancel"
    );
    assert!(rev2_rx.try_recv().unwrap().is_err());
    assert_eq!(
        running.panels.get(&2).unwrap().destroy_request_id,
        Some(close2_wire.request_id)
    );
    while updates.try_recv().is_ok() {}

    // Late verified Ack (even expired) reclaims panel 2 tombstone & emits PluginEvent::PanelClosed!
    let mut late_ack2 =
        Envelope::new(running.identity, 3, 1, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION)
            .with_panel_id(2);
    late_ack2.in_reply_to = close2_wire.request_id;
    late_ack2.deadline_unix_ms = 1;
    running.frame(late_ack2, &outgoing).await.unwrap();
    assert!(!running.panels.contains_key(&2));
    assert!(matches!(
        updates.try_recv(),
        Ok(PluginEvent::PanelClosed { panel_id: 2, .. })
    ));

    // 2. Panel 3 destroy explicitly rejected by worker Error -> clears destroy_request_id so wrong Ack cannot reclaim it!
    let (rev3_tx, mut rev3_rx) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: 3,
                id: Uuid::new_v4(),
                remove_panel: true,
                reply: rev3_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let close3_wire = wire.try_recv().unwrap();
    let mut err3 = Envelope::new(
        running.identity,
        4,
        1,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "destroy refused")),
        MAX_OPERATION,
    )
    .with_panel_id(3);
    err3.in_reply_to = close3_wire.request_id;
    running.frame(err3, &outgoing).await.unwrap();
    assert!(rev3_rx.try_recv().unwrap().is_err());
    assert_eq!(running.panels.get(&3).unwrap().destroy_request_id, None);
    while updates.try_recv().is_ok() {}

    let mut bogus_ack3 =
        Envelope::new(running.identity, 5, 1, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION)
            .with_panel_id(3);
    bogus_ack3.in_reply_to = close3_wire.request_id;
    running.frame(bogus_ack3, &outgoing).await.unwrap();
    assert!(running.panels.contains_key(&3));
}

#[tokio::test]
async fn on_cancel_cleanup_host_call_allows_safe_ops_clamps_deadline_and_blocks_mutations() {
    let (mut running, mut calls, _updates) = running();
    running.supports_panel_multiplexing = true;
    running.broker.grants.insert(Permission::RepeaterOpen);
    running.broker.grants.insert(Permission::TrafficModify);
    let (outgoing, mut wire) = mpsc::channel(QUEUE_CAPACITY);
    let res2 = Uuid::new_v4();

    let (open_tx, open_rx) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: 2,
                id: res2,
                revision: 1,
                read_only: false,
                request: request(),
                reply: open_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let ow = wire.try_recv().unwrap();
    let mut snap = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(PanelSnapshot {
            revision: 1,
            title: "p2".into(),
            root: Some(panel::Node::new(
                1,
                "ed",
                panel::node::Kind::Editor(panel::Editor {
                    document_id: 1,
                    revision: 1,
                    contents: b"init".to_vec(),
                    format: panel::EditorFormat::Utf8 as i32,
                    read_only: false,
                }),
            )),
        }),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    snap.in_reply_to = ow.request_id;
    running.frame(snap, &outgoing).await.unwrap();
    assert!(open_rx.await.is_ok());

    let commit_op = Uuid::new_v4();
    let (c_tx, _c_rx) = oneshot::channel();
    running
        .control(
            Control::Commit {
                panel_id: 2,
                op_id: commit_op,
                request: CommitEditor {
                    resource_id: res2.as_bytes().to_vec(),
                    message_revision: 1,
                    request: Some(request().into()),
                    editor: Some(EditorSnapshot {
                        document_id: 1,
                        revision: 1,
                        contents: b"init".to_vec(),
                        ..Default::default()
                    }),
                },
                cancelled: CancellationToken::new(),
                reply: c_tx,
                            expected_snapshot: None,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let _commit_wire = wire.try_recv().unwrap();

    // Cancel Commit -> host emits Cancel frame whose request_id is recorded in cancel_notifications.
    running
        .control(
            Control::CancelPanelOp {
                panel_id: 2,
                op_id: commit_op,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let cancel_wire = wire.try_recv().unwrap();
    assert!(matches!(cancel_wire.body, Some(envelope::Body::Cancel(_))));
    let expected_cancel_deadline = running.cancel_notifications.back().unwrap().deadline;

    // 1. Safe allowlisted cleanup calls (`QueryHistory`, `ScopeCheck`) succeed and clamp deadline.
    for op in [
        host_call::Operation::QueryHistory(protocol::HistoryQuery {
            offset: 0,
            limit: 1,
            filter: String::new(),
        }),
        host_call::Operation::ScopeCheck("https://example.invalid/".into()),
    ] {
        let hc_safe = HostCall {
            operation: Some(op),
            parent_request_id: cancel_wire.request_id,
        };
        let env_safe = Envelope::new(
            running.identity,
            2,
            1,
            envelope::Body::HostCall(hc_safe.clone()),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.host_call(&env_safe, hc_safe, &outgoing).await.unwrap();
        let req_safe = calls.try_recv().unwrap();
        assert!(req_safe.deadline <= expected_cancel_deadline);
    }

    // 2. Non-allowlisted mutating calls (`UpdateDraft`, `OpenRepeater`, `ScopeUpdate`, `OpenOrganiser`)
    // are ALL rejected with ErrorCode::Cancelled even when granted!
    for op in [
        host_call::Operation::UpdateDraft(protocol::DraftUpdate {
            resource_id: res2.as_bytes().to_vec(),
            expected_revision: 1,
            request: Some(request().into()),
        }),
        host_call::Operation::OpenRepeater(request().into()),
        host_call::Operation::ScopeUpdate(protocol::ScopeUpdateDto {
            url_prefix: "https://example.invalid/".into(),
            exclude: false,
            include_subdomains: true,
        }),
        host_call::Operation::OpenOrganiser(protocol::OpenOrganiserDto {
            request: Some(request().into()),
            response: None,
            notes: "cancel-attempt".into(),
            highlight_color: None,
        }),
    ] {
        let hc_mut = HostCall {
            operation: Some(op),
            parent_request_id: cancel_wire.request_id,
        };
        let env_mut = Envelope::new(
            running.identity,
            3,
            1,
            envelope::Body::HostCall(hc_mut.clone()),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.host_call(&env_mut, hc_mut, &outgoing).await.unwrap();
        assert!(calls.try_recv().is_err());
        let reply = wire.try_recv().unwrap();
        assert!(matches!(
            reply.body,
            Some(envelope::Body::HostResult(HostResult {
                outcome: Some(host_result::Outcome::Error(Failure { code, .. })),
            })) if code == ErrorCode::Cancelled as i32
        ));
    }

    // 3. Past cancel window -> even QueryHistory is rejected with Deadline!
    running.cancel_notifications.back_mut().unwrap().deadline =
        Instant::now() - Duration::from_millis(5);
    let hc_late = HostCall {
        operation: Some(host_call::Operation::QueryHistory(protocol::HistoryQuery {
            offset: 0,
            limit: 1,
            filter: String::new(),
        })),
        parent_request_id: cancel_wire.request_id,
    };
    let env_late = Envelope::new(
        running.identity,
        4,
        1,
        envelope::Body::HostCall(hc_late.clone()),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    running.host_call(&env_late, hc_late, &outgoing).await.unwrap();
    assert!(calls.try_recv().is_err());
    let late_reply = wire.try_recv().unwrap();
    assert!(matches!(
        late_reply.body,
        Some(envelope::Body::HostResult(HostResult {
            outcome: Some(host_result::Outcome::Error(Failure { code, .. })),
        })) if code == ErrorCode::Deadline as i32
    ));

    // 4. Sending Ack(in_reply_to: cancel_wire.request_id) immediately terminates the cancel window -> Stale!
    running.cancel_notifications.back_mut().unwrap().deadline = Instant::now() + MAX_OPERATION;
    let mut cancel_ack =
        Envelope::new(running.identity, 2, 1, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION)
            .with_panel_id(2);
    cancel_ack.in_reply_to = cancel_wire.request_id;
    running.frame(cancel_ack, &outgoing).await.unwrap();
    assert!(running.cancel_notifications.is_empty());

    let hc_after_ack = HostCall {
        operation: Some(host_call::Operation::QueryHistory(protocol::HistoryQuery {
            offset: 0,
            limit: 1,
            filter: String::new(),
        })),
        parent_request_id: cancel_wire.request_id,
    };
    let env_after_ack = Envelope::new(
        running.identity,
        5,
        1,
        envelope::Body::HostCall(hc_after_ack.clone()),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    running
        .host_call(&env_after_ack, hc_after_ack, &outgoing)
        .await
        .unwrap();
    assert!(calls.try_recv().is_err());
    let after_ack_reply = wire.try_recv().unwrap();
    assert!(matches!(
        after_ack_reply.body,
        Some(envelope::Body::HostResult(HostResult {
            outcome: Some(host_result::Outcome::Error(Failure { code, .. })),
        })) if code == ErrorCode::Stale as i32
    ));
}

#[tokio::test]
async fn correlated_hook_replies_do_not_exhaust_unsolicited_frame_rate_limit() {
    let (mut running, _calls, _updates) = running();
    let (outgoing, mut wire) = mpsc::channel(QUEUE_CAPACITY * 2);

    for seq in 1..=520u64 {
        let (reply, mut rx) = oneshot::channel();
        running
            .control(
                Control::Hook {
                    permission: Permission::TrafficRead,
                    body: envelope::Body::HttpHookCall(protocol::HttpHookCall {
                        flow_id: Uuid::new_v4().as_bytes().to_vec(),
                        source: protocol::SourceDto::Proxy as i32,
                        plugin_id: String::new(),
                        response_stage: false,
                        response_head_only: false,
                        request: Some(request().into()),
                        response: None,
                    }),
                    cancelled: CancellationToken::new(),
                    reply,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let sent = wire.try_recv().unwrap();
        let mut resp = Envelope::new(
            running.identity,
            seq,
            1,
            envelope::Body::HttpHookResult(protocol::HttpHookResult {
                action: protocol::HookAction::Pass as i32,
                request: None,
                response: None,
            }),
            MAX_OPERATION,
        );
        resp.in_reply_to = sent.request_id;
        running.frame(resp, &outgoing).await.unwrap();
        assert!(matches!(rx.try_recv(), Ok(Ok(HookReply::Http(_)))));
    }

    let mut hit_rate_limit = false;
    for offset in 1..=520u64 {
        let env = Envelope::new(
            running.identity,
            520 + offset,
            1,
            envelope::Body::HostCall(HostCall {
                parent_request_id: 0,
                operation: Some(host_call::Operation::ScopeCheck(
                    "http://127.0.0.1/".into(),
                )),
            }),
            MAX_OPERATION,
        );
        if let Err(err) = running.frame(env, &outgoing).await {
            assert!(err.to_string().contains("消息频率"), "{err}");
            hit_rate_limit = true;
            break;
        }
    }
    assert!(
        hit_rate_limit,
        "unsolicited plugin frames above 512/s must still be rate-limited"
    );
}
