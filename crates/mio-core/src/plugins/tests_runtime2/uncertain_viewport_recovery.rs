use super::*;

#[tokio::test]
async fn resync_bare_ack_settles_uncertain_viewport_for_ui_and_commit() {
    let make_snap = |rev: u64, vp_rev: u64| PanelSnapshot {
        revision: rev,
        title: "resync-ack-settles-uncertain-vp".into(),
        root: Some(panel::Node::new(
            1,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        2,
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
                        3,
                        "tbl",
                        panel::node::Kind::VirtualTable(panel::VirtualTable {
                            columns: vec![panel::TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 80.0,
                            }],
                            total_rows: 100,
                            offset: 0,
                            viewport_revision: vp_rev,
                            rows: vec![],
                            multi_select: false,
                        }),
                    ),
                ],
            }),
        )),
    };
    let read_vp = |snap: &PanelSnapshot| {
        match snap
            .root
            .as_ref()
            .unwrap()
            .find(3)
            .unwrap()
            .kind
            .as_ref()
            .unwrap()
        {
            panel::node::Kind::VirtualTable(t) => t.viewport_revision,
            other => panic!("expected table, got {other:?}"),
        }
    };

    let (mut running, _calls, mut updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
    let child_res = Uuid::new_v4();
    let (o_tx, o_rx) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: 2,
                id: child_res,
                revision: 1,
                read_only: false,
                request: request(),
                reply: o_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let o_w = wire.try_recv().unwrap();
    let mut o_s = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(make_snap(1, 1)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    o_s.in_reply_to = o_w.request_id;
    running.frame(o_s, &outgoing).await.unwrap();
    o_rx.await.unwrap().unwrap();
    while updates.try_recv().is_ok() {}

    // Claimed Viewport(2) times out -> Host marks it uncertain and emits Resync(vp=2).
    running
        .control(
            Control::Event {
                panel_id: 2,
                expected_snapshot: None,
                event_seq: 1,
                event: UiEvent {
                    node_id: 3,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                        revision: 2,
                        offset: 50,
                        limit: 50,
                    })),
                },
            },
            &outgoing,
        )
        .await
        .unwrap();
    let vp_w = wire.try_recv().unwrap();
    running.requests.get_mut(&vp_w.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
    let _cancel_w = wire.try_recv().unwrap();
    let resync_w = wire.try_recv().unwrap();
    assert!(matches!(
        resync_w.body,
        Some(envelope::Body::Resync(ref s)) if read_vp(s) == 2
    ));
    while updates.try_recv().is_ok() {}

    // Worker adopts Resync(vp=2) and replies with bare Ack -> Host settles uncertain vp=2,
    // emits PanelSynced with vp=2, and clears drift so CommitEditor(expected vp=2) succeeds!
    let mut resync_ack = Envelope::new(
        running.identity,
        2,
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    resync_ack.in_reply_to = resync_w.request_id;
    running.frame(resync_ack, &outgoing).await.unwrap();

    assert!(matches!(
        updates.try_recv(),
        Ok(PluginEvent::PanelSynced {
            panel_id: 2,
            synced_ui_seq: 1,
            ref snapshot,
            ..
        }) if read_vp(snapshot) == 2
    ));
    assert!(!running.is_panel_semantic_drifted(2));

    let (commit_tx, mut commit_rx) = oneshot::channel();
    running
        .control(
            Control::Commit {
                panel_id: 2,
                op_id: Uuid::new_v4(),
                request: CommitEditor {
                    resource_id: child_res.as_bytes().to_vec(),
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
                reply: commit_tx,
                expected_snapshot: Some(Arc::new(make_snap(1, 2))),
            },
            &outgoing,
        )
        .await
        .unwrap();
    let commit_w = wire.try_recv().unwrap();
    assert!(matches!(
        commit_w.body,
        Some(envelope::Body::CommitEditor(_))
    ));
    assert!(commit_rx.try_recv().is_err());
}

#[tokio::test]
async fn aborted_resync_after_uncertain_select_timeout_latches_drift_and_blocks_commit() {
    let make_snap = |rev: u64, selected: u32| PanelSnapshot {
        revision: rev,
        title: "uncertain-select-abort-drift".into(),
        root: Some(panel::Node::new(
            1,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        2,
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
                        3,
                        "select",
                        panel::node::Kind::Select(panel::SelectNode {
                            label: "Choice".into(),
                            options: vec!["A".into(), "B".into()],
                            selected_index: selected,
                            enabled: true,
                        }),
                    ),
                ],
            }),
        )),
    };

    let (mut running, _calls, mut updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
    let child_res = Uuid::new_v4();
    let (o_tx, o_rx) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: 2,
                id: child_res,
                revision: 1,
                read_only: false,
                request: request(),
                reply: o_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let o_w = wire.try_recv().unwrap();
    let mut o_s = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(make_snap(1, 0)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    o_s.in_reply_to = o_w.request_id;
    running.frame(o_s, &outgoing).await.unwrap();
    o_rx.await.unwrap().unwrap();
    while updates.try_recv().is_ok() {}

    // Claimed Select(1 -> "B") times out; Worker fails Resync with same-revision Error(Invalid).
    running
        .control(
            Control::Event {
                panel_id: 2,
                expected_snapshot: None,
                event_seq: 1,
                event: UiEvent {
                    node_id: 3,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::SelectedOption(1)),
                },
            },
            &outgoing,
        )
        .await
        .unwrap();
    let sel_w = wire.try_recv().unwrap();
    running.requests.get_mut(&sel_w.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
    let _cancel_w = wire.try_recv().unwrap();
    let resync_w = wire.try_recv().unwrap();
    while updates.try_recv().is_ok() {}

    let mut resync_err = Envelope::new(
        running.identity,
        2,
        1,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "resync unsupported")),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    resync_err.in_reply_to = resync_w.request_id;
    running.frame(resync_err, &outgoing).await.unwrap();

    // Retracting uncertain Select(1 -> 0) MUST latch semantic drift and block same-revision CommitEditor!
    assert!(running.is_panel_semantic_drifted(2));
    let (commit_tx, mut commit_rx) = oneshot::channel();
    running
        .control(
            Control::Commit {
                panel_id: 2,
                op_id: Uuid::new_v4(),
                request: CommitEditor {
                    resource_id: child_res.as_bytes().to_vec(),
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
                reply: commit_tx,
                expected_snapshot: Some(Arc::new(make_snap(1, 0))),
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(commit_rx.try_recv().unwrap().is_err());

    // Authoritative full Snapshot(rev=2, Select=1) clears drift and reconciles state.
    let snap2 = Envelope::new(
        running.identity,
        3,
        2,
        envelope::Body::Snapshot(make_snap(2, 1)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    running.frame(snap2, &outgoing).await.unwrap();
    assert!(!running.is_panel_semantic_drifted(2));
}

#[tokio::test]
async fn unsolicited_base_snapshot_accepts_and_preserves_unclaimed_viewport_in_writes() {
    let make_snap = |rev: u64, vp_rev: u64| PanelSnapshot {
        revision: rev,
        title: "unsolicited-base-unclaimed-vp".into(),
        root: Some(panel::Node::new(
            1,
            "tbl",
            panel::node::Kind::VirtualTable(panel::VirtualTable {
                columns: vec![panel::TableColumn {
                    id: "c1".into(),
                    title: "Col".into(),
                    width: 80.0,
                }],
                total_rows: 100,
                offset: 0,
                viewport_revision: vp_rev,
                rows: vec![],
                multi_select: false,
            }),
        )),
    };
    let read_vp = |snap: &PanelSnapshot| match snap.root.as_ref().unwrap().kind.as_ref().unwrap() {
        panel::node::Kind::VirtualTable(t) => t.viewport_revision,
        other => panic!("expected table, got {other:?}"),
    };

    let (mut running, _calls, mut updates) = running();
    let claims = Arc::new(OutboundWriteClaims::default());
    running.outbound_claims = Some(claims.clone());
    *running.main_panel_mut() = PanelState::new(make_snap(1, 1)).unwrap();
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

    // Enqueue Viewport(2) with seq=1 leaving it UNCLAIMED in `claims` (`writes`).
    running
        .control(
            Control::Event {
                panel_id: protocol::PANEL_ID,
                expected_snapshot: None,
                event_seq: 1,
                event: UiEvent {
                    node_id: 1,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                        revision: 2,
                        offset: 50,
                        limit: 50,
                    })),
                },
            },
            &outgoing,
        )
        .await
        .unwrap();
    let vp_w = wire.try_recv().unwrap();
    while updates.try_recv().is_ok() {}

    // Worker sends an independent unsolicited Snapshot(rev=2, vp=1, in_reply_to=0).
    let unsolicited = Envelope::new(
        running.identity,
        1,
        2,
        envelope::Body::Snapshot(make_snap(2, 1)),
        MAX_OPERATION,
    );
    running.frame(unsolicited, &outgoing).await.unwrap();

    // Host MUST accept rev=2 without drift/resync, keep Viewport(2) claim alive, and publish rev=2
    // with synced_ui_seq=0 and confirmed table vp=1.
    assert_eq!(running.panel_revision(protocol::PANEL_ID), 2);
    assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
    assert!(!running.has_inflight_resync(protocol::PANEL_ID));
    assert!(matches!(
        updates.try_recv(),
        Ok(PluginEvent::Panel {
            panel_id: protocol::PANEL_ID,
            synced_ui_seq: 0,
            ref snapshot,
            ..
        }) if snapshot.revision == 2 && read_vp(snapshot) == 1
    ));

    // Writer now claims Viewport(2) and Worker publishes Patch(Change::Table(vp=2)) -> Host settles vp=2 cleanly!
    assert!(claims.claim_for_write(protocol::PANEL_ID, vp_w.request_id));
    let mut vp_patch = Envelope::new(
        running.identity,
        2,
        3,
        envelope::Body::Patch(panel::PanelPatch {
            base_revision: 2,
            revision: 3,
            operations: vec![panel::PatchOperation {
                node_id: 1,
                change: Some(panel::patch_operation::Change::Table(panel::VirtualTable {
                    columns: vec![panel::TableColumn {
                        id: "c1".into(),
                        title: "Col".into(),
                        width: 80.0,
                    }],
                    total_rows: 100,
                    offset: 50,
                    viewport_revision: 2,
                    rows: vec![],
                    multi_select: false,
                })),
            }],
        }),
        MAX_OPERATION,
    );
    vp_patch.in_reply_to = vp_w.request_id;
    running.frame(vp_patch, &outgoing).await.unwrap();
    assert!(matches!(
        updates.try_recv(),
        Ok(PluginEvent::Panel {
            panel_id: protocol::PANEL_ID,
            synced_ui_seq: 1,
            ref snapshot,
            ..
        }) if snapshot.revision == 3 && read_vp(snapshot) == 2
    ));
}

#[tokio::test]
async fn resync_partial_patch_settles_uncertain_viewport_for_ui_and_commit() {
    let make_snap = |rev: u64, vp_rev: u64, status: &str| PanelSnapshot {
        revision: rev,
        title: "resync-patch-settles-uncertain-vp".into(),
        root: Some(panel::Node::new(
            1,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        2,
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
                        3,
                        "tbl",
                        panel::node::Kind::VirtualTable(panel::VirtualTable {
                            columns: vec![panel::TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 80.0,
                            }],
                            total_rows: 100,
                            offset: 0,
                            viewport_revision: vp_rev,
                            rows: vec![],
                            multi_select: false,
                        }),
                    ),
                    panel::Node::new(
                        4,
                        "txt",
                        panel::node::Kind::Text(panel::Text {
                            text: status.into(),
                            tone: panel::Tone::Foreground as i32,
                        }),
                    ),
                ],
            }),
        )),
    };
    let read_vp = |snap: &PanelSnapshot| {
        match snap
            .root
            .as_ref()
            .unwrap()
            .find(3)
            .unwrap()
            .kind
            .as_ref()
            .unwrap()
        {
            panel::node::Kind::VirtualTable(t) => t.viewport_revision,
            other => panic!("expected table, got {other:?}"),
        }
    };

    let (mut running, _calls, mut updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
    let child_res = Uuid::new_v4();
    let (o_tx, o_rx) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: 2,
                id: child_res,
                revision: 1,
                read_only: false,
                request: request(),
                reply: o_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let o_w = wire.try_recv().unwrap();
    let mut o_s = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(make_snap(1, 1, "v1")),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    o_s.in_reply_to = o_w.request_id;
    running.frame(o_s, &outgoing).await.unwrap();
    o_rx.await.unwrap().unwrap();
    while updates.try_recv().is_ok() {}

    // Claimed Viewport(2) times out -> Host marks uncertain and sends Resync(vp=2).
    running
        .control(
            Control::Event {
                panel_id: 2,
                expected_snapshot: None,
                event_seq: 1,
                event: UiEvent {
                    node_id: 3,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                        revision: 2,
                        offset: 50,
                        limit: 50,
                    })),
                },
            },
            &outgoing,
        )
        .await
        .unwrap();
    let vp_w = wire.try_recv().unwrap();
    running.requests.get_mut(&vp_w.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
    let _cancel_w = wire.try_recv().unwrap();
    let resync_w = wire.try_recv().unwrap();
    while updates.try_recv().is_ok() {}

    // Resync completes with a partial Patch that updates only sibling Text(4), without touching Table(3).
    let mut resync_patch = Envelope::new(
        running.identity,
        2,
        2,
        envelope::Body::Patch(panel::PanelPatch {
            base_revision: 1,
            revision: 2,
            operations: vec![panel::PatchOperation {
                node_id: 4,
                change: Some(panel::patch_operation::Change::Replace(panel::Node::new(
                    4,
                    "txt",
                    panel::node::Kind::Text(panel::Text {
                        text: "v2".into(),
                        tone: panel::Tone::Foreground as i32,
                    }),
                ))),
            }],
        }),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    resync_patch.in_reply_to = resync_w.request_id;
    running.frame(resync_patch, &outgoing).await.unwrap();

    assert!(matches!(
        updates.try_recv(),
        Ok(PluginEvent::Panel {
            panel_id: 2,
            synced_ui_seq: 1,
            ref snapshot,
            ..
        }) if snapshot.revision == 2 && read_vp(snapshot) == 2
    ));
    assert!(!running.is_panel_semantic_drifted(2));
    assert!(!running.is_panel_resyncing(2));

    let (commit_tx, mut commit_rx) = oneshot::channel();
    running
        .control(
            Control::Commit {
                panel_id: 2,
                op_id: Uuid::new_v4(),
                request: CommitEditor {
                    resource_id: child_res.as_bytes().to_vec(),
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
                reply: commit_tx,
                expected_snapshot: Some(Arc::new(make_snap(2, 2, "v2"))),
            },
            &outgoing,
        )
        .await
        .unwrap();
    let commit_w = wire.try_recv().unwrap();
    assert!(matches!(
        commit_w.body,
        Some(envelope::Body::CommitEditor(_))
    ));
    assert!(commit_rx.try_recv().is_err());
}

#[tokio::test]
async fn unsolicited_base_snapshot_accepts_while_claimed_viewport_in_pipe_and_retracts_shrunk_offset() {
    let make_snap = |rev: u64, vp_rev: u64, total_rows: u64| PanelSnapshot {
        revision: rev,
        title: "claimed-pipe-vs-shrunk".into(),
        root: Some(panel::Node::new(
            1,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        2,
                        "txt",
                        panel::node::Kind::Text(panel::Text {
                            text: "ok".into(),
                            tone: panel::Tone::Foreground as i32,
                        }),
                    ),
                    panel::Node::new(
                        3,
                        "tbl",
                        panel::node::Kind::VirtualTable(panel::VirtualTable {
                            columns: vec![panel::TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 80.0,
                            }],
                            total_rows,
                            offset: 0,
                            viewport_revision: vp_rev,
                            rows: vec![],
                            multi_select: false,
                        }),
                    ),
                ],
            }),
        )),
    };
    let read_vp = |snap: &PanelSnapshot| {
        match snap
            .root
            .as_ref()
            .unwrap()
            .find(3)
            .unwrap()
            .kind
            .as_ref()
            .unwrap()
        {
            panel::node::Kind::VirtualTable(t) => t.viewport_revision,
            other => panic!("expected table, got {other:?}"),
        }
    };

    // Part A: Viewport(2) IS CLAIMED (write blocked in pipe before Worker read it); Worker sends
    // unsolicited Snapshot(rev=2, vp=1, in_reply_to=0). Host MUST accept rev=2 cleanly with NO drift,
    // drop the claimed ledger entry so unsolicited state is not overstated, and accept vp_w's real
    // Snapshot(rev=3, vp=2) reply when it completes!
    {
        let (mut running, _calls, mut updates) = running();
        let claims = Arc::new(OutboundWriteClaims::default());
        running.outbound_claims = Some(claims.clone());
        *running.main_panel_mut() = PanelState::new(make_snap(1, 1, 100)).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                            revision: 2,
                            offset: 50,
                            limit: 50,
                        })),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let vp_w = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(protocol::PANEL_ID, vp_w.request_id));
        while updates.try_recv().is_ok() {}

        let unsolicited = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, 100)),
            MAX_OPERATION,
        );
        running.frame(unsolicited, &outgoing).await.unwrap();

        assert_eq!(running.panel_revision(protocol::PANEL_ID), 2);
        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        assert!(!running.has_panel_optimistic_request(protocol::PANEL_ID, vp_w.request_id));
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 0,
                ref snapshot,
                ..
            }) if snapshot.revision == 2 && read_vp(snapshot) == 1
        ));

        let mut vp_snap = Envelope::new(
            running.identity,
            2,
            3,
            envelope::Body::Snapshot(make_snap(3, 2, 100)),
            MAX_OPERATION,
        );
        vp_snap.in_reply_to = vp_w.request_id;
        running.frame(vp_snap, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 1,
                ref snapshot,
                ..
            }) if snapshot.revision == 3 && read_vp(snapshot) == 2
        ));
    }

    // Part B: Unclaimed Viewport(2, offset=50) in writes + in-flight Focus(seq=1); unsolicited Snapshot(rev=2,
    // vp=1, total_rows=20) MUST accept rev=2, retract out-of-range Viewport(2) atomically, and emit
    // PluginEvent::Panel with rejected_ui_seqs == [2]!
    {
        let (mut running, _calls, mut updates) = running();
        let claims = Arc::new(OutboundWriteClaims::default());
        running.outbound_claims = Some(claims.clone());
        *running.main_panel_mut() = PanelState::new(make_snap(1, 1, 100)).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 2,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Focus(true)),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let _focus_w = wire.try_recv().unwrap();

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 2,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                            revision: 2,
                            offset: 50,
                            limit: 50,
                        })),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let vp_w = wire.try_recv().unwrap();
        while updates.try_recv().is_ok() {}

        let unsolicited = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, 20)),
            MAX_OPERATION,
        );
        running.frame(unsolicited, &outgoing).await.unwrap();

        assert!(!claims.claim_for_write(protocol::PANEL_ID, vp_w.request_id));
        assert!(!running.requests.contains_key(&vp_w.request_id));
        assert_eq!(running.panel_revision(protocol::PANEL_ID), 2);
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Error { failure, .. }) if failure.code == ErrorCode::Stale as i32
        ));
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 0,
                ref rejected_ui_seqs,
                ref snapshot,
                ..
            }) if snapshot.revision == 2 && read_vp(snapshot) == 1 && rejected_ui_seqs == &[2]
        ));
    }

    // Part C: Claimed Viewport(2) detached by unsolicited Snapshot(rev=2, vp=1) then finishes with a
    // causal Change::Table(vp=2) Patch (instead of full Snapshot); Host MUST accept it with NO drift!
    {
        let (mut running, _calls, mut updates) = running();
        let claims = Arc::new(OutboundWriteClaims::default());
        running.outbound_claims = Some(claims.clone());
        *running.main_panel_mut() = PanelState::new(make_snap(1, 1, 100)).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                            revision: 2,
                            offset: 50,
                            limit: 50,
                        })),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let vp_w = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(protocol::PANEL_ID, vp_w.request_id));
        while updates.try_recv().is_ok() {}

        let unsolicited = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, 100)),
            MAX_OPERATION,
        );
        running.frame(unsolicited, &outgoing).await.unwrap();
        assert!(!running.has_panel_optimistic_request(protocol::PANEL_ID, vp_w.request_id));
        while updates.try_recv().is_ok() {}

        let mut vp_patch = Envelope::new(
            running.identity,
            2,
            3,
            envelope::Body::Patch(panel::PanelPatch {
                base_revision: 2,
                revision: 3,
                operations: vec![panel::PatchOperation {
                    node_id: 3,
                    change: Some(panel::patch_operation::Change::Table(panel::VirtualTable {
                        columns: vec![panel::TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 80.0,
                        }],
                        total_rows: 100,
                        offset: 50,
                        viewport_revision: 2,
                        rows: vec![],
                        multi_select: false,
                    })),
                }],
            }),
            MAX_OPERATION,
        );
        vp_patch.in_reply_to = vp_w.request_id;
        running.frame(vp_patch, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 1,
                ref snapshot,
                ..
            }) if snapshot.revision == 3 && read_vp(snapshot) == 2
        ));
    }
}

#[tokio::test]
async fn unsolicited_snapshot_before_resync_abort_preserves_uncertain_select_drift_guard() {
    let make_snap = |rev: u64, selected: u32| PanelSnapshot {
        revision: rev,
        title: "uncertain-select-after-unsolicited-snap".into(),
        root: Some(panel::Node::new(
            1,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        2,
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
                        3,
                        "select",
                        panel::node::Kind::Select(panel::SelectNode {
                            label: "Choice".into(),
                            options: vec!["A".into(), "B".into()],
                            selected_index: selected,
                            enabled: true,
                        }),
                    ),
                ],
            }),
        )),
    };

    let (mut running, _calls, mut updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
    let child_res = Uuid::new_v4();
    let (o_tx, o_rx) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: 2,
                id: child_res,
                revision: 1,
                read_only: false,
                request: request(),
                reply: o_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let o_w = wire.try_recv().unwrap();
    let mut o_s = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(make_snap(1, 0)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    o_s.in_reply_to = o_w.request_id;
    running.frame(o_s, &outgoing).await.unwrap();
    o_rx.await.unwrap().unwrap();
    while updates.try_recv().is_ok() {}

    // Claimed Select(1 -> "B") times out -> Host emits Resync #1.
    running
        .control(
            Control::Event {
                panel_id: 2,
                expected_snapshot: None,
                event_seq: 1,
                event: UiEvent {
                    node_id: 3,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::SelectedOption(1)),
                },
            },
            &outgoing,
        )
        .await
        .unwrap();
    let sel_w = wire.try_recv().unwrap();
    running.requests.get_mut(&sel_w.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
    let _cancel_w = wire.try_recv().unwrap();
    let resync_1 = wire.try_recv().unwrap();

    // Unsolicited Snapshot(rev=2, Select=0 "A", in_reply_to=0) arrives before Resync #1 finishes!
    let unsolicited = Envelope::new(
        running.identity,
        2,
        2,
        envelope::Body::Snapshot(make_snap(2, 0)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    running.frame(unsolicited, &outgoing).await.unwrap();

    // Superseded Resync #1 fails -> Host schedules & dispatches Resync #2 on rev=2.
    let mut resync_err1 = Envelope::new(
        running.identity,
        3,
        1,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "resync unsupported")),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    resync_err1.in_reply_to = resync_1.request_id;
    running.frame(resync_err1, &outgoing).await.unwrap();
    let resync_2 = wire.try_recv().unwrap();
    assert!(matches!(resync_2.body, Some(envelope::Body::Resync(_))));

    // Resync #2 fails with Error(Invalid) -> abort_panel_resync MUST STILL see uncertain_optimistic_ids
    // and latch semantic drift, blocking CommitEditor!
    let mut resync_err2 = Envelope::new(
        running.identity,
        4,
        2,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "resync unsupported")),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    resync_err2.in_reply_to = resync_2.request_id;
    running.frame(resync_err2, &outgoing).await.unwrap();

    assert!(running.is_panel_semantic_drifted(2));
    let (commit_tx, mut commit_rx) = oneshot::channel();
    running
        .control(
            Control::Commit {
                panel_id: 2,
                op_id: Uuid::new_v4(),
                request: CommitEditor {
                    resource_id: child_res.as_bytes().to_vec(),
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
                reply: commit_tx,
                expected_snapshot: Some(Arc::new(make_snap(2, 0))),
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(commit_rx.try_recv().unwrap().is_err());

    // Authoritative causal full Snapshot(rev=3, Select=1) clears uncertain state and drift!
    let snap3 = Envelope::new(
        running.identity,
        5,
        3,
        envelope::Body::Snapshot(make_snap(3, 1)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    running.frame(snap3, &outgoing).await.unwrap();
    assert!(!running.is_panel_semantic_drifted(2));
}

#[tokio::test]
async fn detached_viewport_deadline_and_late_uncertain_deadline_clear_tracking_and_emit_panel_synced()
{
    let make_table_snap = |rev: u64, vp_rev: u64, offset: u64, total_rows: u64| PanelSnapshot {
        revision: rev,
        title: "detached-deadline-sync".into(),
        root: Some(panel::Node::new(
            1,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        2,
                        "inp",
                        panel::node::Kind::Text(panel::Text {
                            text: "ok".into(),
                            tone: panel::Tone::Foreground as i32,
                        }),
                    ),
                    panel::Node::new(
                        3,
                        "tbl",
                        panel::node::Kind::VirtualTable(panel::VirtualTable {
                            columns: vec![panel::TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 80.0,
                            }],
                            total_rows,
                            offset,
                            viewport_revision: vp_rev,
                            rows: vec![],
                            multi_select: false,
                        }),
                    ),
                ],
            }),
        )),
    };
    let read_vp = |snap: &PanelSnapshot| match snap
        .root
        .as_ref()
        .unwrap()
        .find(3)
        .unwrap()
        .kind
        .as_ref()
        .unwrap()
    {
        panel::node::Kind::VirtualTable(t) => t.viewport_revision,
        other => panic!("expected table, got {other:?}"),
    };

    // Part A1: Claimed Viewport(2, seq=1) detached by unsolicited Snapshot(rev=2, vp=1) receives SDK
    // Error(Deadline) -> MUST clear tracked detached state and emit terminal PanelSynced(synced_ui_seq=1)
    // with NO drift or Resync.
    {
        let (mut running, _calls, mut updates) = running();
        *running.main_panel_mut() = PanelState::new(make_table_snap(1, 1, 0, 100)).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                            revision: 2,
                            offset: 50,
                            limit: 50,
                        })),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let vp_w = wire.try_recv().unwrap();
        while updates.try_recv().is_ok() {}

        let unsolicited = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_table_snap(2, 1, 0, 100)),
            MAX_OPERATION,
        );
        running.frame(unsolicited, &outgoing).await.unwrap();
        assert!(!running.has_panel_optimistic_request(protocol::PANEL_ID, vp_w.request_id));
        assert!(running.has_panel_tracked_optimistic_request(protocol::PANEL_ID, vp_w.request_id));
        while updates.try_recv().is_ok() {}

        let mut err = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Error(Failure::new(ErrorCode::Deadline, "pre-dispatch deadline")),
            MAX_OPERATION,
        );
        err.in_reply_to = vp_w.request_id;
        running.frame(err, &outgoing).await.unwrap();

        assert!(!running.has_panel_tracked_optimistic_request(protocol::PANEL_ID, vp_w.request_id));
        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        assert!(wire.try_recv().is_err());

        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Error { panel_id: protocol::PANEL_ID, failure, .. })
                if failure.code == ErrorCode::Deadline as i32
        ));
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::PanelSynced {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 1,
                ref rejected_ui_seqs,
                ref snapshot,
                ..
            }) if snapshot.revision == 2 && read_vp(snapshot) == 1 && rejected_ui_seqs == &[1]
        ));
    }

    // Part A2: When earlier Focus(seq=1) pins synced_ui_seq == 0, detached Viewport(2, seq=2) receiving
    // Error(Deadline) MUST emit PanelSynced { synced_ui_seq: 0, rejected_ui_seqs: [2] }.
    {
        let (mut running, _calls, mut updates) = running();
        *running.main_panel_mut() = PanelState::new(make_table_snap(1, 1, 0, 100)).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 2,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Focus(true)),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let _focus_w = wire.try_recv().unwrap();

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 2,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                            revision: 2,
                            offset: 50,
                            limit: 50,
                        })),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let vp_w = wire.try_recv().unwrap();

        let unsolicited = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_table_snap(2, 1, 0, 100)),
            MAX_OPERATION,
        );
        running.frame(unsolicited, &outgoing).await.unwrap();
        while updates.try_recv().is_ok() {}

        let mut err = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Error(Failure::new(ErrorCode::Deadline, "pre-dispatch deadline")),
            MAX_OPERATION,
        );
        err.in_reply_to = vp_w.request_id;
        running.frame(err, &outgoing).await.unwrap();

        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Error { panel_id: protocol::PANEL_ID, .. })
        ));
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::PanelSynced {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 0,
                ref rejected_ui_seqs,
                ..
            }) if rejected_ui_seqs == &[2]
        ));
    }

    // Part B: Timed-out claimed Select(1) drops from optimistic_fields via unsolicited Snapshot leaving
    // ONLY uncertain_optimistic_ids; late Error(Deadline) MUST clear uncertain_optimistic_ids and emit
    // PanelSynced so subsequent Resync abort does NOT falsely latch drift.
    {
        let make_sel_snap = |rev: u64, selected: u32| PanelSnapshot {
            revision: rev,
            title: "late-uncertain-deadline".into(),
            root: Some(panel::Node::new(
                1,
                "select",
                panel::node::Kind::Select(panel::SelectNode {
                    label: "Choice".into(),
                    options: vec!["A".into(), "B".into()],
                    selected_index: selected,
                    enabled: true,
                }),
            )),
        };

        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        let child_res = Uuid::new_v4();
        let (o_tx, o_rx) = oneshot::channel();
        running
            .control(
                Control::Open {
                    panel_id: 2,
                    id: child_res,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: o_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let o_w = wire.try_recv().unwrap();
        let mut o_s = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Snapshot(make_sel_snap(1, 0)),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        o_s.in_reply_to = o_w.request_id;
        running.frame(o_s, &outgoing).await.unwrap();
        o_rx.await.unwrap().unwrap();

        running
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 1,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::SelectedOption(1)),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let sel_w = wire.try_recv().unwrap();
        running.requests.get_mut(&sel_w.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let _cancel_w = wire.try_recv().unwrap();
        let resync_1 = wire.try_recv().unwrap();

        let unsolicited = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Snapshot(make_sel_snap(2, 0)),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(unsolicited, &outgoing).await.unwrap();
        assert!(!running.has_panel_optimistic_request(2, sel_w.request_id));
        assert!(running.panel_has_uncertain_optimistic(2));
        while updates.try_recv().is_ok() {}

        let mut late_deadline = Envelope::new(
            running.identity,
            3,
            2,
            envelope::Body::Error(Failure::new(ErrorCode::Deadline, "late deadline")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_deadline.in_reply_to = sel_w.request_id;
        running.frame(late_deadline, &outgoing).await.unwrap();

        assert!(!running.panel_has_uncertain_optimistic(2));
        assert!(!running.has_panel_tracked_optimistic_request(2, sel_w.request_id));
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Error { panel_id: 2, failure, .. })
                if failure.code == ErrorCode::Stale as i32
        ));
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::PanelSynced { panel_id: 2, synced_ui_seq: 1, .. })
        ));

        // Now Resync #1 fails as superseded -> Resync #2 fails with Error(Invalid); since uncertain set
        // was proven unexecuted and cleared, abort_panel_resync MUST NOT latch semantic drift!
        let mut resync_err1 = Envelope::new(
            running.identity,
            4,
            1,
            envelope::Body::Error(Failure::new(ErrorCode::Invalid, "resync unsupported")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        resync_err1.in_reply_to = resync_1.request_id;
        running.frame(resync_err1, &outgoing).await.unwrap();
        let resync_2 = wire.try_recv().unwrap();
        let mut resync_err2 = Envelope::new(
            running.identity,
            5,
            2,
            envelope::Body::Error(Failure::new(ErrorCode::Invalid, "resync unsupported")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        resync_err2.in_reply_to = resync_2.request_id;
        running.frame(resync_err2, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
    }
}

#[tokio::test]
async fn unsolicited_snapshot_covering_claimed_viewport_avoids_drift_and_settles_reply() {
    let make_snap = |rev: u64, vp_rev: u64, offset: u64, total_rows: u64, text: &str| PanelSnapshot {
        revision: rev,
        title: "unsolicited-vp-confirm".into(),
        root: Some(panel::Node::new(
            1,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        2,
                        "status",
                        panel::node::Kind::Text(panel::Text {
                            text: text.into(),
                            tone: panel::Tone::Foreground as i32,
                        }),
                    ),
                    panel::Node::new(
                        3,
                        "tbl",
                        panel::node::Kind::VirtualTable(panel::VirtualTable {
                            columns: vec![panel::TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 80.0,
                            }],
                            total_rows,
                            offset,
                            viewport_revision: vp_rev,
                            rows: vec![],
                            multi_select: false,
                        }),
                    ),
                    panel::Node::new(
                        4,
                        "ed",
                        panel::node::Kind::Editor(panel::Editor {
                            document_id: 1,
                            revision: 1,
                            contents: b"init".to_vec(),
                            format: panel::EditorFormat::Utf8 as i32,
                            read_only: false,
                        }),
                    ),
                ],
            }),
        )),
    };
    let read_vp = |snap: &PanelSnapshot| match snap
        .root
        .as_ref()
        .unwrap()
        .find(3)
        .unwrap()
        .kind
        .as_ref()
        .unwrap()
    {
        panel::node::Kind::VirtualTable(t) => (t.viewport_revision, t.offset),
        other => panic!("expected table, got {other:?}"),
    };

    // Part A: Claimed Viewport(2, offset=50) directly in optimistic_fields + unsolicited Snapshot(vp=2, offset=50)
    // followed by bare Ack -> ZERO drift, ZERO Resync, Panel shows vp=2, Ack emits PanelSynced(1).
    {
        let (mut running, _calls, mut updates) = running();
        *running.main_panel_mut() = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                            revision: 2,
                            offset: 50,
                            limit: 50,
                        })),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let vp_w = wire.try_recv().unwrap();
        while updates.try_recv().is_ok() {}

        let unsolicited = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 2, 50, 100, "page2")),
            MAX_OPERATION,
        );
        running.frame(unsolicited, &outgoing).await.unwrap();

        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 0,
                ref snapshot,
                ..
            }) if snapshot.revision == 2 && read_vp(snapshot) == (2, 50)
        ));

        let mut ack = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        );
        ack.in_reply_to = vp_w.request_id;
        running.frame(ack, &outgoing).await.unwrap();

        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::PanelSynced {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 1,
                ref snapshot,
                ..
            }) if snapshot.revision == 2 && read_vp(snapshot) == (2, 50)
        ));
    }

    // Part B: Claimed Viewport(2, offset=50) detached by unsolicited Snapshot(rev=2, vp=1), then second
    // unsolicited Snapshot(rev=3, vp=2, offset=50) arrives, followed by causal Text Patch -> ZERO drift,
    // ZERO Resync, and subsequent CommitEditor on child/main panel succeeds!
    {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        let child_res = Uuid::new_v4();
        let (o_tx, o_rx) = oneshot::channel();
        running
            .control(
                Control::Open {
                    panel_id: 2,
                    id: child_res,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: o_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let o_w = wire.try_recv().unwrap();
        let mut o_s = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Snapshot(make_snap(1, 1, 0, 100, "init")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        o_s.in_reply_to = o_w.request_id;
        running.frame(o_s, &outgoing).await.unwrap();
        o_rx.await.unwrap().unwrap();

        running
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                            revision: 2,
                            offset: 50,
                            limit: 50,
                        })),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let vp_w = wire.try_recv().unwrap();
        while updates.try_recv().is_ok() {}

        let unsolicited_1 = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, 0, 100, "stale-vp")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(unsolicited_1, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));

        let unsolicited_2 = Envelope::new(
            running.identity,
            3,
            3,
            envelope::Body::Snapshot(make_snap(3, 2, 50, 100, "page2")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(unsolicited_2, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(!running.has_inflight_resync(2));
        assert!(wire.try_recv().is_err());
        while updates.try_recv().is_ok() {}

        let mut vp_patch = Envelope::new(
            running.identity,
            4,
            4,
            envelope::Body::Patch(panel::PanelPatch {
                base_revision: 3,
                revision: 4,
                operations: vec![panel::PatchOperation {
                    node_id: 2,
                    change: Some(panel::patch_operation::Change::Replace(panel::Node::new(
                        2,
                        "status",
                        panel::node::Kind::Text(panel::Text {
                            text: "page2-done".into(),
                            tone: panel::Tone::Foreground as i32,
                        }),
                    ))),
                }],
            }),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        vp_patch.in_reply_to = vp_w.request_id;
        running.frame(vp_patch, &outgoing).await.unwrap();

        assert!(!running.is_panel_semantic_drifted(2));
        assert!(!running.has_inflight_resync(2));
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: 2,
                synced_ui_seq: 1,
                ref snapshot,
                ..
            }) if snapshot.revision == 4 && read_vp(snapshot) == (2, 50)
        ));

        let (commit_tx, commit_rx) = oneshot::channel();
        running
            .control(
                Control::Commit {
                    panel_id: 2,
                    op_id: Uuid::new_v4(),
                    request: CommitEditor {
                        resource_id: child_res.as_bytes().to_vec(),
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
                    reply: commit_tx,
                    expected_snapshot: Some(Arc::new(make_snap(4, 2, 50, 100, "page2-done"))),
                },
                &outgoing,
            )
            .await
            .unwrap();
        let commit_w = wire.try_recv().unwrap();
        assert!(matches!(commit_w.body, Some(envelope::Body::CommitEditor(_))));
        let mut committed = Envelope::new(
            running.identity,
            5,
            4,
            envelope::Body::Committed(protocol::Committed {
                resource_id: child_res.as_bytes().to_vec(),
                message_revision: 1,
                document_id: 1,
                document_revision: 1,
                request: Some(request().into()),
            }),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        committed.in_reply_to = commit_w.request_id;
        running.frame(committed, &outgoing).await.unwrap();
        commit_rx.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn confirmed_viewport_timeout_and_out_of_order_patch_keep_panel_usable_without_drift() {
    let make_snap =
        |rev: u64, vp_rev: u64, offset: u64, total_rows: u64, text: &str| PanelSnapshot {
            revision: rev,
            title: "confirmed-timeout-and-ooo".into(),
            root: Some(panel::Node::new(
                1,
                "col",
                panel::node::Kind::Column(panel::Container {
                    children: vec![
                        panel::Node::new(
                            2,
                            "status",
                            panel::node::Kind::Text(panel::Text {
                                text: text.into(),
                                tone: panel::Tone::Foreground as i32,
                            }),
                        ),
                        panel::Node::new(
                            3,
                            "tbl",
                            panel::node::Kind::VirtualTable(panel::VirtualTable {
                                columns: vec![panel::TableColumn {
                                    id: "c1".into(),
                                    title: "Col".into(),
                                    width: 80.0,
                                }],
                                total_rows,
                                offset,
                                viewport_revision: vp_rev,
                                rows: vec![],
                                multi_select: false,
                            }),
                        ),
                        panel::Node::new(
                            4,
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
                            5,
                            "btn",
                            panel::node::Kind::Button(panel::Button {
                                label: "Action".into(),
                                enabled: true,
                                commit_editors: false,
                            }),
                        ),
                    ],
                }),
            )),
        };
    let read_vp = |snap: &PanelSnapshot| match snap
        .root
        .as_ref()
        .unwrap()
        .find(3)
        .unwrap()
        .kind
        .as_ref()
        .unwrap()
    {
        panel::node::Kind::VirtualTable(t) => (t.viewport_revision, t.offset),
        other => panic!("expected table, got {other:?}"),
    };

    // Part A: Child panel Viewport(2, offset=50) is confirmed by unsolicited Snapshot(rev=2, vp=2, offset=50)
    // BEFORE original vp_w times out. Timing out MUST NOT mark confirmed req uncertain; when SDK default
    // rejects Resync with Error(Invalid), panel MUST NOT lock semantic drift, CommitEditor succeeds,
    // and late bare Ack settles cleanly without Resync.
    {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        let child_res = Uuid::new_v4();
        let (o_tx, o_rx) = oneshot::channel();
        running
            .control(
                Control::Open {
                    panel_id: 2,
                    id: child_res,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: o_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let o_w = wire.try_recv().unwrap();
        let mut o_s = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Snapshot(make_snap(1, 1, 0, 100, "init")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        o_s.in_reply_to = o_w.request_id;
        running.frame(o_s, &outgoing).await.unwrap();
        o_rx.await.unwrap().unwrap();

        running
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                            revision: 2,
                            offset: 50,
                            limit: 50,
                        })),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let vp_w = wire.try_recv().unwrap();

        let unsolicited = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Snapshot(make_snap(2, 2, 50, 100, "page2")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(unsolicited, &outgoing).await.unwrap();
        assert!(running.panel_has_confirmed_viewport_request(2, vp_w.request_id));
        while updates.try_recv().is_ok() {}

        running.requests.get_mut(&vp_w.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        assert!(!running.panel_has_uncertain_optimistic(2));
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(!running.has_inflight_resync(2));
        // Confirmed timeout MUST NOT emit Cancel or Resync on wire!
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Error { panel_id: 2, .. })
        ));
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::PanelSynced {
                panel_id: 2,
                synced_ui_seq: 1,
                ref rejected_ui_seqs,
                ..
            }) if rejected_ui_seqs.is_empty()
        ));

        // Follow-up Click & CommitEditor execute immediately without resync deferral or cancellation!
        running
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: Some(Arc::new(make_snap(2, 2, 50, 100, "page2"))),
                    event_seq: 2,
                    event: UiEvent {
                        node_id: 5,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Click(panel::Empty {})),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let click_w = wire.try_recv().unwrap();
        let mut click_ack = Envelope::new(
            running.identity,
            3,
            2,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        click_ack.in_reply_to = click_w.request_id;
        running.frame(click_ack, &outgoing).await.unwrap();

        // Late bare Ack for vp_w settles confirmed request without triggering Resync!
        let mut late_ack = Envelope::new(
            running.identity,
            4,
            2,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_ack.in_reply_to = vp_w.request_id;
        running.frame(late_ack, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(!running.has_inflight_resync(2));
        assert!(!running.panel_has_confirmed_viewport_request(2, vp_w.request_id));
        assert!(wire.try_recv().is_err());

        // CommitEditor on panel 2 still succeeds!
        let (commit_tx, commit_rx) = oneshot::channel();
        running
            .control(
                Control::Commit {
                    panel_id: 2,
                    op_id: Uuid::new_v4(),
                    request: CommitEditor {
                        resource_id: child_res.as_bytes().to_vec(),
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
                    reply: commit_tx,
                    expected_snapshot: Some(Arc::new(make_snap(2, 2, 50, 100, "page2"))),
                },
                &outgoing,
            )
            .await
            .unwrap();
        let commit_w = wire.try_recv().unwrap();
        let mut committed = Envelope::new(
            running.identity,
            5,
            2,
            envelope::Body::Committed(protocol::Committed {
                resource_id: child_res.as_bytes().to_vec(),
                message_revision: 1,
                document_id: 1,
                document_revision: 1,
                request: Some(request().into()),
            }),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        committed.in_reply_to = commit_w.request_id;
        running.frame(committed, &outgoing).await.unwrap();
        commit_rx.await.unwrap().unwrap();
    }

    // Part B: Confirmed Viewport(2, offset=50) (req_a) followed by unrelated Click (req_b > req_a) that
    // replies FIRST with causal Patch(rev=3); then req_a replies with bare Ack -> confirmed record for
    // req_a MUST survive req_b's earlier Patch and settle cleanly with NO Resync or drift.
    {
        let (mut running, _calls, mut updates) = running();
        *running.main_panel_mut() = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                            revision: 2,
                            offset: 50,
                            limit: 50,
                        })),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let req_a = wire.try_recv().unwrap();

        let unsolicited = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 2, 50, 100, "page2")),
            MAX_OPERATION,
        );
        running.frame(unsolicited, &outgoing).await.unwrap();
        assert!(running.panel_has_confirmed_viewport_request(protocol::PANEL_ID, req_a.request_id));

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 2,
                    event: UiEvent {
                        node_id: 5,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Click(panel::Empty {})),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let req_b = wire.try_recv().unwrap();
        assert!(req_b.request_id > req_a.request_id);

        let mut patch_b = Envelope::new(
            running.identity,
            2,
            3,
            envelope::Body::Patch(panel::PanelPatch {
                base_revision: 2,
                revision: 3,
                operations: vec![panel::PatchOperation {
                    node_id: 2,
                    change: Some(panel::patch_operation::Change::Replace(panel::Node::new(
                        2,
                        "status",
                        panel::node::Kind::Text(panel::Text {
                            text: "clicked".into(),
                            tone: panel::Tone::Foreground as i32,
                        }),
                    ))),
                }],
            }),
            MAX_OPERATION,
        );
        patch_b.in_reply_to = req_b.request_id;
        running.frame(patch_b, &outgoing).await.unwrap();
        assert!(running.panel_has_confirmed_viewport_request(protocol::PANEL_ID, req_a.request_id));
        while updates.try_recv().is_ok() {}

        let mut ack_a = Envelope::new(
            running.identity,
            3,
            3,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        );
        ack_a.in_reply_to = req_a.request_id;
        running.frame(ack_a, &outgoing).await.unwrap();

        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::PanelSynced {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 2,
                ref snapshot,
                ..
            }) if snapshot.revision == 3 && read_vp(snapshot) == (2, 50)
        ));
    }

    // Part C: Unsolicited loading Snapshot(vp=2, offset=0) keeps Viewport(2, offset=50) pending without
    // confirming or drifting; subsequent target Snapshot(vp=2, offset=50) confirms it so bare Ack settles.
    {
        let (mut running, _calls, mut updates) = running();
        *running.main_panel_mut() = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                            revision: 2,
                            offset: 50,
                            limit: 50,
                        })),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let vp_w = wire.try_recv().unwrap();

        let loading = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 2, 0, 100, "loading")),
            MAX_OPERATION,
        );
        running.frame(loading, &outgoing).await.unwrap();
        assert!(!running.panel_has_confirmed_viewport_request(protocol::PANEL_ID, vp_w.request_id));
        assert!(running.has_panel_tracked_optimistic_request(protocol::PANEL_ID, vp_w.request_id));
        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));

        let ready = Envelope::new(
            running.identity,
            2,
            3,
            envelope::Body::Snapshot(make_snap(3, 2, 50, 100, "ready")),
            MAX_OPERATION,
        );
        running.frame(ready, &outgoing).await.unwrap();
        assert!(running.panel_has_confirmed_viewport_request(protocol::PANEL_ID, vp_w.request_id));
        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        while updates.try_recv().is_ok() {}

        let mut ack = Envelope::new(
            running.identity,
            3,
            3,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        );
        ack.in_reply_to = vp_w.request_id;
        running.frame(ack, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::PanelSynced {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 1,
                ref snapshot,
                ..
            }) if snapshot.revision == 3 && read_vp(snapshot) == (2, 50)
        ));
    }
}

#[tokio::test]
async fn expired_confirmed_ack_async_loading_ack_and_ooo_snapshot_behave_without_false_drift() {
    let make_snap =
        |rev: u64, vp_rev: u64, offset: u64, total_rows: u64, text: &str| PanelSnapshot {
            revision: rev,
            title: "round48-regression".into(),
            root: Some(panel::Node::new(
                1,
                "col",
                panel::node::Kind::Column(panel::Container {
                    children: vec![
                        panel::Node::new(
                            2,
                            "status",
                            panel::node::Kind::Text(panel::Text {
                                text: text.into(),
                                tone: panel::Tone::Foreground as i32,
                            }),
                        ),
                        panel::Node::new(
                            3,
                            "tbl",
                            panel::node::Kind::VirtualTable(panel::VirtualTable {
                                columns: vec![panel::TableColumn {
                                    id: "c1".into(),
                                    title: "Col".into(),
                                    width: 80.0,
                                }],
                                total_rows,
                                offset,
                                viewport_revision: vp_rev,
                                rows: vec![],
                                multi_select: false,
                            }),
                        ),
                        panel::Node::new(
                            4,
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
                            5,
                            "btn",
                            panel::node::Kind::Button(panel::Button {
                                label: "Action".into(),
                                enabled: true,
                                commit_editors: false,
                            }),
                        ),
                    ],
                }),
            )),
        };
    let read_vp = |snap: &PanelSnapshot| match snap
        .root
        .as_ref()
        .unwrap()
        .find(3)
        .unwrap()
        .kind
        .as_ref()
        .unwrap()
    {
        panel::node::Kind::VirtualTable(t) => (t.viewport_revision, t.offset),
        other => panic!("expected table, got {other:?}"),
    };

    // Part A: Child panel confirmed Viewport(2, offset=50) receives its bare Ack AFTER local deadline
    // (expired child-frame path io.rs:443): MUST NOT mark semantic drift, SDK default Error(Invalid)
    // leaves CommitEditor unblocked!
    {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        let child_res = Uuid::new_v4();
        let (o_tx, o_rx) = oneshot::channel();
        running
            .control(
                Control::Open {
                    panel_id: 2,
                    id: child_res,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: o_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let o_w = wire.try_recv().unwrap();
        let mut o_s = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Snapshot(make_snap(1, 1, 0, 100, "init")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        o_s.in_reply_to = o_w.request_id;
        running.frame(o_s, &outgoing).await.unwrap();
        o_rx.await.unwrap().unwrap();

        running
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                            revision: 2,
                            offset: 50,
                            limit: 50,
                        })),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let vp_w = wire.try_recv().unwrap();

        let unsolicited = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Snapshot(make_snap(2, 2, 50, 100, "page2")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(unsolicited, &outgoing).await.unwrap();
        assert!(running.panel_has_confirmed_viewport_request(2, vp_w.request_id));

        // Second unsolicited snapshot advances child panel revision to 3 before vp_w's older rev=2 Ack arrives.
        let unsolicited_3 = Envelope::new(
            running.identity,
            3,
            3,
            envelope::Body::Snapshot(make_snap(3, 2, 50, 100, "v3")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(unsolicited_3, &outgoing).await.unwrap();
        while updates.try_recv().is_ok() {}

        // Expire pending vp_w before receiving its older rev=2 bare Ack -> hits expired child frame branch!
        running.requests.get_mut(&vp_w.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        let mut expired_ack = Envelope::new(
            running.identity,
            4,
            2,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        expired_ack.in_reply_to = vp_w.request_id;
        running.frame(expired_ack, &outgoing).await.unwrap();

        assert!(!running.is_panel_semantic_drifted(2));
        assert!(!running.has_inflight_resync(2));
        assert!(!running.has_panel_tracked_optimistic_request(2, vp_w.request_id));
        // Confirmed expired Ack MUST NOT emit Cancel or Resync on the wire!
        assert!(wire.try_recv().is_err());

        // PanelSynced from confirmed Ack MUST NOT mark seq=1 as rejected.
        let mut saw_synced = false;
        while let Ok(ev) = updates.try_recv() {
            if let PluginEvent::PanelSynced {
                panel_id: 2,
                synced_ui_seq: 1,
                rejected_ui_seqs,
                ..
            } = ev
            {
                assert!(rejected_ui_seqs.is_empty());
                saw_synced = true;
            }
        }
        assert!(saw_synced);

        // Follow-up interaction and CommitEditor on panel 2 run immediately without resync deferral!
        running
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: Some(Arc::new(make_snap(3, 2, 50, 100, "v3"))),
                    event_seq: 2,
                    event: UiEvent {
                        node_id: 5,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Click(panel::Empty {})),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let click_w = wire.try_recv().unwrap();
        let mut click_ack = Envelope::new(
            running.identity,
            5,
            3,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        click_ack.in_reply_to = click_w.request_id;
        running.frame(click_ack, &outgoing).await.unwrap();

        let (commit_tx, commit_rx) = oneshot::channel();
        running
            .control(
                Control::Commit {
                    panel_id: 2,
                    op_id: Uuid::new_v4(),
                    request: CommitEditor {
                        resource_id: child_res.as_bytes().to_vec(),
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
                    reply: commit_tx,
                    expected_snapshot: Some(Arc::new(make_snap(3, 2, 50, 100, "v3"))),
                },
                &outgoing,
            )
            .await
            .unwrap();
        let commit_w = wire.try_recv().unwrap();
        let mut committed = Envelope::new(
            running.identity,
            6,
            3,
            envelope::Body::Committed(protocol::Committed {
                resource_id: child_res.as_bytes().to_vec(),
                message_revision: 1,
                document_id: 1,
                document_revision: 1,
                request: Some(request().into()),
            }),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        committed.in_reply_to = commit_w.request_id;
        running.frame(committed, &outgoing).await.unwrap();
        commit_rx.await.unwrap().unwrap();
    }

    // Part B: Claimed Viewport(2, offset=50, seq=1) detached by unsolicited loading Snapshot(rev=2, vp=2, offset=0)
    // then fails with Error(Deadline) -> emitted PanelSynced MUST keep rejected_ui_seqs == [1] even though
    // synced_ui_seq == 1 (never stripped by > synced_ui_seq!). Same for child timeout.
    {
        let (mut running, _calls, mut updates) = running();
        *running.main_panel_mut() = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                            revision: 2,
                            offset: 50,
                            limit: 50,
                        })),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let vp_w = wire.try_recv().unwrap();

        let loading = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 2, 0, 100, "loading")),
            MAX_OPERATION,
        );
        running.frame(loading, &outgoing).await.unwrap();
        while updates.try_recv().is_ok() {}

        let mut err = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Error(Failure::new(ErrorCode::Deadline, "pre-dispatch deadline")),
            MAX_OPERATION,
        );
        err.in_reply_to = vp_w.request_id;
        running.frame(err, &outgoing).await.unwrap();

        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Error { panel_id: protocol::PANEL_ID, .. })
        ));
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::PanelSynced {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 1,
                ref rejected_ui_seqs,
                ..
            }) if rejected_ui_seqs == &[1]
        ));
    }

    // Part C: Claimed Viewport(2, offset=50, seq=1) detached by unsolicited loading Snapshot(rev=2, vp=2, offset=0)
    // then receives timely bare Ack(rev=2, in_reply_to=vp_w.request_id) BEFORE target page arrives ->
    // MUST settle cleanly with NO semantic drift and NO Resync!
    {
        let (mut running, _calls, mut updates) = running();
        *running.main_panel_mut() = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                            revision: 2,
                            offset: 50,
                            limit: 50,
                        })),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let vp_w = wire.try_recv().unwrap();

        let loading = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 2, 0, 100, "loading")),
            MAX_OPERATION,
        );
        running.frame(loading, &outgoing).await.unwrap();
        assert!(running.panel_has_detached_viewport(
            protocol::PANEL_ID,
            vp_w.request_id,
        ));
        while updates.try_recv().is_ok() {}

        let mut ack = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        );
        ack.in_reply_to = vp_w.request_id;
        running.frame(ack, &outgoing).await.unwrap();

        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::PanelSynced {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 1,
                ref rejected_ui_seqs,
                ..
            }) if rejected_ui_seqs.is_empty()
        ));
    }

    // Part D: Claimed Viewport(2, offset=50, seq=1) (req_a) STILL in optimistic_fields + later Button(req_b > req_a)
    // replies FIRST with causal full Snapshot(rev=2, vp=2, offset=50); then req_a replies with bare Ack ->
    // MUST confirm req_a from req_b's Snapshot and settle req_a cleanly without Resync or drift!
    {
        let (mut running, _calls, mut updates) = running();
        *running.main_panel_mut() = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                            revision: 2,
                            offset: 50,
                            limit: 50,
                        })),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let req_a = wire.try_recv().unwrap();

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 2,
                    event: UiEvent {
                        node_id: 5,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Click(panel::Empty {})),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let req_b = wire.try_recv().unwrap();
        assert!(req_b.request_id > req_a.request_id);

        let mut snap_b = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 2, 50, 100, "page2-by-b")),
            MAX_OPERATION,
        );
        snap_b.in_reply_to = req_b.request_id;
        running.frame(snap_b, &outgoing).await.unwrap();
        assert!(running.panel_has_confirmed_viewport_request(protocol::PANEL_ID, req_a.request_id));
        while updates.try_recv().is_ok() {}

        let mut ack_a = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        );
        ack_a.in_reply_to = req_a.request_id;
        running.frame(ack_a, &outgoing).await.unwrap();

        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::PanelSynced {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 2,
                ref rejected_ui_seqs,
                ref snapshot,
                ..
            }) if snapshot.revision == 2
                && read_vp(snapshot) == (2, 50)
                && rejected_ui_seqs.is_empty()
        ));
    }

    // Part E: Claimed Viewport(2, offset=50, seq=1) (req_a) in optimistic_fields + later Button(req_b > req_a)
    // replies FIRST with causal full Snapshot(rev=2, vp=1, offset=0) carrying true older vp=1 ->
    // Host MUST accept it with NO drift/Resync, move req_a to detached tracking, and accept req_a's causal
    // Change::Table(vp=2, offset=50) Patch!
    {
        let (mut running, _calls, mut updates) = running();
        *running.main_panel_mut() = PanelState::new(make_snap(1, 1, 0, 100, "init")).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                            revision: 2,
                            offset: 50,
                            limit: 50,
                        })),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let req_a = wire.try_recv().unwrap();

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 2,
                    event: UiEvent {
                        node_id: 5,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Click(panel::Empty {})),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let req_b = wire.try_recv().unwrap();
        assert!(req_b.request_id > req_a.request_id);
        while updates.try_recv().is_ok() {}

        let mut snap_b = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, 0, 100, "clicked-still-vp1")),
            MAX_OPERATION,
        );
        snap_b.in_reply_to = req_b.request_id;
        running.frame(snap_b, &outgoing).await.unwrap();

        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        assert!(wire.try_recv().is_err());
        assert!(running.has_panel_tracked_optimistic_request(protocol::PANEL_ID, req_a.request_id));
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 0,
                ref snapshot,
                ..
            }) if snapshot.revision == 2 && read_vp(snapshot) == (1, 0)
        ));

        let mut patch_a = Envelope::new(
            running.identity,
            2,
            3,
            envelope::Body::Patch(panel::PanelPatch {
                base_revision: 2,
                revision: 3,
                operations: vec![panel::PatchOperation {
                    node_id: 3,
                    change: Some(panel::patch_operation::Change::Table(panel::VirtualTable {
                        columns: vec![panel::TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 80.0,
                        }],
                        total_rows: 100,
                        offset: 50,
                        viewport_revision: 2,
                        rows: vec![],
                        multi_select: false,
                    })),
                }],
            }),
            MAX_OPERATION,
        );
        patch_a.in_reply_to = req_a.request_id;
        running.frame(patch_a, &outgoing).await.unwrap();

        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 2,
                ref rejected_ui_seqs,
                ref snapshot,
                ..
            }) if snapshot.revision == 3
                && read_vp(snapshot) == (2, 50)
                && rejected_ui_seqs.is_empty()
        ));
    }
}
