use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retracting_unclaimed_viewport_rolls_back_host_watermark_so_worker_snapshot_recovers() {
    let make_snap = |rev: u64, ed_rev: u64, contents: &[u8], vp_rev: u64, sel_idx: u32, opts: [&str; 2]| PanelSnapshot {
        revision: rev,
        title: "vp-rollback".into(),
        root: Some(panel::Node::new(
            10,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        1,
                        "ed",
                        panel::node::Kind::Editor(panel::Editor {
                            document_id: 1,
                            revision: ed_rev,
                            contents: contents.to_vec(),
                            format: panel::EditorFormat::Utf8 as i32,
                            read_only: false,
                        }),
                    ),
                    panel::Node::new(
                        2,
                        "tbl",
                        panel::node::Kind::VirtualTable(panel::VirtualTable {
                            columns: vec![panel::TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 80.0,
                            }],
                            total_rows: 10,
                            offset: 0,
                            viewport_revision: vp_rev,
                            rows: vec![],
                            multi_select: false,
                        }),
                    ),
                    panel::Node::new(
                        3,
                        "select",
                        panel::node::Kind::Select(panel::SelectNode {
                            label: "Choice".into(),
                            options: vec![opts[0].into(), opts[1].into()],
                            selected_index: sel_idx,
                            enabled: true,
                        }),
                    ),
                ],
            }),
        )),
    };

    for target_panel in [protocol::PANEL_ID, 2u64] {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        let claims = Arc::new(OutboundWriteClaims::default());
        claims.set_stop_cancel(running.cancel.clone());
        running.outbound_claims = Some(claims.clone());
        let (outgoing, writes) = mpsc::channel(OUTGOING_CAPACITY);
        let (wire, _incoming) = mpsc::channel(QUEUE_CAPACITY);
        let (wp_tx, _wp_rx) = tokio::sync::watch::channel(0u64);
        let io_cancel = CancellationToken::new();
        let mut io_tasks = JoinSet::new();

        let (host_to_worker, mut worker_rx) = tokio::io::duplex(256);
        let (_worker_tx, worker_to_host) = tokio::io::duplex(64 * 1024);
        Running::spawn_stream_io(
            host_to_worker,
            worker_to_host,
            writes,
            &wire,
            claims,
            wp_tx,
            &io_cancel,
            &mut io_tasks,
        );

        let (o_tx, o_rx) = oneshot::channel();
        running
            .control(
                Control::Open {
                    panel_id: target_panel,
                    id: Uuid::new_v4(),
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: o_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let o_env = timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
            .await
            .unwrap()
            .unwrap();
        let init_rev = if target_panel == protocol::PANEL_ID { 2 } else { 1 };
        let mut o_snap = Envelope::new(
            running.identity,
            1,
            init_rev,
            envelope::Body::Snapshot(make_snap(init_rev, 2, b"host-ed", 1, 0, ["A", "B"])),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        o_snap.in_reply_to = o_env.request_id;
        running.frame(o_snap, &outgoing).await.unwrap();
        o_rx.await.unwrap().unwrap();

        // Event A claimed and sent onto duplex pipe.
        running
            .control(
                Control::Event {
                    panel_id: target_panel,
                    expected_snapshot: None,
                    event: UiEvent {
                        node_id: 1,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Focus(true)),
                    },
                    event_seq: 0,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let ev_a = timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
            .await
            .unwrap()
            .unwrap();

        // Saturate duplex buffer so writer blocks before reading Viewport(2), Viewport(3), SelectedOption(1), EditorChanged(3).
        for s in 1..=24u64 {
            running
                .send(
                    &outgoing,
                    envelope::Body::HostResult(HostResult {
                        outcome: Some(host_result::Outcome::Done(panel::Empty {})),
                    }),
                    s,
                )
                .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(15)).await;

        for vp_rev in [2u64, 3u64] {
            running
                .control(
                    Control::Event {
                        panel_id: target_panel,
                        expected_snapshot: None,
                        event: UiEvent {
                            node_id: 2,
                            editors: vec![],
                            event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                                revision: vp_rev,
                                offset: 0,
                                limit: 5,
                            })),
                        },
                        event_seq: 0,
                    },
                    &outgoing,
                )
                .await
                .unwrap();
        }
        running
            .control(
                Control::Event {
                    panel_id: target_panel,
                    expected_snapshot: None,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::SelectedOption(1)),
                    },
                    event_seq: 0,
                },
                &outgoing,
            )
            .await
            .unwrap();
        running
            .control(
                Control::Event {
                    panel_id: target_panel,
                    expected_snapshot: None,
                    event: UiEvent {
                        node_id: 1,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::EditorChanged(EditorSnapshot {
                            document_id: 1,
                            revision: 3,
                            contents: b"host-ed-3".to_vec(),
                            ..Default::default()
                        })),
                    },
                    event_seq: 0,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert!(running.outbound_queue.is_empty());

        // Event A replies with stale-editor reordered Snapshot -> rejected -> marks semantic drift
        // AND retracts Viewport(3) -> Viewport(2) + SelectedOption(1) in LIFO order, restoring vp_rev == 1!
        let stale_rev = init_rev + 1;
        let mut stale_ev_a_snap = Envelope::new(
            running.identity,
            2,
            stale_rev,
            envelope::Body::Snapshot(make_snap(stale_rev, 1, b"stale-ed", 1, 0, ["B", "A"])),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        stale_ev_a_snap.in_reply_to = ev_a.request_id;
        running.frame(stale_ev_a_snap, &outgoing).await.unwrap();

        let snap_after_rollback = running.panel_snapshot(target_panel).unwrap();
        let rolled_vp = match snap_after_rollback
            .root
            .as_ref()
            .unwrap()
            .find(2)
            .unwrap()
            .kind
            .as_ref()
            .unwrap()
        {
            panel::node::Kind::VirtualTable(t) => t.viewport_revision,
            other => panic!("expected VirtualTable, got {other:?}"),
        };
        assert_eq!(rolled_vp, 1, "chained unclaimed Viewports must unwind back to 1");
        let rolled_sel = match snap_after_rollback
            .root
            .as_ref()
            .unwrap()
            .find(3)
            .unwrap()
            .kind
            .as_ref()
            .unwrap()
        {
            panel::node::Kind::Select(s) => s.selected_index,
            other => panic!("expected Select, got {other:?}"),
        };
        assert_eq!(rolled_sel, 0);

        let reader_task = tokio::spawn(async move {
            let mut ec_req_id = None;
            loop {
                let env = read_async_frame(&mut worker_rx).await.unwrap();
                match env.body {
                    Some(envelope::Body::HostResult(_)) => {}
                    Some(envelope::Body::UiEvent(ref ev))
                        if matches!(ev.event, Some(panel::ui_event::Event::EditorChanged(_))) =>
                    {
                        ec_req_id = Some(env.request_id);
                    }
                    Some(envelope::Body::Resync(resync_snap)) => {
                        return (ec_req_id, env.request_id, resync_snap);
                    }
                    Some(other) => panic!("unexpected retracted event on wire: {other:?}"),
                    None => {}
                }
            }
        });
        let (ec_req_id, resync_req_id, resync_snap) =
            timeout(MAX_OPERATION, reader_task).await.unwrap().unwrap();
        let ec_req_id = ec_req_id.expect("EditorChanged must reach worker");
        let resync_vp = match resync_snap.root.as_ref().unwrap().find(2).unwrap().kind.as_ref().unwrap() {
            panel::node::Kind::VirtualTable(t) => t.viewport_revision,
            _ => unreachable!(),
        };
        assert_eq!(resync_vp, 1, "Resync must carry rolled-back viewport_revision=1");
        let resync_ed_rev = match resync_snap.root.as_ref().unwrap().find(1).unwrap().kind.as_ref().unwrap() {
            panel::node::Kind::Editor(e) => e.revision,
            _ => unreachable!(),
        };
        assert_eq!(resync_ed_rev, 3, "EditorChanged must stay intact");

        // Worker replies to Resync with genuine Snapshot at vp_rev=1, ed_rev=3, ["B", "A"] -> ACCEPTED!
        let confirmed_rev = stale_rev + 1;
        let mut confirmed_snap = Envelope::new(
            running.identity,
            3,
            confirmed_rev,
            envelope::Body::Snapshot(make_snap(confirmed_rev, 3, b"host-ed-3", 1, 0, ["B", "A"])),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        confirmed_snap.in_reply_to = resync_req_id;
        running.frame(confirmed_snap, &outgoing).await.unwrap();
        let mut ec_ack = Envelope::new(
            running.identity,
            4,
            confirmed_rev,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        ec_ack.in_reply_to = ec_req_id;
        running.frame(ec_ack, &outgoing).await.unwrap();
        assert!(running.requests.is_empty());
        assert!(!running.is_panel_semantic_drifted(target_panel));
        assert!(!running.is_panel_resyncing(target_panel));
        while updates.try_recv().is_ok() {}

        running
            .control(
                Control::Event {
                    panel_id: target_panel,
                    expected_snapshot: None,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::SelectedOption(0)),
                    },
                    event_seq: 0,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert_eq!(running.requests.len(), 1);

        running.prepare_shutdown(&outgoing);
        io_cancel.cancel();
        io_tasks.abort_all();
    }
}

#[tokio::test]
async fn non_lifo_and_split_timeout_child_viewport_retractions_restore_true_base_watermark() {
    let make_snap = |rev: u64, vp_rev: u64| PanelSnapshot {
        revision: rev,
        title: format!("vp-order-{rev}"),
        root: Some(panel::Node::new(
            10,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![panel::Node::new(
                    2,
                    "tbl",
                    panel::node::Kind::VirtualTable(panel::VirtualTable {
                        columns: vec![panel::TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 80.0,
                        }],
                        total_rows: 10,
                        offset: 0,
                        viewport_revision: vp_rev,
                        rows: vec![],
                        multi_select: false,
                    }),
                )],
            }),
        )),
    };
    let read_vp = |snap: &PanelSnapshot| -> u64 {
        match snap.root.as_ref().unwrap().find(2).unwrap().kind.as_ref().unwrap() {
            panel::node::Kind::VirtualTable(t) => t.viewport_revision,
            other => panic!("expected VirtualTable, got {other:?}"),
        }
    };
    let vp_event = |vp_rev: u64| Control::Event {
        panel_id: 2,
        expected_snapshot: None,
        event: UiEvent {
            node_id: 2,
            editors: vec![],
            event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                revision: vp_rev,
                offset: 0,
                limit: 5,
            })),
        },
        event_seq: 0,
    };

    // Subcase A: Same handle_timeouts batch expires unclaimed Viewport(2) then Viewport(3) in ascending order.
    {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let claims = Arc::new(OutboundWriteClaims::default());
        running.outbound_claims = Some(claims.clone());
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

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
                &outgoing,
            )
            .await
            .unwrap();
        let o_w = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(2, o_w.request_id));
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

        running.control(vp_event(2), &outgoing).await.unwrap();
        let vp2 = wire.try_recv().unwrap();
        running.control(vp_event(3), &outgoing).await.unwrap();
        let vp3 = wire.try_recv().unwrap();
        assert_eq!(read_vp(&running.panel_snapshot(2).unwrap()), 3);

        running.requests.get_mut(&vp2.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(2);
        running.requests.get_mut(&vp3.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();

        assert_eq!(
            read_vp(&running.panel_snapshot(2).unwrap()),
            1,
            "ascending retraction of Viewport(2) then Viewport(3) must unwind back to 1"
        );
        let resync = wire.try_recv().unwrap();
        assert!(matches!(
            resync.body,
            Some(envelope::Body::Resync(ref s)) if read_vp(s) == 1
        ));
        assert!(wire.try_recv().is_err());
        assert!(!claims.claim_for_write(2, vp2.request_id));
        assert!(!claims.claim_for_write(2, vp3.request_id));
        assert!(claims.claim_for_write(2, resync.request_id));

        let mut ok_snap = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Snapshot(make_snap(2, 1)),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        ok_snap.in_reply_to = resync.request_id;
        running.frame(ok_snap, &outgoing).await.unwrap();
        assert!(!running.is_panel_resyncing(2));
    }

    // Subcase B1: Two separate timeout batches retract Viewport(2) first, then Viewport(3) second.
    {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let claims = Arc::new(OutboundWriteClaims::default());
        running.outbound_claims = Some(claims.clone());
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

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
                &outgoing,
            )
            .await
            .unwrap();
        let o_w = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(2, o_w.request_id));
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

        running.control(vp_event(2), &outgoing).await.unwrap();
        let vp2 = wire.try_recv().unwrap();
        running.control(vp_event(3), &outgoing).await.unwrap();
        let vp3 = wire.try_recv().unwrap();

        running.requests.get_mut(&vp2.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        assert_eq!(read_vp(&running.panel_snapshot(2).unwrap()), 3);
        let resync1 = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(2, resync1.request_id));

        running.requests.get_mut(&vp3.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        assert_eq!(
            read_vp(&running.panel_snapshot(2).unwrap()),
            1,
            "second timeout retracting Viewport(3) must restore true base watermark 1"
        );

        let mut stale_snap = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Snapshot(make_snap(2, 1)),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        stale_snap.in_reply_to = resync1.request_id;
        running.frame(stale_snap, &outgoing).await.unwrap();
        assert!(running.is_panel_semantic_drifted(2));
        let resync2 = wire.try_recv().unwrap();
        assert!(matches!(
            resync2.body,
            Some(envelope::Body::Resync(ref s)) if read_vp(s) == 1
        ));

        let mut ok_snap = Envelope::new(
            running.identity,
            3,
            2,
            envelope::Body::Snapshot(make_snap(2, 1)),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        ok_snap.in_reply_to = resync2.request_id;
        running.frame(ok_snap, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert_eq!(running.panel_revision(2), 2);
    }

    // Subcase B2: Claimed Viewport(2) + unclaimed Viewport(3) & Viewport(4) split across two batches
    // MUST preserve claimed watermark 2.
    {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let claims = Arc::new(OutboundWriteClaims::default());
        running.outbound_claims = Some(claims.clone());
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

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
                &outgoing,
            )
            .await
            .unwrap();
        let o_w = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(2, o_w.request_id));
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

        running.control(vp_event(2), &outgoing).await.unwrap();
        let vp2 = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(2, vp2.request_id));
        running.control(vp_event(3), &outgoing).await.unwrap();
        let vp3 = wire.try_recv().unwrap();
        running.control(vp_event(4), &outgoing).await.unwrap();
        let vp4 = wire.try_recv().unwrap();
        assert_eq!(read_vp(&running.panel_snapshot(2).unwrap()), 4);

        running.requests.get_mut(&vp3.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        assert_eq!(read_vp(&running.panel_snapshot(2).unwrap()), 4);
        let resync1 = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(2, resync1.request_id));

        running.requests.get_mut(&vp4.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        assert_eq!(
            read_vp(&running.panel_snapshot(2).unwrap()),
            2,
            "claimed Viewport(2) base must be preserved after retracting Viewport(3) and Viewport(4)"
        );

        let mut stale_snap = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Snapshot(make_snap(2, 2)),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        stale_snap.in_reply_to = resync1.request_id;
        running.frame(stale_snap, &outgoing).await.unwrap();
        assert!(running.is_panel_semantic_drifted(2));
        let resync2 = wire.try_recv().unwrap();
        assert!(matches!(
            resync2.body,
            Some(envelope::Body::Resync(ref s)) if read_vp(s) == 2
        ));

        let mut ok_snap = Envelope::new(
            running.identity,
            3,
            2,
            envelope::Body::Snapshot(make_snap(2, 2)),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        ok_snap.in_reply_to = resync2.request_id;
        running.frame(ok_snap, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(2));
        assert_eq!(read_vp(&running.panel_snapshot(2).unwrap()), 2);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unrelated_patch_advancing_panel_revision_preserves_unclaimed_viewport_rollback_on_drift()
{
    let make_snap = |rev: u64, ed_rev: u64, ed_bytes: &[u8], vp_rev: u64, opts: [&str; 2]| {
        PanelSnapshot {
            revision: rev,
            title: "patch-vp-rollback".into(),
            root: Some(panel::Node::new(
                10,
                "col",
                panel::node::Kind::Column(panel::Container {
                    children: vec![
                        panel::Node::new(
                            1,
                            "ed",
                            panel::node::Kind::Editor(panel::Editor {
                                document_id: 1,
                                revision: ed_rev,
                                contents: ed_bytes.to_vec(),
                                format: panel::EditorFormat::Utf8 as i32,
                                read_only: false,
                            }),
                        ),
                        panel::Node::new(
                            2,
                            "tbl",
                            panel::node::Kind::VirtualTable(panel::VirtualTable {
                                columns: vec![panel::TableColumn {
                                    id: "c1".into(),
                                    title: "Col".into(),
                                    width: 80.0,
                                }],
                                total_rows: 10,
                                offset: 0,
                                viewport_revision: vp_rev,
                                rows: vec![],
                                multi_select: false,
                            }),
                        ),
                        panel::Node::new(
                            3,
                            "select",
                            panel::node::Kind::Select(panel::SelectNode {
                                label: "Choice".into(),
                                options: vec![opts[0].into(), opts[1].into()],
                                selected_index: 0,
                                enabled: true,
                            }),
                        ),
                    ],
                }),
            )),
        }
    };
    let read_vp = |snap: &PanelSnapshot| -> u64 {
        match snap.root.as_ref().unwrap().find(2).unwrap().kind.as_ref().unwrap() {
            panel::node::Kind::VirtualTable(t) => t.viewport_revision,
            other => panic!("expected VirtualTable, got {other:?}"),
        }
    };

    for target_panel in [protocol::PANEL_ID, 2u64] {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        let claims = Arc::new(OutboundWriteClaims::default());
        claims.set_stop_cancel(running.cancel.clone());
        running.outbound_claims = Some(claims.clone());
        let (outgoing, writes) = mpsc::channel(OUTGOING_CAPACITY);
        let (wire, _incoming) = mpsc::channel(QUEUE_CAPACITY);
        let (wp_tx, _wp_rx) = tokio::sync::watch::channel(0u64);
        let io_cancel = CancellationToken::new();
        let mut io_tasks = JoinSet::new();

        let (host_to_worker, mut worker_rx) = tokio::io::duplex(256);
        let (_worker_tx, worker_to_host) = tokio::io::duplex(64 * 1024);
        Running::spawn_stream_io(
            host_to_worker,
            worker_to_host,
            writes,
            &wire,
            claims,
            wp_tx,
            &io_cancel,
            &mut io_tasks,
        );

        let (o_tx, o_rx) = oneshot::channel();
        running
            .control(
                Control::Open {
                    panel_id: target_panel,
                    id: Uuid::new_v4(),
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: o_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let o_env = timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
            .await
            .unwrap()
            .unwrap();
        let init_rev = if target_panel == protocol::PANEL_ID { 2 } else { 1 };
        let mut o_snap = Envelope::new(
            running.identity,
            1,
            init_rev,
            envelope::Body::Snapshot(make_snap(init_rev, 1, b"retained", 1, ["A", "B"])),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        o_snap.in_reply_to = o_env.request_id;
        running.frame(o_snap, &outgoing).await.unwrap();
        o_rx.await.unwrap().unwrap();

        // Event A & Event B both claimed onto duplex pipe.
        let mut focus_envs = Vec::new();
        for _ in 0..2 {
            running
                .control(
                    Control::Event {
                        panel_id: target_panel,
                        expected_snapshot: None,
                        event: UiEvent {
                            node_id: 1,
                            editors: vec![],
                            event: Some(panel::ui_event::Event::Focus(true)),
                        },
                        event_seq: 0,
                    },
                    &outgoing,
                )
                .await
                .unwrap();
            focus_envs.push(
                timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        let ev_a = &focus_envs[0];
        let ev_b = &focus_envs[1];

        // Backpressure duplex writer, then enqueue unclaimed Viewport(2) at init_rev.
        for s in 1..=24u64 {
            running
                .send(
                    &outgoing,
                    envelope::Body::HostResult(HostResult {
                        outcome: Some(host_result::Outcome::Done(panel::Empty {})),
                    }),
                    s,
                )
                .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(15)).await;

        running
            .control(
                Control::Event {
                    panel_id: target_panel,
                    expected_snapshot: None,
                    event: UiEvent {
                        node_id: 2,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                            revision: 2,
                            offset: 0,
                            limit: 5,
                        })),
                    },
                    event_seq: 0,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert_eq!(read_vp(&running.panel_snapshot(target_panel).unwrap()), 2);

        // Event A replies with a legitimate unrelated Patch updating ONLY Editor(node=1 -> rev=2)
        // and advancing panel.revision from `init_rev` -> `init_rev + 1`!
        let patch_rev = init_rev + 1;
        let mut ed_patch = Envelope::new(
            running.identity,
            2,
            patch_rev,
            envelope::Body::Patch(panel::PanelPatch {
                base_revision: init_rev,
                revision: patch_rev,
                operations: vec![panel::PatchOperation {
                    node_id: 1,
                    change: Some(panel::patch_operation::Change::Editor(
                        panel::EditorUpdate {
                            base_revision: 1,
                            snapshot: Some(EditorSnapshot {
                                document_id: 1,
                                revision: 2,
                                contents: b"patched-ed".to_vec(),
                                selection_start: 0,
                                selection_end: 0,
                            }),
                        },
                    )),
                }],
            }),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        ed_patch.in_reply_to = ev_a.request_id;
        running.frame(ed_patch, &outgoing).await.unwrap();
        assert_eq!(running.panel_revision(target_panel), patch_rev);
        assert_eq!(read_vp(&running.panel_snapshot(target_panel).unwrap()), 2);

        // Event B replies with reordered Snapshot carrying stale editor_rev=1 -> rejected -> marks drift!
        // Unclaimed Viewport(2) MUST STILL roll back to 1 across the unrelated Patch!
        let stale_rev = patch_rev + 1;
        let mut stale_snap = Envelope::new(
            running.identity,
            3,
            stale_rev,
            envelope::Body::Snapshot(make_snap(stale_rev, 1, b"stale-ed", 1, ["B", "A"])),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        stale_snap.in_reply_to = ev_b.request_id;
        running.frame(stale_snap, &outgoing).await.unwrap();

        assert_eq!(
            read_vp(&running.panel_snapshot(target_panel).unwrap()),
            1,
            "unclaimed Viewport(2) must roll back to 1 even after unrelated Editor Patch bumped panel revision"
        );

        let reader_task = tokio::spawn(async move {
            loop {
                let env = read_async_frame(&mut worker_rx).await.unwrap();
                match env.body {
                    Some(envelope::Body::HostResult(_)) => {}
                    Some(envelope::Body::Resync(resync_snap)) => return (env.request_id, resync_snap),
                    Some(other) => panic!("retracted Viewport(2) leaked on wire: {other:?}"),
                    None => {}
                }
            }
        });
        let (resync_req_id, resync_snap) =
            timeout(MAX_OPERATION, reader_task).await.unwrap().unwrap();
        assert_eq!(read_vp(&resync_snap), 1);

        let confirmed_rev = stale_rev + 1;
        let mut confirmed_snap = Envelope::new(
            running.identity,
            4,
            confirmed_rev,
            envelope::Body::Snapshot(make_snap(
                confirmed_rev,
                2,
                b"patched-ed",
                1,
                ["B", "A"],
            )),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        confirmed_snap.in_reply_to = resync_req_id;
        running.frame(confirmed_snap, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(target_panel));
        assert!(!running.is_panel_resyncing(target_panel));
        while updates.try_recv().is_ok() {}

        running.prepare_shutdown(&outgoing);
        io_cancel.cancel();
        io_tasks.abort_all();
    }
}

#[tokio::test]
async fn repeated_same_value_optimistic_selected_option_retracting_unclaimed_first_stays_settled_at_second()
{
    let make_snap = |rev: u64, ed_rev: u64, ed_bytes: &[u8], selected: u32| PanelSnapshot {
        revision: rev,
        title: "same-value-select".into(),
        root: Some(panel::Node::new(
            10,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        1,
                        "ed",
                        panel::node::Kind::Editor(panel::Editor {
                            document_id: 1,
                            revision: ed_rev,
                            contents: ed_bytes.to_vec(),
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
    let read_selected = |snap: &PanelSnapshot| -> u32 {
        match snap.root.as_ref().unwrap().find(3).unwrap().kind.as_ref().unwrap() {
            panel::node::Kind::Select(s) => s.selected_index,
            other => panic!("expected Select, got {other:?}"),
        }
    };

    let (mut running, _calls, mut updates) = running();
    running.supports_panel_multiplexing = true;
    let claims = Arc::new(OutboundWriteClaims::default());
    running.outbound_claims = Some(claims.clone());
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
    assert!(claims.claim_for_write(2, o_w.request_id));
    let mut o_s = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(make_snap(1, 1, b"init", 0)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    o_s.in_reply_to = o_w.request_id;
    running.frame(o_s, &outgoing).await.unwrap();
    o_rx.await.unwrap().unwrap();
    while updates.try_recv().is_ok() {}

    let select_b = || Control::Event {
        panel_id: 2,
        expected_snapshot: None,
        event: UiEvent {
            node_id: 3,
            editors: vec![],
            event: Some(panel::ui_event::Event::SelectedOption(1)),
        },
        event_seq: 0,
    };
    running.control(select_b(), &outgoing).await.unwrap();
    let sel1 = wire.try_recv().unwrap();
    // Leave sel1 UNCLAIMED; claim sel2.
    running.control(select_b(), &outgoing).await.unwrap();
    let sel2 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, sel2.request_id));
    assert_eq!(read_selected(&running.panel_snapshot(2).unwrap()), 1);

    // Expire unclaimed sel1 -> retracted without rolling back to 0 because sel2 is still pending.
    running.requests.get_mut(&sel1.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
    assert_eq!(
        read_selected(&running.panel_snapshot(2).unwrap()),
        1,
        "retracting unclaimed duplicate #1 must keep #2's optimistic value 1"
    );
    let resync1 = wire.try_recv().unwrap();

    // Worker ran sel2 (rev=1) and confirmed non-drifted resync1 with Ack(rev=1).
    let mut ack2 = Envelope::new(
        running.identity,
        2,
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    ack2.in_reply_to = sel2.request_id;
    running.frame(ack2, &outgoing).await.unwrap();
    assert_eq!(read_selected(&running.panel_snapshot(2).unwrap()), 1);

    let mut resync_ack = Envelope::new(
        running.identity,
        3,
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    resync_ack.in_reply_to = resync1.request_id;
    running.frame(resync_ack, &outgoing).await.unwrap();
    assert!(!running.is_panel_resyncing(2));

    let mut saw_synced_1 = false;
    while let Ok(ev) = updates.try_recv() {
        if let PluginEvent::PanelSynced { panel_id: 2, snapshot, .. } = ev {
            assert_eq!(read_selected(&snapshot), 1);
            saw_synced_1 = true;
        }
    }
    assert!(saw_synced_1, "settling sel2 via bare Ack must emit PanelSynced(1)");

    // CommitEditor expecting old Select=0 MUST reject, while Select=1 MUST succeed.
    let (stale_tx, mut stale_rx) = oneshot::channel();
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
                reply: stale_tx,
                expected_snapshot: Some(Arc::new(make_snap(1, 1, b"init", 0))),
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(stale_rx.try_recv().unwrap().is_err());
    assert!(wire.try_recv().is_err());

    let (ok_tx, ok_rx) = oneshot::channel();
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
                reply: ok_tx,
                expected_snapshot: Some(Arc::new(make_snap(1, 1, b"init", 1))),
            },
            &outgoing,
        )
        .await
        .unwrap();
    let cm_wire = wire.try_recv().unwrap();
    let mut cm_reply = Envelope::new(
        running.identity,
        4,
        1,
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
    cm_reply.in_reply_to = cm_wire.request_id;
    running.frame(cm_reply, &outgoing).await.unwrap();
    assert!(ok_rx.await.unwrap().is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sdk_deadline_error_and_late_deadline_retract_optimistic_viewport_and_sync_ui() {
    let make_snap = |rev: u64, vp_rev: u64| PanelSnapshot {
        revision: rev,
        title: format!("vp-deadline-{rev}"),
        root: Some(panel::Node::new(
            10,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        1,
                        "ed",
                        panel::node::Kind::Editor(panel::Editor {
                            document_id: 1,
                            revision: 1,
                            contents: b"retained".to_vec(),
                            format: panel::EditorFormat::Utf8 as i32,
                            read_only: false,
                        }),
                    ),
                    panel::Node::new(
                        2,
                        "tbl",
                        panel::node::Kind::VirtualTable(panel::VirtualTable {
                            columns: vec![panel::TableColumn {
                                id: "c1".into(),
                                title: "Col".into(),
                                width: 80.0,
                            }],
                            total_rows: 10,
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
    let read_vp = |snap: &PanelSnapshot| -> u64 {
        match snap.root.as_ref().unwrap().find(2).unwrap().kind.as_ref().unwrap() {
            panel::node::Kind::VirtualTable(t) => t.viewport_revision,
            other => panic!("expected VirtualTable, got {other:?}"),
        }
    };
    let vp_event = |panel_id: u64, vp_rev: u64| Control::Event {
        panel_id,
        expected_snapshot: None,
        event: UiEvent {
            node_id: 2,
            editors: vec![],
            event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                revision: vp_rev,
                offset: 5,
                limit: 5,
            })),
        },
        event_seq: 0,
    };

    // Subtest 1: Timely Error(Deadline) on main & child panels retracts Viewport(2) -> 1, emits PanelSynced(1),
    // and accepts subsequent Snapshot(rev=2, vp=1).
    for target_panel in [protocol::PANEL_ID, 2u64] {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        *running.main_panel_mut() = PanelState::new(make_snap(1, 1)).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        if target_panel != protocol::PANEL_ID {
            let (o_tx, o_rx) = oneshot::channel();
            running
                .control(
                    Control::Open {
                        panel_id: target_panel,
                        id: Uuid::new_v4(),
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
            .with_panel_id(target_panel);
            o_s.in_reply_to = o_w.request_id;
            running.frame(o_s, &outgoing).await.unwrap();
            o_rx.await.unwrap().unwrap();
        }
        while updates.try_recv().is_ok() {}

        running.control(vp_event(target_panel, 2), &outgoing).await.unwrap();
        let vp_w = wire.try_recv().unwrap();
        assert_eq!(read_vp(&running.panel_snapshot(target_panel).unwrap()), 2);

        let mut err_env = Envelope::new(
            running.identity,
            2,
            1,
            envelope::Body::Error(Failure::new(ErrorCode::Deadline, "过期未执行")),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        err_env.in_reply_to = vp_w.request_id;
        running.frame(err_env, &outgoing).await.unwrap();

        assert_eq!(
            read_vp(&running.panel_snapshot(target_panel).unwrap()),
            1,
            "timely Error(Deadline) must roll back Viewport(2) -> 1"
        );
        assert!(matches!(updates.try_recv(), Ok(PluginEvent::Error { panel_id, .. }) if panel_id == target_panel));
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::PanelSynced { panel_id, ref snapshot, .. })
                if panel_id == target_panel && read_vp(snapshot) == 1
        ));

        running
            .frame(
                Envelope::new(
                    running.identity,
                    3,
                    2,
                    envelope::Body::Snapshot(make_snap(2, 1)),
                    MAX_OPERATION,
                )
                .with_panel_id(target_panel),
                &outgoing,
            )
            .await
            .unwrap();
        assert_eq!(read_vp(&running.panel_snapshot(target_panel).unwrap()), 1);
        assert_eq!(running.panel_revision(target_panel), 2);
    }

    // Subtest 2: Expired-in-frame child Error(Deadline) arriving BEFORE handle_timeouts retracts vp -> 1,
    // sends Resync carrying vp=1, emits PanelSynced(1), and accepts Snapshot(rev=2, vp=1).
    {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
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

        running.control(vp_event(2, 2), &outgoing).await.unwrap();
        let vp_w = wire.try_recv().unwrap();
        running.requests.get_mut(&vp_w.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);

        let mut expired_err = Envelope::new(
            running.identity,
            2,
            1,
            envelope::Body::Error(Failure::new(ErrorCode::Deadline, "过期未执行")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        expired_err.in_reply_to = vp_w.request_id;
        running.frame(expired_err, &outgoing).await.unwrap();

        assert_eq!(read_vp(&running.panel_snapshot(2).unwrap()), 1);
        let _cancel_w = wire.try_recv().unwrap();
        let resync_w = wire.try_recv().unwrap();
        assert!(matches!(
            resync_w.body,
            Some(envelope::Body::Resync(ref s)) if read_vp(s) == 1
        ));
        let mut saw_synced = false;
        while let Ok(ev) = updates.try_recv() {
            if let PluginEvent::PanelSynced { panel_id: 2, snapshot, .. } = ev {
                assert_eq!(read_vp(&snapshot), 1);
                saw_synced = true;
            }
        }
        assert!(saw_synced);

        let mut snap2 = Envelope::new(
            running.identity,
            3,
            2,
            envelope::Body::Snapshot(make_snap(2, 1)),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        snap2.in_reply_to = resync_w.request_id;
        running.frame(snap2, &outgoing).await.unwrap();
        assert!(!running.is_panel_resyncing(2));
        assert_eq!(running.panel_revision(2), 2);
    }

    // Subtest 3: Timer fires FIRST on claimed child Viewport(2), THEN late same-revision Error(Deadline) arrives.
    // Part A: With Snapshot(rev=2, vp=1) recovery; Part B: With bare Ack(rev=1) latching semantic drift.
    for reply_with_snapshot in [true, false] {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        let claims = Arc::new(OutboundWriteClaims::default());
        running.outbound_claims = Some(claims.clone());
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

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
                &outgoing,
            )
            .await
            .unwrap();
        let o_w = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(2, o_w.request_id));
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

        running.control(vp_event(2, 2), &outgoing).await.unwrap();
        let vp_w = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(2, vp_w.request_id));
        assert_eq!(read_vp(&running.panel_snapshot(2).unwrap()), 2);

        // Timer fires FIRST -> ledger preserves (vp_w.request_id, 2) & base 1; Cancel + Resync sent.
        running.requests.get_mut(&vp_w.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let cancel_w = wire.try_recv().unwrap();
        let resync_w = wire.try_recv().unwrap();
        assert!(matches!(cancel_w.body, Some(envelope::Body::Cancel(_))));
        assert!(matches!(resync_w.body, Some(envelope::Body::Resync(_))));
        while updates.try_recv().is_ok() {}

        // Late same-revision Error(Deadline) from SDK -> MUST retract vp 2 -> 1 and emit PanelSynced(1).
        let mut late_deadline = Envelope::new(
            running.identity,
            2,
            1,
            envelope::Body::Error(Failure::new(ErrorCode::Deadline, "子面板请求已超时")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_deadline.in_reply_to = vp_w.request_id;
        running.frame(late_deadline, &outgoing).await.unwrap();
        assert_eq!(
            read_vp(&running.panel_snapshot(2).unwrap()),
            1,
            "late same-revision Error(Deadline) after timer must still roll back vp 2 -> 1"
        );
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Error { panel_id: 2, ref failure, .. })
                if failure.code == ErrorCode::Stale as i32
        ));
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::PanelSynced { panel_id: 2, ref snapshot, .. })
                if read_vp(snapshot) == 1
        ));

        let mut seq = 3u64;
        let mut cancel_ack = Envelope::new(
            running.identity,
            seq,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        seq += 1;
        cancel_ack.in_reply_to = cancel_w.request_id;
        running.frame(cancel_ack, &outgoing).await.unwrap();

        if reply_with_snapshot {
            // Worker echoes Resync #1's stale vp=2 Snapshot -> MUST be superseded, sending Resync #2 with vp=1!
            let mut echoed_stale = Envelope::new(
                running.identity,
                seq,
                2,
                envelope::Body::Snapshot(make_snap(2, 2)),
                MAX_OPERATION,
            )
            .with_panel_id(2);
            seq += 1;
            echoed_stale.in_reply_to = resync_w.request_id;
            running.frame(echoed_stale, &outgoing).await.unwrap();
            assert!(running.is_panel_semantic_drifted(2));
            assert_eq!(read_vp(&running.panel_snapshot(2).unwrap()), 1);

            let resync2 = wire.try_recv().unwrap();
            assert!(matches!(
                resync2.body,
                Some(envelope::Body::Resync(ref s)) if read_vp(s) == 1
            ));

            let mut snap3 = Envelope::new(
                running.identity,
                seq,
                3,
                envelope::Body::Snapshot(make_snap(3, 1)),
                MAX_OPERATION,
            )
            .with_panel_id(2);
            snap3.in_reply_to = resync2.request_id;
            running.frame(snap3, &outgoing).await.unwrap();
            assert!(!running.is_panel_semantic_drifted(2));
            assert!(!running.is_panel_resyncing(2));
            assert_eq!(read_vp(&running.panel_snapshot(2).unwrap()), 1);
            assert_eq!(running.panel_revision(2), 3);
        } else {
            let mut bare_ack = Envelope::new(
                running.identity,
                seq,
                1,
                envelope::Body::Ack(panel::Empty {}),
                MAX_OPERATION,
            )
            .with_panel_id(2);
            seq += 1;
            bare_ack.in_reply_to = resync_w.request_id;
            running.frame(bare_ack, &outgoing).await.unwrap();
            assert!(
                running.is_panel_semantic_drifted(2),
                "bare Ack to pre-retraction Resync must keep semantic drift latched"
            );
            let resync2 = wire.try_recv().unwrap();
            assert!(matches!(
                resync2.body,
                Some(envelope::Body::Resync(ref s)) if read_vp(s) == 1
            ));
            let mut bare_ack2 = Envelope::new(
                running.identity,
                seq,
                1,
                envelope::Body::Ack(panel::Empty {}),
                MAX_OPERATION,
            )
            .with_panel_id(2);
            bare_ack2.in_reply_to = resync2.request_id;
            running.frame(bare_ack2, &outgoing).await.unwrap();
            assert!(
                running.is_panel_semantic_drifted(2),
                "bare Ack to second Resync must still keep semantic drift latched until full Snapshot"
            );
        }
    }
}

#[tokio::test]
async fn panel_synced_carries_monotonic_processed_ui_seq_excluding_deferred_events() {
    let make_snap = |rev: u64, selected: u32| PanelSnapshot {
        revision: rev,
        title: "ui-seq-sync".into(),
        root: Some(panel::Node::new(
            10,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        1,
                        "ed",
                        panel::node::Kind::Editor(panel::Editor {
                            document_id: 1,
                            revision: 1,
                            contents: b"retained".to_vec(),
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
    *running.main_panel_mut() = PanelState::new(make_snap(1, 0)).unwrap();
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
                    event: Some(panel::ui_event::Event::SelectedOption(1)),
                },
            },
            &outgoing,
        )
        .await
        .unwrap();
    let ev1 = wire.try_recv().unwrap();

    // Activate resync barrier and defer event_seq=2.
    running.schedule_panel_resync(protocol::PANEL_ID);
    running.flush_pending_resyncs(&outgoing).unwrap();
    let resync_w = wire.try_recv().unwrap();

    running
        .control(
            Control::Event {
                panel_id: protocol::PANEL_ID,
                expected_snapshot: None,
                event_seq: 2,
                event: UiEvent {
                    node_id: 3,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::SelectedOption(0)),
                },
            },
            &outgoing,
        )
        .await
        .unwrap();

    // Settling ev1 via Ack emits PanelSynced with synced_ui_seq == 1 (NOT 2, which is still deferred!).
    let mut ack1 = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    ack1.in_reply_to = ev1.request_id;
    running.frame(ack1, &outgoing).await.unwrap();
    assert!(matches!(
        updates.try_recv(),
        Ok(PluginEvent::PanelSynced {
            panel_id: protocol::PANEL_ID,
            synced_ui_seq: 1,
            ..
        })
    ));

    // When resync finishes and deferred event_seq=2 is dispatched, processed_ui_seq advances to 2.
    let mut resync_ack = Envelope::new(
        running.identity,
        2,
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    resync_ack.in_reply_to = resync_w.request_id;
    running.frame(resync_ack, &outgoing).await.unwrap();
    assert!(matches!(
        updates.try_recv(),
        Ok(PluginEvent::PanelSynced {
            panel_id: protocol::PANEL_ID,
            synced_ui_seq: 1,
            ..
        })
    ));
    let ev2 = wire.try_recv().unwrap();
    let mut ack2 = Envelope::new(
        running.identity,
        3,
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    ack2.in_reply_to = ev2.request_id;
    running.frame(ack2, &outgoing).await.unwrap();
    assert!(matches!(
        updates.try_recv(),
        Ok(PluginEvent::PanelSynced {
            panel_id: protocol::PANEL_ID,
            synced_ui_seq: 2,
            ..
        })
    ));
}

#[tokio::test]
async fn earlier_snapshot_preserves_inflight_subsequent_select_across_ack_and_callback_error() {
    let make_snap = |rev: u64, selected: u32| PanelSnapshot {
        revision: rev,
        title: "pipelined-snap-select".into(),
        root: Some(panel::Node::new(
            10,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        1,
                        "ed",
                        panel::node::Kind::Editor(panel::Editor {
                            document_id: 1,
                            revision: 1,
                            contents: b"retained".to_vec(),
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
    let read_sel = |snap: &PanelSnapshot| -> u32 {
        match snap.root.as_ref().unwrap().find(3).unwrap().kind.as_ref().unwrap() {
            panel::node::Kind::Select(s) => s.selected_index,
            other => panic!("expected Select, got {other:?}"),
        }
    };

    // Subcase A: Request A (Focus) completes with full Snapshot(rev=2, Select=0) while B (SelectedOption(1))
    // is in flight. B's ledger survives and handles both Ack(rev=2) -> stays B=1, and Error(Invalid) -> marks drift!
    for reply_with_ack in [true, false] {
        let (mut running, _calls, mut updates) = running();
        *running.main_panel_mut() = PanelState::new(make_snap(1, 0)).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 0,
                    event: UiEvent {
                        node_id: 1,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Focus(true)),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let ev_a = wire.try_recv().unwrap();

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
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
        let ev_b = wire.try_recv().unwrap();

        let mut snap_a = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 0)),
            MAX_OPERATION,
        );
        snap_a.in_reply_to = ev_a.request_id;
        running.frame(snap_a, &outgoing).await.unwrap();
        assert_eq!(read_sel(&running.panel_snapshot(protocol::PANEL_ID).unwrap()), 1);
        while updates.try_recv().is_ok() {}

        if reply_with_ack {
            let mut ack_b = Envelope::new(
                running.identity,
                2,
                2,
                envelope::Body::Ack(panel::Empty {}),
                MAX_OPERATION,
            );
            ack_b.in_reply_to = ev_b.request_id;
            running.frame(ack_b, &outgoing).await.unwrap();
            assert_eq!(read_sel(&running.panel_snapshot(protocol::PANEL_ID).unwrap()), 1);
            assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
            assert!(matches!(
                updates.try_recv(),
                Ok(PluginEvent::PanelSynced { panel_id: protocol::PANEL_ID, ref snapshot, .. })
                    if read_sel(snapshot) == 1
            ));
        } else {
            let mut err_b = Envelope::new(
                running.identity,
                2,
                2,
                envelope::Body::Error(Failure::new(ErrorCode::Invalid, "callback failed after select")),
                MAX_OPERATION,
            );
            err_b.in_reply_to = ev_b.request_id;
            running.frame(err_b, &outgoing).await.unwrap();
            assert_eq!(read_sel(&running.panel_snapshot(protocol::PANEL_ID).unwrap()), 0);
            assert!(running.is_panel_semantic_drifted(protocol::PANEL_ID));
            assert!(matches!(
                wire.try_recv().unwrap().body,
                Some(envelope::Body::Resync(_))
            ));
        }
    }

    // Subcase B: Unsolicited Snapshot(rev=2, in_reply_to=0) clears ledger, yet is_optimistic still
    // forces semantic drift & Resync if B later returns bare Ack(rev=2) or Error(Invalid, rev=2).
    for reply_with_ack in [true, false] {
        let (mut running, _calls, _updates) = running();
        *running.main_panel_mut() = PanelState::new(make_snap(1, 0)).unwrap();
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
                        event: Some(panel::ui_event::Event::SelectedOption(1)),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let ev_b = wire.try_recv().unwrap();

        running
            .frame(
                Envelope::new(
                    running.identity,
                    1,
                    2,
                    envelope::Body::Snapshot(make_snap(2, 0)),
                    MAX_OPERATION,
                ),
                &outgoing,
            )
            .await
            .unwrap();
        assert!(!running.has_panel_optimistic_request(protocol::PANEL_ID, ev_b.request_id));

        let body = if reply_with_ack {
            envelope::Body::Ack(panel::Empty {})
        } else {
            envelope::Body::Error(Failure::new(ErrorCode::Invalid, "callback failed"))
        };
        let mut reply_b = Envelope::new(running.identity, 2, 2, body, MAX_OPERATION);
        reply_b.in_reply_to = ev_b.request_id;
        running.frame(reply_b, &outgoing).await.unwrap();
        assert!(running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(matches!(
            wire.try_recv().unwrap().body,
            Some(envelope::Body::Resync(_))
        ));
    }
}

#[tokio::test]
async fn deferred_queue_overflow_uses_contiguous_prefix_and_rejected_seq() {
    let make_snap = |rev: u64, selected: u32| PanelSnapshot {
        revision: rev,
        title: "deferred-overflow-seq".into(),
        root: Some(panel::Node::new(
            10,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        1,
                        "ed",
                        panel::node::Kind::Editor(panel::Editor {
                            document_id: 1,
                            revision: 1,
                            contents: b"retained".to_vec(),
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
    *running.main_panel_mut() = PanelState::new(make_snap(1, 0)).unwrap();
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

    // First process seq=1 cleanly.
    running
        .control(
            Control::Event {
                panel_id: protocol::PANEL_ID,
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
    let ev1 = wire.try_recv().unwrap();
    let mut ack1 = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    ack1.in_reply_to = ev1.request_id;
    running.frame(ack1, &outgoing).await.unwrap();
    while updates.try_recv().is_ok() {}

    // Enter Resync barrier and fill deferred queue with seq=2..=(MAX+1).
    running.schedule_panel_resync(protocol::PANEL_ID);
    running.flush_pending_resyncs(&outgoing).unwrap();
    let resync_w = wire.try_recv().unwrap();
    for i in 0..MAX_DEFERRED_PANEL_COMMANDS {
        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 2 + i as u64,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::SelectedOption(0)),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
    }

    // Overflow with high seq=500 -> fails immediately, yet synced_ui_seq MUST stay 1 (below seq=2) with rejected_ui_seqs=[500]!
    running
        .control(
            Control::Event {
                panel_id: protocol::PANEL_ID,
                expected_snapshot: None,
                event_seq: 500,
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
    assert!(matches!(updates.try_recv(), Ok(PluginEvent::Error { .. })));
    assert!(matches!(
        updates.try_recv(),
        Ok(PluginEvent::PanelSynced {
            panel_id: protocol::PANEL_ID,
            synced_ui_seq: 1,
            ref rejected_ui_seqs,
            ..
        }) if rejected_ui_seqs == &[500]
    ));

    // Once Resync completes, deferred commands drain sequentially and processed prefix advances monotonically.
    let mut resync_ack = Envelope::new(
        running.identity,
        2,
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    resync_ack.in_reply_to = resync_w.request_id;
    running.frame(resync_ack, &outgoing).await.unwrap();
    assert_eq!(running.contiguous_processed_ui_seq(protocol::PANEL_ID), 1);
    let mut worker_seq = 3u64;
    while let Ok(ev_wire) = wire.try_recv() {
        let mut ack = Envelope::new(
            running.identity,
            worker_seq,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        );
        worker_seq += 1;
        ack.in_reply_to = ev_wire.request_id;
        running.frame(ack, &outgoing).await.unwrap();
        while updates.try_recv().is_ok() {}
    }
    assert!(running.main_slot().deferred_commands.is_empty());
    assert_eq!(running.contiguous_processed_ui_seq(protocol::PANEL_ID), 500);
}

#[tokio::test]
async fn reordered_snapshot_retracts_unclaimed_select_and_drifts_claimed_select() {
    let make_snap = |rev: u64, options: [&str; 2], selected: u32| PanelSnapshot {
        revision: rev,
        title: "reorder-semantic-guard".into(),
        root: Some(panel::Node::new(
            10,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        1,
                        "ed",
                        panel::node::Kind::Editor(panel::Editor {
                            document_id: 1,
                            revision: 1,
                            contents: b"ok".to_vec(),
                            format: panel::EditorFormat::Utf8 as i32,
                            read_only: false,
                        }),
                    ),
                    panel::Node::new(
                        3,
                        "select",
                        panel::node::Kind::Select(panel::SelectNode {
                            label: "Choice".into(),
                            options: options.into_iter().map(str::to_owned).collect(),
                            selected_index: selected,
                            enabled: true,
                        }),
                    ),
                ],
            }),
        )),
    };

    // Part A: Unclaimed SelectedOption(0 -> "A") is retracted before writer claims it when req A's Snapshot
    // normalizes editor input and reorders options ["A", "B"] -> ["B", "A"]; causal editor_bases and synced_ui_seq
    // are delivered together on ONE PluginEvent::Panel (never dropped by an earlier empty-base PanelSynced).
    {
        let (mut running, _calls, mut updates) = running();
        let claims = Arc::new(OutboundWriteClaims::default());
        running.outbound_claims = Some(claims.clone());
        *running.main_panel_mut() = PanelState::new(make_snap(1, ["A", "B"], 1)).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        let raw_base = panel::EditorSnapshot {
            document_id: 1,
            revision: 2,
            contents: b"  raw  ".to_vec(),
            selection_start: 0,
            selection_end: 0,
        };
        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 1,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::EditorChanged(raw_base.clone())),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let ev_a = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(protocol::PANEL_ID, ev_a.request_id));

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 2,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::SelectedOption(0)),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let ev_b = wire.try_recv().unwrap();
        // Do NOT claim ev_b (simulates blocked/slow writer before claiming from `writes`).
        while updates.try_recv().is_ok() {}

        let mut normalized = make_snap(2, ["B", "A"], 1);
        if let Some(panel::node::Kind::Column(col)) =
            normalized.root.as_mut().and_then(|n| n.kind.as_mut())
            && let Some(panel::node::Kind::Editor(ed)) = col.children[0].kind.as_mut()
        {
            ed.revision = 3;
            ed.contents = b"raw".to_vec();
        }
        let mut snap_a = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(normalized),
            MAX_OPERATION,
        );
        snap_a.in_reply_to = ev_a.request_id;
        running.frame(snap_a, &outgoing).await.unwrap();

        // ev_b MUST have been retracted from claims, rejected, and NEVER mark semantic drift.
        assert!(!claims.claim_for_write(protocol::PANEL_ID, ev_b.request_id));
        assert!(!running.requests.contains_key(&ev_b.request_id));
        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));

        let mut saw_stale_error = false;
        let mut saw_separate_panel_synced = false;
        let mut saw_merged_panel = false;
        while let Ok(ev) = updates.try_recv() {
            match ev {
                PluginEvent::Error { failure, .. }
                    if failure.code == protocol::ErrorCode::Stale as i32 =>
                {
                    saw_stale_error = true;
                }
                PluginEvent::PanelSynced { .. } => {
                    saw_separate_panel_synced = true;
                }
                PluginEvent::Panel {
                    snapshot,
                    editor_bases,
                    synced_ui_seq,
                    rejected_ui_seqs,
                    ..
                } if snapshot.revision == 2
                    && editor_bases == vec![raw_base.clone()]
                    && synced_ui_seq == 2
                    && rejected_ui_seqs == [2] =>
                {
                    saw_merged_panel = true;
                }
                _ => {}
            }
        }
        assert!(saw_stale_error);
        assert!(!saw_separate_panel_synced);
        assert!(saw_merged_panel);
    }

    // Part B: Already-claimed SelectedOption(0 -> "A") cannot be retracted when Snapshot reorders
    // ["A", "B"] -> ["B", "A"]; Host immediately arms semantic drift & resync until Snapshot confirms.
    {
        let (mut running, _calls, _updates) = running();
        let claims = Arc::new(OutboundWriteClaims::default());
        running.outbound_claims = Some(claims.clone());
        *running.main_panel_mut() = PanelState::new(make_snap(1, ["A", "B"], 1)).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 1,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Focus(true)),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let ev_a = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(protocol::PANEL_ID, ev_a.request_id));

        running
            .control(
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 2,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::SelectedOption(0)),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let ev_b = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(protocol::PANEL_ID, ev_b.request_id));

        let mut snap_a = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, ["B", "A"], 1)),
            MAX_OPERATION,
        );
        snap_a.in_reply_to = ev_a.request_id;
        running.frame(snap_a, &outgoing).await.unwrap();

        assert!(running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(running.has_inflight_resync(protocol::PANEL_ID));
        let resync_w = wire.try_recv().unwrap();
        assert!(matches!(resync_w.body, Some(envelope::Body::Resync(_))));

        let mut resync_snap = Envelope::new(
            running.identity,
            2,
            3,
            envelope::Body::Snapshot(make_snap(3, ["B", "A"], 0)),
            MAX_OPERATION,
        );
        resync_snap.in_reply_to = resync_w.request_id;
        running.frame(resync_snap, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(protocol::PANEL_ID));
        assert!(!running.has_inflight_resync(protocol::PANEL_ID));
    }
}

#[test]
fn snapshot_validation_and_retraction_hold_writer_claim_barrier() {
    let claims = Arc::new(OutboundWriteClaims::default());
    claims.register(protocol::PANEL_ID, 42);

    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let claims_thread = claims.clone();

    let mut barrier = claims.lock_barrier();
    let handle = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let claimed = claims_thread.claim_for_write(protocol::PANEL_ID, 42);
        finished_tx.send(claimed).unwrap();
    });

    started_rx.recv().unwrap();
    // Writer claim MUST block on the barrier while Snapshot validation/retraction is in progress.
    assert!(
        finished_rx
            .recv_timeout(std::time::Duration::from_millis(20))
            .is_err(),
        "writer claim_for_write must block until OutboundWriteClaims barrier is released"
    );

    let retracted = barrier.try_retract_batch(&[(protocol::PANEL_ID, 42)]);
    assert_eq!(retracted, vec![(protocol::PANEL_ID, 42)]);
    drop(barrier);

    assert!(
        !finished_rx.recv().unwrap(),
        "writer must observe retracted frame after barrier release"
    );
    handle.join().unwrap();
}

#[tokio::test]
async fn retraction_and_error_sync_report_ui_event_seq_not_worker_cmd_seq() {
    let make_snap = |rev: u64, options: [&str; 2], selected: u32| PanelSnapshot {
        revision: rev,
        title: "seq-separation".into(),
        root: Some(panel::Node::new(
            10,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        1,
                        "ed",
                        panel::node::Kind::Editor(panel::Editor {
                            document_id: 1,
                            revision: 1,
                            contents: b"ok".to_vec(),
                            format: panel::EditorFormat::Utf8 as i32,
                            read_only: false,
                        }),
                    ),
                    panel::Node::new(
                        3,
                        "select",
                        panel::node::Kind::Select(panel::SelectNode {
                            label: "Choice".into(),
                            options: options.into_iter().map(str::to_owned).collect(),
                            selected_index: selected,
                            enabled: true,
                        }),
                    ),
                ],
            }),
        )),
    };

    // Part A: Explicit Error(Deadline) after diverged cmd_seq=10 and ui_event_seq=3 while deferred seq=2
    // pins contiguous_processed_ui_seq == 1 -> PanelSynced must report rejected_ui_seqs == [3], NOT [10].
    {
        let (mut running, _calls, mut updates) = running();
        *running.main_panel_mut() = PanelState::new(make_snap(1, ["A", "B"], 1)).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        running
            .control_with_seq(
                10,
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 3,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::SelectedOption(0)),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let ev_err = wire.try_recv().unwrap();

        running.schedule_panel_resync(protocol::PANEL_ID);
        running.flush_pending_resyncs(&outgoing).unwrap();
        let resync_w = wire.try_recv().unwrap();

        running
            .control_with_seq(
                11,
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 2,
                    event: UiEvent {
                        node_id: 1,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Focus(true)),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert_eq!(running.contiguous_processed_ui_seq(protocol::PANEL_ID), 1);
        while updates.try_recv().is_ok() {}

        let mut deadline_err = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Error(Failure::new(ErrorCode::Deadline, "expired")),
            MAX_OPERATION,
        );
        deadline_err.in_reply_to = ev_err.request_id;
        running.frame(deadline_err, &outgoing).await.unwrap();

        assert!(matches!(updates.try_recv(), Ok(PluginEvent::Error { .. })));
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::PanelSynced {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 1,
                ref rejected_ui_seqs,
                ..
            }) if rejected_ui_seqs == &[3]
        ));

        let mut ack = Envelope::new(
            running.identity,
            2,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        );
        ack.in_reply_to = resync_w.request_id;
        running.frame(ack, &outgoing).await.unwrap();
    }

    // Part B: Invalidated optimistic retraction under diverged cmd_seq=21 and ui_event_seq=4 while deferred
    // seq=2 pins contiguous_processed_ui_seq == 1 -> merged PluginEvent::Panel carries rejected_ui_seqs == [4], NOT [21].
    {
        let (mut running, _calls, mut updates) = running();
        let claims = Arc::new(OutboundWriteClaims::default());
        running.outbound_claims = Some(claims.clone());
        *running.main_panel_mut() = PanelState::new(make_snap(1, ["A", "B"], 1)).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        running
            .control_with_seq(
                20,
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 1,
                    event: UiEvent {
                        node_id: 1,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Focus(true)),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let ev_a = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(protocol::PANEL_ID, ev_a.request_id));

        running
            .control_with_seq(
                21,
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 4,
                    event: UiEvent {
                        node_id: 3,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::SelectedOption(0)),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let _ev_b = wire.try_recv().unwrap();

        running.schedule_panel_resync(protocol::PANEL_ID);
        running.flush_pending_resyncs(&outgoing).unwrap();
        let _resync_w = wire.try_recv().unwrap();
        running
            .control_with_seq(
                22,
                Control::Event {
                    panel_id: protocol::PANEL_ID,
                    expected_snapshot: None,
                    event_seq: 2,
                    event: UiEvent {
                        node_id: 1,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Focus(false)),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert_eq!(running.contiguous_processed_ui_seq(protocol::PANEL_ID), 0);
        while updates.try_recv().is_ok() {}

        let mut snap_a = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, ["B", "A"], 1)),
            MAX_OPERATION,
        );
        snap_a.in_reply_to = ev_a.request_id;
        running.frame(snap_a, &outgoing).await.unwrap();
        assert_eq!(running.contiguous_processed_ui_seq(protocol::PANEL_ID), 1);

        let mut saw_panel = false;
        while let Ok(ev) = updates.try_recv() {
            if let PluginEvent::Panel {
                snapshot,
                synced_ui_seq: 1,
                ref rejected_ui_seqs,
                ..
            } = ev
                && snapshot.revision == 2
                && rejected_ui_seqs == &[4]
            {
                saw_panel = true;
            }
        }
        assert!(saw_panel);
    }
}

#[tokio::test]
async fn claimed_child_viewport_timeout_emits_terminal_panel_synced_across_resync_ack_and_error() {
    let make_snap = |rev: u64, vp_rev: u64| PanelSnapshot {
        revision: rev,
        title: "child-vp-timeout".into(),
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

    // Case 1: Claimed child Viewport times out -> handle_timeouts emits PanelSynced;
    // follow-up Resync same-revision Ack emits terminal PanelSynced.
    {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
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

        running
            .control(
                Control::Event {
                    panel_id: 2,
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
        running.requests.get_mut(&vp_w.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();

        let _cancel_w = wire.try_recv().unwrap();
        let resync_w = wire.try_recv().unwrap();
        assert!(matches!(resync_w.body, Some(envelope::Body::Resync(_))));
        let mut saw_timeout_sync = false;
        while let Ok(ev) = updates.try_recv() {
            if matches!(
                ev,
                PluginEvent::PanelSynced {
                    panel_id: 2,
                    synced_ui_seq: 1,
                    ..
                }
            ) {
                saw_timeout_sync = true;
            }
        }
        assert!(saw_timeout_sync);

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
                ..
            })
        ));
    }

    // Case 2: Claimed child Viewport times out -> Resync fails with Error(Invalid) -> emits terminal PanelSynced.
    {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
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

        running
            .control(
                Control::Event {
                    panel_id: 2,
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
        running.requests.get_mut(&vp_w.request_id).unwrap().deadline =
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

        let mut saw_error = false;
        let mut saw_terminal_sync = false;
        while let Ok(ev) = updates.try_recv() {
            match ev {
                PluginEvent::Error { panel_id: 2, .. } => saw_error = true,
                PluginEvent::PanelSynced {
                    panel_id: 2,
                    synced_ui_seq: 1,
                    ..
                } => saw_terminal_sync = true,
                _ => {}
            }
        }
        assert!(saw_error);
        assert!(saw_terminal_sync);
    }
}

#[tokio::test]
async fn timed_out_claimed_viewport_after_resync_abort_accepts_base_watermark_snapshot() {
    let make_snap = |rev: u64, vp_rev: u64| PanelSnapshot {
        revision: rev,
        title: "uncertain-vp-resync-abort".into(),
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

    let (mut running, _calls, mut updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
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

    // Claimed Viewport(2) times out (no late reply ever arrives), then Resync fails with Error(Invalid).
    running
        .control(
            Control::Event {
                panel_id: 2,
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
    running.requests.get_mut(&vp_w.request_id).unwrap().deadline =
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
    while updates.try_recv().is_ok() {}

    // Worker never applied vp=2 and publishes a legitimate Snapshot with vp=1 (both unsolicited rev=2
    // and later causal rev=3); Host MUST accept it without rejecting vp=1 or re-applying vp=2!
    let snap2 = Envelope::new(
        running.identity,
        3,
        2,
        envelope::Body::Snapshot(make_snap(2, 1)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    running.frame(snap2, &outgoing).await.unwrap();
    let read_vp = |snap: &PanelSnapshot| match snap.root.as_ref().unwrap().kind.as_ref().unwrap() {
        panel::node::Kind::VirtualTable(t) => t.viewport_revision,
        other => panic!("expected table, got {other:?}"),
    };
    assert_eq!(running.panel_revision(2), 2);
    assert_eq!(read_vp(&running.panel_snapshot(2).unwrap()), 1);
    assert!(!running.is_panel_semantic_drifted(2));
}

#[tokio::test]
async fn earlier_checkbox_ack_and_unrelated_patch_do_not_clear_inflight_viewport_watermark_or_seq() {
    let make_snap = |rev: u64, checked: bool, vp_rev: u64, text: &str| PanelSnapshot {
        revision: rev,
        title: "inflight-vp-isolation".into(),
        root: Some(panel::Node::new(
            1,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        2,
                        "chk",
                        panel::node::Kind::Checkbox(panel::CheckboxNode {
                            label: "Flag".into(),
                            checked,
                            enabled: true,
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
                            text: text.into(),
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

    // Subtest A: Earlier Checkbox Ack while Viewport(seq=2, vp=2) is in flight MUST emit
    // PanelSynced with synced_ui_seq=1 and confirmed table vp=1.
    {
        let (mut running, _calls, mut updates) = running();
        *running.main_panel_mut() = PanelState::new(make_snap(1, false, 1, "v1")).unwrap();
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
                        event: Some(panel::ui_event::Event::Toggled(true)),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let chk_w = wire.try_recv().unwrap();

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

        let mut chk_ack = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        );
        chk_ack.in_reply_to = chk_w.request_id;
        running.frame(chk_ack, &outgoing).await.unwrap();
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::PanelSynced {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 1,
                ref snapshot,
                ..
            }) if read_vp(snapshot) == 1
        ));

        let mut vp_patch = Envelope::new(
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
        vp_patch.in_reply_to = vp_w.request_id;
        running.frame(vp_patch, &outgoing).await.unwrap();
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 2,
                ref snapshot,
                ..
            }) if read_vp(snapshot) == 2
        ));
    }

    // Subtest B: Unrelated newer-revision Patch(rev=2) while Viewport(seq=2, vp=2) is in flight MUST
    // publish Panel(rev=2) with synced_ui_seq=1 and confirmed table vp=1.
    {
        let (mut running, _calls, mut updates) = running();
        *running.main_panel_mut() = PanelState::new(make_snap(1, false, 1, "v1")).unwrap();
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
                        event: Some(panel::ui_event::Event::Toggled(true)),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let chk_w = wire.try_recv().unwrap();

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

        let mut patch_ev = Envelope::new(
            running.identity,
            1,
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
        );
        patch_ev.in_reply_to = chk_w.request_id;
        running.frame(patch_ev, &outgoing).await.unwrap();

        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 1,
                ref snapshot,
                ..
            }) if snapshot.revision == 2 && read_vp(snapshot) == 1
        ));

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
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Panel {
                panel_id: protocol::PANEL_ID,
                synced_ui_seq: 2,
                ref snapshot,
                ..
            }) if read_vp(snapshot) == 2
        ));
    }
}
