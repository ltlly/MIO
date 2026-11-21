use super::*;

#[tokio::test]
async fn resync_retries_boundedly_on_deadline_error_and_clears_only_on_valid_ack() {
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

    // 1. Schedule Resync -> emits Resync #1 while retaining resync_needed_seq.
    running.schedule_panel_resync(2);
    running.flush_pending_resyncs(&outgoing).unwrap();
    let resync1 = wire.try_recv().unwrap();
    assert!(matches!(resync1.body, Some(envelope::Body::Resync(_))));
    assert_eq!(running.panels[&2].resync_attempts, 1);
    assert_ne!(running.panels[&2].resync_needed_seq, 0);

    // 2. Worker replies Deadline Error -> host MUST retain need & retry Resync #2 immediately!
    let mut deadline_err = Envelope::new(
        running.identity,
        2,
        1,
        envelope::Body::Error(Failure::new(ErrorCode::Deadline, "queued resync expired")),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    deadline_err.in_reply_to = resync1.request_id;
    running.frame(deadline_err, &outgoing).await.unwrap();
    let resync2 = wire.try_recv().unwrap();
    assert!(matches!(resync2.body, Some(envelope::Body::Resync(_))));
    assert_eq!(running.panels[&2].resync_attempts, 2);

    // 3. Worker replies Ack for Resync #2 -> resync_needed_seq clears to 0!
    let mut ack2 =
        Envelope::new(running.identity, 3, 1, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION)
            .with_panel_id(2);
    ack2.in_reply_to = resync2.request_id;
    running.frame(ack2, &outgoing).await.unwrap();
    assert_eq!(running.panels[&2].resync_needed_seq, 0);
    assert_eq!(running.panels[&2].resync_attempts, 0);
    assert!(wire.try_recv().is_err());

    // 4. Exhaust MAX_RESYNC_RETRIES (3) via repeated Deadline errors -> stops cleanly without infinite loop!
    while updates.try_recv().is_ok() {}
    running.schedule_panel_resync(2);
    running.flush_pending_resyncs(&outgoing).unwrap();
    for seq in 4..=(3 + u64::from(MAX_RESYNC_RETRIES)) {
        let r = wire.try_recv().unwrap();
        assert!(matches!(r.body, Some(envelope::Body::Resync(_))));
        let mut err = Envelope::new(
            running.identity,
            seq,
            1,
            envelope::Body::Error(Failure::new(ErrorCode::Deadline, "expired")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        err.in_reply_to = r.request_id;
        running.frame(err, &outgoing).await.unwrap();
    }
    assert!(
        wire.try_recv().is_err(),
        "resync retries must stop after MAX_RESYNC_RETRIES"
    );
    assert_eq!(running.panels[&2].resync_needed_seq, 0);
    assert!(matches!(
        updates.try_recv(),
        Ok(PluginEvent::Error { panel_id: 2, .. })
    ));
}

#[tokio::test]
async fn oversized_multi_editor_ui_event_and_oversized_resync_fail_panel_only() {
    let (mut running, _calls, mut updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

    // Build main panel with 4 editable 256 KiB editors + 1 commit button.
    let big_editor_bytes = vec![b'a'; panel::MAX_EDITOR_BYTES];
    let mut main_children = Vec::new();
    let mut editor_snaps = Vec::new();
    for doc_id in 1..=4u64 {
        main_children.push(panel::Node::new(
            doc_id,
            format!("ed-{doc_id}"),
            panel::node::Kind::Editor(panel::Editor {
                document_id: doc_id,
                revision: 1,
                contents: b"init".to_vec(),
                format: panel::EditorFormat::Utf8 as i32,
                read_only: false,
            }),
        ));
        editor_snaps.push(EditorSnapshot {
            document_id: doc_id,
            revision: 2,
            contents: big_editor_bytes.clone(),
            ..Default::default()
        });
    }
    main_children.push(panel::Node::new(
        10,
        "commit-btn",
        panel::node::Kind::Button(panel::Button {
            label: "Go".into(),
            enabled: true,
            commit_editors: true,
        }),
    ));
    *running.main_panel_mut() = PanelState::new(PanelSnapshot {
        revision: 1,
        title: "multi-big".into(),
        root: Some(panel::Node::new(
            99,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: main_children,
            }),
        )),
    })
    .unwrap();

    // 1. Multi-editor UiEvent > 1 MiB MUST emit single-panel Error without killing worker OR mutating state!
    running
        .control(
            Control::Event {
                panel_id: protocol::PANEL_ID,
                event: panel::UiEvent {
                    node_id: 10,
                    editors: editor_snaps,
                    event: Some(panel::ui_event::Event::Click(panel::Empty {})),
                },
                expected_snapshot: None,
                event_seq: 0,
            },
            &outgoing,
        )
        .await
        .expect("oversized multi-editor UiEvent must NOT kill worker");
    assert!(wire.try_recv().is_err());
    assert_eq!(running.main_panel().editor(1).unwrap().revision, 1);
    assert!(matches!(
        updates.try_recv(),
        Ok(PluginEvent::Error {
            panel_id: protocol::PANEL_ID,
            ..
        })
    ));

    // 2. Child panel accumulates 5 x 220 KiB editors via valid patches (> 1 MiB full snapshot):
    // automatic Resync must emit single-panel Error and clear retry without killing shared worker!
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
    let child_children: Vec<_> = (1..=5u64)
        .map(|doc_id| {
            panel::Node::new(
                doc_id,
                format!("c-{doc_id}"),
                panel::node::Kind::Editor(panel::Editor {
                    document_id: doc_id,
                    revision: 1,
                    contents: vec![b'x'; 160 * 1024],
                    format: panel::EditorFormat::Utf8 as i32,
                    read_only: false,
                }),
            )
        })
        .collect();
    let mut open_snap = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(PanelSnapshot {
            revision: 1,
            title: "child-grow".into(),
            root: Some(panel::Node::new(
                99,
                "col",
                panel::node::Kind::Column(panel::Container {
                    children: child_children,
                }),
            )),
        }),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    open_snap.in_reply_to = ow.request_id;
    running.frame(open_snap, &outgoing).await.unwrap();
    assert!(open_rx.await.is_ok());

    for (idx, doc_id) in (1..=5u64).enumerate() {
        let rev = idx as u64 + 2;
        let patch_env = Envelope::new(
            running.identity,
            rev,
            rev,
            envelope::Body::Patch(panel::PanelPatch {
                base_revision: rev - 1,
                revision: rev,
                operations: vec![panel::PatchOperation {
                    node_id: doc_id,
                    change: Some(panel::patch_operation::Change::Editor(panel::EditorUpdate {
                        base_revision: 1,
                        snapshot: Some(EditorSnapshot {
                            document_id: doc_id,
                            revision: 2,
                            contents: vec![b'y'; 230 * 1024],
                            ..Default::default()
                        }),
                    })),
                }],
            }),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(patch_env, &outgoing).await.unwrap();
    }
    while updates.try_recv().is_ok() {}

    // Now full PanelSnapshot on child panel 2 is > 1 MiB: resync_panel and flush_pending_resyncs fail panel-only!
    running
        .resync_panel(2, &outgoing, 0, "stale patch".into())
        .await
        .expect("oversized resync must not terminate shared worker");
    assert_eq!(running.panels[&2].resync_needed_seq, 0);
    assert!(wire.try_recv().is_err());
    assert!(matches!(
        updates.try_recv(),
        Ok(PluginEvent::Error { panel_id: 2, .. })
    ));
}

#[tokio::test]
async fn resync_repeated_invalid_snapshot_respects_retry_budget_and_never_loops() {
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
    let ow = wire.try_recv().unwrap();
    let child_root = || {
        Some(panel::Node::new(
            1,
            "ed",
            panel::node::Kind::Editor(panel::Editor {
                document_id: 1,
                revision: 1,
                contents: b"init".to_vec(),
                format: panel::EditorFormat::Utf8 as i32,
                read_only: false,
            }),
        ))
    };
    let mut open_snap = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(PanelSnapshot {
            revision: 1,
            title: "p2".into(),
            root: child_root(),
        }),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    open_snap.in_reply_to = ow.request_id;
    running.frame(open_snap, &outgoing).await.unwrap();
    assert!(open_rx.await.is_ok());

    // Advance child panel 2 locally to rev=2 so Snapshot(rev=1) will be rejected by PanelState::replace.
    let mut rev2_snap = Envelope::new(
        running.identity,
        2,
        2,
        envelope::Body::Snapshot(PanelSnapshot {
            revision: 2,
            title: "p2-rev2".into(),
            root: child_root(),
        }),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    rev2_snap.in_reply_to = 0;
    running.frame(rev2_snap, &outgoing).await.unwrap();
    while updates.try_recv().is_ok() {}

    // 1. Rejection starts ONE recovery cycle and emits Resync #1 (`resync_attempts == 1`).
    running
        .resync_panel(2, &outgoing, 0, "initial mismatch".into())
        .await
        .unwrap();
    let resync1 = wire.try_recv().unwrap();
    assert!(matches!(resync1.body, Some(envelope::Body::Resync(_))));
    assert_eq!(running.panels[&2].resync_attempts, 1);
    let seq = running.panels[&2].resync_needed_seq;
    assert_ne!(seq, 0);

    // 2. While Resync #1 is in flight, an unrelated late Snapshot arrives -> must NOT open a new generation or emit duplicate Resync!
    let mut late_unrelated = Envelope::new(
        running.identity,
        3,
        1,
        envelope::Body::Snapshot(PanelSnapshot {
            revision: 1,
            title: "late-stale".into(),
            root: child_root(),
        }),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    late_unrelated.in_reply_to = 9999;
    running.frame(late_unrelated, &outgoing).await.unwrap();
    assert!(wire.try_recv().is_err());
    assert_eq!(running.panels[&2].resync_needed_seq, seq);
    assert_eq!(running.panels[&2].resync_attempts, 1);

    // 3. Repeatedly rejecting Resync responses with stale rev=1 Snapshot advances within the SAME budget up to MAX_RESYNC_RETRIES (3).
    let mut current_resync_id = resync1.request_id;
    let mut wire_seq = 4u64;
    for expected_next_attempt in 2..=MAX_RESYNC_RETRIES {
        let mut bad_reply = Envelope::new(
            running.identity,
            wire_seq,
            1,
            envelope::Body::Snapshot(PanelSnapshot {
                revision: 1,
                title: "still-stale".into(),
                root: child_root(),
            }),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        bad_reply.in_reply_to = current_resync_id;
        wire_seq += 1;
        running.frame(bad_reply, &outgoing).await.unwrap();
        let next_resync = wire.try_recv().unwrap();
        assert!(matches!(next_resync.body, Some(envelope::Body::Resync(_))));
        assert_eq!(running.panels[&2].resync_needed_seq, seq);
        assert_eq!(running.panels[&2].resync_attempts, expected_next_attempt);
        current_resync_id = next_resync.request_id;
    }

    // Rejecting the 3rd Resync hits MAX_RESYNC_RETRIES -> stops retrying cleanly and leaves wire empty!
    let mut final_bad = Envelope::new(
        running.identity,
        wire_seq,
        1,
        envelope::Body::Snapshot(PanelSnapshot {
            revision: 1,
            title: "still-stale-final".into(),
            root: child_root(),
        }),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    final_bad.in_reply_to = current_resync_id;
    running.frame(final_bad, &outgoing).await.unwrap();
    assert!(
        wire.try_recv().is_err(),
        "repeated rejected resync snapshots must halt at MAX_RESYNC_RETRIES"
    );
    assert_eq!(running.panels[&2].resync_needed_seq, 0);
}

#[tokio::test]
async fn fresh_wire_snapshot_after_local_pending_deadline_recovers_child_without_killing_worker() {
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
    let ow = wire.try_recv().unwrap();
    let mut open_snap = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(PanelSnapshot {
            revision: 1,
            title: "p2-base".into(),
            root: Some(panel::Node::new(
                1,
                "ed",
                panel::node::Kind::Editor(panel::Editor {
                    document_id: 1,
                    revision: 1,
                    contents: b"base".to_vec(),
                    format: panel::EditorFormat::Utf8 as i32,
                    read_only: false,
                }),
            )),
        }),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    open_snap.in_reply_to = ow.request_id;
    running.frame(open_snap, &outgoing).await.unwrap();
    assert!(open_rx.await.is_ok());

    running
        .control(
            Control::Event {
                panel_id: 2,
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
    let ev_wire = wire.try_recv().unwrap();

    // Expire local Pending.deadline BEFORE handle_timeouts runs, while worker reply carries a FRESH wire deadline!
    running.requests.get_mut(&ev_wire.request_id).unwrap().deadline =
        Instant::now() - Duration::from_millis(10);
    while updates.try_recv().is_ok() {}

    let mut fresh_wire_late_snap = Envelope::new(
        running.identity,
        2,
        2,
        envelope::Body::Snapshot(PanelSnapshot {
            revision: 2,
            title: "p2-mutated-late".into(),
            root: Some(panel::Node::new(
                1,
                "ed",
                panel::node::Kind::Editor(panel::Editor {
                    document_id: 1,
                    revision: 2,
                    contents: b"stale".to_vec(),
                    format: panel::EditorFormat::Utf8 as i32,
                    read_only: false,
                }),
            )),
        }),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    fresh_wire_late_snap.in_reply_to = ev_wire.request_id;
    running
        .frame(fresh_wire_late_snap, &outgoing)
        .await
        .expect("fresh-wire reply after local Pending deadline must recover child instead of failing loop");

    assert_eq!(
        running.panel_revision(2),
        1,
        "expired local Pending MUST prevent Snapshot(rev=2) from mutating host panel state"
    );
    let cancel_msg = wire.try_recv().unwrap();
    let resync_msg = wire.try_recv().unwrap();
    assert!(matches!(cancel_msg.body, Some(envelope::Body::Cancel(_))));
    assert!(matches!(resync_msg.body, Some(envelope::Body::Resync(ref s)) if s.revision == 1));
    assert!(matches!(
        updates.try_recv(),
        Ok(PluginEvent::Error { panel_id: 2, .. })
    ));
}

#[tokio::test]
async fn legacy_cancel_snapshot_reply_closes_window_and_resyncs_main_panel() {
    let (mut running, mut calls, mut updates) = running();
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
    let res1 = Uuid::new_v4();

    let (open_tx, _open_rx) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: protocol::PANEL_ID,
                id: res1,
                revision: 1,
                read_only: false,
                request: request(),
                reply: open_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let _open_wire = wire.try_recv().unwrap();

    // Revoke main panel resource while Open is active -> host sends Cancel and tracks it in cancel_notifications.
    let (rev_tx, _rev_rx) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: protocol::PANEL_ID,
                id: res1,
                remove_panel: false,
                reply: rev_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let cancel_wire = wire.try_recv().unwrap();
    let _close_wire = wire.try_recv().unwrap();
    assert!(matches!(cancel_wire.body, Some(envelope::Body::Cancel(_))));
    assert_eq!(running.cancel_notifications.len(), 1);
    while updates.try_recv().is_ok() {}

    // Legacy plugin replies to Cancel with a drifted Snapshot(rev=2) -> consumes silently & triggers Resync(1)!
    let mut legacy_snap = Envelope::new(
        running.identity,
        1,
        2,
        envelope::Body::Snapshot(PanelSnapshot {
            revision: 2,
            title: "legacy-cancel-snap".into(),
            root: Some(panel::Node::new(
                1,
                "ed",
                panel::node::Kind::Editor(panel::Editor {
                    document_id: 1,
                    revision: 1,
                    contents: b"legacy".to_vec(),
                    format: panel::EditorFormat::Utf8 as i32,
                    read_only: false,
                }),
            )),
        }),
        MAX_OPERATION,
    );
    legacy_snap.in_reply_to = cancel_wire.request_id;
    running.frame(legacy_snap, &outgoing).await.unwrap();

    assert!(running.cancel_notifications.is_empty());
    assert_eq!(running.panel_revision(protocol::PANEL_ID), 1);
    assert!(
        updates.try_recv().is_err(),
        "legacy Cancel Snapshot reply must not emit spurious Stale diagnostic"
    );
    let resync_msg = wire.try_recv().unwrap();
    assert_eq!(resync_msg.panel_id, protocol::PANEL_ID);
    assert!(matches!(resync_msg.body, Some(envelope::Body::Resync(ref s)) if s.revision == 1));

    // After Cancel reply window closed, HostCall referencing cancel_wire.request_id is rejected as Stale.
    let hc_after = HostCall {
        operation: Some(host_call::Operation::QueryHistory(protocol::HistoryQuery {
            offset: 0,
            limit: 1,
            filter: String::new(),
        })),
        parent_request_id: cancel_wire.request_id,
    };
    let env_after = Envelope::new(
        running.identity,
        2,
        1,
        envelope::Body::HostCall(hc_after.clone()),
        MAX_OPERATION,
    );
    running.host_call(&env_after, hc_after, &outgoing).await.unwrap();
    assert!(calls.try_recv().is_err());
    let stale_reply = wire.try_recv().unwrap();
    assert!(matches!(
        stale_reply.body,
        Some(envelope::Body::HostResult(HostResult {
            outcome: Some(host_result::Outcome::Error(Failure { code, .. })),
        })) if code == ErrorCode::Stale as i32
    ));
}

#[tokio::test]
async fn late_resync_snapshot_after_budget_exhausted_does_not_restart_cycle() {
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
    let ow = wire.try_recv().unwrap();
    let child_snap = |rev: u64| PanelSnapshot {
        revision: rev,
        title: format!("p2-{rev}"),
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
    };
    let mut open_snap = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(child_snap(1)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    open_snap.in_reply_to = ow.request_id;
    running.frame(open_snap, &outgoing).await.unwrap();
    assert!(open_rx.await.is_ok());

    // Exhaust 7 recovery cycles (21 Resync attempts > 16) on the SAME child panel 2.
    let mut sent_resync_ids = Vec::new();
    for _cycle in 0..7 {
        running.schedule_panel_resync(2);
        running.flush_pending_resyncs(&outgoing).unwrap();
        for _ in 0..MAX_RESYNC_RETRIES {
            let r = wire.try_recv().unwrap();
            assert!(matches!(r.body, Some(envelope::Body::Resync(_))));
            sent_resync_ids.push(r.request_id);
            running.requests.get_mut(&r.request_id).unwrap().deadline =
                Instant::now() - Duration::from_secs(1);
            running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
            let cancel_msg = wire.try_recv().unwrap();
            assert!(matches!(cancel_msg.body, Some(envelope::Body::Cancel(_))));
        }
        assert_eq!(running.panels[&2].resync_needed_seq, 0);
        assert!(wire.try_recv().is_err());
    }
    assert!(sent_resync_ids.len() > 16);
    while updates.try_recv().is_ok() {}

    // Late Snapshots replying to ANY of the 21 exhausted Resync request_ids (including the earliest) MUST NOT restart recovery!
    let mut seq = 2u64;
    for old_resync_id in sent_resync_ids {
        let mut late_resync_snap = Envelope::new(
            running.identity,
            seq,
            2,
            envelope::Body::Snapshot(child_snap(2)),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        seq += 1;
        late_resync_snap.in_reply_to = old_resync_id;
        running.frame(late_resync_snap, &outgoing).await.unwrap();
        assert!(
            wire.try_recv().is_err(),
            "late reply to exhausted Resync cycle must not re-trigger Resync even after >16 same-panel resyncs"
        );
        assert_eq!(running.panels[&2].resync_needed_seq, 0);
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Error { panel_id: 2, .. })
        ));
    }

    // An unknown/unassociated request_id is also dropped without starting Resync.
    let mut unknown_late = Envelope::new(
        running.identity,
        seq,
        2,
        envelope::Body::Snapshot(child_snap(2)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    unknown_late.in_reply_to = 9999;
    running.frame(unknown_late, &outgoing).await.unwrap();
    assert!(wire.try_recv().is_err());
    assert_eq!(running.panels[&2].resync_needed_seq, 0);
    assert!(running.panels[&2].resync_semantic_drift);
    while updates.try_recv().is_ok() {}

    // By contrast, a genuinely cancelled non-resync child UiEvent STILL triggers a fresh Resync once on its first late Snapshot.
    let (mut running, _calls, _updates) = crate::plugins::tests_runtime::running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
    let (open_tx, open_rx) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: 2,
                id: Uuid::new_v4(),
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
    let mut open_snap = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(child_snap(1)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    open_snap.in_reply_to = ow.request_id;
    running.frame(open_snap, &outgoing).await.unwrap();
    assert!(open_rx.await.is_ok());
    let mut seq = 2u64;

    running
        .control(
            Control::Event {
                panel_id: 2,
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
    let ui_req = wire.try_recv().unwrap();
    running.requests.get_mut(&ui_req.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
    let _ui_cancel = wire.try_recv().unwrap();
    let initial_ui_resync = wire.try_recv().unwrap();
    assert!(matches!(initial_ui_resync.body, Some(envelope::Body::Resync(_))));
    let mut resync_ack =
        Envelope::new(running.identity, seq, 1, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION)
            .with_panel_id(2);
    seq += 1;
    resync_ack.in_reply_to = initial_ui_resync.request_id;
    running.frame(resync_ack, &outgoing).await.unwrap();
    assert_eq!(running.panels[&2].resync_needed_seq, 0);
    while wire.try_recv().is_ok() {}

    let mut late_ui_snap = Envelope::new(
        running.identity,
        seq,
        2,
        envelope::Body::Snapshot(child_snap(2)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    seq += 1;
    late_ui_snap.in_reply_to = ui_req.request_id;
    running.frame(late_ui_snap, &outgoing).await.unwrap();
    let fresh_resync = wire.try_recv().unwrap();
    assert!(matches!(fresh_resync.body, Some(envelope::Body::Resync(_))));
    let mut resync_ack2 =
        Envelope::new(running.identity, seq, 1, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION)
            .with_panel_id(2);
    seq += 1;
    resync_ack2.in_reply_to = fresh_resync.request_id;
    running.frame(resync_ack2, &outgoing).await.unwrap();

    // Replaying the same cancelled UiEvent id a second time must NOT re-trigger Resync.
    let mut replay_ui_snap = Envelope::new(
        running.identity,
        seq,
        2,
        envelope::Body::Snapshot(child_snap(2)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    replay_ui_snap.in_reply_to = ui_req.request_id;
    running.frame(replay_ui_snap, &outgoing).await.unwrap();
    assert!(wire.try_recv().is_err());
}

#[tokio::test]
async fn main_panel_resync_timeout_retries_boundedly_without_killing_worker() {
    let (mut running, _calls, _updates) = running();
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

    running.schedule_panel_resync(protocol::PANEL_ID);
    running.flush_pending_resyncs(&outgoing).unwrap();
    let resync1 = wire.try_recv().unwrap();
    assert!(matches!(resync1.body, Some(envelope::Body::Resync(_))));
    assert_eq!(running.main_slot().resync_attempts, 1);

    // 1. Natural timeout on main-panel Resync #1 MUST NOT kill worker -> emits Cancel + Resync #2!
    running.requests.get_mut(&resync1.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    running
        .handle_timeouts(Instant::now(), &outgoing)
        .await
        .expect("main-panel Resync timeout must retry boundedly without terminating worker");
    let cancel1 = wire.try_recv().unwrap();
    let resync2 = wire.try_recv().unwrap();
    assert!(matches!(cancel1.body, Some(envelope::Body::Cancel(_))));
    assert!(matches!(resync2.body, Some(envelope::Body::Resync(_))));
    assert_eq!(running.main_slot().resync_attempts, 2);

    // 2. Wire-expired reply to main-panel Resync #2 (even with fresh local deadline) ALSO retries boundedly without killing worker!
    let mut wire_expired_resync2_snap = Envelope::new(
        running.identity,
        1,
        2,
        envelope::Body::Snapshot(PanelSnapshot {
            revision: 2,
            title: "wire-expired-resync2".into(),
            root: Some(panel::Node::new(
                1,
                "ed",
                panel::node::Kind::Editor(panel::Editor {
                    document_id: 1,
                    revision: 2,
                    contents: b"late".to_vec(),
                    format: panel::EditorFormat::Utf8 as i32,
                    read_only: false,
                }),
            )),
        }),
        MAX_OPERATION,
    );
    wire_expired_resync2_snap.deadline_unix_ms = 1;
    wire_expired_resync2_snap.in_reply_to = resync2.request_id;
    running
        .frame(wire_expired_resync2_snap, &outgoing)
        .await
        .expect("wire-expired main-panel Resync Snapshot must not terminate worker");
    assert_eq!(running.panel_revision(protocol::PANEL_ID), 1);
    let cancel2 = wire.try_recv().unwrap();
    let resync3 = wire.try_recv().unwrap();
    assert!(matches!(cancel2.body, Some(envelope::Body::Cancel(_))));
    assert!(matches!(resync3.body, Some(envelope::Body::Resync(_))));
    assert_eq!(running.main_slot().resync_attempts, 3);

    // 3. Local-expired + fresh-wire Snapshot replying to main-panel Resync #3 stops cleanly at MAX_RESYNC_RETRIES without killing worker!
    running.requests.get_mut(&resync3.request_id).unwrap().deadline =
        Instant::now() - Duration::from_millis(5);
    let mut local_expired_resync3_snap = Envelope::new(
        running.identity,
        2,
        2,
        envelope::Body::Snapshot(PanelSnapshot {
            revision: 2,
            title: "local-expired-resync3".into(),
            root: Some(panel::Node::new(
                1,
                "ed",
                panel::node::Kind::Editor(panel::Editor {
                    document_id: 1,
                    revision: 2,
                    contents: b"late".to_vec(),
                    format: panel::EditorFormat::Utf8 as i32,
                    read_only: false,
                }),
            )),
        }),
        MAX_OPERATION,
    );
    local_expired_resync3_snap.in_reply_to = resync3.request_id;
    running.frame(local_expired_resync3_snap, &outgoing).await.unwrap();
    let cancel3 = wire.try_recv().unwrap();
    assert!(matches!(cancel3.body, Some(envelope::Body::Cancel(_))));
    assert!(wire.try_recv().is_err());
    assert_eq!(running.main_slot().resync_needed_seq, 0);
    assert!(running.main_slot().resync_semantic_drift);

    // 4. Ordinary main-panel UiEvent timeout still terminates the worker loop.
    let (mut running, _calls, mut updates) = crate::plugins::tests_runtime::running();
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
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
    let ev_wire = wire.try_recv().unwrap();
    running.requests.get_mut(&ev_wire.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    assert!(running.handle_timeouts(Instant::now(), &outgoing).await.is_err());
    let _ = updates.try_recv();
}

#[tokio::test]
async fn main_panel_local_expired_non_resync_matches_fatal_timeout_policy_and_drifted_host_call_resyncs()
{
    // 1. Main-panel drifted HostCall (revision=2 > host_rev=1) schedules main-panel Resync(1)!
    {
        let (mut running, _calls, _updates) = running();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        let hc_drifted = HostCall {
            operation: Some(host_call::Operation::QueryHistory(protocol::HistoryQuery {
                offset: 0,
                limit: 1,
                filter: String::new(),
            })),
            parent_request_id: 0,
        };
        let env_drifted = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::HostCall(hc_drifted),
            MAX_OPERATION,
        );
        running.frame(env_drifted, &outgoing).await.unwrap();
        let main_hc_resync = wire.try_recv().unwrap();
        assert!(matches!(
            main_hc_resync.body,
            Some(envelope::Body::Resync(ref s)) if s.revision == 1
        ));
        let mut ack_hc_resync = Envelope::new(
            running.identity,
            2,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        );
        ack_hc_resync.in_reply_to = main_hc_resync.request_id;
        running.frame(ack_hc_resync, &outgoing).await.unwrap();
        assert!(running.main_slot().resync_semantic_drift);
    }

    let (mut running, _calls, _updates) = running();
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
    let res1 = Uuid::new_v4();

    // 2. Main-panel Open expires locally before timer -> Snapshot rejected with fatal timeout policy without mutating panel!
    let (open_tx, mut open_rx) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: protocol::PANEL_ID,
                id: res1,
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
    running.requests.get_mut(&ow.request_id).unwrap().deadline =
        Instant::now() - Duration::from_millis(5);
    let mut late_open_snap = Envelope::new(
        running.identity,
        3,
        2,
        envelope::Body::Snapshot(PanelSnapshot {
            revision: 2,
            title: "late-open-main".into(),
            root: Some(panel::Node::new(
                1,
                "ed",
                panel::node::Kind::Editor(panel::Editor {
                    document_id: 1,
                    revision: 2,
                    contents: b"stale".to_vec(),
                    format: panel::EditorFormat::Utf8 as i32,
                    read_only: false,
                }),
            )),
        }),
        MAX_OPERATION,
    );
    late_open_snap.in_reply_to = ow.request_id;
    let err_open = running.frame(late_open_snap, &outgoing).await.unwrap_err();
    assert!(err_open.to_string().contains("超时"));
    assert!(open_rx.try_recv().unwrap().is_err());
    assert_eq!(running.panel_revision(protocol::PANEL_ID), 1);
    while wire.try_recv().is_ok() {}

    // 3. Main-panel Close expires locally before timer -> fresh-wire Ack triggers fatal timeout instead of Ok(())!
    let (rev_tx, mut rev_rx) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: protocol::PANEL_ID,
                id: res1,
                remove_panel: false,
                reply: rev_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let close_wire = wire.try_recv().unwrap();
    running.requests.get_mut(&close_wire.request_id).unwrap().deadline =
        Instant::now() - Duration::from_millis(5);
    let mut late_close_ack =
        Envelope::new(running.identity, 4, 1, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION);
    late_close_ack.in_reply_to = close_wire.request_id;
    let err_close = running.frame(late_close_ack, &outgoing).await.unwrap_err();
    assert!(err_close.to_string().contains("超时"));
    assert!(rev_rx.try_recv().unwrap().is_err());
    while wire.try_recv().is_ok() {}

    // 4. Main-panel non-resync UiEvent expires locally before timer -> fresh-wire Ack triggers fatal timeout Err!
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
    let ev_wire = wire.try_recv().unwrap();
    running.requests.get_mut(&ev_wire.request_id).unwrap().deadline =
        Instant::now() - Duration::from_millis(5);
    let mut late_ev_ack =
        Envelope::new(running.identity, 5, 1, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION);
    late_ev_ack.in_reply_to = ev_wire.request_id;
    let err_ev = running.frame(late_ev_ack, &outgoing).await.unwrap_err();
    assert!(err_ev.to_string().contains("超时"));
}

#[tokio::test]
async fn interleaved_resync_ack_matching_sent_baseline_resyncs_latest_without_marking_incompatible() {
    let (mut running, mut calls, mut updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
    let snap = |rev: u64| PanelSnapshot {
        revision: rev,
        title: format!("rev-{rev}"),
        root: Some(panel::Node::new(
            1,
            "ed",
            panel::node::Kind::Editor(panel::Editor {
                document_id: 1,
                revision: rev,
                contents: format!("body-{rev}").into_bytes(),
                format: panel::EditorFormat::Utf8 as i32,
                read_only: false,
            }),
        )),
    };

    // 1. Child panel: Open at rev=1, enqueue UiEvent, then enqueue Resync(1) before Snapshot(2) returns.
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
    let mut open_snap = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(snap(1)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    open_snap.in_reply_to = ow.request_id;
    running.frame(open_snap, &outgoing).await.unwrap();
    assert!(open_rx.await.is_ok());

    running
        .control(
            Control::Event {
                panel_id: 2,
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
    let child_ev = wire.try_recv().unwrap();
    running.schedule_panel_resync(2);
    running.flush_pending_resyncs(&outgoing).unwrap();
    let child_resync1 = wire.try_recv().unwrap();
    let child_seq = running.panels[&2].resync_needed_seq;
    assert_ne!(child_seq, 0);
    assert!(matches!(
        child_resync1.body,
        Some(envelope::Body::Resync(ref s)) if s.revision == 1
    ));

    // Earlier in-flight UiEvent advances host child panel from rev 1 -> rev 2.
    let mut child_snap2 = Envelope::new(
        running.identity,
        2,
        2,
        envelope::Body::Snapshot(snap(2)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    child_snap2.in_reply_to = child_ev.request_id;
    running.frame(child_snap2, &outgoing).await.unwrap();
    assert_eq!(running.panel_revision(2), 2);
    while updates.try_recv().is_ok() {}

    // SDK processed Resync(1), reverted published=1, and replies Ack(1) matching resync_revision=1!
    let mut child_ack1 = Envelope::new(
        running.identity,
        3,
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    child_ack1.in_reply_to = child_resync1.request_id;
    running.frame(child_ack1, &outgoing).await.unwrap();

    assert!(updates.try_recv().is_err());
    assert_eq!(running.panels[&2].resync_needed_seq, child_seq);
    assert_eq!(running.panels[&2].resync_attempts, 2);
    let child_resync2 = wire.try_recv().unwrap();
    assert!(matches!(
        child_resync2.body,
        Some(envelope::Body::Resync(ref s)) if s.revision == 2
    ));

    let mut child_ack2 = Envelope::new(
        running.identity,
        4,
        2,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    child_ack2.in_reply_to = child_resync2.request_id;
    running.frame(child_ack2, &outgoing).await.unwrap();
    assert_eq!(running.panels[&2].resync_needed_seq, 0);
    assert_eq!(running.panels[&2].resync_attempts, 0);

    let child_hc = HostCall {
        operation: Some(host_call::Operation::QueryHistory(protocol::HistoryQuery {
            offset: 0,
            limit: 1,
            filter: String::new(),
        })),
        parent_request_id: 0,
    };
    running
        .frame(
            Envelope::new(
                running.identity,
                5,
                2,
                envelope::Body::HostCall(child_hc),
                MAX_OPERATION,
            )
            .with_panel_id(2),
            &outgoing,
        )
        .await
        .unwrap();
    assert_eq!(calls.try_recv().unwrap().panel_id, 2);

    // 2. Main panel: same interleaving of in-flight UiEvent + Resync(1) -> Snapshot(2) -> Ack(1) -> Resync(2) -> Ack(2).
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
    let main_ev = wire.try_recv().unwrap();
    running.schedule_panel_resync(protocol::PANEL_ID);
    running.flush_pending_resyncs(&outgoing).unwrap();
    let main_resync1 = wire.try_recv().unwrap();
    let main_seq = running.main_slot().resync_needed_seq;
    assert_ne!(main_seq, 0);
    assert!(matches!(
        main_resync1.body,
        Some(envelope::Body::Resync(ref s)) if s.revision == 1
    ));

    let mut main_snap2 = Envelope::new(
        running.identity,
        6,
        2,
        envelope::Body::Snapshot(snap(2)),
        MAX_OPERATION,
    );
    main_snap2.in_reply_to = main_ev.request_id;
    running.frame(main_snap2, &outgoing).await.unwrap();
    assert_eq!(running.panel_revision(protocol::PANEL_ID), 2);
    while updates.try_recv().is_ok() {}

    let mut main_ack1 = Envelope::new(
        running.identity,
        7,
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    main_ack1.in_reply_to = main_resync1.request_id;
    running.frame(main_ack1, &outgoing).await.unwrap();

    assert!(updates.try_recv().is_err());
    assert_eq!(running.main_slot().resync_needed_seq, main_seq);
    assert_eq!(running.main_slot().resync_attempts, 2);
    let main_resync2 = wire.try_recv().unwrap();
    assert!(matches!(
        main_resync2.body,
        Some(envelope::Body::Resync(ref s)) if s.revision == 2
    ));

    let mut main_ack2 = Envelope::new(
        running.identity,
        8,
        2,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    main_ack2.in_reply_to = main_resync2.request_id;
    running.frame(main_ack2, &outgoing).await.unwrap();
    assert_eq!(running.main_slot().resync_needed_seq, 0);
    assert_eq!(running.main_slot().resync_attempts, 0);

    let main_hc = HostCall {
        operation: Some(host_call::Operation::QueryHistory(protocol::HistoryQuery {
            offset: 0,
            limit: 1,
            filter: String::new(),
        })),
        parent_request_id: 0,
    };
    running
        .frame(
            Envelope::new(
                running.identity,
                9,
                2,
                envelope::Body::HostCall(main_hc),
                MAX_OPERATION,
            ),
            &outgoing,
        )
        .await
        .unwrap();
    assert_eq!(calls.try_recv().unwrap().panel_id, protocol::PANEL_ID);
}

#[tokio::test]
async fn final_resync_cancel_snapshot_reply_does_not_restart_exhausted_budget() {
    let (mut running, mut calls, mut updates) = running();
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
    let ow = wire.try_recv().unwrap();
    let make_snap = |rev: u64| PanelSnapshot {
        revision: rev,
        title: format!("rev-{rev}"),
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
    };
    let mut open_snap = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(make_snap(1)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    open_snap.in_reply_to = ow.request_id;
    running.frame(open_snap, &outgoing).await.unwrap();
    assert!(open_rx.await.is_ok());

    let mut wire_in_seq = 2u64;
    for target_panel in [2u64, protocol::PANEL_ID] {
        running.schedule_panel_resync(target_panel);
        running.flush_pending_resyncs(&outgoing).unwrap();
        let mut last_cancel_id = 0u64;
        for _ in 0..MAX_RESYNC_RETRIES {
            let resync = wire.try_recv().unwrap();
            assert_eq!(resync.panel_id, target_panel);
            assert!(matches!(resync.body, Some(envelope::Body::Resync(_))));
            running.requests.get_mut(&resync.request_id).unwrap().deadline =
                Instant::now() - Duration::from_secs(1);
            running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
            let cancel_msg = wire.try_recv().unwrap();
            assert!(matches!(cancel_msg.body, Some(envelope::Body::Cancel(_))));
            last_cancel_id = cancel_msg.request_id;
        }
        assert!(wire.try_recv().is_err());
        while updates.try_recv().is_ok() {}
        assert_ne!(last_cancel_id, 0);
        assert!(
            running
                .cancel_notifications
                .iter()
                .any(|n| n.panel_id == target_panel
                    && n.cancel_frame_id == last_cancel_id
                    && !n.can_restart_resync)
        );

        // 1. Active window: allowlisted on_cancel cleanup HostCall referencing final Resync's Cancel with drifted rev MUST NOT restart Resync.
        let hc = HostCall {
            operation: Some(host_call::Operation::QueryHistory(protocol::HistoryQuery {
                offset: 0,
                limit: 1,
                filter: String::new(),
            })),
            parent_request_id: last_cancel_id,
        };
        running
            .frame(
                Envelope::new(
                    running.identity,
                    wire_in_seq,
                    2,
                    envelope::Body::HostCall(hc.clone()),
                    MAX_OPERATION,
                )
                .with_panel_id(target_panel),
                &outgoing,
            )
            .await
            .unwrap();
        wire_in_seq += 1;
        assert_eq!(calls.try_recv().unwrap().panel_id, target_panel);
        assert!(wire.try_recv().is_err());

        // 2. Expired window: drifted HostCall arriving after Cancel notification's 5s window expires is rejected WITHOUT restarting Resync!
        running
            .cancel_notifications
            .iter_mut()
            .rev()
            .find(|n| n.panel_id == target_panel && n.cancel_frame_id == last_cancel_id)
            .unwrap()
            .deadline = Instant::now() - Duration::from_millis(5);
        running
            .frame(
                Envelope::new(
                    running.identity,
                    wire_in_seq,
                    2,
                    envelope::Body::HostCall(hc.clone()),
                    MAX_OPERATION,
                )
                .with_panel_id(target_panel),
                &outgoing,
            )
            .await
            .unwrap();
        wire_in_seq += 1;
        assert!(calls.try_recv().is_err());
        assert!(matches!(
            wire.try_recv().unwrap().body,
            Some(envelope::Body::HostResult(HostResult {
                outcome: Some(host_result::Outcome::Error(Failure { code, .. })),
            })) if code == ErrorCode::Stale as i32
        ));
        assert!(wire.try_recv().is_err());

        // 3. Evicted notification: even if capacity evicts a Cancel notification before a drifted HostCall arrives, unknown parent CANNOT restart Resync!
        let evicted_cancel_id = 800_000 + target_panel;
        running.record_cancel_notification(target_panel, evicted_cancel_id, true, None);
        for offset in 1..=QUEUE_CAPACITY {
            running.record_cancel_notification(
                target_panel,
                evicted_cancel_id + offset as u64,
                false,
                None,
            );
        }
        assert!(
            !running
                .cancel_notifications
                .iter()
                .any(|n| n.panel_id == target_panel && n.cancel_frame_id == evicted_cancel_id)
        );
        running
            .frame(
                Envelope::new(
                    running.identity,
                    wire_in_seq,
                    2,
                    envelope::Body::HostCall(HostCall {
                        operation: hc.operation.clone(),
                        parent_request_id: evicted_cancel_id,
                    }),
                    MAX_OPERATION,
                )
                .with_panel_id(target_panel),
                &outgoing,
            )
            .await
            .unwrap();
        wire_in_seq += 1;
        assert!(calls.try_recv().is_err());
        assert!(matches!(
            wire.try_recv().unwrap().body,
            Some(envelope::Body::HostResult(HostResult {
                outcome: Some(host_result::Outcome::Error(Failure { code, .. })),
            })) if code == ErrorCode::Stale as i32
        ));
        assert!(wire.try_recv().is_err());
        while updates.try_recv().is_ok() {}
        running.cancel_notifications.clear();
        running.record_cancel_notification(target_panel, last_cancel_id, true, None);

        // 4. Legacy Cancel reply Snapshot(rev=2) on the final Resync's Cancel settles window without restarting Resync!
        let mut legacy_cancel_snap = Envelope::new(
            running.identity,
            wire_in_seq,
            2,
            envelope::Body::Snapshot(make_snap(2)),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        wire_in_seq += 1;
        legacy_cancel_snap.in_reply_to = last_cancel_id;
        running.frame(legacy_cancel_snap, &outgoing).await.unwrap();

        assert!(
            !running
                .cancel_notifications
                .iter()
                .any(|n| n.panel_id == target_panel && n.cancel_frame_id == last_cancel_id)
        );
        assert!(wire.try_recv().is_err());
        assert!(updates.try_recv().is_err());
        assert_eq!(running.panel_revision(target_panel), 1);
        if target_panel == protocol::PANEL_ID {
            assert_eq!(running.main_slot().resync_needed_seq, 0);
        } else {
            assert_eq!(running.panels[&target_panel].resync_needed_seq, 0);
        }
    }
}

#[tokio::test]
async fn interleaved_resync_host_call_error_keeps_budget_and_resyncs_latest() {
    let (mut running, mut calls, mut updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
    let snap = |rev: u64| PanelSnapshot {
        revision: rev,
        title: format!("rev-{rev}"),
        root: Some(panel::Node::new(
            1,
            "ed",
            panel::node::Kind::Editor(panel::Editor {
                document_id: 1,
                revision: rev,
                contents: format!("body-{rev}").into_bytes(),
                format: panel::EditorFormat::Utf8 as i32,
                read_only: false,
            }),
        )),
    };

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
    let mut open_snap = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(snap(1)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    open_snap.in_reply_to = ow.request_id;
    running.frame(open_snap, &outgoing).await.unwrap();
    assert!(open_rx.await.is_ok());

    let mut in_seq = 2u64;
    for target_panel in [2u64, protocol::PANEL_ID] {
        running
            .control(
                Control::Event {
                    panel_id: target_panel,
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
        let ev_wire = wire.try_recv().unwrap();
        running.schedule_panel_resync(target_panel);
        running.flush_pending_resyncs(&outgoing).unwrap();
        let resync1 = wire.try_recv().unwrap();
        assert!(matches!(resync1.body, Some(envelope::Body::Resync(ref s)) if s.revision == 1));
        let seq = if target_panel == protocol::PANEL_ID {
            running.main_slot().resync_needed_seq
        } else {
            running.panels[&target_panel].resync_needed_seq
        };
        assert_ne!(seq, 0);

        // Earlier in-flight UiEvent completes and advances host revision from 1 -> 2.
        let mut snap2 = Envelope::new(
            running.identity,
            in_seq,
            2,
            envelope::Body::Snapshot(snap(2)),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        in_seq += 1;
        snap2.in_reply_to = ev_wire.request_id;
        running.frame(snap2, &outgoing).await.unwrap();
        assert_eq!(running.panel_revision(target_panel), 2);
        while updates.try_recv().is_ok() {}

        // Inside on_panel_resync(1), SDK calls QueryHistory at published=1 referencing in-flight resync1.
        let hc1 = HostCall {
            operation: Some(host_call::Operation::QueryHistory(protocol::HistoryQuery {
                offset: 0,
                limit: 1,
                filter: String::new(),
            })),
            parent_request_id: resync1.request_id,
        };
        running
            .frame(
                Envelope::new(
                    running.identity,
                    in_seq,
                    1,
                    envelope::Body::HostCall(hc1),
                    MAX_OPERATION,
                )
                .with_panel_id(target_panel),
                &outgoing,
            )
            .await
            .unwrap();
        in_seq += 1;
        assert!(calls.try_recv().is_err());
        let stale_hr = wire.try_recv().unwrap();
        assert!(matches!(
            stale_hr.body,
            Some(envelope::Body::HostResult(HostResult {
                outcome: Some(host_result::Outcome::Error(Failure { code, .. })),
            })) if code == ErrorCode::Stale as i32
        ));
        assert!(wire.try_recv().is_err());

        // SDK propagates that HostCall error as Error(Invalid, rev=1) for superseded resync1!
        let mut err1 = Envelope::new(
            running.identity,
            in_seq,
            1,
            envelope::Body::Error(Failure::new(
                ErrorCode::Invalid,
                "服务调用来自旧面板 revision",
            )),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        in_seq += 1;
        err1.in_reply_to = resync1.request_id;
        running.frame(err1, &outgoing).await.unwrap();

        // Must preserve recovery cycle, keep budget intact, NOT mark incompatible, and immediately send Resync(2)!
        let resync2 = wire.try_recv().unwrap();
        assert!(matches!(resync2.body, Some(envelope::Body::Resync(ref s)) if s.revision == 2));
        if target_panel == protocol::PANEL_ID {
            assert_eq!(running.main_slot().resync_needed_seq, seq);
            assert_eq!(running.main_slot().resync_attempts, 2);
        } else {
            assert_eq!(running.panels[&target_panel].resync_needed_seq, seq);
            assert_eq!(running.panels[&target_panel].resync_attempts, 2);
        }

        // Now inside on_panel_resync(2), HostCall at rev=2 referencing resync2 succeeds and Ack(2) completes recovery!
        let hc2 = HostCall {
            operation: Some(host_call::Operation::QueryHistory(protocol::HistoryQuery {
                offset: 0,
                limit: 1,
                filter: String::new(),
            })),
            parent_request_id: resync2.request_id,
        };
        running
            .frame(
                Envelope::new(
                    running.identity,
                    in_seq,
                    2,
                    envelope::Body::HostCall(hc2),
                    MAX_OPERATION,
                )
                .with_panel_id(target_panel),
                &outgoing,
            )
            .await
            .unwrap();
        in_seq += 1;
        assert_eq!(calls.try_recv().unwrap().panel_id, target_panel);

        let mut ack2 = Envelope::new(
            running.identity,
            in_seq,
            2,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        in_seq += 1;
        ack2.in_reply_to = resync2.request_id;
        running.frame(ack2, &outgoing).await.unwrap();
        if target_panel == protocol::PANEL_ID {
            assert_eq!(running.main_slot().resync_needed_seq, 0);
            assert_eq!(running.main_slot().resync_attempts, 0);
        } else {
            assert_eq!(running.panels[&target_panel].resync_needed_seq, 0);
            assert_eq!(running.panels[&target_panel].resync_attempts, 0);
        }
    }
}

#[tokio::test]
async fn child_non_destroy_close_timer_timeout_resyncs_and_recovers_late_snapshot() {
    let (mut running, _calls, _updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
    let primary = Uuid::new_v4();
    let extra = Uuid::new_v4();
    let make_snap = |rev: u64| PanelSnapshot {
        revision: rev,
        title: format!("child-{rev}"),
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
    };

    let (open_tx, open_rx) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: 2,
                id: primary,
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
    let mut open_snap = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(make_snap(1)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    open_snap.in_reply_to = ow.request_id;
    running.frame(open_snap, &outgoing).await.unwrap();
    assert!(open_rx.await.is_ok());

    running.broker.bind_panel_resource(2, extra, 1, false).unwrap();
    let (rev_tx, mut rev_rx) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: 2,
                id: extra,
                remove_panel: false,
                reply: rev_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let close_wire = wire.try_recv().unwrap();
    assert!(matches!(
        close_wire.body,
        Some(envelope::Body::CloseRequest(ref c)) if !c.destroy_panel
    ));

    // Timer fires FIRST on CloseRequest(destroy_panel=false): cancels close, records allowlist id, emits Cancel + Resync(1).
    running.requests.get_mut(&close_wire.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
    assert!(rev_rx.try_recv().unwrap().is_err());
    assert!(
        running.panels[&2]
            .cancelled_state_requests
            .iter()
            .any(|&(id, _)| id == close_wire.request_id)
    );
    let cancel_msg = wire.try_recv().unwrap();
    assert!(matches!(cancel_msg.body, Some(envelope::Body::Cancel(_))));
    let resync1 = wire.try_recv().unwrap();
    assert!(matches!(resync1.body, Some(envelope::Body::Resync(_))));

    let mut resync1_ack =
        Envelope::new(running.identity, 2, 1, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION)
            .with_panel_id(2);
    resync1_ack.in_reply_to = resync1.request_id;
    running.frame(resync1_ack, &outgoing).await.unwrap();
    assert_eq!(running.panels[&2].resync_needed_seq, 0);
    assert!(wire.try_recv().is_err());

    // SDK's on_panel_close_request finally finishes and publishes Snapshot(2) after timer recovery already settled!
    let mut late_close_snap = Envelope::new(
        running.identity,
        3,
        2,
        envelope::Body::Snapshot(make_snap(2)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    late_close_snap.in_reply_to = close_wire.request_id;
    running.frame(late_close_snap, &outgoing).await.unwrap();

    let resync2 = wire.try_recv().unwrap();
    assert!(matches!(resync2.body, Some(envelope::Body::Resync(ref s)) if s.revision == 1));
    assert!(
        !running.panels[&2]
            .cancelled_state_requests
            .iter()
            .any(|&(id, _)| id == close_wire.request_id)
    );
    let mut resync2_ack =
        Envelope::new(running.identity, 4, 1, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION)
            .with_panel_id(2);
    resync2_ack.in_reply_to = resync2.request_id;
    running.frame(resync2_ack, &outgoing).await.unwrap();

    // Replay of the same close id must not trigger another Resync.
    let mut replay_close_snap = Envelope::new(
        running.identity,
        5,
        2,
        envelope::Body::Snapshot(make_snap(2)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    replay_close_snap.in_reply_to = close_wire.request_id;
    running.frame(replay_close_snap, &outgoing).await.unwrap();
    assert!(wire.try_recv().is_err());
}

#[tokio::test]
async fn interleaved_resync_snapshot_reply_does_not_overwrite_newer_confirmed_panel() {
    let (mut running, _calls, mut updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
    let make_snap = |rev: u64, status: &str| PanelSnapshot {
        revision: rev,
        title: format!("panel-{rev}"),
        root: Some(panel::Node::new(
            3,
            "layout",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        1,
                        "ed",
                        panel::node::Kind::Editor(panel::Editor {
                            document_id: 1,
                            revision: 1,
                            contents: b"init".to_vec(),
                            format: panel::EditorFormat::Utf8 as i32,
                            read_only: false,
                        }),
                    ),
                    panel::Node::new(
                        2,
                        "status",
                        panel::node::Kind::Text(panel::Text {
                            text: status.into(),
                            tone: 0,
                        }),
                    ),
                ],
            }),
        )),
    };
    *running.main_panel_mut() = PanelState::new(make_snap(1, "v1-old")).unwrap();

    // Prepare child panel 2 at rev=1 (keeping existing main panel at rev=1 too).
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
    let mut open_snap = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(make_snap(1, "v1-old")),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    open_snap.in_reply_to = ow.request_id;
    running.frame(open_snap, &outgoing).await.unwrap();
    assert!(open_rx.await.is_ok());

    let mut in_seq = 2u64;
    for target_panel in [2u64, protocol::PANEL_ID] {
        running
            .control(
                Control::Event {
                    panel_id: target_panel,
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
        let ev_wire = wire.try_recv().unwrap();
        running.schedule_panel_resync(target_panel);
        running.flush_pending_resyncs(&outgoing).unwrap();
        let resync1 = wire.try_recv().unwrap();
        assert!(matches!(resync1.body, Some(envelope::Body::Resync(ref s)) if s.revision == 1));
        let seq = if target_panel == protocol::PANEL_ID {
            running.main_slot().resync_needed_seq
        } else {
            running.panels[&target_panel].resync_needed_seq
        };
        assert_ne!(seq, 0);

        // Earlier in-flight UiEvent completes and advances confirmed panel to rev=2 with "v2-confirmed".
        let mut snap2 = Envelope::new(
            running.identity,
            in_seq,
            2,
            envelope::Body::Snapshot(make_snap(2, "v2-confirmed")),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        in_seq += 1;
        snap2.in_reply_to = ev_wire.request_id;
        running.frame(snap2, &outgoing).await.unwrap();
        assert_eq!(running.panel_revision(target_panel), 2);
        while updates.try_recv().is_ok() {}

        // Callback handling stale Resync(1) rebuilds state from rev=1 and replies with higher rev=3 full Snapshot!
        // Because resync_revision(1) != current host_rev(2), host MUST reject overwriting "v2-confirmed" and send Resync(2).
        let mut stale_resync_snap3 = Envelope::new(
            running.identity,
            in_seq,
            3,
            envelope::Body::Snapshot(make_snap(3, "v1-rebuilt-stale")),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        in_seq += 1;
        stale_resync_snap3.in_reply_to = resync1.request_id;
        running.frame(stale_resync_snap3, &outgoing).await.unwrap();

        assert_eq!(running.panel_revision(target_panel), 2);
        let retained_snap = running.panel_snapshot(target_panel).unwrap();
        let status_node = retained_snap.root.as_ref().unwrap().find(2).unwrap();
        assert!(matches!(
            status_node.kind,
            Some(panel::node::Kind::Text(ref t)) if t.text == "v2-confirmed"
        ));

        let resync2 = wire.try_recv().unwrap();
        assert!(matches!(
            resync2.body,
            Some(envelope::Body::Resync(ref s))
                if s.revision == 2
                    && matches!(
                        s.root.as_ref().and_then(|r| r.find(2)).and_then(|n| n.kind.as_ref()),
                        Some(panel::node::Kind::Text(t)) if t.text == "v2-confirmed"
                    )
        ));
        if target_panel == protocol::PANEL_ID {
            assert_eq!(running.main_slot().resync_needed_seq, seq);
            assert_eq!(running.main_slot().resync_attempts, 2);
        } else {
            assert_eq!(running.panels[&target_panel].resync_needed_seq, seq);
            assert_eq!(running.panels[&target_panel].resync_attempts, 2);
        }

        // Reply to Resync(2) with full Snapshot(4) succeeds and settles recovery cleanly.
        let mut ok_resync_snap4 = Envelope::new(
            running.identity,
            in_seq,
            4,
            envelope::Body::Snapshot(make_snap(4, "v2-resynced")),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        in_seq += 1;
        ok_resync_snap4.in_reply_to = resync2.request_id;
        running.frame(ok_resync_snap4, &outgoing).await.unwrap();
        assert_eq!(running.panel_revision(target_panel), 4);
        if target_panel == protocol::PANEL_ID {
            assert_eq!(running.main_slot().resync_needed_seq, 0);
            assert_eq!(running.main_slot().resync_attempts, 0);
        } else {
            assert_eq!(running.panels[&target_panel].resync_needed_seq, 0);
            assert_eq!(running.panels[&target_panel].resync_attempts, 0);
        }
    }
}
