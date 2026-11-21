use super::*;
use super::helpers::{BlockingReader, BlockingWriter};

#[tokio::test]
async fn child_resync_barrier_defers_commands_until_settled_and_revoke_cancels() {
    let (mut running, _calls, _updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);
    let primary = Uuid::new_v4();
    let extra = Uuid::new_v4();
    let make_snap = |rev: u64| PanelSnapshot {
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

    running.schedule_panel_resync(2);
    running.flush_pending_resyncs(&outgoing).unwrap();
    let resync1 = wire.try_recv().unwrap();
    assert!(matches!(resync1.body, Some(envelope::Body::Resync(_))));

    // While child panel 2 resync is in flight, both UiEvent and Open(extra) are deferred behind barrier.
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
    let (extra_tx, mut extra_rx) = oneshot::channel();
    running
        .control(
            Control::Open {
                panel_id: 2,
                id: extra,
                revision: 1,
                read_only: false,
                request: request(),
                reply: extra_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(wire.try_recv().is_err());
    assert_eq!(running.panels[&2].deferred_commands.len(), 2);

    // Revoking `extra` (destroy_panel=false) immediately cancels deferred Open(extra) and revokes broker,
    // while deferring CloseRequest behind the active Resync barrier!
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
    assert!(extra_rx.try_recv().unwrap().is_err());
    assert_eq!(running.panels[&2].deferred_commands.len(), 1);
    assert_eq!(running.pending_revokes.len(), 1);
    assert!(
        wire.try_recv().is_err(),
        "non-destroy CloseRequest must not overtake active Resync barrier"
    );

    // Settling Resync releases ONLY non-destroy CloseRequest first while keeping deferred UiEvent queued
    // behind the in-flight non-destroy close barrier; ONLY after CloseRequest settles does UiEvent flush!
    let mut resync_ack =
        Envelope::new(running.identity, 2, 1, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION)
            .with_panel_id(2);
    resync_ack.in_reply_to = resync1.request_id;
    running.frame(resync_ack, &outgoing).await.unwrap();
    assert!(running.pending_revokes.is_empty());
    assert_eq!(
        running.panels[&2].deferred_commands.len(),
        1,
        "deferred UiEvent must remain queued while non-destroy CloseRequest is in flight"
    );
    let close_wire = wire.try_recv().unwrap();
    assert_eq!(close_wire.panel_id, 2);
    assert!(matches!(
        close_wire.body,
        Some(envelope::Body::CloseRequest(ref c)) if !c.destroy_panel
    ));
    assert!(
        wire.try_recv().is_err(),
        "deferred UiEvent must not overtake in-flight non-destroy CloseRequest"
    );

    let mut close_ack =
        Envelope::new(running.identity, 3, 1, envelope::Body::Ack(panel::Empty {}), MAX_OPERATION)
            .with_panel_id(2);
    close_ack.in_reply_to = close_wire.request_id;
    running.frame(close_ack, &outgoing).await.unwrap();
    assert!(rev_rx.try_recv().unwrap().is_ok());
    assert!(running.panels[&2].deferred_commands.is_empty());
    let flushed_ev = wire.try_recv().unwrap();
    assert_eq!(flushed_ev.panel_id, 2);
    assert!(matches!(flushed_ev.body, Some(envelope::Body::UiEvent(_))));

    // Failure completion policy: when Resync aborts while a non-destroy Revoke is deferred,
    // the Revoke reply completes with an explicit error without emitting CloseRequest!
    running.schedule_panel_resync(2);
    running.flush_pending_resyncs(&outgoing).unwrap();
    let resync2 = wire.try_recv().unwrap();
    let (rev2_tx, mut rev2_rx) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: 2,
                id: primary,
                remove_panel: false,
                reply: rev2_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert_eq!(running.pending_revokes.len(), 1);
    assert!(wire.try_recv().is_err());

    let mut abort_err = Envelope::new(
        running.identity,
        4,
        1,
        envelope::Body::Error(Failure::new(ErrorCode::Invalid, "resync aborted")),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    abort_err.in_reply_to = resync2.request_id;
    running.frame(abort_err, &outgoing).await.unwrap();
    assert!(running.pending_revokes.is_empty());
    assert!(wire.try_recv().is_err());
    let err_msg = rev2_rx.try_recv().unwrap().unwrap_err().to_string();
    assert!(err_msg.contains("重同步未完成") && err_msg.contains("本地资源授权已撤销"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_sdk_resync_barrier_prevents_queued_ui_event_from_reverting_confirmed_panel() {
    use std::sync::{Arc, Mutex};


    struct InterleavedBarrierPlugin {
        status: Mutex<String>,
        ev0_entered: Arc<tokio::sync::Notify>,
        ev0_release: Arc<tokio::sync::Notify>,
        resync1_entered: Arc<tokio::sync::Notify>,
        resync1_release: Arc<tokio::sync::Notify>,
        rt: tokio::runtime::Handle,
    }

    impl InterleavedBarrierPlugin {
        fn make_snap(rev: u64, status: &str, ed_rev: u64, ed_bytes: Vec<u8>) -> PanelSnapshot {
            PanelSnapshot {
                revision: rev,
                title: "sdk-resync-barrier".into(),
                root: Some(panel::Node::new(
                    3,
                    "col",
                    panel::node::Kind::Column(panel::Container {
                        children: vec![
                            panel::Node::new(
                                1,
                                "ed",
                                panel::node::Kind::Editor(panel::Editor {
                                    document_id: 1,
                                    revision: ed_rev,
                                    contents: ed_bytes,
                                    format: panel::EditorFormat::Utf8 as i32,
                                    read_only: false,
                                }),
                            ),
                            panel::Node::new(
                                2,
                                "status",
                                panel::node::Kind::Text(panel::Text {
                                    text: status.to_owned(),
                                    tone: panel::Tone::Foreground as i32,
                                }),
                            ),
                            panel::Node::new(
                                10,
                                "btn0",
                                panel::node::Kind::Button(panel::Button {
                                    label: "TriggerStale".into(),
                                    enabled: true,
                                    commit_editors: false,
                                }),
                            ),
                            panel::Node::new(
                                12,
                                "btnB",
                                panel::node::Kind::Button(panel::Button {
                                    label: "EventB".into(),
                                    enabled: true,
                                    commit_editors: false,
                                }),
                            ),
                        ],
                    }),
                )),
            }
        }
    }

    impl mio_plugin_sdk::MioPlugin for InterleavedBarrierPlugin {
        fn id(&self) -> &str {
            "lab.sdk-resync-barrier"
        }

        fn version(&self) -> &str {
            "1.5.0"
        }

        fn supports_panel_multiplexing(&self) -> bool {
            false
        }

        fn initial_panel(&self) -> PanelSnapshot {
            Self::make_snap(1, "v1-old", 1, b"init".to_vec())
        }

        fn on_panel_ui_event(
            &mut self,
            ctx: &mut dyn mio_plugin_sdk::HostClient,
            _panel_id: u64,
            event: UiEvent,
        ) -> std::result::Result<Option<PanelSnapshot>, String> {
            match (event.node_id, &event.event) {
                (10, Some(panel::ui_event::Event::Click(_))) => {
                    self.ev0_entered.notify_one();
                    self.rt.block_on(self.ev0_release.notified());
                    let rev = ctx.next_panel_revision();
                    return Ok(Some(Self::make_snap(
                        rev,
                        "stale-from-ev0",
                        1,
                        b"init".to_vec(),
                    )));
                }
                (1, Some(panel::ui_event::Event::EditorChanged(ed))) => {
                    let mut st = self.status.lock().unwrap();
                    *st = "v2-confirmed".into();
                    let rev = ctx.next_panel_revision();
                    return Ok(Some(Self::make_snap(
                        rev,
                        &st,
                        ed.revision,
                        ed.contents.clone(),
                    )));
                }
                (12, Some(panel::ui_event::Event::Click(_))) => {
                    let mut st = self.status.lock().unwrap();
                    *st = format!("{st}+evB");
                    let rev = ctx.next_panel_revision();
                    return Ok(Some(Self::make_snap(
                        rev,
                        &st,
                        2,
                        b"user-edit".to_vec(),
                    )));
                }
                _ => {}
            }
            Ok(None)
        }

        fn on_panel_resync(
            &mut self,
            ctx: &mut dyn mio_plugin_sdk::HostClient,
            _panel_id: u64,
            snapshot: PanelSnapshot,
        ) -> std::result::Result<Option<PanelSnapshot>, String> {
            let status_text = snapshot
                .root
                .as_ref()
                .and_then(|r| r.find(2))
                .and_then(|n| match &n.kind {
                    Some(panel::node::Kind::Text(t)) => Some(t.text.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            if status_text == "v1-old" {
                self.resync1_entered.notify_one();
                self.rt.block_on(self.resync1_release.notified());
                *self.status.lock().unwrap() = status_text.clone();
                let rev = ctx.next_panel_revision();
                return Ok(Some(Self::make_snap(rev, &status_text, 2, b"user-edit".to_vec())));
            }
            *self.status.lock().unwrap() = status_text.clone();
            let rev = ctx.next_panel_revision();
            Ok(Some(Self::make_snap(rev, &status_text, 2, b"user-edit".to_vec())))
        }
    }

    let rt_handle = tokio::runtime::Handle::current();
    let identity = crate::plugins::tests_runtime::identity();
    let manifest = PluginManifest {
        id: "lab.sdk-resync-barrier".into(),
        version: "1.5.0".into(),
        api_version: 1,
        engine: "native".into(),
        entry: "worker".into(),
        permissions: vec![],
        contributions: vec!["suite_panel".into()],
    };
    let (host_tx, worker_rx) = tokio::io::duplex(64 * 1024);
    let (worker_tx, host_rx) = tokio::io::duplex(64 * 1024);
    let ev0_entered = Arc::new(tokio::sync::Notify::new());
    let ev0_release = Arc::new(tokio::sync::Notify::new());
    let resync1_entered = Arc::new(tokio::sync::Notify::new());
    let resync1_release = Arc::new(tokio::sync::Notify::new());
    let ev0_entered_w = ev0_entered.clone();
    let ev0_release_w = ev0_release.clone();
    let resync1_entered_w = resync1_entered.clone();
    let resync1_release_w = resync1_release.clone();

    let worker_thread = std::thread::spawn(move || {
        let mut plugin = InterleavedBarrierPlugin {
            status: Mutex::new("v1-old".into()),
            ev0_entered: ev0_entered_w,
            ev0_release: ev0_release_w,
            resync1_entered: resync1_entered_w,
            resync1_release: resync1_release_w,
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
        BTreeSet::from([Permission::TrafficRead]),
        host_tx,
        host_rx,
    )
    .await
    .unwrap();
    let handle = supervisor.handle();

    let extract_status = |snap: &PanelSnapshot| -> String {
        snap.root
            .as_ref()
            .and_then(|r| r.find(2))
            .and_then(|n| match &n.kind {
                Some(panel::node::Kind::Text(t)) => Some(t.text.clone()),
                _ => None,
            })
            .unwrap_or_default()
    };

    let mut seen_statuses = Vec::new();
    let ev_init = timeout(MAX_OPERATION, supervisor.events.recv())
        .await
        .unwrap()
        .unwrap();
    let PluginEvent::Panel { snapshot, .. } = ev_init else {
        panic!("expected initial panel");
    };
    seen_statuses.push(extract_status(&snapshot));
    assert_eq!(seen_statuses.last().map(String::as_str), Some("v1-old"));

    // 1. Trigger ev0 -> enters worker on_panel_ui_event and pauses before returning stale editor rev=1.
    handle
        .try_event(UiEvent {
            node_id: 10,
            editors: vec![],
            event: Some(panel::ui_event::Event::Click(panel::Empty {})),
        })
        .unwrap();
    timeout(MAX_OPERATION, ev0_entered.notified()).await.unwrap();

    // 2. While ev0 is in flight on worker, native EditorChanged (UiEvent A) updates host editor rev=2 and queues on wire.
    handle
        .try_event(UiEvent {
            node_id: 1,
            editors: vec![],
            event: Some(panel::ui_event::Event::EditorChanged(panel::EditorSnapshot {
                document_id: 1,
                revision: 2,
                contents: b"user-edit".to_vec(),
                selection_start: 0,
                selection_end: 0,
            })),
        })
        .unwrap();

    // 3. Release ev0 -> host rejects ev0's stale Snapshot -> emits Resync(1) AFTER UiEvent A;
    //    worker first completes UiEvent A ("v2-confirmed") and then blocks inside Resync(1).
    ev0_release.notify_one();
    timeout(MAX_OPERATION, resync1_entered.notified()).await.unwrap();
    timeout(MAX_OPERATION, async {
        loop {
            if let Some(PluginEvent::Panel { snapshot, .. }) = supervisor.events.recv().await {
                let s = extract_status(&snapshot);
                seen_statuses.push(s.clone());
                if s == "v2-confirmed" {
                    break;
                }
            }
        }
    })
    .await
    .unwrap();

    // 4. While stale Resync(1) is STILL in flight on worker, issue UiEvent B (Click node 12)!
    //    Per-panel barrier MUST defer UiEvent B until corrective Resync settles!
    handle
        .try_event(UiEvent {
            node_id: 12,
            editors: vec![],
            event: Some(panel::ui_event::Event::Click(panel::Empty {})),
        })
        .unwrap();
    resync1_release.notify_one();

    // 5. Expect final confirmed state "v2-confirmed+evB", and NEVER "v1-old" or "v1-old+evB" after "v2-confirmed"!
    timeout(MAX_OPERATION, async {
        loop {
            if let Some(PluginEvent::Panel { snapshot, .. }) = supervisor.events.recv().await {
                let s = extract_status(&snapshot);
                seen_statuses.push(s.clone());
                if s == "v2-confirmed+evB" {
                    break;
                }
            }
        }
    })
    .await
    .unwrap();

    let v2_pos = seen_statuses.iter().position(|s| s == "v2-confirmed").unwrap();
    assert!(
        !seen_statuses[v2_pos + 1..]
            .iter()
            .any(|s| s == "v1-old" || s == "v1-old+evB"),
        "confirmed panel reverted during interleaved resync + queued UiEvent: {seen_statuses:?}"
    );

    supervisor.stop().await.unwrap();
    worker_thread.join().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_sdk_non_destroy_close_waits_for_resync_barrier_and_never_reverts_panel() {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};


    struct ChildCloseBarrierPlugin {
        statuses: Mutex<HashMap<u64, String>>,
        ev0_entered: Arc<tokio::sync::Notify>,
        ev0_release: Arc<tokio::sync::Notify>,
        resync1_entered: Arc<tokio::sync::Notify>,
        resync1_release: Arc<tokio::sync::Notify>,
        rt: tokio::runtime::Handle,
    }

    impl ChildCloseBarrierPlugin {
        fn make_snap(rev: u64, status: &str, ed_rev: u64, ed_bytes: Vec<u8>) -> PanelSnapshot {
            PanelSnapshot {
                revision: rev,
                title: format!("sdk-child-{rev}"),
                root: Some(panel::Node::new(
                    3,
                    "col",
                    panel::node::Kind::Column(panel::Container {
                        children: vec![
                            panel::Node::new(
                                1,
                                "ed",
                                panel::node::Kind::Editor(panel::Editor {
                                    document_id: 1,
                                    revision: ed_rev,
                                    contents: ed_bytes,
                                    format: panel::EditorFormat::Utf8 as i32,
                                    read_only: false,
                                }),
                            ),
                            panel::Node::new(
                                2,
                                "status",
                                panel::node::Kind::Text(panel::Text {
                                    text: status.to_owned(),
                                    tone: panel::Tone::Foreground as i32,
                                }),
                            ),
                            panel::Node::new(
                                10,
                                "btn0",
                                panel::node::Kind::Button(panel::Button {
                                    label: "TriggerStale".into(),
                                    enabled: true,
                                    commit_editors: false,
                                }),
                            ),
                        ],
                    }),
                )),
            }
        }
    }

    impl mio_plugin_sdk::MioPlugin for ChildCloseBarrierPlugin {
        fn id(&self) -> &str {
            "lab.sdk-close-barrier"
        }

        fn version(&self) -> &str {
            "1.5.0"
        }

        fn supports_panel_multiplexing(&self) -> bool {
            true
        }

        fn initial_panel(&self) -> PanelSnapshot {
            Self::make_snap(1, "main-init", 1, b"init".to_vec())
        }

        fn on_panel_open_request(
            &mut self,
            ctx: &mut dyn mio_plugin_sdk::HostClient,
            panel_id: u64,
            _open: protocol::OpenRequest,
        ) -> std::result::Result<Option<PanelSnapshot>, String> {
            self.statuses
                .lock()
                .unwrap()
                .insert(panel_id, "v1-old".into());
            let rev = ctx.next_panel_revision();
            Ok(Some(Self::make_snap(rev, "v1-old", 1, b"init".to_vec())))
        }

        fn on_panel_ui_event(
            &mut self,
            ctx: &mut dyn mio_plugin_sdk::HostClient,
            panel_id: u64,
            event: UiEvent,
        ) -> std::result::Result<Option<PanelSnapshot>, String> {
            match (event.node_id, &event.event) {
                (10, Some(panel::ui_event::Event::Click(_))) => {
                    self.ev0_entered.notify_one();
                    self.rt.block_on(self.ev0_release.notified());
                    let rev = ctx.next_panel_revision();
                    return Ok(Some(Self::make_snap(
                        rev,
                        "stale-from-ev0",
                        1,
                        b"init".to_vec(),
                    )));
                }
                (1, Some(panel::ui_event::Event::EditorChanged(ed))) => {
                    let mut map = self.statuses.lock().unwrap();
                    map.insert(panel_id, "v2-confirmed".into());
                    let rev = ctx.next_panel_revision();
                    return Ok(Some(Self::make_snap(
                        rev,
                        "v2-confirmed",
                        ed.revision,
                        ed.contents.clone(),
                    )));
                }
                _ => {}
            }
            Ok(None)
        }

        fn on_panel_resync(
            &mut self,
            ctx: &mut dyn mio_plugin_sdk::HostClient,
            panel_id: u64,
            snapshot: PanelSnapshot,
        ) -> std::result::Result<Option<PanelSnapshot>, String> {
            let status_text = snapshot
                .root
                .as_ref()
                .and_then(|r| r.find(2))
                .and_then(|n| match &n.kind {
                    Some(panel::node::Kind::Text(t)) => Some(t.text.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            if status_text == "v1-old" {
                self.resync1_entered.notify_one();
                self.rt.block_on(self.resync1_release.notified());
                self.statuses
                    .lock()
                    .unwrap()
                    .insert(panel_id, status_text.clone());
                let rev = ctx.next_panel_revision();
                return Ok(Some(Self::make_snap(rev, &status_text, 2, b"user-edit".to_vec())));
            }
            self.statuses.lock().unwrap().insert(panel_id, status_text);
            Ok(None)
        }

        fn on_panel_close_request(
            &mut self,
            ctx: &mut dyn mio_plugin_sdk::HostClient,
            panel_id: u64,
            close: protocol::CloseRequest,
        ) -> std::result::Result<Option<PanelSnapshot>, String> {
            assert!(!close.destroy_panel);
            let mut map = self.statuses.lock().unwrap();
            let next = format!(
                "{}+closed",
                map.get(&panel_id).map_or("", String::as_str)
            );
            map.insert(panel_id, next.clone());
            let rev = ctx.next_panel_revision();
            Ok(Some(Self::make_snap(rev, &next, 2, b"user-edit".to_vec())))
        }
    }

    let rt_handle = tokio::runtime::Handle::current();
    let identity = crate::plugins::tests_runtime::identity();
    let manifest = PluginManifest {
        id: "lab.sdk-close-barrier".into(),
        version: "1.5.0".into(),
        api_version: 1,
        engine: "native".into(),
        entry: "worker".into(),
        permissions: vec![Permission::TrafficRead],
        contributions: vec!["suite_panel".into()],
    };
    let (host_tx, worker_rx) = tokio::io::duplex(64 * 1024);
    let (worker_tx, host_rx) = tokio::io::duplex(64 * 1024);
    let ev0_entered = Arc::new(tokio::sync::Notify::new());
    let ev0_release = Arc::new(tokio::sync::Notify::new());
    let resync1_entered = Arc::new(tokio::sync::Notify::new());
    let resync1_release = Arc::new(tokio::sync::Notify::new());
    let ev0_entered_w = ev0_entered.clone();
    let ev0_release_w = ev0_release.clone();
    let resync1_entered_w = resync1_entered.clone();
    let resync1_release_w = resync1_release.clone();

    let worker_thread = std::thread::spawn(move || {
        let mut plugin = ChildCloseBarrierPlugin {
            statuses: Mutex::new(HashMap::new()),
            ev0_entered: ev0_entered_w,
            ev0_release: ev0_release_w,
            resync1_entered: resync1_entered_w,
            resync1_release: resync1_release_w,
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
        BTreeSet::from([Permission::TrafficRead]),
        host_tx,
        host_rx,
    )
    .await
    .unwrap();
    let handle = supervisor.handle();
    let primary = Uuid::new_v4();
    let extra = Uuid::new_v4();

    let extract_status = |snap: &PanelSnapshot| -> String {
        snap.root
            .as_ref()
            .and_then(|r| r.find(2))
            .and_then(|n| match &n.kind {
                Some(panel::node::Kind::Text(t)) => Some(t.text.clone()),
                _ => None,
            })
            .unwrap_or_default()
    };

    // Wait initial main panel, then open child panel 2 with `primary`.
    let _ = timeout(MAX_OPERATION, supervisor.events.recv()).await.unwrap().unwrap();
    let opened = timeout(
        MAX_OPERATION,
        handle.open_request_on_panel(2, primary, 1, false, request()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(extract_status(&opened), "v1-old");

    let mut seen_child_statuses = Vec::new();
    // 1. Trigger ev0 -> worker pauses inside on_panel_ui_event(10) on child panel 2.
    handle
        .try_event_on_panel(
            2,
            UiEvent {
                node_id: 10,
                editors: vec![],
                event: Some(panel::ui_event::Event::Click(panel::Empty {})),
            },
        )
        .unwrap();
    timeout(MAX_OPERATION, ev0_entered.notified()).await.unwrap();

    // 2. Native EditorChanged (UiEvent A) bumps host child editor rev=2 and queues on wire.
    handle
        .try_event_on_panel(
            2,
            UiEvent {
                node_id: 1,
                editors: vec![],
                event: Some(panel::ui_event::Event::EditorChanged(panel::EditorSnapshot {
                    document_id: 1,
                    revision: 2,
                    contents: b"user-edit".to_vec(),
                    selection_start: 0,
                    selection_end: 0,
                })),
            },
        )
        .unwrap();

    // 3. Release ev0 -> host rejects stale Snapshot -> sends Resync(1) AFTER UiEvent A;
    //    worker first completes UiEvent A ("v2-confirmed") then blocks inside Resync(1).
    ev0_release.notify_one();
    timeout(MAX_OPERATION, resync1_entered.notified()).await.unwrap();
    timeout(MAX_OPERATION, async {
        loop {
            if let Some(PluginEvent::Panel { panel_id: 2, snapshot, .. }) =
                supervisor.events.recv().await
            {
                let s = extract_status(&snapshot);
                seen_child_statuses.push(s.clone());
                if s == "v2-confirmed" {
                    break;
                }
            }
        }
    })
    .await
    .unwrap();

    // 4. Issue non-destroy revoke while stale Resync(1) is active on worker!
    let revoke_handle = handle.clone();
    let revoke_task = tokio::spawn(async move {
        revoke_handle.revoke_panel_resource(2, extra, false).await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    resync1_release.notify_one();

    // CloseRequest(destroy_panel=false) must wait until corrective Resync settles "v2-confirmed",
    // yielding final state "v2-confirmed+closed" without EVER reverting to "v1-old" or "v1-old+closed".
    timeout(MAX_OPERATION, revoke_task).await.unwrap().unwrap().unwrap();
    timeout(MAX_OPERATION, async {
        loop {
            if let Some(PluginEvent::Panel { panel_id: 2, snapshot, .. }) =
                supervisor.events.recv().await
            {
                let s = extract_status(&snapshot);
                seen_child_statuses.push(s.clone());
                if s == "v2-confirmed+closed" {
                    break;
                }
            }
        }
    })
    .await
    .unwrap();

    let v2_pos = seen_child_statuses.iter().position(|s| s == "v2-confirmed").unwrap();
    assert!(
        !seen_child_statuses[v2_pos + 1..]
            .iter()
            .any(|s| s == "v1-old" || s == "v1-old+closed"),
        "confirmed child panel reverted via CloseRequest(destroy=false): {seen_child_statuses:?}"
    );

    supervisor.stop().await.unwrap();
    worker_thread.join().unwrap();
}

#[tokio::test]
async fn deferred_command_event_rejected_when_resync_changes_widget_semantics() {
    let make_snap = |rev: u64, opts: [&str; 2], btn_ctx: &str, ed_text: &[u8]| PanelSnapshot {
        revision: rev,
        title: "semantic-panel".into(),
        root: Some(panel::Node::new(
            1,
            "col",
            panel::node::Kind::Column(panel::Container {
                children: vec![
                    panel::Node::new(
                        2,
                        "record-ctx",
                        panel::node::Kind::Text(panel::Text {
                            text: btn_ctx.to_owned(),
                            tone: 0,
                        }),
                    ),
                    panel::Node::new(
                        5,
                        "action-select",
                        panel::node::Kind::Select(panel::SelectNode {
                            label: "Action".into(),
                            options: opts.iter().map(|s| (*s).to_owned()).collect(),
                            selected_index: 1,
                            enabled: true,
                        }),
                    ),
                    panel::Node::new(
                        10,
                        "delete-btn",
                        panel::node::Kind::Button(panel::Button {
                            label: "Delete".into(),
                            enabled: true,
                            commit_editors: false,
                        }),
                    ),
                    panel::Node::new(
                        20,
                        "ed",
                        panel::node::Kind::Editor(panel::Editor {
                            document_id: 1,
                            revision: 1,
                            contents: ed_text.to_vec(),
                            format: panel::EditorFormat::Utf8 as i32,
                            read_only: false,
                        }),
                    ),
                ],
            }),
        )),
    };

    let (mut running, _calls, mut updates) = running();
    let snap_rev1 = Arc::new(make_snap(1, ["Allow", "Deny"], "Record A", b"init"));
    *running.main_panel_mut() = PanelState::new((*snap_rev1).clone()).unwrap();
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

    // 1. Trigger main-panel Resync(1) -> barrier active; defer SelectedOption(0) & Click(10) from snap_rev1.
    running.schedule_panel_resync(protocol::PANEL_ID);
    running.flush_pending_resyncs(&outgoing).unwrap();
    let resync1 = wire.try_recv().unwrap();

    running
        .control(
            Control::Event {
                panel_id: protocol::PANEL_ID,
                event: UiEvent {
                    node_id: 5,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::SelectedOption(0)),
                },
                expected_snapshot: Some(snap_rev1.clone()),
                event_seq: 0,
            },
            &outgoing,
        )
        .await
        .unwrap();
    running
        .control(
            Control::Event {
                panel_id: protocol::PANEL_ID,
                event: UiEvent {
                    node_id: 10,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::Click(panel::Empty {})),
                },
                expected_snapshot: Some(snap_rev1.clone()),
                event_seq: 0,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(wire.try_recv().is_err());

    // Corrective Resync reply returns Snapshot(2) that REORDERED Select options ["Allow","Deny"] -> ["Deny","Allow"]
    // AND rebound sibling context "Record A" -> "Record B"!
    while updates.try_recv().is_ok() {}
    let mut resync_snap2 = Envelope::new(
        running.identity,
        1,
        2,
        envelope::Body::Snapshot(make_snap(2, ["Deny", "Allow"], "Record B", b"init")),
        MAX_OPERATION,
    );
    resync_snap2.in_reply_to = resync1.request_id;
    running.frame(resync_snap2, &outgoing).await.unwrap();

    // Neither SelectedOption(0) nor Click(10) may be emitted onto wire; both emit Stale diagnostics!
    assert!(wire.try_recv().is_err());
    let mut stale_count = 0usize;
    while let Ok(ev) = updates.try_recv() {
        if let PluginEvent::Error { failure, .. } = ev
            && failure.code == ErrorCode::Stale as i32
            && failure.message.contains("插件面板内容已更新")
        {
            stale_count += 1;
        }
    }
    assert_eq!(stale_count, 2);

    // 2. Identical content tree across revision bump (2 -> 3) STILL rebases Click(10) cleanly;
    //    EditorChanged STILL merges cleanly even when sibling text ("Record B" -> "Record C") changes!
    let snap_rev2 = Arc::new(running.main_panel().snapshot().clone());
    running.schedule_panel_resync(protocol::PANEL_ID);
    running.flush_pending_resyncs(&outgoing).unwrap();
    let resync2 = wire.try_recv().unwrap();

    running
        .control(
            Control::Event {
                panel_id: protocol::PANEL_ID,
                event: UiEvent {
                    node_id: 10,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::Click(panel::Empty {})),
                },
                expected_snapshot: Some(snap_rev2),
                event_seq: 0,
            },
            &outgoing,
        )
        .await
        .unwrap();
    running
        .control(
            Control::Event {
                panel_id: protocol::PANEL_ID,
                event: UiEvent {
                    node_id: 20,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::EditorChanged(panel::EditorSnapshot {
                        document_id: 1,
                        revision: 2,
                        contents: b"typed-during-resync".to_vec(),
                        selection_start: 0,
                        selection_end: 0,
                    })),
                },
                expected_snapshot: Some(snap_rev1),
                event_seq: 0,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(wire.try_recv().is_err());

    let mut resync_snap3 = Envelope::new(
        running.identity,
        2,
        3,
        envelope::Body::Snapshot(make_snap(3, ["Deny", "Allow"], "Record B", b"init")),
        MAX_OPERATION,
    );
    resync_snap3.in_reply_to = resync2.request_id;
    running.frame(resync_snap3, &outgoing).await.unwrap();

    // Deferred commands serialize: ONLY Click(10) emits first while EditorChanged waits for it to settle.
    let ev_click = wire.try_recv().unwrap();
    assert_eq!(ev_click.revision, 3);
    assert!(matches!(ev_click.body, Some(envelope::Body::UiEvent(ref u)) if u.node_id == 10));
    assert!(
        wire.try_recv().is_err(),
        "second deferred command must wait for first state-updating deferred event to settle"
    );

    let mut click_snap4 = Envelope::new(
        running.identity,
        3,
        4,
        envelope::Body::Snapshot(make_snap(4, ["Deny", "Allow"], "Record C", b"init")),
        MAX_OPERATION,
    );
    click_snap4.in_reply_to = ev_click.request_id;
    running.frame(click_snap4, &outgoing).await.unwrap();

    // Now EditorChanged merges cleanly at revision 4 even though sibling text ("Record B" -> "Record C") changed!
    let ev_edit = wire.try_recv().unwrap();
    assert_eq!(ev_edit.revision, 4);
    assert!(matches!(ev_edit.body, Some(envelope::Body::UiEvent(ref u)) if u.node_id == 20));
    assert_eq!(running.main_panel().editor(1).unwrap().contents, b"typed-during-resync");
}

#[tokio::test]
async fn child_deferred_command_wakes_when_outgoing_capacity_recovers_without_new_frames() {
    let identity = crate::plugins::tests_runtime::identity();
    let manifest = PluginManifest {
        id: "lab.outbound-wakeup".into(),
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
    let send_ack = Arc::new(tokio::sync::Notify::new());
    let resume_reading = Arc::new(tokio::sync::Notify::new());
    let pause_reading_w = pause_reading.clone();
    let send_ack_w = send_ack.clone();
    let resume_reading_w = resume_reading.clone();
    let open2_ready = Arc::new(tokio::sync::Notify::new());
    let open2_ready_w = open2_ready.clone();

    let send_frame = move |_tx: &mut tokio::io::DuplexStream,
                      seq: &mut u64,
                      panel_id: u64,
                      rev: u64,
                      in_reply_to: u64,
                      body: envelope::Body| {
        *seq += 1;
        let mut env = Envelope::new(identity, *seq, rev, body, MAX_OPERATION)
            .with_panel_id(panel_id);
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
            &mut worker_tx,
            &mut out_seq,
            protocol::PANEL_ID,
            1,
            hello.request_id,
            envelope::Body::Register(protocol::Register {
                plugin_id: "lab.outbound-wakeup".into(),
                plugin_version: "1.5.0".into(),
                required_components: vec![],
                panel: Some(make_snap(1, "main")),
                supports_panel_multiplexing: true,
            }),
        );
        worker_tx.write_all(&protocol::encode_frame(&reg)?).await?;
        worker_tx.flush().await?;
        let _ready = read_async_frame(&mut worker_rx).await?;

        // Open child panel 2 at rev=1, then send unsolicited Snapshot(rev=2) + stale Snapshot(rev=1) to trigger Resync(2).
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

        // Stop reading worker_rx so duplex buffer fills and wait until test queues `open_extra` behind Resync(2).
        pause_reading_w.notify_one();
        send_ack_w.notified().await;

        // Emit enough invalid-parent HostCalls so `outgoing` holds > (OUTGOING_CAPACITY - RESERVED_CONTROL_OUTGOING) HostResult frames.
        for _ in 0..(OUTGOING_CAPACITY - RESERVED_CONTROL_OUTGOING + 8) {
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
        // Now settle Resync(2) via Ack(2) and STOP sending any further inbound frames!
        let ack = send_frame(
            &mut worker_tx,
            &mut out_seq,
            2,
            2,
            resync.request_id,
            envelope::Body::Ack(panel::Empty {}),
        );
        worker_tx.write_all(&protocol::encode_frame(&ack)?).await?;
        worker_tx.flush().await?;

        // Wait until test confirms `open_extra` is queued while `worker_rx` is paused, then resume draining ONLY worker_rx!
        resume_reading_w.notified().await;
        let open_extra = loop {
            let env = read_async_frame(&mut worker_rx).await?;
            if matches!(env.body, Some(envelope::Body::OpenRequest(_))) {
                break env;
            }
        };
        open2_ready_w.notify_one();
        let reply_snap = send_frame(
            &mut worker_tx,
            &mut out_seq,
            2,
            3,
            open_extra.request_id,
            envelope::Body::Snapshot(make_snap(3, "c2-extra-opened")),
        );
        worker_tx.write_all(&protocol::encode_frame(&reply_snap)?).await?;
        worker_tx.flush().await?;

        loop {
            match read_async_frame(&mut worker_rx).await {
                Ok(env) if matches!(env.body, Some(envelope::Body::Shutdown(_))) => break,
                Ok(_) => {}
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

    // Wait until worker received Resync(2) and paused reading worker_rx, then queue open_extra behind Resync(2).
    timeout(MAX_OPERATION, pause_reading.notified()).await.unwrap();
    let open_handle = handle.clone();
    let mut open_extra_task = tokio::spawn(async move {
        open_handle
            .open_request_on_panel(2, extra, 1, false, request())
            .await
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    send_ack.notify_one();

    // Wait long enough for Running to process HostCalls + Ack(2) and hit outgoing capacity congestion with zero further events.
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut open_extra_task)
            .await
            .is_err(),
        "open_extra must still be blocked while outgoing capacity is saturated"
    );

    // Resume reading ONLY on worker_rx — zero new inbound frames or commands are sent —
    // Running MUST wake purely from outbound capacity recovery and deliver OpenRequest(extra)!
    resume_reading.notify_one();
    timeout(MAX_OPERATION, open2_ready.notified()).await.unwrap();
    let reopened_snap = timeout(MAX_OPERATION, open_extra_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(reopened_snap.title, "c2-extra-opened");

    supervisor.stop().await.unwrap();
    timeout(MAX_OPERATION, worker_task).await.unwrap().unwrap().unwrap();
}

#[tokio::test]
async fn resync_panel_schedules_barrier_before_flushing_queued_non_destroy_revoke() {
    let (mut running, _calls, _updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

    let make_snap = |rev: u64| PanelSnapshot {
        revision: rev,
        title: format!("panel-r{rev}"),
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
    let third = Uuid::new_v4();
    let (open1_tx, open1_rx) = oneshot::channel();
    running
        .control(
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
    let open1_wire = wire.try_recv().unwrap();
    let mut open1_snap = Envelope::new(
        running.identity,
        1,
        2,
        envelope::Body::Snapshot(make_snap(2)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    open1_snap.in_reply_to = open1_wire.request_id;
    running.frame(open1_snap, &outgoing).await.unwrap();
    assert!(open1_rx.await.is_ok());

    // Keep one in-flight Open on child panel 2, then saturate `requests` to QUEUE_CAPACITY on main panel.
    let (open2_tx, mut open2_rx) = oneshot::channel();
    running
        .control(
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
    let open2_wire = wire.try_recv().unwrap();

    let mut main_req_ids = Vec::new();
    while running.requests.len() < QUEUE_CAPACITY {
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
        main_req_ids.push(ev_wire.request_id);
    }
    assert_eq!(running.requests.len(), QUEUE_CAPACITY);

    // Queue a non-destroy Revoke while `requests` is full -> waits in `pending_revokes`.
    let (rev_tx, mut rev_rx) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: 2,
                id: third,
                remove_panel: false,
                reply: rev_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert_eq!(running.pending_revokes.len(), 1);
    assert!(wire.try_recv().is_err());

    // An unapplicable Snapshot on `open2` invokes `resync_panel`, which frees a slot
    // (`128 -> 127`). `schedule_panel_resync(2)` MUST run BEFORE `flush_pending_revokes`!
    let mut bad_snap = Envelope::new(
        running.identity,
        2,
        1,
        envelope::Body::Snapshot(make_snap(1)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    bad_snap.in_reply_to = open2_wire.request_id;
    running.frame(bad_snap, &outgoing).await.unwrap();
    assert!(open2_rx.try_recv().unwrap().is_err());
    assert!(running.is_panel_resyncing(2));
    assert_eq!(
        running.pending_revokes.len(),
        1,
        "queued non-destroy CloseRequest must remain behind Resync barrier freed by reject_response"
    );
    assert!(
        wire.try_recv().is_err(),
        "CloseRequest must not leak ahead of scheduled child Resync"
    );

    // Drain main requests below CHILD_QUEUE_SOFT_LIMIT -> only Resync(2) flushes; CloseRequest stays queued.
    let mut seq = 3u64;
    while running.requests.len() >= CHILD_QUEUE_SOFT_LIMIT {
        let req_id = main_req_ids.pop().unwrap();
        let mut ack = Envelope::new(
            running.identity,
            seq,
            1,
            envelope::Body::Ack(panel::Empty {}),
            MAX_OPERATION,
        );
        seq += 1;
        ack.in_reply_to = req_id;
        running.frame(ack, &outgoing).await.unwrap();
    }
    let resync_wire = wire.try_recv().unwrap();
    assert_eq!(resync_wire.panel_id, 2);
    assert!(matches!(resync_wire.body, Some(envelope::Body::Resync(_))));
    assert!(wire.try_recv().is_err());
    assert_eq!(running.pending_revokes.len(), 1);

    // Settling Resync releases the deferred CloseRequest!
    let mut resync_ack = Envelope::new(
        running.identity,
        seq,
        2,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    resync_ack.in_reply_to = resync_wire.request_id;
    running.frame(resync_ack, &outgoing).await.unwrap();
    assert!(running.pending_revokes.is_empty());
    let close_wire = wire.try_recv().unwrap();
    assert_eq!(close_wire.panel_id, 2);
    assert!(matches!(
        close_wire.body,
        Some(envelope::Body::CloseRequest(ref c)) if !c.destroy_panel
    ));
    let mut close_ack = Envelope::new(
        running.identity,
        seq + 1,
        2,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    close_ack.in_reply_to = close_wire.request_id;
    running.frame(close_ack, &outgoing).await.unwrap();
    assert!(rev_rx.try_recv().unwrap().is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_sdk_non_destroy_close_reordering_select_rejects_deferred_ui_event() {
    use std::sync::{Arc, Mutex};


    struct SelectCloseReorderPlugin {
        seen_select_events: Arc<Mutex<Vec<u32>>>,
        ev0_entered: Arc<tokio::sync::Notify>,
        ev0_release: Arc<tokio::sync::Notify>,
        resync1_entered: Arc<tokio::sync::Notify>,
        resync1_release: Arc<tokio::sync::Notify>,
        rt: tokio::runtime::Handle,
    }

    impl SelectCloseReorderPlugin {
        fn make_snap(
            rev: u64,
            status: &str,
            ed_rev: u64,
            ed_bytes: Vec<u8>,
            opts: [&str; 2],
        ) -> PanelSnapshot {
            PanelSnapshot {
                revision: rev,
                title: "sdk-select-close".into(),
                root: Some(panel::Node::new(
                    3,
                    "col",
                    panel::node::Kind::Column(panel::Container {
                        children: vec![
                            panel::Node::new(
                                1,
                                "ed",
                                panel::node::Kind::Editor(panel::Editor {
                                    document_id: 1,
                                    revision: ed_rev,
                                    contents: ed_bytes,
                                    format: panel::EditorFormat::Utf8 as i32,
                                    read_only: false,
                                }),
                            ),
                            panel::Node::new(
                                2,
                                "status",
                                panel::node::Kind::Text(panel::Text {
                                    text: status.to_owned(),
                                    tone: panel::Tone::Foreground as i32,
                                }),
                            ),
                            panel::Node::new(
                                10,
                                "btn0",
                                panel::node::Kind::Button(panel::Button {
                                    label: "TriggerStale".into(),
                                    enabled: true,
                                    commit_editors: false,
                                }),
                            ),
                            panel::Node::new(
                                20,
                                "action-select",
                                panel::node::Kind::Select(panel::SelectNode {
                                    label: "Action".into(),
                                    options: opts.iter().map(|s| (*s).to_owned()).collect(),
                                    selected_index: 1,
                                    enabled: true,
                                }),
                            ),
                        ],
                    }),
                )),
            }
        }
    }

    impl mio_plugin_sdk::MioPlugin for SelectCloseReorderPlugin {
        fn id(&self) -> &str {
            "lab.sdk-select-close-reorder"
        }

        fn version(&self) -> &str {
            "1.5.0"
        }

        fn supports_panel_multiplexing(&self) -> bool {
            true
        }

        fn initial_panel(&self) -> PanelSnapshot {
            Self::make_snap(1, "main-init", 1, b"init".to_vec(), ["Allow", "Deny"])
        }

        fn on_panel_open_request(
            &mut self,
            ctx: &mut dyn mio_plugin_sdk::HostClient,
            _panel_id: u64,
            _open: protocol::OpenRequest,
        ) -> std::result::Result<Option<PanelSnapshot>, String> {
            let rev = ctx.next_panel_revision();
            Ok(Some(Self::make_snap(
                rev,
                "v1-old",
                1,
                b"init".to_vec(),
                ["Allow", "Deny"],
            )))
        }

        fn on_panel_ui_event(
            &mut self,
            ctx: &mut dyn mio_plugin_sdk::HostClient,
            _panel_id: u64,
            event: UiEvent,
        ) -> std::result::Result<Option<PanelSnapshot>, String> {
            match (event.node_id, &event.event) {
                (10, Some(panel::ui_event::Event::Click(_))) => {
                    self.ev0_entered.notify_one();
                    self.rt.block_on(self.ev0_release.notified());
                    let rev = ctx.next_panel_revision();
                    return Ok(Some(Self::make_snap(
                        rev,
                        "stale-from-ev0",
                        1,
                        b"init".to_vec(),
                        ["Allow", "Deny"],
                    )));
                }
                (1, Some(panel::ui_event::Event::EditorChanged(ed))) => {
                    let rev = ctx.next_panel_revision();
                    return Ok(Some(Self::make_snap(
                        rev,
                        "v2-confirmed",
                        ed.revision,
                        ed.contents.clone(),
                        ["Allow", "Deny"],
                    )));
                }
                (20, Some(panel::ui_event::Event::SelectedOption(idx))) => {
                    self.seen_select_events.lock().unwrap().push(*idx);
                }
                _ => {}
            }
            Ok(None)
        }

        fn on_panel_resync(
            &mut self,
            _ctx: &mut dyn mio_plugin_sdk::HostClient,
            _panel_id: u64,
            snapshot: PanelSnapshot,
        ) -> std::result::Result<Option<PanelSnapshot>, String> {
            let status_text = snapshot
                .root
                .as_ref()
                .and_then(|r| r.find(2))
                .and_then(|n| match &n.kind {
                    Some(panel::node::Kind::Text(t)) => Some(t.text.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            if status_text == "v1-old" {
                self.resync1_entered.notify_one();
                self.rt.block_on(self.resync1_release.notified());
            }
            Ok(None)
        }

        fn on_panel_close_request(
            &mut self,
            ctx: &mut dyn mio_plugin_sdk::HostClient,
            _panel_id: u64,
            close: protocol::CloseRequest,
        ) -> std::result::Result<Option<PanelSnapshot>, String> {
            assert!(!close.destroy_panel);
            let rev = ctx.next_panel_revision();
            Ok(Some(Self::make_snap(
                rev,
                "v2-closed-reordered",
                2,
                b"user-edit".to_vec(),
                ["Deny", "Allow"],
            )))
        }
    }

    let rt_handle = tokio::runtime::Handle::current();
    let identity = crate::plugins::tests_runtime::identity();
    let manifest = PluginManifest {
        id: "lab.sdk-select-close-reorder".into(),
        version: "1.5.0".into(),
        api_version: 1,
        engine: "native".into(),
        entry: "worker".into(),
        permissions: vec![Permission::TrafficRead],
        contributions: vec!["suite_panel".into()],
    };
    let (host_tx, worker_rx) = tokio::io::duplex(64 * 1024);
    let (worker_tx, host_rx) = tokio::io::duplex(64 * 1024);
    let seen_select_events = Arc::new(Mutex::new(Vec::new()));
    let seen_select_events_w = seen_select_events.clone();
    let ev0_entered = Arc::new(tokio::sync::Notify::new());
    let ev0_release = Arc::new(tokio::sync::Notify::new());
    let resync1_entered = Arc::new(tokio::sync::Notify::new());
    let resync1_release = Arc::new(tokio::sync::Notify::new());
    let ev0_entered_w = ev0_entered.clone();
    let ev0_release_w = ev0_release.clone();
    let resync1_entered_w = resync1_entered.clone();
    let resync1_release_w = resync1_release.clone();

    let worker_thread = std::thread::spawn(move || {
        let mut plugin = SelectCloseReorderPlugin {
            seen_select_events: seen_select_events_w,
            ev0_entered: ev0_entered_w,
            ev0_release: ev0_release_w,
            resync1_entered: resync1_entered_w,
            resync1_release: resync1_release_w,
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
        BTreeSet::from([Permission::TrafficRead]),
        host_tx,
        host_rx,
    )
    .await
    .unwrap();
    let handle = supervisor.handle();
    let primary = Uuid::new_v4();
    let extra = Uuid::new_v4();

    let extract_status = |snap: &PanelSnapshot| -> String {
        snap.root
            .as_ref()
            .and_then(|r| r.find(2))
            .and_then(|n| match &n.kind {
                Some(panel::node::Kind::Text(t)) => Some(t.text.clone()),
                _ => None,
            })
            .unwrap_or_default()
    };

    let _ = timeout(MAX_OPERATION, supervisor.events.recv()).await.unwrap().unwrap();
    timeout(
        MAX_OPERATION,
        handle.open_request_on_panel(2, primary, 1, false, request()),
    )
    .await
    .unwrap()
    .unwrap();

    // 1. Enter ev0 and advance confirmed state to v2 ("Allow", "Deny") via EditorChanged.
    handle
        .try_event_on_panel(
            2,
            UiEvent {
                node_id: 10,
                editors: vec![],
                event: Some(panel::ui_event::Event::Click(panel::Empty {})),
            },
        )
        .unwrap();
    timeout(MAX_OPERATION, ev0_entered.notified()).await.unwrap();
    handle
        .try_event_on_panel(
            2,
            UiEvent {
                node_id: 1,
                editors: vec![],
                event: Some(panel::ui_event::Event::EditorChanged(panel::EditorSnapshot {
                    document_id: 1,
                    revision: 2,
                    contents: b"user-edit".to_vec(),
                    selection_start: 0,
                    selection_end: 0,
                })),
            },
        )
        .unwrap();

    // 2. Release ev0 -> triggers Resync(1), worker confirms v2-confirmed, then pauses in Resync(1).
    ev0_release.notify_one();
    timeout(MAX_OPERATION, resync1_entered.notified()).await.unwrap();
    timeout(MAX_OPERATION, async {
        loop {
            if let Some(PluginEvent::Panel { panel_id: 2, snapshot, .. }) =
                supervisor.events.recv().await
                && extract_status(&snapshot) == "v2-confirmed"
            {
                break;
            }
        }
    })
    .await
    .unwrap();

    // 3. While Resync is active:
    //    - Capture and defer `SelectedOption(0)` (meaning "Allow" on `v2-confirmed`)
    //    - Queue non-destroy `revoke_panel_resource(2, extra, false)` behind `Resync`
    handle
        .try_event_on_panel(
            2,
            UiEvent {
                node_id: 20,
                editors: vec![],
                event: Some(panel::ui_event::Event::SelectedOption(0)),
            },
        )
        .unwrap();
    let revoke_handle = handle.clone();
    let revoke_task = tokio::spawn(async move {
        revoke_handle.revoke_panel_resource(2, extra, false).await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    resync1_release.notify_one();

    // CloseRequest settles with snapshot reordering Select to ["Deny", "Allow"],
    // so deferred `SelectedOption(0)` MUST wait for CloseRequest AND be rejected as Stale!
    timeout(MAX_OPERATION, revoke_task).await.unwrap().unwrap().unwrap();
    timeout(MAX_OPERATION, async {
        loop {
            if let Some(PluginEvent::Error { panel_id: 2, failure, .. }) =
                supervisor.events.recv().await
                && failure.code == ErrorCode::Stale as i32
                && failure.message.contains("插件面板内容已更新")
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert!(
        seen_select_events.lock().unwrap().is_empty(),
        "deferred SelectedOption must NOT execute on worker after CloseRequest reordered Select options"
    );

    supervisor.stop().await.unwrap();
    worker_thread.join().unwrap();
}

#[tokio::test]
async fn child_headroom_blocked_rechecks_recovered_outgoing_capacity() {
    let (mut running, _calls, _updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

    let make_snap = |rev: u64| PanelSnapshot {
        revision: rev,
        title: format!("panel-r{rev}"),
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
    let open_wire = wire.try_recv().unwrap();
    let mut open_snap = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(make_snap(1)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    open_snap.in_reply_to = open_wire.request_id;
    running.frame(open_snap, &outgoing).await.unwrap();
    assert!(open_rx.await.is_ok());

    let identity = running.identity;
    let fill_to_reserved = |tx: &mpsc::Sender<Envelope>| {
        while tx.capacity() > RESERVED_CONTROL_OUTGOING {
            let env = Envelope::new(
                identity,
                0,
                1,
                envelope::Body::Ack(panel::Empty {}),
                MAX_OPERATION,
            );
            tx.try_send(env).unwrap();
        }
        assert_eq!(tx.capacity(), RESERVED_CONTROL_OUTGOING);
    };

    // Part A: deferred child command blocked on outgoing capacity sets `child_headroom_blocked`.
    fill_to_reserved(&outgoing);
    running
        .panels
        .get_mut(&2)
        .unwrap()
        .deferred_commands
        .push_back(SequencedControl {
            seq: 0,
            command: Control::Event {
                panel_id: 2,
                event: panel::UiEvent {
                    node_id: 1,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::Focus(true)),
                },
                expected_snapshot: None,
                event_seq: 0,
            },
        });
    running.child_headroom_blocked = false;
    running.flush_deferred_panel_commands(&outgoing).await.unwrap();
    assert_eq!(running.panels[&2].deferred_commands.len(), 1);
    assert!(running.child_headroom_blocked);

    // Writer drains 1 permit between flush and select! -> recheck condition triggers immediately.
    let _ = wire.try_recv().unwrap();
    assert!(running.child_headroom_blocked && outgoing.capacity() > RESERVED_CONTROL_OUTGOING);

    running.child_headroom_blocked = false;
    running.flush_deferred_panel_commands(&outgoing).await.unwrap();
    assert!(running.panels[&2].deferred_commands.is_empty());
    assert!(!running.child_headroom_blocked);
    while wire.try_recv().is_ok() {}

    // Part B: child pending Resync blocked on outgoing capacity also sets `child_headroom_blocked`.
    fill_to_reserved(&outgoing);
    running.schedule_panel_resync(2);
    running.child_headroom_blocked = false;
    running.flush_pending_resyncs(&outgoing).unwrap();
    assert!(running.is_panel_resyncing(2));
    assert!(running.child_headroom_blocked);

    let _ = wire.try_recv().unwrap();
    assert!(running.child_headroom_blocked && outgoing.capacity() > RESERVED_CONTROL_OUTGOING);

    running.child_headroom_blocked = false;
    running.flush_pending_resyncs(&outgoing).unwrap();
    assert!(!running.child_headroom_blocked);
    let mut saw_resync = false;
    while let Ok(env) = wire.try_recv() {
        if matches!(env.body, Some(envelope::Body::Resync(_))) {
            saw_resync = true;
        }
    }
    assert!(saw_resync);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_sdk_deferred_interactions_serialize_after_close_ack_and_reject_reordered_second_event() {
    use std::sync::{Arc, Mutex};


    struct SerializedInteractionsPlugin {
        seen_clicks: Arc<Mutex<Vec<u32>>>,
        seen_selects: Arc<Mutex<Vec<u32>>>,
        close_entered: Arc<tokio::sync::Notify>,
        close_release: Arc<tokio::sync::Notify>,
        rt: tokio::runtime::Handle,
    }

    impl SerializedInteractionsPlugin {
        fn make_snap(rev: u64, status: &str, opts: [&str; 2]) -> PanelSnapshot {
            PanelSnapshot {
                revision: rev,
                title: "sdk-close-ack-serialize".into(),
                root: Some(panel::Node::new(
                    3,
                    "col",
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
                                    text: status.to_owned(),
                                    tone: panel::Tone::Foreground as i32,
                                }),
                            ),
                            panel::Node::new(
                                11,
                                "btn-reorder",
                                panel::node::Kind::Button(panel::Button {
                                    label: "ReorderSelect".into(),
                                    enabled: true,
                                    commit_editors: false,
                                }),
                            ),
                            panel::Node::new(
                                20,
                                "action-select",
                                panel::node::Kind::Select(panel::SelectNode {
                                    label: "Action".into(),
                                    options: opts.iter().map(|s| (*s).to_owned()).collect(),
                                    selected_index: 1,
                                    enabled: true,
                                }),
                            ),
                        ],
                    }),
                )),
            }
        }
    }

    impl mio_plugin_sdk::MioPlugin for SerializedInteractionsPlugin {
        fn id(&self) -> &str {
            "lab.sdk-close-ack-serialize"
        }

        fn version(&self) -> &str {
            "1.5.0"
        }

        fn supports_panel_multiplexing(&self) -> bool {
            true
        }

        fn initial_panel(&self) -> PanelSnapshot {
            Self::make_snap(1, "main-init", ["Allow", "Deny"])
        }

        fn on_panel_open_request(
            &mut self,
            ctx: &mut dyn mio_plugin_sdk::HostClient,
            _panel_id: u64,
            _open: protocol::OpenRequest,
        ) -> std::result::Result<Option<PanelSnapshot>, String> {
            let rev = ctx.next_panel_revision();
            Ok(Some(Self::make_snap(rev, "v1-open", ["Allow", "Deny"])))
        }

        fn on_panel_close_request(
            &mut self,
            _ctx: &mut dyn mio_plugin_sdk::HostClient,
            _panel_id: u64,
            close: protocol::CloseRequest,
        ) -> std::result::Result<Option<PanelSnapshot>, String> {
            assert!(!close.destroy_panel);
            self.close_entered.notify_one();
            self.rt.block_on(self.close_release.notified());
            Ok(None)
        }

        fn on_panel_ui_event(
            &mut self,
            ctx: &mut dyn mio_plugin_sdk::HostClient,
            _panel_id: u64,
            event: UiEvent,
        ) -> std::result::Result<Option<PanelSnapshot>, String> {
            match (event.node_id, &event.event) {
                (11, Some(panel::ui_event::Event::Click(_))) => {
                    self.seen_clicks.lock().unwrap().push(11);
                    let rev = ctx.next_panel_revision();
                    return Ok(Some(Self::make_snap(
                        rev,
                        "v2-reordered",
                        ["Deny", "Allow"],
                    )));
                }
                (20, Some(panel::ui_event::Event::SelectedOption(idx))) => {
                    self.seen_selects.lock().unwrap().push(*idx);
                }
                _ => {}
            }
            Ok(None)
        }
    }

    let rt_handle = tokio::runtime::Handle::current();
    let identity = crate::plugins::tests_runtime::identity();
    let manifest = PluginManifest {
        id: "lab.sdk-close-ack-serialize".into(),
        version: "1.5.0".into(),
        api_version: 1,
        engine: "native".into(),
        entry: "worker".into(),
        permissions: vec![Permission::TrafficRead],
        contributions: vec!["suite_panel".into()],
    };
    let (host_tx, worker_rx) = tokio::io::duplex(64 * 1024);
    let (worker_tx, host_rx) = tokio::io::duplex(64 * 1024);
    let seen_clicks = Arc::new(Mutex::new(Vec::new()));
    let seen_selects = Arc::new(Mutex::new(Vec::new()));
    let close_entered = Arc::new(tokio::sync::Notify::new());
    let close_release = Arc::new(tokio::sync::Notify::new());
    let seen_clicks_w = seen_clicks.clone();
    let seen_selects_w = seen_selects.clone();
    let close_entered_w = close_entered.clone();
    let close_release_w = close_release.clone();

    let worker_thread = std::thread::spawn(move || {
        let mut plugin = SerializedInteractionsPlugin {
            seen_clicks: seen_clicks_w,
            seen_selects: seen_selects_w,
            close_entered: close_entered_w,
            close_release: close_release_w,
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
        BTreeSet::from([Permission::TrafficRead]),
        host_tx,
        host_rx,
    )
    .await
    .unwrap();
    let handle = supervisor.handle();
    let primary = Uuid::new_v4();
    let extra = Uuid::new_v4();

    let _ = timeout(MAX_OPERATION, supervisor.events.recv())
        .await
        .unwrap()
        .unwrap();
    timeout(
        MAX_OPERATION,
        handle.open_request_on_panel(2, primary, 1, false, request()),
    )
    .await
    .unwrap()
    .unwrap();

    // Start non-destroy CloseRequest -> worker pauses inside on_panel_close_request before Ack.
    let revoke_handle = handle.clone();
    let revoke_task = tokio::spawn(async move {
        revoke_handle.revoke_panel_resource(2, extra, false).await
    });
    timeout(MAX_OPERATION, close_entered.notified()).await.unwrap();

    // Queue TWO interactions on panel 2 behind the close barrier against `["Allow", "Deny"]`:
    // 1) Click(11) which will reorder Select options to ["Deny", "Allow"],
    // 2) SelectedOption(0) captured against the old ["Allow", "Deny"] view.
    handle
        .try_event_on_panel(
            2,
            UiEvent {
                node_id: 11,
                editors: vec![],
                event: Some(panel::ui_event::Event::Click(panel::Empty {})),
            },
        )
        .unwrap();
    handle
        .try_event_on_panel(
            2,
            UiEvent {
                node_id: 20,
                editors: vec![],
                event: Some(panel::ui_event::Event::SelectedOption(0)),
            },
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    close_release.notify_one();

    // Close Ack settles, then Click(11) ONLY runs -> returns reordered Snapshot ->
    // SelectedOption(0) MUST wait for Click(11)'s Snapshot and be rejected as Stale!
    timeout(MAX_OPERATION, revoke_task).await.unwrap().unwrap().unwrap();
    timeout(MAX_OPERATION, async {
        loop {
            if let Some(PluginEvent::Error { panel_id: 2, failure, .. }) =
                supervisor.events.recv().await
                && failure.code == ErrorCode::Stale as i32
                && failure.message.contains("插件面板内容已更新")
            {
                break;
            }
        }
    })
    .await
    .unwrap();

    supervisor.stop().await.unwrap();
    worker_thread.join().unwrap();
    assert_eq!(*seen_clicks.lock().unwrap(), vec![11]);
    assert!(
        seen_selects.lock().unwrap().is_empty(),
        "second deferred SelectedOption must NEVER run on SDK after first deferred interaction reordered options"
    );
}

#[tokio::test]
async fn single_capacity_sample_sets_child_headroom_blocked_when_writer_drains_mid_check() {
    let (mut running, _calls, _updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

    let make_snap = |rev: u64| PanelSnapshot {
        revision: rev,
        title: format!("panel-r{rev}"),
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
    let open_wire = wire.try_recv().unwrap();
    let mut open_snap = Envelope::new(
        running.identity,
        1,
        1,
        envelope::Body::Snapshot(make_snap(1)),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    open_snap.in_reply_to = open_wire.request_id;
    running.frame(open_snap, &outgoing).await.unwrap();
    assert!(open_rx.await.is_ok());

    let identity = running.identity;
    let fill_to_reserved = |tx: &mpsc::Sender<Envelope>| {
        while tx.capacity() > RESERVED_CONTROL_OUTGOING {
            let env = Envelope::new(
                identity,
                0,
                1,
                envelope::Body::Ack(panel::Empty {}),
                MAX_OPERATION,
            );
            tx.try_send(env).unwrap();
        }
        assert_eq!(tx.capacity(), RESERVED_CONTROL_OUTGOING);
    };

    // Part A: writer drains ONE permit immediately AFTER `flush_deferred_panel_commands_with`'s
    // single capacity sample -> must sample ONLY ONCE and still set `child_headroom_blocked = true`.
    fill_to_reserved(&outgoing);
    running
        .panels
        .get_mut(&2)
        .unwrap()
        .deferred_commands
        .push_back(SequencedControl {
            seq: 0,
            command: Control::Event {
                panel_id: 2,
                event: panel::UiEvent {
                    node_id: 1,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::Focus(true)),
                },
                expected_snapshot: None,
                event_seq: 0,
            },
        });
    running.child_headroom_blocked = false;
    let mut cmd_reads = 0usize;
    running
        .flush_deferred_panel_commands_with(&outgoing, |tx| {
            cmd_reads += 1;
            let cap = tx.capacity();
            if cmd_reads == 1 {
                let _ = wire.try_recv();
            }
            cap
        })
        .await
        .unwrap();
    assert_eq!(cmd_reads, 1);
    assert_eq!(running.panels[&2].deferred_commands.len(), 1);
    assert!(running.child_headroom_blocked);
    assert!(outgoing.capacity() > RESERVED_CONTROL_OUTGOING);

    running.child_headroom_blocked = false;
    running.flush_deferred_panel_commands(&outgoing).await.unwrap();
    assert!(running.panels[&2].deferred_commands.is_empty());
    assert!(!running.child_headroom_blocked);
    while wire.try_recv().is_ok() {}
    running.requests.clear();

    // Part B: identical mid-check writer drain during `flush_pending_resyncs_with`.
    fill_to_reserved(&outgoing);
    running.schedule_panel_resync(2);
    running.child_headroom_blocked = false;
    let mut resync_reads = 0usize;
    running
        .flush_pending_resyncs_with(&outgoing, |tx| {
            resync_reads += 1;
            let cap = tx.capacity();
            if resync_reads == 1 {
                let _ = wire.try_recv();
            }
            cap
        })
        .unwrap();
    assert_eq!(resync_reads, 1);
    assert!(running.is_panel_resyncing(2));
    assert!(running.child_headroom_blocked);
    assert!(outgoing.capacity() > RESERVED_CONTROL_OUTGOING);

    running.child_headroom_blocked = false;
    running.flush_pending_resyncs(&outgoing).unwrap();
    assert!(!running.child_headroom_blocked);
    let mut saw_resync = false;
    while let Ok(env) = wire.try_recv() {
        if matches!(env.body, Some(envelope::Body::Resync(_))) {
            saw_resync = true;
        }
    }
    assert!(saw_resync);
}

#[tokio::test]
async fn revoke_and_cancel_op_precede_deferred_flush_after_capacity_recovery() {
    let (mut running, _calls, _updates) = running();
    running.supports_panel_multiplexing = true;
    let (outgoing, mut wire) = mpsc::channel(OUTGOING_CAPACITY);

    let make_snap = |rev: u64| PanelSnapshot {
        revision: rev,
        title: format!("panel-r{rev}"),
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

    let open_child = |panel_id: u64, res: Uuid, seq: u64| {
        (panel_id, res, seq)
    };
    for &(panel_id, res, seq) in &[
        open_child(2, Uuid::new_v4(), 1),
        open_child(3, Uuid::new_v4(), 2),
        open_child(4, Uuid::new_v4(), 3),
    ] {
        let (open_tx, open_rx) = oneshot::channel();
        running
            .control(
                Control::Open {
                    panel_id,
                    id: res,
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
            seq,
            1,
            envelope::Body::Snapshot(make_snap(1)),
            MAX_OPERATION,
        )
        .with_panel_id(panel_id);
        snap.in_reply_to = ow.request_id;
        running.frame(snap, &outgoing).await.unwrap();
        assert!(open_rx.await.is_ok());
    }

    // Subcase A: panel 2 has deferred `Open(extra)` + `Event(Focus)` queued during capacity saturation.
    // Once capacity recovers AND `Control::Revoke(2, extra, false)` arrives before the next loop flush,
    // `Open(extra)` MUST be cancelled BEFORE any flush and MUST NEVER emit onto `wire`.
    let extra = Uuid::new_v4();
    let (open_extra_tx, mut open_extra_rx) = oneshot::channel();
    {
        let slot2 = running.panels.get_mut(&2).unwrap();
        slot2.deferred_commands.push_back(SequencedControl {
            seq: 0,
            command: Control::Open {
                panel_id: 2,
                id: extra,
                revision: 1,
                read_only: false,
                request: request(),
                reply: open_extra_tx,
            },
        });
        slot2.deferred_commands.push_back(SequencedControl {
            seq: 0,
            command: Control::Event {
                panel_id: 2,
                event: panel::UiEvent {
                    node_id: 1,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::Focus(true)),
                },
                expected_snapshot: None,
                event_seq: 0,
            },
        });
    }
    let (rev2_tx, mut rev2_rx) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: 2,
                id: extra,
                remove_panel: false,
                reply: rev2_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    assert!(open_extra_rx.try_recv().unwrap().is_err());
    let close2 = wire.try_recv().unwrap();
    assert_eq!(close2.panel_id, 2);
    assert!(matches!(
        close2.body,
        Some(envelope::Body::CloseRequest(ref c)) if !c.destroy_panel
    ));
    assert!(
        wire.try_recv().is_err(),
        "neither cancelled Open(extra) nor deferred UiEvent behind close may emit while CloseRequest is in flight"
    );
    let mut ack2 = Envelope::new(
        running.identity,
        10,
        1,
        envelope::Body::Ack(panel::Empty {}),
        MAX_OPERATION,
    )
    .with_panel_id(2);
    ack2.in_reply_to = close2.request_id;
    running.frame(ack2, &outgoing).await.unwrap();
    assert!(rev2_rx.try_recv().unwrap().is_ok());
    let focus2 = wire.try_recv().unwrap();
    assert_eq!(focus2.panel_id, 2);
    assert!(matches!(focus2.body, Some(envelope::Body::UiEvent(_))));

    // Subcase B: panel 3 has deferred `Event(Focus)` + pending resync; capacity recovers when
    // `Control::Revoke(3, res, true)` arrives -> ONLY `CloseRequest(destroy_panel=true)` emits!
    let (p3_res, _) = running.panels[&3].active_resource.unwrap();
    running
        .panels
        .get_mut(&3)
        .unwrap()
        .deferred_commands
        .push_back(SequencedControl {
            seq: 0,
            command: Control::Event {
                panel_id: 3,
                event: panel::UiEvent {
                    node_id: 1,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::Focus(true)),
                },
                expected_snapshot: None,
                event_seq: 0,
            },
        });
    running.schedule_panel_resync(3);
    let (rev3_tx, _rev3_rx) = oneshot::channel();
    running
        .control(
            Control::Revoke {
                panel_id: 3,
                id: p3_res,
                remove_panel: true,
                reply: rev3_tx,
            },
            &outgoing,
        )
        .await
        .unwrap();
    let close3 = wire.try_recv().unwrap();
    assert_eq!(close3.panel_id, 3);
    assert!(matches!(
        close3.body,
        Some(envelope::Body::CloseRequest(ref c)) if c.destroy_panel
    ));
    assert!(
        wire.try_recv().is_err(),
        "destroying revoke must emit neither deferred UiEvent nor Resync after capacity recovery"
    );
    assert!(running.panels[&3].deferred_commands.is_empty());

    // Subcase C: panel 4 has deferred `Control::Commit`; capacity recovers when
    // `Control::CancelPanelOp(4, op_id)` arrives -> cancelled BEFORE flush, zero `CommitEditor` emitted!
    let (p4_res, p4_msg_rev) = running.panels[&4].active_resource.unwrap();
    let op_id = Uuid::new_v4();
    let (commit_tx, mut commit_rx) = oneshot::channel();
    running
        .panels
        .get_mut(&4)
        .unwrap()
        .deferred_commands
        .push_back(SequencedControl {
            seq: 0,
            command: Control::Commit {
                panel_id: 4,
                op_id,
                request: CommitEditor {
                    resource_id: p4_res.as_bytes().to_vec(),
                    message_revision: p4_msg_rev,
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
        });
    running
        .control(Control::CancelPanelOp { panel_id: 4, op_id }, &outgoing)
        .await
        .unwrap();
    assert!(commit_rx.try_recv().unwrap().is_err());
    assert!(wire.try_recv().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_sdk_empty_deferred_queue_still_blocks_new_event_until_inflight_deferred_click_settles() {
    use std::sync::{Arc, Mutex};


    struct DeferredEntryBarrierPlugin {
        seen_clicks: Arc<Mutex<Vec<u32>>>,
        seen_selects: Arc<Mutex<Vec<u32>>>,
        close_entered: Arc<tokio::sync::Notify>,
        close_release: Arc<tokio::sync::Notify>,
        click_entered: Arc<tokio::sync::Notify>,
        click_release: Arc<tokio::sync::Notify>,
        rt: tokio::runtime::Handle,
    }

    impl DeferredEntryBarrierPlugin {
        fn make_snap(rev: u64, status: &str, opts: [&str; 2]) -> PanelSnapshot {
            PanelSnapshot {
                revision: rev,
                title: "sdk-entry-barrier".into(),
                root: Some(panel::Node::new(
                    3,
                    "col",
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
                                    text: status.to_owned(),
                                    tone: panel::Tone::Foreground as i32,
                                }),
                            ),
                            panel::Node::new(
                                11,
                                "btn-reorder",
                                panel::node::Kind::Button(panel::Button {
                                    label: "ReorderSelect".into(),
                                    enabled: true,
                                    commit_editors: false,
                                }),
                            ),
                            panel::Node::new(
                                20,
                                "action-select",
                                panel::node::Kind::Select(panel::SelectNode {
                                    label: "Action".into(),
                                    options: opts.iter().map(|s| (*s).to_owned()).collect(),
                                    selected_index: 1,
                                    enabled: true,
                                }),
                            ),
                        ],
                    }),
                )),
            }
        }
    }

    impl mio_plugin_sdk::MioPlugin for DeferredEntryBarrierPlugin {
        fn id(&self) -> &str {
            "lab.sdk-entry-barrier"
        }

        fn version(&self) -> &str {
            "1.5.0"
        }

        fn supports_panel_multiplexing(&self) -> bool {
            true
        }

        fn initial_panel(&self) -> PanelSnapshot {
            Self::make_snap(1, "main-init", ["Allow", "Deny"])
        }

        fn on_panel_open_request(
            &mut self,
            ctx: &mut dyn mio_plugin_sdk::HostClient,
            _panel_id: u64,
            _open: protocol::OpenRequest,
        ) -> std::result::Result<Option<PanelSnapshot>, String> {
            let rev = ctx.next_panel_revision();
            Ok(Some(Self::make_snap(rev, "v1-open", ["Allow", "Deny"])))
        }

        fn on_panel_close_request(
            &mut self,
            _ctx: &mut dyn mio_plugin_sdk::HostClient,
            _panel_id: u64,
            close: protocol::CloseRequest,
        ) -> std::result::Result<Option<PanelSnapshot>, String> {
            assert!(!close.destroy_panel);
            self.close_entered.notify_one();
            self.rt.block_on(self.close_release.notified());
            Ok(None)
        }

        fn on_panel_ui_event(
            &mut self,
            ctx: &mut dyn mio_plugin_sdk::HostClient,
            _panel_id: u64,
            event: UiEvent,
        ) -> std::result::Result<Option<PanelSnapshot>, String> {
            match (event.node_id, &event.event) {
                (11, Some(panel::ui_event::Event::Click(_))) => {
                    self.seen_clicks.lock().unwrap().push(11);
                    self.click_entered.notify_one();
                    self.rt.block_on(self.click_release.notified());
                    let rev = ctx.next_panel_revision();
                    return Ok(Some(Self::make_snap(
                        rev,
                        "v2-reordered",
                        ["Deny", "Allow"],
                    )));
                }
                (20, Some(panel::ui_event::Event::SelectedOption(idx))) => {
                    self.seen_selects.lock().unwrap().push(*idx);
                }
                _ => {}
            }
            Ok(None)
        }
    }

    let rt_handle = tokio::runtime::Handle::current();
    let identity = crate::plugins::tests_runtime::identity();
    let manifest = PluginManifest {
        id: "lab.sdk-entry-barrier".into(),
        version: "1.5.0".into(),
        api_version: 1,
        engine: "native".into(),
        entry: "worker".into(),
        permissions: vec![Permission::TrafficRead],
        contributions: vec!["suite_panel".into()],
    };
    let (host_tx, worker_rx) = tokio::io::duplex(64 * 1024);
    let (worker_tx, host_rx) = tokio::io::duplex(64 * 1024);
    let seen_clicks = Arc::new(Mutex::new(Vec::new()));
    let seen_selects = Arc::new(Mutex::new(Vec::new()));
    let close_entered = Arc::new(tokio::sync::Notify::new());
    let close_release = Arc::new(tokio::sync::Notify::new());
    let click_entered = Arc::new(tokio::sync::Notify::new());
    let click_release = Arc::new(tokio::sync::Notify::new());
    let seen_clicks_w = seen_clicks.clone();
    let seen_selects_w = seen_selects.clone();
    let close_entered_w = close_entered.clone();
    let close_release_w = close_release.clone();
    let click_entered_w = click_entered.clone();
    let click_release_w = click_release.clone();

    let worker_thread = std::thread::spawn(move || {
        let mut plugin = DeferredEntryBarrierPlugin {
            seen_clicks: seen_clicks_w,
            seen_selects: seen_selects_w,
            close_entered: close_entered_w,
            close_release: close_release_w,
            click_entered: click_entered_w,
            click_release: click_release_w,
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
        BTreeSet::from([Permission::TrafficRead]),
        host_tx,
        host_rx,
    )
    .await
    .unwrap();
    let handle = supervisor.handle();
    let primary = Uuid::new_v4();
    let extra = Uuid::new_v4();

    let _ = timeout(MAX_OPERATION, supervisor.events.recv())
        .await
        .unwrap()
        .unwrap();
    timeout(
        MAX_OPERATION,
        handle.open_request_on_panel(2, primary, 1, false, request()),
    )
    .await
    .unwrap()
    .unwrap();

    // 1. Pause non-destroy CloseRequest inside worker; queue ONLY ONE Click(11) behind it.
    let revoke_handle = handle.clone();
    let revoke_task = tokio::spawn(async move {
        revoke_handle.revoke_panel_resource(2, extra, false).await
    });
    timeout(MAX_OPERATION, close_entered.notified()).await.unwrap();
    handle
        .try_event_on_panel(
            2,
            UiEvent {
                node_id: 11,
                editors: vec![],
                event: Some(panel::ui_event::Event::Click(panel::Empty {})),
            },
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    // 2. Release CloseRequest -> Click(11) pops (deferred queue is now EMPTY!), enters worker and blocks.
    close_release.notify_one();
    timeout(MAX_OPERATION, revoke_task).await.unwrap().unwrap().unwrap();
    timeout(MAX_OPERATION, click_entered.notified()).await.unwrap();

    // 3. While deferred_commands is EMPTY and Click(11) (from_deferred_queue=true) is still in flight,
    //    send a fresh SelectedOption(0) -> MUST defer behind Click(11)'s entry barrier!
    handle
        .try_event_on_panel(
            2,
            UiEvent {
                node_id: 20,
                editors: vec![],
                event: Some(panel::ui_event::Event::SelectedOption(0)),
            },
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    click_release.notify_one();

    // 4. Click(11) returns reordered Select(["Deny", "Allow"]) -> SelectedOption(0) MUST fail Stale
    //    and never reach the SDK worker!
    timeout(MAX_OPERATION, async {
        loop {
            if let Some(PluginEvent::Error { panel_id: 2, failure, .. }) =
                supervisor.events.recv().await
                && failure.code == ErrorCode::Stale as i32
                && failure.message.contains("插件面板内容已更新")
            {
                break;
            }
        }
    })
    .await
    .unwrap();

    supervisor.stop().await.unwrap();
    worker_thread.join().unwrap();
    assert_eq!(*seen_clicks.lock().unwrap(), vec![11]);
    assert!(
        seen_selects.lock().unwrap().is_empty(),
        "new SelectedOption arriving while deferred Click was in flight must not overtake and run against reordered options"
    );
}
