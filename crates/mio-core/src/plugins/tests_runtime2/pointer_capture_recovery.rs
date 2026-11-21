use super::*;
use super::helpers::{BlockingReader, BlockingWriter};

#[tokio::test]
async fn child_pointer_capture_retraction_resets_host_and_cancels_worker_drag() {
    let canvas_snap = |rev: u64| PanelSnapshot {
        revision: rev,
        title: "canvas".into(),
        root: Some(panel::Node::new(
            2,
            "canvas",
            panel::node::Kind::Canvas(panel::Canvas {
                width: 100.0,
                height: 100.0,
                commands: vec![],
                hit_regions: vec![panel::HitRegion {
                    id: 10,
                    bounds: Some(panel::Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 100.0,
                    }),
                    accessibility_label: "hit".into(),
                    focusable: false,
                    capture_pointer: true,
                }],
            }),
        )),
    };

    // Part A: Unclaimed Pointer(Down) timeout & retraction resets host capture without Resync or
    // spurious Worker Pointer(Cancel), so outside Move is rejected locally.
    {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let claims = Arc::new(OutboundWriteClaims::default());
        running.outbound_claims = Some(claims.clone());
        let (outgoing, writes) = mpsc::channel(OUTGOING_CAPACITY);
        let (wire, _incoming) = mpsc::channel(QUEUE_CAPACITY);
        let (wp_tx, _wp_rx) = tokio::sync::watch::channel(0u64);
        let io_cancel = CancellationToken::new();
        let mut io_tasks = JoinSet::new();

        let (host_to_worker, mut worker_rx) = tokio::io::duplex(512);
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

        let res_child = Uuid::new_v4();
        let (o_tx, o_rx) = oneshot::channel();
        running
            .control_with_seq(
                1,
                Control::Open {
                    panel_id: 2,
                    id: res_child,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: o_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let open_env = timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
            .await
            .unwrap()
            .unwrap();
        let mut open_snap = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Snapshot(canvas_snap(1)),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        open_snap.in_reply_to = open_env.request_id;
        running.frame(open_snap, &outgoing).await.unwrap();
        o_rx.await.unwrap().unwrap();

        for s in 1..=20u64 {
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
        tokio::time::sleep(Duration::from_millis(25)).await;

        running
            .control_with_seq(
                2,
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event: UiEvent {
                        node_id: 2,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Pointer(panel::PointerEvent {
                            hit_id: 10,
                            phase: panel::PointerPhase::Down as i32,
                            x: 50.0,
                            y: 50.0,
                        })),
                    },
                    event_seq: 0,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert_eq!(running.panel_capture(2), Some((2, 10)));
        let down_req_id = *running.requests.keys().next().unwrap();
        running.requests.get_mut(&down_req_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);

        running
            .handle_timeouts(Instant::now(), &outgoing)
            .await
            .unwrap();
        assert_eq!(running.panel_capture(2), None);
        assert_eq!(running.panels.get(&2).unwrap().resync_needed_seq, 0);
        assert!(running
            .panels
            .get(&2)
            .unwrap()
            .pending_pointer_cancels
            .is_empty());

        // Outside-bounds Move must be rejected locally because capture was cleared.
        running
            .control_with_seq(
                3,
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event: UiEvent {
                        node_id: 2,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Pointer(panel::PointerEvent {
                            hit_id: 10,
                            phase: panel::PointerPhase::Move as i32,
                            x: 500.0,
                            y: 500.0,
                        })),
                    },
                    event_seq: 0,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert!(running.requests.is_empty());

        running.prepare_shutdown(&outgoing);
        let mut saw_shutdown = false;
        timeout(MAX_OPERATION, async {
            while let Ok(env) = read_async_frame(&mut worker_rx).await {
                match env.body {
                    Some(envelope::Body::HostResult(_)) => {}
                    Some(envelope::Body::Shutdown(_)) => {
                        saw_shutdown = true;
                        break;
                    }
                    Some(other) => {
                        panic!("unexpected frame on Worker after unclaimed Down retract: {other:?}")
                    }
                    None => {}
                }
            }
        })
        .await
        .unwrap();
        assert!(saw_shutdown);

        io_cancel.cancel();
        io_tasks.abort_all();
    }

    // Part B: Already-written Pointer(Down) starts drag on Worker, then unclaimed Pointer(Up) times
    // out & retracts -> host keeps capture None, retracted Up is skipped, and Worker receives
    // tracked UiEvent::Pointer(Cancel) behind a command barrier before newer events flush.
    {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let claims = Arc::new(OutboundWriteClaims::default());
        running.outbound_claims = Some(claims.clone());
        let (outgoing, writes) = mpsc::channel(OUTGOING_CAPACITY);
        let (wire, _incoming) = mpsc::channel(QUEUE_CAPACITY);
        let (wp_tx, _wp_rx) = tokio::sync::watch::channel(0u64);
        let io_cancel = CancellationToken::new();
        let mut io_tasks = JoinSet::new();

        let (host_to_worker, mut worker_rx) = tokio::io::duplex(512);
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

        let res_child = Uuid::new_v4();
        let (o_tx, o_rx) = oneshot::channel();
        running
            .control_with_seq(
                1,
                Control::Open {
                    panel_id: 2,
                    id: res_child,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: o_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let open_env = timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
            .await
            .unwrap()
            .unwrap();
        let mut open_snap = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Snapshot(canvas_snap(1)),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        open_snap.in_reply_to = open_env.request_id;
        running.frame(open_snap, &outgoing).await.unwrap();
        o_rx.await.unwrap().unwrap();

        running
            .control_with_seq(
                2,
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event: UiEvent {
                        node_id: 2,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Pointer(panel::PointerEvent {
                            hit_id: 10,
                            phase: panel::PointerPhase::Down as i32,
                            x: 50.0,
                            y: 50.0,
                        })),
                    },
                    event_seq: 0,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let down_env = timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
            .await
            .unwrap()
            .unwrap();
        let mut down_ack =
            Envelope::new(running.identity, 2, 1, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION)
                .with_panel_id(2);
        down_ack.in_reply_to = down_env.request_id;
        running.frame(down_ack, &outgoing).await.unwrap();
        assert_eq!(running.panel_capture(2), Some((2, 10)));

        // Block writer and enqueue Pointer(Up).
        for s in 1..=20u64 {
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
        tokio::time::sleep(Duration::from_millis(25)).await;

        running
            .control_with_seq(
                3,
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event: UiEvent {
                        node_id: 2,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Pointer(panel::PointerEvent {
                            hit_id: 10,
                            phase: panel::PointerPhase::Up as i32,
                            x: 250.0,
                            y: 250.0,
                        })),
                    },
                    event_seq: 0,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert_eq!(running.panel_capture(2), None);
        let up_req_id = *running.requests.keys().next().unwrap();
        running.requests.get_mut(&up_req_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);

        running
            .handle_timeouts(Instant::now(), &outgoing)
            .await
            .unwrap();
        assert_eq!(running.panel_capture(2), None);
        assert_eq!(running.panels.get(&2).unwrap().resync_needed_seq, 0);
        assert!(!running
            .panels
            .get(&2)
            .unwrap()
            .pending_pointer_cancels
            .is_empty());

        // Newer interaction on panel 2 must wait behind the pointer-cancel recovery barrier.
        running
            .control_with_seq(
                4,
                Control::Event {
                    panel_id: 2,
                    expected_snapshot: None,
                    event: UiEvent {
                        node_id: 2,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::Pointer(panel::PointerEvent {
                            hit_id: 10,
                            phase: panel::PointerPhase::Down as i32,
                            x: 40.0,
                            y: 40.0,
                        })),
                    },
                    event_seq: 0,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert_eq!(running.panels.get(&2).unwrap().deferred_commands.len(), 1);

        let cancel_env = timeout(MAX_OPERATION, async {
            loop {
                let env = read_async_frame(&mut worker_rx).await.unwrap();
                match &env.body {
                    Some(envelope::Body::HostResult(_)) => continue,
                    Some(envelope::Body::UiEvent(ev)) => match &ev.event {
                        Some(panel::ui_event::Event::Pointer(p))
                            if p.phase == panel::PointerPhase::Cancel as i32
                                && ev.node_id == 2
                                && p.hit_id == 10 =>
                        {
                            return env;
                        }
                        other => panic!("unexpected UiEvent on Worker: {other:?}"),
                    },
                    other => panic!("unexpected frame on Worker: {other:?}"),
                }
            }
        })
        .await
        .unwrap();

        let mut cancel_ack =
            Envelope::new(running.identity, 3, 1, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION)
                .with_panel_id(2);
        cancel_ack.in_reply_to = cancel_env.request_id;
        running.frame(cancel_ack, &outgoing).await.unwrap();
        assert!(running
            .panels
            .get(&2)
            .unwrap()
            .pending_pointer_cancels
            .is_empty());
        assert!(running
            .panels
            .get(&2)
            .unwrap()
            .deferred_commands
            .is_empty());
        assert_eq!(running.panel_capture(2), Some((2, 10)));

        let new_down_env = timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            new_down_env.body,
            Some(envelope::Body::UiEvent(ref ev))
                if matches!(
                    ev.event,
                    Some(panel::ui_event::Event::Pointer(ref p))
                        if p.phase == panel::PointerPhase::Down as i32
                            && (p.x - 40.0).abs() < f32::EPSILON
                )
        ));

        io_cancel.cancel();
        io_tasks.abort_all();
    }
}

#[tokio::test]
async fn child_pointer_error_recovery_handles_acquired_released_deadline_retry_and_definitive_abort()
{
    let canvas_snap = |rev: u64| PanelSnapshot {
        revision: rev,
        title: "canvas".into(),
        root: Some(panel::Node::new(
            2,
            "canvas",
            panel::node::Kind::Canvas(panel::Canvas {
                width: 100.0,
                height: 100.0,
                commands: vec![],
                hit_regions: vec![panel::HitRegion {
                    id: 10,
                    bounds: Some(panel::Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 100.0,
                    }),
                    accessibility_label: "hit".into(),
                    focusable: false,
                    capture_pointer: true,
                }],
            }),
        )),
    };

    let (mut running, _calls, _updates) = running();
    running.supports_panel_multiplexing = true;
    let claims = Arc::new(OutboundWriteClaims::default());
    running.outbound_claims = Some(claims.clone());
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

    let mut worker_seq = 0u64;
    let mut next_worker_seq = || {
        worker_seq += 1;
        worker_seq
    };
    let mut cmd_seq = 0u64;
    let mut next_cmd_seq = || {
        cmd_seq += 1;
        cmd_seq
    };

    // Open child panel 2 with Canvas hit region.
    let res_child = Uuid::new_v4();
    let (o_tx, o_rx) = oneshot::channel();
    running
        .control_with_seq(
            next_cmd_seq(),
            Control::Open {
                panel_id: 2,
                id: res_child,
                revision: 1,
                read_only: false,
                request: request(),
                reply: o_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let open_env = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, open_env.request_id));
    let mut open_snap = Envelope::new(
        running.identity,
        next_worker_seq(),
        1,
        envelope::Body::Snapshot(canvas_snap(1)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    open_snap.in_reply_to = open_env.request_id;
    running.frame(open_snap, &outgoing).await.unwrap();
    o_rx.await.unwrap().unwrap();

    // Helper closures cannot mutably borrow `running`+`claims` easily; inline clear helpers:
    let send_pointer = |phase: panel::PointerPhase, x: f32, y: f32| Control::Event {
        panel_id: 2,
        expected_snapshot: None,
        event: UiEvent {
            node_id: 2,
            editors: vec![],
            event: Some(panel::ui_event::Event::Pointer(panel::PointerEvent {
                hit_id: 10,
                phase: phase as i32,
                x,
                y,
            })),
        },
        event_seq: 0,
    };

    // Case 1: Worker rejects already-sent Pointer(Down) (`Acquired`) with Error(Invalid) ->
    // Host clears capture AND dispatches tracked Pointer(Cancel) in case Worker mutated gesture.
    running
        .control_with_seq(
            next_cmd_seq(),
            send_pointer(panel::PointerPhase::Down, 50.0, 50.0),
            &outgoing,
        )
        .await
        .unwrap();
    let down1 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, down1.request_id));
    let mut down1_err = Envelope::new(
        running.identity,
        next_worker_seq(),
        1,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "down rejected")),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    down1_err.in_reply_to = down1.request_id;
    running.frame(down1_err, &outgoing).await.unwrap();
    assert_eq!(running.panel_capture(2), None);
    let cancel1 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, cancel1.request_id));
    assert!(matches!(
        cancel1.body,
        Some(envelope::Body::UiEvent(ref ev))
            if matches!(
                ev.event,
                Some(panel::ui_event::Event::Pointer(ref p))
                    if p.phase == panel::PointerPhase::Cancel as i32 && p.hit_id == 10
            )
    ));
    let mut cancel1_ack = Envelope::new(
        running.identity,
        next_worker_seq(),
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    cancel1_ack.in_reply_to = cancel1.request_id;
    running.frame(cancel1_ack, &outgoing).await.unwrap();
    assert!(running
        .panels
        .get(&2)
        .unwrap()
        .pending_pointer_cancels
        .is_empty());

    // Case 2: Written Pointer(Down) ACKed -> Pointer(Up) rejected with Error(Invalid) (`Released`)
    // -> synthetic Pointer(Cancel) receives Error(Deadline) (must retain seq, keep barrier & retry!)
    // -> 2nd synthetic Pointer(Cancel) ACKed -> releases deferred Pointer(Down).
    running
        .control_with_seq(
            next_cmd_seq(),
            send_pointer(panel::PointerPhase::Down, 50.0, 50.0),
            &outgoing,
        )
        .await
        .unwrap();
    let down2 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, down2.request_id));
    let mut down2_ack = Envelope::new(
        running.identity,
        next_worker_seq(),
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    down2_ack.in_reply_to = down2.request_id;
    running.frame(down2_ack, &outgoing).await.unwrap();
    assert_eq!(running.panel_capture(2), Some((2, 10)));

    running
        .control_with_seq(
            next_cmd_seq(),
            send_pointer(panel::PointerPhase::Up, 250.0, 250.0),
            &outgoing,
        )
        .await
        .unwrap();
    assert_eq!(running.panel_capture(2), None);
    let up2 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, up2.request_id));
    let mut up2_err = Envelope::new(
        running.identity,
        next_worker_seq(),
        1,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "up rejected")),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    up2_err.in_reply_to = up2.request_id;
    running.frame(up2_err, &outgoing).await.unwrap();

    let rec2_1 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, rec2_1.request_id));
    let cancel_seq2 = running
        .panels
        .get(&2)
        .unwrap()
        .pending_pointer_cancels
        .front()
        .unwrap()
        .seq;
    assert_eq!(
        running
            .panels
            .get(&2)
            .unwrap()
            .pending_pointer_cancels
            .front()
            .unwrap()
            .attempts,
        1
    );

    // Deferred Pointer(Down) must wait behind the synthetic Cancel barrier.
    running
        .control_with_seq(
            next_cmd_seq(),
            send_pointer(panel::PointerPhase::Down, 40.0, 40.0),
            &outgoing,
        )
        .await
        .unwrap();
    assert_eq!(running.panels.get(&2).unwrap().deferred_commands.len(), 1);
    assert!(wire.try_recv().is_err());

    // Synthetic Pointer(Cancel) gets Error(Deadline): preserves `cancel_seq2`, keeps barrier, retries!
    let mut rec2_1_deadline = Envelope::new(
        running.identity,
        next_worker_seq(),
        1,
        envelope::Body::Error(Failure::new(ErrorCode::Deadline, "cancel timed out")),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    rec2_1_deadline.in_reply_to = rec2_1.request_id;
    running.frame(rec2_1_deadline, &outgoing).await.unwrap();
    {
        let slot = running.panels.get(&2).unwrap();
        let front = slot.pending_pointer_cancels.front().unwrap();
        assert_eq!(front.seq, cancel_seq2);
        assert_eq!(front.attempts, 2);
        assert_eq!(slot.deferred_commands.len(), 1);
    }
    let rec2_2 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, rec2_2.request_id));
    assert!(wire.try_recv().is_err());

    let mut rec2_2_ack = Envelope::new(
        running.identity,
        next_worker_seq(),
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    rec2_2_ack.in_reply_to = rec2_2.request_id;
    running.frame(rec2_2_ack, &outgoing).await.unwrap();
    assert!(running
        .panels
        .get(&2)
        .unwrap()
        .pending_pointer_cancels
        .is_empty());
    assert!(running
        .panels
        .get(&2)
        .unwrap()
        .deferred_commands
        .is_empty());
    assert_eq!(running.panel_capture(2), Some((2, 10)));

    // Deferred Pointer(Down) flushed now!
    let down3 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, down3.request_id));

    // Case 3: Definitive non-Deadline Error(Invalid) on synthetic Pointer(Cancel) aborts barrier
    // and fails all deferred commands cleanly.
    let mut down3_ack = Envelope::new(
        running.identity,
        next_worker_seq(),
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    down3_ack.in_reply_to = down3.request_id;
    running.frame(down3_ack, &outgoing).await.unwrap();

    running
        .control_with_seq(
            next_cmd_seq(),
            send_pointer(panel::PointerPhase::Up, 250.0, 250.0),
            &outgoing,
        )
        .await
        .unwrap();
    let up3 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, up3.request_id));
    let mut up3_err = Envelope::new(
        running.identity,
        next_worker_seq(),
        1,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "up rejected")),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    up3_err.in_reply_to = up3.request_id;
    running.frame(up3_err, &outgoing).await.unwrap();
    let rec3_1 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, rec3_1.request_id));

    let (def_open_tx, def_open_rx) = oneshot::channel();
    running
        .control_with_seq(
            next_cmd_seq(),
            Control::Open {
                panel_id: 2,
                id: res_child,
                revision: 2,
                read_only: false,
                request: request(),
                reply: def_open_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    running
        .control_with_seq(
            next_cmd_seq(),
            send_pointer(panel::PointerPhase::Down, 45.0, 45.0),
            &outgoing,
        )
        .await
        .unwrap();
    assert_eq!(running.panels.get(&2).unwrap().deferred_commands.len(), 2);

    let mut rec3_err = Envelope::new(
        running.identity,
        next_worker_seq(),
        1,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "cancel unsupported")),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    rec3_err.in_reply_to = rec3_1.request_id;
    running.frame(rec3_err, &outgoing).await.unwrap();
    assert!(running
        .panels
        .get(&2)
        .unwrap()
        .pending_pointer_cancels
        .is_empty());
    assert!(running
        .panels
        .get(&2)
        .unwrap()
        .deferred_commands
        .is_empty());
    assert!(wire.try_recv().is_err());
    let def_open_err = def_open_rx.await.unwrap().unwrap_err().to_string();
    assert!(
        def_open_err.contains("面板指针手势取消失败"),
        "unexpected abort err: {def_open_err}"
    );

    // Case 4: Exhausting `MAX_RESYNC_RETRIES` Deadline replies clears barrier & fails deferred cmds.
    running
        .control_with_seq(
            next_cmd_seq(),
            send_pointer(panel::PointerPhase::Down, 50.0, 50.0),
            &outgoing,
        )
        .await
        .unwrap();
    let down4 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, down4.request_id));
    let mut down4_ack = Envelope::new(
        running.identity,
        next_worker_seq(),
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    down4_ack.in_reply_to = down4.request_id;
    running.frame(down4_ack, &outgoing).await.unwrap();

    running
        .control_with_seq(
            next_cmd_seq(),
            send_pointer(panel::PointerPhase::Up, 250.0, 250.0),
            &outgoing,
        )
        .await
        .unwrap();
    let up4 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, up4.request_id));
    let mut up4_err = Envelope::new(
        running.identity,
        next_worker_seq(),
        1,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "up rejected")),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    up4_err.in_reply_to = up4.request_id;
    running.frame(up4_err, &outgoing).await.unwrap();

    let mut rec4_env = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, rec4_env.request_id));

    let (def2_tx, mut def2_rx) = oneshot::channel();
    running
        .control_with_seq(
            next_cmd_seq(),
            Control::Open {
                panel_id: 2,
                id: res_child,
                revision: 3,
                read_only: false,
                request: request(),
                reply: def2_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();

    for attempt in 1..=MAX_RESYNC_RETRIES {
        let mut dl = Envelope::new(
            running.identity,
            next_worker_seq(),
            1,
            envelope::Body::Error(Failure::new(ErrorCode::Deadline, "cancel timeout")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        dl.in_reply_to = rec4_env.request_id;
        running.frame(dl, &outgoing).await.unwrap();
        if attempt < MAX_RESYNC_RETRIES {
            assert!(def2_rx.try_recv().is_err());
            rec4_env = wire.try_recv().unwrap();
            assert!(claims.claim_for_write(2, rec4_env.request_id));
        }
    }
    assert!(running
        .panels
        .get(&2)
        .unwrap()
        .pending_pointer_cancels
        .is_empty());
    assert!(wire.try_recv().is_err());
    let exhausted_err = def2_rx.await.unwrap().unwrap_err().to_string();
    assert!(
        exhausted_err.contains("重试超过上限"),
        "unexpected budget exhaustion err: {exhausted_err}"
    );
}

#[tokio::test]
async fn main_panel_pointer_cancel_recovery_is_non_fatal_across_timeout_late_reply_and_host_call() {
    let canvas_snap = |rev: u64| PanelSnapshot {
        revision: rev,
        title: "main-canvas".into(),
        root: Some(panel::Node::new(
            2,
            "canvas",
            panel::node::Kind::Canvas(panel::Canvas {
                width: 100.0,
                height: 100.0,
                commands: vec![],
                hit_regions: vec![panel::HitRegion {
                    id: 10,
                    bounds: Some(panel::Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 100.0,
                    }),
                    accessibility_label: "hit".into(),
                    focusable: false,
                    capture_pointer: true,
                }],
            }),
        )),
    };

    let (mut running, _calls, _updates) = running();
    let claims = Arc::new(OutboundWriteClaims::default());
    running.outbound_claims = Some(claims.clone());
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

    let mut worker_seq = 0u64;
    let mut next_worker_seq = || {
        worker_seq += 1;
        worker_seq
    };
    let mut cmd_seq = 0u64;
    let mut next_cmd_seq = || {
        cmd_seq += 1;
        cmd_seq
    };

    // Replace main panel with Canvas snapshot at revision 2.
    let (om_tx, om_rx) = oneshot::channel();
    running
        .control_with_seq(
            next_cmd_seq(),
            Control::Open {
                panel_id: protocol::PANEL_ID,
                id: Uuid::new_v4(),
                revision: 1,
                read_only: false,
                request: request(),
                reply: om_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let open_env = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, open_env.request_id));
    let mut open_snap = Envelope::new(
        running.identity,
        next_worker_seq(),
        2,
        envelope::Body::Snapshot(canvas_snap(2)),
        MAX_OPERATION,
    );
    open_snap.in_reply_to = open_env.request_id;
    running.frame(open_snap, &outgoing).await.unwrap();
    om_rx.await.unwrap().unwrap();

    let send_main_pointer = |phase: panel::PointerPhase, x: f32, y: f32| Control::Event {
        panel_id: protocol::PANEL_ID,
        expected_snapshot: None,
        event: UiEvent {
            node_id: 2,
            editors: vec![],
            event: Some(panel::ui_event::Event::Pointer(panel::PointerEvent {
                hit_id: 10,
                phase: phase as i32,
                x,
                y,
            })),
        },
        event_seq: 0,
    };

    // Start capture on main panel -> Ack, then fail Up -> dispatches attempt 1 of Pointer(Cancel).
    running
        .control_with_seq(
            next_cmd_seq(),
            send_main_pointer(panel::PointerPhase::Down, 50.0, 50.0),
            &outgoing,
        )
        .await
        .unwrap();
    let down1 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, down1.request_id));
    let mut down1_ack = Envelope::new(
        running.identity,
        next_worker_seq(),
        2,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    down1_ack.in_reply_to = down1.request_id;
    running.frame(down1_ack, &outgoing).await.unwrap();
    assert_eq!(running.panel_capture(protocol::PANEL_ID), Some((2, 10)));

    running
        .control_with_seq(
            next_cmd_seq(),
            send_main_pointer(panel::PointerPhase::Up, 250.0, 250.0),
            &outgoing,
        )
        .await
        .unwrap();
    let up1 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, up1.request_id));
    let mut up1_err = Envelope::new(
        running.identity,
        next_worker_seq(),
        2,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "up rejected")),
        MAX_OPERATION,
    );
    up1_err.in_reply_to = up1.request_id;
    running.frame(up1_err, &outgoing).await.unwrap();

    let att1 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, att1.request_id));
    let recovery_seq = running.main_slot().pending_pointer_cancels.front().unwrap().seq;
    assert_eq!(running.main_slot().pending_pointer_cancels.front().unwrap().attempts, 1);

    // Enqueue a deferred main-panel interaction; it must stay blocked across retries.
    running
        .control_with_seq(
            next_cmd_seq(),
            send_main_pointer(panel::PointerPhase::Down, 40.0, 40.0),
            &outgoing,
        )
        .await
        .unwrap();
    assert_eq!(running.main_slot().deferred_commands.len(), 1);

    // Subcase A: Local timer expires on attempt 1 -> NON-FATAL! Keeps seq & retries as attempt 2.
    running.requests.get_mut(&att1.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    running
        .handle_timeouts(Instant::now(), &outgoing)
        .await
        .unwrap();
    assert_eq!(running.main_slot().pending_pointer_cancels.front().unwrap().seq, recovery_seq);
    assert_eq!(running.main_slot().pending_pointer_cancels.front().unwrap().attempts, 2);
    assert_eq!(running.main_slot().deferred_commands.len(), 1);
    let cancel_att1 = wire.try_recv().unwrap();
    assert!(matches!(
        cancel_att1.body,
        Some(envelope::Body::Cancel(ref c)) if c.request_id == att1.request_id
    ));
    let att2 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, att2.request_id));

    // Subcase B: Attempt 2 expires locally while fresh-wire Ack arrives -> NON-FATAL! Retries attempt 3.
    running.requests.get_mut(&att2.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    let mut late_ack2 = Envelope::new(
        running.identity,
        next_worker_seq(),
        2,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    late_ack2.in_reply_to = att2.request_id;
    running.frame(late_ack2, &outgoing).await.unwrap();
    assert_eq!(running.main_slot().pending_pointer_cancels.front().unwrap().seq, recovery_seq);
    assert_eq!(running.main_slot().pending_pointer_cancels.front().unwrap().attempts, 3);
    assert_eq!(running.main_slot().deferred_commands.len(), 1);
    let cancel_att2 = wire.try_recv().unwrap();
    assert!(matches!(
        cancel_att2.body,
        Some(envelope::Body::Cancel(ref c)) if c.request_id == att2.request_id
    ));
    let att3 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, att3.request_id));

    // Subcase C: Derived HostCall against locally expired attempt 3 -> NON-FATAL! Exhausts budget & fails deferred command.
    running.requests.get_mut(&att3.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    let hc = HostCall {
        operation: Some(host_call::Operation::QueryHistory(protocol::HistoryQuery {
            offset: 0,
            limit: 1,
            filter: String::new(),
        })),
        parent_request_id: att3.request_id,
    };
    let hc_env = Envelope::new(
        running.identity,
        next_worker_seq(),
        2,
        envelope::Body::HostCall(hc),
        MAX_OPERATION,
    );
    running.frame(hc_env, &outgoing).await.unwrap();
    assert!(running.main_slot().pending_pointer_cancels.is_empty());
    assert!(running.main_slot().deferred_commands.is_empty());
    let cancel_att3 = wire.try_recv().unwrap();
    assert!(matches!(
        cancel_att3.body,
        Some(envelope::Body::Cancel(ref c)) if c.request_id == att3.request_id
    ));
    let hr = wire.try_recv().unwrap();
    assert!(matches!(
        hr.body,
        Some(envelope::Body::HostResult(HostResult {
            outcome: Some(host_result::Outcome::Error(ref f))
        })) if f.code == ErrorCode::Deadline as i32
    ));
    assert!(wire.try_recv().is_err());

    // Subcase D: Retry succeeds after timer timeout and releases new deferred Pointer(Down).
    running
        .control_with_seq(
            next_cmd_seq(),
            send_main_pointer(panel::PointerPhase::Down, 50.0, 50.0),
            &outgoing,
        )
        .await
        .unwrap();
    let down_ok = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, down_ok.request_id));
    let mut down_ok_ack = Envelope::new(
        running.identity,
        next_worker_seq(),
        2,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    down_ok_ack.in_reply_to = down_ok.request_id;
    running.frame(down_ok_ack, &outgoing).await.unwrap();

    running
        .control_with_seq(
            next_cmd_seq(),
            send_main_pointer(panel::PointerPhase::Up, 250.0, 250.0),
            &outgoing,
        )
        .await
        .unwrap();
    let up_ok = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, up_ok.request_id));
    let mut up_ok_err = Envelope::new(
        running.identity,
        next_worker_seq(),
        2,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "up rejected")),
        MAX_OPERATION,
    );
    up_ok_err.in_reply_to = up_ok.request_id;
    running.frame(up_ok_err, &outgoing).await.unwrap();
    let r_att1 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, r_att1.request_id));

    running.requests.get_mut(&r_att1.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    running
        .handle_timeouts(Instant::now(), &outgoing)
        .await
        .unwrap();
    let _cancel_r1 = wire.try_recv().unwrap();
    let r_att2 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, r_att2.request_id));

    running
        .control_with_seq(
            next_cmd_seq(),
            send_main_pointer(panel::PointerPhase::Down, 30.0, 30.0),
            &outgoing,
        )
        .await
        .unwrap();
    assert_eq!(running.main_slot().deferred_commands.len(), 1);

    let mut ack_r2 = Envelope::new(
        running.identity,
        next_worker_seq(),
        2,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    ack_r2.in_reply_to = r_att2.request_id;
    running.frame(ack_r2, &outgoing).await.unwrap();
    assert!(running.main_slot().pending_pointer_cancels.is_empty());
    assert!(running.main_slot().deferred_commands.is_empty());
    assert_eq!(running.panel_capture(protocol::PANEL_ID), Some((2, 10)));
    let flushed_down = wire.try_recv().unwrap();
    assert!(matches!(
        flushed_down.body,
        Some(envelope::Body::UiEvent(ref ev))
            if matches!(
                ev.event,
                Some(panel::ui_event::Event::Pointer(ref p))
                    if p.phase == panel::PointerPhase::Down as i32
                        && (p.x - 30.0).abs() < f32::EPSILON
            )
    ));
}

#[tokio::test]
async fn multi_target_pointer_cancel_queue_exhausting_head_still_cancels_following_target() {
    let canvas_snap = |rev: u64| PanelSnapshot {
        revision: rev,
        title: "two-hit-canvas".into(),
        root: Some(panel::Node::new(
            2,
            "canvas",
            panel::node::Kind::Canvas(panel::Canvas {
                width: 100.0,
                height: 100.0,
                commands: vec![],
                hit_regions: vec![
                    panel::HitRegion {
                        id: 10,
                        bounds: Some(panel::Rect {
                            x: 0.0,
                            y: 0.0,
                            width: 50.0,
                            height: 100.0,
                        }),
                        accessibility_label: "hit-a".into(),
                        focusable: false,
                        capture_pointer: true,
                    },
                    panel::HitRegion {
                        id: 20,
                        bounds: Some(panel::Rect {
                            x: 50.0,
                            y: 0.0,
                            width: 50.0,
                            height: 100.0,
                        }),
                        accessibility_label: "hit-b".into(),
                        focusable: false,
                        capture_pointer: true,
                    },
                ],
            }),
        )),
    };

    let (mut running, _calls, _updates) = running();
    running.supports_panel_multiplexing = true;
    let claims = Arc::new(OutboundWriteClaims::default());
    running.outbound_claims = Some(claims.clone());
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

    let mut worker_seq = 0u64;
    let mut next_worker_seq = || {
        worker_seq += 1;
        worker_seq
    };
    let mut cmd_seq = 0u64;
    let mut next_cmd_seq = || {
        cmd_seq += 1;
        cmd_seq
    };

    let res_child = Uuid::new_v4();
    let (o_tx, o_rx) = oneshot::channel();
    running
        .control_with_seq(
            next_cmd_seq(),
            Control::Open {
                panel_id: 2,
                id: res_child,
                revision: 1,
                read_only: false,
                request: request(),
                reply: o_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let open_env = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, open_env.request_id));
    let mut open_snap = Envelope::new(
        running.identity,
        next_worker_seq(),
        1,
        envelope::Body::Snapshot(canvas_snap(1)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    open_snap.in_reply_to = open_env.request_id;
    running.frame(open_snap, &outgoing).await.unwrap();
    o_rx.await.unwrap().unwrap();

    // 1. Capture region A (hit_id: 10) -> Ack.
    running
        .control_with_seq(
            next_cmd_seq(),
            Control::Event {
                panel_id: 2,
                expected_snapshot: None,
                event: UiEvent {
                    node_id: 2,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::Pointer(panel::PointerEvent {
                        hit_id: 10,
                        phase: panel::PointerPhase::Down as i32,
                        x: 25.0,
                        y: 50.0,
                    })),
                },
                event_seq: 0,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let down_a = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, down_a.request_id));
    let mut ack_a = Envelope::new(
        running.identity,
        next_worker_seq(),
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    ack_a.in_reply_to = down_a.request_id;
    running.frame(ack_a, &outgoing).await.unwrap();
    assert_eq!(running.panel_capture(2), Some((2, 10)));

    // 2. Capture switches to B (hit_id: 20), which fails with Error(Invalid) -> queues [A(10), B(20)].
    running
        .control_with_seq(
            next_cmd_seq(),
            Control::Event {
                panel_id: 2,
                expected_snapshot: None,
                event: UiEvent {
                    node_id: 2,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::Pointer(panel::PointerEvent {
                        hit_id: 20,
                        phase: panel::PointerPhase::Down as i32,
                        x: 75.0,
                        y: 50.0,
                    })),
                },
                event_seq: 0,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let down_b = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, down_b.request_id));
    let mut err_b = Envelope::new(
        running.identity,
        next_worker_seq(),
        1,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "B mutated then failed")),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    err_b.in_reply_to = down_b.request_id;
    running.frame(err_b, &outgoing).await.unwrap();
    assert_eq!(running.panel_capture(2), None);
    {
        let slot = running.panels.get(&2).unwrap();
        assert_eq!(slot.pending_pointer_cancels.len(), 2);
        assert_eq!(slot.pending_pointer_cancels[0].hit_id, 10);
        assert_eq!(slot.pending_pointer_cancels[1].hit_id, 20);
    }

    let mut cancel_a = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, cancel_a.request_id));
    assert!(matches!(
        cancel_a.body,
        Some(envelope::Body::UiEvent(ref ev))
            if matches!(
                ev.event,
                Some(panel::ui_event::Event::Pointer(ref p))
                    if p.phase == panel::PointerPhase::Cancel as i32 && p.hit_id == 10
            )
    ));

    let (def_open_tx, mut def_open_rx) = oneshot::channel();
    running
        .control_with_seq(
            next_cmd_seq(),
            Control::Open {
                panel_id: 2,
                id: res_child,
                revision: 2,
                read_only: false,
                request: request(),
                reply: def_open_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();

    // 3. Exhaust A's retry budget with `MAX_RESYNC_RETRIES` Deadline replies -> ONLY A is removed;
    // B MUST stay queued and flush immediately!
    for attempt in 1..=MAX_RESYNC_RETRIES {
        let mut dl = Envelope::new(
            running.identity,
            next_worker_seq(),
            1,
            envelope::Body::Error(Failure::new(ErrorCode::Deadline, "cancel A timeout")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        dl.in_reply_to = cancel_a.request_id;
        running.frame(dl, &outgoing).await.unwrap();
        if attempt < MAX_RESYNC_RETRIES {
            assert!(def_open_rx.try_recv().is_err());
            cancel_a = wire.try_recv().unwrap();
            assert!(claims.claim_for_write(2, cancel_a.request_id));
        }
    }

    let exhausted_err = def_open_rx.await.unwrap().unwrap_err().to_string();
    assert!(
        exhausted_err.contains("重试超过上限"),
        "unexpected budget exhaustion err: {exhausted_err}"
    );
    {
        let slot = running.panels.get(&2).unwrap();
        assert_eq!(slot.pending_pointer_cancels.len(), 1);
        assert_eq!(slot.pending_pointer_cancels[0].hit_id, 20);
        assert_eq!(slot.pending_pointer_cancels[0].attempts, 1);
    }

    let cancel_b = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, cancel_b.request_id));
    assert!(matches!(
        cancel_b.body,
        Some(envelope::Body::UiEvent(ref ev))
            if matches!(
                ev.event,
                Some(panel::ui_event::Event::Pointer(ref p))
                    if p.phase == panel::PointerPhase::Cancel as i32 && p.hit_id == 20
            )
    ));
    let mut ack_b = Envelope::new(
        running.identity,
        next_worker_seq(),
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    ack_b.in_reply_to = cancel_b.request_id;
    running.frame(ack_b, &outgoing).await.unwrap();
    assert!(running
        .panels
        .get(&2)
        .unwrap()
        .pending_pointer_cancels
        .is_empty());
    assert!(wire.try_recv().is_err());
}

#[tokio::test]
async fn main_panel_timed_out_pointer_cancel_late_snapshot_resyncs_before_releasing_deferred_event()
{
    let make_snap = |rev: u64, opt_a: &str, opt_b: &str| PanelSnapshot {
        revision: rev,
        title: "canvas-select".into(),
        root: Some(panel::Node::new(
            1,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        2,
                        "canvas",
                        panel::node::Kind::Canvas(panel::Canvas {
                            width: 100.0,
                            height: 100.0,
                            commands: vec![],
                            hit_regions: vec![panel::HitRegion {
                                id: 10,
                                bounds: Some(panel::Rect {
                                    x: 0.0,
                                    y: 0.0,
                                    width: 100.0,
                                    height: 100.0,
                                }),
                                accessibility_label: "hit".into(),
                                focusable: false,
                                capture_pointer: true,
                            }],
                        }),
                    ),
                    panel::Node::new(
                        3,
                        "select",
                        panel::node::Kind::Select(panel::SelectNode {
                            label: "Choice".into(),
                            options: vec![opt_a.into(), opt_b.into()],
                            selected_index: 0,
                            enabled: true,
                        }),
                    ),
                ],
            }),
        )),
    };

    let (mut running, _calls, _updates) = running();
    let claims = Arc::new(OutboundWriteClaims::default());
    running.outbound_claims = Some(claims.clone());
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

    let mut worker_seq = 0u64;
    let mut next_worker_seq = || {
        worker_seq += 1;
        worker_seq
    };
    let mut cmd_seq = 0u64;
    let mut next_cmd_seq = || {
        cmd_seq += 1;
        cmd_seq
    };

    // Replace main panel to rev 2 with options ["alpha", "beta"].
    let (om_tx, om_rx) = oneshot::channel();
    running
        .control_with_seq(
            next_cmd_seq(),
            Control::Open {
                panel_id: protocol::PANEL_ID,
                id: Uuid::new_v4(),
                revision: 1,
                read_only: false,
                request: request(),
                reply: om_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let open_env = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, open_env.request_id));
    let mut open_snap = Envelope::new(
        running.identity,
        next_worker_seq(),
        2,
        envelope::Body::Snapshot(make_snap(2, "alpha", "beta")),
        MAX_OPERATION,
    );
    open_snap.in_reply_to = open_env.request_id;
    running.frame(open_snap, &outgoing).await.unwrap();
    om_rx.await.unwrap().unwrap();

    let send_pointer = |phase: panel::PointerPhase, x: f32, y: f32| Control::Event {
        panel_id: protocol::PANEL_ID,
        expected_snapshot: None,
        event: UiEvent {
            node_id: 2,
            editors: vec![],
            event: Some(panel::ui_event::Event::Pointer(panel::PointerEvent {
                hit_id: 10,
                phase: phase as i32,
                x,
                y,
            })),
        },
        event_seq: 0,
    };

    // Subcase A: Timer removes attempt 1 -> late Snapshot(rev=3) updates Worker options -> idempotent
    // retry returns Ack(rev=3) -> Resync updates Host to rev 4 -> deferred SelectedOption(0) rejected!
    running
        .control_with_seq(
            next_cmd_seq(),
            send_pointer(panel::PointerPhase::Down, 50.0, 50.0),
            &outgoing,
        )
        .await
        .unwrap();
    let down1 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, down1.request_id));
    let mut down1_ack = Envelope::new(
        running.identity,
        next_worker_seq(),
        2,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    down1_ack.in_reply_to = down1.request_id;
    running.frame(down1_ack, &outgoing).await.unwrap();

    running
        .control_with_seq(
            next_cmd_seq(),
            send_pointer(panel::PointerPhase::Up, 250.0, 250.0),
            &outgoing,
        )
        .await
        .unwrap();
    let up1 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, up1.request_id));
    let mut up1_err = Envelope::new(
        running.identity,
        next_worker_seq(),
        2,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "up rejected")),
        MAX_OPERATION,
    );
    up1_err.in_reply_to = up1.request_id;
    running.frame(up1_err, &outgoing).await.unwrap();
    let att1 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, att1.request_id));

    running
        .control_with_seq(
            next_cmd_seq(),
            Control::Event {
                panel_id: protocol::PANEL_ID,
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
    assert_eq!(running.main_slot().deferred_commands.len(), 1);

    // 1. Timer expires attempt 1, retaining cancelled correlation & dispatching attempt 2.
    running.requests.get_mut(&att1.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    running
        .handle_timeouts(Instant::now(), &outgoing)
        .await
        .unwrap();
    assert!(running
        .main_slot()
        .cancelled_state_requests
        .iter()
        .any(|&(id, _)| id == att1.request_id));
    let cancel_att1 = wire.try_recv().unwrap();
    assert!(matches!(
        cancel_att1.body,
        Some(envelope::Body::Cancel(ref c)) if c.request_id == att1.request_id
    ));
    let att2 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, att2.request_id));

    // 2. Late Snapshot(rev=3) for timed-out attempt 1 arrives with reordered options -> schedules Resync!
    let mut late_snap = Envelope::new(
        running.identity,
        next_worker_seq(),
        3,
        envelope::Body::Snapshot(make_snap(3, "beta", "alpha")),
        MAX_OPERATION,
    );
    late_snap.in_reply_to = att1.request_id;
    running.frame(late_snap, &outgoing).await.unwrap();
    assert_eq!(running.main_panel().snapshot().revision, 2);
    assert_ne!(running.main_slot().resync_needed_seq, 0);
    assert_eq!(running.main_slot().deferred_commands.len(), 1);
    let resync1 = wire.try_recv().unwrap();
    assert!(matches!(resync1.body, Some(envelope::Body::Resync(_))));
    assert!(claims.claim_for_write(protocol::PANEL_ID, resync1.request_id));

    // 3. Idempotent retry attempt 2 returns Ack(rev=3): pointer cancel barrier clears, but Resync STILL holds deferred event!
    let mut ack2 = Envelope::new(
        running.identity,
        next_worker_seq(),
        3,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    ack2.in_reply_to = att2.request_id;
    running.frame(ack2, &outgoing).await.unwrap();
    assert!(running.main_slot().pending_pointer_cancels.is_empty());
    assert_ne!(running.main_slot().resync_needed_seq, 0);
    assert_eq!(running.main_slot().deferred_commands.len(), 1);
    assert!(wire.try_recv().is_err());

    // 4. Resync settles at rev 4 with reordered options ["beta", "alpha"] -> stale SelectedOption(0) rejected locally!
    let mut resync1_snap = Envelope::new(
        running.identity,
        next_worker_seq(),
        4,
        envelope::Body::Snapshot(make_snap(4, "beta", "alpha")),
        MAX_OPERATION,
    );
    resync1_snap.in_reply_to = resync1.request_id;
    running.frame(resync1_snap, &outgoing).await.unwrap();
    assert_eq!(running.main_panel().snapshot().revision, 4);
    assert_eq!(running.main_slot().resync_needed_seq, 0);
    assert!(running.main_slot().deferred_commands.is_empty());
    assert!(wire.try_recv().is_err());

    // Subcase B: No late Snapshot, but retry Ack carries drifted revision (5 != host 4) ->
    // Ack revision check STILL schedules Resync before releasing deferred SelectedOption(0).
    running
        .control_with_seq(
            next_cmd_seq(),
            send_pointer(panel::PointerPhase::Down, 50.0, 50.0),
            &outgoing,
        )
        .await
        .unwrap();
    let down_b = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, down_b.request_id));
    let mut ack_down_b = Envelope::new(
        running.identity,
        next_worker_seq(),
        4,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    ack_down_b.in_reply_to = down_b.request_id;
    running.frame(ack_down_b, &outgoing).await.unwrap();

    running
        .control_with_seq(
            next_cmd_seq(),
            send_pointer(panel::PointerPhase::Up, 250.0, 250.0),
            &outgoing,
        )
        .await
        .unwrap();
    let up_b = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, up_b.request_id));
    let mut err_up_b = Envelope::new(
        running.identity,
        next_worker_seq(),
        4,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "up rejected")),
        MAX_OPERATION,
    );
    err_up_b.in_reply_to = up_b.request_id;
    running.frame(err_up_b, &outgoing).await.unwrap();
    let att_b1 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, att_b1.request_id));

    running
        .control_with_seq(
            next_cmd_seq(),
            Control::Event {
                panel_id: protocol::PANEL_ID,
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
    assert_eq!(running.main_slot().deferred_commands.len(), 1);

    running.requests.get_mut(&att_b1.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    running
        .handle_timeouts(Instant::now(), &outgoing)
        .await
        .unwrap();
    let _cancel_b1 = wire.try_recv().unwrap();
    let att_b2 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, att_b2.request_id));

    let mut drifted_ack = Envelope::new(
        running.identity,
        next_worker_seq(),
        5,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    drifted_ack.in_reply_to = att_b2.request_id;
    running.frame(drifted_ack, &outgoing).await.unwrap();
    assert!(running.main_slot().pending_pointer_cancels.is_empty());
    assert_ne!(running.main_slot().resync_needed_seq, 0);
    assert_eq!(running.main_slot().deferred_commands.len(), 1);

    let resync_b = wire.try_recv().unwrap();
    assert!(matches!(resync_b.body, Some(envelope::Body::Resync(_))));
    assert!(claims.claim_for_write(protocol::PANEL_ID, resync_b.request_id));

    let mut ack_resync_b = Envelope::new(
        running.identity,
        next_worker_seq(),
        4,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    ack_resync_b.in_reply_to = resync_b.request_id;
    running.frame(ack_resync_b, &outgoing).await.unwrap();
    assert_eq!(running.main_slot().resync_needed_seq, 0);
    assert!(running.main_slot().resync_semantic_drift);
    assert!(running.main_slot().deferred_commands.is_empty());
    // Bare Ack on a semantic-drift Resync must reject deferred interactive events AND keep rejecting subsequent ones.
    assert!(wire.try_recv().is_err());

    running
        .control_with_seq(
            next_cmd_seq(),
            Control::Event {
                panel_id: protocol::PANEL_ID,
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
    assert!(wire.try_recv().is_err());

    // Deliver a confirming full Snapshot(5, "beta", "alpha") so drift clears for Subcase C.
    running
        .frame(
            Envelope::new(
                running.identity,
                next_worker_seq(),
                5,
                envelope::Body::Snapshot(make_snap(5, "beta", "alpha")),
                MAX_OPERATION,
            ),
            &outgoing,
        )
        .await
        .unwrap();
    assert!(!running.main_slot().resync_semantic_drift);

    // Confirming semantic Resync with an explicit Snapshot preserving ["beta", "alpha"] releases deferred SelectedOption(0).
    running
        .control_with_seq(
            next_cmd_seq(),
            send_pointer(panel::PointerPhase::Down, 50.0, 50.0),
            &outgoing,
        )
        .await
        .unwrap();
    let down_c = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, down_c.request_id));
    let mut ack_down_c = Envelope::new(
        running.identity,
        next_worker_seq(),
        5,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    ack_down_c.in_reply_to = down_c.request_id;
    running.frame(ack_down_c, &outgoing).await.unwrap();

    running
        .control_with_seq(
            next_cmd_seq(),
            send_pointer(panel::PointerPhase::Up, 250.0, 250.0),
            &outgoing,
        )
        .await
        .unwrap();
    let up_c = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, up_c.request_id));
    let mut err_up_c = Envelope::new(
        running.identity,
        next_worker_seq(),
        5,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "up rejected")),
        MAX_OPERATION,
    );
    err_up_c.in_reply_to = up_c.request_id;
    running.frame(err_up_c, &outgoing).await.unwrap();
    let att_c1 = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(protocol::PANEL_ID, att_c1.request_id));

    running
        .control_with_seq(
            next_cmd_seq(),
            Control::Event {
                panel_id: protocol::PANEL_ID,
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
    let mut drifted_ack_c = Envelope::new(
        running.identity,
        next_worker_seq(),
        6,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    drifted_ack_c.in_reply_to = att_c1.request_id;
    running.frame(drifted_ack_c, &outgoing).await.unwrap();
    let resync_c = wire.try_recv().unwrap();
    assert!(matches!(resync_c.body, Some(envelope::Body::Resync(_))));
    assert!(claims.claim_for_write(protocol::PANEL_ID, resync_c.request_id));

    let mut snap_resync_c = Envelope::new(
        running.identity,
        next_worker_seq(),
        7,
        envelope::Body::Snapshot(make_snap(7, "beta", "alpha")),
        MAX_OPERATION,
    );
    snap_resync_c.in_reply_to = resync_c.request_id;
    running.frame(snap_resync_c, &outgoing).await.unwrap();
    assert_eq!(running.main_panel().snapshot().revision, 7);
    assert_eq!(running.main_slot().resync_needed_seq, 0);
    assert!(!running.main_slot().resync_semantic_drift);
    assert!(running.main_slot().deferred_commands.is_empty());
    let flushed = wire.try_recv().unwrap();
    assert!(matches!(
        flushed.body,
        Some(envelope::Body::UiEvent(ref ev))
            if matches!(ev.event, Some(panel::ui_event::Event::SelectedOption(0)))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_sdk_default_on_resync_and_bare_ack_cannot_release_stale_selected_option_after_pointer_cancel_drift()
{
    use std::sync::{Arc, Mutex};


    struct PointerDriftNoResyncPlugin {
        options: Mutex<[&'static str; 2]>,
        executed: Arc<Mutex<Vec<String>>>,
        cancel_calls: Mutex<usize>,
        down_seen: Arc<tokio::sync::Notify>,
        cancel1_entered: Arc<tokio::sync::Notify>,
        cancel1_release: Arc<tokio::sync::Notify>,
        cancel2_seen: Arc<tokio::sync::Notify>,
        rt: tokio::runtime::Handle,
    }

    impl PointerDriftNoResyncPlugin {
        fn make_snap(rev: u64, opts: [&str; 2]) -> PanelSnapshot {
            PanelSnapshot {
                revision: rev,
                title: "sdk-pointer-drift".into(),
                root: Some(panel::Node::new(
                    1,
                    "col",
                    panel::node::Kind::Column(panel::Container {
                        children: vec![
                            panel::Node::new(
                                2,
                                "canvas",
                                panel::node::Kind::Canvas(panel::Canvas {
                                    width: 100.0,
                                    height: 100.0,
                                    commands: vec![],
                                    hit_regions: vec![panel::HitRegion {
                                        id: 10,
                                        bounds: Some(panel::Rect {
                                            x: 0.0,
                                            y: 0.0,
                                            width: 100.0,
                                            height: 100.0,
                                        }),
                                        accessibility_label: "hit".into(),
                                        focusable: false,
                                        capture_pointer: true,
                                    }],
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
        }
    }

    // Intentionally does NOT override `on_resync` / `on_panel_resync` -> exercises SDK default!
    impl mio_plugin_sdk::MioPlugin for PointerDriftNoResyncPlugin {
        fn id(&self) -> &str {
            "lab.sdk-default-resync-drift"
        }

        fn version(&self) -> &str {
            "1.5.0"
        }

        fn initial_panel(&self) -> PanelSnapshot {
            Self::make_snap(1, ["A", "B"])
        }

        fn on_ui_event(
            &mut self,
            ctx: &mut dyn mio_plugin_sdk::HostClient,
            _panel_id: u64,
            event: UiEvent,
        ) -> std::result::Result<mio_plugin_sdk::PanelUpdate, String> {
            match (event.node_id, &event.event) {
                (2, Some(panel::ui_event::Event::Pointer(p))) => {
                    match panel::PointerPhase::try_from(p.phase).unwrap() {
                        panel::PointerPhase::Down => {
                            self.down_seen.notify_one();
                            Ok(mio_plugin_sdk::PanelUpdate::None)
                        }
                        panel::PointerPhase::Up => Err("up rejected".into()),
                        panel::PointerPhase::Cancel => {
                            let mut count = self.cancel_calls.lock().unwrap();
                            let call_idx = *count;
                            *count += 1;
                            drop(count);
                            if call_idx == 0 {
                                *self.options.lock().unwrap() = ["B", "A"];
                                self.cancel1_entered.notify_one();
                                self.rt.block_on(self.cancel1_release.notified());
                                let rev = ctx.next_panel_revision();
                                Ok(Self::make_snap(rev, ["B", "A"]).into())
                            } else {
                                self.cancel2_seen.notify_one();
                                Ok(mio_plugin_sdk::PanelUpdate::None)
                            }
                        }
                        panel::PointerPhase::Move => Ok(mio_plugin_sdk::PanelUpdate::None),
                    }
                }
                (3, Some(panel::ui_event::Event::SelectedOption(idx))) => {
                    let chosen = self.options.lock().unwrap()[*idx as usize].to_owned();
                    self.executed.lock().unwrap().push(chosen);
                    Ok(mio_plugin_sdk::PanelUpdate::None)
                }
                _ => Ok(mio_plugin_sdk::PanelUpdate::None),
            }
        }
    }

    let rt_handle = tokio::runtime::Handle::current();
    let identity = crate::plugins::tests_runtime::identity();
    let manifest = PluginManifest {
        id: "lab.sdk-default-resync-drift".into(),
        version: "1.5.0".into(),
        api_version: 1,
        engine: "native".into(),
        entry: "worker".into(),
        permissions: Vec::new(),
        contributions: vec!["suite_panel".into()],
    };
    let (host_tx, worker_rx) = tokio::io::duplex(64 * 1024);
    let (worker_tx, host_rx) = tokio::io::duplex(64 * 1024);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let down_seen = Arc::new(tokio::sync::Notify::new());
    let cancel1_entered = Arc::new(tokio::sync::Notify::new());
    let cancel1_release = Arc::new(tokio::sync::Notify::new());
    let cancel2_seen = Arc::new(tokio::sync::Notify::new());

    let executed_w = executed.clone();
    let down_seen_w = down_seen.clone();
    let cancel1_entered_w = cancel1_entered.clone();
    let cancel1_release_w = cancel1_release.clone();
    let cancel2_seen_w = cancel2_seen.clone();
    let worker_thread = std::thread::spawn(move || {
        let mut plugin = PointerDriftNoResyncPlugin {
            options: Mutex::new(["A", "B"]),
            executed: executed_w,
            cancel_calls: Mutex::new(0),
            down_seen: down_seen_w,
            cancel1_entered: cancel1_entered_w,
            cancel1_release: cancel1_release_w,
            cancel2_seen: cancel2_seen_w,
            rt: rt_handle.clone(),
        };
        mio_plugin_sdk::run_with_io(
            BlockingReader(worker_rx, rt_handle.clone()),
            BlockingWriter(worker_tx, rt_handle),
            &mut plugin,
        )
        .unwrap();
    });

    let (mut supervisor, _services) = crate::plugins::supervisor::spawn_test_duplex_supervisor(
        identity,
        manifest,
        BTreeSet::new(),
        host_tx,
        host_rx,
    )
    .await
    .unwrap();
    let handle = supervisor.handle();

    let ev_init = timeout(MAX_OPERATION, supervisor.events.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(ev_init, PluginEvent::Panel { .. }));

    handle
        .try_event(UiEvent {
            node_id: 2,
            editors: vec![],
            event: Some(panel::ui_event::Event::Pointer(panel::PointerEvent {
                hit_id: 10,
                phase: panel::PointerPhase::Down as i32,
                x: 50.0,
                y: 50.0,
            })),
        })
        .unwrap();
    timeout(MAX_OPERATION, down_seen.notified()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(25)).await;

    // Trigger Pointer(Up) error -> schedules synthetic Pointer(Cancel) #1, which reorders options and blocks.
    handle
        .try_event(UiEvent {
            node_id: 2,
            editors: vec![],
            event: Some(panel::ui_event::Event::Pointer(panel::PointerEvent {
                hit_id: 10,
                phase: panel::PointerPhase::Up as i32,
                x: 250.0,
                y: 250.0,
            })),
        })
        .unwrap();
    timeout(MAX_OPERATION, cancel1_entered.notified()).await.unwrap();

    // Queue user selecting index 0 ("A") under old host view while Pointer(Cancel) barrier is active.
    handle
        .try_event(UiEvent {
            node_id: 3,
            editors: vec![],
            event: Some(panel::ui_event::Event::SelectedOption(0)),
        })
        .unwrap();

    // Wait for natural 5s host timer to expire Pointer(Cancel) #1 and schedule attempt #2.
    timeout(Duration::from_secs(7), async {
        loop {
            match supervisor.events.recv().await {
                Some(PluginEvent::Error { failure, .. })
                    if failure.message.contains("面板指针手势取消操作超时") =>
                {
                    break;
                }
                Some(PluginEvent::Stopped { reason, .. }) => {
                    panic!("pointer cancel timeout unexpectedly killed worker: {reason}")
                }
                Some(_) => continue,
                None => panic!("events closed unexpectedly"),
            }
        }
    })
    .await
    .unwrap();

    // Unblock #1 -> returns late Snapshot(rev=3, ["B","A"]); Worker then handles attempt #2 + Resync(rev=1).
    cancel1_release.notify_one();
    timeout(MAX_OPERATION, cancel2_seen.notified()).await.unwrap();

    let mut saw_resync_error = false;
    let mut saw_stale_deferred = false;
    timeout(MAX_OPERATION, async {
        while !(saw_resync_error && saw_stale_deferred) {
            match supervisor.events.recv().await {
                Some(PluginEvent::Error { failure, .. }) => {
                    if failure.message.contains("does not implement on_resync") {
                        saw_resync_error = true;
                    }
                    if failure.message.contains("面板重同步未完成") {
                        saw_stale_deferred = true;
                    }
                }
                Some(PluginEvent::Stopped { reason, .. }) => {
                    panic!("resync failure unexpectedly killed worker: {reason}")
                }
                Some(_) => continue,
                None => panic!("events closed unexpectedly"),
            }
        }
    })
    .await
    .unwrap();

    assert!(
        executed.lock().unwrap().is_empty(),
        "stale SelectedOption(0) executed wrong option B: {:?}",
        *executed.lock().unwrap()
    );

    supervisor.stop().await.unwrap();
    worker_thread.join().unwrap();
}

#[tokio::test]
async fn timely_rejected_pointer_cancel_snapshot_marks_semantic_drift_against_bare_resync_ack() {
    let (mut running, _calls, mut updates) = running();
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
    let make_snap = |rev: u64, ed_rev: u64, contents: &[u8], opts: [&str; 2]| PanelSnapshot {
        revision: rev,
        title: format!("cancel-reject-{rev}"),
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
                        "canvas",
                        panel::node::Kind::Canvas(panel::Canvas {
                            width: 100.0,
                            height: 100.0,
                            commands: vec![],
                            hit_regions: vec![panel::HitRegion {
                                id: 10,
                                bounds: Some(panel::Rect {
                                    x: 0.0,
                                    y: 0.0,
                                    width: 100.0,
                                    height: 100.0,
                                }),
                                accessibility_label: "hit".into(),
                                focusable: false,
                                capture_pointer: true,
                            }],
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
    };
    *running.main_panel_mut() = PanelState::new(make_snap(1, 2, b"newer-host", ["A", "B"])).unwrap();

    let pointer_cmd = |phase: panel::PointerPhase| Control::Event {
        panel_id: protocol::PANEL_ID,
        expected_snapshot: None,
        event: UiEvent {
            node_id: 2,
            editors: vec![],
            event: Some(panel::ui_event::Event::Pointer(panel::PointerEvent {
                hit_id: 10,
                phase: phase as i32,
                x: 50.0,
                y: 50.0,
            })),
        },
        event_seq: 0,
    };
    let mut seq = 1u64;
    let mut next_seq = || {
        let s = seq;
        seq += 1;
        s
    };

    running
        .control(pointer_cmd(panel::PointerPhase::Down), &outgoing)
        .await
        .unwrap();
    let down = wire.try_recv().unwrap();
    let mut down_ack = Envelope::new(
        running.identity,
        next_seq(),
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    down_ack.in_reply_to = down.request_id;
    running.frame(down_ack, &outgoing).await.unwrap();

    running
        .control(pointer_cmd(panel::PointerPhase::Up), &outgoing)
        .await
        .unwrap();
    let up = wire.try_recv().unwrap();
    let mut up_err = Envelope::new(
        running.identity,
        next_seq(),
        1,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "up rejected")),
        MAX_OPERATION,
    );
    up_err.in_reply_to = up.request_id;
    running.frame(up_err, &outgoing).await.unwrap();
    let cancel_req = wire.try_recv().unwrap();

    // Defer SelectedOption(0) while Pointer(Cancel) barrier is active.
    running
        .control(
            Control::Event {
                panel_id: protocol::PANEL_ID,
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
    assert_eq!(running.main_slot().deferred_commands.len(), 1);
    while updates.try_recv().is_ok() {}

    // Timely, structurally valid Pointer(Cancel) Snapshot reorders ["B", "A"] with stale editor_rev=1.
    let mut rejected_snap = Envelope::new(
        running.identity,
        next_seq(),
        2,
        envelope::Body::Snapshot(make_snap(2, 1, b"stale-editor", ["B", "A"])),
        MAX_OPERATION,
    );
    rejected_snap.in_reply_to = cancel_req.request_id;
    running.frame(rejected_snap, &outgoing).await.unwrap();

    assert!(running.main_slot().pending_pointer_cancels.is_empty());
    assert!(running.main_slot().resync_semantic_drift);
    assert_eq!(running.panel_revision(protocol::PANEL_ID), 1);
    let resync = wire.try_recv().unwrap();
    assert!(matches!(resync.body, Some(envelope::Body::Resync(ref s)) if s.revision == 1));
    assert!(wire.try_recv().is_err());

    // Bare Ack(1) from legacy Worker must fail BOTH deferred SelectedOption(0) AND any subsequent SelectedOption(0).
    let mut bare_ack = Envelope::new(
        running.identity,
        next_seq(),
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    );
    bare_ack.in_reply_to = resync.request_id;
    running.frame(bare_ack, &outgoing).await.unwrap();
    assert_eq!(running.main_slot().resync_needed_seq, 0);
    assert!(running.main_slot().resync_semantic_drift);
    assert!(running.main_slot().deferred_commands.is_empty());
    assert!(wire.try_recv().is_err());
    while updates.try_recv().is_ok() {}

    running
        .control(
            Control::Event {
                panel_id: protocol::PANEL_ID,
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
    assert!(wire.try_recv().is_err());
    assert!(matches!(
        updates.try_recv(),
        Ok(PluginEvent::Error { ref failure, .. }) if failure.code == ErrorCode::Stale as i32
    ));

    // Confirming full Snapshot clears drift and allows subsequent SelectedOption(0) to dispatch.
    running
        .frame(
            Envelope::new(
                running.identity,
                next_seq(),
                3,
                envelope::Body::Snapshot(make_snap(3, 2, b"newer-host", ["B", "A"])),
                MAX_OPERATION,
            ),
            &outgoing,
        )
        .await
        .unwrap();
    assert!(!running.main_slot().resync_semantic_drift);

    running
        .control(
            Control::Event {
                panel_id: protocol::PANEL_ID,
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
    let ok_ev = wire.try_recv().unwrap();
    assert!(matches!(
        ok_ev.body,
        Some(envelope::Body::UiEvent(ref ev))
            if matches!(ev.event, Some(panel::ui_event::Event::SelectedOption(0)))
    ));
}

#[tokio::test]
async fn expired_and_late_high_revision_pointer_cancel_acks_latch_semantic_drift_after_budget_exhaustion()
{
    let make_snap = |rev: u64, opts: [&str; 2]| PanelSnapshot {
        revision: rev,
        title: format!("pc-ack-{rev}"),
        root: Some(panel::Node::new(
            10,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        2,
                        "canvas",
                        panel::node::Kind::Canvas(panel::Canvas {
                            width: 100.0,
                            height: 100.0,
                            commands: vec![],
                            hit_regions: vec![panel::HitRegion {
                                id: 10,
                                bounds: Some(panel::Rect {
                                    x: 0.0,
                                    y: 0.0,
                                    width: 100.0,
                                    height: 100.0,
                                }),
                                accessibility_label: "hit".into(),
                                focusable: false,
                                capture_pointer: true,
                            }],
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
    };
    let pointer_cmd = |panel_id: u64, phase: panel::PointerPhase| Control::Event {
        panel_id,
        expected_snapshot: None,
        event: UiEvent {
            node_id: 2,
            editors: vec![],
            event: Some(panel::ui_event::Event::Pointer(panel::PointerEvent {
                hit_id: 10,
                phase: phase as i32,
                x: 50.0,
                y: 50.0,
            })),
        },
        event_seq: 0,
    };
    let select_cmd = |panel_id: u64| Control::Event {
        panel_id,
        expected_snapshot: None,
        event: UiEvent {
            node_id: 3,
            editors: vec![],
            event: Some(panel::ui_event::Event::SelectedOption(0)),
        },
        event_seq: 0,
    };

    // Subcase A: Child panel 2 receives an expired high-rev Ack(2) BEFORE timer on final 3rd Pointer(Cancel) attempt.
    {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        let mut seq = 1u64;
        let mut next_seq = || {
            let s = seq;
            seq += 1;
            s
        };

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
        let o_wire = wire.try_recv().unwrap();
        let mut o_snap = Envelope::new(
            running.identity,
            next_seq(),
            1,
            envelope::Body::Snapshot(make_snap(1, ["A", "B"])),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        o_snap.in_reply_to = o_wire.request_id;
        running.frame(o_snap, &outgoing).await.unwrap();
        o_rx.await.unwrap().unwrap();

        running
            .control(pointer_cmd(2, panel::PointerPhase::Down), &outgoing)
            .await
            .unwrap();
        let down = wire.try_recv().unwrap();
        let mut down_ack = Envelope::new(
            running.identity,
            next_seq(),
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        down_ack.in_reply_to = down.request_id;
        running.frame(down_ack, &outgoing).await.unwrap();

        running
            .control(pointer_cmd(2, panel::PointerPhase::Up), &outgoing)
            .await
            .unwrap();
        let up = wire.try_recv().unwrap();
        let mut up_err = Envelope::new(
            running.identity,
            next_seq(),
            1,
            envelope::Body::Error(Failure::new(ErrorCode::Invalid, "up failed")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        up_err.in_reply_to = up.request_id;
        running.frame(up_err, &outgoing).await.unwrap();

        let mut current_attempt = wire.try_recv().unwrap();
        for attempt in 1..MAX_RESYNC_RETRIES {
            let _ = attempt;
            running.requests.get_mut(&current_attempt.request_id).unwrap().deadline =
                Instant::now() - Duration::from_secs(1);
            running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
            let cancel_frame = wire.try_recv().unwrap();
            assert!(matches!(cancel_frame.body, Some(envelope::Body::Cancel(_))));
            current_attempt = wire.try_recv().unwrap();
        }
        assert_eq!(running.panels[&2].pending_pointer_cancels[0].attempts, MAX_RESYNC_RETRIES);

        let mut expired_high_ack = Envelope::new(
            running.identity,
            next_seq(),
            2,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        expired_high_ack.deadline_unix_ms = 1;
        expired_high_ack.in_reply_to = current_attempt.request_id;
        running.frame(expired_high_ack, &outgoing).await.unwrap();

        assert!(running.panels[&2].resync_semantic_drift);
        let cancel3 = wire.try_recv().unwrap();
        assert!(matches!(cancel3.body, Some(envelope::Body::Cancel(_))));
        let resync = wire.try_recv().unwrap();
        assert!(matches!(resync.body, Some(envelope::Body::Resync(ref s)) if s.revision == 1));
        assert!(wire.try_recv().is_err());

        let mut bare_resync_ack = Envelope::new(
            running.identity,
            next_seq(),
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        bare_resync_ack.in_reply_to = resync.request_id;
        running.frame(bare_resync_ack, &outgoing).await.unwrap();
        assert_eq!(running.panels[&2].resync_needed_seq, 0);
        assert!(running.panels[&2].pending_pointer_cancels.is_empty());
        assert!(running.panels[&2].resync_semantic_drift);
        assert!(wire.try_recv().is_err());
        while updates.try_recv().is_ok() {}

        running.control(select_cmd(2), &outgoing).await.unwrap();
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Error { panel_id: 2, ref failure, .. })
                if failure.code == ErrorCode::Stale as i32
        ));
    }

    // Subcase B & C: Timer fires FIRST on final 3rd Pointer(Cancel) attempt (exhausting budget),
    // THEN late high-rev Ack(2) arrives for both main panel and child panel 2.
    for target_panel in [protocol::PANEL_ID, 2u64] {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        *running.main_panel_mut() = PanelState::new(make_snap(1, ["A", "B"])).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        let mut seq = 1u64;
        let mut next_seq = || {
            let s = seq;
            seq += 1;
            s
        };

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
            let o_wire = wire.try_recv().unwrap();
            let mut o_snap = Envelope::new(
                running.identity,
                next_seq(),
                1,
                envelope::Body::Snapshot(make_snap(1, ["A", "B"])),
                MAX_OPERATION,
            )
            .with_panel_id(target_panel);
            o_snap.in_reply_to = o_wire.request_id;
            running.frame(o_snap, &outgoing).await.unwrap();
            o_rx.await.unwrap().unwrap();
        }

        running
            .control(pointer_cmd(target_panel, panel::PointerPhase::Down), &outgoing)
            .await
            .unwrap();
        let down = wire.try_recv().unwrap();
        let mut down_ack = Envelope::new(
            running.identity,
            next_seq(),
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        down_ack.in_reply_to = down.request_id;
        running.frame(down_ack, &outgoing).await.unwrap();

        running
            .control(pointer_cmd(target_panel, panel::PointerPhase::Up), &outgoing)
            .await
            .unwrap();
        let up = wire.try_recv().unwrap();
        let mut up_err = Envelope::new(
            running.identity,
            next_seq(),
            1,
            envelope::Body::Error(Failure::new(ErrorCode::Invalid, "up failed")),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        up_err.in_reply_to = up.request_id;
        running.frame(up_err, &outgoing).await.unwrap();

        let mut current_attempt = wire.try_recv().unwrap();
        for attempt in 1..=MAX_RESYNC_RETRIES {
            running.requests.get_mut(&current_attempt.request_id).unwrap().deadline =
                Instant::now() - Duration::from_secs(1);
            running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
            let cancel_frame = wire.try_recv().unwrap();
            assert!(matches!(cancel_frame.body, Some(envelope::Body::Cancel(_))));
            if attempt < MAX_RESYNC_RETRIES {
                current_attempt = wire.try_recv().unwrap();
            }
        }
        assert!(wire.try_recv().is_err());

        // Late high-revision Ack for final cancelled attempt #3 must latch semantic drift & emit Resync!
        let mut late_high_ack = Envelope::new(
            running.identity,
            next_seq(),
            2,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        late_high_ack.in_reply_to = current_attempt.request_id;
        running.frame(late_high_ack, &outgoing).await.unwrap();

        assert!(running.is_panel_semantic_drifted(target_panel));
        let resync = wire.try_recv().unwrap();
        assert!(matches!(resync.body, Some(envelope::Body::Resync(ref s)) if s.revision == 1));

        let mut bare_resync_ack = Envelope::new(
            running.identity,
            next_seq(),
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        bare_resync_ack.in_reply_to = resync.request_id;
        running.frame(bare_resync_ack, &outgoing).await.unwrap();
        assert!(!running.is_panel_resyncing(target_panel));
        assert!(running.is_panel_semantic_drifted(target_panel));
        while updates.try_recv().is_ok() {}

        running.control(select_cmd(target_panel), &outgoing).await.unwrap();
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Error { panel_id, ref failure, .. })
                if panel_id == target_panel && failure.code == ErrorCode::Stale as i32
        ));
    }
}
