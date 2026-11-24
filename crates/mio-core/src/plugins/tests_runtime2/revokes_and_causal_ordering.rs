use super::*;

#[tokio::test]
async fn real_duplex_supervisor_prioritizes_queued_revoke_over_deferred_open_on_capacity_recovery() {
    let identity = crate::plugins::tests_runtime::identity();
    let manifest = PluginManifest {
        id: "lab.revoke-priority-recovery".into(),
        version: "1.5.0".into(),
        api_version: 1,
        engine: "native".into(),
        entry: "worker".into(),
        permissions: vec![Permission::TrafficRead],
        contributions: vec!["suite_panel".into()],
    };
    let (host_to_worker, mut worker_rx) = tokio::io::duplex(512);
    let (mut worker_tx, worker_to_host) = tokio::io::duplex(64 * 1024);

    let pause_reading = Arc::new(tokio::sync::Notify::new());
    let send_ack_and_fill_events = Arc::new(tokio::sync::Notify::new());
    let resume_reading = Arc::new(tokio::sync::Notify::new());
    let results_drained = Arc::new(tokio::sync::Notify::new());
    let pause_reading_w = pause_reading.clone();
    let send_ack_w = send_ack_and_fill_events.clone();
    let resume_reading_w = resume_reading.clone();
    let results_drained_w = results_drained.clone();

    let send_frame = move |_tx: &mut tokio::io::DuplexStream,
                      seq: &mut u64,
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

    let host_call_count = OUTGOING_CAPACITY - RESERVED_CONTROL_OUTGOING + 8;
    let worker_task = tokio::spawn(async move {
        let hello = read_async_frame(&mut worker_rx).await?;
        let mut out_seq = 0u64;
        let reg = send_frame(
            &mut worker_tx,
            &mut out_seq,
            protocol::PANEL_ID,
            1,
            hello.request_id,
            envelope::Body::Register(protocol::Register {
                plugin_id: "lab.revoke-priority-recovery".into(),
                plugin_version: "1.5.0".into(),
                required_components: vec![],
                panel: Some(make_snap(1, "main")),
                supports_panel_multiplexing: true,
                supports_large_body: false,
            }),
        );
        worker_tx.write_all(&protocol::encode_frame(&reg)?).await?;
        worker_tx.flush().await?;
        let _ready = read_async_frame(&mut worker_rx).await?;

        let open2 = read_async_frame(&mut worker_rx).await?;
        let s1 = send_frame(
            &mut worker_tx,
            &mut out_seq,
            2,
            1,
            open2.request_id,
            envelope::Body::Snapshot(make_snap(1, "c2-v1")),
        );
        worker_tx.write_all(&protocol::encode_frame(&s1)?).await?;
        let s2 = send_frame(
            &mut worker_tx,
            &mut out_seq,
            2,
            2,
            0,
            envelope::Body::Snapshot(make_snap(2, "c2-v2")),
        );
        worker_tx.write_all(&protocol::encode_frame(&s2)?).await?;
        let stale = send_frame(
            &mut worker_tx,
            &mut out_seq,
            2,
            1,
            0,
            envelope::Body::Snapshot(make_snap(1, "c2-stale")),
        );
        worker_tx.write_all(&protocol::encode_frame(&stale)?).await?;
        worker_tx.flush().await?;

        let resync = read_async_frame(&mut worker_rx).await?;
        ensure!(matches!(resync.body, Some(envelope::Body::Resync(_))));

        // Pause reading worker_rx while test queues `open_extra` behind Resync(2).
        pause_reading_w.notify_one();
        send_ack_w.notified().await;

        // 1) Saturate host outgoing with invalid-parent HostCalls (HostResult replies fill pipe & outgoing),
        // 2) Settle Resync(2) via Ack(2) so `open_extra` stays in deferred_commands blocked on headroom,
        // 3) Saturate host->supervisor `events` channel so Running blocks inside `self.events.send`!
        for _ in 0..host_call_count {
            let hc = send_frame(
                &mut worker_tx,
                &mut out_seq,
                2,
                2,
                0,
                envelope::Body::HostCall(HostCall {
                    operation: Some(host_call::Operation::QueryHistory(protocol::HistoryQuery {
                        offset: 0,
                        limit: 1,
                        filter: String::new(),
                    })),
                    parent_request_id: 999_999,
                }),
            );
            worker_tx.write_all(&protocol::encode_frame(&hc)?).await?;
        }
        let ack = send_frame(
            &mut worker_tx,
            &mut out_seq,
            2,
            2,
            resync.request_id,
            envelope::Body::Ack(panel::Empty {}),
        );
        worker_tx.write_all(&protocol::encode_frame(&ack)?).await?;
        for _ in 0..(QUEUE_CAPACITY + 2) {
            let err_ev = send_frame(
                &mut worker_tx,
                &mut out_seq,
                2,
                2,
                0,
                envelope::Body::Error(Failure::new(ErrorCode::Internal, "fill-events")),
            );
            worker_tx.write_all(&protocol::encode_frame(&err_ev)?).await?;
        }
        worker_tx.flush().await?;

        // Resume reading ONLY until all HostResults are drained -> `outgoing` capacity fully recovers!
        resume_reading_w.notified().await;
        let mut seen_results = 0usize;
        let mut saw_open_extra = false;
        let close_req = loop {
            let env = read_async_frame(&mut worker_rx).await?;
            match env.body {
                Some(envelope::Body::HostResult(_)) => {
                    seen_results += 1;
                    if seen_results == host_call_count {
                        results_drained_w.notify_one();
                    }
                }
                Some(envelope::Body::OpenRequest(_)) => {
                    saw_open_extra = true;
                }
                Some(envelope::Body::CloseRequest(ref c)) => {
                    ensure!(!c.destroy_panel);
                    break env;
                }
                _ => {}
            }
        };
        ensure!(
            !saw_open_extra,
            "deferred Open(extra) must NOT overtake queued Revoke when capacity recovers"
        );
        let close_ack = send_frame(
            &mut worker_tx,
            &mut out_seq,
            2,
            2,
            close_req.request_id,
            envelope::Body::Ack(panel::Empty {}),
        );
        worker_tx.write_all(&protocol::encode_frame(&close_ack)?).await?;
        worker_tx.flush().await?;

        loop {
            match read_async_frame(&mut worker_rx).await {
                Ok(env) if matches!(env.body, Some(envelope::Body::Shutdown(_))) => break,
                Ok(env) => {
                    ensure!(!matches!(env.body, Some(envelope::Body::OpenRequest(_))));
                }
                Err(_) => break,
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

    let primary = Uuid::new_v4();
    let extra = Uuid::new_v4();
    let _ = timeout(MAX_OPERATION, supervisor.events.recv()).await.unwrap().unwrap();
    handle
        .open_request_on_panel(2, primary, 1, false, request())
        .await
        .unwrap();
    while supervisor.events.try_recv().is_ok() {}

    // Queue `open_extra` behind Resync(2), then pause Running on full `supervisor.events`.
    timeout(MAX_OPERATION, pause_reading.notified()).await.unwrap();
    let open_handle = handle.clone();
    let mut open_extra_task = tokio::spawn(async move {
        open_handle
            .open_request_on_panel(2, extra, 1, false, request())
            .await
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    send_ack_and_fill_events.notify_one();
    tokio::time::sleep(Duration::from_millis(80)).await;

    // Drain outgoing completely so outbound capacity + write_progress_rx are ready WHILE Running
    // is still blocked in `events.send`, then enqueue `Control::Revoke(extra, false)` into `commands`!
    resume_reading.notify_one();
    timeout(MAX_OPERATION, results_drained.notified()).await.unwrap();
    let revoke_handle = handle.clone();
    let mut revoke_task = tokio::spawn(async move {
        revoke_handle.revoke_panel_resource(2, extra, false).await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Unblock `Running::run` by draining `supervisor.events`: BOTH outbound capacity recovery AND
    // queued `Control::Revoke` are simultaneously ready -> Revoke MUST win cleanly!
    let mut revoke_res = None;
    let mut open_res = None;
    timeout(MAX_OPERATION, async {
        while revoke_res.is_none() || open_res.is_none() {
            tokio::select! {
                res = &mut revoke_task, if revoke_res.is_none() => {
                    revoke_res = Some(res.unwrap());
                }
                res = &mut open_extra_task, if open_res.is_none() => {
                    open_res = Some(res.unwrap());
                }
                _ = supervisor.events.recv() => {}
            }
        }
    })
    .await
    .unwrap();
    while supervisor.events.try_recv().is_ok() {}
    revoke_res.unwrap().unwrap();
    assert!(open_res.unwrap().is_err());

    supervisor.stop().await.unwrap();
    timeout(MAX_OPERATION, worker_task).await.unwrap().unwrap().unwrap();
}

#[tokio::test]
async fn real_duplex_supervisor_full_control_queue_revoke_cancels_queued_and_deferred_opens() {
    let identity = crate::plugins::tests_runtime::identity();
    let manifest = PluginManifest {
        id: "lab.full-ctrl-revoke".into(),
        version: "1.5.0".into(),
        api_version: 1,
        engine: "native".into(),
        entry: "worker".into(),
        permissions: vec![Permission::TrafficRead],
        contributions: vec!["suite_panel".into()],
    };
    let (host_to_worker, mut worker_rx) = tokio::io::duplex(512);
    let (mut worker_tx, worker_to_host) = tokio::io::duplex(64 * 1024);

    let pause_reading = Arc::new(tokio::sync::Notify::new());
    let send_ack_and_fill_events = Arc::new(tokio::sync::Notify::new());
    let resume_reading = Arc::new(tokio::sync::Notify::new());
    let results_drained = Arc::new(tokio::sync::Notify::new());
    let pause_reading_w = pause_reading.clone();
    let send_ack_w = send_ack_and_fill_events.clone();
    let resume_reading_w = resume_reading.clone();
    let results_drained_w = results_drained.clone();

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

    let host_call_count = OUTGOING_CAPACITY - RESERVED_CONTROL_OUTGOING + 8;
    let worker_task = tokio::spawn(async move {
        let hello = read_async_frame(&mut worker_rx).await?;
        let mut out_seq = 0u64;
        let reg = send_frame(
            &mut out_seq,
            protocol::PANEL_ID,
            1,
            hello.request_id,
            envelope::Body::Register(protocol::Register {
                plugin_id: "lab.full-ctrl-revoke".into(),
                plugin_version: "1.5.0".into(),
                required_components: vec![],
                panel: Some(make_snap(1, "main")),
                supports_panel_multiplexing: true,
                supports_large_body: false,
            }),
        );
        worker_tx.write_all(&protocol::encode_frame(&reg)?).await?;
        worker_tx.flush().await?;
        let _ready = read_async_frame(&mut worker_rx).await?;

        let open2 = read_async_frame(&mut worker_rx).await?;
        let s1 = send_frame(
            &mut out_seq,
            2,
            1,
            open2.request_id,
            envelope::Body::Snapshot(make_snap(1, "c2-v1")),
        );
        worker_tx.write_all(&protocol::encode_frame(&s1)?).await?;
        let s2 = send_frame(
            &mut out_seq,
            2,
            2,
            0,
            envelope::Body::Snapshot(make_snap(2, "c2-v2")),
        );
        worker_tx.write_all(&protocol::encode_frame(&s2)?).await?;
        let stale = send_frame(
            &mut out_seq,
            2,
            1,
            0,
            envelope::Body::Snapshot(make_snap(1, "c2-stale")),
        );
        worker_tx.write_all(&protocol::encode_frame(&stale)?).await?;
        worker_tx.flush().await?;

        let resync = read_async_frame(&mut worker_rx).await?;
        ensure!(matches!(resync.body, Some(envelope::Body::Resync(_))));

        pause_reading_w.notify_one();
        send_ack_w.notified().await;

        for _ in 0..host_call_count {
            let hc = send_frame(
                &mut out_seq,
                2,
                2,
                0,
                envelope::Body::HostCall(HostCall {
                    operation: Some(host_call::Operation::QueryHistory(protocol::HistoryQuery {
                        offset: 0,
                        limit: 1,
                        filter: String::new(),
                    })),
                    parent_request_id: 999_999,
                }),
            );
            worker_tx.write_all(&protocol::encode_frame(&hc)?).await?;
        }
        let ack = send_frame(
            &mut out_seq,
            2,
            2,
            resync.request_id,
            envelope::Body::Ack(panel::Empty {}),
        );
        worker_tx.write_all(&protocol::encode_frame(&ack)?).await?;
        for _ in 0..(QUEUE_CAPACITY + 2) {
            let err_ev = send_frame(
                &mut out_seq,
                2,
                2,
                0,
                envelope::Body::Error(Failure::new(ErrorCode::Internal, "fill-events")),
            );
            worker_tx.write_all(&protocol::encode_frame(&err_ev)?).await?;
        }
        worker_tx.flush().await?;

        resume_reading_w.notified().await;
        let mut seen_results = 0usize;
        let mut acked_main_events = 0usize;
        let mut close_count = 0usize;
        while let Ok(env) = read_async_frame(&mut worker_rx).await {
            match env.body {
                    Some(envelope::Body::Shutdown(_)) => break,
                    Some(envelope::Body::HostResult(_)) => {
                        seen_results += 1;
                        if seen_results == host_call_count {
                            results_drained_w.notify_one();
                        }
                    }
                    Some(envelope::Body::CloseRequest(ref c)) => {
                        ensure!(!c.destroy_panel);
                        close_count += 1;
                        let close_ack = send_frame(
                            &mut out_seq,
                            env.panel_id,
                            2,
                            env.request_id,
                            envelope::Body::Ack(panel::Empty {}),
                        );
                        worker_tx.write_all(&protocol::encode_frame(&close_ack)?).await?;
                        worker_tx.flush().await?;
                    }
                    Some(envelope::Body::OpenRequest(ref open)) => {
                        ensure!(
                            open.message_revision == 2 && close_count == 2,
                            "pre-revoke OpenRequest (rev={}) leaked past urgent Revoke (close_count={close_count})",
                            open.message_revision
                        );
                        let snap = send_frame(
                            &mut out_seq,
                            env.panel_id,
                            3,
                            env.request_id,
                            envelope::Body::Snapshot(make_snap(3, "c2-reopened-after-revoke")),
                        );
                        worker_tx.write_all(&protocol::encode_frame(&snap)?).await?;
                        worker_tx.flush().await?;
                    }
                    Some(envelope::Body::UiEvent(_)) if acked_main_events < 48 => {
                        acked_main_events += 1;
                        let ev_ack = send_frame(
                            &mut out_seq,
                            env.panel_id,
                            1,
                            env.request_id,
                            envelope::Body::Ack(panel::Empty {}),
                        );
                        worker_tx.write_all(&protocol::encode_frame(&ev_ack)?).await?;
                        worker_tx.flush().await?;
                    }
                    _ => {}
            }
        }
        ensure!(close_count == 2, "expected both CloseRequests, saw {close_count}");
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

    let primary = Uuid::new_v4();
    let extra_deferred = Uuid::new_v4();
    let extra_head = Uuid::new_v4();
    let _ = timeout(MAX_OPERATION, supervisor.events.recv()).await.unwrap().unwrap();
    handle
        .open_request_on_panel(2, primary, 1, false, request())
        .await
        .unwrap();

    // 1. Queue `open_deferred_task` behind Resync(2), then pause Running on full `supervisor.events`.
    timeout(MAX_OPERATION, pause_reading.notified()).await.unwrap();
    let open_deferred_handle = handle.clone();
    let mut open_deferred_task = tokio::spawn(async move {
        open_deferred_handle
            .open_request_on_panel(2, extra_deferred, 1, false, request())
            .await
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    while supervisor.events.try_recv().is_ok() {}
    send_ack_and_fill_events.notify_one();
    tokio::time::sleep(Duration::from_millis(80)).await;

    // 2. While Running is paused in `events.send`, fill ALL 128 slots of `handle.control`:
    //    head slot = `open_head_task`, trailing 127 slots = main-panel Focus events.
    let open_head_handle = handle.clone();
    let mut open_head_task = tokio::spawn(async move {
        open_head_handle
            .open_request_on_panel(2, extra_head, 1, false, request())
            .await
    });
    tokio::time::sleep(Duration::from_millis(15)).await;
    for i in 0..(QUEUE_CAPACITY - 1) {
        handle
            .try_event(panel::UiEvent {
                node_id: 1,
                editors: vec![],
                event: Some(panel::ui_event::Event::Focus(true)),
            })
            .unwrap_or_else(|e| panic!("failed filling control slot {i}: {e}"));
    }
    assert!(
        handle
            .try_event(panel::UiEvent {
                node_id: 1,
                editors: vec![],
                event: Some(panel::ui_event::Event::Focus(true)),
            })
            .is_err(),
        "control channel must be 128/128 full before sending urgent revokes"
    );

    // 3. Drain all HostResults so outbound capacity recovers, then enqueue BOTH urgent revokes
    //    via `urgent_control` (succeeding even though `control` is 128/128 full!).
    resume_reading.notify_one();
    timeout(MAX_OPERATION, results_drained.notified()).await.unwrap();
    let rev_def_handle = handle.clone();
    let mut revoke_deferred = tokio::spawn(async move {
        rev_def_handle.revoke_panel_resource(2, extra_deferred, false).await
    });
    let rev_head_handle = handle.clone();
    let mut revoke_head = tokio::spawn(async move {
        rev_head_handle.revoke_panel_resource(2, extra_head, false).await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;

    // 4. Resume Running by draining `supervisor.events`: both pre-revoke Opens MUST fail,
    //    both Revokes MUST complete Ok(()), and neither pre-revoke Open may ever hit worker_rx!
    let mut rev_def_res = None;
    let mut rev_head_res = None;
    let mut open_def_res = None;
    let mut open_head_res = None;
    timeout(MAX_OPERATION, async {
        while rev_def_res.is_none()
            || rev_head_res.is_none()
            || open_def_res.is_none()
            || open_head_res.is_none()
        {
            tokio::select! {
                res = &mut revoke_deferred, if rev_def_res.is_none() => rev_def_res = Some(res.unwrap()),
                res = &mut revoke_head, if rev_head_res.is_none() => rev_head_res = Some(res.unwrap()),
                res = &mut open_deferred_task, if open_def_res.is_none() => open_def_res = Some(res.unwrap()),
                res = &mut open_head_task, if open_head_res.is_none() => open_head_res = Some(res.unwrap()),
                _ = supervisor.events.recv() => {}
            }
        }
    })
    .await
    .unwrap();
    rev_def_res.unwrap().unwrap();
    rev_head_res.unwrap().unwrap();
    assert!(open_def_res.unwrap().is_err());
    assert!(open_head_res.unwrap().is_err());

    // 5. Causal ordering preserves explicit post-revoke reopen (`seq > revoke_head.seq`).
    let reopen_handle = handle.clone();
    let mut reopen_task = tokio::spawn(async move {
        reopen_handle
            .open_request_on_panel(2, extra_head, 2, false, request())
            .await
    });
    let reopened = timeout(MAX_OPERATION, async {
        loop {
            tokio::select! {
                res = &mut reopen_task => break res.unwrap(),
                _ = supervisor.events.recv() => {}
            }
        }
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(reopened.title, "c2-reopened-after-revoke");
    while supervisor.events.try_recv().is_ok() {}

    supervisor.stop().await.unwrap();
    timeout(MAX_OPERATION, worker_task).await.unwrap().unwrap().unwrap();
}

#[tokio::test]
async fn overtaken_control_event_uses_pre_close_snapshot_and_rejects_after_reorder() {
    let (mut running, _calls, mut updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

    let make_snap = |rev: u64, opts: [&str; 2]| PanelSnapshot {
        revision: rev,
        title: "overtaken-select".into(),
        root: Some(panel::Node::new(
            20,
            "action-select",
            panel::node::Kind::Select(panel::SelectNode {
                label: "Action".into(),
                options: opts.iter().map(|s| (*s).to_owned()).collect(),
                selected_index: 1,
                enabled: true,
            }),
        )),
    };

    let primary = Uuid::new_v4();
    let extra = Uuid::new_v4();
    let (open_tx, open_rx) = oneshot::channel();
    running
        .control_with_seq(
            1,
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
    let open_wire = wire.try_recv().unwrap();
    let mut open_snap = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(make_snap(1, ["Allow", "Deny"])),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    open_snap.in_reply_to = open_wire.request_id;
    running.frame(open_snap, &outgoing).await.unwrap();
    assert!(open_rx.await.is_ok());

    // Urgent Revoke at seq=20 overtakes an earlier Click/Select at seq=10,
    // and CloseRequest returns a Snapshot(2) reordering options to ["Deny", "Allow"].
    let (rev_tx, rev_rx) = oneshot::channel();
    running
        .control_with_seq(
            20,
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
    let mut close_snap = Envelope::new(
        running.identity,
        2,
        2,
        envelope::Body::Snapshot(make_snap(2, ["Deny", "Allow"])),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    close_snap.in_reply_to = close_wire.request_id;
    running.frame(close_snap, &outgoing).await.unwrap();
    assert!(rev_rx.await.unwrap().is_ok());
    while updates.try_recv().is_ok() {}

    // Overtaken SelectedOption(0) at seq=10 (< 20) MUST compare against pre-close rev=1 snapshot
    // and fail as Stale without emitting onto wire!
    running
        .control_with_seq(
            10,
            Control::Event {
                panel_id: 2,
                event: UiEvent {
                    node_id: 20,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::SelectedOption(0)),
                },
                expected_snapshot: None,
                event_seq: 0,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(wire.try_recv().is_err());
    let err = updates.try_recv().unwrap();
    assert!(matches!(
        err,
        PluginEvent::Error { panel_id: 2, ref failure, .. }
            if failure.code == ErrorCode::Stale as i32
                && failure.message.contains("插件面板内容已更新")
    ));

    // Post-revoke SelectedOption(0) at seq=25 (> 20) reflects the current snapshot and dispatches!
    running
        .control_with_seq(
            25,
            Control::Event {
                panel_id: 2,
                event: UiEvent {
                    node_id: 20,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::SelectedOption(0)),
                },
                expected_snapshot: None,
                event_seq: 0,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let sent = wire.try_recv().unwrap();
    assert_eq!(sent.panel_id, 2);
    assert_eq!(sent.revision, 2);
}

#[tokio::test]
async fn evicted_causal_seq_floors_reject_older_destroy_revoke_and_cancel_op() {
    let (mut running, _calls, mut updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

    let snap = PanelSnapshot {
        revision: 1,
        title: "floor-test".into(),
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
    let primary = Uuid::new_v4();
    let (open_tx, open_rx) = oneshot::channel();
    running
        .control_with_seq(
            1,
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
    let mut osnap = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(snap),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    osnap.in_reply_to = ow.request_id;
    running.frame(osnap, &outgoing).await.unwrap();
    assert!(open_rx.await.is_ok());
    while updates.try_recv().is_ok() {}

    // 1. Evict panel_destroy_control_seq past MAX_TRACKED_DESTROY_SEQS.
    let mut seq = 10u64;
    let old_destroy_seq = seq;
    let (rtx, _) = oneshot::channel();
    running
        .control_with_seq(
            old_destroy_seq,
            Control::Revoke {
                panel_id: 500,
                id: Uuid::new_v4(),
                remove_panel: true,
                reply: rtx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    for i in 0..MAX_TRACKED_DESTROY_SEQS {
        seq += 1;
        let (tx, _) = oneshot::channel();
        running
            .control_with_seq(
                seq,
                Control::Revoke {
                    panel_id: 600 + i as u64,
                    id: Uuid::new_v4(),
                    remove_panel: true,
                    reply: tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
    }
    assert!(running.panel_destroy_seq_floor >= old_destroy_seq);
    assert!(!running.panel_destroy_control_seq.contains_key(&500));
    let (old_open_tx, mut old_open_rx) = oneshot::channel();
    running
        .control_with_seq(
            old_destroy_seq - 1,
            Control::Open {
                panel_id: 500,
                id: Uuid::new_v4(),
                revision: 1,
                read_only: false,
                request: request(),
                reply: old_open_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(old_open_rx.try_recv().unwrap().is_err());
    assert!(wire.try_recv().is_err());

    // Main panel (`PANEL_ID`) is never falsely rejected by child-only destroy/cancel floors!
    let (main_open_tx, main_open_rx) = oneshot::channel();
    running
        .control_with_seq(
            old_destroy_seq - 1,
            Control::Open {
                panel_id: protocol::PANEL_ID,
                id: Uuid::new_v4(),
                revision: 1,
                read_only: false,
                request: request(),
                reply: main_open_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(wire.try_recv().is_ok());
    drop(main_open_rx);

    // 2. Evict resource_revoke_control_seq past MAX_TRACKED_REVOKE_SEQS.
    seq += 1;
    let old_rev_seq = seq;
    let old_rev_res = Uuid::new_v4();
    let (rtx2, _) = oneshot::channel();
    running
        .control_with_seq(
            old_rev_seq,
            Control::Revoke {
                panel_id: 2,
                id: old_rev_res,
                remove_panel: false,
                reply: rtx2,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let cw = wire.try_recv().unwrap();
    let mut cack = Envelope::new(
        running.identity,
        2,
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    cack.in_reply_to = cw.request_id;
    running.frame(cack, &outgoing).await.unwrap();
    for i in 0..MAX_TRACKED_REVOKE_SEQS {
        seq += 1;
        let (tx, _) = oneshot::channel();
        running
            .control_with_seq(
                seq,
                Control::Revoke {
                    panel_id: 2000 + i as u64,
                    id: Uuid::new_v4(),
                    remove_panel: false,
                    reply: tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
    }
    assert!(running.resource_revoke_seq_floor >= old_rev_seq);
    assert!(!running.resource_revoke_control_seq.contains_key(&(2, old_rev_res)));
    let (old_r_tx, mut old_r_rx) = oneshot::channel();
    running
        .control_with_seq(
            old_rev_seq - 1,
            Control::Open {
                panel_id: 2,
                id: old_rev_res,
                revision: 1,
                read_only: false,
                request: request(),
                reply: old_r_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(old_r_rx.try_recv().unwrap().is_err());
    assert!(wire.try_recv().is_err());

    // 3. Evict cancelled_op_control_seq past MAX_TRACKED_CANCELLED_OPS.
    seq += 1;
    let old_op_seq = seq;
    let old_op = Uuid::new_v4();
    running
        .control_with_seq(
            old_op_seq,
            Control::CancelPanelOp {
                panel_id: 2,
                op_id: old_op,
            },
            &outgoing,
        )
        .await
        .unwrap();
    for _ in 0..MAX_TRACKED_CANCELLED_OPS {
        seq += 1;
        running
            .control_with_seq(
                seq,
                Control::CancelPanelOp {
                    panel_id: 2,
                    op_id: Uuid::new_v4(),
                },
                &outgoing,
            )
            .await
            .unwrap();
    }
    assert!(running.cancelled_op_seq_floor >= old_op_seq);
    assert!(!running.cancelled_op_control_seq.contains_key(&(2, old_op)));
    let (commit_tx, mut commit_rx) = oneshot::channel();
    running
        .control_with_seq(
            old_op_seq - 1,
            Control::Commit {
                panel_id: 2,
                op_id: old_op,
                request: CommitEditor {
                    resource_id: primary.as_bytes().to_vec(),
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
                            expected_snapshot: None,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(commit_rx.try_recv().unwrap().is_err());
    assert!(wire.try_recv().is_err());
}

#[tokio::test]
async fn real_duplex_supervisor_evicted_revoke_seq_floor_still_blocks_old_queued_open() {
    let identity = crate::plugins::tests_runtime::identity();
    let manifest = PluginManifest {
        id: "lab.evict-floor".into(),
        version: "1.5.0".into(),
        api_version: 1,
        engine: "native".into(),
        entry: "worker".into(),
        permissions: vec![Permission::TrafficRead],
        contributions: vec!["suite_panel".into()],
    };
    let (host_to_worker, mut worker_rx) = tokio::io::duplex(64 * 1024);
    let (mut worker_tx, worker_to_host) = tokio::io::duplex(64 * 1024);

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

    let worker_task = tokio::spawn(async move {
        let hello = read_async_frame(&mut worker_rx).await?;
        let mut out_seq = 0u64;
        let reg = send_frame(
            &mut out_seq,
            protocol::PANEL_ID,
            1,
            hello.request_id,
            envelope::Body::Register(protocol::Register {
                plugin_id: "lab.evict-floor".into(),
                plugin_version: "1.5.0".into(),
                required_components: vec![],
                panel: Some(make_snap(1, "main")),
                supports_panel_multiplexing: true,
                supports_large_body: false,
            }),
        );
        worker_tx.write_all(&protocol::encode_frame(&reg)?).await?;
        worker_tx.flush().await?;
        let _ready = read_async_frame(&mut worker_rx).await?;

        let open1 = read_async_frame(&mut worker_rx).await?;
        let s1 = send_frame(
            &mut out_seq,
            2,
            1,
            open1.request_id,
            envelope::Body::Snapshot(make_snap(1, "c2-v1")),
        );
        worker_tx.write_all(&protocol::encode_frame(&s1)?).await?;
        // Fill supervisor.events (128 slots) exactly without blocking yet!
        for _ in 0..QUEUE_CAPACITY {
            let err_ev = send_frame(
                &mut out_seq,
                2,
                1,
                0,
                envelope::Body::Error(Failure::new(ErrorCode::Internal, "fill-events")),
            );
            worker_tx.write_all(&protocol::encode_frame(&err_ev)?).await?;
        }
        worker_tx.flush().await?;

        let mut close_count = 0usize;
        let mut reopen_ok = false;
        while let Ok(env) = read_async_frame(&mut worker_rx).await {
            match env.body {
                    Some(envelope::Body::Shutdown(_)) => break,
                    Some(envelope::Body::CloseRequest(ref c)) => {
                        ensure!(!c.destroy_panel);
                        close_count += 1;
                        let ack = send_frame(
                            &mut out_seq,
                            env.panel_id,
                            1,
                            env.request_id,
                            envelope::Body::Ack(panel::Empty {}),
                        );
                        worker_tx.write_all(&protocol::encode_frame(&ack)?).await?;
                        worker_tx.flush().await?;
                    }
                    Some(envelope::Body::OpenRequest(ref open)) => {
                        ensure!(
                            open.message_revision == 2,
                            "evicted old OpenRequest(rev={}) must never reach worker after floor eviction",
                            open.message_revision
                        );
                        reopen_ok = true;
                        let snap = send_frame(
                            &mut out_seq,
                            env.panel_id,
                            2,
                            env.request_id,
                            envelope::Body::Snapshot(make_snap(2, "c2-reopened-after-evict")),
                        );
                        worker_tx.write_all(&protocol::encode_frame(&snap)?).await?;
                        worker_tx.flush().await?;
                    }
                    _ => {}
            }
        }
        ensure!(close_count == 1 && reopen_ok);
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

    let primary = Uuid::new_v4();
    let extra_old = Uuid::new_v4();
    let _ = timeout(MAX_OPERATION, supervisor.events.recv()).await.unwrap().unwrap();
    handle
        .open_request_on_panel(2, primary, 1, false, request())
        .await
        .unwrap();

    // Wait until Running has pushed all 128 Errors into `supervisor.events` (channel 128/128 full!).
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Enqueue invalid UiEvent at control[0] -> Running immediately blocks in `self.events.send`!
    handle
        .try_event_on_panel(
            2,
            UiEvent {
                node_id: 9999,
                editors: vec![],
                event: Some(panel::ui_event::Event::Focus(true)),
            },
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Now while Running is parked on `events.send`, queue 5 invalid UiEvents + `open_old_task` in `control`,
    // then `revoke_old` in `urgent_control`. `open_old` has committed `seq < revoke_old.seq`.
    for _ in 0..5 {
        handle
            .try_event_on_panel(
                2,
                UiEvent {
                    node_id: 9999,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::Focus(true)),
                },
            )
            .unwrap();
    }
    let open_old_handle = handle.clone();
    let mut open_old_task = tokio::spawn(async move {
        open_old_handle
            .open_request_on_panel(2, extra_old, 1, false, request())
            .await
    });
    tokio::time::sleep(Duration::from_millis(15)).await;
    let rev_old_handle = handle.clone();
    let mut revoke_old_task = tokio::spawn(async move {
        rev_old_handle.revoke_panel_resource(2, extra_old, false).await
    });
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Step through the 5 parked invalid-event waves, issuing 120 unrelated urgent revokes per wave
    // (600 > MAX_TRACKED_REVOKE_SEQS = 512) so `revoke_old`'s map entry IS EVICTED BEFORE `open_old`
    // is ever reached on `control`!
    for wave in 0..5u64 {
        let mut wave_tasks = Vec::with_capacity(120);
        for j in 0..120u64 {
            let h = handle.clone();
            let panel_id = 1000 + wave * 120 + j;
            let res = Uuid::new_v4();
            wave_tasks.push(tokio::spawn(async move {
                h.revoke_panel_resource(panel_id, res, false).await
            }));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        // Release exactly 1 event slot: Running drains all urgent revokes first, then blocks on next invalid event!
        let _ = timeout(MAX_OPERATION, supervisor.events.recv()).await.unwrap().unwrap();
        for t in wave_tasks {
            timeout(MAX_OPERATION, t).await.unwrap().unwrap().unwrap();
        }
        if wave == 0 {
            timeout(MAX_OPERATION, &mut revoke_old_task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
    }

    // Release the final event slot and drain `supervisor.events`: `open_old` MUST still be rejected
    // by `resource_revoke_seq_floor >= open_old.seq` even though `revoke_old`'s key was evicted!
    let open_old_res = timeout(MAX_OPERATION, async {
        loop {
            tokio::select! {
                res = &mut open_old_task => break res.unwrap(),
                _ = supervisor.events.recv() => {}
            }
        }
    })
    .await
    .unwrap();
    assert!(
        open_old_res.is_err(),
        "old Open must fail against evicted floor even after 600 newer revokes"
    );

    // Explicit reopen AFTER eviction has `seq > floor` and succeeds cleanly!
    let reopen_handle = handle.clone();
    let mut reopen_task = tokio::spawn(async move {
        reopen_handle
            .open_request_on_panel(2, extra_old, 2, false, request())
            .await
    });
    let reopened = timeout(MAX_OPERATION, async {
        loop {
            tokio::select! {
                res = &mut reopen_task => break res.unwrap(),
                _ = supervisor.events.recv() => {}
            }
        }
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(reopened.title, "c2-reopened-after-evict");
    while supervisor.events.try_recv().is_ok() {}

    supervisor.stop().await.unwrap();
    timeout(MAX_OPERATION, worker_task).await.unwrap().unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_handles_and_late_older_revoke_preserve_newer_explicit_open() {
    // Part A: Exact unit proof protecting newer in-flight and bound opens against older late Revokes.
    {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

        let make_snap = |rev: u64| PanelSnapshot {
            revision: rev,
            title: format!("p2-r{rev}"),
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
        let primary = Uuid::new_v4();
        let extra = Uuid::new_v4();
        let (open1_tx, open1_rx) = oneshot::channel();
        running
            .control_with_seq(
                1,
                Control::Open {
                    panel_id: 2,
                    id: primary,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: open1_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let ow1 = wire.try_recv().unwrap();
        let mut s1 = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Snapshot(make_snap(1)),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        s1.in_reply_to = ow1.request_id;
        running.frame(s1, &outgoing).await.unwrap();
        assert!(open1_rx.await.is_ok());

        // Newer explicit Open arrives at seq=20 (in flight).
        let (open2_tx, open2_rx) = oneshot::channel();
        running
            .control_with_seq(
                20,
                Control::Open {
                    panel_id: 2,
                    id: extra,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: open2_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let ow2 = wire.try_recv().unwrap();

        // Late older non-destroy Revoke at seq=10 (< 20): must preserve in-flight Open(20) and emit nothing!
        let (rev_old_inflight_tx, rev_old_inflight_rx) = oneshot::channel();
        running
            .control_with_seq(
                10,
                Control::Revoke {
                    panel_id: 2,
                    id: extra,
                    remove_panel: false,
                    reply: rev_old_inflight_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert!(rev_old_inflight_rx.await.unwrap().is_ok());
        assert!(wire.try_recv().is_err());

        // Settle Open(20) cleanly -> binds `(2, extra)` with `open_seq = 20`.
        let mut s2 = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Snapshot(make_snap(2)),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        s2.in_reply_to = ow2.request_id;
        running.frame(s2, &outgoing).await.unwrap();
        assert!(open2_rx.await.is_ok());
        assert!(running.broker.has_panel_resource(2, extra));

        // Late older non-destroy Revoke at seq=15 (< 20): must preserve active `(2, extra)` binding!
        let (rev_old_bound_tx, rev_old_bound_rx) = oneshot::channel();
        running
            .control_with_seq(
                15,
                Control::Revoke {
                    panel_id: 2,
                    id: extra,
                    remove_panel: false,
                    reply: rev_old_bound_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert!(rev_old_bound_rx.await.unwrap().is_ok());
        assert!(wire.try_recv().is_err());
        assert!(running.broker.has_panel_resource(2, extra));
        assert_eq!(running.panels[&2].active_resource, Some((extra, 1)));

        // Late older destroying Revoke at seq=18 (< 20): must preserve panel 2 because `open_seq = 20 > 18`!
        let (rev_old_destroy_tx, rev_old_destroy_rx) = oneshot::channel();
        running
            .control_with_seq(
                18,
                Control::Revoke {
                    panel_id: 2,
                    id: primary,
                    remove_panel: true,
                    reply: rev_old_destroy_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert!(rev_old_destroy_rx.await.unwrap().is_ok());
        assert!(wire.try_recv().is_err());
        assert!(!running.panels[&2].closing);
        assert!(running.broker.has_panel_resource(2, extra));

        // Explicitly newer Revoke at seq=25 (> 20) DOES revoke `(2, extra)` and emits CloseRequest!
        let (rev_new_tx, rev_new_rx) = oneshot::channel();
        running
            .control_with_seq(
                25,
                Control::Revoke {
                    panel_id: 2,
                    id: extra,
                    remove_panel: false,
                    reply: rev_new_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert!(!running.broker.has_panel_resource(2, extra));
        let close_w = wire.try_recv().unwrap();
        assert!(matches!(close_w.body, Some(envelope::Body::CloseRequest(_))));
        let mut cack = Envelope::new(
            running.identity,
            3,
            2,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        cack.in_reply_to = close_w.request_id;
        running.frame(cack, &outgoing).await.unwrap();
        assert!(rev_new_rx.await.unwrap().is_ok());
    }

    // Part B: Two cloned `PluginHandle`s racing `open_request_on_panel` and `revoke_panel_resource`
    // concurrently over a real duplex supervisor across multiple rounds.
    let identity = crate::plugins::tests_runtime::identity();
    let manifest = PluginManifest {
        id: "lab.dual-handle-race".into(),
        version: "1.5.0".into(),
        api_version: 1,
        engine: "native".into(),
        entry: "worker".into(),
        permissions: vec![Permission::TrafficRead],
        contributions: vec!["suite_panel".into()],
    };
    let (host_to_worker, mut worker_rx) = tokio::io::duplex(64 * 1024);
    let (mut worker_tx, worker_to_host) = tokio::io::duplex(64 * 1024);
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
    let make_snap = |rev: u64| PanelSnapshot {
        revision: rev,
        title: format!("race-r{rev}"),
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

    let worker_task = tokio::spawn(async move {
        let hello = read_async_frame(&mut worker_rx).await?;
        let mut out_seq = 0u64;
        let mut rev = 1u64;
        let reg = send_frame(
            &mut out_seq,
            protocol::PANEL_ID,
            rev,
            hello.request_id,
            envelope::Body::Register(protocol::Register {
                plugin_id: "lab.dual-handle-race".into(),
                plugin_version: "1.5.0".into(),
                required_components: vec![],
                panel: Some(make_snap(rev)),
                supports_panel_multiplexing: true,
                supports_large_body: false,
            }),
        );
        worker_tx.write_all(&protocol::encode_frame(&reg)?).await?;
        worker_tx.flush().await?;
        let _ready = read_async_frame(&mut worker_rx).await?;

        let mut p2_rev = 0u64;
        while let Ok(env) = read_async_frame(&mut worker_rx).await {
            match env.body {
                    Some(envelope::Body::Shutdown(_)) => break,
                    Some(envelope::Body::OpenRequest(_)) => {
                        p2_rev += 1;
                        rev = p2_rev;
                        let snap = send_frame(
                            &mut out_seq,
                            env.panel_id,
                            rev,
                            env.request_id,
                            envelope::Body::Snapshot(make_snap(rev)),
                        );
                        worker_tx.write_all(&protocol::encode_frame(&snap)?).await?;
                        worker_tx.flush().await?;
                    }
                    Some(envelope::Body::CloseRequest(_)) => {
                        let ack = send_frame(
                            &mut out_seq,
                            env.panel_id,
                            p2_rev.max(1),
                            env.request_id,
                            envelope::Body::Ack(panel::Empty {}),
                        );
                        worker_tx.write_all(&protocol::encode_frame(&ack)?).await?;
                        worker_tx.flush().await?;
                    }
                    Some(envelope::Body::Resync(ref resync)) => {
                        p2_rev = resync.revision;
                        let ack = send_frame(
                            &mut out_seq,
                            env.panel_id,
                            p2_rev,
                            env.request_id,
                            envelope::Body::Ack(panel::Empty {}),
                        );
                        worker_tx.write_all(&protocol::encode_frame(&ack)?).await?;
                        worker_tx.flush().await?;
                    }
                    _ => {}
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
    let h1 = supervisor.handle();
    let h2 = h1.clone();
    let _ = timeout(MAX_OPERATION, supervisor.events.recv()).await.unwrap().unwrap();

    for round in 1..=12u64 {
        let res = Uuid::new_v4();
        let a = h1.clone();
        let b = h2.clone();
        let open_fut = tokio::spawn(async move {
            a.open_request_on_panel(2, res, round, false, request()).await
        });
        let revoke_fut = tokio::spawn(async move {
            b.revoke_panel_resource(2, res, false).await
        });
        let (open_res, rev_res) = timeout(MAX_OPERATION, async {
            tokio::join!(open_fut, revoke_fut)
        })
        .await
        .unwrap();
        rev_res.unwrap().unwrap();
        if open_res.unwrap().is_ok() {
            // If `Open` won the causal order after `Revoke`, an explicit subsequent `Revoke` cleans up!
            timeout(MAX_OPERATION, h1.revoke_panel_resource(2, res, false))
                .await
                .unwrap()
                .unwrap();
        }
        while supervisor.events.try_recv().is_ok() {}
    }

    supervisor.stop().await.unwrap();
    timeout(MAX_OPERATION, worker_task).await.unwrap().unwrap().unwrap();
}

#[tokio::test]
async fn nonexistent_and_destroyed_panel_revokes_never_leak_snapshot_history() {
    let (mut running, _calls, _updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

    // 1. Non-destroy revokes on non-existent child panels create NO snapshot history or floor entries.
    for i in 0..100u64 {
        let (tx, rx) = oneshot::channel();
        running
            .control_with_seq(
                i + 1,
                Control::Revoke {
                    panel_id: 100 + i,
                    id: Uuid::new_v4(),
                    remove_panel: false,
                    reply: tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert!(rx.await.unwrap().is_ok());
    }
    assert!(running.panel_revoke_snapshots.is_empty());
    assert!(running.panel_revoke_snapshot_floor.is_empty());

    // 2. Open live child panel 2 and exceed MAX_PANEL_REVOKE_SNAPSHOTS -> bounded to 16 + floor updated.
    let make_snap = |rev: u64| PanelSnapshot {
        revision: rev,
        title: format!("p-{rev}"),
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
    for &(panel_id, seq) in &[(2u64, 101u64), (3u64, 201u64)] {
        let res = Uuid::new_v4();
        let (otx, orx) = oneshot::channel();
        running
            .control_with_seq(
                seq,
                Control::Open {
                    panel_id,
                    id: res,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: otx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let ow = wire.try_recv().unwrap();
        let mut s = Envelope::new(
            running.identity,
            panel_id,
            1,
            envelope::Body::Snapshot(make_snap(1)),
            MAX_OPERATION,
        )
        .with_panel_id(panel_id);
        s.in_reply_to = ow.request_id;
        running.frame(s, &outgoing).await.unwrap();
        assert!(orx.await.is_ok());
    }
    let mut seq = 300u64;
    for panel_id in [2u64, 3u64] {
        for _ in 0..20 {
            seq += 1;
            let (tx, rx) = oneshot::channel();
            running
                .control_with_seq(
                    seq,
                    Control::Revoke {
                        panel_id,
                        id: Uuid::new_v4(),
                        remove_panel: false,
                        reply: tx,
                    },
                    &outgoing,
                )
                .await
                .unwrap();
            let cw = wire.try_recv().unwrap();
            seq += 1;
            let mut cack = Envelope::new(
                running.identity,
                seq,
                1,
                envelope::Body::Ack(panel::Empty {}),
                MAX_OPERATION,
            )
            .with_panel_id(panel_id);
            cack.in_reply_to = cw.request_id;
            running.frame(cack, &outgoing).await.unwrap();
            assert!(rx.await.unwrap().is_ok());
        }
        assert_eq!(
            running.panel_revoke_snapshots[&panel_id].len(),
            MAX_PANEL_REVOKE_SNAPSHOTS
        );
        assert!(running.panel_revoke_snapshot_floor.contains_key(&panel_id));
    }

    // 3. Destroying panel 2 and marking panel 3 destroy_failed clean both maps for those panels!
    let (d2_tx, d2_rx) = oneshot::channel();
    running
        .control_with_seq(
            900,
            Control::Revoke {
                panel_id: 2,
                id: Uuid::new_v4(),
                remove_panel: true,
                reply: d2_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let d2_w = wire.try_recv().unwrap();
    let mut d2_ack = Envelope::new(
        running.identity,
        900,
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    d2_ack.in_reply_to = d2_w.request_id;
    running.frame(d2_ack, &outgoing).await.unwrap();
    assert!(d2_rx.await.unwrap().is_ok());
    assert!(!running.panel_revoke_snapshots.contains_key(&2));
    assert!(!running.panel_revoke_snapshot_floor.contains_key(&2));

    running.mark_panel_destroy_failed(3, None);
    assert!(running.panel_revoke_snapshots.is_empty());
    assert!(running.panel_revoke_snapshot_floor.is_empty());

    // 4. Late non-destroy revokes on closing/destroy-failed panel 3 & unknown panels 200..250 do NOT leak entries!
    for i in 0..50u64 {
        let (tx3, _rx3) = oneshot::channel();
        running
            .control_with_seq(
                1000 + i * 2,
                Control::Revoke {
                    panel_id: 3,
                    id: Uuid::new_v4(),
                    remove_panel: false,
                    reply: tx3,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let (txu, _rxu) = oneshot::channel();
        running
            .control_with_seq(
                1001 + i * 2,
                Control::Revoke {
                    panel_id: 200 + i,
                    id: Uuid::new_v4(),
                    remove_panel: false,
                    reply: txu,
                },
                &outgoing,
            )
            .await
            .unwrap();
    }
    assert!(running.panel_revoke_snapshots.is_empty());
    assert!(running.panel_revoke_snapshot_floor.is_empty());
}

#[tokio::test]
async fn real_duplex_supervisor_revoke_cancels_unpermitted_open_waiting_on_full_control_channel() {
    let identity = crate::plugins::tests_runtime::identity();
    let manifest = PluginManifest {
        id: "lab.prepermit-revoke".into(),
        version: "1.5.0".into(),
        api_version: 1,
        engine: "native".into(),
        entry: "worker".into(),
        permissions: vec![Permission::TrafficRead, Permission::DraftWrite],
        contributions: vec!["suite_panel".into()],
    };
    let (host_to_worker, mut worker_rx) = tokio::io::duplex(64 * 1024);
    let (mut worker_tx, worker_to_host) = tokio::io::duplex(64 * 1024);
    let send_errors = Arc::new(tokio::sync::Notify::new());
    let send_errors_w = send_errors.clone();

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

    let worker_task = tokio::spawn(async move {
        let hello = read_async_frame(&mut worker_rx).await?;
        let mut out_seq = 0u64;
        let reg = send_frame(
            &mut out_seq,
            protocol::PANEL_ID,
            1,
            hello.request_id,
            envelope::Body::Register(protocol::Register {
                plugin_id: "lab.prepermit-revoke".into(),
                plugin_version: "1.5.0".into(),
                required_components: vec![],
                panel: Some(make_snap(1, "main")),
                supports_panel_multiplexing: true,
                supports_large_body: false,
            }),
        );
        worker_tx.write_all(&protocol::encode_frame(&reg)?).await?;
        worker_tx.flush().await?;
        let _ready = read_async_frame(&mut worker_rx).await?;

        let open1 = read_async_frame(&mut worker_rx).await?;
        let s1 = send_frame(
            &mut out_seq,
            2,
            1,
            open1.request_id,
            envelope::Body::Snapshot(make_snap(1, "c2-v1")),
        );
        worker_tx.write_all(&protocol::encode_frame(&s1)?).await?;
        worker_tx.flush().await?;
        send_errors_w.notified().await;
        for _ in 0..=QUEUE_CAPACITY {
            let err_ev = send_frame(
                &mut out_seq,
                2,
                1,
                0,
                envelope::Body::Error(Failure::new(ErrorCode::Internal, "fill-events")),
            );
            worker_tx.write_all(&protocol::encode_frame(&err_ev)?).await?;
        }
        worker_tx.flush().await?;

        let mut close_count = 0usize;
        let mut reopen_ok = false;
        while let Ok(env) = read_async_frame(&mut worker_rx).await {
            match env.body {
                    Some(envelope::Body::Shutdown(_)) => break,
                    Some(envelope::Body::CommitEditor(_)) => {
                        bail!("revoked pre-permit CommitEditor must never reach worker");
                    }
                    Some(envelope::Body::CloseRequest(ref c)) => {
                        ensure!(!c.destroy_panel);
                        close_count += 1;
                        let ack = send_frame(
                            &mut out_seq,
                            env.panel_id,
                            1,
                            env.request_id,
                            envelope::Body::Ack(panel::Empty {}),
                        );
                        worker_tx.write_all(&protocol::encode_frame(&ack)?).await?;
                        worker_tx.flush().await?;
                    }
                    Some(envelope::Body::OpenRequest(ref open)) => {
                        ensure!(
                            open.message_revision == 2,
                            "pre-permit OpenRequest(rev={}) must never reach worker after revoke",
                            open.message_revision
                        );
                        reopen_ok = true;
                        let snap = send_frame(
                            &mut out_seq,
                            env.panel_id,
                            2,
                            env.request_id,
                            envelope::Body::Snapshot(make_snap(
                                2,
                                "c2-reopened-after-unpermitted-cancel",
                            )),
                        );
                        worker_tx.write_all(&protocol::encode_frame(&snap)?).await?;
                        worker_tx.flush().await?;
                    }
                    Some(envelope::Body::UiEvent(_)) => {
                        let ack = send_frame(
                            &mut out_seq,
                            env.panel_id,
                            1,
                            env.request_id,
                            envelope::Body::Ack(panel::Empty {}),
                        );
                        worker_tx.write_all(&protocol::encode_frame(&ack)?).await?;
                        worker_tx.flush().await?;
                    }
                    _ => {}
            }
        }
        ensure!(close_count == 1 && reopen_ok);
        Ok::<_, anyhow::Error>(())
    });

    let (mut supervisor, _services) = crate::plugins::supervisor::spawn_test_duplex_supervisor(
        identity,
        manifest,
        BTreeSet::from([Permission::TrafficRead, Permission::DraftWrite]),
        host_to_worker,
        worker_to_host,
    )
    .await
    .unwrap();
    let handle = supervisor.handle();

    let primary = Uuid::new_v4();
    let extra_res = Uuid::new_v4();
    let _ = timeout(MAX_OPERATION, supervisor.events.recv()).await.unwrap().unwrap();
    handle
        .open_request_on_panel(2, primary, 1, false, request())
        .await
        .unwrap();
    let _ = timeout(MAX_OPERATION, supervisor.events.recv()).await.unwrap().unwrap();

    // Signal worker to send QUEUE_CAPACITY + 1 Errors: first 128 fill `supervisor.events`,
    // Error #129 parks Running inside `self.events.send` while `handle.control` is 0/128 empty.
    send_errors.notify_one();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 1. Saturate ALL 128 slots of `handle.control` with Focus events (0 channel permits left).
    for i in 0..QUEUE_CAPACITY {
        handle
            .try_event(UiEvent {
                node_id: 1,
                editors: vec![],
                event: Some(panel::ui_event::Event::Focus(true)),
            })
            .unwrap_or_else(|e| panic!("failed filling control slot {i}: {e}"));
    }
    assert!(
        handle
            .try_event(UiEvent {
                node_id: 1,
                editors: vec![],
                event: Some(panel::ui_event::Event::Focus(true)),
            })
            .is_err()
    );

    // 2. Spawn `open_request_on_panel` and `commit_on_panel` while `control` is 128/128 full
    //    (neither can reserve a permit yet!), plus a pre-polled `begin_open` handle!
    let open_h = handle.clone();
    let mut open_unpermitted_task = tokio::spawn(async move {
        open_h
            .open_request_on_panel(2, extra_res, 1, false, request())
            .await
    });
    let commit_h = handle.clone();
    let mut commit_unpermitted_task = tokio::spawn(async move {
        commit_h
            .commit_on_panel(
                2,
                CommitEditor {
                    resource_id: extra_res.as_bytes().to_vec(),
                    message_revision: 1,
                    request: Some(request().into()),
                    editor: Some(EditorSnapshot {
                        document_id: 1,
                        revision: 1,
                        contents: b"stale-draft".to_vec(),
                        ..Default::default()
                    }),
                },
                CancellationToken::new(),
            )
            .await
    });
    let pre_polled_open = handle.begin_open(2, extra_res);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut open_unpermitted_task)
            .await
            .is_err(),
        "open must be waiting on full control channel before revoke"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut commit_unpermitted_task)
            .await
            .is_err(),
        "commit must be waiting on full control channel before revoke"
    );

    // 3. Issue `revoke_panel_resource(2, extra_res, false)` -> MUST immediately cancel the parked
    //    waiters BEFORE `handle.control` ever recovers capacity, AND settle Ok(()) once events drain!
    let rev_h = handle.clone();
    let mut revoke_task = tokio::spawn(async move {
        rev_h.revoke_panel_resource(2, extra_res, false).await
    });
    assert!(
        timeout(MAX_OPERATION, open_unpermitted_task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(
        timeout(MAX_OPERATION, commit_unpermitted_task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(
        timeout(
            MAX_OPERATION,
            pre_polled_open.open_request(1, false, request()),
        )
        .await
        .unwrap()
        .is_err()
    );

    // 4. Drain `supervisor.events` -> Running resumes, settles `CloseRequest`, drains all 128 Focus
    //    events cleanly, and NEVER receives `OpenRequest(rev=1)` or `CommitEditor`.
    timeout(MAX_OPERATION, async {
        loop {
            tokio::select! {
                res = &mut revoke_task => {
                    res.unwrap().unwrap();
                    break;
                }
                _ = supervisor.events.recv() => {}
            }
        }
    })
    .await
    .unwrap();

    // 5. Explicit post-revoke reopen with `rev = 2` succeeds cleanly!
    let reopen_h = handle.clone();
    let mut reopen_task = tokio::spawn(async move {
        reopen_h
            .open_request_on_panel(2, extra_res, 2, false, request())
            .await
    });
    let reopened = timeout(MAX_OPERATION, async {
        loop {
            tokio::select! {
                res = &mut reopen_task => break res.unwrap(),
                _ = supervisor.events.recv() => {}
            }
        }
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(reopened.title, "c2-reopened-after-unpermitted-cancel");
    while supervisor.events.try_recv().is_ok() {}

    supervisor.stop().await.unwrap();
    timeout(MAX_OPERATION, worker_task).await.unwrap().unwrap().unwrap();
}

#[tokio::test]
async fn real_duplex_supervisor_begin_commit_cancelled_before_poll_on_bound_resource() {
    let identity = crate::plugins::tests_runtime::identity();
    let manifest = PluginManifest {
        id: "lab.begin-commit-prepoll".into(),
        version: "1.5.0".into(),
        api_version: 1,
        engine: "native".into(),
        entry: "worker".into(),
        permissions: vec![Permission::TrafficRead, Permission::DraftWrite],
        contributions: vec!["suite_panel".into(), "message_editor".into()],
    };
    let (host_to_worker, mut worker_rx) = tokio::io::duplex(64 * 1024);
    let (mut worker_tx, worker_to_host) = tokio::io::duplex(64 * 1024);
    let commit_requests = Arc::new(std::sync::Mutex::new(Vec::<(u64, Uuid, u64)>::new()));
    let commit_responses = Arc::new(std::sync::Mutex::new(Vec::<(u64, Uuid, u64)>::new()));
    let commit_reqs_w = commit_requests.clone();
    let commit_resps_w = commit_responses.clone();

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

    let worker_task = tokio::spawn(async move {
        let hello = read_async_frame(&mut worker_rx).await?;
        let mut out_seq = 0u64;
        let reg = send_frame(
            &mut out_seq,
            protocol::PANEL_ID,
            1,
            hello.request_id,
            envelope::Body::Register(protocol::Register {
                plugin_id: "lab.begin-commit-prepoll".into(),
                plugin_version: "1.5.0".into(),
                required_components: vec![],
                panel: Some(make_snap(1, "main")),
                supports_panel_multiplexing: true,
                supports_large_body: false,
            }),
        );
        worker_tx.write_all(&protocol::encode_frame(&reg)?).await?;
        worker_tx.flush().await?;
        let _ready = read_async_frame(&mut worker_rx).await?;

        while let Ok(env) = read_async_frame(&mut worker_rx).await {
            match env.body {
                    Some(envelope::Body::Shutdown(_)) => break,
                    Some(envelope::Body::OpenRequest(_)) => {
                        let snap = send_frame(
                            &mut out_seq,
                            env.panel_id,
                            1,
                            env.request_id,
                            envelope::Body::Snapshot(make_snap(1, "c2-bound")),
                        );
                        worker_tx.write_all(&protocol::encode_frame(&snap)?).await?;
                        worker_tx.flush().await?;
                    }
                    Some(envelope::Body::CommitEditor(c)) => {
                        let rid = protocol::uuid(&c.resource_id).map_err(anyhow::Error::msg)?;
                        commit_reqs_w
                            .lock()
                            .unwrap()
                            .push((env.panel_id, rid, c.message_revision));
                        let committed = send_frame(
                            &mut out_seq,
                            env.panel_id,
                            1,
                            env.request_id,
                            envelope::Body::Committed(protocol::Committed {
                                resource_id: c.resource_id,
                                message_revision: c.message_revision,
                                document_id: 1,
                                document_revision: c.editor.as_ref().map_or(1, |e| e.revision),
                                request: c.request,
                            }),
                        );
                        worker_tx
                            .write_all(&protocol::encode_frame(&committed)?)
                            .await?;
                        worker_tx.flush().await?;
                    }
                    Some(envelope::Body::CommitResponseEditor(c)) => {
                        let rid = protocol::uuid(&c.resource_id).map_err(anyhow::Error::msg)?;
                        commit_resps_w
                            .lock()
                            .unwrap()
                            .push((env.panel_id, rid, c.message_revision));
                        let committed = send_frame(
                            &mut out_seq,
                            env.panel_id,
                            1,
                            env.request_id,
                            envelope::Body::ResponseCommitted(protocol::ResponseCommitted {
                                resource_id: c.resource_id,
                                message_revision: c.message_revision,
                                document_id: 1,
                                document_revision: c.editor.as_ref().map_or(1, |e| e.revision),
                                response: c.response,
                            }),
                        );
                        worker_tx
                            .write_all(&protocol::encode_frame(&committed)?)
                            .await?;
                        worker_tx.flush().await?;
                    }
                    Some(envelope::Body::CloseRequest(_)) => {
                        let ack = send_frame(
                            &mut out_seq,
                            env.panel_id,
                            1,
                            env.request_id,
                            envelope::Body::Ack(panel::Empty {}),
                        );
                        worker_tx.write_all(&protocol::encode_frame(&ack)?).await?;
                        worker_tx.flush().await?;
                    }
                    _ => {}
            }
        }
        Ok::<_, anyhow::Error>(())
    });

    let (mut supervisor, _services) = crate::plugins::supervisor::spawn_test_duplex_supervisor(
        identity,
        manifest,
        BTreeSet::from([Permission::TrafficRead, Permission::DraftWrite]),
        host_to_worker,
        worker_to_host,
    )
    .await
    .unwrap();
    let handle = supervisor.handle();

    let bound_res = Uuid::new_v4();
    let _ = timeout(MAX_OPERATION, supervisor.events.recv()).await.unwrap().unwrap();
    let snap = handle
        .open_request_on_panel(2, bound_res, 1, false, request())
        .await
        .unwrap();
    assert_eq!(snap.title, "c2-bound");
    let _ = timeout(MAX_OPERATION, supervisor.events.recv()).await.unwrap().unwrap();

    // Register PendingCommit waiters while `bound_res` is genuinely bound and control channel is empty,
    // cancel them BEFORE first poll (before any remote revoke runs), then poll them.
    let unpolled_req_commit = handle.begin_commit(2, Some(bound_res));
    let unpolled_resp_commit = handle.begin_commit(2, Some(bound_res));
    handle.cancel_pending_resource_ops(2, bound_res, false);

    assert!(
        timeout(
            MAX_OPERATION,
            unpolled_req_commit.commit_request(
                CommitEditor {
                    resource_id: bound_res.as_bytes().to_vec(),
                    message_revision: 1,
                    request: Some(request().into()),
                    editor: Some(EditorSnapshot {
                        document_id: 1,
                        revision: 2,
                        contents: b"prepoll-stale-req".to_vec(),
                        ..Default::default()
                    }),
                },
                CancellationToken::new(),
            ),
        )
        .await
        .unwrap()
        .is_err()
    );
    assert!(
        timeout(
            MAX_OPERATION,
            unpolled_resp_commit.commit_response(
                protocol::CommitResponseEditor {
                    resource_id: bound_res.as_bytes().to_vec(),
                    message_revision: 1,
                    response: Some(
                        HttpResponse {
                            status: 200,
                            version: mio_plugin_api::HttpVersion::Http11,
                            reason_phrase: Some("OK".into()),
                            headers: vec![],
                            body: b"body".to_vec(),
                            trailers: vec![],
                        }
                        .into(),
                    ),
                    editor: Some(EditorSnapshot {
                        document_id: 1,
                        revision: 2,
                        contents: b"prepoll-stale-resp".to_vec(),
                        ..Default::default()
                    }),
                },
                CancellationToken::new(),
            ),
        )
        .await
        .unwrap()
        .is_err()
    );
    assert!(commit_requests.lock().unwrap().is_empty());
    assert!(commit_responses.lock().unwrap().is_empty());

    // Positive control: `bound_res` IS genuinely bound on panel 2 and still commits normally.
    let committed_req = handle
        .commit_on_panel(
            2,
            CommitEditor {
                resource_id: bound_res.as_bytes().to_vec(),
                message_revision: 1,
                request: Some(request().into()),
                editor: Some(EditorSnapshot {
                    document_id: 1,
                    revision: 2,
                    contents: b"live-req".to_vec(),
                    ..Default::default()
                }),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(committed_req.url, request().url);
    let committed_resp = handle
        .commit_response_on_panel(
            2,
            protocol::CommitResponseEditor {
                resource_id: bound_res.as_bytes().to_vec(),
                message_revision: 1,
                response: Some(
                    HttpResponse {
                        status: 200,
                        version: mio_plugin_api::HttpVersion::Http11,
                        reason_phrase: Some("OK".into()),
                        headers: vec![],
                        body: b"live-resp".to_vec(),
                        trailers: vec![],
                    }
                    .into(),
                ),
                editor: Some(EditorSnapshot {
                    document_id: 1,
                    revision: 2,
                    contents: b"live-resp".to_vec(),
                    ..Default::default()
                }),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(committed_resp.body, b"live-resp");
    assert_eq!(commit_requests.lock().unwrap().as_slice(), &[(2, bound_res, 1)]);
    assert_eq!(commit_responses.lock().unwrap().as_slice(), &[(2, bound_res, 1)]);

    handle
        .revoke_panel_resource(2, bound_res, true)
        .await
        .unwrap();
    while supervisor.events.try_recv().is_ok() {}
    supervisor.stop().await.unwrap();
    timeout(MAX_OPERATION, worker_task).await.unwrap().unwrap().unwrap();
}

#[tokio::test]
async fn real_duplex_supervisor_batch_revoke_cancels_deferred_second_open_before_first_close_ack() {
    let identity = crate::plugins::tests_runtime::identity();
    let manifest = PluginManifest {
        id: "lab.batch-revoke-deferred".into(),
        version: "1.5.0".into(),
        api_version: 1,
        engine: "native".into(),
        entry: "worker".into(),
        permissions: vec![Permission::TrafficRead, Permission::DraftWrite],
        contributions: vec!["suite_panel".into(), "message_editor".into()],
    };
    let (host_to_worker, mut worker_rx) = tokio::io::duplex(64 * 1024);
    let (mut worker_tx, worker_to_host) = tokio::io::duplex(64 * 1024);

    let res_c = Uuid::new_v4();
    let res_a = Uuid::new_v4();
    let res_b = Uuid::new_v4();
    let saw_close_c = Arc::new(tokio::sync::Notify::new());
    let saw_close_c_w = saw_close_c.clone();
    let release_close_c = Arc::new(tokio::sync::Notify::new());
    let release_close_c_w = release_close_c.clone();
    let received_opens = Arc::new(std::sync::Mutex::new(Vec::<(u64, Uuid, u64)>::new()));
    let received_closes = Arc::new(std::sync::Mutex::new(Vec::<(u64, Uuid, bool)>::new()));
    let max_inflight_closes = Arc::new(std::sync::Mutex::new(0usize));
    let opens_w = received_opens.clone();
    let closes_w = received_closes.clone();
    let max_inflight_w = max_inflight_closes.clone();

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

    let worker_task = tokio::spawn(async move {
        let hello = read_async_frame(&mut worker_rx).await?;
        let mut out_seq = 0u64;
        let reg = send_frame(
            &mut out_seq,
            protocol::PANEL_ID,
            1,
            hello.request_id,
            envelope::Body::Register(protocol::Register {
                plugin_id: "lab.batch-revoke-deferred".into(),
                plugin_version: "1.5.0".into(),
                required_components: vec![],
                panel: Some(make_snap(1, "main")),
                supports_panel_multiplexing: true,
                supports_large_body: false,
            }),
        );
        worker_tx.write_all(&protocol::encode_frame(&reg)?).await?;
        worker_tx.flush().await?;
        let _ready = read_async_frame(&mut worker_rx).await?;

        let mut revs: HashMap<u64, u64> = HashMap::from([(protocol::PANEL_ID, 1)]);
        let mut inflight_closes = 0usize;
        let mut held_close_c: Option<(u64, u64, u64)> = None;
        loop {
            tokio::select! {
                biased;
                _ = release_close_c_w.notified(), if held_close_c.is_some() => {
                    let (panel_id, req_id, rev) = held_close_c.take().unwrap();
                    inflight_closes = inflight_closes.saturating_sub(1);
                    let ack = send_frame(
                        &mut out_seq,
                        panel_id,
                        rev,
                        req_id,
                        envelope::Body::Ack(panel::Empty {}),
                    );
                    worker_tx.write_all(&protocol::encode_frame(&ack)?).await?;
                    worker_tx.flush().await?;
                }
                incoming = read_async_frame(&mut worker_rx) => {
                    let Ok(env) = incoming else { break; };
                    match env.body {
                        Some(envelope::Body::Shutdown(_)) => break,
                        Some(envelope::Body::OpenRequest(ref open)) => {
                            let rid = protocol::uuid(&open.resource_id).map_err(anyhow::Error::msg)?;
                            opens_w
                                .lock()
                                .unwrap()
                                .push((env.panel_id, rid, open.message_revision));
                            let rev = revs.entry(env.panel_id).or_insert(0);
                            *rev += 1;
                            let snap = send_frame(
                                &mut out_seq,
                                env.panel_id,
                                *rev,
                                env.request_id,
                                envelope::Body::Snapshot(make_snap(
                                    *rev,
                                    &format!("p-{}-rev-{}", env.panel_id, open.message_revision),
                                )),
                            );
                            worker_tx.write_all(&protocol::encode_frame(&snap)?).await?;
                            worker_tx.flush().await?;
                        }
                        Some(envelope::Body::CloseRequest(ref close)) => {
                            let rid = protocol::uuid(&close.resource_id).map_err(anyhow::Error::msg)?;
                            closes_w
                                .lock()
                                .unwrap()
                                .push((env.panel_id, rid, close.destroy_panel));
                            inflight_closes += 1;
                            {
                                let mut max = max_inflight_w.lock().unwrap();
                                *max = (*max).max(inflight_closes);
                            }
                            let rev = *revs.get(&env.panel_id).unwrap_or(&1);
                            if rid == res_c && !close.destroy_panel {
                                held_close_c = Some((env.panel_id, env.request_id, rev));
                                saw_close_c_w.notify_one();
                            } else {
                                inflight_closes = inflight_closes.saturating_sub(1);
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
        BTreeSet::from([Permission::TrafficRead, Permission::DraftWrite]),
        host_to_worker,
        worker_to_host,
    )
    .await
    .unwrap();
    let handle = supervisor.handle();

    let _ = timeout(MAX_OPERATION, supervisor.events.recv()).await.unwrap().unwrap();
    for res in [res_c, res_a, res_b] {
        handle
            .open_request_on_panel(2, res, 1, false, request())
            .await
            .unwrap();
        let _ = timeout(MAX_OPERATION, supervisor.events.recv()).await.unwrap().unwrap();
    }

    // 1. Start CloseRequest(res_c) and hold its Ack at the worker -> non-destroy close barrier active on panel 2.
    let h_c = handle.clone();
    let revoke_c_task =
        tokio::spawn(async move { h_c.revoke_panel_resource(2, res_c, false).await });
    timeout(MAX_OPERATION, saw_close_c.notified()).await.unwrap();

    // 2. Enqueue older `Open(res_b, rev=2)` on panel 2 -> enters Running::panels[2].deferred_commands behind CloseRequest(res_c).
    let h_b = handle.clone();
    let mut deferred_open_b = tokio::spawn(async move {
        h_b.open_request_on_panel(2, res_b, 2, false, request()).await
    });
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut deferred_open_b)
            .await
            .is_err(),
        "open_b must be parked behind CloseRequest(res_c) barrier"
    );

    // 3. Batch revoke [res_a, res_b] while CloseRequest(res_c) is still unacked:
    //    `deferred_open_b` MUST fail immediately BEFORE CloseRequest(res_c) receives its Ack!
    let h_batch = handle.clone();
    let revoke_batch_task = tokio::spawn(async move {
        h_batch.revoke_panel_resources(2, vec![res_a, res_b]).await
    });
    assert!(
        timeout(MAX_OPERATION, deferred_open_b)
            .await
            .unwrap()
            .unwrap()
            .is_err(),
        "batch revoke must cancel deferred open for res_b before CloseRequest(res_c) Ack"
    );

    // Worker must not have seen res_b rev=2 open or res_a/res_b closes yet while res_c is held.
    assert!(received_closes.lock().unwrap().as_slice() == [(2, res_c, false)]);
    assert!(
        received_opens
            .lock()
            .unwrap()
            .iter()
            .all(|(_, rid, rev)| !(*rid == res_b && *rev == 2))
    );

    // Unrelated healthy child panel 3 must NOT be rejected as congested while panel 2 queues [res_a, res_b]
    // behind panel 2's in-flight non-destroy CloseRequest(res_c).
    let p3_res = Uuid::new_v4();
    let p3_snap = handle
        .open_request_on_panel(3, p3_res, 1, false, request())
        .await
        .unwrap();
    assert_eq!(p3_snap.title, "p-3-rev-1");
    let _ = timeout(MAX_OPERATION, supervisor.events.recv()).await.unwrap().unwrap();

    // 4. Release CloseRequest(res_c) Ack -> res_a then res_b close sequentially without ever dispatching res_b rev=2.
    release_close_c.notify_one();
    timeout(MAX_OPERATION, revoke_c_task).await.unwrap().unwrap().unwrap();
    timeout(MAX_OPERATION, revoke_batch_task).await.unwrap().unwrap().unwrap();

    assert_eq!(*max_inflight_closes.lock().unwrap(), 1);
    assert_eq!(
        received_closes.lock().unwrap().as_slice(),
        &[(2, res_c, false), (2, res_a, false), (2, res_b, false)]
    );
    assert!(
        received_opens
            .lock()
            .unwrap()
            .iter()
            .all(|(_, rid, rev)| !(*rid == res_b && *rev == 2))
    );

    // 5. Positive control: post-batch reopen with newer revision `rev = 3` succeeds cleanly.
    let reopened = handle
        .open_request_on_panel(2, res_b, 3, false, request())
        .await
        .unwrap();
    assert_eq!(reopened.title, "p-2-rev-3");
    handle.revoke_panel_resource(2, res_b, true).await.unwrap();
    while supervisor.events.try_recv().is_ok() {}
    supervisor.stop().await.unwrap();
    timeout(MAX_OPERATION, worker_task).await.unwrap().unwrap().unwrap();
}

#[tokio::test]
async fn real_duplex_supervisor_revoke_retracts_unsent_open_from_outbound_queue_when_writer_blocked()
{
    // Part 0: Direct Running::control_with_seq verification — even when `outgoing` regains capacity before
    // `control_with_seq` runs, `Revoke` drains urgent/retracts the unsent `Open` from `outbound_queue`
    // BEFORE flushing outbound frames, sending ONLY `CloseRequest` and ZERO `OpenRequest` or `Cancel`.
    {
        let (mut running, _calls, _updates) = running();
        let (outgoing, mut wire) = mpsc::channel(2);
        running
            .send(&outgoing, envelope::Body::Cancel(protocol::Cancel { request_id: 901 }), 0)
            .unwrap();
        running
            .send(&outgoing, envelope::Body::Cancel(protocol::Cancel { request_id: 902 }), 0)
            .unwrap();
        let res_unsent = Uuid::new_v4();
        let (open_tx, mut open_rx) = oneshot::channel();
        running
            .control_with_seq(
                9,
                Control::Open {
                    panel_id: protocol::PANEL_ID,
                    id: res_unsent,
                    revision: 1,
                    read_only: false,
                    request: request(),
                    reply: open_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert_eq!(running.outbound_queue.len(), 1);
        let _ = wire.try_recv().unwrap();
        let _ = wire.try_recv().unwrap();

        let (rev_tx, rev_rx) = oneshot::channel();
        running
            .control_with_seq(
                10,
                Control::Revoke {
                    panel_id: protocol::PANEL_ID,
                    id: res_unsent,
                    remove_panel: false,
                    reply: rev_tx,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert!(open_rx.try_recv().unwrap().is_err());
        assert!(running.outbound_queue.is_empty());

        let first = wire.try_recv().unwrap();
        assert!(
            matches!(first.body, Some(envelope::Body::CloseRequest(ref c)) if !c.destroy_panel),
            "expected only CloseRequest after retracting unsent OpenRequest, got {:?}",
            first.body
        );
        assert!(wire.try_recv().is_err(), "retracted OpenRequest and Cancel must not be sent");

        let mut ack = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        );
        ack.in_reply_to = first.request_id;
        running.frame(ack, &outgoing).await.unwrap();
        rev_rx.await.unwrap().unwrap();
    }

    // Part A + B: Real duplex transport with 512B blocked writer pipe and contrast with already-written Open.
    let identity = crate::plugins::tests_runtime::identity();
    let manifest = PluginManifest {
        id: "lab.outbound-retract".into(),
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

    let burst_count = OUTGOING_CAPACITY + 24;
    let worker_task = tokio::spawn(async move {
        let hello = read_async_frame(&mut worker_rx).await?;
        let mut out_seq = 0u64;
        let reg = send_frame(
            &mut out_seq,
            protocol::PANEL_ID,
            1,
            hello.request_id,
            envelope::Body::Register(protocol::Register {
                plugin_id: "lab.outbound-retract".into(),
                plugin_version: "1.5.0".into(),
                required_components: vec![],
                panel: Some(make_snap(1, "main")),
                supports_panel_multiplexing: false,
                supports_large_body: false,
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
                    for _ in 0..burst_count {
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
                        envelope::Body::Error(Failure::new(ErrorCode::Internal, "flush-fence")),
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

    // Block worker reader and fill the 512B writer pipe + `outgoing` (256) so `HostResult`s spill
    // into `Running::outbound_queue`.
    block_reader.notify_one();
    timeout(MAX_OPERATION, burst_sent.notified()).await.unwrap();
    let fence_ev = timeout(MAX_OPERATION, supervisor.events.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        fence_ev,
        PluginEvent::Error { ref failure, .. } if failure.message == "flush-fence"
    ));

    // Spawn `open_request(res_b)` -> accepted into `Running::outbound_queue` behind the spilled HostResults.
    let h_b = handle.clone();
    let mut open_b_task = tokio::spawn(async move {
        h_b.open_request(res_b, 1, false, request()).await
    });
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut open_b_task)
            .await
            .is_err(),
        "open_b must be parked inside blocked outbound_queue"
    );

    // Issue batch revoke [res_a, res_b] while writer pipe is STILL blocked:
    // `open_b_task` MUST immediately fail (retracted from `outbound_queue`) BEFORE `unblock_reader` fires!
    let revoke_batch = handle.begin_revoke_panel_resources(protocol::PANEL_ID, vec![res_a, res_b]);
    assert!(
        timeout(MAX_OPERATION, open_b_task)
            .await
            .unwrap()
            .unwrap()
            .is_err(),
        "batch revoke must fail unsent open_b immediately while writer is still blocked"
    );

    // Unblock worker reader -> HostResults + CloseRequest(res_a) then CloseRequest(res_b) flush cleanly,
    // with ZERO `OpenRequest(res_b)` and ZERO `Cancel` sent to the worker!
    unblock_reader.notify_one();
    timeout(MAX_OPERATION, async {
        let mut wait_fut = std::pin::pin!(revoke_batch.wait());
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
        &[(protocol::PANEL_ID, res_a, false), (protocol::PANEL_ID, res_b, false)]
    );

    // Part B: Contrast — when `OpenRequest(res_c)` HAS already been written to the worker, revoking `res_c`
    // sends an explicit `Cancel` frame targeting `OpenRequest(res_c)`'s `request_id` plus `CloseRequest(res_c)`.
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
        &[
            (protocol::PANEL_ID, res_a, false),
            (protocol::PANEL_ID, res_b, false),
            (protocol::PANEL_ID, res_c, false),
        ]
    );

    while supervisor.events.try_recv().is_ok() {}
    supervisor.stop().await.unwrap();
    timeout(MAX_OPERATION, worker_task).await.unwrap().unwrap().unwrap();
}

#[tokio::test]
async fn real_duplex_supervisor_urgent_burst_past_queue_capacity_still_cancels_deferred_open_before_ready_close_ack()
{
    let identity = crate::plugins::tests_runtime::identity();
    let manifest = PluginManifest {
        id: "lab.urgent-burst-revoke".into(),
        version: "1.5.0".into(),
        api_version: 1,
        engine: "native".into(),
        entry: "worker".into(),
        permissions: vec![Permission::TrafficRead, Permission::DraftWrite],
        contributions: vec!["suite_panel".into(), "message_editor".into()],
    };
    let (host_to_worker, mut worker_rx) = tokio::io::duplex(64 * 1024);
    let (mut worker_tx, worker_to_host) = tokio::io::duplex(64 * 1024);

    let res_c = Uuid::new_v4();
    let res_a = Uuid::new_v4();
    let res_b = Uuid::new_v4();
    let saw_close_c = Arc::new(tokio::sync::Notify::new());
    let release_close_c = Arc::new(tokio::sync::Notify::new());
    let sent_close_c_ack = Arc::new(tokio::sync::Notify::new());
    let send_errors = Arc::new(tokio::sync::Notify::new());
    let errors_sent = Arc::new(tokio::sync::Notify::new());
    let saw_close_c_w = saw_close_c.clone();
    let release_close_c_w = release_close_c.clone();
    let sent_close_c_ack_w = sent_close_c_ack.clone();
    let send_errors_w = send_errors.clone();
    let errors_sent_w = errors_sent.clone();
    let received_opens = Arc::new(std::sync::Mutex::new(Vec::<(u64, Uuid, u64)>::new()));
    let received_closes = Arc::new(std::sync::Mutex::new(Vec::<(u64, Uuid, bool)>::new()));
    let max_inflight_closes = Arc::new(std::sync::Mutex::new(0usize));
    let opens_w = received_opens.clone();
    let closes_w = received_closes.clone();
    let max_inflight_w = max_inflight_closes.clone();

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

    let worker_task = tokio::spawn(async move {
        let hello = read_async_frame(&mut worker_rx).await?;
        let mut out_seq = 0u64;
        let reg = send_frame(
            &mut out_seq,
            protocol::PANEL_ID,
            1,
            hello.request_id,
            envelope::Body::Register(protocol::Register {
                plugin_id: "lab.urgent-burst-revoke".into(),
                plugin_version: "1.5.0".into(),
                required_components: vec![],
                panel: Some(make_snap(1, "main")),
                supports_panel_multiplexing: true,
                supports_large_body: false,
            }),
        );
        worker_tx.write_all(&protocol::encode_frame(&reg)?).await?;
        worker_tx.flush().await?;
        let _ready = read_async_frame(&mut worker_rx).await?;

        let mut revs: HashMap<u64, u64> = HashMap::from([(protocol::PANEL_ID, 1)]);
        let mut inflight_closes = 0usize;
        let mut held_close_c: Option<(u64, u64, u64)> = None;
        let mut sent_err_burst = false;
        loop {
            tokio::select! {
                biased;
                _ = send_errors_w.notified(), if !sent_err_burst => {
                    sent_err_burst = true;
                    for _ in 0..=QUEUE_CAPACITY {
                        let err_ev = send_frame(
                            &mut out_seq,
                            protocol::PANEL_ID,
                            1,
                            0,
                            envelope::Body::Error(Failure::new(ErrorCode::Internal, "park-running")),
                        );
                        worker_tx.write_all(&protocol::encode_frame(&err_ev)?).await?;
                    }
                    worker_tx.flush().await?;
                    errors_sent_w.notify_one();
                }
                _ = release_close_c_w.notified(), if held_close_c.is_some() => {
                    let (panel_id, req_id, rev) = held_close_c.take().unwrap();
                    inflight_closes = inflight_closes.saturating_sub(1);
                    let ack = send_frame(
                        &mut out_seq,
                        panel_id,
                        rev,
                        req_id,
                        envelope::Body::Ack(panel::Empty {}),
                    );
                    worker_tx.write_all(&protocol::encode_frame(&ack)?).await?;
                    worker_tx.flush().await?;
                    sent_close_c_ack_w.notify_one();
                }
                incoming = read_async_frame(&mut worker_rx) => {
                    let Ok(env) = incoming else { break; };
                    match env.body {
                        Some(envelope::Body::Shutdown(_)) => break,
                        Some(envelope::Body::OpenRequest(ref open)) => {
                            let rid = protocol::uuid(&open.resource_id).map_err(anyhow::Error::msg)?;
                            opens_w
                                .lock()
                                .unwrap()
                                .push((env.panel_id, rid, open.message_revision));
                            let rev = revs.entry(env.panel_id).or_insert(0);
                            *rev += 1;
                            let snap = send_frame(
                                &mut out_seq,
                                env.panel_id,
                                *rev,
                                env.request_id,
                                envelope::Body::Snapshot(make_snap(
                                    *rev,
                                    &format!("p-{}-rev-{}", env.panel_id, open.message_revision),
                                )),
                            );
                            worker_tx.write_all(&protocol::encode_frame(&snap)?).await?;
                            worker_tx.flush().await?;
                        }
                        Some(envelope::Body::CloseRequest(ref close)) => {
                            let rid = protocol::uuid(&close.resource_id).map_err(anyhow::Error::msg)?;
                            closes_w
                                .lock()
                                .unwrap()
                                .push((env.panel_id, rid, close.destroy_panel));
                            inflight_closes += 1;
                            {
                                let mut max = max_inflight_w.lock().unwrap();
                                *max = (*max).max(inflight_closes);
                            }
                            let rev = *revs.get(&env.panel_id).unwrap_or(&1);
                            if rid == res_c && !close.destroy_panel {
                                held_close_c = Some((env.panel_id, env.request_id, rev));
                                saw_close_c_w.notify_one();
                            } else {
                                inflight_closes = inflight_closes.saturating_sub(1);
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
        BTreeSet::from([Permission::TrafficRead, Permission::DraftWrite]),
        host_to_worker,
        worker_to_host,
    )
    .await
    .unwrap();
    let handle = supervisor.handle();

    let _ = timeout(MAX_OPERATION, supervisor.events.recv()).await.unwrap().unwrap();
    for res in [res_c, res_a, res_b] {
        handle
            .open_request_on_panel(2, res, 1, false, request())
            .await
            .unwrap();
        let _ = timeout(MAX_OPERATION, supervisor.events.recv()).await.unwrap().unwrap();
    }

    // 1. Start CloseRequest(res_c) and hold its Ack -> active non-destroy close barrier on panel 2.
    let h_c = handle.clone();
    let mut revoke_c_task =
        tokio::spawn(async move { h_c.revoke_panel_resource(2, res_c, false).await });
    timeout(MAX_OPERATION, saw_close_c.notified()).await.unwrap();

    // 2. Park older `Open(res_b, rev=2)` inside Running's panel-2 `deferred_commands`.
    let h_b = handle.clone();
    let mut deferred_open_b = tokio::spawn(async move {
        h_b.open_request_on_panel(2, res_b, 2, false, request()).await
    });
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut deferred_open_b)
            .await
            .is_err()
    );

    // 3. Park Running inside `self.events.send` with a full supervisor.events channel.
    send_errors.notify_one();
    timeout(MAX_OPERATION, errors_sent.notified()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;

    // 4. Burst past old `QUEUE_CAPACITY` (128) with unrelated urgent revokes, THEN synchronously
    //    register `batch_revoke` for [res_a, res_b] WITHOUT polling `.wait()` yet, AND make
    //    CloseRequest(res_c)'s Ack ALREADY readable in Running's `incoming` channel before waking it!
    let mut noise_revokes = Vec::with_capacity(QUEUE_CAPACITY + 8);
    for i in 0..(QUEUE_CAPACITY + 8) {
        noise_revokes.push(handle.begin_revoke_panel_resource(
            100 + i as u64,
            Uuid::new_v4(),
            false,
        ));
    }
    let batch_revoke = handle.begin_revoke_panel_resources(2, vec![res_a, res_b]);
    release_close_c.notify_one();
    timeout(MAX_OPERATION, sent_close_c_ack.notified()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(15)).await;

    // 5. Unblock Running by draining supervisor.events — WITHOUT ever polling `batch_revoke.wait()`:
    //    `Running` MUST drain all 136 noise revokes + `batch_revoke` BEFORE CloseRequest(res_c)'s ready
    //    Ack dispatches deferred `Open(res_b, rev=2)`.
    let mut open_b_done = false;
    let mut rev_c_done = false;
    timeout(MAX_OPERATION, async {
        while !(open_b_done && rev_c_done) {
            tokio::select! {
                _ = supervisor.events.recv() => {}
                res = &mut deferred_open_b, if !open_b_done => {
                    assert!(
                        res.unwrap().is_err(),
                        "synchronously registered batch revoke past QUEUE_CAPACITY must cancel deferred open before already-ready Ack"
                    );
                    open_b_done = true;
                }
                res = &mut revoke_c_task, if !rev_c_done => {
                    res.unwrap().unwrap();
                    rev_c_done = true;
                }
            }
        }
    })
    .await
    .unwrap();

    // 6. Now collect `batch_revoke.wait()` and all noise revoke replies cleanly while draining events.
    timeout(MAX_OPERATION, async {
        let mut wait_fut = std::pin::pin!(batch_revoke.wait());
        loop {
            tokio::select! {
                res = &mut wait_fut => {
                    res.unwrap();
                    break;
                }
                _ = supervisor.events.recv() => {}
            }
        }
        for nr in noise_revokes {
            nr.wait().await.unwrap();
        }
    })
    .await
    .unwrap();

    assert_eq!(*max_inflight_closes.lock().unwrap(), 1);
    assert_eq!(
        received_closes.lock().unwrap().as_slice(),
        &[(2, res_c, false), (2, res_a, false), (2, res_b, false)]
    );
    assert!(
        received_opens
            .lock()
            .unwrap()
            .iter()
            .all(|(_, rid, rev)| !(*rid == res_b && *rev == 2))
    );

    // 7. Positive control: post-batch reopen with `rev = 3` succeeds cleanly.
    let reopened = handle
        .open_request_on_panel(2, res_b, 3, false, request())
        .await
        .unwrap();
    assert_eq!(reopened.title, "p-2-rev-3");
    handle.revoke_panel_resource(2, res_b, true).await.unwrap();
    while supervisor.events.try_recv().is_ok() {}
    supervisor.stop().await.unwrap();
    timeout(MAX_OPERATION, worker_task).await.unwrap().unwrap().unwrap();
}

#[tokio::test]
async fn two_independent_ready_urgent_revokes_retract_second_unsent_open_before_first_flushes_outbound()
{
    // Sub-part 1: Two independent ready revokes sitting together in `urgent_commands` during `drain_urgent_controls`.
    let make_snap = |rev: u64, label: &str| PanelSnapshot {
        revision: rev,
        title: label.into(),
        root: Some(panel::Node::new(
            1,
            "ed",
            panel::node::Kind::Editor(panel::Editor {
                document_id: 1,
                revision: 1,
                contents: b"retained".to_vec(),
                format: panel::EditorFormat::Utf8 as i32,
                read_only: false,
            }),
        )),
    };
    let (mut running, _calls, _updates) = running();
    let (urgent_tx, urgent_rx) = mpsc::unbounded_channel();
    running.urgent_commands = Some(urgent_rx);
    let (outgoing, mut wire) = mpsc::channel(4);

    let res_a = Uuid::new_v4();
    let res_b = Uuid::new_v4();
    let (oa_tx, oa_rx) = oneshot::channel();
    running
        .control_with_seq(
            1,
            Control::Open {
                panel_id: protocol::PANEL_ID,
                id: res_a,
                revision: 1,
                read_only: false,
                request: request(),
                reply: oa_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let oa_w = wire.try_recv().unwrap();
    let mut snap_a = Envelope::new(
        running.identity,
        1,
        2,
        envelope::Body::Snapshot(make_snap(2, "a-bound")),
        MAX_OPERATION,
    );
    snap_a.in_reply_to = oa_w.request_id;
    running.frame(snap_a, &outgoing).await.unwrap();
    oa_rx.await.unwrap().unwrap();
    while wire.try_recv().is_ok() {}

    // Fill `outgoing` (4/4), enqueue `Open(res_b)` into `running.outbound_queue`, then drain `wire`
    // so `outgoing` has free capacity when urgent revokes arrive.
    for s in 901..=904u64 {
        running
            .send(&outgoing, envelope::Body::Cancel(protocol::Cancel { request_id: s }), 0)
            .unwrap();
    }
    let (ob_tx, mut ob_rx) = oneshot::channel();
    running
        .control_with_seq(
            2,
            Control::Open {
                panel_id: protocol::PANEL_ID,
                id: res_b,
                revision: 1,
                read_only: false,
                request: request(),
                reply: ob_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert_eq!(running.outbound_queue.len(), 1);
    for _ in 0..4 {
        let _ = wire.try_recv().unwrap();
    }

    let (ra_tx, ra_rx) = oneshot::channel();
    let (rb_tx, rb_rx) = oneshot::channel();
    urgent_tx
        .send(SequencedControl {
            seq: 20,
            command: Control::Revoke {
                panel_id: protocol::PANEL_ID,
                id: res_a,
                remove_panel: false,
                reply: ra_tx,
            },
        })
        .unwrap();
    urgent_tx
        .send(SequencedControl {
            seq: 21,
            command: Control::Revoke {
                panel_id: protocol::PANEL_ID,
                id: res_b,
                remove_panel: false,
                reply: rb_tx,
            },
        })
        .unwrap();

    running.drain_urgent_controls(&outgoing).unwrap();
    assert!(ob_rx.try_recv().unwrap().is_err());
    assert!(running.outbound_queue.is_empty());

    let cw_a = wire.try_recv().unwrap();
    assert!(
        matches!(cw_a.body, Some(envelope::Body::CloseRequest(ref c)) if protocol::uuid(&c.resource_id).ok() == Some(res_a) && !c.destroy_panel),
        "unexpected wire frame: {:?}",
        cw_a.body
    );
    assert!(wire.try_recv().is_err(), "retracted Open(res_b) or Cancel must not be sent");

    let mut ack_a = Envelope::new(running.identity, 2, 2, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION);
    ack_a.in_reply_to = cw_a.request_id;
    running.frame(ack_a, &outgoing).await.unwrap();
    ra_rx.await.unwrap().unwrap();

    let cw_b = wire.try_recv().unwrap();
    assert!(
        matches!(cw_b.body, Some(envelope::Body::CloseRequest(ref c)) if protocol::uuid(&c.resource_id).ok() == Some(res_b) && !c.destroy_panel)
    );
    assert!(wire.try_recv().is_err());
    let mut ack_b = Envelope::new(running.identity, 3, 2, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION);
    ack_b.in_reply_to = cw_b.request_id;
    running.frame(ack_b, &outgoing).await.unwrap();
    rb_rx.await.unwrap().unwrap();

    // Sub-part 2: First Revoke delivered via `control_with_seq` while second Revoke sits in `urgent_commands`,
    // plus child-panel `[NonDestroy, Destroy]` coalescing in one urgent pass.
    let res_a2 = Uuid::new_v4();
    let res_b2 = Uuid::new_v4();
    for s in 911..=914u64 {
        running
            .send(&outgoing, envelope::Body::Cancel(protocol::Cancel { request_id: s }), 0)
            .unwrap();
    }
    let (ob2_tx, mut ob2_rx) = oneshot::channel();
    running
        .control_with_seq(
            25,
            Control::Open {
                panel_id: protocol::PANEL_ID,
                id: res_b2,
                revision: 1,
                read_only: false,
                request: request(),
                reply: ob2_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert_eq!(running.outbound_queue.len(), 1);
    for _ in 0..4 {
        let _ = wire.try_recv().unwrap();
    }

    let (ra2_tx, ra2_rx) = oneshot::channel();
    let (rb2_tx, rb2_rx) = oneshot::channel();
    urgent_tx
        .send(SequencedControl {
            seq: 31,
            command: Control::Revoke {
                panel_id: protocol::PANEL_ID,
                id: res_b2,
                remove_panel: false,
                reply: rb2_tx,
            },
        })
        .unwrap();
    running
        .control_with_seq(
            30,
            Control::Revoke {
                panel_id: protocol::PANEL_ID,
                id: res_a2,
                remove_panel: false,
                reply: ra2_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(ob2_rx.try_recv().unwrap().is_err());
    assert!(running.outbound_queue.is_empty());

    let cw_a2 = wire.try_recv().unwrap();
    assert!(matches!(cw_a2.body, Some(envelope::Body::CloseRequest(ref c)) if protocol::uuid(&c.resource_id).ok() == Some(res_a2)));
    assert!(wire.try_recv().is_err());
    let mut ack_a2 = Envelope::new(running.identity, 4, 2, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION);
    ack_a2.in_reply_to = cw_a2.request_id;
    running.frame(ack_a2, &outgoing).await.unwrap();
    ra2_rx.await.unwrap().unwrap();
    let cw_b2 = wire.try_recv().unwrap();
    let mut ack_b2 = Envelope::new(running.identity, 5, 2, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION);
    ack_b2.in_reply_to = cw_b2.request_id;
    running.frame(ack_b2, &outgoing).await.unwrap();
    rb2_rx.await.unwrap().unwrap();

    // Child panel 2: ready [NonDestroy(extra), Destroy(primary)] revokes retract extra child Open claim
    // from `c_out` and coalesce to ONE destroy CloseRequest.
    let (c_out, mut c_wire) = mpsc::channel(OUTGOING_CAPACITY);
    let c_claims = Arc::new(OutboundWriteClaims::default());
    running.outbound_claims = Some(c_claims.clone());
    running.supports_panel_multiplexing = true;
    let c_primary = Uuid::new_v4();
    let c_extra = Uuid::new_v4();
    let (oc_tx, oc_rx) = oneshot::channel();
    running
        .control_with_seq(
            35,
            Control::Open {
                panel_id: 2,
                id: c_primary,
                revision: 1,
                read_only: false,
                request: request(),
                reply: oc_tx,
            },
            &c_out,
        )
        .await
        .unwrap();
    let oc_w = c_wire.try_recv().unwrap();
    assert!(c_claims.claim_for_write(2, oc_w.request_id));
    let mut snap_c = Envelope::new(
        running.identity,
        6,
        1,
        envelope::Body::Snapshot(make_snap(1, "c2")),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    snap_c.in_reply_to = oc_w.request_id;
    running.frame(snap_c, &c_out).await.unwrap();
    oc_rx.await.unwrap().unwrap();
    while c_wire.try_recv().is_ok() {}

    let (ce_tx, mut ce_rx) = oneshot::channel();
    running
        .control_with_seq(
            36,
            Control::Open {
                panel_id: 2,
                id: c_extra,
                revision: 1,
                read_only: false,
                request: request(),
                reply: ce_tx,
            },
            &c_out,
        )
        .await
        .unwrap();
    // Simulate `c_extra` OpenRequest sitting inside `c_out` before the writer task pops & claims it.
    let ce_w = c_wire.try_recv().unwrap();

    let (re_tx, re_rx) = oneshot::channel();
    let (rd_tx, rd_rx) = oneshot::channel();
    urgent_tx
        .send(SequencedControl {
            seq: 40,
            command: Control::Revoke {
                panel_id: 2,
                id: c_extra,
                remove_panel: false,
                reply: re_tx,
            },
        })
        .unwrap();
    urgent_tx
        .send(SequencedControl {
            seq: 41,
            command: Control::Revoke {
                panel_id: 2,
                id: c_primary,
                remove_panel: true,
                reply: rd_tx,
            },
        })
        .unwrap();
    running.drain_urgent_controls(&c_out).unwrap();
    assert!(ce_rx.try_recv().unwrap().is_err());
    assert!(!c_claims.claim_for_write(2, ce_w.request_id));

    let cw_destroy = c_wire.try_recv().unwrap();
    assert_eq!(cw_destroy.panel_id, 2);
    assert!(matches!(cw_destroy.body, Some(envelope::Body::CloseRequest(ref c)) if c.destroy_panel));
    assert!(c_wire.try_recv().is_err());
    let mut ack_destroy = Envelope::new(running.identity, 7, 1, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION).with_panel_id(2);
    ack_destroy.in_reply_to = cw_destroy.request_id;
    running.frame(ack_destroy, &c_out).await.unwrap();
    re_rx.await.unwrap().unwrap();
    rd_rx.await.unwrap().unwrap();
    assert!(!running.panels.contains_key(&2));
}
