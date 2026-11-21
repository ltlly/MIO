use super::*;
use super::helpers::{BlockingReader, BlockingWriter};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persistent_semantic_drift_rejects_ordinary_and_repeated_interactions_until_full_snapshot() {
    use std::sync::{Arc, Mutex};

    // Part A: Real SDK plugin without on_resync keeps rejecting ordinary & repeated SelectedOption(0)
    // after default on_resync Error until EditorChanged triggers a full confirming Snapshot.
    {

        struct PersistentDriftPlugin {
            options: Mutex<[&'static str; 2]>,
            editor_rev: Mutex<u64>,
            editor_contents: Mutex<Vec<u8>>,
            executed: Arc<Mutex<Vec<String>>>,
            select_seen: Arc<tokio::sync::Notify>,
            cancel_calls: Mutex<usize>,
            down_seen: Arc<tokio::sync::Notify>,
            cancel1_entered: Arc<tokio::sync::Notify>,
            cancel1_release: Arc<tokio::sync::Notify>,
            cancel2_seen: Arc<tokio::sync::Notify>,
            rt: tokio::runtime::Handle,
        }

        impl PersistentDriftPlugin {
            fn make_snap(rev: u64, opts: [&str; 2], ed_rev: u64, contents: &[u8]) -> PanelSnapshot {
                PanelSnapshot {
                    revision: rev,
                    title: "persistent-drift".into(),
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
                }
            }
        }

        impl mio_plugin_sdk::MioPlugin for PersistentDriftPlugin {
            fn id(&self) -> &str {
                "lab.persistent-semantic-drift"
            }

            fn version(&self) -> &str {
                "1.5.0"
            }

            fn initial_panel(&self) -> PanelSnapshot {
                Self::make_snap(1, ["A", "B"], 1, b"init")
            }

            fn on_ui_event(
                &mut self,
                ctx: &mut dyn mio_plugin_sdk::HostClient,
                event: UiEvent,
            ) -> std::result::Result<Option<PanelSnapshot>, String> {
                match (event.node_id, &event.event) {
                    (2, Some(panel::ui_event::Event::Pointer(p))) => {
                        match panel::PointerPhase::try_from(p.phase).unwrap() {
                            panel::PointerPhase::Down => {
                                self.down_seen.notify_one();
                                Ok(None)
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
                                    let opts = *self.options.lock().unwrap();
                                    let ed_rev = *self.editor_rev.lock().unwrap();
                                    let contents = self.editor_contents.lock().unwrap().clone();
                                    Ok(Some(Self::make_snap(rev, opts, ed_rev, &contents)))
                                } else {
                                    self.cancel2_seen.notify_one();
                                    Ok(None)
                                }
                            }
                            panel::PointerPhase::Move => Ok(None),
                        }
                    }
                    (1, Some(panel::ui_event::Event::EditorChanged(ed))) => {
                        *self.editor_rev.lock().unwrap() = ed.revision;
                        *self.editor_contents.lock().unwrap() = ed.contents.clone();
                        let rev = ctx.next_panel_revision();
                        let opts = *self.options.lock().unwrap();
                        Ok(Some(Self::make_snap(rev, opts, ed.revision, &ed.contents)))
                    }
                    (3, Some(panel::ui_event::Event::SelectedOption(idx))) => {
                        let chosen = self.options.lock().unwrap()[*idx as usize].to_owned();
                        self.executed.lock().unwrap().push(chosen);
                        self.select_seen.notify_one();
                        Ok(None)
                    }
                    _ => Ok(None),
                }
            }
        }

        let rt_handle = tokio::runtime::Handle::current();
        let identity = crate::plugins::tests_runtime::identity();
        let manifest = PluginManifest {
            id: "lab.persistent-semantic-drift".into(),
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
        let select_seen = Arc::new(tokio::sync::Notify::new());
        let down_seen = Arc::new(tokio::sync::Notify::new());
        let cancel1_entered = Arc::new(tokio::sync::Notify::new());
        let cancel1_release = Arc::new(tokio::sync::Notify::new());
        let cancel2_seen = Arc::new(tokio::sync::Notify::new());

        let executed_w = executed.clone();
        let select_seen_w = select_seen.clone();
        let down_seen_w = down_seen.clone();
        let cancel1_entered_w = cancel1_entered.clone();
        let cancel1_release_w = cancel1_release.clone();
        let cancel2_seen_w = cancel2_seen.clone();
        let worker_thread = std::thread::spawn(move || {
            let mut plugin = PersistentDriftPlugin {
                options: Mutex::new(["A", "B"]),
                editor_rev: Mutex::new(1),
                editor_contents: Mutex::new(b"init".to_vec()),
                executed: executed_w,
                select_seen: select_seen_w,
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
        timeout(MAX_OPERATION, cancel1_entered.notified())
            .await
            .unwrap();

        // Deferred SelectedOption(0) queued while Pointer(Cancel) barrier is active.
        handle
            .try_event(UiEvent {
                node_id: 3,
                editors: vec![],
                event: Some(panel::ui_event::Event::SelectedOption(0)),
            })
            .unwrap();

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

        // Resync cycle ended, yet subsequent & repeated ordinary SelectedOption(0) events MUST still fail locally as Stale!
        for _ in 0..2 {
            handle
                .try_event(UiEvent {
                    node_id: 3,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::SelectedOption(0)),
                })
                .unwrap();
            let err_ev = timeout(MAX_OPERATION, async {
                loop {
                    match supervisor.events.recv().await {
                        Some(PluginEvent::PanelSynced { .. }) => continue,
                        other => return other,
                    }
                }
            })
            .await
            .unwrap()
            .unwrap();
            assert!(
                matches!(
                    err_ev,
                    PluginEvent::Error { ref failure, .. }
                        if failure.code == ErrorCode::Stale as i32
                            && failure.message.contains("尚未通过最新快照确认状态")
                ),
                "expected Stale drift rejection after recovery ended, got {err_ev:?}"
            );
        }
        assert!(executed.lock().unwrap().is_empty());

        // EditorChanged still proceeds and returns a confirming Snapshot(["B", "A"]) which clears drift.
        handle
            .try_event(UiEvent {
                node_id: 1,
                editors: vec![],
                event: Some(panel::ui_event::Event::EditorChanged(panel::EditorSnapshot {
                    document_id: 1,
                    revision: 2,
                    contents: b"confirmed".to_vec(),
                    selection_start: 0,
                    selection_end: 0,
                })),
            })
            .unwrap();
        timeout(MAX_OPERATION, async {
            loop {
                match supervisor.events.recv().await {
                    Some(PluginEvent::Panel { .. }) => break,
                    Some(PluginEvent::PanelSynced { .. }) => continue,
                    Some(other) => panic!("expected confirmed panel after EditorChanged, got {other:?}"),
                    None => panic!("events closed unexpectedly"),
                }
            }
        })
        .await
        .unwrap();

        handle
            .try_event(UiEvent {
                node_id: 3,
                editors: vec![],
                event: Some(panel::ui_event::Event::SelectedOption(0)),
            })
            .unwrap();
        timeout(MAX_OPERATION, select_seen.notified()).await.unwrap();
        assert_eq!(*executed.lock().unwrap(), vec!["B".to_string()]);

        supervisor.stop().await.unwrap();
        worker_thread.join().unwrap();
    }

    // Part B: Unit verification that partial Body::Patch leaves semantic drift active while full Body::Snapshot clears it.
    {
        let (mut running, _calls, mut updates) = running();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        let make_snap = |rev: u64, ed_rev: u64, contents: &[u8], opts: [&str; 2]| PanelSnapshot {
            revision: rev,
            title: format!("unit-drift-{rev}"),
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
        *running.main_panel_mut() = PanelState::new(make_snap(1, 1, b"init", ["alpha", "beta"])).unwrap();

        running.schedule_panel_semantic_resync(protocol::PANEL_ID);
        running.flush_pending_resyncs(&outgoing).unwrap();
        let resync = wire.try_recv().unwrap();

        let mut bare_ack = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        );
        bare_ack.in_reply_to = resync.request_id;
        running.frame(bare_ack, &outgoing).await.unwrap();
        assert_eq!(running.main_slot().resync_needed_seq, 0);
        assert!(running.main_slot().resync_semantic_drift);
        while updates.try_recv().is_ok() {}

        let select_ev = Control::Event {
            panel_id: protocol::PANEL_ID,
            expected_snapshot: None,
            event: UiEvent {
                node_id: 3,
                editors: vec![],
                event: Some(panel::ui_event::Event::SelectedOption(0)),
            },
            event_seq: 0,
        };
        running.control(select_ev, &outgoing).await.unwrap();
        assert!(wire.try_recv().is_err());
        assert!(matches!(
            updates.try_recv(),
            Ok(PluginEvent::Error { ref failure, .. }) if failure.code == ErrorCode::Stale as i32
        ));

        // Partial Body::Patch updates only the Editor -> MUST NOT clear semantic drift on Select!
        running
            .frame(
                Envelope::new(
                    running.identity,
                    2,
                    2,
                    envelope::Body::Patch(panel::PanelPatch {
                        base_revision: 1,
                        revision: 2,
                        operations: vec![panel::PatchOperation {
                            node_id: 1,
                            change: Some(panel::patch_operation::Change::Editor(
                                panel::EditorUpdate {
                                    base_revision: 1,
                                    snapshot: Some(panel::EditorSnapshot {
                                        document_id: 1,
                                        revision: 2,
                                        contents: b"patched".to_vec(),
                                        selection_start: 0,
                                        selection_end: 0,
                                    }),
                                },
                            )),
                        }],
                    }),
                    MAX_OPERATION,
                ),
                &outgoing,
            )
            .await
            .unwrap();
        assert_eq!(running.panel_revision(protocol::PANEL_ID), 2);
        assert!(running.main_slot().resync_semantic_drift);
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

        // Full confirming Body::Snapshot clears semantic drift and re-enables Select interaction.
        running
            .frame(
                Envelope::new(
                    running.identity,
                    3,
                    3,
                    envelope::Body::Snapshot(make_snap(3, 2, b"patched", ["beta", "alpha"])),
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
        let dispatched = wire.try_recv().unwrap();
        assert!(matches!(
            dispatched.body,
            Some(envelope::Body::UiEvent(ref ev))
                if matches!(ev.event, Some(panel::ui_event::Event::SelectedOption(0)))
        ));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn marking_semantic_drift_atomically_retracts_unclaimed_in_writes_events_and_commits_before_resync()
{
    let make_snap = |rev: u64, ed_rev: u64, contents: &[u8], opts: [&str; 2]| PanelSnapshot {
        revision: rev,
        title: format!("retract-drift-{rev}"),
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

        let res = Uuid::new_v4();
        let (o_tx, o_rx) = oneshot::channel();
        running
            .control(
                Control::Open {
                    panel_id: target_panel,
                    id: res,
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
            envelope::Body::Snapshot(make_snap(init_rev, 2, b"newer-host", ["A", "B"])),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        o_snap.in_reply_to = o_env.request_id;
        running.frame(o_snap, &outgoing).await.unwrap();
        o_rx.await.unwrap().unwrap();

        // Event A is claimed & written to worker_rx.
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

        // Saturate 256-byte duplex buffer so writer blocks in write_all on HostResult.
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

        // Without any active barrier, B1 (SelectedOption(0)), B2 (CommitEditor), and C (EditorChanged)
        // enter `writes` unclaimed.
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
        let (cm_tx, mut cm_rx) = oneshot::channel();
        running
            .control(
                Control::Commit {
                    panel_id: target_panel,
                    op_id: Uuid::new_v4(),
                    request: CommitEditor {
                        resource_id: res.as_bytes().to_vec(),
                        message_revision: 1,
                        request: Some(request().into()),
                        editor: Some(EditorSnapshot {
                            document_id: 1,
                            revision: 2,
                            contents: b"newer-host".to_vec(),
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
        while updates.try_recv().is_ok() {}

        // Event A replies with a reordered Snapshot rejected by PanelState::replace -> resync_panel ->
        // mark_panel_semantic_drift MUST retract B1 & B2 in `writes` BEFORE emitting Resync!
        let stale_rev = init_rev + 1;
        let mut stale_ev_a_snap = Envelope::new(
            running.identity,
            2,
            stale_rev,
            envelope::Body::Snapshot(make_snap(stale_rev, 1, b"stale-ed", ["B", "A"])),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        stale_ev_a_snap.in_reply_to = ev_a.request_id;
        running.frame(stale_ev_a_snap, &outgoing).await.unwrap();

        assert!(cm_rx.try_recv().unwrap().is_err());
        assert_eq!(running.requests.len(), 2); // Only EditorChanged + Resync remain!

        let reader_task = tokio::spawn(async move {
            let mut saw_editor_changed = false;
            let mut saw_resync = false;
            while !(saw_editor_changed && saw_resync) {
                let env = read_async_frame(&mut worker_rx).await.unwrap();
                match env.body {
                    Some(envelope::Body::HostResult(_)) => {}
                    Some(envelope::Body::UiEvent(ref ev))
                        if matches!(ev.event, Some(panel::ui_event::Event::EditorChanged(_))) =>
                    {
                        saw_editor_changed = true;
                    }
                    Some(envelope::Body::Resync(_)) => {
                        saw_resync = true;
                    }
                    Some(other) => {
                        panic!("retracted interaction/commit leaked before Resync: {other:?}")
                    }
                    None => {}
                }
            }
        });
        timeout(MAX_OPERATION, reader_task).await.unwrap().unwrap();

        running.prepare_shutdown(&outgoing);
        io_cancel.cancel();
        io_tasks.abort_all();
    }
}

#[tokio::test]
async fn semantic_drift_blocks_deferred_new_and_inflight_commit_and_commit_response_until_snapshot()
{
    let make_snap = |rev: u64| PanelSnapshot {
        revision: rev,
        title: format!("commit-drift-{rev}"),
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
    let make_resp = || mio_plugin_api::HttpResponse {
        status: 200,
        version: mio_plugin_api::HttpVersion::Http11,
        reason_phrase: Some("OK".into()),
        headers: vec![],
        body: b"resp".to_vec(),
        trailers: vec![],
    };

    // Subcase A: Deferred and new Commit/CommitResponse stay blocked across bare Resync Ack until confirming Snapshot.
    {
        let (mut running, _calls, _updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        let res2 = Uuid::new_v4();
        let (o2_tx, o2_rx) = oneshot::channel();
        running
            .control(
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
        let o2_wire = wire.try_recv().unwrap();
        let mut o2_snap = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Snapshot(make_snap(1)),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        o2_snap.in_reply_to = o2_wire.request_id;
        running.frame(o2_snap, &outgoing).await.unwrap();
        o2_rx.await.unwrap().unwrap();

        running.schedule_panel_semantic_resync(2);
        running.flush_pending_resyncs(&outgoing).unwrap();
        let resync = wire.try_recv().unwrap();

        let (def_cm_tx, mut def_cm_rx) = oneshot::channel();
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
                    reply: def_cm_tx,
                                    expected_snapshot: None,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let (def_rcm_tx, mut def_rcm_rx) = oneshot::channel();
        running
            .control(
                Control::CommitResponse {
                    panel_id: 2,
                    op_id: Uuid::new_v4(),
                    request: protocol::CommitResponseEditor {
                        resource_id: res2.as_bytes().to_vec(),
                        message_revision: 1,
                        response: Some(make_resp().into()),
                        editor: Some(EditorSnapshot {
                            document_id: 1,
                            revision: 1,
                            contents: b"init".to_vec(),
                            ..Default::default()
                        }),
                    },
                    cancelled: CancellationToken::new(),
                    reply: def_rcm_tx,
                                    expected_snapshot: None,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert_eq!(running.panels[&2].deferred_commands.len(), 2);

        let mut bare_ack = Envelope::new(
            running.identity,
            2,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        bare_ack.in_reply_to = resync.request_id;
        running.frame(bare_ack, &outgoing).await.unwrap();
        assert!(def_cm_rx.try_recv().unwrap().is_err());
        assert!(def_rcm_rx.try_recv().unwrap().is_err());
        assert!(wire.try_recv().is_err());

        // New Commit / CommitResponse after bare Ack must also be blocked without writing to wire!
        let (new_cm_tx, mut new_cm_rx) = oneshot::channel();
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
                    reply: new_cm_tx,
                                    expected_snapshot: None,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let (new_rcm_tx, mut new_rcm_rx) = oneshot::channel();
        running
            .control(
                Control::CommitResponse {
                    panel_id: 2,
                    op_id: Uuid::new_v4(),
                    request: protocol::CommitResponseEditor {
                        resource_id: res2.as_bytes().to_vec(),
                        message_revision: 1,
                        response: Some(make_resp().into()),
                        editor: Some(EditorSnapshot {
                            document_id: 1,
                            revision: 1,
                            contents: b"init".to_vec(),
                            ..Default::default()
                        }),
                    },
                    cancelled: CancellationToken::new(),
                    reply: new_rcm_tx,
                                    expected_snapshot: None,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert!(new_cm_rx.try_recv().unwrap().is_err());
        assert!(new_rcm_rx.try_recv().unwrap().is_err());
        assert!(wire.try_recv().is_err());
    }

    // Subcase B & C: In-flight Committed/ResponseCommitted fail when drifted or when envelope.revision > host_rev,
    // and full confirming Snapshot restores Commit/CommitResponse execution.
    {
        let (mut running, _calls, _updates) = running();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        let res = Uuid::new_v4();
        running.broker.bind_resource(res, 1, false).unwrap();
        running.main_slot_mut().active_resource = Some((res, 1));
        *running.main_panel_mut() = PanelState::new(make_snap(1)).unwrap();

        let (c1_tx, mut c1_rx) = oneshot::channel();
        running
            .control(
                Control::Commit {
                    panel_id: protocol::PANEL_ID,
                    op_id: Uuid::new_v4(),
                    request: CommitEditor {
                        resource_id: res.as_bytes().to_vec(),
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
                    reply: c1_tx,
                                    expected_snapshot: None,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let c1_wire = wire.try_recv().unwrap();

        let (rc1_tx, mut rc1_rx) = oneshot::channel();
        running
            .control(
                Control::CommitResponse {
                    panel_id: protocol::PANEL_ID,
                    op_id: Uuid::new_v4(),
                    request: protocol::CommitResponseEditor {
                        resource_id: res.as_bytes().to_vec(),
                        message_revision: 1,
                        response: Some(make_resp().into()),
                        editor: Some(EditorSnapshot {
                            document_id: 1,
                            revision: 1,
                            contents: b"init".to_vec(),
                            ..Default::default()
                        }),
                    },
                    cancelled: CancellationToken::new(),
                    reply: rc1_tx,
                                    expected_snapshot: None,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let rc1_wire = wire.try_recv().unwrap();

        // Mark semantic drift while c1 & rc1 are in flight -> replies MUST fail!
        running.schedule_panel_semantic_resync(protocol::PANEL_ID);
        running.flush_pending_resyncs(&outgoing).unwrap();
        let resync1 = wire.try_recv().unwrap();

        let mut c1_reply = Envelope::new(
            running.identity,
            1,
            1,
            envelope::Body::Committed(protocol::Committed {
                resource_id: res.as_bytes().to_vec(),
                message_revision: 1,
                document_id: 1,
                document_revision: 1,
                request: Some(request().into()),
            }),
            MAX_OPERATION,
        );
        c1_reply.in_reply_to = c1_wire.request_id;
        running.frame(c1_reply, &outgoing).await.unwrap();
        assert!(c1_rx.try_recv().unwrap().is_err());

        let mut rc1_reply = Envelope::new(
            running.identity,
            2,
            1,
            envelope::Body::ResponseCommitted(protocol::ResponseCommitted {
                resource_id: res.as_bytes().to_vec(),
                message_revision: 1,
                document_id: 1,
                document_revision: 1,
                response: Some(make_resp().into()),
            }),
            MAX_OPERATION,
        );
        rc1_reply.in_reply_to = rc1_wire.request_id;
        running.frame(rc1_reply, &outgoing).await.unwrap();
        assert!(rc1_rx.try_recv().unwrap().is_err());

        // Confirm resync1 with Snapshot(rev=2) -> clears semantic drift!
        let mut ok_snap = Envelope::new(
            running.identity,
            3,
            2,
            envelope::Body::Snapshot(make_snap(2)),
            MAX_OPERATION,
        );
        ok_snap.in_reply_to = resync1.request_id;
        running.frame(ok_snap, &outgoing).await.unwrap();
        assert!(!running.main_slot().resync_semantic_drift);

        // Even with no prior drift flag, Committed/ResponseCommitted with envelope.revision > host_rev
        // MUST fail AND mark semantic drift + schedule Resync(2)!
        let (c2_tx, mut c2_rx) = oneshot::channel();
        running
            .control(
                Control::Commit {
                    panel_id: protocol::PANEL_ID,
                    op_id: Uuid::new_v4(),
                    request: CommitEditor {
                        resource_id: res.as_bytes().to_vec(),
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

        let (rc2_tx, mut rc2_rx) = oneshot::channel();
        running
            .control(
                Control::CommitResponse {
                    panel_id: protocol::PANEL_ID,
                    op_id: Uuid::new_v4(),
                    request: protocol::CommitResponseEditor {
                        resource_id: res.as_bytes().to_vec(),
                        message_revision: 1,
                        response: Some(make_resp().into()),
                        editor: Some(EditorSnapshot {
                            document_id: 1,
                            revision: 1,
                            contents: b"init".to_vec(),
                            ..Default::default()
                        }),
                    },
                    cancelled: CancellationToken::new(),
                    reply: rc2_tx,
                                    expected_snapshot: None,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let rc2_wire = wire.try_recv().unwrap();

        let mut c2_drifted = Envelope::new(
            running.identity,
            4,
            3,
            envelope::Body::Committed(protocol::Committed {
                resource_id: res.as_bytes().to_vec(),
                message_revision: 1,
                document_id: 1,
                document_revision: 1,
                request: Some(request().into()),
            }),
            MAX_OPERATION,
        );
        c2_drifted.in_reply_to = c2_wire.request_id;
        running.frame(c2_drifted, &outgoing).await.unwrap();
        assert!(c2_rx.try_recv().unwrap().is_err());
        assert!(running.main_slot().resync_semantic_drift);
        let resync2 = wire.try_recv().unwrap();
        assert!(matches!(resync2.body, Some(envelope::Body::Resync(ref s)) if s.revision == 2));

        let mut rc2_drifted = Envelope::new(
            running.identity,
            5,
            3,
            envelope::Body::ResponseCommitted(protocol::ResponseCommitted {
                resource_id: res.as_bytes().to_vec(),
                message_revision: 1,
                document_id: 1,
                document_revision: 1,
                response: Some(make_resp().into()),
            }),
            MAX_OPERATION,
        );
        rc2_drifted.in_reply_to = rc2_wire.request_id;
        running.frame(rc2_drifted, &outgoing).await.unwrap();
        assert!(rc2_rx.try_recv().unwrap().is_err());

        // Confirm resync2 with Snapshot(rev=4) -> Commit and CommitResponse succeed at rev=4!
        let mut ok_snap2 = Envelope::new(
            running.identity,
            6,
            4,
            envelope::Body::Snapshot(make_snap(4)),
            MAX_OPERATION,
        );
        ok_snap2.in_reply_to = resync2.request_id;
        running.frame(ok_snap2, &outgoing).await.unwrap();
        assert!(!running.main_slot().resync_semantic_drift);

        let (ok_cm_tx, ok_cm_rx) = oneshot::channel();
        running
            .control(
                Control::Commit {
                    panel_id: protocol::PANEL_ID,
                    op_id: Uuid::new_v4(),
                    request: CommitEditor {
                        resource_id: res.as_bytes().to_vec(),
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
                    reply: ok_cm_tx,
                                    expected_snapshot: None,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let ok_cm_wire = wire.try_recv().unwrap();
        let mut ok_cm_reply = Envelope::new(
            running.identity,
            7,
            4,
            envelope::Body::Committed(protocol::Committed {
                resource_id: res.as_bytes().to_vec(),
                message_revision: 1,
                document_id: 1,
                document_revision: 1,
                request: Some(request().into()),
            }),
            MAX_OPERATION,
        );
        ok_cm_reply.in_reply_to = ok_cm_wire.request_id;
        running.frame(ok_cm_reply, &outgoing).await.unwrap();
        assert_eq!(ok_cm_rx.await.unwrap().unwrap().url, request().url);

        let (ok_rcm_tx, ok_rcm_rx) = oneshot::channel();
        running
            .control(
                Control::CommitResponse {
                    panel_id: protocol::PANEL_ID,
                    op_id: Uuid::new_v4(),
                    request: protocol::CommitResponseEditor {
                        resource_id: res.as_bytes().to_vec(),
                        message_revision: 1,
                        response: Some(make_resp().into()),
                        editor: Some(EditorSnapshot {
                            document_id: 1,
                            revision: 1,
                            contents: b"init".to_vec(),
                            ..Default::default()
                        }),
                    },
                    cancelled: CancellationToken::new(),
                    reply: ok_rcm_tx,
                                    expected_snapshot: None,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let ok_rcm_wire = wire.try_recv().unwrap();
        let mut ok_rcm_reply = Envelope::new(
            running.identity,
            8,
            4,
            envelope::Body::ResponseCommitted(protocol::ResponseCommitted {
                resource_id: res.as_bytes().to_vec(),
                message_revision: 1,
                document_id: 1,
                document_revision: 1,
                response: Some(make_resp().into()),
            }),
            MAX_OPERATION,
        );
        ok_rcm_reply.in_reply_to = ok_rcm_wire.request_id;
        running.frame(ok_rcm_reply, &outgoing).await.unwrap();
        assert_eq!(ok_rcm_rx.await.unwrap().unwrap().status, 200);
    }
}

#[tokio::test]
async fn timed_out_resync_late_snapshot_marks_semantic_drift_without_restarting_exhausted_budget()
{
    let make_snap = |rev: u64, ed_rev: u64, contents: &[u8], opts: [&str; 2]| PanelSnapshot {
        revision: rev,
        title: format!("resync-late-{rev}"),
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

    // Subcase A: Child panel normal timeout schedules non-semantic Resync; attempt #1 times out and
    // returns late Snapshot(rev=2, ["B", "A"]) after cancellation -> MUST latch resync_semantic_drift
    // so bare Ack(1) on attempt #2 fails deferred SelectedOption(0) and blocks subsequent ops.
    {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        let res2 = Uuid::new_v4();
        let (o_tx, o_rx) = oneshot::channel();
        running
            .control(
                Control::Open {
                    panel_id: 2,
                    id: res2,
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
            1,
            1,
            envelope::Body::Snapshot(make_snap(1, 1, b"init", ["A", "B"])),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        o_snap.in_reply_to = o_wire.request_id;
        running.frame(o_snap, &outgoing).await.unwrap();
        o_rx.await.unwrap().unwrap();

        running
            .control(
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
        let ev1 = wire.try_recv().unwrap();
        running.requests.get_mut(&ev1.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let cancel_ev1 = wire.try_recv().unwrap();
        assert!(matches!(cancel_ev1.body, Some(envelope::Body::Cancel(_))));
        let resync1 = wire.try_recv().unwrap();
        assert!(matches!(resync1.body, Some(envelope::Body::Resync(ref s)) if s.revision == 1));
        assert!(!running.panels[&2].resync_semantic_drift);

        // Resync attempt #1 times out -> Resync attempt #2 sent!
        running.requests.get_mut(&resync1.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let cancel_resync1 = wire.try_recv().unwrap();
        assert!(matches!(cancel_resync1.body, Some(envelope::Body::Cancel(_))));
        let resync2 = wire.try_recv().unwrap();
        assert!(matches!(resync2.body, Some(envelope::Body::Resync(ref s)) if s.revision == 1));

        // Defer SelectedOption(0) while Resync #2 is active.
        running
            .control(
                Control::Event {
                    panel_id: 2,
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
        assert_eq!(running.panels[&2].deferred_commands.len(), 1);

        // Late reordered Snapshot replying to timed-out Resync #1 arrives!
        let mut late_resync1_snap = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, b"init", ["B", "A"])),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_resync1_snap.in_reply_to = resync1.request_id;
        running.frame(late_resync1_snap, &outgoing).await.unwrap();
        assert!(running.panels[&2].resync_semantic_drift);
        assert!(wire.try_recv().is_err());

        // Legacy Worker answers Resync #2 with bare Ack(1): barrier clears, deferred & new ops stay rejected!
        let mut bare_ack2 = Envelope::new(
            running.identity,
            3,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        bare_ack2.in_reply_to = resync2.request_id;
        running.frame(bare_ack2, &outgoing).await.unwrap();
        assert!(!running.is_panel_resyncing(2));
        assert!(running.panels[&2].resync_semantic_drift);
        assert!(running.panels[&2].deferred_commands.is_empty());
        assert!(wire.try_recv().is_err());
        while updates.try_recv().is_ok() {}

        running
            .control(
                Control::Event {
                    panel_id: 2,
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
            Ok(PluginEvent::Error { panel_id: 2, ref failure, .. })
                if failure.code == ErrorCode::Stale as i32
        ));

        let (cm_tx, mut cm_rx) = oneshot::channel();
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
                    reply: cm_tx,
                    expected_snapshot: None,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert!(cm_rx.try_recv().unwrap().is_err());
        assert!(wire.try_recv().is_err());
    }

    // Subcase B: Exhausting MAX_RESYNC_RETRIES on main & child panels (both CancelNotification and
    // expired-in-frame Resync BEFORE timer, plus late Snapshot after timer) latches semantic drift
    // WITHOUT restarting the exhausted Resync budget!
    for target_panel in [protocol::PANEL_ID, 2u64] {
        // B1: All MAX_RESYNC_RETRIES time out; late Snapshot replying to final Resync's CancelNotification
        //     OR late Snapshot replying to final Resync's request_id latches drift without restarting.
        for reply_to_cancel_frame in [false, true] {
            let (mut running, _calls, mut updates) = running();
            running.supports_panel_multiplexing = true;
            *running.main_panel_mut() = PanelState::new(make_snap(1, 1, b"init", ["A", "B"])).unwrap();
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
                let o_w = wire.try_recv().unwrap();
                let mut o_s = Envelope::new(
                    running.identity,
                    next_seq(),
                    1,
                    envelope::Body::Snapshot(make_snap(1, 1, b"init", ["A", "B"])),
                    MAX_OPERATION,
                )
                .with_panel_id(target_panel);
                o_s.in_reply_to = o_w.request_id;
                running.frame(o_s, &outgoing).await.unwrap();
                o_rx.await.unwrap().unwrap();
            }

            running.schedule_panel_resync(target_panel);
            running.flush_pending_resyncs(&outgoing).unwrap();
            let mut last_resync_id = 0u64;
            let mut last_cancel_id = 0u64;
            for _ in 0..MAX_RESYNC_RETRIES {
                let r = wire.try_recv().unwrap();
                last_resync_id = r.request_id;
                running.requests.get_mut(&r.request_id).unwrap().deadline =
                    Instant::now() - Duration::from_secs(1);
                running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
                let c = wire.try_recv().unwrap();
                last_cancel_id = c.request_id;
            }
            assert!(!running.is_panel_resyncing(target_panel));
            assert!(!running.is_panel_semantic_drifted(target_panel));
            assert!(wire.try_recv().is_err());

            let mut late_snap = Envelope::new(
                running.identity,
                next_seq(),
                2,
                envelope::Body::Snapshot(make_snap(2, 1, b"init", ["B", "A"])),
                MAX_OPERATION,
            )
            .with_panel_id(target_panel);
            late_snap.in_reply_to = if reply_to_cancel_frame {
                last_cancel_id
            } else {
                last_resync_id
            };
            running.frame(late_snap, &outgoing).await.unwrap();
            assert!(running.is_panel_semantic_drifted(target_panel));
            assert!(!running.is_panel_resyncing(target_panel));
            assert!(wire.try_recv().is_err());
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
            assert!(wire.try_recv().is_err());
            assert!(matches!(
                updates.try_recv(),
                Ok(PluginEvent::Error { panel_id, ref failure, .. })
                    if panel_id == target_panel && failure.code == ErrorCode::Stale as i32
            ));
        }

        // B2: Expired reply arriving BEFORE timer on final Resync attempt #3 also latches drift.
        {
            let (mut running, _calls, _updates) = running();
            running.supports_panel_multiplexing = true;
            *running.main_panel_mut() = PanelState::new(make_snap(1, 1, b"init", ["A", "B"])).unwrap();
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
                let o_w = wire.try_recv().unwrap();
                let mut o_s = Envelope::new(
                    running.identity,
                    next_seq(),
                    1,
                    envelope::Body::Snapshot(make_snap(1, 1, b"init", ["A", "B"])),
                    MAX_OPERATION,
                )
                .with_panel_id(target_panel);
                o_s.in_reply_to = o_w.request_id;
                running.frame(o_s, &outgoing).await.unwrap();
                o_rx.await.unwrap().unwrap();
            }

            running.schedule_panel_resync(target_panel);
            running.flush_pending_resyncs(&outgoing).unwrap();
            let mut current_resync = wire.try_recv().unwrap();
            for _ in 1..MAX_RESYNC_RETRIES {
                running.requests.get_mut(&current_resync.request_id).unwrap().deadline =
                    Instant::now() - Duration::from_secs(1);
                running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
                let _cancel = wire.try_recv().unwrap();
                current_resync = wire.try_recv().unwrap();
            }

            let mut expired_final_snap = Envelope::new(
                running.identity,
                next_seq(),
                2,
                envelope::Body::Snapshot(make_snap(2, 1, b"init", ["B", "A"])),
                MAX_OPERATION,
            )
            .with_panel_id(target_panel);
            expired_final_snap.deadline_unix_ms = 1;
            expired_final_snap.in_reply_to = current_resync.request_id;
            running.frame(expired_final_snap, &outgoing).await.unwrap();
            assert!(running.is_panel_semantic_drifted(target_panel));
            assert!(!running.is_panel_resyncing(target_panel));
        }
    }
}

#[tokio::test]
async fn commit_and_commit_response_reject_when_panel_options_change_across_deferral_inflight_or_ui_lag()
{
    let make_snap = |rev: u64, ed_rev: u64, ed_bytes: &[u8], opts: [&str; 2]| PanelSnapshot {
        revision: rev,
        title: "commit-sem".into(),
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
                            label: "Mode".into(),
                            options: vec![opts[0].into(), opts[1].into()],
                            selected_index: 0,
                            enabled: true,
                        }),
                    ),
                ],
            }),
        )),
    };
    let make_resp = || mio_plugin_api::HttpResponse {
        status: 200,
        version: mio_plugin_api::HttpVersion::Http11,
        reason_phrase: Some("OK".into()),
        headers: vec![],
        body: b"resp".to_vec(),
        trailers: vec![],
    };

    // Subcase A: Deferred Commit + CommitResponse across a confirming Snapshot that reorders Select
    // options (["A", "B"] -> ["B", "A"]) without changing resource or editor revision MUST fail!
    {
        let (mut running, _calls, _updates) = running();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        let res = Uuid::new_v4();
        running.broker.bind_resource(res, 1, false).unwrap();
        running.main_slot_mut().active_resource = Some((res, 1));
        *running.main_panel_mut() = PanelState::new(make_snap(1, 1, b"init", ["A", "B"])).unwrap();

        running.schedule_panel_resync(protocol::PANEL_ID);
        running.flush_pending_resyncs(&outgoing).unwrap();
        let resync = wire.try_recv().unwrap();

        let (def_cm_tx, mut def_cm_rx) = oneshot::channel();
        running
            .control_with_seq(
                1,
                Control::Commit {
                    panel_id: protocol::PANEL_ID,
                    op_id: Uuid::new_v4(),
                    request: CommitEditor {
                        resource_id: res.as_bytes().to_vec(),
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
                    reply: def_cm_tx,
                    expected_snapshot: None,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let (def_rcm_tx, mut def_rcm_rx) = oneshot::channel();
        running
            .control_with_seq(
                2,
                Control::CommitResponse {
                    panel_id: protocol::PANEL_ID,
                    op_id: Uuid::new_v4(),
                    request: protocol::CommitResponseEditor {
                        resource_id: res.as_bytes().to_vec(),
                        message_revision: 1,
                        response: Some(make_resp().into()),
                        editor: Some(EditorSnapshot {
                            document_id: 1,
                            revision: 1,
                            contents: b"init".to_vec(),
                            ..Default::default()
                        }),
                    },
                    cancelled: CancellationToken::new(),
                    reply: def_rcm_tx,
                    expected_snapshot: None,
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert_eq!(running.main_slot().deferred_commands.len(), 2);

        // Worker confirms Resync with Snapshot(rev=2) that keeps ed_rev=1/"init" & resource rev=1,
        // ONLY changing Select options ["A", "B"] -> ["B", "A"]!
        let mut changed_opts_snap = Envelope::new(
            running.identity,
            1,
            2,
            envelope::Body::Snapshot(make_snap(2, 1, b"init", ["B", "A"])),
            MAX_OPERATION,
        );
        changed_opts_snap.in_reply_to = resync.request_id;
        running.frame(changed_opts_snap, &outgoing).await.unwrap();

        assert!(def_cm_rx.try_recv().unwrap().is_err());
        assert!(def_rcm_rx.try_recv().unwrap().is_err());
        assert!(wire.try_recv().is_err());
    }

    // Subcase B: In-flight Commit + CommitResponse fail if concurrent Snapshot changes options before
    // Committed / ResponseCommitted arrives (even at matching latest envelope.revision == host_rev).
    {
        let (mut running, _calls, _updates) = running();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        let res = Uuid::new_v4();
        running.broker.bind_resource(res, 1, false).unwrap();
        running.main_slot_mut().active_resource = Some((res, 1));
        *running.main_panel_mut() = PanelState::new(make_snap(1, 1, b"init", ["A", "B"])).unwrap();

        let (cm_tx, mut cm_rx) = oneshot::channel();
        running
            .control(
                Control::Commit {
                    panel_id: protocol::PANEL_ID,
                    op_id: Uuid::new_v4(),
                    request: CommitEditor {
                        resource_id: res.as_bytes().to_vec(),
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
                    reply: cm_tx,
                    expected_snapshot: None,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let cm_wire = wire.try_recv().unwrap();

        let (rcm_tx, mut rcm_rx) = oneshot::channel();
        running
            .control(
                Control::CommitResponse {
                    panel_id: protocol::PANEL_ID,
                    op_id: Uuid::new_v4(),
                    request: protocol::CommitResponseEditor {
                        resource_id: res.as_bytes().to_vec(),
                        message_revision: 1,
                        response: Some(make_resp().into()),
                        editor: Some(EditorSnapshot {
                            document_id: 1,
                            revision: 1,
                            contents: b"init".to_vec(),
                            ..Default::default()
                        }),
                    },
                    cancelled: CancellationToken::new(),
                    reply: rcm_tx,
                    expected_snapshot: None,
                },
                &outgoing,
            )
            .await
            .unwrap();
        let rcm_wire = wire.try_recv().unwrap();

        // Unsolicited Snapshot rev=2 reorders Select options without touching resource or editor!
        running
            .frame(
                Envelope::new(
                    running.identity,
                    1,
                    2,
                    envelope::Body::Snapshot(make_snap(2, 1, b"init", ["B", "A"])),
                    MAX_OPERATION,
                ),
                &outgoing,
            )
            .await
            .unwrap();

        let mut cm_reply = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Committed(protocol::Committed {
                resource_id: res.as_bytes().to_vec(),
                message_revision: 1,
                document_id: 1,
                document_revision: 1,
                request: Some(request().into()),
            }),
            MAX_OPERATION,
        );
        cm_reply.in_reply_to = cm_wire.request_id;
        running.frame(cm_reply, &outgoing).await.unwrap();
        assert!(cm_rx.try_recv().unwrap().is_err());

        let mut rcm_reply = Envelope::new(
            running.identity,
            3,
            2,
            envelope::Body::ResponseCommitted(protocol::ResponseCommitted {
                resource_id: res.as_bytes().to_vec(),
                message_revision: 1,
                document_id: 1,
                document_revision: 1,
                response: Some(make_resp().into()),
            }),
            MAX_OPERATION,
        );
        rcm_reply.in_reply_to = rcm_wire.request_id;
        running.frame(rcm_reply, &outgoing).await.unwrap();
        assert!(rcm_rx.try_recv().unwrap().is_err());
    }

    // Subcase C: Positive control (confirming only committed Editor's own update lets Commit pass)
    // AND UI-lag protection (explicit older expected_snapshot rejects stale Commit).
    {
        let (mut running, _calls, _updates) = running();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        let res = Uuid::new_v4();
        running.broker.bind_resource(res, 1, false).unwrap();
        running.main_slot_mut().active_resource = Some((res, 1));
        let snap1 = Arc::new(make_snap(1, 1, b"init", ["A", "B"]));
        *running.main_panel_mut() = PanelState::new((*snap1).clone()).unwrap();

        // Confirming Snapshot rev=2 updates ONLY the committed Editor to (ed_rev=2, b"typed")
        // while Select options stay identical -> Commit captured at snap1 MUST succeed!
        running
            .frame(
                Envelope::new(
                    running.identity,
                    1,
                    2,
                    envelope::Body::Snapshot(make_snap(2, 2, b"typed", ["A", "B"])),
                    MAX_OPERATION,
                ),
                &outgoing,
            )
            .await
            .unwrap();
        let (ok_cm_tx, ok_cm_rx) = oneshot::channel();
        running
            .control(
                Control::Commit {
                    panel_id: protocol::PANEL_ID,
                    op_id: Uuid::new_v4(),
                    request: CommitEditor {
                        resource_id: res.as_bytes().to_vec(),
                        message_revision: 1,
                        request: Some(request().into()),
                        editor: Some(EditorSnapshot {
                            document_id: 1,
                            revision: 2,
                            contents: b"typed".to_vec(),
                            ..Default::default()
                        }),
                    },
                    cancelled: CancellationToken::new(),
                    reply: ok_cm_tx,
                    expected_snapshot: Some(snap1.clone()),
                },
                &outgoing,
            )
            .await
            .unwrap();
        let ok_cm_wire = wire.try_recv().unwrap();
        let mut ok_cm_reply = Envelope::new(
            running.identity,
            2,
            2,
            envelope::Body::Committed(protocol::Committed {
                resource_id: res.as_bytes().to_vec(),
                message_revision: 1,
                document_id: 1,
                document_revision: 2,
                request: Some(request().into()),
            }),
            MAX_OPERATION,
        );
        ok_cm_reply.in_reply_to = ok_cm_wire.request_id;
        running.frame(ok_cm_reply, &outgoing).await.unwrap();
        assert_eq!(ok_cm_rx.await.unwrap().unwrap().url, request().url);

        // UI lag: Snapshot rev=3 changes Select options ["A", "B"] -> ["B", "A"];
        // Commit stamped with older snap1 MUST be rejected immediately!
        running
            .frame(
                Envelope::new(
                    running.identity,
                    3,
                    3,
                    envelope::Body::Snapshot(make_snap(3, 2, b"typed", ["B", "A"])),
                    MAX_OPERATION,
                ),
                &outgoing,
            )
            .await
            .unwrap();
        let (lag_cm_tx, mut lag_cm_rx) = oneshot::channel();
        running
            .control(
                Control::Commit {
                    panel_id: protocol::PANEL_ID,
                    op_id: Uuid::new_v4(),
                    request: CommitEditor {
                        resource_id: res.as_bytes().to_vec(),
                        message_revision: 1,
                        request: Some(request().into()),
                        editor: Some(EditorSnapshot {
                            document_id: 1,
                            revision: 2,
                            contents: b"typed".to_vec(),
                            ..Default::default()
                        }),
                    },
                    cancelled: CancellationToken::new(),
                    reply: lag_cm_tx,
                    expected_snapshot: Some(snap1),
                },
                &outgoing,
            )
            .await
            .unwrap();
        assert!(lag_cm_rx.try_recv().unwrap().is_err());
        assert!(wire.try_recv().is_err());
    }
}

#[tokio::test]
async fn same_revision_optimistic_option_change_rejects_stale_deferred_new_and_inflight_commits() {
    let make_snap = |rev: u64, ed_rev: u64, checked: bool, selected_index: u32| PanelSnapshot {
        revision: rev,
        title: "same-rev-commit".into(),
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
                            contents: b"init".to_vec(),
                            format: panel::EditorFormat::Utf8 as i32,
                            read_only: false,
                        }),
                    ),
                    panel::Node::new(
                        2,
                        "chk",
                        panel::node::Kind::Checkbox(panel::CheckboxNode {
                            label: "Sign".into(),
                            checked,
                            enabled: true,
                        }),
                    ),
                    panel::Node::new(
                        3,
                        "select",
                        panel::node::Kind::Select(panel::SelectNode {
                            label: "Mode".into(),
                            options: vec!["A".into(), "B".into()],
                            selected_index,
                            enabled: true,
                        }),
                    ),
                ],
            }),
        )),
    };
    let make_resp = || mio_plugin_api::HttpResponse {
        status: 200,
        version: mio_plugin_api::HttpVersion::Http11,
        reason_phrase: Some("OK".into()),
        headers: vec![],
        body: b"resp".to_vec(),
        trailers: vec![],
    };

    let (mut running, _calls, _updates) = running();
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
    let res = Uuid::new_v4();
    running.broker.bind_resource(res, 1, false).unwrap();
    running.main_slot_mut().active_resource = Some((res, 1));
    let snap_old = Arc::new(make_snap(1, 1, false, 0));
    *running.main_panel_mut() = PanelState::new((*snap_old).clone()).unwrap();

    // Subcase A: Within the SAME panel revision (rev=1), optimistic Toggled(true) changes UI options
    // without incrementing panel.revision -> older snap_old Commit & CommitResponse MUST reject!
    running
        .control(
            Control::Event {
                panel_id: protocol::PANEL_ID,
                expected_snapshot: None,
                event: UiEvent {
                    node_id: 2,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::Toggled(true)),
                },
                event_seq: 0,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let _toggle_wire = wire.try_recv().unwrap();
    assert_eq!(running.panel_revision(protocol::PANEL_ID), 1);

    let (stale_cm_tx, mut stale_cm_rx) = oneshot::channel();
    running
        .control(
            Control::Commit {
                panel_id: protocol::PANEL_ID,
                op_id: Uuid::new_v4(),
                request: CommitEditor {
                    resource_id: res.as_bytes().to_vec(),
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
                reply: stale_cm_tx,
                expected_snapshot: Some(snap_old.clone()),
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(stale_cm_rx.try_recv().unwrap().is_err());

    let (stale_rcm_tx, mut stale_rcm_rx) = oneshot::channel();
    running
        .control(
            Control::CommitResponse {
                panel_id: protocol::PANEL_ID,
                op_id: Uuid::new_v4(),
                request: protocol::CommitResponseEditor {
                    resource_id: res.as_bytes().to_vec(),
                    message_revision: 1,
                    response: Some(make_resp().into()),
                    editor: Some(EditorSnapshot {
                        document_id: 1,
                        revision: 1,
                        contents: b"init".to_vec(),
                        ..Default::default()
                    }),
                },
                cancelled: CancellationToken::new(),
                reply: stale_rcm_tx,
                expected_snapshot: Some(snap_old),
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(stale_rcm_rx.try_recv().unwrap().is_err());
    assert!(wire.try_recv().is_err());

    // Subcase B: Fresh Commit & CommitResponse at current (checked=true, selected=0, rev=1),
    // then same-revision SelectedOption(1) occurs while both are in flight -> replies at rev=1 MUST reject!
    let (cm_tx, mut cm_rx) = oneshot::channel();
    running
        .control(
            Control::Commit {
                panel_id: protocol::PANEL_ID,
                op_id: Uuid::new_v4(),
                request: CommitEditor {
                    resource_id: res.as_bytes().to_vec(),
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
                reply: cm_tx,
                expected_snapshot: None,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let cm_wire = wire.try_recv().unwrap();

    let (rcm_tx, mut rcm_rx) = oneshot::channel();
    running
        .control(
            Control::CommitResponse {
                panel_id: protocol::PANEL_ID,
                op_id: Uuid::new_v4(),
                request: protocol::CommitResponseEditor {
                    resource_id: res.as_bytes().to_vec(),
                    message_revision: 1,
                    response: Some(make_resp().into()),
                    editor: Some(EditorSnapshot {
                        document_id: 1,
                        revision: 1,
                        contents: b"init".to_vec(),
                        ..Default::default()
                    }),
                },
                cancelled: CancellationToken::new(),
                reply: rcm_tx,
                expected_snapshot: None,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let rcm_wire = wire.try_recv().unwrap();

    running
        .control(
            Control::Event {
                panel_id: protocol::PANEL_ID,
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
    let _select_wire = wire.try_recv().unwrap();
    assert_eq!(running.panel_revision(protocol::PANEL_ID), 1);

    let mut cm_reply = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Committed(protocol::Committed {
            resource_id: res.as_bytes().to_vec(),
            message_revision: 1,
            document_id: 1,
            document_revision: 1,
            request: Some(request().into()),
        }),
        MAX_OPERATION,
    );
    cm_reply.in_reply_to = cm_wire.request_id;
    running.frame(cm_reply, &outgoing).await.unwrap();
    assert!(cm_rx.try_recv().unwrap().is_err());

    let mut rcm_reply = Envelope::new(
        running.identity,
        2,
        1,
        envelope::Body::ResponseCommitted(protocol::ResponseCommitted {
            resource_id: res.as_bytes().to_vec(),
            message_revision: 1,
            document_id: 1,
            document_revision: 1,
            response: Some(make_resp().into()),
        }),
        MAX_OPERATION,
    );
    rcm_reply.in_reply_to = rcm_wire.request_id;
    running.frame(rcm_reply, &outgoing).await.unwrap();
    assert!(rcm_rx.try_recv().unwrap().is_err());
}

#[tokio::test]
async fn callback_error_on_optimistic_event_marks_drift_and_blocks_commit_until_snapshot() {
    let make_snap = |rev: u64, ed_rev: u64, ed_bytes: &[u8], selected: u32| PanelSnapshot {
        revision: rev,
        title: "cb-error-drift".into(),
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

    for target_panel in [protocol::PANEL_ID, 2u64] {
        let (mut running, _calls, mut updates) = running();
        running.supports_panel_multiplexing = true;
        *running.main_panel_mut() = PanelState::new(make_snap(1, 1, b"retained", 0)).unwrap();
        let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
        let res_id = Uuid::new_v4();
        if target_panel == protocol::PANEL_ID {
            running.broker.bind_resource(res_id, 1, false).unwrap();
            running.main_slot_mut().active_resource = Some((res_id, 1));
        } else {
            let (o_tx, o_rx) = oneshot::channel();
            running
                .control(
                    Control::Open {
                        panel_id: 2,
                        id: res_id,
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
                envelope::Body::Snapshot(make_snap(1, 1, b"retained", 0)),
                MAX_OPERATION,
            )
            .with_panel_id(2);
            o_s.in_reply_to = o_w.request_id;
            running.frame(o_s, &outgoing).await.unwrap();
            o_rx.await.unwrap().unwrap();
        }
        while updates.try_recv().is_ok() {}

        running
            .control(
                Control::Event {
                    panel_id: target_panel,
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
        let ev_w = wire.try_recv().unwrap();

        // Worker callback mutated Select=1 and then returned Invalid Error at same rev=1.
        let mut cb_err = Envelope::new(
            running.identity,
            2,
            1,
            envelope::Body::Error(Failure::new(ErrorCode::Invalid, "callback failed after partial mutation")),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        cb_err.in_reply_to = ev_w.request_id;
        running.frame(cb_err, &outgoing).await.unwrap();

        assert!(
            running.is_panel_semantic_drifted(target_panel),
            "non-Deadline callback Error on optimistic event must mark semantic drift"
        );
        let resync_w = wire.try_recv().unwrap();
        assert!(matches!(resync_w.body, Some(envelope::Body::Resync(_))));

        // Commits submitted while Resync is in flight defer behind the barrier and MUST reject if Resync only ACKs.
        let mut deferred_rxs = Vec::new();
        for exp_sel in [0u32, 1u32] {
            let (cm_tx, mut cm_rx) = oneshot::channel();
            running
                .control(
                    Control::Commit {
                        panel_id: target_panel,
                        op_id: Uuid::new_v4(),
                        request: CommitEditor {
                            resource_id: res_id.as_bytes().to_vec(),
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
                        reply: cm_tx,
                        expected_snapshot: Some(Arc::new(make_snap(1, 1, b"retained", exp_sel))),
                    },
                    &outgoing,
                )
                .await
                .unwrap();
            assert!(matches!(cm_rx.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
            deferred_rxs.push(cm_rx);
        }
        assert!(wire.try_recv().is_err());

        let mut bare_resync_ack = Envelope::new(
            running.identity,
            3,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        bare_resync_ack.in_reply_to = resync_w.request_id;
        running.frame(bare_resync_ack, &outgoing).await.unwrap();
        for mut rx in deferred_rxs {
            assert!(rx.try_recv().unwrap().is_err());
        }
        assert!(running.is_panel_semantic_drifted(target_panel));
        assert!(!running.is_panel_resyncing(target_panel));

        // Fresh Commits after Resync ended with bare Ack MUST still reject immediately while drifted!
        for exp_sel in [0u32, 1u32] {
            let (cm_tx, mut cm_rx) = oneshot::channel();
            running
                .control(
                    Control::Commit {
                        panel_id: target_panel,
                        op_id: Uuid::new_v4(),
                        request: CommitEditor {
                            resource_id: res_id.as_bytes().to_vec(),
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
                        reply: cm_tx,
                        expected_snapshot: Some(Arc::new(make_snap(1, 1, b"retained", exp_sel))),
                    },
                    &outgoing,
                )
                .await
                .unwrap();
            assert!(cm_rx.try_recv().unwrap().is_err());
        }
        assert!(wire.try_recv().is_err());

        // EditorChanged -> confirming Snapshot(rev=2, B=1) clears drift and unlocks Commit(B=1).
        running
            .control(
                Control::Event {
                    panel_id: target_panel,
                    expected_snapshot: None,
                    event_seq: 2,
                    event: UiEvent {
                        node_id: 1,
                        editors: vec![],
                        event: Some(panel::ui_event::Event::EditorChanged(EditorSnapshot {
                            document_id: 1,
                            revision: 2,
                            contents: b"retained-2".to_vec(),
                            ..Default::default()
                        })),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let ed_w = wire.try_recv().unwrap();
        let mut snap2 = Envelope::new(
            running.identity,
            4,
            2,
            envelope::Body::Snapshot(make_snap(2, 2, b"retained-2", 1)),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        snap2.in_reply_to = ed_w.request_id;
        running.frame(snap2, &outgoing).await.unwrap();
        assert!(!running.is_panel_semantic_drifted(target_panel));
        assert!(!running.is_panel_resyncing(target_panel));

        let (ok_tx, ok_rx) = oneshot::channel();
        running
            .control(
                Control::Commit {
                    panel_id: target_panel,
                    op_id: Uuid::new_v4(),
                    request: CommitEditor {
                        resource_id: res_id.as_bytes().to_vec(),
                        message_revision: 1,
                        request: Some(request().into()),
                        editor: Some(EditorSnapshot {
                            document_id: 1,
                            revision: 2,
                            contents: b"retained-2".to_vec(),
                            ..Default::default()
                        }),
                    },
                    cancelled: CancellationToken::new(),
                    reply: ok_tx,
                    expected_snapshot: Some(Arc::new(make_snap(2, 2, b"retained-2", 1))),
                },
                &outgoing,
            )
            .await
            .unwrap();
        let cm_w = wire.try_recv().unwrap();
        let mut cm_reply = Envelope::new(
            running.identity,
            5,
            2,
            envelope::Body::Committed(protocol::Committed {
                resource_id: res_id.as_bytes().to_vec(),
                message_revision: 1,
                document_id: 1,
                document_revision: 2,
                request: Some(request().into()),
            }),
            MAX_OPERATION,
        )
        .with_panel_id(target_panel);
        cm_reply.in_reply_to = cm_w.request_id;
        running.frame(cm_reply, &outgoing).await.unwrap();
        assert!(ok_rx.await.unwrap().is_ok());
    }

    // Late-after-timer Error(Invalid) on child panel MUST also mark semantic drift and schedule resync.
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
            envelope::Body::Snapshot(make_snap(1, 1, b"retained", 0)),
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
                        event: Some(panel::ui_event::Event::SelectedOption(1)),
                    },
                },
                &outgoing,
            )
            .await
            .unwrap();
        let ev_w = wire.try_recv().unwrap();
        assert!(claims.claim_for_write(2, ev_w.request_id));

        running.requests.get_mut(&ev_w.request_id).unwrap().deadline =
            Instant::now() - Duration::from_secs(1);
        running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
        let _cancel_w = wire.try_recv().unwrap();
        let _resync1 = wire.try_recv().unwrap();
        assert!(!running.is_panel_semantic_drifted(2));

        let mut late_inv = Envelope::new(
            running.identity,
            2,
            1,
            envelope::Body::Error(Failure::new(ErrorCode::Invalid, "late callback error")),
            MAX_OPERATION,
        )
        .with_panel_id(2);
        late_inv.in_reply_to = ev_w.request_id;
        running.frame(late_inv, &outgoing).await.unwrap();
        assert!(running.is_panel_semantic_drifted(2));
    }
}

#[tokio::test]
async fn superseded_resync_patch_replacing_table_watermark_is_rejected_and_retried() {
    let make_snap = |rev: u64, vp_rev: u64| PanelSnapshot {
        revision: rev,
        title: "patch-resync-baseline".into(),
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

    running
        .control(
            Control::Event {
                panel_id: 2,
                expected_snapshot: None,
                event_seq: 1,
                event: UiEvent {
                    node_id: 2,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::Viewport(panel::ViewportRequest {
                        revision: 2,
                        offset: 5,
                        limit: 5,
                    })),
                },
            },
            &outgoing,
        )
        .await
        .unwrap();
    let vp_w = wire.try_recv().unwrap();
    assert!(claims.claim_for_write(2, vp_w.request_id));

    // Timer fires first -> Resync #1 sent with vp=2.
    running.requests.get_mut(&vp_w.request_id).unwrap().deadline =
        Instant::now() - Duration::from_secs(1);
    running.handle_timeouts(Instant::now(), &outgoing).await.unwrap();
    let _cancel_w = wire.try_recv().unwrap();
    let resync1 = wire.try_recv().unwrap();

    // Late Error(Deadline) retracts vp 2 -> 1.
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
    assert_eq!(read_vp(&running.panel_snapshot(2).unwrap()), 1);

    // Worker replies Body::Patch(base=1, rev=2) replacing node 2 with stale vp=2 -> MUST be rejected!
    let mut stale_patch = Envelope::new(
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
                    "tbl",
                    panel::node::Kind::VirtualTable(panel::VirtualTable {
                        columns: vec![panel::TableColumn {
                            id: "c1".into(),
                            title: "Col".into(),
                            width: 80.0,
                        }],
                        total_rows: 10,
                        offset: 0,
                        viewport_revision: 2,
                        rows: vec![],
                        multi_select: false,
                    }),
                ))),
            }],
        }),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    stale_patch.in_reply_to = resync1.request_id;
    running.frame(stale_patch, &outgoing).await.unwrap();

    assert!(running.is_panel_semantic_drifted(2));
    assert_eq!(running.panel_revision(2), 1);
    assert_eq!(read_vp(&running.panel_snapshot(2).unwrap()), 1);

    let resync2 = wire.try_recv().unwrap();
    assert!(matches!(
        resync2.body,
        Some(envelope::Body::Resync(ref s)) if read_vp(s) == 1
    ));

    let mut snap2 = Envelope::new(
        running.identity,
        4,
        2,
        envelope::Body::Snapshot(make_snap(2, 1)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    snap2.in_reply_to = resync2.request_id;
    running.frame(snap2, &outgoing).await.unwrap();
    assert!(!running.is_panel_semantic_drifted(2));
    assert!(!running.is_panel_resyncing(2));
    assert_eq!(read_vp(&running.panel_snapshot(2).unwrap()), 1);
}
