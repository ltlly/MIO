use super::*;

#[tokio::test]
async fn bare_ack_viewport_reverse_ooo_confirmation_and_stale_patch_resync_rejection() {
    let make_snap =
        |rev: u64, vp_rev: u64, offset: u64, total_rows: u64, text: &str| PanelSnapshot {
            revision: rev,
            title: "round50-regression".into(),
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

    // Part A: Claimed Viewport(2, offset=50, seq=1) receives bare Ack(rev=1) (no loading snapshot) ->
    // Host rolls watermark back to 1 instead of promoting 2, emits PanelSynced(vp=1), and accepts a later
    // unrelated Click's Snapshot(rev=2, vp=1, offset=0) without drift or Resync!
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

        let mut ack = Envelope::new(
            running.identity,
            1,
            1,
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
                ref snapshot,
                ..
            }) if snapshot.revision == 1 && read_vp(snapshot) == (1, 0) && rejected_ui_seqs.is_empty()
        ));

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
        let click_w = wire.try_recv().unwrap();
        let mut snap = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, 0, 100, "clicked")),
            MAX_OPERATION,
        );
        snap.in_reply_to = click_w.request_id;
        running.frame(snap, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 2,
                ref snapshot,
                ..
            }) if snapshot.revision == 2 && read_vp(snapshot) == (1, 0)
        ));
    }

    // Part B: Reverse out-of-order: earlier Click (req_a) + later claimed Viewport(2, offset=50) (req_b > req_a);
    // req_a's causal Snapshot(rev=2, vp=2, offset=50) arrives FIRST -> confirms req_b cleanly with NO Resync,
    // and req_b's bare Ack settles cleanly with synced_ui_seq=2!
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
                        node_id: 5,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Click(panel::Empty {})),
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
        let req_b = wire.try_recv().unwrap();
        assert!(req_b.request_id > req_a.request_id);
        while updates.try_recv().is_ok() {}

        let mut snap_a = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 2, 50, 100, "page2-via-a")),
            MAX_OPERATION,
        );
        snap_a.in_reply_to = req_a.request_id;
        running.frame(snap_a, &outgoing).await.unwrap();

        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        assert!(wire.try_recv().is_err());
        assert!(running.panel_has_confirmed_viewport_request(protocol::PANEL_ID, req_b.request_id));
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 1,
                ref snapshot,
                ..
            }) if snapshot.revision == 2 && read_vp(snapshot) == (2, 50)
        ));

        let mut ack_b = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        );
        ack_b.in_reply_to = req_b.request_id;
        running.frame(ack_b, &outgoing).await.unwrap();
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
            }) if snapshot.revision == 2 && read_vp(snapshot) == (2, 50) && rejected_ui_seqs.is_empty()
        ));
    }

    // Part C: Claimed Viewport(2, offset=50, seq=1) (vp_w) receives loading Snapshot(rev=2, vp=2, offset=0),
    // then another unsolicited Snapshot(rev=3, vp=2, offset=0), then vp_w's Patch(base=2, rev=3) fails
    // base-revision validation in resync_panel -> MUST emit PanelSynced carrying rejected_ui_seqs=[1]
    // immediately so UI clears loading lock even before Resync finishes/fails!
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

        let loading_2 = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 2, 0, 100, "loading")),
            MAX_OPERATION,
        );
        running.frame(loading_2, &outgoing).await.unwrap();

        let intervening_3 = Envelope::new(
            running.identity,
            2,
            3,
            envelope::Body::Snapshot(make_snap(3, 2, 0, 100, "other-update")),
            MAX_OPERATION,
        );
        running.frame(intervening_3, &outgoing).await.unwrap();
        while updates.try_recv().is_ok() {}

        let mut stale_patch = Envelope::new(
            running.identity,
            3,
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
        stale_patch.in_reply_to = vp_w.request_id;
        running.frame(stale_patch, &outgoing).await.unwrap();

        let resync_w = wire.try_recv().unwrap();
        assert!(matches!(resync_w.body, Some(envelope::Body::Resync(_))));
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Error { panel_id: protocol::PANEL_ID, failure, .. })
                if failure.code == ErrorCode::Stale as i32
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

    // Part D:
    // D1: Claimed Viewport(2, offset=50, seq=1) detached by unsolicited old-watermark Snapshot(rev=2, vp=1)
    //     receives bare Ack(rev=2) -> settles cleanly without Resync or drift and records acked ceiling;
    // D2: Follow-up unsolicited async Patch(base=2, rev=3, Change::Table(vp=2, offset=50)) succeeds
    //     cleanly after bare Ack (from BOTH detached and non-detached bare Ack paths)!
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

        let unsolicited = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, 0, 100, "still-vp1")),
            MAX_OPERATION,
        );
        running.frame(unsolicited, &outgoing).await.unwrap();
        assert!(running.panel_has_detached_viewport(protocol::PANEL_ID, vp_w.request_id));
        while updates.try_recv().is_ok() {}

        let mut bare_ack = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        );
        bare_ack.in_reply_to = vp_w.request_id;
        running.frame(bare_ack, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::PanelSynced {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 1,
                ref rejected_ui_seqs,
                ref snapshot,
                ..
            }) if snapshot.revision == 2 && read_vp(snapshot) == (1, 0) && rejected_ui_seqs.is_empty()
        ));

        let async_table_patch = Envelope::new(
            running.identity,
            3,
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
        running.frame(async_table_patch, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 1,
                ref snapshot,
                ..
            }) if snapshot.revision == 3 && read_vp(snapshot) == (2, 50)
        ));
    }
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

        let mut bare_ack = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        );
        bare_ack.in_reply_to = vp_w.request_id;
        running.frame(bare_ack, &outgoing).await.unwrap();
        while updates.try_recv().is_ok() {}

        let async_table_patch = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Patch(panel::PanelPatch {
                base_revision: 1,
                revision: 2,
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
        running.frame(async_table_patch, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 1,
                ref snapshot,
                ..
            }) if snapshot.revision == 2 && read_vp(snapshot) == (2, 50)
        ));
    }

    // Part E: Runtime coverage for all 3 Round 51 P2s:
    // E1 (P2-①): Bare Ack settles Viewport(2), then an ancestor Replace(Column) updating only status text
    //            preserves acked_viewports so a follow-up Change::Table(vp=2, offset=50) Patch succeeds.
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

        let mut bare_ack = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        );
        bare_ack.in_reply_to = vp_w.request_id;
        running.frame(bare_ack, &outgoing).await.unwrap();

        let ancestor_patch = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Patch(panel::PanelPatch {
                base_revision: 1,
                revision: 2,
                operations: vec![panel::PatchOperation {
                    node_id: 1,
                    change: Some(panel::patch_operation::Change::Replace(
                        make_snap(2, 1, 0, 100, "status-only").root.unwrap(),
                    )),
                }],
            }),
            MAX_OPERATION,
        );
        running.frame(ancestor_patch, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        while updates.try_recv().is_ok() {}

        let async_table_patch = Envelope::new(
            running.identity,
            3,
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
        running.frame(async_table_patch, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 1,
                ref snapshot,
                ..
            }) if snapshot.revision == 3 && read_vp(snapshot) == (2, 50)
        ));
    }

    // E2 (P2-②): Detached Viewport(2, offset=50) replies with causal Patch(Change::Table(vp=1, total_rows=120))
    //            keeping old watermark 1; Host MUST transfer (2, 50) to acked_viewports so the follow-up
    //            unsolicited Patch(Change::Table(vp=2, offset=50)) succeeds cleanly!
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

        let unsolicited = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, 0, 100, "unsolicited")),
            MAX_OPERATION,
        );
        running.frame(unsolicited, &outgoing).await.unwrap();

        let mut causal_old_vp_patch = Envelope::new(
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
                        total_rows: 120,
                        offset: 0,
                        viewport_revision: 1,
                        rows: vec![],
                        multi_select: false,
                    })),
                }],
            }),
            MAX_OPERATION,
        );
        causal_old_vp_patch.in_reply_to = vp_w.request_id;
        running.frame(causal_old_vp_patch, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        while updates.try_recv().is_ok() {}

        let target_patch = Envelope::new(
            running.identity,
            3,
            4,
            envelope::Body::Patch(panel::PanelPatch {
                base_revision: 3,
                revision: 4,
                operations: vec![panel::PatchOperation {
                    node_id: 3,
                    change: Some(panel::patch_operation::Change::Table(panel::VirtualTable {
                        columns: vec![panel::TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 80.0,
                        }],
                        total_rows: 120,
                        offset: 50,
                        viewport_revision: 2,
                        rows: vec![],
                        multi_select: false,
                    })),
                }],
            }),
            MAX_OPERATION,
        );
        running.frame(target_patch, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 1,
                ref snapshot,
                ..
            }) if snapshot.revision == 4 && read_vp(snapshot) == (2, 50)
        ));
    }

    // E3 (P2-③): Claimed Viewport(2, offset=50) whose Patch(base=0, rev=2) is REJECTED by resync_panel
    //            MUST discard the viewport WITHOUT recording acked_viewports; after Resync Ack recovers
    //            to vp=1, a late unsolicited Patch(Change::Table(vp=2, offset=50)) MUST be rejected!
    {
        let (mut running, _calls, _updates) = running();
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

        let mut bad_patch = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Patch(panel::PanelPatch {
                base_revision: 0,
                revision: 2,
                operations: vec![panel::PatchOperation {
                    node_id: 2,
                    change: Some(panel::patch_operation::Change::Replace(panel::Node::new(
                        2,
                        "status",
                        panel::node::Kind::Text(panel::Text {
                            text: "bad".into(),
                            tone: panel::Tone::Foreground as i32,
                        }),
                    ))),
                }],
            }),
            MAX_OPERATION,
        );
        bad_patch.in_reply_to = vp_w.request_id;
        running.frame(bad_patch, &outgoing).await.unwrap();
        let resync_w = wire.try_recv().unwrap();
        assert!(matches!(resync_w.body, Some(envelope::Body::Resync(_))));

        // Even BEFORE Resync finishes, late unsolicited Patch(Change::Table(vp=2)) MUST be rejected
        // (proving reject_response discarded vp_w without recording acked_viewports)!
        let late_vp2_patch = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Patch(panel::PanelPatch {
                base_revision: 1,
                revision: 2,
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
        running.frame(late_vp2_patch, &outgoing).await.unwrap();
        assert_eq!(running.panel_revision(protocol::PANEL_ID), 1);

        // Now Resync Snapshot(rev=2, vp=1) completes recovery cleanly (!semantic_drift), and a
        // subsequent unsolicited Patch(base=2, rev=3, Change::Table(vp=2)) is STILL rejected!
        let mut resync_snap = Envelope::new(
            running.identity,
            3,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, 0, 100, "resynced-vp1")),
            MAX_OPERATION,
        );
        resync_snap.in_reply_to = resync_w.request_id;
        running.frame(resync_snap, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));

        let late_vp2_patch_after = Envelope::new(
            running.identity,
            4,
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
        running.frame(late_vp2_patch_after, &outgoing).await.unwrap();
        let resync_2 = wire.try_recv().unwrap();
        assert!(matches!(resync_2.body, Some(envelope::Body::Resync(_))));
    }
}

#[tokio::test]
async fn pruned_viewport_gap_aborted_resync_acked_vp_and_stale_checkbox_patch_rollback() {
    let make_snap =
        |rev: u64, vp_rev: u64, offset: u64, total_rows: u64, checked: bool, text: &str| {
            PanelSnapshot {
                revision: rev,
                title: "round53-regression".into(),
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
                                "chk",
                                panel::node::Kind::Checkbox(panel::CheckboxNode {
                                    label: "Flag".into(),
                                    checked,
                                    enabled: true,
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
            }
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
        panel::node::Kind::VirtualTable(t) => (t.viewport_revision, t.offset, t.total_rows),
        other => panic!("expected table, got {other:?}"),
    };
    let read_chk = |snap: &PanelSnapshot| match snap
        .root
        .as_ref()
        .unwrap()
        .find(4)
        .unwrap()
        .kind
        .as_ref()
        .unwrap()
    {
        panel::node::Kind::Checkbox(c) => c.checked,
        other => panic!("expected checkbox, got {other:?}"),
    };

    // Part A (P2-①): Both vp=2(offset=50) and vp=3(offset=10) bare-Acked; Snapshot(rev=2, vp=1, total_rows=20)
    // prunes vp=2 while preserving vp=3. Stale Patch(Change::Table(vp=2, total_rows=100)) MUST be rejected
    // (membership check), whereas valid Change::Table(vp=3, total_rows=20) succeeds cleanly!
    {
        let (mut running, _calls, _updates) = running();
        *running.main_panel_mut() = PanelState::new(make_snap(1, 1, 0, 100, false, "init")).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        for (seq, vp_rev, offset) in [(1, 2, 50), (2, 3, 10)] {
            running
                .control(
                    Control::Event {
                        panel_id: protocol::PANEL_ID,
                        expected_snapshot: None,
                        event_seq: seq,
                        event: UiEvent {
                            node_id: 3,
                            editors: vec![],
                            event: Some(panel::ui_event::Event::Viewport(
                                panel::ViewportRequest {
                                    revision: vp_rev,
                                    offset,
                                    limit: 50,
                                },
                            )),
                        },
                    },
                    &outgoing,
                )
                .await
                .unwrap();
            let vp_w = wire.try_recv().unwrap();
            let mut ack = Envelope::new(
                running.identity,
                seq,
                1,
                envelope::Body::Ack(panel::Empty {}),
                MAX_OPERATION,
            );
            ack.in_reply_to = vp_w.request_id;
            running.frame(ack, &outgoing).await.unwrap();
        }

        let shrunk_snap = Envelope::new(
            running.identity,
            3,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, 0, 20, false, "shrunk")),
            MAX_OPERATION,
        );
        running.frame(shrunk_snap, &outgoing).await.unwrap();

        let stale_vp2_patch = Envelope::new(
            running.identity,
            4,
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
        running.frame(stale_vp2_patch, &outgoing).await.unwrap();
        let resync_w = wire.try_recv().unwrap();
        assert!(matches!(resync_w.body, Some(envelope::Body::Resync(_))));
        assert_eq!(read_vp(running.main_panel().snapshot()), (1, 0, 20));
    }
    {
        let (mut running, _calls, mut updates) = running();
        *running.main_panel_mut() = PanelState::new(make_snap(1, 1, 0, 100, false, "init")).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        for (seq, vp_rev, offset) in [(1, 2, 50), (2, 3, 10)] {
            running
                .control(
                    Control::Event {
                        panel_id: protocol::PANEL_ID,
                        expected_snapshot: None,
                        event_seq: seq,
                        event: UiEvent {
                            node_id: 3,
                            editors: vec![],
                            event: Some(panel::ui_event::Event::Viewport(
                                panel::ViewportRequest {
                                    revision: vp_rev,
                                    offset,
                                    limit: 50,
                                },
                            )),
                        },
                    },
                    &outgoing,
                )
                .await
                .unwrap();
            let vp_w = wire.try_recv().unwrap();
            let mut ack = Envelope::new(
                running.identity,
                seq,
                1,
                envelope::Body::Ack(panel::Empty {}),
                MAX_OPERATION,
            );
            ack.in_reply_to = vp_w.request_id;
            running.frame(ack, &outgoing).await.unwrap();
        }
        let shrunk_snap = Envelope::new(
            running.identity,
            3,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, 0, 20, false, "shrunk")),
            MAX_OPERATION,
        );
        running.frame(shrunk_snap, &outgoing).await.unwrap();
        while updates.try_recv().is_ok() {}

        let valid_vp3_patch = Envelope::new(
            running.identity,
            4,
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
                        total_rows: 20,
                        offset: 10,
                        viewport_revision: 3,
                        rows: vec![],
                        multi_select: false,
                    })),
                }],
            }),
            MAX_OPERATION,
        );
        running.frame(valid_vp3_patch, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: protocol::PANEL_ID,
                ref snapshot,
                ..
            }) if snapshot.revision == 3 && read_vp(snapshot) == (3, 10, 20)
        ));
    }

    // Part B (P2-②): Child panel has bare-Acked Viewport(2, offset=50); another unrelated request times out
    // -> Resync rejected with Error(Invalid) (abort_panel_resync); subsequent async Change::Table(vp=2)
    // Patch MUST still succeed without drift!
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
            envelope::Body::Snapshot(make_snap(1, 1, 0, 100, false, "init")),
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
        let mut vp_ack = Envelope::new(
            running.identity,
            2,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        vp_ack.in_reply_to = vp_w.request_id;
        running.frame(vp_ack, &outgoing).await.unwrap();

        // Unrelated non-optimistic Button Click times out -> Resync rejected with Error(Invalid).
        running
            .control(
                Control::Event {
                    panel_id: 2,
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
        let btn_w = wire.try_recv().unwrap();
        running.requests.get_mut(&btn_w.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let _cancel_w = wire.try_recv().unwrap();
        let resync_w = wire.try_recv().unwrap();

        let mut resync_err = Envelope::new(
            running.identity,
            3,
            1,
            envelope::Body::Error(Failure::new(ErrorCode::Invalid, "resync unsupported")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        resync_err.in_reply_to = resync_w.request_id;
        running.frame(resync_err, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        while updates.try_recv().is_ok() {}

        let async_target = Envelope::new(
            running.identity,
            4,
            2,
            envelope::Body::Patch(panel::PanelPatch {
                base_revision: 1,
                revision: 2,
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
        )
        .with_panel_id(2);
        running.frame(async_target, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: 2,
                ref snapshot,
                ..
            }) if snapshot.revision == 2 && read_vp(snapshot) == (2, 50, 100)
        ));
    }

    // Part C (P2-③): Stale Patch on Checkbox(true) rejected by resync_panel MUST roll Checkbox back to
    // false (`ledger.base`) rather than promoting rejected true into Resync and PanelSynced!
    {
        let (mut running, _calls, mut updates) = running();
        *running.main_panel_mut() = PanelState::new(make_snap(1, 1, 0, 100, false, "init")).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 4,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Toggled(true)),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let chk_w = wire.try_recv().unwrap();

        let text_patch = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Patch(panel::PanelPatch {
                base_revision: 1,
                revision: 2,
                operations: vec![panel::PatchOperation {
                    node_id: 2,
                    change: Some(panel::patch_operation::Change::Replace(panel::Node::new(
                        2,
                        "status",
                        panel::node::Kind::Text(panel::Text {
                            text: "v2".into(),
                            tone: panel::Tone::Foreground as i32,
                        }),
                    ))),
                }],
            }),
            MAX_OPERATION,
        );
        running.frame(text_patch, &outgoing).await.unwrap();
        assert!(read_chk(running.main_panel().snapshot()));
        while updates.try_recv().is_ok() {}

        let mut stale_chk_patch = Envelope::new(
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
                        "chk",
                        panel::node::Kind::Checkbox(panel::CheckboxNode {
                            label: "Flag".into(),
                            checked: true,
                            enabled: true,
                        }),
                    ))),
                }],
            }),
            MAX_OPERATION,
        );
        stale_chk_patch.in_reply_to = chk_w.request_id;
        running.frame(stale_chk_patch, &outgoing).await.unwrap();

        assert!(!read_chk(running.main_panel().snapshot()));
        let resync_w = wire.try_recv().unwrap();
        assert!(matches!(
            resync_w.body,
            Some(envelope::Body::Resync(ref snap)) if !read_chk(snap)
        ));
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Error { panel_id: protocol::PANEL_ID, failure, .. })
                if failure.code == ErrorCode::Stale as i32
        ));
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::PanelSynced {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 1,
                ref rejected_ui_seqs,
                ref snapshot,
                ..
            }) if !read_chk(snapshot) && rejected_ui_seqs == &[1]
        ));
    }
}

#[tokio::test]
async fn pruned_viewport_snapshot_replace_unrelated_resync_acked_vp_and_converged_late_ack() {
    let make_snap = |rev: u64, vp_rev: u64, offset: u64, total_rows: u64, sel_idx: u32, text: &str| {
        PanelSnapshot {
            revision: rev,
            title: "round54-regression".into(),
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
                            "sel",
                            panel::node::Kind::Select(panel::SelectNode {
                                label: "Mode".into(),
                                options: vec!["A".into(), "B".into()],
                                selected_index: sel_idx,
                                enabled: true,
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
        }
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
        panel::node::Kind::VirtualTable(t) => (t.viewport_revision, t.offset, t.total_rows),
        other => panic!("expected table, got {other:?}"),
    };
    async fn open_child(
        running: &mut Running,
        outgoing: &mpsc::Sender<Envelope>,
        wire: &mut mpsc::Receiver<Envelope>,
        snap: PanelSnapshot,
    ) {
        let rev = snap.revision;
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
                outgoing,
            )
            .await
            .unwrap();
        let o_w = wire.try_recv().unwrap();
        let mut o_s = Envelope::new(
            running.identity,
            1,
            rev,
            envelope::Body::Snapshot(snap),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        o_s.in_reply_to = o_w.request_id;
        running.frame(o_s, outgoing).await.unwrap();
        o_rx.await.unwrap().unwrap();
    }

    // Part A (P2-①): After Snapshot(rev=2, vp=1, total_rows=20) prunes bare-Acked Viewport(2, offset=50),
    // late unsolicited Snapshot(rev=3, vp=2, offset=50, total_rows=100) AND ancestor Replace(Column) with
    // (vp=2, offset=50, total_rows=100) MUST both be rejected (triggering Resync) so table never restores
    // total_rows=100!
    {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, 100, 0, "init")).await;

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
        let mut vp_ack = Envelope::new(
            running.identity,
            2,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        vp_ack.in_reply_to = vp_w.request_id;
        running.frame(vp_ack, &outgoing).await.unwrap();

        let shrink_snap = Envelope::new(
            running.identity,
            3,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, 0, 20, 0, "shrunk")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(shrink_snap, &outgoing).await.unwrap();
        assert_eq!(read_vp(running.panel_snapshot(2).as_deref().unwrap()), (1, 0, 20));
        while updates.try_recv().is_ok() {}

        let late_stale_snap = Envelope::new(
            running.identity,
            4,
            3,
            envelope::Body::Snapshot(make_snap(3, 2, 50, 100, 0, "stale-tree")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(late_stale_snap, &outgoing).await.unwrap();
        assert_eq!(read_vp(running.panel_snapshot(2).as_deref().unwrap()), (1, 0, 20));
        let resync_1 = wire.try_recv().unwrap();
        let mut resync_1_snap = Envelope::new(
            running.identity,
            5,
            3,
            envelope::Body::Snapshot(make_snap(3, 1, 0, 20, 0, "resynced")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        resync_1_snap.in_reply_to = resync_1.request_id;
        running.frame(resync_1_snap, &outgoing).await.unwrap();
        assert_eq!(read_vp(running.panel_snapshot(2).as_deref().unwrap()), (1, 0, 20));

        // Still blocks ancestor Replace(Column) carrying the pruned vp=2, total_rows=100!
        let stale_ancestor_patch = Envelope::new(
            running.identity,
            6,
            4,
            envelope::Body::Patch(panel::PanelPatch {
                base_revision: 3,
                revision: 4,
                operations: vec![panel::PatchOperation {
                    node_id: 1,
                    change: Some(panel::patch_operation::Change::Replace(
                        make_snap(4, 2, 50, 100, 0, "stale-ancestor").root.unwrap(),
                    )),
                }],
            }),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(stale_ancestor_patch, &outgoing).await.unwrap();
        assert_eq!(read_vp(running.panel_snapshot(2).as_deref().unwrap()), (1, 0, 20));
        assert!(running.is_panel_semantic_drifted(2));
        let resync_2 = wire.try_recv().unwrap();
        assert!(matches!(resync_2.body, Some(envelope::Body::Resync(_))));
    }

    // Part B (P2-②): Child panel has bare-Acked Viewport(2, offset=50); unrelated Button Click times out:
    //   - B1: Resync finishes with same-revision bare Ack
    //   - B2: Resync finishes with non-table Patch(Replace(Text))
    // In BOTH cases, subsequent async Change::Table(vp=2, offset=50) Patch MUST succeed with zero drift!
    for resync_uses_text_patch in [false, true] {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, 100, 0, "init")).await;

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
        let mut vp_ack = Envelope::new(
            running.identity,
            2,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        vp_ack.in_reply_to = vp_w.request_id;
        running.frame(vp_ack, &outgoing).await.unwrap();

        running
            .control(
                Control::Event {
                    panel_id: 2,
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
        let btn_w = wire.try_recv().unwrap();
        running.requests.get_mut(&btn_w.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let _cancel_w = wire.try_recv().unwrap();
        let resync_w = wire.try_recv().unwrap();

        let (base_rev, target_rev) = if resync_uses_text_patch {
            let mut text_patch = Envelope::new(
                running.identity,
                3,
                2,
                envelope::Body::Patch(panel::PanelPatch {
                    base_revision: 1,
                    revision: 2,
                    operations: vec![panel::PatchOperation {
                        node_id: 2,
                        change: Some(panel::patch_operation::Change::Replace(panel::Node::new(
                            2,
                            "status",
                            panel::node::Kind::Text(panel::Text {
                                text: "resynced-text".into(),
                                tone: panel::Tone::Foreground as i32,
                            }),
                        ))),
                    }],
                }),
                MAX_OPERATION,
            )
            .with_panel_id(2);
            text_patch.in_reply_to = resync_w.request_id;
            running.frame(text_patch, &outgoing).await.unwrap();
            (2, 3)
        } else {
            let mut ack = Envelope::new(
                running.identity,
                3,
                1,
                envelope::Body::Ack(panel::Empty {}),
                MAX_OPERATION,
            )
            .with_panel_id(2);
            ack.in_reply_to = resync_w.request_id;
            running.frame(ack, &outgoing).await.unwrap();
            (1, 2)
        };
        assert!(!running.is_panel_semantic_drifted(2));
        while updates.try_recv().is_ok() {}

        let async_target = Envelope::new(
            running.identity,
            4,
            target_rev,
            envelope::Body::Patch(panel::PanelPatch {
                base_revision: base_rev,
                revision: target_rev,
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
        )
        .with_panel_id(2);
        running.frame(async_target, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: 2,
                ref snapshot,
                ..
            }) if snapshot.revision == target_rev && read_vp(snapshot) == (2, 50, 100)
        ));
    }

    // Part C (P2-③):
    //   C1: Normal child panel Select(1) timeout settles via same-rev Resync Ack -> subsequent late
    //       same-rev bare Ack for timed-out Select(1) MUST NOT re-latch semantic drift or emit Resync!
    //   C2: Contrast: If a late higher-revision Ack(rev=2) OR late Error(Invalid, rev=1) arrives for that
    //       Resync-Ack-settled Select(1), Host STILL catches drift / callback ambiguity and emits Resync!
    //   C3: If semantic Resync completes via authoritative Snapshot(rev=2), subsequent late Ack(rev=1) AND
    //       late Error(rev=1) do NOT re-latch semantic drift!
    {
        // C1
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, 100, 0, "init")).await;

        running
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 4,
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
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(wire.try_recv().is_err());

        let mut late_sel_ack = Envelope::new(
            running.identity,
            3,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_sel_ack.in_reply_to = sel_w.request_id;
        running.frame(late_sel_ack, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(wire.try_recv().is_err());
    }

    {
        // C2a: higher-rev late Ack on Resync-Ack-settled Select still triggers semantic Resync.
        let (mut running_hi, _calls2, _updates2) = running();
        running_hi.supports_panel_multiplexing = true;
        let (out_hi, mut wire_hi) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running_hi, &out_hi, &mut wire_hi, make_snap(1, 1, 0, 100, 0, "init")).await;
        running_hi
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 4,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::SelectedOption(1)),
                    },
                },
                &out_hi,
            )
            .await
            .unwrap();
        let sel_hi = wire_hi.try_recv().unwrap();
        running_hi.requests.get_mut(&sel_hi.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running_hi.handle_timeouts(Instant::now(), &out_hi).await.unwrap();
        let _ = wire_hi.try_recv().unwrap();
        let res_hi = wire_hi.try_recv().unwrap();
        let mut ack_hi = Envelope::new(
            running_hi.identity,
            2,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        ack_hi.in_reply_to = res_hi.request_id;
        running_hi.frame(ack_hi, &out_hi).await.unwrap();

        let mut late_rev2_ack = Envelope::new(
            running_hi.identity,
            3,
            2,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_rev2_ack.in_reply_to = sel_hi.request_id;
        running_hi.frame(late_rev2_ack, &out_hi).await.unwrap();
        assert!(running_hi.is_panel_semantic_drifted(2));
        let resync_hi = wire_hi.try_recv().unwrap();
        assert!(matches!(resync_hi.body, Some(envelope::Body::Resync(_))));

        // C2b: late Error(Invalid, rev=1) on Resync-Ack-settled Select triggers semantic Resync.
        let (mut running_err, _calls3, _updates3) = running();
        running_err.supports_panel_multiplexing = true;
        let (out_err, mut wire_err) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running_err, &out_err, &mut wire_err, make_snap(1, 1, 0, 100, 0, "init")).await;
        running_err
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 4,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::SelectedOption(1)),
                    },
                },
                &out_err,
            )
            .await
            .unwrap();
        let sel_err = wire_err.try_recv().unwrap();
        running_err.requests.get_mut(&sel_err.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running_err.handle_timeouts(Instant::now(), &out_err).await.unwrap();
        let _ = wire_err.try_recv().unwrap();
        let res_err = wire_err.try_recv().unwrap();
        let mut ack_err = Envelope::new(
            running_err.identity,
            2,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        ack_err.in_reply_to = res_err.request_id;
        running_err.frame(ack_err, &out_err).await.unwrap();

        let mut late_cb_err = Envelope::new(
            running_err.identity,
            3,
            1,
            envelope::Body::Error(Failure::new(ErrorCode::Invalid, "rejected")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_cb_err.in_reply_to = sel_err.request_id;
        running_err.frame(late_cb_err, &out_err).await.unwrap();
        assert!(running_err.is_panel_semantic_drifted(2));
        let resync_cb = wire_err.try_recv().unwrap();
        assert!(matches!(resync_cb.body, Some(envelope::Body::Resync(_))));

        // C3: Full authoritative Snapshot(rev=2) on Resync converges timed-out Select completely so
        //     subsequent late same-rev Error(Invalid) does NOT re-latch semantic drift!
        let (mut running_snap, _calls4, _updates4) = running();
        running_snap.supports_panel_multiplexing = true;
        let (out_snap, mut wire_snap) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running_snap, &out_snap, &mut wire_snap, make_snap(1, 1, 0, 100, 0, "init")).await;
        running_snap
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 4,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::SelectedOption(1)),
                    },
                },
                &out_snap,
            )
            .await
            .unwrap();
        let sel_snap = wire_snap.try_recv().unwrap();
        running_snap.requests.get_mut(&sel_snap.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running_snap.handle_timeouts(Instant::now(), &out_snap).await.unwrap();
        let _ = wire_snap.try_recv().unwrap();
        let res_snap = wire_snap.try_recv().unwrap();
        let mut resync_snap = Envelope::new(
            running_snap.identity,
            2,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, 0, 100, 1, "converged")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        resync_snap.in_reply_to = res_snap.request_id;
        running_snap.frame(resync_snap, &out_snap).await.unwrap();
        assert!(!running_snap.is_panel_semantic_drifted(2));
        assert!(wire_snap.try_recv().is_err());

        let mut late_snap_err = Envelope::new(
            running_snap.identity,
            3,
            1,
            envelope::Body::Error(Failure::new(ErrorCode::Invalid, "late-rejected")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_snap_err.in_reply_to = sel_snap.request_id;
        running_snap.frame(late_snap_err, &out_snap).await.unwrap();
        assert!(!running_snap.is_panel_semantic_drifted(2));
        assert!(wire_snap.try_recv().is_err());
    }
}

#[tokio::test]
async fn clamped_resync_pruned_vp_detached_deadline_and_pre_recovery_late_tree() {
    let make_snap = |rev: u64, vp_rev: u64, offset: u64, total_rows: u64, sel_idx: u32, text: &str| {
        PanelSnapshot {
            revision: rev,
            title: "round55-regression".into(),
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
                            "sel",
                            panel::node::Kind::Select(panel::SelectNode {
                                label: "Mode".into(),
                                options: vec!["A".into(), "B".into()],
                                selected_index: sel_idx,
                                enabled: true,
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
        }
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
        panel::node::Kind::VirtualTable(t) => (t.viewport_revision, t.offset, t.total_rows),
        other => panic!("expected table, got {other:?}"),
    };
    async fn open_child(
        running: &mut Running,
        outgoing: &mpsc::Sender<Envelope>,
        wire: &mut mpsc::Receiver<Envelope>,
        snap: PanelSnapshot,
    ) {
        let rev = snap.revision;
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
                outgoing,
            )
            .await
            .unwrap();
        let o_w = wire.try_recv().unwrap();
        let mut o_s = Envelope::new(
            running.identity,
            1,
            rev,
            envelope::Body::Snapshot(snap),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        o_s.in_reply_to = o_w.request_id;
        running.frame(o_s, outgoing).await.unwrap();
        o_rx.await.unwrap().unwrap();
    }

    // Part A (P2-①): After Snapshot(rev=2, vp=1, total_rows=20) prunes (vp=2, offset=50) and Resync
    // clamps vp=2 to offset=0 (Snapshot rev=3, vp=2, offset=0, total_rows=20), stale Change::Table(vp=2,
    // offset=50), unsolicited Snapshot(vp=2, offset=50), AND ancestor Replace(Column { vp=2, offset=50 })
    // MUST STILL all be rejected while valid Change::Table(vp=2, offset=0, total_rows=25) succeeds!
    {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, 100, 0, "init")).await;

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
        let mut vp_ack = Envelope::new(
            running.identity,
            2,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        vp_ack.in_reply_to = vp_w.request_id;
        running.frame(vp_ack, &outgoing).await.unwrap();

        let shrink_snap = Envelope::new(
            running.identity,
            3,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, 0, 20, 0, "shrunk")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(shrink_snap, &outgoing).await.unwrap();

        let stale_snap = Envelope::new(
            running.identity,
            4,
            3,
            envelope::Body::Snapshot(make_snap(3, 2, 50, 100, 0, "stale")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(stale_snap, &outgoing).await.unwrap();
        let resync_1 = wire.try_recv().unwrap();
        let mut clamped_resync = Envelope::new(
            running.identity,
            5,
            3,
            envelope::Body::Snapshot(make_snap(3, 2, 0, 20, 0, "clamped")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        clamped_resync.in_reply_to = resync_1.request_id;
        running.frame(clamped_resync, &outgoing).await.unwrap();
        assert_eq!(read_vp(running.panel_snapshot(2).as_deref().unwrap()), (2, 0, 20));
        assert!(!running.is_panel_semantic_drifted(2));

        // 1) Stale Change::Table(vp=2, offset=50, total_rows=100) is REJECTED even after clamp.
        let stale_patch = Envelope::new(
            running.identity,
            6,
            4,
            envelope::Body::Patch(panel::PanelPatch {
                base_revision: 3,
                revision: 4,
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
        )
        .with_panel_id(2);
        running.frame(stale_patch, &outgoing).await.unwrap();
        assert_eq!(read_vp(running.panel_snapshot(2).as_deref().unwrap()), (2, 0, 20));
        assert!(running.is_panel_semantic_drifted(2));
        let resync_2 = wire.try_recv().unwrap();
        let mut clamped_resync_2 = Envelope::new(
            running.identity,
            7,
            4,
            envelope::Body::Snapshot(make_snap(4, 2, 0, 20, 0, "clamped-2")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        clamped_resync_2.in_reply_to = resync_2.request_id;
        running.frame(clamped_resync_2, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));

        // 2) Stale unsolicited Snapshot(vp=2, offset=50, total_rows=100) is ALSO REJECTED.
        let stale_snap_2 = Envelope::new(
            running.identity,
            8,
            5,
            envelope::Body::Snapshot(make_snap(5, 2, 50, 100, 0, "stale-2")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(stale_snap_2, &outgoing).await.unwrap();
        assert_eq!(read_vp(running.panel_snapshot(2).as_deref().unwrap()), (2, 0, 20));
        let resync_3 = wire.try_recv().unwrap();
        let mut clamped_resync_3 = Envelope::new(
            running.identity,
            9,
            5,
            envelope::Body::Snapshot(make_snap(5, 2, 0, 20, 0, "clamped-3")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        clamped_resync_3.in_reply_to = resync_3.request_id;
        running.frame(clamped_resync_3, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));

        // 3) Valid live Change::Table at clamped (vp=2, offset=0, total_rows=25) succeeds!
        let valid_clamped_patch = Envelope::new(
            running.identity,
            10,
            6,
            envelope::Body::Patch(panel::PanelPatch {
                base_revision: 5,
                revision: 6,
                operations: vec![panel::PatchOperation {
                    node_id: 3,
                    change: Some(panel::patch_operation::Change::Table(panel::VirtualTable {
                        columns: vec![panel::TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 80.0,
                        }],
                        total_rows: 25,
                        offset: 0,
                        viewport_revision: 2,
                        rows: vec![],
                        multi_select: false,
                    })),
                }],
            }),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(valid_clamped_patch, &outgoing).await.unwrap();
        assert_eq!(read_vp(running.panel_snapshot(2).as_deref().unwrap()), (2, 0, 25));
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(wire.try_recv().is_err());
    }

    // Part B (P2-②): Detached Viewport settled at old base watermark by same-rev Resync Ack ->
    //   B1: late pre-dispatch Error(Deadline) MUST NOT latch semantic drift!
    //   B2: contrast: late Error(Invalid) on that detached Viewport STILL latches semantic drift!
    //   B3: contrast: late Error(Deadline) on a PROMOTED Select settled by same-rev Resync Ack
    //       STILL latches semantic drift!
    {
        // B1
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, 100, 0, "init")).await;
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
        // Detach vp_w via unsolicited Snapshot(rev=2, vp=1).
        let unsol = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, 0, 100, 0, "status-changed")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(unsol, &outgoing).await.unwrap();

        running.requests.get_mut(&vp_w.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let _cancel_w = wire.try_recv().unwrap();
        let resync_w = wire.try_recv().unwrap();
        let mut resync_ack = Envelope::new(
            running.identity,
            3,
            2,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        resync_ack.in_reply_to = resync_w.request_id;
        running.frame(resync_ack, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(wire.try_recv().is_err());

        let mut late_deadline = Envelope::new(
            running.identity,
            4,
            2,
            envelope::Body::Error(Failure::new(ErrorCode::Deadline, "pre-dispatch deadline")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_deadline.in_reply_to = vp_w.request_id;
        running.frame(late_deadline, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(wire.try_recv().is_err());
    }
    {
        // B2: callback Error(Invalid) on detached settled Viewport still drifts.
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, 100, 0, "init")).await;
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
        let unsol = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, 0, 100, 0, "status-changed")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(unsol, &outgoing).await.unwrap();
        running.requests.get_mut(&vp_w.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let _ = wire.try_recv().unwrap();
        let resync_w = wire.try_recv().unwrap();
        let mut resync_ack = Envelope::new(
            running.identity,
            3,
            2,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        resync_ack.in_reply_to = resync_w.request_id;
        running.frame(resync_ack, &outgoing).await.unwrap();

        let mut late_invalid = Envelope::new(
            running.identity,
            4,
            2,
            envelope::Body::Error(Failure::new(ErrorCode::Invalid, "callback err")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_invalid.in_reply_to = vp_w.request_id;
        running.frame(late_invalid, &outgoing).await.unwrap();
        assert!(running.is_panel_semantic_drifted(2));
        let resync_cb = wire.try_recv().unwrap();
        assert!(matches!(resync_cb.body, Some(envelope::Body::Resync(_))));
    }
    {
        // B3: Promoted Select(1) settled by same-rev Resync Ack + late Error(Deadline) STILL drifts!
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, 100, 0, "init")).await;
        running
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 4,
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
        let _ = wire.try_recv().unwrap();
        let resync_w = wire.try_recv().unwrap();
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

        let mut late_deadline = Envelope::new(
            running.identity,
            3,
            1,
            envelope::Body::Error(Failure::new(ErrorCode::Deadline, "pre-dispatch deadline")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_deadline.in_reply_to = sel_w.request_id;
        running.frame(late_deadline, &outgoing).await.unwrap();
        assert!(running.is_panel_semantic_drifted(2));
        let resync_promo = wire.try_recv().unwrap();
        assert!(matches!(resync_promo.body, Some(envelope::Body::Resync(_))));
    }

    // Part C (P2-③): Full authoritative Resync Snapshot(rev=3) converges timed-out Select / Click / Resync:
    //   C1: Late pre-recovery Snapshot(rev=2 < 3) AND Patch(rev=2 < 3) do NOT re-latch semantic drift!
    //   C2: Contrast: late same-or-higher Snapshot(rev=3 >= 3) STILL triggers semantic Resync!
    //   C3: Cancelled Resync #1 followed by Resync #2 Snapshot(rev=3): late Snapshot(rev=2 < 3) for Resync #1
    //       does NOT mark semantic drift, while Snapshot(rev=3 >= 3) DOES!
    for send_patch in [false, true] {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, 100, 0, "init")).await;
        running
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 4,
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
        let _ = wire.try_recv().unwrap();
        let resync_w = wire.try_recv().unwrap();
        let mut resync_snap = Envelope::new(
            running.identity,
            2,
            3,
            envelope::Body::Snapshot(make_snap(3, 1, 0, 100, 1, "recovered")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        resync_snap.in_reply_to = resync_w.request_id;
        running.frame(resync_snap, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(wire.try_recv().is_err());

        let mut late_older_tree = Envelope::new(
            running.identity,
            3,
            2,
            if send_patch {
                envelope::Body::Patch(panel::PanelPatch {
                    base_revision: 1,
                    revision: 2,
                    operations: vec![panel::PatchOperation {
                        node_id: 2,
                        change: Some(panel::patch_operation::Change::Replace(panel::Node::new(
                            2,
                            "status",
                            panel::node::Kind::Text(panel::Text {
                                text: "old-patch".into(),
                                tone: panel::Tone::Foreground as i32,
                            }),
                        ))),
                    }],
                })
            } else {
                envelope::Body::Snapshot(make_snap(2, 1, 0, 100, 0, "old-snap"))
            },
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_older_tree.in_reply_to = sel_w.request_id;
        running.frame(late_older_tree, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(wire.try_recv().is_err());

        // Interactive Click on panel 2 remains unblocked!
        running
            .control(
                Control::Event {
                    panel_id: 2,
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
        assert!(wire.try_recv().is_ok());
    }
    {
        // C2: Same-revision Snapshot(rev=3) STILL triggers semantic Resync.
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, 100, 0, "init")).await;
        running
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 4,
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
        let _ = wire.try_recv().unwrap();
        let resync_w = wire.try_recv().unwrap();
        let mut resync_snap = Envelope::new(
            running.identity,
            2,
            3,
            envelope::Body::Snapshot(make_snap(3, 1, 0, 100, 1, "recovered")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        resync_snap.in_reply_to = resync_w.request_id;
        running.frame(resync_snap, &outgoing).await.unwrap();

        let mut late_same_rev_snap = Envelope::new(
            running.identity,
            3,
            3,
            envelope::Body::Snapshot(make_snap(3, 1, 0, 100, 0, "diverged-same-rev")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_same_rev_snap.in_reply_to = sel_w.request_id;
        running.frame(late_same_rev_snap, &outgoing).await.unwrap();
        assert!(running.is_panel_semantic_drifted(2));
        let resync_next = wire.try_recv().unwrap();
        assert!(matches!(resync_next.body, Some(envelope::Body::Resync(_))));
    }
    {
        // C3: Cancelled Resync #1 followed by Resync #2 Snapshot(rev=3):
        //     rev=2 < 3 does NOT mark drift; contrast rev=3 >= 3 DOES!
        for late_rev in [2u64, 3u64] {
            let (mut running, _calls, _updates) = running();
            running.supports_panel_multiplexing = true;
            let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
            open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, 100, 0, "init")).await;
            running
                .control(
                    Control::Event {
                        panel_id: 2,
                        expected_snapshot: None,
                        event_seq: 1,
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
            let btn_w = wire.try_recv().unwrap();
            running.requests.get_mut(&btn_w.request_id).unwrap().deadline =
                Instant::now() - Duration::from_secs(1);
            running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
            let _ = wire.try_recv().unwrap();
            let resync_1 = wire.try_recv().unwrap();

            running.requests.get_mut(&resync_1.request_id).unwrap().deadline =
                Instant::now() - Duration::from_secs(1);
            running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
            let _ = wire.try_recv().unwrap();
            let resync_2 = wire.try_recv().unwrap();

            let mut resync2_snap = Envelope::new(
                running.identity,
                2,
                3,
                envelope::Body::Snapshot(make_snap(3, 1, 0, 100, 0, "recovered-2")),
                MAX_OPERATION,
            )
            .with_panel_id(2);
            resync2_snap.in_reply_to = resync_2.request_id;
            running.frame(resync2_snap, &outgoing).await.unwrap();
            assert!(!running.is_panel_semantic_drifted(2));

            let mut late_resync1 = Envelope::new(
                running.identity,
                3,
                late_rev,
                envelope::Body::Snapshot(make_snap(late_rev, 1, 0, 100, 0, "old-resync-1")),
                MAX_OPERATION,
            )
            .with_panel_id(2);
            late_resync1.in_reply_to = resync_1.request_id;
            running.frame(late_resync1, &outgoing).await.unwrap();
            assert_eq!(running.is_panel_semantic_drifted(2), late_rev >= 3);
            if late_rev >= 3 {
                let fresh_resync = wire.try_recv().unwrap();
                assert!(matches!(fresh_resync.body, Some(envelope::Body::Resync(_))));
            } else {
                assert!(wire.try_recv().is_err());
            }
        }
    }
}

#[tokio::test]
async fn resync_clamp_rejects_old_offset_cancel_notif_honors_recovery_rev_and_recovered_resync_restarts() {
    let make_snap = |rev: u64, vp_rev: u64, offset: u64, total_rows: u64, sel_idx: u32, text: &str| {
        PanelSnapshot {
            revision: rev,
            title: "round56-regression".into(),
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
                            "sel",
                            panel::node::Kind::Select(panel::SelectNode {
                                label: "Mode".into(),
                                options: vec!["A".into(), "B".into()],
                                selected_index: sel_idx,
                                enabled: true,
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
        }
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
        panel::node::Kind::VirtualTable(t) => (t.viewport_revision, t.offset, t.total_rows),
        other => panic!("expected table, got {other:?}"),
    };
    async fn open_child(
        running: &mut Running,
        outgoing: &mpsc::Sender<Envelope>,
        wire: &mut mpsc::Receiver<Envelope>,
        snap: PanelSnapshot,
    ) {
        let rev = snap.revision;
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
                outgoing,
            )
            .await
            .unwrap();
        let o_w = wire.try_recv().unwrap();
        let mut o_s = Envelope::new(
            running.identity,
            1,
            rev,
            envelope::Body::Snapshot(snap),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        o_s.in_reply_to = o_w.request_id;
        running.frame(o_s, outgoing).await.unwrap();
        o_rx.await.unwrap().unwrap();
    }

    // Part A (P2-①): After clamped Resync(vp=2, offset=0, total_rows=20), a second Resync Snapshot
    // trying to restore pruned (vp=2, offset=50, total_rows=100) MUST STILL be rejected and trigger
    // retry Resync #3, while clamped (vp=2, offset=0, total_rows=20) on Resync #3 succeeds and STILL
    // rejects subsequent stale Change::Table(vp=2, offset=50, total_rows=100)!
    {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, 100, 0, "init")).await;

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
        let mut vp_ack = Envelope::new(
            running.identity,
            2,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        vp_ack.in_reply_to = vp_w.request_id;
        running.frame(vp_ack, &outgoing).await.unwrap();

        let shrink_snap = Envelope::new(
            running.identity,
            3,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, 0, 20, 0, "shrunk")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(shrink_snap, &outgoing).await.unwrap();

        let stale_snap = Envelope::new(
            running.identity,
            4,
            3,
            envelope::Body::Snapshot(make_snap(3, 2, 50, 100, 0, "stale")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(stale_snap, &outgoing).await.unwrap();
        let resync_1 = wire.try_recv().unwrap();
        let mut clamped_resync_1 = Envelope::new(
            running.identity,
            5,
            3,
            envelope::Body::Snapshot(make_snap(3, 2, 0, 20, 0, "clamped-1")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        clamped_resync_1.in_reply_to = resync_1.request_id;
        running.frame(clamped_resync_1, &outgoing).await.unwrap();
        assert_eq!(read_vp(running.panel_snapshot(2).as_deref().unwrap()), (2, 0, 20));

        // Trigger Resync #2 via stale Change::Table(vp=2, offset=50).
        let stale_patch_1 = Envelope::new(
            running.identity,
            6,
            4,
            envelope::Body::Patch(panel::PanelPatch {
                base_revision: 3,
                revision: 4,
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
        )
        .with_panel_id(2);
        running.frame(stale_patch_1, &outgoing).await.unwrap();
        let resync_2 = wire.try_recv().unwrap();

        // Worker replies to Resync #2 with a stale Snapshot carrying the pruned (vp=2, offset=50, total_rows=100):
        // MUST be REJECTED and schedule Resync #3 without clearing isolation!
        let mut stale_resync_2 = Envelope::new(
            running.identity,
            7,
            4,
            envelope::Body::Snapshot(make_snap(4, 2, 50, 100, 0, "stale-resync-2")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        stale_resync_2.in_reply_to = resync_2.request_id;
        running.frame(stale_resync_2, &outgoing).await.unwrap();
        assert_eq!(read_vp(running.panel_snapshot(2).as_deref().unwrap()), (2, 0, 20));
        assert!(running.is_panel_semantic_drifted(2));
        let resync_3 = wire.try_recv().unwrap();
        assert!(matches!(resync_3.body, Some(envelope::Body::Resync(_))));

        // Resync #3 recovers with clamped (vp=2, offset=0, total_rows=20), and STILL blocks stale (vp=2, offset=50)!
        let mut clamped_resync_3 = Envelope::new(
            running.identity,
            8,
            4,
            envelope::Body::Snapshot(make_snap(4, 2, 0, 20, 0, "clamped-3")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        clamped_resync_3.in_reply_to = resync_3.request_id;
        running.frame(clamped_resync_3, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(wire.try_recv().is_err());

        let stale_patch_after = Envelope::new(
            running.identity,
            9,
            5,
            envelope::Body::Patch(panel::PanelPatch {
                base_revision: 4,
                revision: 5,
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
        )
        .with_panel_id(2);
        running.frame(stale_patch_after, &outgoing).await.unwrap();
        assert_eq!(read_vp(running.panel_snapshot(2).as_deref().unwrap()), (2, 0, 20));
        assert!(running.is_panel_semantic_drifted(2));
        assert!(matches!(wire.try_recv().unwrap().body, Some(envelope::Body::Resync(_))));
    }

    // Part B (P2-②): Late pre-recovery Snapshot(rev=2) / Patch(rev=2) replying to Cancel notification
    // MUST NOT latch drift after Resync recovered with Snapshot(rev=3), whereas rev=3 >= 3 MUST!
    for reply_kind in ["patch_rev2", "snap_rev2", "snap_rev3"] {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, 100, 0, "init")).await;

        running
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 4,
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
        let cancel_w = wire.try_recv().unwrap();
        assert!(matches!(cancel_w.body, Some(envelope::Body::Cancel(_))));
        let resync_w = wire.try_recv().unwrap();

        let mut resync_snap = Envelope::new(
            running.identity,
            2,
            3,
            envelope::Body::Snapshot(make_snap(3, 1, 0, 100, 1, "recovered")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        resync_snap.in_reply_to = resync_w.request_id;
        running.frame(resync_snap, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(wire.try_recv().is_err());

        let mut late_cancel_reply = match reply_kind {
            "patch_rev2" => Envelope::new(
                running.identity,
                3,
                2,
                envelope::Body::Patch(panel::PanelPatch {
                    base_revision: 1,
                    revision: 2,
                    operations: vec![panel::PatchOperation {
                        node_id: 2,
                        change: Some(panel::patch_operation::Change::Replace(panel::Node::new(
                            2,
                            "status",
                            panel::node::Kind::Text(panel::Text {
                                text: "old-cancel-patch".into(),
                                tone: panel::Tone::Foreground as i32,
                            }),
                        ))),
                    }],
                }),
                MAX_OPERATION,
            ),
            "snap_rev2" => Envelope::new(
                running.identity,
                3,
                2,
                envelope::Body::Snapshot(make_snap(2, 1, 0, 100, 0, "old-cancel-snap")),
                MAX_OPERATION,
            ),
            _ => Envelope::new(
                running.identity,
                3,
                3,
                envelope::Body::Snapshot(make_snap(3, 1, 0, 100, 0, "new-cancel-snap")),
                MAX_OPERATION,
            ),
        }
        .with_panel_id(2);
        late_cancel_reply.in_reply_to = cancel_w.request_id;
        running.frame(late_cancel_reply, &outgoing).await.unwrap();

        let expect_drift = reply_kind == "snap_rev3";
        assert_eq!(running.is_panel_semantic_drifted(2), expect_drift);
        if expect_drift {
            assert!(matches!(wire.try_recv().unwrap().body, Some(envelope::Body::Resync(_))));
        } else {
            assert!(wire.try_recv().is_err());
        }
    }

    // Part C (P2-③): When Resync #1 times out and Resync #2 recovers (either via Snapshot(rev=3) OR via
    // same-rev Ack(rev=1)), a subsequent drift-worthy reply to Resync #1 OR Cancel #1 MUST schedule and
    // emit a fresh Resync automatically so the panel can recover and accept new interactions!
    {
        // C1: Recovered via Snapshot(rev=3) -> late Snapshot(rev=3) on Resync #1 emits fresh Resync -> recovers cleanly!
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, 100, 0, "init")).await;

        running
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 1,
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
        let btn_w = wire.try_recv().unwrap();
        running.requests.get_mut(&btn_w.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let _ = wire.try_recv().unwrap();
        let resync_1 = wire.try_recv().unwrap();

        running.requests.get_mut(&resync_1.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let _cancel_1 = wire.try_recv().unwrap();
        let resync_2 = wire.try_recv().unwrap();

        let mut resync2_snap = Envelope::new(
            running.identity,
            2,
            3,
            envelope::Body::Snapshot(make_snap(3, 1, 0, 100, 0, "recovered-2")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        resync2_snap.in_reply_to = resync_2.request_id;
        running.frame(resync2_snap, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));

        let mut late_resync1_snap = Envelope::new(
            running.identity,
            3,
            3,
            envelope::Body::Snapshot(make_snap(3, 1, 0, 100, 0, "late-resync1-drift")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_resync1_snap.in_reply_to = resync_1.request_id;
        running.frame(late_resync1_snap, &outgoing).await.unwrap();
        assert!(running.is_panel_semantic_drifted(2));
        let resync_3 = wire.try_recv().unwrap();
        assert!(matches!(resync_3.body, Some(envelope::Body::Resync(_))));

        let mut resync3_snap = Envelope::new(
            running.identity,
            4,
            4,
            envelope::Body::Snapshot(make_snap(4, 1, 0, 100, 0, "recovered-3")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        resync3_snap.in_reply_to = resync_3.request_id;
        running.frame(resync3_snap, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));

        running
            .control(
                Control::Event {
                    panel_id: 2,
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
        assert!(wire.try_recv().is_ok());
    }
    {
        // C2: Recovered via same-rev Ack(rev=1) -> late Snapshot(rev=2) on Cancel #1 OR Resync #1 emits fresh Resync!
        for target_cancel_frame in [false, true] {
            let (mut r, _c, _u) = running();
            r.supports_panel_multiplexing = true;
            let (out, mut w) = mpsc::channel(OUTGOING_CAPACITY);
            open_child(&mut r, &out, &mut w, make_snap(1, 1, 0, 100, 0, "init")).await;

            r.control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 5,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Click(panel::Empty {})),
                    },
                },
                &out,
            )
            .await
            .unwrap();
            let bw = w.try_recv().unwrap();
            r.requests.get_mut(&bw.request_id).unwrap().deadline =
                Instant::now() - Duration::from_secs(1);
            r.handle_timeouts(Instant::now(), &out).await.unwrap();
            let _ = w.try_recv().unwrap();
            let r1 = w.try_recv().unwrap();

            r.requests.get_mut(&r1.request_id).unwrap().deadline =
                Instant::now() - Duration::from_secs(1);
            r.handle_timeouts(Instant::now(), &out).await.unwrap();
            let c1 = w.try_recv().unwrap();
            let r2 = w.try_recv().unwrap();

            let mut r2_ack = Envelope::new(
                r.identity,
                2,
                1,
                envelope::Body::Ack(panel::Empty {}),
                MAX_OPERATION,
            )
            .with_panel_id(2);
            r2_ack.in_reply_to = r2.request_id;
            r.frame(r2_ack, &out).await.unwrap();
            assert!(!r.is_panel_semantic_drifted(2));

            let mut late = Envelope::new(
                r.identity,
                3,
                2,
                envelope::Body::Snapshot(make_snap(2, 1, 0, 100, 0, "late-drift")),
                MAX_OPERATION,
            )
            .with_panel_id(2);
            late.in_reply_to = if target_cancel_frame {
                c1.request_id
            } else {
                r1.request_id
            };
            r.frame(late, &out).await.unwrap();
            assert!(r.is_panel_semantic_drifted(2));
            assert!(matches!(w.try_recv().unwrap().body, Some(envelope::Body::Resync(_))));
        }
    }
}

#[tokio::test]
async fn unsolicited_snapshot_updates_exhausted_resync_evidence_and_exhaustion_clears_old_restart_eligibility() {
    let make_snap = |rev: u64, text: &str| PanelSnapshot {
        revision: rev,
        title: "round57-regression".into(),
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
    async fn open_child(
        running: &mut Running,
        outgoing: &mpsc::Sender<Envelope>,
        wire: &mut mpsc::Receiver<Envelope>,
        snap: PanelSnapshot,
    ) {
        let rev = snap.revision;
        let (o_tx, o_rx) = oneshot::channel();
        running
            .control(
                Control::Open {
                    panel_id: 2,
                    id: Uuid::new_v4(),
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: o_tx,
                },
                outgoing,
            )
            .await
            .unwrap();
        let o_w = wire.try_recv().unwrap();
        let mut o_s = Envelope::new(
            running.identity,
            1,
            rev,
            envelope::Body::Snapshot(snap),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        o_s.in_reply_to = o_w.request_id;
        running.frame(o_s, outgoing).await.unwrap();
        o_rx.await.unwrap().unwrap();
    }

    // Part A (P2-①): After Resync #1..#3 exhaust, an unsolicited full Snapshot(rev=5) recovers
    // the child panel and updates converged_snapshot_rev & recovered_by_resync on earlier
    // cancelled Resync and CancelNotification entries:
    //   A1: Delayed pre-recovery Snapshot(rev=2 < 5) replying to resync_1 AND cancel_1 MUST NOT
    //       latch semantic drift or emit Resync; panel remains interactive!
    //   A2: Conflicting newer Snapshot(rev=5 >= 5) replying to resync_2 STILL latches semantic drift
    //       AND automatically emits a fresh Resync (recovering cleanly on Snapshot(rev=6))!
    {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, "init")).await;

        running.schedule_panel_resync(2);
        running.flush_pending_resyncs(&outgoing).unwrap();
        let mut resync_ids = Vec::new();
        let mut cancel_ids = Vec::new();
        for _ in 0..MAX_RESYNC_RETRIES {
            let r = wire.try_recv().unwrap();
            resync_ids.push(r.request_id);
            running.requests.get_mut(&r.request_id).unwrap().deadline =
                Instant::now() - Duration::from_secs(1);
            running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
            let c = wire.try_recv().unwrap();
            cancel_ids.push(c.request_id);
        }
        assert!(!running.is_panel_resyncing(2));
        assert!(wire.try_recv().is_err());

        let unsol_recover = Envelope::new(
            running.identity,
            2,
            5,
            envelope::Body::Snapshot(make_snap(5, "unsolicited-recovered")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(unsol_recover, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(!running.is_panel_resyncing(2));

        // A1: Older Snapshot(rev=2 < 5) on resync_1 and cancel_1 does not lock drift.
        let mut late_r1_old = Envelope::new(
            running.identity,
            3,
            2,
            envelope::Body::Snapshot(make_snap(2, "old-r1")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_r1_old.in_reply_to = resync_ids[0];
        running.frame(late_r1_old, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(wire.try_recv().is_err());

        let mut late_c1_old = Envelope::new(
            running.identity,
            4,
            2,
            envelope::Body::Snapshot(make_snap(2, "old-c1")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_c1_old.in_reply_to = cancel_ids[0];
        running.frame(late_c1_old, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(wire.try_recv().is_err());

        running
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Click(panel::Empty {})),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let btn_w = wire.try_recv().unwrap();
        let mut btn_ack = Envelope::new(
            running.identity,
            5,
            5,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        btn_ack.in_reply_to = btn_w.request_id;
        running.frame(btn_ack, &outgoing).await.unwrap();

        // A2: Snapshot(rev=5 >= 5) on resync_2 catches genuine drift and emits a fresh Resync!
        let mut late_r2_drift = Envelope::new(
            running.identity,
            6,
            5,
            envelope::Body::Snapshot(make_snap(5, "drift-r2")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_r2_drift.in_reply_to = resync_ids[1];
        running.frame(late_r2_drift, &outgoing).await.unwrap();
        assert!(running.is_panel_semantic_drifted(2));
        let fresh_r = wire.try_recv().unwrap();
        assert!(matches!(fresh_r.body, Some(envelope::Body::Resync(_))));

        let mut fresh_snap = Envelope::new(
            running.identity,
            7,
            6,
            envelope::Body::Snapshot(make_snap(6, "resynced-after-unsol")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        fresh_snap.in_reply_to = fresh_r.request_id;
        running.frame(fresh_snap, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
    }

    // Part B (P2-②): Two earlier cancelled Resyncs (resync_0, resync_1) plus cancel_1 recover via
    // Snapshot(rev=3), then resync_1's late Snapshot(rev=3) triggers Round 2 which EXHAUSTS all
    // MAX_RESYNC_RETRIES. Subsequent late replies to cancel_1 OR resync_0 MUST ONLY latch semantic
    // drift and MUST NOT restart an exhausted Resync budget (on both main and child panels)!
    for target_panel in [protocol::PANEL_ID, 2u64] {
        for late_target in ["cancel_1_snap", "resync_0_snap", "cancel_1_high_rev_ack"] {
            let (mut running, _calls, _updates) = running();
            running.supports_panel_multiplexing = true;
            *running.main_panel_mut() = PanelState::new(make_snap(1, "main-init")).unwrap();
            let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
            if target_panel != protocol::PANEL_ID {
                open_child(&mut running, &outgoing, &mut wire, make_snap(1, "child-init")).await;
            }
            let mut seq = if target_panel == protocol::PANEL_ID {
                1u64
            } else {
                2u64
            };
            let mut next_seq = || {
                let s = seq;
                seq += 1;
                s
            };

            // Round 1: Resync #0 and Resync #1 time out; Resync #2 recovers cleanly with Snapshot(rev=3).
            running.schedule_panel_resync(target_panel);
            running.flush_pending_resyncs(&outgoing).unwrap();
            let r0 = wire.try_recv().unwrap();
            running.requests.get_mut(&r0.request_id).unwrap().deadline =
                Instant::now() - Duration::from_secs(1);
            running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
            let _c0 = wire.try_recv().unwrap();
            let r1 = wire.try_recv().unwrap();
            running.requests.get_mut(&r1.request_id).unwrap().deadline =
                Instant::now() - Duration::from_secs(1);
            running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
            let c1 = wire.try_recv().unwrap();
            let r2 = wire.try_recv().unwrap();

            let mut r2_snap = Envelope::new(
                running.identity,
                next_seq(),
                3,
                envelope::Body::Snapshot(make_snap(3, "round1-recovered")),
                MAX_OPERATION,
            )
            .with_panel_id(target_panel);
            r2_snap.in_reply_to = r2.request_id;
            running.frame(r2_snap, &outgoing).await.unwrap();
            assert!(!running.is_panel_semantic_drifted(target_panel));
            assert!(wire.try_recv().is_err());

            // r1's late Snapshot(rev=3) triggers Round 2, clearing restart eligibility on older
            // entries (r0, c1), and Round 2 exhausts all MAX_RESYNC_RETRIES.
            let mut r1_late = Envelope::new(
                running.identity,
                next_seq(),
                3,
                envelope::Body::Snapshot(make_snap(3, "round1-late-drift")),
                MAX_OPERATION,
            )
            .with_panel_id(target_panel);
            r1_late.in_reply_to = r1.request_id;
            running.frame(r1_late, &outgoing).await.unwrap();
            for _ in 0..MAX_RESYNC_RETRIES {
                let rr = wire.try_recv().unwrap();
                running.requests.get_mut(&rr.request_id).unwrap().deadline =
                    Instant::now() - Duration::from_secs(1);
                running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
                let _cc = wire.try_recv().unwrap();
            }
            assert!(!running.is_panel_resyncing(target_panel));
            assert!(wire.try_recv().is_err());

            // Now late reply to remaining eligible-looking cancel_1 OR resync_0 arrives AFTER Round 2
            // exhausted: MUST mark semantic drift cleanly WITHOUT restarting Resync!
            let mut after_exhaust = match late_target {
                "cancel_1_high_rev_ack" => Envelope::new(
                    running.identity,
                    next_seq(),
                    4,
                    envelope::Body::Ack(panel::Empty {}),
                    MAX_OPERATION,
                ),
                _ => Envelope::new(
                    running.identity,
                    next_seq(),
                    3,
                    envelope::Body::Snapshot(make_snap(3, "post-exhaust-late")),
                    MAX_OPERATION,
                ),
            }
            .with_panel_id(target_panel);
            after_exhaust.in_reply_to = if late_target == "resync_0_snap" {
                r0.request_id
            } else {
                c1.request_id
            };
            running.frame(after_exhaust, &outgoing).await.unwrap();
            assert!(running.is_panel_semantic_drifted(target_panel));
            assert!(!running.is_panel_resyncing(target_panel));
            assert!(wire.try_recv().is_err());
        }
    }
}

#[tokio::test]
async fn exhausted_normal_cancel_cannot_restart_unsolicited_snapshot_clears_promoted_deadline_and_inflight_final_resync_keeps_drift() {
    let make_snap = |rev: u64, sel_idx: u32, text: &str| PanelSnapshot {
        revision: rev,
        title: "round58-regression".into(),
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
                        "sel",
                        panel::node::Kind::Select(panel::SelectNode {
                            label: "Mode".into(),
                            options: vec!["A".into(), "B".into()],
                            selected_index: sel_idx,
                            enabled: true,
                        }),
                    ),
                    panel::Node::new(
                        4,
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
    async fn open_child(
        running: &mut Running,
        outgoing: &mpsc::Sender<Envelope>,
        wire: &mut mpsc::Receiver<Envelope>,
        snap: PanelSnapshot,
    ) {
        let rev = snap.revision;
        let (o_tx, o_rx) = oneshot::channel();
        running
            .control(
                Control::Open {
                    panel_id: 2,
                    id: Uuid::new_v4(),
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: o_tx,
                },
                outgoing,
            )
            .await
            .unwrap();
        let o_w = wire.try_recv().unwrap();
        let mut o_s = Envelope::new(
            running.identity,
            1,
            rev,
            envelope::Body::Snapshot(snap),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        o_s.in_reply_to = o_w.request_id;
        running.frame(o_s, outgoing).await.unwrap();
        o_rx.await.unwrap().unwrap();
    }

    // Part A (P2-①): Exhausted recovery MUST clear restart eligibility on BOTH the original normal
    // CancelNotification (for_resync=false) AND the cancelled state request so neither a late higher-rev
    // Ack on cancel_btn nor a late Snapshot on btn_w can restart an exhausted Resync cycle!
    for reply_to_state_req in [false, true] {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 0, "init")).await;

        running
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 4,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Click(panel::Empty {})),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let btn_w = wire.try_recv().unwrap();
        running.requests.get_mut(&btn_w.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let cancel_btn = wire.try_recv().unwrap();
        assert!(matches!(cancel_btn.body, Some(envelope::Body::Cancel(_))));

        for _ in 0..MAX_RESYNC_RETRIES {
            let r = wire.try_recv().unwrap();
            assert!(matches!(r.body, Some(envelope::Body::Resync(_))));
            running.requests.get_mut(&r.request_id).unwrap().deadline =
                Instant::now() - Duration::from_secs(1);
            running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
            let _c = wire.try_recv().unwrap();
        }
        assert!(!running.is_panel_resyncing(2));
        assert!(wire.try_recv().is_err());

        let mut after_exhaust = if reply_to_state_req {
            Envelope::new(
                running.identity,
                2,
                2,
                envelope::Body::Snapshot(make_snap(2, 0, "late-btn-snap")),
                MAX_OPERATION,
            )
        } else {
            Envelope::new(
                running.identity,
                2,
                2,
                envelope::Body::Ack(panel::Empty {}),
                MAX_OPERATION,
            )
        }
        .with_panel_id(2);
        after_exhaust.in_reply_to = if reply_to_state_req {
            btn_w.request_id
        } else {
            cancel_btn.request_id
        };
        running.frame(after_exhaust, &outgoing).await.unwrap();
        assert!(running.is_panel_semantic_drifted(2));
        assert!(!running.is_panel_resyncing(2));
        assert!(wire.try_recv().is_err());
    }

    // Part B (P2-②): Select(1) promoted by same-rev Resync Ack(rev=1), then overwritten by a
    // newer unsolicited Snapshot(rev=2, Select(0)): delayed low-rev pre-dispatch Error(Deadline, rev=1)
    // for sel_w MUST NOT re-latch semantic drift or emit Resync!
    {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 0, "init")).await;

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
        let _cancel_sel = wire.try_recv().unwrap();
        let resync_sel = wire.try_recv().unwrap();

        let mut resync_ack = Envelope::new(
            running.identity,
            2,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        resync_ack.in_reply_to = resync_sel.request_id;
        running.frame(resync_ack, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));

        let unsol_overwrite = Envelope::new(
            running.identity,
            3,
            2,
            envelope::Body::Snapshot(make_snap(2, 0, "worker-overwrote-select")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(unsol_overwrite, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));

        let mut late_deadline = Envelope::new(
            running.identity,
            4,
            1,
            envelope::Body::Error(Failure::new(ErrorCode::Deadline, "pre-dispatch deadline")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_deadline.in_reply_to = sel_w.request_id;
        running.frame(late_deadline, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(wire.try_recv().is_err());
    }

    // Part C (P2-③): While final Resync #3 is still in flight on a drifted panel, an unsolicited
    // full Snapshot(rev=5) MUST NOT prematurely clear resync_semantic_drift; if Resync #3 then times
    // out and exhausts MAX_RESYNC_RETRIES, interactions remain blocked under drift. Then a post-
    // exhaustion unsolicited full Snapshot(rev=6) atomically recovers drift AND restores restart
    // eligibility so a newer conflicting Snapshot(rev=7) on old resync_1 emits a fresh Resync!
    {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 0, "init")).await;

        running.schedule_panel_semantic_resync(2);
        running.flush_pending_resyncs(&outgoing).unwrap();
        let resync_1 = wire.try_recv().unwrap();
        running.requests.get_mut(&resync_1.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let _c1 = wire.try_recv().unwrap();
        let resync_2 = wire.try_recv().unwrap();
        running.requests.get_mut(&resync_2.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let _c2 = wire.try_recv().unwrap();
        let resync_3 = wire.try_recv().unwrap();

        let inflight_unsol = Envelope::new(
            running.identity,
            2,
            5,
            envelope::Body::Snapshot(make_snap(5, 0, "inflight-unsol")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(inflight_unsol, &outgoing).await.unwrap();
        assert!(running.is_panel_semantic_drifted(2));
        assert!(running.is_panel_resyncing(2));

        // Final Resync #3 times out and exhausts MAX_RESYNC_RETRIES -> drift MUST remain latched!
        running.requests.get_mut(&resync_3.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let _c3 = wire.try_recv().unwrap();
        assert!(!running.is_panel_resyncing(2));
        assert!(running.is_panel_semantic_drifted(2));
        assert!(wire.try_recv().is_err());

        while updates.try_recv().is_ok() {}
        running
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 4,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Click(panel::Empty {})),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Error { panel_id: 2, ref failure, .. })
                if failure.code == ErrorCode::Stale as i32
        ));

        // Post-exhaustion unsolicited Snapshot(rev=6) recovers drift & restores restart eligibility.
        let post_exhaust_unsol = Envelope::new(
            running.identity,
            3,
            6,
            envelope::Body::Snapshot(make_snap(6, 0, "post-exhaust-unsol")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(post_exhaust_unsol, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(!running.is_panel_resyncing(2));

        // Newer conflicting Snapshot(rev=7 >= 6) on old resync_1 restarts Resync cleanly!
        let mut late_r1_rev7 = Envelope::new(
            running.identity,
            4,
            7,
            envelope::Body::Snapshot(make_snap(7, 0, "late-conflict")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_r1_rev7.in_reply_to = resync_1.request_id;
        running.frame(late_r1_rev7, &outgoing).await.unwrap();
        assert!(running.is_panel_semantic_drifted(2));
        assert!(matches!(wire.try_recv().unwrap().body, Some(envelope::Body::Resync(_))));
    }
}

#[tokio::test]
async fn non_drifted_snapshot_exhaustion_confirmed_vp_late_tree_and_older_superseded_resync_preserve_confirm() {
    let make_snap = |rev: u64, vp_rev: u64, offset: u64, text: &str| PanelSnapshot {
        revision: rev,
        title: "round59-regression".into(),
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
                            total_rows: 100,
                            offset,
                            viewport_revision: vp_rev,
                            rows: vec![],
                            multi_select: false,
                        }),
                    ),
                    panel::Node::new(
                        4,
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
    async fn open_child(
        running: &mut Running,
        outgoing: &mpsc::Sender<Envelope>,
        wire: &mut mpsc::Receiver<Envelope>,
        snap: PanelSnapshot,
    ) {
        let rev = snap.revision;
        let (o_tx, o_rx) = oneshot::channel();
        running
            .control(
                Control::Open {
                    panel_id: 2,
                    id: Uuid::new_v4(),
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: o_tx,
                },
                outgoing,
            )
            .await
            .unwrap();
        let o_w = wire.try_recv().unwrap();
        let mut o_s = Envelope::new(
            running.identity,
            1,
            rev,
            envelope::Body::Snapshot(snap),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        o_s.in_reply_to = o_w.request_id;
        running.frame(o_s, outgoing).await.unwrap();
        o_rx.await.unwrap().unwrap();
    }

    // Part A (P2-①): Non-optimistic Button Click times out (resync_semantic_drift=false), Resync #1..#2
    // time out, and while final Resync #3 is in flight an unsolicited Snapshot(rev=5) arrives
    // (marking resync_snapshot_confirmed=true). When Resync #3 then times out and exhausts MAX_RESYNC_RETRIES,
    // Host MUST latch semantic drift so interactions are blocked (no split-brain!), and later
    // post-exhaustion Snapshot(rev=6) atomically recovers drift AND restores restart eligibility!
    {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, "init")).await;

        running
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 4,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Click(panel::Empty {})),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let btn_w = wire.try_recv().unwrap();
        running.requests.get_mut(&btn_w.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let _c_btn = wire.try_recv().unwrap();
        let resync_1 = wire.try_recv().unwrap();
        assert!(!running.is_panel_semantic_drifted(2));

        running.requests.get_mut(&resync_1.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let _c1 = wire.try_recv().unwrap();
        let resync_2 = wire.try_recv().unwrap();

        running.requests.get_mut(&resync_2.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let _c2 = wire.try_recv().unwrap();
        let resync_3 = wire.try_recv().unwrap();

        let inflight_unsol = Envelope::new(
            running.identity,
            2,
            5,
            envelope::Body::Snapshot(make_snap(5, 1, 0, "inflight-unsol")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(inflight_unsol, &outgoing).await.unwrap();
        assert!(running.is_panel_resyncing(2));
        assert!(!running.is_panel_semantic_drifted(2));

        running.requests.get_mut(&resync_3.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let _c3 = wire.try_recv().unwrap();
        assert!(!running.is_panel_resyncing(2));
        assert!(running.is_panel_semantic_drifted(2));
        assert!(wire.try_recv().is_err());

        while updates.try_recv().is_ok() {}
        running
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 2,
                    event: UiEvent {
                        node_id: 4,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Click(panel::Empty {})),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Error { panel_id: 2, ref failure, .. })
                if failure.code == ErrorCode::Stale as i32
        ));

        let post_unsol = Envelope::new(
            running.identity,
            3,
            6,
            envelope::Body::Snapshot(make_snap(6, 1, 0, "post-recover")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(post_unsol, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));

        let mut late_r1_rev7 = Envelope::new(
            running.identity,
            4,
            7,
            envelope::Body::Snapshot(make_snap(7, 1, 0, "new-drift")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_r1_rev7.in_reply_to = resync_1.request_id;
        running.frame(late_r1_rev7, &outgoing).await.unwrap();
        assert!(running.is_panel_semantic_drifted(2));
        assert!(matches!(wire.try_recv().unwrap().body, Some(envelope::Body::Resync(_))));
    }

    // Part B (P2-②): Confirmed Viewport(vp=2, offset=50) times out (staying in confirmed_viewport_requests),
    // then newer Snapshot(rev=5) updates converged_snapshot_rev=5. Delayed lower-rev Snapshot(rev=2 < 5) OR
    // Patch(rev=2 < 5) for that Viewport MUST NOT trigger semantic drift/Resync, while rev=5 >= 5 STILL does!
    for reply_kind in ["patch_rev2", "snap_rev2", "snap_rev5"] {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, "init")).await;

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

        // Confirm target page via unsolicited Snapshot(rev=2, vp=2, offset=50) while vp_w is still active.
        let confirm_snap = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Snapshot(make_snap(2, 2, 50, "vp2-confirmed")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(confirm_snap, &outgoing).await.unwrap();

        // Now vp_w times out (recorded in cancelled_state_requests, still in confirmed_viewport_requests).
        running.requests.get_mut(&vp_w.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        assert!(!running.is_panel_resyncing(2));
        assert!(wire.try_recv().is_err());

        // Newer unsolicited full Snapshot(rev=5) updates converged_snapshot_rev=5 even though vp_w is tracked!
        let snap5 = Envelope::new(
            running.identity,
            3,
            5,
            envelope::Body::Snapshot(make_snap(5, 2, 50, "newer-tree")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(snap5, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));

        let mut late_vp = match reply_kind {
            "patch_rev2" => Envelope::new(
                running.identity,
                4,
                2,
                envelope::Body::Patch(panel::PanelPatch {
                    base_revision: 1,
                    revision: 2,
                    operations: vec![panel::PatchOperation {
                        node_id: 2,
                        change: Some(panel::patch_operation::Change::Replace(panel::Node::new(
                            2,
                            "status",
                            panel::node::Kind::Text(panel::Text {
                                text: "old-vp-patch".into(),
                                tone: panel::Tone::Foreground as i32,
                            }),
                        ))),
                    }],
                }),
                MAX_OPERATION,
            ),
            "snap_rev2" => Envelope::new(
                running.identity,
                4,
                2,
                envelope::Body::Snapshot(make_snap(2, 2, 50, "old-vp-snap")),
                MAX_OPERATION,
            ),
            _ => Envelope::new(
                running.identity,
                4,
                5,
                envelope::Body::Snapshot(make_snap(5, 2, 50, "conflict-vp-snap")),
                MAX_OPERATION,
            ),
        }
        .with_panel_id(2);
        late_vp.in_reply_to = vp_w.request_id;
        running.frame(late_vp, &outgoing).await.unwrap();

        let expect_drift = reply_kind == "snap_rev5";
        assert_eq!(running.is_panel_semantic_drifted(2), expect_drift);
        if expect_drift {
            assert!(matches!(wire.try_recv().unwrap().body, Some(envelope::Body::Resync(_))));
        } else {
            assert!(wire.try_recv().is_err());
        }
    }

    // Part C (P2-③): Drifted panel has Resync #1 (rev=1) in flight -> higher-rev Snapshot(rev=3) arrives
    // (setting resync_snapshot_confirmed=true) -> old Resync #1 replies with strictly older superseded
    // Patch(base=1, rev=2) OR Snapshot(rev=2) -> Host preserves resync_snapshot_confirmed and emits
    // Resync #2 (rev=3) -> Worker answers Resync #2 with Ack(rev=3) -> panel cleanly unlocks drift!
    for superseded_with_snap in [false, true] {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, "init")).await;

        running.schedule_panel_semantic_resync(2);
        running.flush_pending_resyncs(&outgoing).unwrap();
        let resync_1 = wire.try_recv().unwrap();
        assert!(running.is_panel_semantic_drifted(2));

        let unsol_rev3 = Envelope::new(
            running.identity,
            2,
            3,
            envelope::Body::Snapshot(make_snap(3, 1, 0, "confirmed-rev3")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(unsol_rev3, &outgoing).await.unwrap();
        assert_eq!(running.panel_revision(2), 3);

        let mut superseded_reply = if superseded_with_snap {
            Envelope::new(
                running.identity,
                3,
                2,
                envelope::Body::Snapshot(make_snap(2, 1, 0, "old-superseded-snap")),
                MAX_OPERATION,
            )
        } else {
            Envelope::new(
                running.identity,
                3,
                2,
                envelope::Body::Patch(panel::PanelPatch {
                    base_revision: 1,
                    revision: 2,
                    operations: vec![panel::PatchOperation {
                        node_id: 2,
                        change: Some(panel::patch_operation::Change::Replace(panel::Node::new(
                            2,
                            "status",
                            panel::node::Kind::Text(panel::Text {
                                text: "old-superseded-patch".into(),
                                tone: panel::Tone::Foreground as i32,
                            }),
                        ))),
                    }],
                }),
                MAX_OPERATION,
            )
        }
        .with_panel_id(2);
        superseded_reply.in_reply_to = resync_1.request_id;
        running.frame(superseded_reply, &outgoing).await.unwrap();

        let resync_2 = wire.try_recv().unwrap();
        assert!(matches!(
            resync_2.body,
            Some(envelope::Body::Resync(ref s)) if s.revision == 3
        ));

        let mut resync2_ack = Envelope::new(
            running.identity,
            4,
            3,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        resync2_ack.in_reply_to = resync_2.request_id;
        running.frame(resync2_ack, &outgoing).await.unwrap();

        assert!(!running.is_panel_semantic_drifted(2));
        assert!(!running.is_panel_resyncing(2));
        running
            .control(
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 4,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Click(panel::Empty {})),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert!(wire.try_recv().is_ok());
    }
}

#[tokio::test]
async fn snapshot_before_timeout_inherits_recovery_rev_and_expired_older_resync_preserves_confirm() {
    let make_snap = |rev: u64, vp_rev: u64, offset: u64, text: &str| PanelSnapshot {
        revision: rev,
        title: "round60-regression".into(),
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
                            total_rows: 100,
                            offset,
                            viewport_revision: vp_rev,
                            rows: vec![],
                            multi_select: false,
                        }),
                    ),
                    panel::Node::new(
                        4,
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
    async fn open_child(
        running: &mut Running,
        outgoing: &mpsc::Sender<Envelope>,
        wire: &mut mpsc::Receiver<Envelope>,
        snap: PanelSnapshot,
    ) {
        let rev = snap.revision;
        let (o_tx, o_rx) = oneshot::channel();
        running
            .control(
                Control::Open {
                    panel_id: 2,
                    id: Uuid::new_v4(),
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: o_tx,
                },
                outgoing,
            )
            .await
            .unwrap();
        let o_w = wire.try_recv().unwrap();
        let mut o_s = Envelope::new(
            running.identity,
            1,
            rev,
            envelope::Body::Snapshot(snap),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        o_s.in_reply_to = o_w.request_id;
        running.frame(o_s, outgoing).await.unwrap();
        o_rx.await.unwrap().unwrap();
    }

    // Part A (P2-①): Snapshot arrives BEFORE timeout (so no cancelled entry exists yet at Snapshot time):
    //   A1: Confirmed Viewport(vp=2) superseded by Snapshot(rev=5) BEFORE vp_w times out MUST inherit
    //       converged_snapshot_rev=5 when vp_w later times out; older Snapshot(rev=2) / Patch(rev=2)
    //       afterward does NOT latch drift, while rev=5 >= 5 STILL does!
    for reply_kind in ["patch_rev2", "snap_rev2", "snap_rev5"] {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, "init")).await;

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

        let confirm_snap = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Snapshot(make_snap(2, 2, 50, "vp2-confirmed")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(confirm_snap, &outgoing).await.unwrap();

        // Snapshot(rev=5) arrives BEFORE vp_w times out (no further Snapshot follows timeout).
        let snap5 = Envelope::new(
            running.identity,
            3,
            5,
            envelope::Body::Snapshot(make_snap(5, 2, 50, "newer-before-timeout")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(snap5, &outgoing).await.unwrap();

        running.requests.get_mut(&vp_w.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        assert!(!running.is_panel_resyncing(2));
        assert!(wire.try_recv().is_err());

        let mut late_vp = match reply_kind {
            "patch_rev2" => Envelope::new(
                running.identity,
                4,
                2,
                envelope::Body::Patch(panel::PanelPatch {
                    base_revision: 1,
                    revision: 2,
                    operations: vec![panel::PatchOperation {
                        node_id: 2,
                        change: Some(panel::patch_operation::Change::Replace(panel::Node::new(
                            2,
                            "status",
                            panel::node::Kind::Text(panel::Text {
                                text: "old-vp-patch".into(),
                                tone: panel::Tone::Foreground as i32,
                            }),
                        ))),
                    }],
                }),
                MAX_OPERATION,
            ),
            "snap_rev2" => Envelope::new(
                running.identity,
                4,
                2,
                envelope::Body::Snapshot(make_snap(2, 2, 50, "old-vp-snap")),
                MAX_OPERATION,
            ),
            _ => Envelope::new(
                running.identity,
                4,
                5,
                envelope::Body::Snapshot(make_snap(5, 2, 50, "conflict-vp-snap")),
                MAX_OPERATION,
            ),
        }
        .with_panel_id(2);
        late_vp.in_reply_to = vp_w.request_id;
        running.frame(late_vp, &outgoing).await.unwrap();

        let expect_drift = reply_kind == "snap_rev5";
        assert_eq!(running.is_panel_semantic_drifted(2), expect_drift);
        if expect_drift {
            assert!(matches!(wire.try_recv().unwrap().body, Some(envelope::Body::Resync(_))));
        } else {
            assert!(wire.try_recv().is_err());
        }
    }

    //   A2: Superseded Resync #1 receives Snapshot(rev=5) BEFORE Resync #1 times out -> Resync #1 and
    //       Cancel #1 both inherit converged_snapshot_rev=5 upon timeout -> older Snapshot(rev=2 < 5)
    //       replying to Resync #1 OR Cancel #1 MUST NOT wipe resync_snapshot_confirmed before Resync #2 Ack(5)!
    for target_cancel_frame in [false, true] {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, "init")).await;

        running.schedule_panel_semantic_resync(2);
        running.flush_pending_resyncs(&outgoing).unwrap();
        let resync_1 = wire.try_recv().unwrap();

        let snap5 = Envelope::new(
            running.identity,
            2,
            5,
            envelope::Body::Snapshot(make_snap(5, 1, 0, "snap5-before-timeout")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        running.frame(snap5, &outgoing).await.unwrap();

        running.requests.get_mut(&resync_1.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let cancel_1 = wire.try_recv().unwrap();
        let resync_2 = wire.try_recv().unwrap();
        assert!(matches!(
            resync_2.body,
            Some(envelope::Body::Resync(ref s)) if s.revision == 5
        ));

        let mut old_late = Envelope::new(
            running.identity,
            3,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, 0, "old-before-snap5")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        old_late.in_reply_to = if target_cancel_frame {
            cancel_1.request_id
        } else {
            resync_1.request_id
        };
        running.frame(old_late, &outgoing).await.unwrap();

        let mut resync2_ack = Envelope::new(
            running.identity,
            4,
            5,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        resync2_ack.in_reply_to = resync_2.request_id;
        running.frame(resync2_ack, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(!running.is_panel_resyncing(2));
    }

    // Part B (P2-②): While Resync #1 (base rev=1) is still in pending `requests`, Snapshot(rev=3)
    // arrives (`resync_snapshot_confirmed=true`), and then an EXPIRED older-baseline Patch(base=1, rev=2)
    // OR Snapshot(rev=2) arrives for Resync #1 on BOTH child panel 2 AND main panel PANEL_ID:
    //   - MUST preserve `resync_snapshot_confirmed` so Resync #2 Ack(rev=3) cleanly unlocks drift!
    //   - Contrast: an EXPIRED `Snapshot(rev=3 >= 3)` DOES mark new drift so Ack(rev=3) stays locked!
    for target_panel in [protocol::PANEL_ID, 2u64] {
        for mode in ["expired_patch_rev2", "expired_snap_rev2", "expired_snap_rev3"] {
            let (mut running, _calls, _updates) = running();
            running.supports_panel_multiplexing = true;
            *running.main_panel_mut() = PanelState::new(make_snap(1, 1, 0, "main-init")).unwrap();
            let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
            let mut in_seq = 1u64;
            if target_panel != protocol::PANEL_ID {
                open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, "child-init"))
                    .await;
                in_seq = 2;
            }

            running.schedule_panel_semantic_resync(target_panel);
            running.flush_pending_resyncs(&outgoing).unwrap();
            let resync_1 = wire.try_recv().unwrap();

            let unsol_rev3 = Envelope::new(
                running.identity,
                in_seq,
                3,
                envelope::Body::Snapshot(make_snap(3, 1, 0, "confirmed-rev3")),
                MAX_OPERATION,
            )
            .with_panel_id(target_panel);
            in_seq += 1;
            running.frame(unsol_rev3, &outgoing).await.unwrap();

            // Expire Resync #1 locally while still in `self.requests`, then deliver expired reply.
            running.requests.get_mut(&resync_1.request_id).unwrap().deadline =
                Instant::now() - Duration::from_secs(1);
            let mut expired_reply = match mode {
                "expired_patch_rev2" => Envelope::new(
                    running.identity,
                    in_seq,
                    2,
                    envelope::Body::Patch(panel::PanelPatch {
                        base_revision: 1,
                        revision: 2,
                        operations: vec![panel::PatchOperation {
                            node_id: 2,
                            change: Some(panel::patch_operation::Change::Replace(panel::Node::new(
                                2,
                                "status",
                                panel::node::Kind::Text(panel::Text {
                                    text: "expired-old-patch".into(),
                                    tone: panel::Tone::Foreground as i32,
                                }),
                            ))),
                        }],
                    }),
                    MAX_OPERATION,
                ),
                "expired_snap_rev2" => Envelope::new(
                    running.identity,
                    in_seq,
                    2,
                    envelope::Body::Snapshot(make_snap(2, 1, 0, "expired-old-snap")),
                    MAX_OPERATION,
                ),
                _ => Envelope::new(
                    running.identity,
                    in_seq,
                    3,
                    envelope::Body::Snapshot(make_snap(3, 1, 0, "expired-conflict-snap")),
                    MAX_OPERATION,
                ),
            }
            .with_panel_id(target_panel);
            in_seq += 1;
            expired_reply.in_reply_to = resync_1.request_id;
            running.frame(expired_reply, &outgoing).await.unwrap();
            let _cancel_1 = wire.try_recv().unwrap();
            let resync_2 = wire.try_recv().unwrap();
            assert!(matches!(
                resync_2.body,
                Some(envelope::Body::Resync(ref s)) if s.revision == 3
            ));

            let mut resync2_ack = Envelope::new(
                running.identity,
                in_seq,
                3,
                envelope::Body::Ack(panel::Empty {}),
                MAX_OPERATION,
            )
            .with_panel_id(target_panel);
            resync2_ack.in_reply_to = resync_2.request_id;
            running.frame(resync2_ack, &outgoing).await.unwrap();

            let expect_drift = mode == "expired_snap_rev3";
            assert_eq!(running.is_panel_semantic_drifted(target_panel), expect_drift);
            assert!(!running.is_panel_resyncing(target_panel));
        }
    }

    // Expired older Snapshot(rev=2 < 5) on an in-flight confirmed Viewport after Snapshot(rev=5)
    // ALSO avoids latching drift or emitting Resync!
    {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        open_child(&mut running, &outgoing, &mut wire, make_snap(1, 1, 0, "init")).await;
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

        running
            .frame(
                Envelope::new(
                    running.identity,
                    2,
                    2,
                    envelope::Body::Snapshot(make_snap(2, 2, 50, "vp2-confirmed")),
                    MAX_OPERATION,
                )
                .with_panel_id(2),
                &outgoing,
            )
            .await
            .unwrap();
        running
            .frame(
                Envelope::new(
                    running.identity,
                    3,
                    5,
                    envelope::Body::Snapshot(make_snap(5, 2, 50, "snap5")),
                    MAX_OPERATION,
                )
                .with_panel_id(2),
                &outgoing,
            )
            .await
            .unwrap();

        running.requests.get_mut(&vp_w.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        let mut expired_vp_old = Envelope::new(
            running.identity,
            4,
            2,
            envelope::Body::Snapshot(make_snap(2, 2, 50, "expired-old-vp-snap")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        expired_vp_old.in_reply_to = vp_w.request_id;
        running.frame(expired_vp_old, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert!(!running.is_panel_resyncing(2));
        assert!(wire.try_recv().is_err());
    }
}
