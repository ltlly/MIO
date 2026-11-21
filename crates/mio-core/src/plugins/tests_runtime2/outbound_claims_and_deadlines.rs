use super::*;
use super::helpers::BlockingWriter;

#[tokio::test]
async fn real_duplex_supervisor_writer_blocked_skips_retracted_open_already_in_outgoing_channel()
{
    let identity = crate::plugins::tests_runtime::identity();
    let manifest = PluginManifest {
        id: "lab.outgoing-claim-skip".into(),
        version: "1.5.0".into(),
        api_version: 1,
        engine: "native".into(),
        entry: "worker".into(),
        permissions: vec![Permission::TrafficRead],
        contributions: vec!["suite_panel".into()],
    };
    let (host_to_worker, mut worker_rx) = tokio::io::duplex(512);
    let (mut worker_tx, worker_to_host) = tokio::io::duplex(64 * 1024);

    let res_a = Uuid::new_v4();
    let res_b = Uuid::new_v4();
    let res_c = Uuid::new_v4();
    let block_reader = Arc::new(tokio::sync::Notify::new());
    let unblock_reader = Arc::new(tokio::sync::Notify::new());
    let burst_sent = Arc::new(tokio::sync::Notify::new());
    let saw_open_c = Arc::new(tokio::sync::Notify::new());
    let block_reader_w = block_reader.clone();
    let unblock_reader_w = unblock_reader.clone();
    let burst_sent_w = burst_sent.clone();
    let saw_open_c_w = saw_open_c.clone();

    let received_opens = Arc::new(std::sync::Mutex::new(Vec::<(u64, Uuid, u64)>::new()));
    let received_closes = Arc::new(std::sync::Mutex::new(Vec::<(u64, Uuid, bool)>::new()));
    let received_cancels = Arc::new(std::sync::Mutex::new(Vec::<(u64, u64)>::new()));
    let open_c_req_id = Arc::new(std::sync::Mutex::new(None::<u64>));
    let opens_w = received_opens.clone();
    let closes_w = received_closes.clone();
    let cancels_w = received_cancels.clone();
    let open_c_req_id_w = open_c_req_id.clone();

    let send_frame = move |seq: &mut u64,
                           panel_id: u64,
                           rev: u64,
                           in_reply_to: u64,
                           body: envelope::Body| {
        *seq += 1;
        let mut env = Envelope::new(identity, *seq, rev, body, MAX_OPERATION).with_panel_id(panel_id);
        env.in_reply_to = in_reply_to;
        env
    };
    let make_snap = |rev: u64, label: &str| PanelSnapshot {
        revision: rev,
        title: label.into(),
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

    // Far fewer than `OUTGOING_CAPACITY` (256), enough to saturate the 512B duplex pipe and block
    // `spawn_stream_io`'s writer inside `stdin.write_all` while `outgoing` still has >230 free slots.
    let small_burst = 20usize;
    let worker_task = tokio::spawn(async move {
        let hello = read_async_frame(&mut worker_rx).await?;
        let mut out_seq = 0u64;
        let reg = send_frame(
            &mut out_seq,
            protocol::PANEL_ID,
            1,
            hello.request_id,
            envelope::Body::Register(protocol::Register {
                plugin_id: "lab.outgoing-claim-skip".into(),
                plugin_version: "1.5.0".into(),
                required_components: vec![],
                panel: Some(make_snap(1, "main")),
                supports_panel_multiplexing: false,
            }),
        );
        worker_tx.write_all(&protocol::encode_frame(&reg)?).await?;
        worker_tx.flush().await?;
        let _ready = read_async_frame(&mut worker_rx).await?;

        let mut rev = 1u64;
        let mut reader_blocked = false;
        loop {
            tokio::select! {
                biased;
                _ = block_reader_w.notified(), if !reader_blocked => {
                    reader_blocked = true;
                    for _ in 0..small_burst {
                        let hc = send_frame(
                            &mut out_seq,
                            protocol::PANEL_ID,
                            rev,
                            0,
                            envelope::Body::HostCall(HostCall {
                                operation: Some(host_call::Operation::QueryHistory(
                                    protocol::HistoryQuery {
                                        offset: 0,
                                        limit: 1,
                                        filter: String::new(),
                                    },
                                )),
                                parent_request_id: 999_999,
                            }),
                        );
                        worker_tx.write_all(&protocol::encode_frame(&hc)?).await?;
                    }
                    let fence = send_frame(
                        &mut out_seq,
                        protocol::PANEL_ID,
                        rev,
                        0,
                        envelope::Body::Error(Failure::new(ErrorCode::Internal, "pipe-filled")),
                    );
                    worker_tx.write_all(&protocol::encode_frame(&fence)?).await?;
                    worker_tx.flush().await?;
                    burst_sent_w.notify_one();
                }
                _ = unblock_reader_w.notified(), if reader_blocked => {
                    reader_blocked = false;
                }
                incoming = read_async_frame(&mut worker_rx), if !reader_blocked => {
                    let Ok(env) = incoming else { break; };
                    match env.body {
                        Some(envelope::Body::Shutdown(_)) => break,
                        Some(envelope::Body::OpenRequest(ref open)) => {
                            let rid = protocol::uuid(&open.resource_id).map_err(anyhow::Error::msg)?;
                            opens_w
                                .lock()
                                .unwrap()
                                .push((env.panel_id, rid, open.message_revision));
                            if rid == res_c {
                                *open_c_req_id_w.lock().unwrap() = Some(env.request_id);
                                saw_open_c_w.notify_one();
                            } else {
                                rev += 1;
                                let snap = send_frame(
                                    &mut out_seq,
                                    env.panel_id,
                                    rev,
                                    env.request_id,
                                    envelope::Body::Snapshot(make_snap(rev, "opened")),
                                );
                                worker_tx.write_all(&protocol::encode_frame(&snap)?).await?;
                                worker_tx.flush().await?;
                            }
                        }
                        Some(envelope::Body::Cancel(ref cancel)) => {
                            cancels_w
                                .lock()
                                .unwrap()
                                .push((env.panel_id, cancel.request_id));
                            let ack = send_frame(
                                &mut out_seq,
                                env.panel_id,
                                rev,
                                env.request_id,
                                envelope::Body::Ack(panel::Empty {}),
                            );
                            worker_tx.write_all(&protocol::encode_frame(&ack)?).await?;
                            worker_tx.flush().await?;
                        }
                        Some(envelope::Body::CloseRequest(ref close)) => {
                            let rid = protocol::uuid(&close.resource_id).map_err(anyhow::Error::msg)?;
                            closes_w
                                .lock()
                                .unwrap()
                                .push((env.panel_id, rid, close.destroy_panel));
                            let ack = send_frame(
                                &mut out_seq,
                                env.panel_id,
                                rev,
                                env.request_id,
                                envelope::Body::Ack(panel::Empty {}),
                            );
                            worker_tx.write_all(&protocol::encode_frame(&ack)?).await?;
                            worker_tx.flush().await?;
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok::<_, anyhow::Error>(())
    });

    let (mut supervisor, _services) = crate::plugins::supervisor::spawn_test_duplex_supervisor(
        identity,
        manifest,
        BTreeSet::from([Permission::TrafficRead]),
        host_to_worker,
        worker_to_host,
    )
    .await
    .unwrap();
    let handle = supervisor.handle();

    let _ = timeout(MAX_OPERATION, supervisor.events.recv()).await.unwrap().unwrap();
    handle
        .open_request(res_a, 1, false, request())
        .await
        .unwrap();
    let _ = timeout(MAX_OPERATION, supervisor.events.recv()).await.unwrap().unwrap();

    // Block worker reader and saturate the 512B duplex pipe with 20 HostResults so writer task blocks
    // inside `stdin.write_all`, while `outgoing` (256) still has free slots (`outbound_queue` is empty).
    block_reader.notify_one();
    timeout(MAX_OPERATION, burst_sent.notified()).await.unwrap();
    let fence_ev = timeout(MAX_OPERATION, supervisor.events.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        fence_ev,
        PluginEvent::Error { ref failure, .. } if failure.message == "pipe-filled"
    ));

    // Spawn `open_request(res_b)` -> moves directly into `outgoing` writer channel (not `outbound_queue`),
    // waiting behind the blocked writer task.
    let h_b = handle.clone();
    let mut open_b_task = tokio::spawn(async move {
        h_b.open_request(res_b, 1, false, request()).await
    });
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut open_b_task)
            .await
            .is_err(),
        "open_b must be parked inside blocked outgoing writer channel"
    );

    // Revoke `res_b` while writer is STILL blocked: must immediately retract `OutboundWriteClaims`
    // and fail `open_b_task` BEFORE `unblock_reader` fires!
    let revoke_b = handle.begin_revoke_panel_resource(protocol::PANEL_ID, res_b, false);
    assert!(
        timeout(MAX_OPERATION, open_b_task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );

    // Unblock writer -> writer claims & skips `OpenRequest(res_b)`, writing only `CloseRequest(res_b)`.
    unblock_reader.notify_one();
    timeout(MAX_OPERATION, async {
        let mut wait_fut = std::pin::pin!(revoke_b.wait());
        loop {
            tokio::select! {
                res = &mut wait_fut => {
                    res.unwrap();
                    break;
                }
                _ = supervisor.events.recv() => {}
            }
        }
    })
    .await
    .unwrap();

    assert_eq!(received_opens.lock().unwrap().as_slice(), &[(protocol::PANEL_ID, res_a, 1)]);
    assert!(received_cancels.lock().unwrap().is_empty());
    assert_eq!(
        received_closes.lock().unwrap().as_slice(),
        &[(protocol::PANEL_ID, res_b, false)]
    );

    // Contrast (Part B): `OpenRequest(res_c)` already claimed & received by worker still emits Cancel + CloseRequest.
    let h_c = handle.clone();
    let open_c_task = tokio::spawn(async move {
        h_c.open_request(res_c, 1, false, request()).await
    });
    timeout(MAX_OPERATION, saw_open_c.notified()).await.unwrap();
    let expected_c_req_id = open_c_req_id.lock().unwrap().unwrap();
    handle.revoke_resource(res_c).await.unwrap();
    assert!(timeout(MAX_OPERATION, open_c_task).await.unwrap().unwrap().is_err());

    assert_eq!(
        received_cancels.lock().unwrap().as_slice(),
        &[(protocol::PANEL_ID, expected_c_req_id)]
    );
    assert_eq!(
        received_closes.lock().unwrap().as_slice(),
        &[(protocol::PANEL_ID, res_b, false), (protocol::PANEL_ID, res_c, false)]
    );

    while supervisor.events.try_recv().is_ok() {}
    supervisor.stop().await.unwrap();
    timeout(MAX_OPERATION, worker_task).await.unwrap().unwrap().unwrap();
}

#[tokio::test]
async fn shutdown_and_fatal_timeout_retract_unclaimed_frames_before_sending_shutdown_or_cancel() {
    let make_snap = |rev: u64, ed_rev: u64, contents: &[u8], label: &str| PanelSnapshot {
        revision: rev,
        title: label.into(),
        root: Some(panel::Node::new(
            1,
            "ed",
            panel::node::Kind::Editor(panel::Editor {
                document_id: 1,
                revision: ed_rev,
                contents: contents.to_vec(),
                format: panel::EditorFormat::Utf8 as i32,
                read_only: false,
            }),
        )),
    };

    // Part A: Shutdown grace with blocked 512B duplex writer recovering during STOP_GRACE.
    {
        let (mut running, _calls, _updates) = running();
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

        // Bind `res_bound` cleanly while worker reader is draining `worker_rx`.
        let res_bound = Uuid::new_v4();
        let (ob_tx, ob_rx) = oneshot::channel();
        running
            .control_with_seq(
                1,
                Control::Open {
                    panel_id: protocol::PANEL_ID,
                    id: res_bound,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: ob_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let open_bound_env = timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
            .await
            .unwrap()
            .unwrap();
        let mut snap_bound = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, b"retained", "bound")),
            MAX_OPERATION,
        );
        snap_bound.in_reply_to = open_bound_env.request_id;
        running.frame(snap_bound, &outgoing).await.unwrap();
        ob_rx.await.unwrap().unwrap();

        // Stop reading `worker_rx` and saturate the 512B pipe with 20 HostResults so writer blocks mid-write.
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

        // Enqueue `Commit(res_bound)` and `Open(res_unsent)` into `outgoing` behind the blocked writer.
        let (cm_tx, mut cm_rx) = oneshot::channel();
        running
            .control_with_seq(
                2,
                Control::Commit {
                    panel_id: protocol::PANEL_ID,
                    op_id: Uuid::new_v4(),
                    request: CommitEditor {
                        resource_id: res_bound.as_bytes().to_vec(),
                        message_revision: 1,
                        request: Some(request().into()),
                        editor: Some(EditorSnapshot {
                            document_id: 1,
                            revision: 2,
                            contents: b"edited-stale".to_vec(),
                            ..Default::default()
                        }),
                    },
                    cancelled: CancellationToken::new(),
                    reply: cm_tx,
                                    expected_snapshot: None,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let res_unsent = Uuid::new_v4();
        let (ou_tx, mut ou_rx) = oneshot::channel();
        running
            .control_with_seq(
                3,
                Control::Open {
                    panel_id: protocol::PANEL_ID,
                    id: res_unsent,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: ou_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert!(cm_rx.try_recv().is_err());
        assert!(ou_rx.try_recv().is_err());

        // Shutdown path retracts unclaimed frames BEFORE sending Shutdown!
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
                    Some(other) => panic!("retracted frame leaked during shutdown grace: {other:?}"),
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

    // Part B: Fatal timeout retracts BOTH unclaimed frames in `outgoing` claims AND frames in `outbound_queue`,
    // emitting `Cancel` ONLY for expired frames already claimed by the writer.
    {
        let (mut running, _calls, _updates) = running();
        let claims = Arc::new(OutboundWriteClaims::default());
        running.outbound_claims = Some(claims.clone());
        let (outgoing, mut wire) = mpsc::channel(4);

        let (oc_tx, _oc_rx) = oneshot::channel();
        running
            .control_with_seq(
                1,
                Control::Open {
                    panel_id: protocol::PANEL_ID,
                    id: Uuid::new_v4(),
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: oc_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let env_claimed = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(protocol::PANEL_ID, env_claimed.request_id));

        let (ou_tx, _ou_rx) = oneshot::channel();
        running
            .control_with_seq(
                2,
                Control::Open {
                    panel_id: protocol::PANEL_ID,
                    id: Uuid::new_v4(),
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: ou_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let env_unclaimed = wire.try_recv().unwrap();
        // Do NOT call claim_for_write(env_unclaimed) — simulates unclaimed frame inside `writes` channel.

        // Force a 3rd frame into `outbound_queue`, then free `outgoing` capacity before timeout fires.
        for s in 901..=904u64 {
            running
                .send(
                    &outgoing,
                    envelope::Body::Cancel(protocol::Cancel { request_id: s }),
                    0,
                )
                .unwrap();
        }
        let (oq_tx, _oq_rx) = oneshot::channel();
        running
            .control_with_seq(
                3,
                Control::Open {
                    panel_id: protocol::PANEL_ID,
                    id: Uuid::new_v4(),
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: oq_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let req_queued = running.outbound_queue.back().unwrap().request_id;
        for _ in 0..4 {
            let _ = wire.try_recv().unwrap();
        }
        running
            .requests
            .get_mut(&env_claimed.request_id)
            .unwrap()
            .deadline = Instant::now() - Duration::from_secs(1);

        assert!(running.handle_timeouts(Instant::now(), &outgoing).await.is_err());
        assert!(running.outbound_queue.is_empty());
        assert!(!claims.claim_for_write(protocol::PANEL_ID, env_unclaimed.request_id));
        assert!(!claims.try_retract(protocol::PANEL_ID, req_queued));

        let cancel_env = wire.try_recv().unwrap();
        assert!(matches!(
            cancel_env.body,
            Some(envelope::Body::Cancel(protocol::Cancel { request_id })) if request_id == env_claimed.request_id
        ));
        assert!(wire.try_recv().is_err());
    }
}

#[tokio::test]
async fn child_editor_changed_retracted_on_timeout_schedules_resync_and_keeps_worker_consistent() {
    let make_snap = |rev: u64, ed_rev: u64, contents: &[u8], label: &str| PanelSnapshot {
        revision: rev,
        title: label.into(),
        root: Some(panel::Node::new(
            1,
            "ed",
            panel::node::Kind::Editor(panel::Editor {
                document_id: 1,
                revision: ed_rev,
                contents: contents.to_vec(),
                format: panel::EditorFormat::Utf8 as i32,
                read_only: false,
            }),
        )),
    };

    let (mut running, _calls, _updates) = running();
    running.supports_panel_multiplexing = true;
    let claims = Arc::new(OutboundWriteClaims::default());
    running.outbound_claims = Some(claims.clone());
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

    // 1. Open child panel 2 with editor v1.
    let res_primary = Uuid::new_v4();
    let (o_tx, o_rx) = oneshot::channel();
    running
        .control_with_seq(
            1,
            Control::Open {
                panel_id: 2,
                id: res_primary,
                revision: 1,
                read_only: false,
                request: request(),
                reply: o_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let ow = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, ow.request_id));
    let mut s1 = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(make_snap(1, 1, b"v1", "c2-v1")),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    s1.in_reply_to = ow.request_id;
    running.frame(s1, &outgoing).await.unwrap();
    o_rx.await.unwrap().unwrap();
    while wire.try_recv().is_ok() {}

    // 2. Submit mutating `EditorChanged(v2)` on child panel 2, updating host state BEFORE leaving outgoing unclaimed.
    running
        .control_with_seq(
            2,
            Control::Event {
                panel_id: 2,
                expected_snapshot: None,
                event: UiEvent {
                    node_id: 1,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::EditorChanged(EditorSnapshot {
                        document_id: 1,
                        revision: 2,
                        contents: b"user-typed-v2".to_vec(),
                        ..Default::default()
                    })),
                },
                event_seq: 0,
            },
            &outgoing,
        )
        .await
        .unwrap();
    {
        let ed = running.editor_on_panel(2, 1).unwrap();
        assert_eq!(ed.revision, 2);
        assert_eq!(ed.contents, b"user-typed-v2");
    }
    let editor_ev = wire.try_recv().unwrap();
    // Do NOT call claim_for_write(2, editor_ev.request_id): event sits unclaimed in `outgoing`.

    // 3. Event times out -> retracted from `outgoing` (writer skips it, NO Cancel emitted), host preserves
    //    `user-typed-v2` AND schedules bounded `Resync(user-typed-v2)`!
    running
        .handle_timeouts(Instant::now() + MAX_OPERATION + Duration::from_secs(1), &outgoing)
        .await
        .unwrap();
    assert!(!claims.claim_for_write(2, editor_ev.request_id));
    {
        let ed = running.editor_on_panel(2, 1).unwrap();
        assert_eq!(ed.revision, 2);
        assert_eq!(ed.contents, b"user-typed-v2");
    }
    let resync_w = wire.try_recv().unwrap();
    assert_eq!(resync_w.panel_id, 2);
    assert!(matches!(
        resync_w.body,
        Some(envelope::Body::Resync(ref snap))
            if matches!(
                snap.root.as_ref().and_then(|n| n.kind.as_ref()),
                Some(panel::node::Kind::Editor(ed))
                    if ed.revision == 2 && ed.contents == b"user-typed-v2"
            )
    ));
    assert!(wire.try_recv().is_err());
    assert!(claims.claim_for_write(2, resync_w.request_id));

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
    assert!(!running.is_panel_resyncing(2));

    // Commit after Resync settles using user-typed-v2 succeeds cleanly!
    let (cm_tx, cm_rx) = oneshot::channel();
    running
        .control_with_seq(
            3,
            Control::Commit {
                panel_id: 2,
                op_id: Uuid::new_v4(),
                request: CommitEditor {
                    resource_id: res_primary.as_bytes().to_vec(),
                    message_revision: 1,
                    request: Some(request().into()),
                    editor: Some(EditorSnapshot {
                        document_id: 1,
                        revision: 2,
                        contents: b"user-typed-v2".to_vec(),
                        ..Default::default()
                    }),
                },
                cancelled: CancellationToken::new(),
                reply: cm_tx,
                            expected_snapshot: None,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let cm_w = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, cm_w.request_id));
    let mut committed = Envelope::new(
        running.identity,
        3,
        1,
        envelope::Body::Committed(protocol::Committed {
            resource_id: res_primary.as_bytes().to_vec(),
            message_revision: 1,
            document_id: 1,
            document_revision: 2,
            request: Some(request().into()),
        }),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    committed.in_reply_to = cm_w.request_id;
    running.frame(committed, &outgoing).await.unwrap();
    assert_eq!(cm_rx.await.unwrap().unwrap().url, request().url);

    // 4. Negative control: retracting an unclaimed non-mutating `Focus` event does NOT trigger unnecessary Resync.
    running
        .control_with_seq(
            4,
            Control::Event {
                panel_id: 2,
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
    let focus_ev = wire.try_recv().unwrap();
    running
        .handle_timeouts(Instant::now() + MAX_OPERATION + Duration::from_secs(2), &outgoing)
        .await
        .unwrap();
    assert!(!claims.claim_for_write(2, focus_ev.request_id));
    assert!(wire.try_recv().is_err());
}

#[tokio::test]
async fn main_panel_local_expired_reply_retracts_unclaimed_open_before_sending_fatal_cancel() {
    let make_snap = |rev: u64, ed_rev: u64, contents: &[u8], label: &str| PanelSnapshot {
        revision: rev,
        title: label.into(),
        root: Some(panel::Node::new(
            1,
            "ed",
            panel::node::Kind::Editor(panel::Editor {
                document_id: 1,
                revision: ed_rev,
                contents: contents.to_vec(),
                format: panel::EditorFormat::Utf8 as i32,
                read_only: false,
            }),
        )),
    };

    // Part A: Main panel local-pending-expired late reply with blocked 512B duplex writer retracts
    // another unexpired unclaimed Open in `outgoing` BEFORE sending fatal Cancel (before `prepare_shutdown`).
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

        let res_main = Uuid::new_v4();
        let (om_tx, om_rx) = oneshot::channel();
        running
            .control_with_seq(
                1,
                Control::Open {
                    panel_id: protocol::PANEL_ID,
                    id: res_main,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: om_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let open_main_env = timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
            .await
            .unwrap()
            .unwrap();

        // Saturate 512B pipe so writer task blocks mid-write before draining newer frames.
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

        let res_child = Uuid::new_v4();
        let (oc_tx, _oc_rx) = oneshot::channel();
        running
            .control_with_seq(
                2,
                Control::Open {
                    panel_id: 2,
                    id: res_child,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: oc_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();

        running
            .requests
            .get_mut(&open_main_env.request_id)
            .unwrap()
            .deadline = Instant::now() - Duration::from_secs(1);

        let mut late_snap = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, b"retained", "late-main")),
            MAX_OPERATION,
        );
        late_snap.in_reply_to = open_main_env.request_id;
        let err = running
            .frame(late_snap, &outgoing)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("插件操作超时"), "unexpected fatal err: {err}");
        let om_err = om_rx.await.unwrap().unwrap_err().to_string();
        assert!(om_err.contains("主面板操作已超时"), "unexpected main reply: {om_err}");

        // Writer recovers BEFORE `prepare_shutdown`: Cancel arrives and unexpired child Open never leaks.
        let mut saw_cancel = false;
        timeout(MAX_OPERATION, async {
            while let Ok(env) = read_async_frame(&mut worker_rx).await {
                match env.body {
                    Some(envelope::Body::HostResult(_)) => {}
                    Some(envelope::Body::Cancel(c)) if c.request_id == open_main_env.request_id => {
                        saw_cancel = true;
                        break;
                    }
                    Some(other) => {
                        panic!("unclaimed OpenRequest leaked before prepare_shutdown: {other:?}")
                    }
                    None => {}
                }
            }
        })
        .await
        .unwrap();
        assert!(saw_cancel);

        io_cancel.cancel();
        io_tasks.abort_all();
    }

    // Part B: `outbound_queue` implicit flush guard — sending fatal Cancel must not flush an
    // unexpired Open sitting in `outbound_queue` into a freed `outgoing` slot.
    {
        let (mut running, _calls, _updates) = running();
        let claims = Arc::new(OutboundWriteClaims::default());
        running.outbound_claims = Some(claims.clone());
        let (outgoing, mut wire) = mpsc::channel(4);

        let res1 = Uuid::new_v4();
        let (o1_tx, o1_rx) = oneshot::channel();
        running
            .control_with_seq(
                1,
                Control::Open {
                    panel_id: protocol::PANEL_ID,
                    id: res1,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: o1_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let o1_env = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(protocol::PANEL_ID, o1_env.request_id));
        let mut o1_snap = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, b"retained", "res1")),
            MAX_OPERATION,
        );
        o1_snap.in_reply_to = o1_env.request_id;
        running.frame(o1_snap, &outgoing).await.unwrap();
        o1_rx.await.unwrap().unwrap();

        let (cm_tx, cm_rx) = oneshot::channel();
        running
            .control_with_seq(
                2,
                Control::Commit {
                    panel_id: protocol::PANEL_ID,
                    op_id: Uuid::new_v4(),
                    request: CommitEditor {
                        resource_id: res1.as_bytes().to_vec(),
                        message_revision: 1,
                        request: Some(request().into()),
                        editor: Some(EditorSnapshot {
                            document_id: 1,
                            revision: 2,
                            contents: b"edited".to_vec(),
                            ..Default::default()
                        }),
                    },
                    cancelled: CancellationToken::new(),
                    reply: cm_tx,
                                    expected_snapshot: None,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let cm_env = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(protocol::PANEL_ID, cm_env.request_id));

        for s in 1..=4u64 {
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

        let res2 = Uuid::new_v4();
        let (o2_tx, _o2_rx) = oneshot::channel();
        running
            .control_with_seq(
                3,
                Control::Open {
                    panel_id: protocol::PANEL_ID,
                    id: res2,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: o2_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert!(running.outbound_queue.iter().any(|e| {
            matches!(e.body, Some(envelope::Body::OpenRequest(_)))
        }));

        // Free 1 slot in `outgoing` so any unguarded flush would move Open(res2) into `outgoing`.
        let first_hr = wire.try_recv().unwrap();
        assert!(matches!(first_hr.body, Some(envelope::Body::HostResult(_))));

        running
            .requests
            .get_mut(&cm_env.request_id)
            .unwrap()
            .deadline = Instant::now() - Duration::from_secs(1);

        let mut late_committed = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Committed(protocol::Committed {
                resource_id: res1.as_bytes().to_vec(),
                message_revision: 1,
                document_id: 1,
                document_revision: 2,
                request: Some(request().into()),
            }),
            MAX_OPERATION,
        );
        late_committed.in_reply_to = cm_env.request_id;
        let err = running
            .frame(late_committed, &outgoing)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("插件操作超时"), "unexpected fatal err: {err}");
        assert!(cm_rx.await.unwrap().is_err());
        assert!(!running.outbound_queue.iter().any(|e| {
            matches!(e.body, Some(envelope::Body::OpenRequest(_)))
        }));

        let mut drained = Vec::new();
        while let Ok(env) = wire.try_recv() {
            drained.push(env);
        }
        assert!(
            !drained
                .iter()
                .any(|e| matches!(e.body, Some(envelope::Body::OpenRequest(_)))),
            "OpenRequest leaked into outgoing before prepare_shutdown"
        );
        assert!(
            drained.iter().any(|e| matches!(
                e.body,
                Some(envelope::Body::Cancel(ref c)) if c.request_id == cm_env.request_id
            )),
            "expected fatal Cancel for expired in-flight Commit"
        );
    }
}

#[tokio::test]
async fn running_loop_fatal_timeout_retracts_outbound_queue_before_write_permit_or_late_reply() {
    use tokio::io::AsyncWriteExt as _;

    let make_snap = |rev: u64, ed_rev: u64, contents: &[u8], label: &str| PanelSnapshot {
        revision: rev,
        title: label.into(),
        root: Some(panel::Node::new(
            1,
            "ed",
            panel::node::Kind::Editor(panel::Editor {
                document_id: 1,
                revision: ed_rev,
                contents: contents.to_vec(),
                format: panel::EditorFormat::Utf8 as i32,
                read_only: false,
            }),
        )),
    };

    // Part A: Loop-top check in real `Running::run` — expired main request, unexpired Open in
    // `outbound_queue`, empty `outgoing` (write permit ready), and pre-written late Worker
    // Snapshot reply all ready simultaneously at entry; `Running::run` must fail fatally WITHOUT
    // flushing the queued OpenRequest.
    {
        let (mut running, _calls, _updates) = running();
        let (_cmd_tx, cmd_rx) = mpsc::channel(QUEUE_CAPACITY);
        let (_urgent_tx, urgent_rx) = mpsc::unbounded_channel();

        let (om_tx, om_rx) = oneshot::channel();
        running.requests.insert(
            1,
            Pending {
                panel_id: protocol::PANEL_ID,
                panel_revision_at_send: 0,
                deadline: Instant::now() - Duration::from_secs(1),
                cancel: CancellationToken::new(),
                from_deferred_queue: false,
                source_seq: 1,
                kind: PendingKind::Open {
                    resource: Uuid::new_v4(),
                    revision: 1,
                    read_only: false,
                    reserved: false,
                    reply: om_tx,
                },
            },
        );
        let (oq_tx, oq_rx) = oneshot::channel();
        let queued_open_env = Envelope::new(
            running.identity,
            2,
            1,
            envelope::Body::OpenRequest(protocol::OpenRequest {
                resource_id: Uuid::new_v4().as_bytes().to_vec(),
                message_revision: 1,
                read_only: false,
                request: Some(request().into()),
            }),
            MAX_OPERATION,
        );
        running.outbound_queue.push_back(queued_open_env);
        running.requests.insert(
            2,
            Pending {
                panel_id: protocol::PANEL_ID,
                panel_revision_at_send: 0,
                deadline: Instant::now() + MAX_OPERATION,
                cancel: CancellationToken::new(),
                from_deferred_queue: false,
                source_seq: 2,
                kind: PendingKind::Open {
                    resource: Uuid::new_v4(),
                    revision: 1,
                    read_only: false,
                    reserved: false,
                    reply: oq_tx,
                },
            },
        );
        running.outgoing_sequence = 2;

        let (host_to_worker, mut worker_rx) = tokio::io::duplex(64 * 1024);
        let (mut worker_tx, worker_to_host) = tokio::io::duplex(64 * 1024);
        let mut late_snap = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, b"retained", "late-a")),
            MAX_OPERATION,
        );
        late_snap.in_reply_to = 1;
        worker_tx
            .write_all(&protocol::encode_frame(&late_snap).unwrap())
            .await
            .unwrap();

        let reader_task = tokio::spawn(async move {
            let mut frames = Vec::new();
            while let Ok(env) = read_async_frame(&mut worker_rx).await {
                frames.push(env);
            }
            frames
        });

        let err = timeout(
            MAX_OPERATION,
            running.run(
                WorkerTransport::Duplex {
                    stdin: host_to_worker,
                    stdout: worker_to_host,
                },
                cmd_rx,
                urgent_rx,
            ),
        )
        .await
        .unwrap()
        .unwrap_err()
        .to_string();
        assert!(err.contains("插件操作超时"), "unexpected fatal err: {err}");
        assert!(om_rx.await.unwrap().is_err());
        assert!(oq_rx.await.unwrap().is_err());

        let frames = reader_task.await.unwrap();
        assert!(
            !frames
                .iter()
                .any(|e| matches!(e.body, Some(envelope::Body::OpenRequest(_)))),
            "unexpired queued OpenRequest leaked during Running::run loop-top race: {frames:?}"
        );
    }

    // Part B: Active `tokio::select!` race in real `Running::run` — throttled reader continuously
    // wakes `outgoing.reserve()` across fatal expiry + late Worker reply; unexpired OpenRequest
    // behind HostResults in `outbound_queue` must still be retracted rather than flushed.
    {
        let (mut running, _calls, _updates) = running();
        let (cmd_tx, cmd_rx) = mpsc::channel(QUEUE_CAPACITY);
        let (_urgent_tx, urgent_rx) = mpsc::unbounded_channel();

        let (om_tx, om_rx) = oneshot::channel();
        running.requests.insert(
            1,
            Pending {
                panel_id: protocol::PANEL_ID,
                panel_revision_at_send: 0,
                deadline: Instant::now() + Duration::from_millis(80),
                cancel: CancellationToken::new(),
                from_deferred_queue: false,
                source_seq: 1,
                kind: PendingKind::Open {
                    resource: Uuid::new_v4(),
                    revision: 1,
                    read_only: false,
                    reserved: false,
                    reply: om_tx,
                },
            },
        );
        for seq in 2..=(OUTGOING_CAPACITY as u64 + 14) {
            running.outbound_queue.push_back(Envelope::new(
                running.identity,
                seq,
                1,
                envelope::Body::HostResult(HostResult {
                    outcome: Some(host_result::Outcome::Done(panel::Empty {})),
                }),
                MAX_OPERATION,
            ));
        }
        running.outgoing_sequence = OUTGOING_CAPACITY as u64 + 14;
        let identity = running.identity;

        let (host_to_worker, mut worker_rx) = tokio::io::duplex(512);
        let (mut worker_tx, worker_to_host) = tokio::io::duplex(64 * 1024);

        let reader_task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(15)).await;
            let mut frames = Vec::new();
            while let Ok(env) = read_async_frame(&mut worker_rx).await {
                frames.push(env);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            frames
        });

        let worker_reply_task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            let mut late_snap = Envelope::new(
                identity,
                1,
                2,
                envelope::Body::Snapshot(make_snap(2, 1, b"retained", "late-b")),
                MAX_OPERATION,
            );
            late_snap.in_reply_to = 1;
            let _ = worker_tx
                .write_all(&protocol::encode_frame(&late_snap).unwrap())
                .await;
            worker_tx
        });

        let run_handle = tokio::spawn(async move {
            running
                .run(
                    WorkerTransport::Duplex {
                        stdin: host_to_worker,
                        stdout: worker_to_host,
                    },
                    cmd_rx,
                    urgent_rx,
                )
                .await
        });

        let (oq_tx, oq_rx) = oneshot::channel();
        cmd_tx
            .send(SequencedControl {
                seq: 2,
                command: Control::Open {
                    panel_id: protocol::PANEL_ID,
                    id: Uuid::new_v4(),
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: oq_tx,
                },
            })
            .await
            .unwrap();

        let err = timeout(MAX_OPERATION, run_handle)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .to_string();
        assert!(err.contains("插件操作超时"), "unexpected fatal err: {err}");
        assert!(om_rx.await.unwrap().is_err());
        assert!(oq_rx.await.unwrap().is_err());

        let _keep_tx = worker_reply_task.await.unwrap();
        let frames = timeout(MAX_OPERATION, reader_task).await.unwrap().unwrap();
        assert!(
            !frames
                .iter()
                .any(|e| matches!(e.body, Some(envelope::Body::OpenRequest(_)))),
            "queued OpenRequest leaked past write permit / fatal deadline race"
        );
    }
}

#[tokio::test]
async fn event_backpressure_crossing_fatal_deadline_aborts_before_post_await_flush() {
    let make_snap = |rev: u64, ed_rev: u64, contents: &[u8], label: &str| PanelSnapshot {
        revision: rev,
        title: label.into(),
        root: Some(panel::Node::new(
            1,
            "ed",
            panel::node::Kind::Editor(panel::Editor {
                document_id: 1,
                revision: ed_rev,
                contents: contents.to_vec(),
                format: panel::EditorFormat::Utf8 as i32,
                read_only: false,
            }),
        )),
    };

    // Part A: `handle_timeouts` suspends on saturated `events` channel while emitting child timeout
    // diagnostic; meanwhile a main-panel request crosses its fatal deadline. Once unblocked, urgent
    // Revoke / CloseRequest must NOT push/flush any unexpired outbound frame.
    {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        let (urgent_tx, urgent_rx) = mpsc::unbounded_channel();
        running.urgent_commands = Some(urgent_rx);
        let claims = Arc::new(OutboundWriteClaims::default());
        running.outbound_claims = Some(claims.clone());
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        let res_child = Uuid::new_v4();
        let (oc_tx, oc_rx) = oneshot::channel();
        running
            .control_with_seq(
                1,
                Control::Open {
                    panel_id: 2,
                    id: res_child,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: oc_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let child_open_env = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(2, child_open_env.request_id));
        let mut child_snap = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Snapshot(make_snap(1, 1, b"child", "child")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        child_snap.in_reply_to = child_open_env.request_id;
        running.frame(child_snap, &outgoing).await.unwrap();
        oc_rx.await.unwrap().unwrap();
        while updates.try_recv().is_ok() {}

        let (ce_tx, ce_rx) = oneshot::channel();
        running
            .control_with_seq(
                2,
                Control::Open {
                    panel_id: 2,
                    id: Uuid::new_v4(),
                    revision: 2,
                    read_only: false,
                    request: request(),
                    reply: ce_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let child_expired_env = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(2, child_expired_env.request_id));
        running
            .requests
            .get_mut(&child_expired_env.request_id)
            .unwrap()
            .deadline = Instant::now() - Duration::from_secs(1);

        let (om_tx, _om_rx) = oneshot::channel();
        running
            .control_with_seq(
                3,
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
        let main_env = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(protocol::PANEL_ID, main_env.request_id));
        running
            .requests
            .get_mut(&main_env.request_id)
            .unwrap()
            .deadline = Instant::now() + Duration::from_millis(25);

        // Fill outgoing channel completely, push an unexpired Open into `outbound_queue`, then free 1 slot.
        for s in 1..=(OUTGOING_CAPACITY as u64) {
            running
                .send(
                    &outgoing,
                    envelope::Body::Cancel(protocol::Cancel { request_id: s + 10_000 }),
                    0,
                )
                .unwrap();
        }
        let (oq_tx, _oq_rx) = oneshot::channel();
        running
            .control_with_seq(
                4,
                Control::Open {
                    panel_id: protocol::PANEL_ID,
                    id: Uuid::new_v4(),
                    revision: 2,
                    read_only: false,
                    request: request(),
                    reply: oq_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let req_unexpired = running.outbound_queue.back().unwrap().request_id;
        assert!(matches!(wire.try_recv().unwrap().body, Some(envelope::Body::Cancel(_))));

        // Saturate `running.events` so `diagnostic_on_panel` blocks across `main_env`'s deadline.
        while running
            .events
            .try_send(PluginEvent::Stopped {
                identity: running.identity,
                reason: "fill".into(),
            })
            .is_ok()
        {}

        let helper = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let (rev_tx, rev_rx) = oneshot::channel();
            let _ = urgent_tx.send(SequencedControl {
                seq: 5,
                command: Control::Revoke {
                    panel_id: 2,
                    id: res_child,
                    remove_panel: false,
                    reply: rev_tx,
                },
            });
            while updates.try_recv().is_ok() {}
            (urgent_tx, updates, rev_rx)
        });

        let err = running
            .handle_timeouts(Instant::now(), &outgoing)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("插件操作超时"), "unexpected fatal err: {err}");
        let _ = ce_rx.await;
        let _keep = helper.await.unwrap();

        assert!(!running.outbound_queue.iter().any(|e| {
            matches!(e.body, Some(envelope::Body::OpenRequest(_)))
        }));
        assert!(!claims.try_retract(protocol::PANEL_ID, req_unexpired));
        let mut drained = Vec::new();
        while let Ok(env) = wire.try_recv() {
            drained.push(env);
        }
        assert!(
            !drained
                .iter()
                .any(|e| matches!(e.body, Some(envelope::Body::OpenRequest(_)))),
            "OpenRequest leaked into outgoing after diagnostic await crossed fatal deadline"
        );
        assert!(
            drained.iter().any(|e| matches!(
                e.body,
                Some(envelope::Body::Cancel(ref c)) if c.request_id == main_env.request_id
            )),
            "expected fatal Cancel for main_env"
        );
    }

    // Part B: `io.rs` `Body::Error` suspends on saturated `events` channel while `main_env` crosses
    // its fatal deadline; resuming to `flush_pending_revokes` must abort fatally BEFORE urgent Revoke
    // flushes `outbound_queue`.
    {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        let (urgent_tx, urgent_rx) = mpsc::unbounded_channel();
        running.urgent_commands = Some(urgent_rx);
        let claims = Arc::new(OutboundWriteClaims::default());
        running.outbound_claims = Some(claims.clone());
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        let res_child = Uuid::new_v4();
        let (oc_tx, oc_rx) = oneshot::channel();
        running
            .control_with_seq(
                1,
                Control::Open {
                    panel_id: 2,
                    id: res_child,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: oc_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let child_open_env = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(2, child_open_env.request_id));
        let mut child_snap = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Snapshot(make_snap(1, 1, b"child", "child")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        child_snap.in_reply_to = child_open_env.request_id;
        running.frame(child_snap, &outgoing).await.unwrap();
        oc_rx.await.unwrap().unwrap();
        while updates.try_recv().is_ok() {}

        let (ce_tx, ce_rx) = oneshot::channel();
        running
            .control_with_seq(
                2,
                Control::Open {
                    panel_id: 2,
                    id: Uuid::new_v4(),
                    revision: 2,
                    read_only: false,
                    request: request(),
                    reply: ce_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let child_err_req = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(2, child_err_req.request_id));

        let (om_tx, _om_rx) = oneshot::channel();
        running
            .control_with_seq(
                3,
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
        let main_env = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(protocol::PANEL_ID, main_env.request_id));
        running
            .requests
            .get_mut(&main_env.request_id)
            .unwrap()
            .deadline = Instant::now() + Duration::from_millis(25);

        for s in 1..=(OUTGOING_CAPACITY as u64) {
            running
                .send(
                    &outgoing,
                    envelope::Body::Cancel(protocol::Cancel { request_id: s + 20_000 }),
                    0,
                )
                .unwrap();
        }
        let (oq_tx, _oq_rx) = oneshot::channel();
        running
            .control_with_seq(
                4,
                Control::Open {
                    panel_id: protocol::PANEL_ID,
                    id: Uuid::new_v4(),
                    revision: 2,
                    read_only: false,
                    request: request(),
                    reply: oq_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let req_unexpired = running.outbound_queue.back().unwrap().request_id;
        assert!(matches!(wire.try_recv().unwrap().body, Some(envelope::Body::Cancel(_))));

        while running
            .events
            .try_send(PluginEvent::Stopped {
                identity: running.identity,
                reason: "fill".into(),
            })
            .is_ok()
        {}

        let helper = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let (rev_tx, rev_rx) = oneshot::channel();
            let _ = urgent_tx.send(SequencedControl {
                seq: 5,
                command: Control::Revoke {
                    panel_id: 2,
                    id: res_child,
                    remove_panel: false,
                    reply: rev_tx,
                },
            });
            while updates.try_recv().is_ok() {}
            (urgent_tx, updates, rev_rx)
        });

        let mut child_err_frame = Envelope::new(
            running.identity,
            2,
            1,
            envelope::Body::Error(Failure::new(ErrorCode::Invalid, "child op failed")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        child_err_frame.in_reply_to = child_err_req.request_id;
        let err = running
            .frame(child_err_frame, &outgoing)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("插件操作超时"), "unexpected fatal err: {err}");
        let _ = ce_rx.await;
        let _keep = helper.await.unwrap();

        assert!(!running.outbound_queue.iter().any(|e| {
            matches!(e.body, Some(envelope::Body::OpenRequest(_)))
        }));
        assert!(!claims.try_retract(protocol::PANEL_ID, req_unexpired));
        let mut drained = Vec::new();
        while let Ok(env) = wire.try_recv() {
            drained.push(env);
        }
        assert!(
            !drained
                .iter()
                .any(|e| matches!(e.body, Some(envelope::Body::OpenRequest(_)))),
            "OpenRequest leaked into outgoing after frame diagnostic await crossed fatal deadline"
        );
        assert!(
            drained.iter().any(|e| matches!(
                e.body,
                Some(envelope::Body::Cancel(ref c)) if c.request_id == main_env.request_id
            )),
            "expected fatal Cancel for main_env"
        );
    }
}

#[tokio::test]
async fn writer_task_skips_in_writes_open_when_fatal_deadline_expires_during_event_backpressure() {
    let make_snap = |rev: u64, ed_rev: u64, contents: &[u8], label: &str| PanelSnapshot {
        revision: rev,
        title: label.into(),
        root: Some(panel::Node::new(
            1,
            "ed",
            panel::node::Kind::Editor(panel::Editor {
                document_id: 1,
                revision: ed_rev,
                contents: contents.to_vec(),
                format: panel::EditorFormat::Utf8 as i32,
                read_only: false,
            }),
        )),
    };

    let (mut running, _calls, mut updates) = running();
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

    // 1. Open child panel 2 cleanly.
    let res_child = Uuid::new_v4();
    let (oc_tx, oc_rx) = oneshot::channel();
    running
        .control_with_seq(
            1,
            Control::Open {
                panel_id: 2,
                id: res_child,
                revision: 1,
                read_only: false,
                request: request(),
                reply: oc_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let child_open_env = timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
        .await
        .unwrap()
        .unwrap();
    let mut child_snap = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(make_snap(1, 1, b"child", "child")),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    child_snap.in_reply_to = child_open_env.request_id;
    running.frame(child_snap, &outgoing).await.unwrap();
    oc_rx.await.unwrap().unwrap();
    while updates.try_recv().is_ok() {}

    // 2. Send main-panel Open (already written to Worker) and an expired child Open (triggers diagnostic await).
    let (om_tx, _om_rx) = oneshot::channel();
    running
        .control_with_seq(
            2,
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
    let main_env = timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
        .await
        .unwrap()
        .unwrap();

    let (ce_tx, ce_rx) = oneshot::channel();
    running
        .control_with_seq(
            3,
            Control::Open {
                panel_id: 2,
                id: Uuid::new_v4(),
                revision: 2,
                read_only: false,
                request: request(),
                reply: ce_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let child_expired_env = timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
        .await
        .unwrap()
        .unwrap();
    running
        .requests
        .get_mut(&child_expired_env.request_id)
        .unwrap()
        .deadline = Instant::now() - Duration::from_secs(1);

    // 3. Saturate 512B pipe with 20 HostResults so writer blocks mid-write, then push unexpired
    // main Open directly into `writes` channel (`outgoing`), NOT `outbound_queue`.
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

    let (oq_tx, _oq_rx) = oneshot::channel();
    running
        .control_with_seq(
            4,
            Control::Open {
                panel_id: protocol::PANEL_ID,
                id: Uuid::new_v4(),
                revision: 2,
                read_only: false,
                request: request(),
                reply: oq_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(running.outbound_queue.is_empty());

    // 4. Arrange main_env fatal deadline +30ms and saturate `running.events`.
    running
        .requests
        .get_mut(&main_env.request_id)
        .unwrap()
        .deadline = Instant::now() + Duration::from_millis(30);
    running.sync_fatal_deadline_to_claims();

    while running
        .events
        .try_send(PluginEvent::Stopped {
            identity: running.identity,
            reason: "fill".into(),
        })
        .is_ok()
    {}

    let main_req_id = main_env.request_id;
    let child_expired_id = child_expired_env.request_id;
    let helper = tokio::spawn(async move {
        // Wait until main_env crosses fatal deadline while Running is STILL suspended in diagnostic.await.
        tokio::time::sleep(Duration::from_millis(55)).await;
        // Recover writer pipe BEFORE unblocking `events` so writer pops `Open` from `writes` while
        // `Running` is still blocked on event backpressure.
        for _ in 0..20 {
            let env = timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
                .await
                .unwrap()
                .unwrap();
            assert!(
                matches!(env.body, Some(envelope::Body::HostResult(_))),
                "unexpected frame before draining 20 HostResults: {env:?}"
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Now unblock `events` and confirm Cancel(child_expired_id) + Cancel(main_req_id) arrive with ZERO OpenRequest leaked.
        while updates.try_recv().is_ok() {}
        let mut saw_child_cancel = false;
        let mut saw_main_cancel = false;
        timeout(MAX_OPERATION, async {
            while let Ok(env) = read_async_frame(&mut worker_rx).await {
                match env.body {
                    Some(envelope::Body::Cancel(c)) if c.request_id == child_expired_id => {
                        saw_child_cancel = true;
                    }
                    Some(envelope::Body::Cancel(c)) if c.request_id == main_req_id => {
                        saw_main_cancel = true;
                        break;
                    }
                    Some(other) => panic!("in-writes Open leaked across fatal deadline: {other:?}"),
                    None => {}
                }
            }
        })
        .await
        .unwrap();
        assert!(saw_child_cancel && saw_main_cancel);
        updates
    });

    let err = running
        .handle_timeouts(Instant::now(), &outgoing)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("插件操作超时"), "unexpected fatal err: {err}");
    let _ = ce_rx.await;
    let _updates = helper.await.unwrap();

    io_cancel.cancel();
    io_tasks.abort_all();
}

#[tokio::test]
async fn writer_fatal_expiry_skip_forces_host_fatal_abort_even_if_reply_sampled_before_deadline() {
    let make_snap = |rev: u64, ed_rev: u64, contents: &[u8], label: &str| PanelSnapshot {
        revision: rev,
        title: label.into(),
        root: Some(panel::Node::new(
            1,
            "ed",
            panel::node::Kind::Editor(panel::Editor {
                document_id: 1,
                revision: ed_rev,
                contents: contents.to_vec(),
                format: panel::EditorFormat::Utf8 as i32,
                read_only: false,
            }),
        )),
    };

    // Part A: Writer observes fatal expiry of A and marks B as ExpiredSkipped; even if Host checks
    // A's local deadline cleanly before completion, `fatal_expired_triggered` forces fatal abort
    // without publishing A or leaving B orphaned without retry.
    {
        let (mut running, _calls, _updates) = running();
        let claims = Arc::new(OutboundWriteClaims::default());
        running.outbound_claims = Some(claims.clone());
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        let (om_tx, om_rx) = oneshot::channel();
        running
            .control_with_seq(
                1,
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
        let env_a = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(protocol::PANEL_ID, env_a.request_id));

        let (ob_tx, _ob_rx) = oneshot::channel();
        running
            .control_with_seq(
                2,
                Control::Open {
                    panel_id: protocol::PANEL_ID,
                    id: Uuid::new_v4(),
                    revision: 2,
                    read_only: false,
                    request: request(),
                    reply: ob_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let env_b = wire.try_recv().unwrap();

        running.requests.get_mut(&env_a.request_id).unwrap().deadline =
            Instant::now() - Duration::from_millis(5);
        running.sync_fatal_deadline_to_claims();

        // Writer pops B after A expired and skips it -> latches `fatal_expired_triggered`.
        assert!(!claims.claim_for_write(protocol::PANEL_ID, env_b.request_id));
        assert!(claims.is_fatal_expired_triggered());

        // Restore A's local deadline above `now` to emulate Host having sampled `now < A.deadline`
        // right before the deadline boundary when A's Snapshot frame arrived.
        running.requests.get_mut(&env_a.request_id).unwrap().deadline =
            Instant::now() + MAX_OPERATION;

        let mut snap_a = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, b"retained", "a-rev2")),
            MAX_OPERATION,
        );
        snap_a.in_reply_to = env_a.request_id;
        let err = running
            .frame(snap_a, &outgoing)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("插件操作超时"), "unexpected err: {err}");
        assert_eq!(running.main_panel().snapshot().revision, 1);
        assert!(om_rx.await.unwrap().is_err());

        let mut cancels = Vec::new();
        while let Ok(env) = wire.try_recv() {
            if let Some(envelope::Body::Cancel(c)) = env.body {
                cancels.push(c.request_id);
            }
        }
        assert!(cancels.contains(&env_a.request_id));
        assert!(
            !cancels.contains(&env_b.request_id),
            "ExpiredSkipped B must not trigger Cancel"
        );
    }

    // Part B: If Host settles A BEFORE A's deadline, earliest fatal deadline advances cleanly to B
    // and Writer STILL writes B even after A's original deadline later passes.
    {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let claims = Arc::new(OutboundWriteClaims::default());
        running.outbound_claims = Some(claims.clone());
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        let (om_tx, om_rx) = oneshot::channel();
        running
            .control_with_seq(
                1,
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
        let env_a = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(protocol::PANEL_ID, env_a.request_id));
        running.requests.get_mut(&env_a.request_id).unwrap().deadline =
            Instant::now() + Duration::from_millis(50);

        let (ob_tx, _ob_rx) = oneshot::channel();
        running
            .control_with_seq(
                2,
                Control::Open {
                    panel_id: 2,
                    id: Uuid::new_v4(),
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: ob_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let env_b = wire.try_recv().unwrap();
        running.sync_fatal_deadline_to_claims();

        let mut snap_a = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, b"retained", "a-ok")),
            MAX_OPERATION,
        );
        snap_a.in_reply_to = env_a.request_id;
        running.frame(snap_a, &outgoing).await.unwrap();
        assert!(om_rx.await.unwrap().is_ok());

        tokio::time::sleep(Duration::from_millis(65)).await;
        assert!(
            claims.claim_for_write(2, env_b.request_id),
            "settling A early must clear A's fatal deadline before Writer pops B"
        );
    }
}

#[tokio::test]
async fn unclaimed_child_destroy_close_in_writes_is_skipped_on_fatal_deadline_and_shutdown() {
    let make_snap = |rev: u64, ed_rev: u64, contents: &[u8], label: &str| PanelSnapshot {
        revision: rev,
        title: label.into(),
        root: Some(panel::Node::new(
            1,
            "ed",
            panel::node::Kind::Editor(panel::Editor {
                document_id: 1,
                revision: ed_rev,
                contents: contents.to_vec(),
                format: panel::EditorFormat::Utf8 as i32,
                read_only: false,
            }),
        )),
    };

    // Part A: Fatal deadline during event backpressure skips unclaimed child destroy CloseRequest in `writes`.
    {
        let (mut running, _calls, mut updates) = running();
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

        let res2 = Uuid::new_v4();
        let res3 = Uuid::new_v4();
        for (pid, res, seq) in [(2u64, res2, 1u64), (3u64, res3, 2u64)] {
            let (tx, rx) = oneshot::channel();
            running
                .control_with_seq(
                    seq,
                    Control::Open {
                        panel_id: pid,
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
            let env = timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
                .await
                .unwrap()
                .unwrap();
            let mut snap = Envelope::new(
                running.identity,
                seq,
                1,
                envelope::Body::Snapshot(make_snap(1, 1, b"child", "child")),
                MAX_OPERATION,
            )
            .with_panel_id(pid);
            snap.in_reply_to = env.request_id;
            running.frame(snap, &outgoing).await.unwrap();
            rx.await.unwrap().unwrap();
        }
        while updates.try_recv().is_ok() {}

        let (om_tx, _om_rx) = oneshot::channel();
        running
            .control_with_seq(
                3,
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
        let main_env = timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
            .await
            .unwrap()
            .unwrap();

        let (ce_tx, ce_rx) = oneshot::channel();
        running
            .control_with_seq(
                4,
                Control::Open {
                    panel_id: 2,
                    id: Uuid::new_v4(),
                    revision: 2,
                    read_only: false,
                    request: request(),
                    reply: ce_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let child_expired_env = timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
            .await
            .unwrap()
            .unwrap();
        running
            .requests
            .get_mut(&child_expired_env.request_id)
            .unwrap()
            .deadline = Instant::now() - Duration::from_secs(1);

        // Saturate pipe before dispatching destroy CloseRequest on panel 3 into `writes`.
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

        let (d3_tx, _d3_rx) = oneshot::channel();
        running
            .control_with_seq(
                5,
                Control::Revoke {
                    panel_id: 3,
                    id: res3,
                    remove_panel: true,
                    reply: d3_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert!(running.outbound_queue.is_empty());

        running
            .requests
            .get_mut(&main_env.request_id)
            .unwrap()
            .deadline = Instant::now() + Duration::from_millis(30);
        running.sync_fatal_deadline_to_claims();

        while running
            .events
            .try_send(PluginEvent::Stopped {
                identity: running.identity,
                reason: "fill".into(),
            })
            .is_ok()
        {}

        let main_req_id = main_env.request_id;
        let child_expired_id = child_expired_env.request_id;
        let helper = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(55)).await;
            for _ in 0..20 {
                let env = timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
                    .await
                    .unwrap()
                    .unwrap();
                assert!(matches!(env.body, Some(envelope::Body::HostResult(_))));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;

            while updates.try_recv().is_ok() {}
            let mut saw_child_cancel = false;
            let mut saw_main_cancel = false;
            timeout(MAX_OPERATION, async {
                while let Ok(env) = read_async_frame(&mut worker_rx).await {
                    match env.body {
                        Some(envelope::Body::Cancel(c)) if c.request_id == child_expired_id => {
                            saw_child_cancel = true;
                        }
                        Some(envelope::Body::Cancel(c)) if c.request_id == main_req_id => {
                            saw_main_cancel = true;
                            break;
                        }
                        Some(other) => panic!("destroy CloseRequest leaked past fatal deadline: {other:?}"),
                        None => {}
                    }
                }
            })
            .await
            .unwrap();
            assert!(saw_child_cancel && saw_main_cancel);
            updates
        });

        let err = running
            .handle_timeouts(Instant::now(), &outgoing)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("插件操作超时"), "unexpected fatal err: {err}");
        let _ = ce_rx.await;
        let _updates = helper.await.unwrap();

        io_cancel.cancel();
        io_tasks.abort_all();
    }

    // Part B: `prepare_shutdown` skips unclaimed child destroy CloseRequest in `writes`.
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

        let res2 = Uuid::new_v4();
        let (o2_tx, o2_rx) = oneshot::channel();
        running
            .control_with_seq(
                1,
                Control::Open {
                    panel_id: 2,
                    id: res2,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: o2_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let o2_env = timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
            .await
            .unwrap()
            .unwrap();
        let mut snap2 = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Snapshot(make_snap(1, 1, b"child", "child")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        snap2.in_reply_to = o2_env.request_id;
        running.frame(snap2, &outgoing).await.unwrap();
        o2_rx.await.unwrap().unwrap();

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

        let (d2_tx, _d2_rx) = oneshot::channel();
        running
            .control_with_seq(
                2,
                Control::Revoke {
                    panel_id: 2,
                    id: res2,
                    remove_panel: true,
                    reply: d2_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert!(running.outbound_queue.is_empty());

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
                    Some(other) => panic!("destroy CloseRequest leaked during prepare_shutdown: {other:?}"),
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

    // Part C: Non-fatal child destroy timeout on live worker preserves claim so Worker still destroys panel 2.
    {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let claims = Arc::new(OutboundWriteClaims::default());
        running.outbound_claims = Some(claims.clone());
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        let res2 = Uuid::new_v4();
        let (o2_tx, o2_rx) = oneshot::channel();
        running
            .control_with_seq(
                1,
                Control::Open {
                    panel_id: 2,
                    id: res2,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: o2_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let o2_env = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(2, o2_env.request_id));
        let mut snap2 = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Snapshot(make_snap(1, 1, b"child", "child")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        snap2.in_reply_to = o2_env.request_id;
        running.frame(snap2, &outgoing).await.unwrap();
        o2_rx.await.unwrap().unwrap();

        let (d2_tx, d2_rx) = oneshot::channel();
        running
            .control_with_seq(
                2,
                Control::Revoke {
                    panel_id: 2,
                    id: res2,
                    remove_panel: true,
                    reply: d2_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let destroy_env = wire.try_recv().unwrap();
        running
            .requests
            .get_mut(&destroy_env.request_id)
            .unwrap()
            .deadline = Instant::now() - Duration::from_secs(1);

        running
            .handle_timeouts(Instant::now(), &outgoing)
            .await
            .unwrap();
        assert!(d2_rx.await.unwrap().is_err());
        assert!(
            claims.claim_for_write(2, destroy_env.request_id),
            "live non-fatal child destroy CloseRequest must NOT be retracted on child timeout"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_writer_cannot_claim_destroy_close_during_prepare_shutdown_or_stop_cancel() {
    use std::io::Read;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use tokio::io::{AsyncReadExt, DuplexStream};

    let make_snap = |rev: u64, title: &str| PanelSnapshot {
        revision: rev,
        title: title.into(),
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

    // Part A: Race concurrent writer draining duplex pipe against both `prepare_shutdown` and `stop_cancel`.
    for iter in 0..8usize {
        let cancel_before_shutdown = iter % 2 == 1;
        let (mut running, _calls, _updates) = running();
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

        let res2 = Uuid::new_v4();
        let (o2_tx, o2_rx) = oneshot::channel();
        running
            .control_with_seq(
                1,
                Control::Open {
                    panel_id: 2,
                    id: res2,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: o2_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let o2_env = timeout(MAX_OPERATION, read_async_frame(&mut worker_rx))
            .await
            .unwrap()
            .unwrap();
        let mut snap2 = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Snapshot(make_snap(1, "child")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        snap2.in_reply_to = o2_env.request_id;
        running.frame(snap2, &outgoing).await.unwrap();
        o2_rx.await.unwrap().unwrap();

        // Saturate the 256-byte duplex buffer so writer blocks inside write_all on HostResult.
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

        // Queue BOTH main Open AND child destroy CloseRequest into `writes` while writer is blocked.
        let (o1_tx, _o1_rx) = oneshot::channel();
        running
            .control_with_seq(
                2,
                Control::Open {
                    panel_id: protocol::PANEL_ID,
                    id: Uuid::new_v4(),
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: o1_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let (d2_tx, _d2_rx) = oneshot::channel();
        running
            .control_with_seq(
                3,
                Control::Revoke {
                    panel_id: 2,
                    id: res2,
                    remove_panel: true,
                    reply: d2_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert!(running.outbound_queue.is_empty());

        if cancel_before_shutdown {
            // External stop_cancel happens BEFORE reader wakes writer and BEFORE prepare_shutdown runs!
            running.cancel.cancel();
            let drained_before_shutdown = Arc::new(tokio::sync::Notify::new());
            let drained_w = drained_before_shutdown.clone();
            let reader_task = tokio::spawn(async move {
                let mut host_results = 0usize;
                loop {
                    let env = read_async_frame(&mut worker_rx).await.unwrap();
                    match env.body {
                        Some(envelope::Body::HostResult(_)) => {
                            host_results += 1;
                            if host_results == 24 {
                                drained_w.notify_one();
                            }
                        }
                        Some(envelope::Body::Shutdown(_)) => return,
                        Some(other) => {
                            panic!("iter {iter}: unclaimed frame leaked after stop_cancel: {other:?}")
                        }
                        None => {}
                    }
                }
            });
            timeout(MAX_OPERATION, drained_before_shutdown.notified())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(10)).await;
            running.prepare_shutdown(&outgoing);
            timeout(MAX_OPERATION, reader_task).await.unwrap().unwrap();
        } else {
            // Concurrent reader starts draining in parallel while prepare_shutdown runs!
            let reader_task = tokio::spawn(async move {
                loop {
                    let env = read_async_frame(&mut worker_rx).await.unwrap();
                    match env.body {
                        Some(envelope::Body::HostResult(_)) => {
                            tokio::task::yield_now().await;
                        }
                        Some(envelope::Body::Shutdown(_)) => return,
                        Some(other) => {
                            panic!("iter {iter}: unclaimed frame leaked during prepare_shutdown: {other:?}")
                        }
                        None => {}
                    }
                }
            });
            running.prepare_shutdown(&outgoing);
            timeout(MAX_OPERATION, reader_task).await.unwrap().unwrap();
        }

        io_cancel.cancel();
        io_tasks.abort_all();
    }

    // Part B: Real duplex PluginSupervisor::stop() never delivers queued destroy CloseRequest to SDK on_panel_destroy.
    {
        struct GatedReader {
            stream: DuplexStream,
            rt: tokio::runtime::Handle,
            paused: Arc<AtomicBool>,
            resume: Arc<tokio::sync::Notify>,
        }
        impl Read for GatedReader {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                while self.paused.load(Ordering::SeqCst) {
                    self.rt.block_on(self.resume.notified());
                }
                self.rt.block_on(self.stream.read(buf))
            }
        }

        struct DestroyTrackPlugin {
            destroy_calls: Arc<AtomicUsize>,
        }

        impl mio_plugin_sdk::MioPlugin for DestroyTrackPlugin {
            fn id(&self) -> &str {
                "lab.destroy-shutdown-race"
            }

            fn version(&self) -> &str {
                "1.5.0"
            }

            fn supports_panel_multiplexing(&self) -> bool {
                true
            }

            fn initial_panel(&self) -> PanelSnapshot {
                PanelSnapshot {
                    revision: 1,
                    title: "main".into(),
                    root: Some(panel::Node::new(
                        1,
                        "txt",
                        panel::node::Kind::Text(panel::Text {
                            text: "main".into(),
                            tone: 0,
                        }),
                    )),
                }
            }

            fn on_panel_open_request(
                &mut self,
                _ctx: &mut dyn mio_plugin_sdk::HostClient,
                _panel_id: u64,
                _req: mio_plugin_api::protocol::OpenRequest,
            ) -> std::result::Result<Option<PanelSnapshot>, String> {
                Ok(Some(PanelSnapshot {
                    revision: 1,
                    title: "child".into(),
                    root: Some(panel::Node::new(
                        1,
                        "txt",
                        panel::node::Kind::Text(panel::Text {
                            text: "child".into(),
                            tone: 0,
                        }),
                    )),
                }))
            }

            fn on_panel_destroy(
                &mut self,
                _ctx: &mut dyn mio_plugin_sdk::HostClient,
                _panel_id: u64,
                _close: mio_plugin_api::protocol::CloseRequest,
            ) -> std::result::Result<Option<PanelSnapshot>, String> {
                self.destroy_calls.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            }
        }

        let rt_handle = tokio::runtime::Handle::current();
        let identity = crate::plugins::tests_runtime::identity();
        let manifest = PluginManifest {
            id: "lab.destroy-shutdown-race".into(),
            version: "1.5.0".into(),
            api_version: 1,
            engine: "native".into(),
            entry: "worker".into(),
            permissions: vec![Permission::TrafficRead],
            contributions: vec!["suite_panel".into()],
        };
        let (host_tx, worker_rx) = tokio::io::duplex(512);
        let (worker_tx, host_rx) = tokio::io::duplex(64 * 1024);
        let paused = Arc::new(AtomicBool::new(false));
        let resume = Arc::new(tokio::sync::Notify::new());
        let destroy_calls = Arc::new(AtomicUsize::new(0));

        let paused_w = paused.clone();
        let resume_w = resume.clone();
        let destroy_calls_w = destroy_calls.clone();
        let worker_thread = std::thread::spawn(move || {
            let mut plugin = DestroyTrackPlugin {
                destroy_calls: destroy_calls_w,
            };
            match mio_plugin_sdk::run_with_io(
                GatedReader {
                    stream: worker_rx,
                    rt: rt_handle.clone(),
                    paused: paused_w,
                    resume: resume_w,
                },
                BlockingWriter(worker_tx, rt_handle),
                &mut plugin,
            ) {
                Ok(()) => {}
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof
                    ) => {}
                Err(err) => panic!("unexpected duplex worker error during concurrent stop: {err:?}"),
            }
        });

        let (mut supervisor, _services) = crate::plugins::supervisor::spawn_test_duplex_supervisor(
            identity,
            manifest,
            BTreeSet::from([Permission::TrafficRead]),
            host_tx,
            host_rx,
        )
        .await
        .unwrap();
        let handle = supervisor.handle();

        let _init = timeout(MAX_OPERATION, supervisor.events.recv())
            .await
            .unwrap()
            .unwrap();
        let res2 = Uuid::new_v4();
        handle
            .open_request_on_panel(2, res2, 1, false, request())
            .await
            .unwrap();

        // Pause worker stdin reader so subsequent frames fill the 512-byte pipe and leave Open + CloseRequest in `writes`.
        paused.store(true, Ordering::SeqCst);
        for _ in 0..24 {
            handle
                .try_event(UiEvent {
                    node_id: 1,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::Focus(true)),
                })
                .unwrap();
        }
        tokio::time::sleep(Duration::from_millis(25)).await;

        let main_handle = handle.clone();
        let open_task = tokio::spawn(async move {
            let _ = main_handle
                .open_request(Uuid::new_v4(), 1, false, request())
                .await;
        });
        tokio::time::sleep(Duration::from_millis(15)).await;
        let revoke_handle = handle.begin_revoke_panel_resource(2, res2, true);
        tokio::time::sleep(Duration::from_millis(15)).await;

        let resume_task = tokio::spawn(async move {
            tokio::task::yield_now().await;
            paused.store(false, Ordering::SeqCst);
            resume.notify_one();
        });
        let (stop_res, _, _) = tokio::join!(supervisor.stop(), resume_task, open_task);
        stop_res.unwrap();
        let _ = revoke_handle.wait().await;
        worker_thread.join().unwrap();

        assert_eq!(
            destroy_calls.load(Ordering::SeqCst),
            0,
            "on_panel_destroy must never fire when shutdown races queued Open + destroy CloseRequest"
        );
    }
}
