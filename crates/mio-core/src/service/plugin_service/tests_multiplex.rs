use std::{collections::HashMap, fs, sync::Arc};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use mio_plugin_api::panel::{EditorSnapshot, PanelSnapshot};
use tokio::sync::mpsc;

use super::*;

type MultiplexStateSetup = (
    State,
    mpsc::Receiver<Event>,
    Uuid,
    Arc<std::sync::Mutex<Vec<(String, u64, Uuid)>>>,
    tempfile::TempDir,
);

fn setup_multiplex_state() -> MultiplexStateSetup {
    let dir = tempfile::tempdir().unwrap();
    let worker_path = dir.path().join("mux-worker");
    fs::write(&worker_path, [0xcf, 0xfa, 0xed, 0xfe]).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&worker_path, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        dir.path().join("manifest.json"),
        br#"{"id":"lab.multiplex","version":"1.5.0","api_version":1,"engine":"native","entry":"mux-worker","permissions":["traffic.read","draft.write"],"contributions":["suite_panel","message_editor","message_response_editor"]}"#,
    )
    .unwrap();
    let package = VerifiedPackage::read(dir.path()).unwrap();
    let grants = vec![Permission::TrafficRead, Permission::DraftWrite];
    let (events, receiver) = mpsc::channel(256);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: state.project,
    };
    let initial = Arc::new(PanelSnapshot {
        revision: 1,
        title: "lab.multiplex".into(),
        root: None,
    });
    let (worker, _event_tx, log) =
        crate::plugins::spawn_test_multiplex_supervisor(identity, package.manifest.clone(), initial.clone());
    let (_stx, services) = mpsc::channel(16);
    let info = LoadedPlugin {
        identity,
        directory: package.directory.clone(),
        manifest: package.manifest.clone(),
        grants,
        tab_id: None,
    };
    let m_mod = package.manifest_modified;
    let m_sz = package.manifest_bytes.len() as u64;
    let e_mod = package.modified;
    let e_sz = package.executable_size;
    let id = identity.instance_id;
    state.plugins.instances.insert(
        id,
        Instance {
            worker,
            services,
            package,
            manifest_modified: m_mod,
            manifest_size: m_sz,
            entry_modified: e_mod,
            entry_size: e_sz,
            auto_reload: false,
            info,
            panel: initial,
            resources: HashMap::new(),
        },
    );
    state.plugins.order.push(id);
    (state, receiver, id, log, dir)
}

async fn drain_op_collecting_stops(
    state: &mut State,
    rx: &mut mpsc::Receiver<Event>,
    op_id: Uuid,
    stopped: &mut Vec<Uuid>,
) -> Result<PluginOutcome, String> {
    loop {
        while let Ok(ev) = rx.try_recv() {
            match ev {
                Event::PluginStopped { identity, .. } => {
                    stopped.push(identity.instance_id);
                }
                Event::PluginOperation {
                    operation_id,
                    result,
                    ..
                } if operation_id == op_id => {
                    return result;
                }
                _ => {}
            }
        }
        let signal = state.plugins.next_signal().await;
        state.plugin_signal(signal).await;
    }
}

async fn drain_op(
    state: &mut State,
    rx: &mut mpsc::Receiver<Event>,
    op_id: Uuid,
) -> Result<PluginOutcome, String> {
    let mut ignored = Vec::new();
    drain_op_collecting_stops(state, rx, op_id, &mut ignored).await
}

fn add_tab(state: &mut State, label: &str, url: &str) -> (TabId, u64) {
    let mut ws = state.store.workspace().unwrap();
    let req = parse_request(
        b"GET / HTTP/1.1\r\nHost: example.invalid\r\n\r\n",
        url,
        HttpVersion::Http11,
    )
    .unwrap();
    let tab = WorkTab::from_request(Some(&req), label.into());
    let id = tab.id;
    let rev = tab.draft.revision;
    ws.tabs.push(tab);
    ws.active = Some(id);
    ws.revision += 1;
    state.store.save_workspace(&ws).unwrap();
    (id, rev)
}

#[tokio::test]
async fn v15_multiplexed_children_share_one_worker_and_isolate_panels_and_disable() {
    let (mut state, mut rx, parent_id, log, _dir) = setup_multiplex_state();
    let (tab1, rev1) = add_tab(&mut state, "tab1", "http://127.0.0.1/t1");
    let (tab2, rev2) = add_tab(&mut state, "tab2", "http://127.0.0.1/t2");

    let op1 = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            op1,
            PluginAction::CreateEditor {
                source_instance: parent_id,
                tab_id: tab1,
                draft_revision: rev1,
            },
        )
        .await
        .unwrap();
    let PluginOutcome::Loaded {
        info: info1,
        panel: panel1,
    } = drain_op(&mut state, &mut rx, op1).await.unwrap()
    else {
        panic!("expected Loaded for tab1");
    };
    assert_eq!(info1.tab_id, Some(tab1));
    assert_ne!(info1.identity.instance_id, parent_id);
    assert_eq!(panel1.title, "panel-2");

    let op2 = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            op2,
            PluginAction::CreateEditor {
                source_instance: parent_id,
                tab_id: tab2,
                draft_revision: rev2,
            },
        )
        .await
        .unwrap();
    let PluginOutcome::Loaded {
        info: info2,
        panel: panel2,
    } = drain_op(&mut state, &mut rx, op2).await.unwrap()
    else {
        panic!("expected Loaded for tab2");
    };
    assert_eq!(info2.tab_id, Some(tab2));
    assert_ne!(info2.identity.instance_id, info1.identity.instance_id);
    assert_eq!(panel2.title, "panel-3");

    // Also open context viewer & response viewer on the same single worker.
    let ctx_id = Uuid::new_v4();
    let op_ctx = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            op_ctx,
            PluginAction::OpenContextViewer {
                source_instance: parent_id,
                context_id: ctx_id,
                revision: 1,
                read_only: true,
                request: parse_request(
                    b"GET /ctx HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
                    "http://127.0.0.1/ctx",
                    HttpVersion::Http11,
                )
                .unwrap(),
            },
        )
        .await
        .unwrap();
    let PluginOutcome::ContextOpened { info: ctx_info, .. } =
        drain_op(&mut state, &mut rx, op_ctx).await.unwrap()
    else {
        panic!("expected ContextOpened");
    };

    let resp_id = Uuid::new_v4();
    let op_resp = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            op_resp,
            PluginAction::OpenResponseViewer {
                source_instance: parent_id,
                context_id: resp_id,
                revision: 1,
                read_only: false,
                response: HttpResponse {
                    status: 200,
                    version: HttpVersion::Http11,
                    reason_phrase: Some("OK".into()),
                    headers: vec![],
                    body: b"hello".to_vec(),
                    trailers: vec![],
                },
            },
        )
        .await
        .unwrap();
    let PluginOutcome::ResponseOpened {
        info: resp_info, ..
    } = drain_op(&mut state, &mut rx, op_resp).await.unwrap()
    else {
        panic!("expected ResponseOpened");
    };

    // Only ONE parent instance exists; all 4 children share it via `multiplexed`.
    assert_eq!(state.plugins.instances.len(), 1);
    assert_eq!(state.plugins.multiplexed.len(), 4);

    // CommitEditor on tab2 hits panel 3 only; CommitResponseEditor hits panel 5 only.
    let commit_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            commit_op,
            PluginAction::CommitEditor {
                instance_id: info2.identity.instance_id,
                tab_id: tab2,
                draft_revision: rev2,
                editor: EditorSnapshot {
                    document_id: 1,
                    revision: 1,
                    contents: b"edited".to_vec(),
                    ..Default::default()
                },
                            expected_panel: None,
            },
        )
        .await
        .unwrap();
    let PluginOutcome::Committed { request, .. } =
        drain_op(&mut state, &mut rx, commit_op).await.unwrap()
    else {
        panic!("expected Committed");
    };
    assert!(
        request
            .headers
            .iter()
            .any(|h| h.name == "x-panel-id" && h.value == b"3")
    );

    let commit_resp_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            commit_resp_op,
            PluginAction::CommitResponseEditor {
                instance_id: resp_info.identity.instance_id,
                context_id: resp_id,
                revision: 1,
                response: HttpResponse {
                    status: 200,
                    version: HttpVersion::Http11,
                    reason_phrase: Some("OK".into()),
                    headers: vec![],
                    body: b"hello".to_vec(),
                    trailers: vec![],
                },
                editor: EditorSnapshot {
                    document_id: 1,
                    revision: 1,
                    contents: b"resp-edited".to_vec(),
                    ..Default::default()
                },
                            expected_panel: None,
            },
        )
        .await
        .unwrap();
    let PluginOutcome::ResponseCommitted { response, .. } =
        drain_op(&mut state, &mut rx, commit_resp_op).await.unwrap()
    else {
        panic!("expected ResponseCommitted");
    };
    assert!(
        response
            .headers
            .iter()
            .any(|h| h.name == "x-panel-id" && h.value == b"5")
    );

    // Disabling child 1 removes only child 1 and revokes panel 2; siblings & parent live on.
    let dis_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            dis_op,
            PluginAction::Disable {
                instance_id: info1.identity.instance_id,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        drain_op(&mut state, &mut rx, dis_op).await.unwrap(),
        PluginOutcome::Disabled { identity } if identity == info1.identity
    ));
    assert_eq!(state.plugins.instances.len(), 1);
    assert_eq!(state.plugins.multiplexed.len(), 3);
    assert!(
        log.lock()
            .unwrap()
            .iter()
            .any(|(kind, p, id)| kind == "revoke:true" && *p == 2 && *id == tab1)
    );

    // Disabling parent retires remaining multiplexed children and emits PluginStopped for each.
    let parent_dis = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            parent_dis,
            PluginAction::Disable {
                instance_id: parent_id,
            },
        )
        .await
        .unwrap();
    let mut stopped_ids = Vec::new();
    assert!(matches!(
        drain_op_collecting_stops(&mut state, &mut rx, parent_dis, &mut stopped_ids)
            .await
            .unwrap(),
        PluginOutcome::Disabled { .. }
    ));
    assert!(state.plugins.instances.is_empty());
    assert!(state.plugins.multiplexed.is_empty());
    while let Ok(ev) = rx.try_recv() {
        if let Event::PluginStopped { identity, .. } = ev {
            stopped_ids.push(identity.instance_id);
        }
    }
    assert!(stopped_ids.contains(&parent_id));
    assert!(stopped_ids.contains(&info2.identity.instance_id));
    assert!(stopped_ids.contains(&ctx_info.identity.instance_id));
    assert!(stopped_ids.contains(&resp_info.identity.instance_id));
}

#[tokio::test]
async fn suite_instance_ceiling_and_child_editor_budget_are_independent() {
    let (mut state, mut rx, first_parent, _log, dir) = setup_multiplex_state();
    let mut filler_guards = Vec::new();
    // Fill remaining 15 suite slots to reach MAX_INSTANCES (16).
    for idx in 1..MAX_INSTANCES {
        let pkg = VerifiedPackage::read(dir.path()).unwrap();
        let grants = vec![Permission::TrafficRead];
        let _ = NativeTrust::confirmed(&pkg, grants.clone()).unwrap();
        let identity = ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: state.project,
        };
        let panel = Arc::new(PanelSnapshot {
            revision: 1,
            title: format!("suite-{idx}"),
            root: None,
        });
        let (worker, tx) = crate::plugins::test_supervisor(identity, pkg.manifest.clone(), panel.clone());
        let (stx, services) = mpsc::channel(4);
        filler_guards.push((tx, stx));
        let m_mod = pkg.manifest_modified;
        let m_sz = pkg.manifest_bytes.len() as u64;
        let e_mod = pkg.modified;
        let e_sz = pkg.executable_size;
        state.plugins.instances.insert(
            identity.instance_id,
            Instance {
                worker,
                services,
                package: pkg,
                manifest_modified: m_mod,
                manifest_size: m_sz,
                entry_modified: e_mod,
                entry_size: e_sz,
                auto_reload: false,
                info: LoadedPlugin {
                    identity,
                    directory: dir.path().to_path_buf(),
                    manifest: VerifiedPackage::read(dir.path()).unwrap().manifest,
                    grants,
                    tab_id: None,
                },
                panel,
                resources: HashMap::new(),
            },
        );
    }
    assert_eq!(state.plugins.instances.len(), MAX_INSTANCES);
    assert!(
        state.plugin_capacity().is_err(),
        "17th suite instance must still hit MAX_INSTANCES = 16"
    );

    // Child editor creation uses the independent MAX_CHILD_EDITORS (64) budget without double-counting,
    // even when all 64 CreateEditor commands are in-flight concurrently before draining.
    let mut pending_ops = Vec::with_capacity(MAX_CHILD_EDITORS);
    for idx in 0..MAX_CHILD_EDITORS {
        let (tab_id, draft_revision) = add_tab(&mut state, &format!("t-{idx}"), "http://127.0.0.1/");
        let op = Uuid::new_v4();
        state
            .plugin_command(
                state.project,
                op,
                PluginAction::CreateEditor {
                    source_instance: first_parent,
                    tab_id,
                    draft_revision,
                },
            )
            .await
            .unwrap();
        pending_ops.push(op);
    }
    assert_eq!(state.plugins.multiplexed.len(), MAX_CHILD_EDITORS);
    assert_eq!(state.plugins.instances.len(), MAX_INSTANCES);

    // 65th concurrent child editor hits the 64 child limit cleanly without affecting parent workers.
    let (extra_tab, extra_rev) = add_tab(&mut state, "t-over", "http://127.0.0.1/");
    let over_op = Uuid::new_v4();
    let over_err = state
        .plugin_command(
            state.project,
            over_op,
            PluginAction::CreateEditor {
                source_instance: first_parent,
                tab_id: extra_tab,
                draft_revision: extra_rev,
            },
        )
        .await;
    assert!(over_err.is_err());
    assert_eq!(state.plugins.instances.len(), MAX_INSTANCES);

    let mut created_ids = Vec::with_capacity(MAX_CHILD_EDITORS);
    for op in pending_ops {
        let PluginOutcome::Loaded { info, .. } = drain_op(&mut state, &mut rx, op).await.unwrap()
        else {
            panic!("expected Loaded for concurrent child op {op}");
        };
        created_ids.push(info.identity.instance_id);
    }
    assert_eq!(state.plugins.multiplexed.len(), MAX_CHILD_EDITORS);

    // Enqueue Disable without draining yet: closing slot keeps occupying capacity until Disable completes.
    let dis_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            dis_op,
            PluginAction::Disable {
                instance_id: created_ids[0],
            },
        )
        .await
        .unwrap();
    assert_eq!(state.plugins.closing_panels.len(), 1);
    let during_close_op = Uuid::new_v4();
    let during_close_err = state
        .plugin_command(
            state.project,
            during_close_op,
            PluginAction::CreateEditor {
                source_instance: first_parent,
                tab_id: extra_tab,
                draft_revision: extra_rev,
            },
        )
        .await;
    assert!(
        during_close_err.is_err(),
        "closing child slot must reserve capacity until Disable settles"
    );

    assert!(drain_op(&mut state, &mut rx, dis_op).await.is_ok());
    assert_eq!(state.plugins.multiplexed.len(), MAX_CHILD_EDITORS - 1);
    assert!(state.plugins.closing_panels.is_empty());

    let replace_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            replace_op,
            PluginAction::CreateEditor {
                source_instance: first_parent,
                tab_id: extra_tab,
                draft_revision: extra_rev,
            },
        )
        .await
        .unwrap();
    assert!(drain_op(&mut state, &mut rx, replace_op).await.is_ok());
    assert_eq!(state.plugins.multiplexed.len(), MAX_CHILD_EDITORS);
}

#[tokio::test]
async fn child_panel_extra_repeater_resources_revoke_when_extra_tab_closes() {
    let (mut state, mut rx, parent_id, log, _dir) = setup_multiplex_state();
    let (host_tab, rev) = add_tab(&mut state, "host-tab", "http://127.0.0.1/host");
    let (extra_tab, extra_rev) = add_tab(&mut state, "extra-tab", "http://127.0.0.1/extra");

    let op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            op,
            PluginAction::CreateEditor {
                source_instance: parent_id,
                tab_id: host_tab,
                draft_revision: rev,
            },
        )
        .await
        .unwrap();
    let PluginOutcome::Loaded { info, .. } = drain_op(&mut state, &mut rx, op).await.unwrap()
    else {
        panic!("expected child Loaded");
    };
    let child_id = info.identity.instance_id;
    let panel_id = state.plugins.multiplexed[&child_id].panel_id;
    state
        .plugins
        .multiplexed
        .get_mut(&child_id)
        .unwrap()
        .resources
        .insert(extra_tab, extra_rev);

    // Close extra_tab while host_tab remains open.
    let mut ws = state.store.workspace().unwrap();
    ws.tabs.retain(|t| t.id != extra_tab);
    ws.active = Some(host_tab);
    ws.revision += 1;
    state.store.save_workspace(&ws).unwrap();
    state.plugin_workspace_changed(&ws).await;

    assert!(!state.plugins.multiplexed[&child_id].resources.contains_key(&extra_tab));
    assert!(state.plugins.multiplexed[&child_id].resources.contains_key(&host_tab));
    while !state.plugins.operations.is_empty() {
        let sig = state.plugins.next_signal().await;
        state.plugin_signal(sig).await;
    }
    assert!(
        log.lock()
            .unwrap()
            .iter()
            .any(|(kind, p, id)| kind == "revoke:false" && *p == panel_id && *id == extra_tab),
        "closing child-created extra tab must emit revoke:false on child panel"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_running_multiplex_worker_context_viewer_survives_workspace_save_and_propagates_close_failure()
{
    use std::io::{Read, Write};
    use std::collections::BTreeSet;
    use mio_plugin_api::{
        panel::{self, Editor, EditorFormat, Empty, PanelSnapshot, node},
        protocol::{
            self, CloseRequest, Committed, ErrorCode, Failure, Register, WorkerSession, envelope,
        },
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

    struct BlockingReader(DuplexStream, tokio::runtime::Handle);
    impl Read for BlockingReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.1.block_on(self.0.read(buf))
        }
    }
    struct BlockingWriter(DuplexStream, tokio::runtime::Handle);
    impl Write for BlockingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.1.block_on(self.0.write(buf))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.1.block_on(self.0.flush())
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let worker_path = dir.path().join("real-mux-worker");
    fs::write(&worker_path, [0xcf, 0xfa, 0xed, 0xfe]).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&worker_path, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        dir.path().join("manifest.json"),
        br#"{"id":"lab.real-mux","version":"1.5.0","api_version":1,"engine":"native","entry":"real-mux-worker","permissions":["traffic.read","draft.write","repeater.open"],"contributions":["suite_panel","message_editor","message_response_editor"]}"#,
    )
    .unwrap();
    let package = VerifiedPackage::read(dir.path()).unwrap();
    let grants = vec![
        Permission::TrafficRead,
        Permission::DraftWrite,
        Permission::RepeaterOpen,
    ];
    let grants_set: BTreeSet<Permission> = grants.iter().copied().collect();
    let _ = NativeTrust::confirmed(&package, grants.clone()).unwrap();
    let (events, mut rx) = mpsc::channel(256);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: state.project,
    };

    let (host_tx, worker_rx) = tokio::io::duplex(64 * 1024);
    let (worker_tx, host_rx) = tokio::io::duplex(64 * 1024);
    let rt_handle = tokio::runtime::Handle::current();
    let close_events = Arc::new(std::sync::Mutex::new(Vec::<(u64, bool)>::new()));
    let close_events_worker = close_events.clone();
    let manifest_for_worker = package.manifest.clone();

    let worker_thread = std::thread::spawn(move || {
        let reg = Register {
            plugin_id: manifest_for_worker.id,
            plugin_version: manifest_for_worker.version,
            required_components: vec![],
            panel: Some(PanelSnapshot {
                revision: 1,
                title: "real-mux".into(),
                root: Some(panel::Node::new(
                    1,
                    "root",
                    node::Kind::Text(panel::Text {
                        text: "ready".into(),
                        tone: 0,
                    }),
                )),
            }),
            supports_panel_multiplexing: true,
        };
        let mut session = WorkerSession::accept(
            BlockingReader(worker_rx, rt_handle.clone()),
            BlockingWriter(worker_tx, rt_handle),
            reg,
        )
        .unwrap();
        assert_eq!(session.negotiated_minor(), protocol::PROTOCOL_MINOR);
        let mut revs: HashMap<u64, u64> = HashMap::new();
        while let Ok(env) = session.receive() {
            let reply_to = env.request_id;
            let pid = env.panel_id;
            let Some(body) = env.body else { break };
            match body {
                envelope::Body::Shutdown(_) => break,
                envelope::Body::OpenRequest(_) | envelope::Body::OpenResponse(_) => {
                    let rev = revs.entry(pid).or_insert(0);
                    *rev += 1;
                    let snap = PanelSnapshot {
                        revision: *rev,
                        title: format!("panel-{pid}"),
                        root: Some(panel::Node::new(
                            1,
                            "ed",
                            node::Kind::Editor(Editor {
                                document_id: 1,
                                revision: 1,
                                contents: format!("p-{pid}").into_bytes(),
                                format: EditorFormat::Utf8 as i32,
                                read_only: false,
                            }),
                        )),
                    };
                    session
                        .send_on_panel(pid, envelope::Body::Snapshot(snap), *rev, reply_to)
                        .unwrap();
                }
                envelope::Body::CommitEditor(c) => {
                    let rev = *revs.get(&pid).unwrap_or(&1);
                    let mut req = c.request.unwrap();
                    req.headers.push(protocol::HeaderDto {
                        name: "x-real-worker-panel".into(),
                        value: pid.to_string().into_bytes(),
                    });
                    session
                        .send_on_panel(
                            pid,
                            envelope::Body::Committed(Committed {
                                resource_id: c.resource_id,
                                message_revision: c.message_revision,
                                document_id: 1,
                                document_revision: 1,
                                request: Some(req),
                            }),
                            rev,
                            reply_to,
                        )
                        .unwrap();
                }
                envelope::Body::UiEvent(_) => {
                    let rev = *revs.get(&pid).unwrap_or(&1);
                    let call_id = session
                        .send_on_panel(
                            pid,
                            envelope::Body::HostCall(protocol::HostCall {
                                operation: Some(protocol::host_call::Operation::OpenRepeater(
                                    protocol::HttpRequestDto {
                                        method: "GET".into(),
                                        url: "http://127.0.0.1/extra-from-child".into(),
                                        version: protocol::HttpVersionDto::Http11 as i32,
                                        headers: vec![],
                                        body: b"extra".to_vec(),
                                        trailers: vec![],
                                    },
                                )),
                                parent_request_id: reply_to,
                            }),
                            rev,
                            0,
                        )
                        .unwrap();
                    loop {
                        let incoming = session.receive().unwrap();
                        if incoming.in_reply_to == call_id
                            && matches!(incoming.body, Some(envelope::Body::HostResult(_)))
                        {
                            break;
                        }
                    }
                    let next_rev = revs.entry(pid).or_insert(0);
                    *next_rev += 1;
                    let snap = PanelSnapshot {
                        revision: *next_rev,
                        title: format!("panel-{pid}-opened"),
                        root: Some(panel::Node::new(
                            1,
                            "ed",
                            node::Kind::Editor(Editor {
                                document_id: 1,
                                revision: 1,
                                contents: format!("p-{pid}").into_bytes(),
                                format: EditorFormat::Utf8 as i32,
                                read_only: false,
                            }),
                        )),
                    };
                    session
                        .send_on_panel(pid, envelope::Body::Snapshot(snap), *next_rev, reply_to)
                        .unwrap();
                }
                envelope::Body::CloseRequest(CloseRequest { destroy_panel, .. }) => {
                    close_events_worker.lock().unwrap().push((pid, destroy_panel));
                    let rev = *revs.get(&pid).unwrap_or(&1);
                    // Reject destroy_panel on panel 4 (response viewer) to verify error propagation.
                    if pid == 4 && destroy_panel {
                        session
                            .send_on_panel(
                                pid,
                                envelope::Body::Error(Failure::new(
                                    ErrorCode::Invalid,
                                    "cannot close response viewer",
                                )),
                                rev,
                                reply_to,
                            )
                            .unwrap();
                    } else {
                        if destroy_panel {
                            revs.remove(&pid);
                        }
                        session
                            .send_on_panel(pid, envelope::Body::Ack(Empty {}), rev, reply_to)
                            .unwrap();
                    }
                }
                _ => {}
            }
        }
    });

    let (worker, services) = crate::plugins::spawn_test_duplex_supervisor(
        identity,
        package.manifest.clone(),
        grants_set,
        host_tx,
        host_rx,
    )
    .await
    .unwrap();
    let parent_id = identity.instance_id;
    let initial = worker.initial_panel.clone();
    let info = LoadedPlugin {
        identity,
        directory: package.directory.clone(),
        manifest: package.manifest.clone(),
        grants,
        tab_id: None,
    };
    state.plugins.instances.insert(
        parent_id,
        Instance {
            worker,
            services,
            package: package.clone(),
            manifest_modified: package.manifest_modified,
            manifest_size: package.manifest_bytes.len() as u64,
            entry_modified: package.modified,
            entry_size: package.executable_size,
            auto_reload: false,
            info,
            panel: initial,
            resources: HashMap::new(),
        },
    );
    state.plugins.order.push(parent_id);

    let (host_tab, host_rev) = add_tab(&mut state, "host", "http://127.0.0.1/host");
    let op_rep = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            op_rep,
            PluginAction::CreateEditor {
                source_instance: parent_id,
                tab_id: host_tab,
                draft_revision: host_rev,
            },
        )
        .await
        .unwrap();
    let PluginOutcome::Loaded { info: rep_info, .. } =
        drain_op(&mut state, &mut rx, op_rep).await.unwrap()
    else {
        panic!("expected repeater child Loaded");
    };
    let rep_id = rep_info.identity.instance_id;
    let rep_panel = state.plugins.multiplexed[&rep_id].panel_id;

    let ctx_id = Uuid::new_v4();
    let op_ctx = Uuid::new_v4();
    let req = parse_request(
        b"GET /ctx HTTP/1.1\r\nHost: example.invalid\r\n\r\n",
        "http://example.invalid/ctx",
        HttpVersion::Http11,
    )
    .unwrap();
    state
        .plugin_command(
            state.project,
            op_ctx,
            PluginAction::OpenContextViewer {
                source_instance: parent_id,
                context_id: ctx_id,
                revision: 1,
                read_only: false,
                request: req.clone(),
            },
        )
        .await
        .unwrap();
    let PluginOutcome::ContextOpened {
        info: ctx_loaded, ..
    } = drain_op(&mut state, &mut rx, op_ctx).await.unwrap()
    else {
        panic!("expected ContextOpened");
    };
    let ctx_inst = ctx_loaded.identity.instance_id;
    let ctx_panel = state.plugins.multiplexed[&ctx_inst].panel_id;

    let resp_ctx_id = Uuid::new_v4();
    let op_resp = Uuid::new_v4();
    let resp = HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: Some("OK".into()),
        headers: vec![],
        body: b"resp".to_vec(),
        trailers: vec![],
    };
    state
        .plugin_command(
            state.project,
            op_resp,
            PluginAction::OpenResponseViewer {
                source_instance: parent_id,
                context_id: resp_ctx_id,
                revision: 1,
                read_only: true,
                response: resp,
            },
        )
        .await
        .unwrap();
    let PluginOutcome::ResponseOpened {
        info: resp_loaded, ..
    } = drain_op(&mut state, &mut rx, op_resp).await.unwrap()
    else {
        panic!("expected ResponseOpened");
    };
    let resp_inst = resp_loaded.identity.instance_id;
    let resp_panel = state.plugins.multiplexed[&resp_inst].panel_id;
    assert_eq!(resp_panel, 4);

    // Trigger OpenRepeater via a REAL child HostCall end-to-end through workspace proposal + ApplyWorkspace,
    // keeping standalone context/response viewers intact across workspace saves.
    let rep_panel_rev = state.plugins.multiplexed[&rep_id].panel.revision;
    let ui_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            ui_op,
            PluginAction::Ui {
                instance_id: rep_id,
                panel_revision: rep_panel_rev,
                event_seq: 0,
                event: mio_plugin_api::panel::UiEvent {
                    node_id: 1,
                    editors: vec![],
                    event: Some(mio_plugin_api::panel::ui_event::Event::Focus(true)),
                },
            },
        )
        .await
        .unwrap();
    let ws_req = loop {
        let mut found = None;
        while let Ok(ev) = rx.try_recv() {
            if let Event::PluginWorkspaceRequest(req) = ev {
                found = Some(req);
                break;
            }
        }
        if let Some(req) = found {
            break req;
        }
        let sig = state.plugins.next_signal().await;
        state.plugin_signal(sig).await;
    };
    let PluginWorkspaceEdit::OpenRepeater { request: open_req } = ws_req.edit else {
        panic!("expected OpenRepeater workspace request");
    };
    let extra_tab_entry = WorkTab::from_request(Some(&open_req), "extra-from-child".into());
    let extra_tab = extra_tab_entry.id;
    let mut proposal = state.store.workspace().unwrap();
    proposal.revision += 1;
    proposal.tabs.push(extra_tab_entry);
    proposal.active = Some(extra_tab);
    let apply_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            apply_op,
            PluginAction::ApplyWorkspace {
                instance_id: parent_id,
                request_id: ws_req.request_id,
                workspace: proposal.clone(),
                tab_id: extra_tab,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        drain_op(&mut state, &mut rx, apply_op).await.unwrap(),
        PluginOutcome::WorkspaceApplied
    ));
    assert_eq!(
        state.plugins.multiplexed[&rep_id]
            .resources
            .get(&extra_tab),
        Some(&1)
    );
    while state.plugins.multiplexed[&rep_id].panel.title != format!("panel-{rep_panel}-opened") {
        let sig = state.plugins.next_signal().await;
        state.plugin_signal(sig).await;
    }

    state.plugin_workspace_changed(&proposal).await;
    assert!(state.plugins.multiplexed[&ctx_inst].resources.contains_key(&ctx_id));
    assert!(
        state.plugins.multiplexed[&resp_inst]
            .resources
            .contains_key(&resp_ctx_id)
    );

    // Committing on context viewer AFTER workspace save STILL succeeds over real Running worker!
    let op_commit = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            op_commit,
            PluginAction::CommitContextEditor {
                instance_id: ctx_inst,
                context_id: ctx_id,
                revision: 1,
                request: req,
                editor: EditorSnapshot {
                    document_id: 1,
                    revision: 1,
                    contents: format!("p-{ctx_panel}").into_bytes(),
                    ..Default::default()
                },
                            expected_panel: None,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        drain_op(&mut state, &mut rx, op_commit).await.unwrap(),
        PluginOutcome::ContextCommitted { request, .. }
            if request.headers.iter().any(|h| h.name == "x-real-worker-panel")
    ));

    // Closing extra_tab bound end-to-end to rep_id emits destroy_panel = false while keeping rep_id alive.
    let mut ws_after = proposal;
    ws_after.tabs.retain(|t| t.id != extra_tab);
    ws_after.active = Some(host_tab);
    ws_after.revision += 1;
    state.store.save_workspace(&ws_after).unwrap();
    state.plugin_workspace_changed(&ws_after).await;
    while !state.plugins.operations.is_empty() {
        let sig = state.plugins.next_signal().await;
        state.plugin_signal(sig).await;
    }
    assert!(state.plugins.multiplexed.contains_key(&rep_id));

    // Disable context viewer -> destroy_panel = true succeeds cleanly.
    let dis_ctx = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            dis_ctx,
            PluginAction::Disable {
                instance_id: ctx_inst,
            },
        )
        .await
        .unwrap();
    assert!(drain_op(&mut state, &mut rx, dis_ctx).await.is_ok());

    // Disable response viewer (panel 4) -> worker rejects CloseRequest -> error must NOT be swallowed,
    // and closing_panels retains (parent_id, resp_panel) tombstone until parent worker stops.
    let dis_resp = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            dis_resp,
            PluginAction::Disable {
                instance_id: resp_inst,
            },
        )
        .await
        .unwrap();
    let mut stops = Vec::new();
    let dis_resp_res = drain_op_collecting_stops(&mut state, &mut rx, dis_resp, &mut stops).await;
    assert!(
        matches!(dis_resp_res, Err(ref msg) if msg.contains("cannot close response viewer")),
        "expected propagated close failure, got {dis_resp_res:?}"
    );
    assert!(stops.contains(&resp_inst));
    assert!(!state.plugins.multiplexed.contains_key(&resp_inst));
    assert!(state.plugins.closing_panels.contains(&(parent_id, resp_panel)));

    let seen = close_events.lock().unwrap().clone();
    assert!(seen.contains(&(rep_panel, false)));
    assert!(seen.contains(&(ctx_panel, true)));
    assert!(seen.contains(&(resp_panel, true)));

    let dis_parent = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            dis_parent,
            PluginAction::Disable {
                instance_id: parent_id,
            },
        )
        .await
        .unwrap();
    assert!(drain_op(&mut state, &mut rx, dis_parent).await.is_ok());
    assert!(state.plugins.closing_panels.is_empty());
    worker_thread.join().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_multiplex_init_propagates_close_error_diagnostic_and_keeps_closing_quota() {
    use std::collections::BTreeSet;
    use std::io::{Read, Write};
    use mio_plugin_api::{
        panel::{self, PanelSnapshot, node},
        protocol::{ErrorCode, Failure, Register, WorkerSession, envelope},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

    struct BlockingReader(DuplexStream, tokio::runtime::Handle);
    impl Read for BlockingReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.1.block_on(self.0.read(buf))
        }
    }
    struct BlockingWriter(DuplexStream, tokio::runtime::Handle);
    impl Write for BlockingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.1.block_on(self.0.write(buf))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.1.block_on(self.0.flush())
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let worker_path = dir.path().join("init-fail-mux-worker");
    fs::write(&worker_path, [0xcf, 0xfa, 0xed, 0xfe]).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&worker_path, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        dir.path().join("manifest.json"),
        br#"{"id":"lab.init-fail-mux","version":"1.5.0","api_version":1,"engine":"native","entry":"init-fail-mux-worker","permissions":["traffic.read"],"contributions":["suite_panel","message_editor"]}"#,
    )
    .unwrap();
    let package = VerifiedPackage::read(dir.path()).unwrap();
    let grants = vec![Permission::TrafficRead];
    let grants_set: BTreeSet<Permission> = grants.iter().copied().collect();
    let _ = NativeTrust::confirmed(&package, grants.clone()).unwrap();
    let (events, mut rx) = mpsc::channel(256);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: state.project,
    };

    let (host_tx, worker_rx) = tokio::io::duplex(64 * 1024);
    let (worker_tx, host_rx) = tokio::io::duplex(64 * 1024);
    let rt_handle = tokio::runtime::Handle::current();
    let manifest_for_worker = package.manifest.clone();

    let worker_thread = std::thread::spawn(move || {
        let reg = Register {
            plugin_id: manifest_for_worker.id,
            plugin_version: manifest_for_worker.version,
            required_components: vec![],
            panel: Some(PanelSnapshot {
                revision: 1,
                title: "init-fail".into(),
                root: Some(panel::Node::new(
                    1,
                    "root",
                    node::Kind::Text(panel::Text {
                        text: "ready".into(),
                        tone: 0,
                    }),
                )),
            }),
            supports_panel_multiplexing: true,
        };
        let mut session = WorkerSession::accept(
            BlockingReader(worker_rx, rt_handle.clone()),
            BlockingWriter(worker_tx, rt_handle),
            reg,
        )
        .unwrap();
        while let Ok(env) = session.receive() {
            let reply_to = env.request_id;
            let pid = env.panel_id;
            let Some(body) = env.body else { break };
            match body {
                envelope::Body::Shutdown(_) => break,
                envelope::Body::OpenRequest(_) => {
                    session
                        .send_on_panel(
                            pid,
                            envelope::Body::Error(Failure::new(ErrorCode::Invalid, "open failed")),
                            0,
                            reply_to,
                        )
                        .unwrap();
                }
                envelope::Body::CloseRequest(_) => {
                    session
                        .send_on_panel(
                            pid,
                            envelope::Body::Error(Failure::new(
                                ErrorCode::Invalid,
                                "init cleanup close refused",
                            )),
                            0,
                            reply_to,
                        )
                        .unwrap();
                }
                _ => {}
            }
        }
    });

    let (worker, services) = crate::plugins::spawn_test_duplex_supervisor(
        identity,
        package.manifest.clone(),
        grants_set,
        host_tx,
        host_rx,
    )
    .await
    .unwrap();
    let parent_id = identity.instance_id;
    let initial = worker.initial_panel.clone();
    let info = LoadedPlugin {
        identity,
        directory: package.directory.clone(),
        manifest: package.manifest.clone(),
        grants,
        tab_id: None,
    };
    state.plugins.instances.insert(
        parent_id,
        Instance {
            worker,
            services,
            package: package.clone(),
            manifest_modified: package.manifest_modified,
            manifest_size: package.manifest_bytes.len() as u64,
            entry_modified: package.modified,
            entry_size: package.executable_size,
            auto_reload: false,
            info,
            panel: initial,
            resources: HashMap::new(),
        },
    );
    state.plugins.order.push(parent_id);

    let (host_tab, host_rev) = add_tab(&mut state, "host", "http://127.0.0.1/host");
    let op_rep = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            op_rep,
            PluginAction::CreateEditor {
                source_instance: parent_id,
                tab_id: host_tab,
                draft_revision: host_rev,
            },
        )
        .await
        .unwrap();
    let open_res = drain_op(&mut state, &mut rx, op_rep).await;
    assert!(matches!(open_res, Err(ref msg) if msg.contains("open failed")));

    // Background revoke job runs after open failure and MUST emit PluginDiagnostic (not swallow error),
    // while retaining (parent_id, 2) in closing_panels until parent worker restarts/disables.
    let diag = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            while let Ok(ev) = rx.try_recv() {
                if let Event::PluginDiagnostic { identity, message } = ev
                    && identity.instance_id == parent_id
                    && message.contains("init cleanup close refused")
                {
                    return message;
                }
            }
            let sig = state.plugins.next_signal().await;
            state.plugin_signal(sig).await;
        }
    })
    .await
    .expect("expected PluginDiagnostic from failed init-cleanup close");
    assert!(diag.contains("init cleanup close refused"));
    assert!(state.plugins.multiplexed.is_empty());
    assert!(state.plugins.closing_panels.contains(&(parent_id, 2)));

    let dis_parent = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            dis_parent,
            PluginAction::Disable {
                instance_id: parent_id,
            },
        )
        .await
        .unwrap();
    assert!(drain_op(&mut state, &mut rx, dis_parent).await.is_ok());
    assert!(state.plugins.closing_panels.is_empty());
    worker_thread.join().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_sdk_natural_ui_timeout_resyncs_and_recovers_next_host_call() {
    use std::collections::BTreeSet;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

    struct BlockingReader(DuplexStream, tokio::runtime::Handle);
    impl Read for BlockingReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.1.block_on(self.0.read(buf))
        }
    }
    struct BlockingWriter(DuplexStream, tokio::runtime::Handle);
    impl Write for BlockingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.1.block_on(self.0.write(buf))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.1.block_on(self.0.flush())
        }
    }

    struct DelayedUiSdkPlugin {
        resync_seen: Arc<AtomicBool>,
        resync_notify: Arc<tokio::sync::Notify>,
        cancel_cleanup_ok: Arc<AtomicBool>,
    }

    impl mio_plugin_sdk::MioPlugin for DelayedUiSdkPlugin {
        fn id(&self) -> &str {
            "lab.sdk-ui-timeout"
        }

        fn version(&self) -> &str {
            "1.5.0"
        }

        fn supports_panel_multiplexing(&self) -> bool {
            true
        }

        fn on_open_request(
            &mut self,
            ctx: &mut dyn mio_plugin_sdk::HostClient,
            _panel_id: u64,
            _open: mio_plugin_api::protocol::OpenRequest,
        ) -> Result<mio_plugin_sdk::PanelUpdate, String> {
            let rev = ctx.next_panel_revision();
            Ok(mio_plugin_sdk::ui::snapshot(
                rev,
                self.id(),
                mio_plugin_sdk::ui::text(1, "ed", "initial", mio_plugin_api::panel::Tone::Foreground),
            )
            .into())
        }

        fn on_ui_event(
            &mut self,
            ctx: &mut dyn mio_plugin_sdk::HostClient,
            _panel_id: u64,
            event: mio_plugin_api::panel::UiEvent,
        ) -> Result<mio_plugin_sdk::PanelUpdate, String> {
            if matches!(event.event, Some(mio_plugin_api::panel::ui_event::Event::Focus(true))) {
                std::thread::sleep(std::time::Duration::from_millis(5150));
                let rev = ctx.next_panel_revision();
                return Ok(mio_plugin_sdk::ui::snapshot(
                    rev,
                    "late-timed-out",
                    mio_plugin_sdk::ui::text(
                        1,
                        "ed",
                        "late-timed-out",
                        mio_plugin_api::panel::Tone::Foreground,
                    ),
                )
                .into());
            }
            let _page = ctx.query_history(0, 10, "")?;
            let rev = ctx.next_panel_revision();
            Ok(mio_plugin_sdk::ui::snapshot(
                rev,
                "recovered-ok",
                mio_plugin_sdk::ui::text(
                    1,
                    "ed",
                    "recovered-ok",
                    mio_plugin_api::panel::Tone::Foreground,
                ),
            )
            .into())
        }

        fn on_cancel(
            &mut self,
            ctx: &mut dyn mio_plugin_sdk::HostClient,
            _panel_id: u64,
            _cancel: mio_plugin_api::protocol::Cancel,
        ) -> Result<mio_plugin_sdk::PanelUpdate, String> {
            // Arrives right after the timed-out UI callback bumped local `published` to 2, BEFORE Resync(1).
            let page = ctx.query_history(0, 10, "")?;
            assert_eq!(page.offset, 0);
            self.cancel_cleanup_ok.store(true, Ordering::Release);
            Ok(mio_plugin_sdk::PanelUpdate::None)
        }

        fn on_resync(
            &mut self,
            ctx: &mut dyn mio_plugin_sdk::HostClient,
            panel_id: u64,
            snapshot: PanelSnapshot,
        ) -> Result<mio_plugin_sdk::PanelUpdate, String> {
            if panel_id == 2 && snapshot.revision == 1 {
                self.resync_seen.store(true, Ordering::Release);
                self.resync_notify.notify_one();
                let rev = ctx.next_panel_revision();
                return Ok(mio_plugin_sdk::ui::snapshot(
                    rev,
                    self.id(),
                    mio_plugin_sdk::ui::text(
                        1,
                        "ed",
                        "initial",
                        mio_plugin_api::panel::Tone::Foreground,
                    ),
                )
                .into());
            }
            Ok(mio_plugin_sdk::PanelUpdate::None)
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let worker_path = dir.path().join("sdk-ui-timeout");
    fs::write(&worker_path, [0xcf, 0xfa, 0xed, 0xfe]).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&worker_path, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        dir.path().join("manifest.json"),
        br#"{"id":"lab.sdk-ui-timeout","version":"1.5.0","api_version":1,"engine":"native","entry":"sdk-ui-timeout","permissions":["traffic.read"],"contributions":["suite_panel","message_editor"]}"#,
    )
    .unwrap();
    let package = VerifiedPackage::read(dir.path()).unwrap();
    let grants = vec![Permission::TrafficRead];
    let grants_set: BTreeSet<Permission> = grants.iter().copied().collect();
    let _ = NativeTrust::confirmed(&package, grants.clone()).unwrap();
    let (events, mut rx) = mpsc::channel(256);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: state.project,
    };

    let (host_tx, worker_rx) = tokio::io::duplex(64 * 1024);
    let (worker_tx, host_rx) = tokio::io::duplex(64 * 1024);
    let rt_handle = tokio::runtime::Handle::current();
    let resync_seen = Arc::new(AtomicBool::new(false));
    let resync_seen_worker = resync_seen.clone();
    let resync_notify = Arc::new(tokio::sync::Notify::new());
    let resync_notify_worker = resync_notify.clone();
    let cancel_cleanup_ok = Arc::new(AtomicBool::new(false));
    let cancel_cleanup_ok_worker = cancel_cleanup_ok.clone();

    let worker_thread = std::thread::spawn(move || {
        let mut plugin = DelayedUiSdkPlugin {
            resync_seen: resync_seen_worker,
            resync_notify: resync_notify_worker,
            cancel_cleanup_ok: cancel_cleanup_ok_worker,
        };
        mio_plugin_sdk::run_with_io(
            BlockingReader(worker_rx, rt_handle.clone()),
            BlockingWriter(worker_tx, rt_handle),
            &mut plugin,
        )
        .unwrap();
    });

    let (worker, services) = crate::plugins::spawn_test_duplex_supervisor(
        identity,
        package.manifest.clone(),
        grants_set,
        host_tx,
        host_rx,
    )
    .await
    .unwrap();
    let parent_id = identity.instance_id;
    let initial = worker.initial_panel.clone();
    let info = LoadedPlugin {
        identity,
        directory: package.directory.clone(),
        manifest: package.manifest.clone(),
        grants,
        tab_id: None,
    };
    state.plugins.instances.insert(
        parent_id,
        Instance {
            worker,
            services,
            package: package.clone(),
            manifest_modified: package.manifest_modified,
            manifest_size: package.manifest_bytes.len() as u64,
            entry_modified: package.modified,
            entry_size: package.executable_size,
            auto_reload: false,
            info,
            panel: initial,
            resources: HashMap::new(),
        },
    );
    state.plugins.order.push(parent_id);

    let (host_tab, host_rev) = add_tab(&mut state, "host", "http://127.0.0.1/host");
    let op_open = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            op_open,
            PluginAction::CreateEditor {
                source_instance: parent_id,
                tab_id: host_tab,
                draft_revision: host_rev,
            },
        )
        .await
        .unwrap();
    let PluginOutcome::Loaded { info: rep_info, .. } =
        drain_op(&mut state, &mut rx, op_open).await.unwrap()
    else {
        panic!("expected child Loaded");
    };
    let rep_id = rep_info.identity.instance_id;

    // 1. Trigger UI event whose SDK callback naturally sleeps past 5s and bumps local published to rev=2.
    let ui1 = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            ui1,
            PluginAction::Ui {
                instance_id: rep_id,
                panel_revision: 1,
                event_seq: 0,
                event: mio_plugin_api::panel::UiEvent {
                    node_id: 1,
                    editors: vec![],
                    event: Some(mio_plugin_api::panel::ui_event::Event::Focus(true)),
                },
            },
        )
        .await
        .unwrap();
    assert!(drain_op(&mut state, &mut rx, ui1).await.is_ok());

    // Drive natural host timeout (5s) -> host sends Resync(1) AND drops late rev=2 snapshot.
    tokio::time::timeout(std::time::Duration::from_secs(7), async {
        while !resync_seen.load(Ordering::Acquire) {
            while let Ok(ev) = rx.try_recv() {
                if let Event::PluginStopped { identity, reason } = ev {
                    panic!("unexpected PluginStopped for {:?}: {reason}", identity.instance_id);
                }
            }
            tokio::select! {
                _ = resync_notify.notified() => {}
                sig = state.plugins.next_signal() => {
                    state.plugin_signal(sig).await;
                }
            }
        }
    })
    .await
    .expect("expected natural UI timeout + Resync(1) delivery to SDK");
    assert!(
        cancel_cleanup_ok.load(Ordering::Acquire),
        "allowlisted cleanup HostCall inside on_cancel must succeed even before Resync(1) resets drifted revision"
    );

    // 2. Immediately invoke next UI event on the child panel -> HostCall inside SDK callback MUST succeed!
    let ui2 = Uuid::new_v4();
    let current_rev = state.plugins.multiplexed[&rep_id].panel.revision;
    state
        .plugin_command(
            state.project,
            ui2,
            PluginAction::Ui {
                instance_id: rep_id,
                panel_revision: current_rev,
                event_seq: 0,
                event: mio_plugin_api::panel::UiEvent {
                    node_id: 1,
                    editors: vec![],
                    event: Some(mio_plugin_api::panel::ui_event::Event::Focus(false)),
                },
            },
        )
        .await
        .unwrap();
    assert!(drain_op(&mut state, &mut rx, ui2).await.is_ok());

    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while state.plugins.multiplexed[&rep_id].panel.title != "recovered-ok" {
            let sig = state.plugins.next_signal().await;
            state.plugin_signal(sig).await;
        }
    })
    .await
    .expect("next HostCall after Resync must succeed and publish recovered-ok");

    let dis_parent = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            dis_parent,
            PluginAction::Disable {
                instance_id: parent_id,
            },
        )
        .await
        .unwrap();
    assert!(drain_op(&mut state, &mut rx, dis_parent).await.is_ok());
    worker_thread.join().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_sdk_natural_destroy_timeout_reclaims_runtime_and_project_quota_on_late_ack() {
    use std::collections::BTreeSet;
    use std::io::{Read, Write};
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

    struct BlockingReader(DuplexStream, tokio::runtime::Handle);
    impl Read for BlockingReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.1.block_on(self.0.read(buf))
        }
    }
    struct BlockingWriter(DuplexStream, tokio::runtime::Handle);
    impl Write for BlockingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.1.block_on(self.0.write(buf))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.1.block_on(self.0.flush())
        }
    }

    struct DelayedDestroySdkPlugin;

    impl mio_plugin_sdk::MioPlugin for DelayedDestroySdkPlugin {
        fn id(&self) -> &str {
            "lab.sdk-destroy-timeout"
        }

        fn version(&self) -> &str {
            "1.5.0"
        }

        fn supports_panel_multiplexing(&self) -> bool {
            true
        }

        fn on_open_request(
            &mut self,
            ctx: &mut dyn mio_plugin_sdk::HostClient,
            panel_id: u64,
            _open: mio_plugin_api::protocol::OpenRequest,
        ) -> Result<mio_plugin_sdk::PanelUpdate, String> {
            let rev = ctx.next_panel_revision();
            Ok(mio_plugin_sdk::ui::snapshot(
                rev,
                format!("panel-{panel_id}"),
                mio_plugin_api::panel::Node::new(
                    1,
                    "ed",
                    mio_plugin_api::panel::node::Kind::Editor(mio_plugin_api::panel::Editor {
                        document_id: 1,
                        revision: 1,
                        contents: format!("p-{panel_id}").into_bytes(),
                        format: mio_plugin_api::panel::EditorFormat::Utf8 as i32,
                        read_only: false,
                    }),
                ),
            )
            .into())
        }

        fn on_commit_editor(
            &mut self,
            _ctx: &mut dyn mio_plugin_sdk::HostClient,
            panel_id: u64,
            commit: mio_plugin_api::protocol::CommitEditor,
        ) -> Result<mio_plugin_api::protocol::Committed, String> {
            let mut req = commit.request.unwrap();
            req.headers.push(mio_plugin_api::protocol::HeaderDto {
                name: "x-alive-panel".into(),
                value: panel_id.to_string().into_bytes(),
            });
            Ok(mio_plugin_api::protocol::Committed {
                resource_id: commit.resource_id,
                message_revision: commit.message_revision,
                document_id: commit.editor.as_ref().map_or(1, |e| e.document_id),
                document_revision: commit.editor.as_ref().map_or(1, |e| e.revision),
                request: Some(req),
            })
        }

        fn on_panel_destroy(
            &mut self,
            _ctx: &mut dyn mio_plugin_sdk::HostClient,
            panel_id: u64,
            _close: mio_plugin_api::protocol::CloseRequest,
        ) -> Result<mio_plugin_sdk::PanelUpdate, String> {
            if panel_id == 3 {
                std::thread::sleep(std::time::Duration::from_millis(5150));
            }
            Ok(mio_plugin_sdk::PanelUpdate::None)
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let worker_path = dir.path().join("sdk-destroy-timeout");
    fs::write(&worker_path, [0xcf, 0xfa, 0xed, 0xfe]).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&worker_path, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        dir.path().join("manifest.json"),
        br#"{"id":"lab.sdk-destroy-timeout","version":"1.5.0","api_version":1,"engine":"native","entry":"sdk-destroy-timeout","permissions":["traffic.read","draft.write"],"contributions":["suite_panel","message_editor"]}"#,
    )
    .unwrap();
    let package = VerifiedPackage::read(dir.path()).unwrap();
    let grants = vec![Permission::TrafficRead, Permission::DraftWrite];
    let grants_set: BTreeSet<Permission> = grants.iter().copied().collect();
    let _ = NativeTrust::confirmed(&package, grants.clone()).unwrap();
    let (events, mut rx) = mpsc::channel(256);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: state.project,
    };

    let (host_tx, worker_rx) = tokio::io::duplex(64 * 1024);
    let (worker_tx, host_rx) = tokio::io::duplex(64 * 1024);
    let rt_handle = tokio::runtime::Handle::current();

    let worker_thread = std::thread::spawn(move || {
        let mut plugin = DelayedDestroySdkPlugin;
        mio_plugin_sdk::run_with_io(
            BlockingReader(worker_rx, rt_handle.clone()),
            BlockingWriter(worker_tx, rt_handle),
            &mut plugin,
        )
        .unwrap();
    });

    let (worker, services) = crate::plugins::spawn_test_duplex_supervisor(
        identity,
        package.manifest.clone(),
        grants_set,
        host_tx,
        host_rx,
    )
    .await
    .unwrap();
    let parent_id = identity.instance_id;
    let initial = worker.initial_panel.clone();
    let info = LoadedPlugin {
        identity,
        directory: package.directory.clone(),
        manifest: package.manifest.clone(),
        grants,
        tab_id: None,
    };
    state.plugins.instances.insert(
        parent_id,
        Instance {
            worker,
            services,
            package: package.clone(),
            manifest_modified: package.manifest_modified,
            manifest_size: package.manifest_bytes.len() as u64,
            entry_modified: package.modified,
            entry_size: package.executable_size,
            auto_reload: false,
            info,
            panel: initial,
            resources: HashMap::new(),
        },
    );
    state.plugins.order.push(parent_id);

    let (tab1, rev1) = add_tab(&mut state, "t1", "http://127.0.0.1/t1");
    let (tab2, rev2) = add_tab(&mut state, "t2", "http://127.0.0.1/t2");

    let op1 = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            op1,
            PluginAction::CreateEditor {
                source_instance: parent_id,
                tab_id: tab1,
                draft_revision: rev1,
            },
        )
        .await
        .unwrap();
    let PluginOutcome::Loaded { info: info1, .. } =
        drain_op(&mut state, &mut rx, op1).await.unwrap()
    else {
        panic!("expected child1 Loaded");
    };
    let child1_id = info1.identity.instance_id;

    let op2 = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            op2,
            PluginAction::CreateEditor {
                source_instance: parent_id,
                tab_id: tab2,
                draft_revision: rev2,
            },
        )
        .await
        .unwrap();
    let PluginOutcome::Loaded { info: info2, .. } =
        drain_op(&mut state, &mut rx, op2).await.unwrap()
    else {
        panic!("expected child2 Loaded");
    };
    let child2_id = info2.identity.instance_id;
    assert_eq!(state.plugins.multiplexed[&child2_id].panel_id, 3);

    // Disable child2 (panel 3): times out at 5s while SDK finishes on_panel_destroy at 5.15s.
    let dis2 = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            dis2,
            PluginAction::Disable {
                instance_id: child2_id,
            },
        )
        .await
        .unwrap();
    let dis2_res = tokio::time::timeout(
        std::time::Duration::from_secs(7),
        drain_op(&mut state, &mut rx, dis2),
    )
    .await
    .expect("disable timeout should settle");
    assert!(dis2_res.is_err());
    assert!(state.plugins.closing_panels.contains(&(parent_id, 3)));

    // Wait briefly for SDK's late verified destroy Ack -> closing_panels quota reclaims automatically WITHOUT restarting parent!
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !state.plugins.closing_panels.is_empty() {
            let sig = state.plugins.next_signal().await;
            state.plugin_signal(sig).await;
        }
    })
    .await
    .expect("verified late destroy Ack must reclaim closing_panels quota");
    assert!(state.plugins.closing_panels.is_empty());

    // Sibling child panel 2 on the SAME shared worker still commits cleanly!
    let commit1 = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            commit1,
            PluginAction::CommitEditor {
                instance_id: child1_id,
                tab_id: tab1,
                draft_revision: rev1,
                editor: EditorSnapshot {
                    document_id: 1,
                    revision: 1,
                    contents: b"p-2".to_vec(),
                    ..Default::default()
                },
                            expected_panel: None,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        drain_op(&mut state, &mut rx, commit1).await.unwrap(),
        PluginOutcome::Committed { request, .. }
            if request.headers.iter().any(|h| h.name == "x-alive-panel" && h.value == b"2")
    ));

    let dis_parent = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            dis_parent,
            PluginAction::Disable {
                instance_id: parent_id,
            },
        )
        .await
        .unwrap();
    assert!(drain_op(&mut state, &mut rx, dis_parent).await.is_ok());
    worker_thread.join().unwrap();
}

async fn read_mux_duplex_frame(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
) -> anyhow::Result<mio_plugin_api::protocol::Envelope> {
    use tokio::io::AsyncReadExt;

    let mut prefix = [0u8; 4];
    reader.read_exact(&mut prefix).await?;
    let mut payload = vec![0u8; mio_plugin_api::protocol::frame_length(prefix)?];
    reader.read_exact(&mut payload).await?;
    Ok(mio_plugin_api::protocol::decode_frame(&payload)?)
}

async fn setup_duplex_service_state(
    event_capacity: usize,
    host_tx: tokio::io::DuplexStream,
    host_rx: tokio::io::DuplexStream,
) -> (
    State,
    mpsc::Receiver<Event>,
    mpsc::Sender<Event>,
    Uuid,
    tempfile::TempDir,
) {
    use std::collections::BTreeSet;

    let dir = tempfile::tempdir().unwrap();
    let worker_path = dir.path().join("duplex-service-worker");
    fs::write(&worker_path, [0xcf, 0xfa, 0xed, 0xfe]).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&worker_path, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        dir.path().join("manifest.json"),
        br#"{"id":"lab.duplex-service","version":"1.5.0","api_version":1,"engine":"native","entry":"duplex-service-worker","permissions":["traffic.read","draft.write"],"contributions":["suite_panel","message_editor","message_response_editor"]}"#,
    )
    .unwrap();
    let package = VerifiedPackage::read(dir.path()).unwrap();
    let grants = vec![Permission::TrafficRead, Permission::DraftWrite];
    let grants_set: BTreeSet<Permission> = grants.iter().copied().collect();
    let _ = NativeTrust::confirmed(&package, grants.clone()).unwrap();
    let (events_tx, rx) = mpsc::channel(event_capacity);
    let events_tx_clone = events_tx.clone();
    let mut state = State::new(events_tx, CancellationToken::new()).unwrap();
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: state.project,
    };
    let (worker, services) = crate::plugins::spawn_test_duplex_supervisor(
        identity,
        package.manifest.clone(),
        grants_set,
        host_tx,
        host_rx,
    )
    .await
    .unwrap();
    let parent_id = identity.instance_id;
    let initial = worker.initial_panel.clone();
    let info = LoadedPlugin {
        identity,
        directory: package.directory.clone(),
        manifest: package.manifest.clone(),
        grants,
        tab_id: None,
    };
    state.plugins.instances.insert(
        parent_id,
        Instance {
            worker,
            services,
            package: package.clone(),
            manifest_modified: package.manifest_modified,
            manifest_size: package.manifest_bytes.len() as u64,
            entry_modified: package.modified,
            entry_size: package.executable_size,
            auto_reload: false,
            info,
            panel: initial,
            resources: HashMap::new(),
        },
    );
    state.plugins.order.push(parent_id);
    (state, rx, events_tx_clone, parent_id, dir)
}

fn make_duplex_panel_snapshot(revision: u64, title: &str) -> PanelSnapshot {
    use mio_plugin_api::panel::{Editor, EditorFormat, Node, node};

    PanelSnapshot {
        revision,
        title: title.into(),
        root: Some(Node::new(
            1,
            "ed",
            node::Kind::Editor(Editor {
                document_id: 1,
                revision: 1,
                contents: b"init".to_vec(),
                format: EditorFormat::Utf8 as i32,
                read_only: false,
            }),
        )),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_workspace_changed_cancels_all_waiters_before_event_backpressure_blocks() {
    use std::time::Duration;
    use mio_plugin_api::{
        panel::{self, UiEvent},
        protocol::{self, Committed, Envelope, ErrorCode, Failure, envelope},
    };
    use tokio::io::AsyncWriteExt;

    const CONTROL_QUEUE_CAPACITY: usize = 128;
    const OP_TIMEOUT: Duration = Duration::from_secs(5);

    let (host_tx, mut worker_rx) = tokio::io::duplex(64 * 1024);
    let (mut worker_tx, host_rx) = tokio::io::duplex(64 * 1024);

    let received_opens = Arc::new(std::sync::Mutex::new(Vec::<(u64, Uuid, u64)>::new()));
    let received_commits = Arc::new(std::sync::Mutex::new(Vec::<(u64, Uuid, u64)>::new()));
    let received_closes = Arc::new(std::sync::Mutex::new(Vec::<(u64, Uuid, bool)>::new()));
    let opens_w = received_opens.clone();
    let commits_w = received_commits.clone();
    let closes_w = received_closes.clone();
    let send_errors = Arc::new(tokio::sync::Notify::new());
    let send_errors_w = send_errors.clone();
    let errors_sent = Arc::new(tokio::sync::Notify::new());
    let errors_sent_w = errors_sent.clone();

    let worker_task = tokio::spawn(async move {
        let hello = read_mux_duplex_frame(&mut worker_rx).await?;
        let identity = ChannelIdentity {
            instance_id: protocol::uuid(&hello.instance_id).map_err(anyhow::Error::msg)?,
            project: mio_plugin_api::ProjectKey {
                id: protocol::uuid(&hello.project_id).map_err(anyhow::Error::msg)?,
                generation: hello.project_generation,
            },
        };
        let send_frame = |seq: &mut u64,
                          panel_id: u64,
                          rev: u64,
                          in_reply_to: u64,
                          body: envelope::Body| {
            *seq += 1;
            let mut env = Envelope::new(identity, *seq, rev, body, OP_TIMEOUT).with_panel_id(panel_id);
            env.in_reply_to = in_reply_to;
            env
        };
        let mut out_seq = 0u64;
        let reg = send_frame(
            &mut out_seq,
            protocol::PANEL_ID,
            1,
            hello.request_id,
            envelope::Body::Register(protocol::Register {
                plugin_id: "lab.duplex-service".into(),
                plugin_version: "1.5.0".into(),
                required_components: vec![],
                panel: Some(make_duplex_panel_snapshot(1, "main")),
                supports_panel_multiplexing: true,
            }),
        );
        worker_tx.write_all(&protocol::encode_frame(&reg)?).await?;
        worker_tx.flush().await?;
        let _ready = read_mux_duplex_frame(&mut worker_rx).await?;

        let mut revs: HashMap<u64, u64> = HashMap::from([(protocol::PANEL_ID, 1)]);
        let mut sent_err_burst = false;
        loop {
            tokio::select! {
                biased;
                _ = send_errors_w.notified(), if !sent_err_burst => {
                    sent_err_burst = true;
                    for _ in 0..(CONTROL_QUEUE_CAPACITY + 2) {
                        let err_ev = send_frame(
                            &mut out_seq,
                            protocol::PANEL_ID,
                            1,
                            0,
                            envelope::Body::Error(Failure::new(ErrorCode::Internal, "fill-events")),
                        );
                        worker_tx.write_all(&protocol::encode_frame(&err_ev)?).await?;
                    }
                    worker_tx.flush().await?;
                    errors_sent_w.notify_one();
                }
                incoming = read_mux_duplex_frame(&mut worker_rx) => {
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
                                envelope::Body::Snapshot(make_duplex_panel_snapshot(
                                    *rev,
                                    &format!("p-{}-opened", env.panel_id),
                                )),
                            );
                            worker_tx.write_all(&protocol::encode_frame(&snap)?).await?;
                            worker_tx.flush().await?;
                        }
                        Some(envelope::Body::CommitEditor(ref commit)) => {
                            let rid = protocol::uuid(&commit.resource_id).map_err(anyhow::Error::msg)?;
                            commits_w
                                .lock()
                                .unwrap()
                                .push((env.panel_id, rid, commit.message_revision));
                            let rev = *revs.get(&env.panel_id).unwrap_or(&1);
                            let committed = send_frame(
                                &mut out_seq,
                                env.panel_id,
                                rev,
                                env.request_id,
                                envelope::Body::Committed(Committed {
                                    resource_id: commit.resource_id.clone(),
                                    message_revision: commit.message_revision,
                                    document_id: 1,
                                    document_revision: commit.editor.as_ref().map_or(1, |e| e.revision),
                                    request: commit.request.clone(),
                                }),
                            );
                            worker_tx
                                .write_all(&protocol::encode_frame(&committed)?)
                                .await?;
                            worker_tx.flush().await?;
                        }
                        Some(envelope::Body::UiEvent(_)) => {
                            let rev = *revs.get(&env.panel_id).unwrap_or(&1);
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
                            let rev = *revs.get(&env.panel_id).unwrap_or(&1);
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

    // Forward worker.events through a controllable watch gate so we can park Running on `self.events.send`
    // and then unblock it WHILE `plugin_workspace_changed` is blocked on Event backpressure.
    let (mut state, mut rx, events_tx_bp, parent_id, _dir) =
        setup_duplex_service_state(8, host_tx, host_rx).await;
    let identity = state.plugins.instances[&parent_id].info.identity;
    let (proxy_tx, proxy_rx) = mpsc::channel(256);
    let real_worker_events = std::mem::replace(
        &mut state
            .plugins
            .instances
            .get_mut(&parent_id)
            .unwrap()
            .worker
            .events,
        proxy_rx,
    );
    let (forward_tx, mut forward_rx) = tokio::sync::watch::channel(true);
    let forwarder_task = tokio::spawn(async move {
        let mut real = real_worker_events;
        while let Some(ev) = real.recv().await {
            if forward_rx.wait_for(|&enabled| enabled).await.is_err() {
                break;
            }
            if proxy_tx.send(ev).await.is_err() {
                break;
            }
        }
    });

    let (tab_child1, rev_child1) = add_tab(&mut state, "child1", "http://127.0.0.1/c1");
    let (tab_main, rev_main) = add_tab(&mut state, "main-tab", "http://127.0.0.1/main");

    // 1. child1 (tab-bound multiplexed editor -> closed & stopped in Group 2)
    let op_c1 = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            op_c1,
            PluginAction::CreateEditor {
                source_instance: parent_id,
                tab_id: tab_child1,
                draft_revision: rev_child1,
            },
        )
        .await
        .unwrap();
    let PluginOutcome::Loaded { info: c1_info, .. } =
        drain_op(&mut state, &mut rx, op_c1).await.unwrap()
    else {
        panic!("expected child1 Loaded");
    };
    let child1_id = c1_info.identity.instance_id;
    let child1_panel = state.plugins.multiplexed[&child1_id].panel_id;

    // 2. child2 (context viewer with tab_id=None + extra tab_main bound -> kept alive; extra tab_main revoked in Group 4)
    let ctx_id = Uuid::new_v4();
    let req_fixture = parse_request(
        b"GET /fixture HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        "http://127.0.0.1/fixture",
        HttpVersion::Http11,
    )
    .unwrap();
    let op_c2 = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            op_c2,
            PluginAction::OpenContextViewer {
                source_instance: parent_id,
                context_id: ctx_id,
                revision: 1,
                read_only: false,
                request: req_fixture.clone(),
            },
        )
        .await
        .unwrap();
    let PluginOutcome::ContextOpened { info: c2_info, .. } =
        drain_op(&mut state, &mut rx, op_c2).await.unwrap()
    else {
        panic!("expected child2 ContextOpened");
    };
    let child2_id = c2_info.identity.instance_id;
    let child2_panel = state.plugins.multiplexed[&child2_id].panel_id;

    // 3. Bind `tab_main` onto BOTH main panel (`parent_id`, Group 3) and `child2` (`child2_id`, Group 4).
    let op_open_main = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            op_open_main,
            PluginAction::OpenRequest {
                instance_id: parent_id,
                tab_id: tab_main,
                draft_revision: rev_main,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        drain_op(&mut state, &mut rx, op_open_main).await.unwrap(),
        PluginOutcome::Opened { .. }
    ));

    let op_open_c2_tab = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            op_open_c2_tab,
            PluginAction::OpenRequest {
                instance_id: child2_id,
                tab_id: tab_main,
                draft_revision: rev_main,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        drain_op(&mut state, &mut rx, op_open_c2_tab).await.unwrap(),
        PluginOutcome::Opened { .. }
    ));

    while let Ok(sig) =
        tokio::time::timeout(Duration::from_millis(15), state.plugins.next_signal()).await
    {
        state.plugin_signal(sig).await;
    }
    while rx.try_recv().is_ok() {}

    // Pause forwarding worker.events and park Running inside `self.events.send` with a full worker event queue.
    forward_tx.send(false).unwrap();
    send_errors.notify_one();
    tokio::time::timeout(OP_TIMEOUT, errors_sent.notified())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;

    let parent_handle = state.plugins.instances[&parent_id].worker.handle();
    for i in 0..CONTROL_QUEUE_CAPACITY {
        parent_handle
            .try_event(UiEvent {
                node_id: 1,
                editors: vec![],
                event: Some(panel::ui_event::Event::Focus(true)),
            })
            .unwrap_or_else(|e| panic!("failed filling control slot {i}: {e}"));
    }
    assert!(
        parent_handle
            .try_event(UiEvent {
                node_id: 1,
                editors: vec![],
                event: Some(panel::ui_event::Event::Focus(true)),
            })
            .is_err()
    );

    // Park opens for all three affected targets (child1 panel, main panel tab_main, child2 extra tab_main)
    // plus an accepted CommitEditor on main panel tab_main while `handle.control` is 128/128 full.
    let extra_child1 = Uuid::new_v4();
    let h1 = parent_handle.clone();
    let req1 = req_fixture.clone();
    let mut open_child1_task = tokio::spawn(async move {
        h1.open_request_on_panel(child1_panel, extra_child1, 1, false, req1)
            .await
    });
    let h2 = parent_handle.clone();
    let req2 = req_fixture.clone();
    let mut open_main_task = tokio::spawn(async move {
        h2.open_request_on_panel(protocol::PANEL_ID, tab_main, 2, false, req2)
            .await
    });
    let h3 = parent_handle.clone();
    let req3 = req_fixture.clone();
    let mut open_child2_task = tokio::spawn(async move {
        h3.open_request_on_panel(child2_panel, tab_main, 2, false, req3)
            .await
    });

    let commit_main_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            commit_main_op,
            PluginAction::CommitEditor {
                instance_id: parent_id,
                tab_id: tab_main,
                draft_revision: rev_main,
                editor: EditorSnapshot {
                    document_id: 1,
                    revision: 2,
                    contents: b"stale-during-backpressure".to_vec(),
                    ..Default::default()
                },
                            expected_panel: None,
            },
        )
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut open_child1_task)
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut open_main_task)
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut open_child2_task)
            .await
            .is_err()
    );

    // Fill the Event channel to impose backpressure on `self.emit(PluginStopped).await`.
    while rx.try_recv().is_ok() {}
    while events_tx_bp
        .try_send(Event::PluginDiagnostic {
            identity,
            message: "backpressure".into(),
        })
        .is_ok()
    {}
    assert!(
        events_tx_bp
            .try_send(Event::PluginDiagnostic {
                identity,
                message: "overflow".into(),
            })
            .is_err()
    );

    let mut ws_closed = state.store.workspace().unwrap();
    ws_closed
        .tabs
        .retain(|t| t.id != tab_child1 && t.id != tab_main);
    ws_closed.active = None;
    ws_closed.revision += 1;
    state.store.save_workspace(&ws_closed).unwrap();

    // Start workspace close: synchronous cancellations must run BEFORE `emit(PluginStopped).await` blocks!
    let mut ws_fut = Box::pin(state.plugin_workspace_changed(&ws_closed));
    assert!(
        tokio::time::timeout(Duration::from_millis(40), &mut ws_fut)
            .await
            .is_err(),
        "plugin_workspace_changed must be suspended on Event backpressure"
    );

    // While `ws_fut` is STILL blocked on Event backpressure, recover `handle.control` capacity cleanly.
    forward_tx.send(true).unwrap();
    tokio::time::timeout(OP_TIMEOUT, async {
        loop {
            if parent_handle
                .try_event(UiEvent {
                    node_id: 1,
                    editors: vec![],
                    event: Some(panel::ui_event::Event::Focus(true)),
                })
                .is_ok()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("handle.control capacity must recover while ws_fut is still blocked");

    // All three affected waiters MUST ALREADY be cancelled with Err BEFORE `ws_fut` unblocks!
    assert!(
        tokio::time::timeout(OP_TIMEOUT, open_child1_task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(
        tokio::time::timeout(OP_TIMEOUT, open_main_task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(
        tokio::time::timeout(OP_TIMEOUT, open_child2_task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );

    // Drain Event backpressure and complete workspace close + all spawned operations.
    while rx.try_recv().is_ok() {}
    tokio::time::timeout(OP_TIMEOUT, ws_fut).await.unwrap();

    let mut saw_child1_stopped = false;
    let mut commit_result = None;
    tokio::time::timeout(OP_TIMEOUT, async {
        loop {
            while let Ok(ev) = rx.try_recv() {
                match ev {
                    Event::PluginStopped { identity, .. }
                        if identity.instance_id == child1_id =>
                    {
                        saw_child1_stopped = true;
                    }
                    Event::PluginOperation {
                        operation_id,
                        result,
                        ..
                    } if operation_id == commit_main_op => {
                        commit_result = Some(result);
                    }
                    _ => {}
                }
            }
            if saw_child1_stopped
                && commit_result.is_some()
                && state.plugins.operations.is_empty()
            {
                break;
            }
            let sig = state.plugins.next_signal().await;
            state.plugin_signal(sig).await;
        }
    })
    .await
    .unwrap();

    assert!(saw_child1_stopped);
    assert!(commit_result.unwrap().is_err());

    {
        let opens = received_opens.lock().unwrap();
        assert_eq!(
            opens.len(),
            4,
            "unexpected opens reached worker after backpressured revoke: {opens:?}"
        );
        assert!(
            opens.iter().all(|(_, rid, rev)| *rid != extra_child1 && *rev != 2),
            "stale opens reached worker: {opens:?}"
        );
    }
    assert!(
        received_commits.lock().unwrap().is_empty(),
        "revoked CommitEditor must never reach worker"
    );
    {
        let closes = received_closes.lock().unwrap();
        assert!(closes.contains(&(child1_panel, tab_child1, true)), "{closes:?}");
        assert!(closes.contains(&(protocol::PANEL_ID, tab_main, false)), "{closes:?}");
        assert!(closes.contains(&(child2_panel, tab_main, false)), "{closes:?}");
    }

    let dis_parent = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            dis_parent,
            PluginAction::Disable {
                instance_id: parent_id,
            },
        )
        .await
        .unwrap();
    assert!(drain_op(&mut state, &mut rx, dis_parent).await.is_ok());
    let _ = forwarder_task.await;
    tokio::time::timeout(OP_TIMEOUT, worker_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn service_unpolled_commits_cancelled_synchronously_with_bound_resources() {
    use std::time::Duration;
    use mio_plugin_api::{
        panel,
        protocol::{self, Committed, Envelope, ResponseCommitted, envelope},
    };
    use tokio::io::AsyncWriteExt;

    const OP_TIMEOUT: Duration = Duration::from_secs(5);

    let (host_tx, mut worker_rx) = tokio::io::duplex(64 * 1024);
    let (mut worker_tx, host_rx) = tokio::io::duplex(64 * 1024);

    let commit_requests = Arc::new(std::sync::Mutex::new(Vec::<(u64, Uuid, u64)>::new()));
    let commit_responses = Arc::new(std::sync::Mutex::new(Vec::<(u64, Uuid, u64)>::new()));
    let commit_reqs_w = commit_requests.clone();
    let commit_resps_w = commit_responses.clone();

    let worker_task = tokio::spawn(async move {
        let hello = read_mux_duplex_frame(&mut worker_rx).await?;
        let identity = ChannelIdentity {
            instance_id: protocol::uuid(&hello.instance_id).map_err(anyhow::Error::msg)?,
            project: mio_plugin_api::ProjectKey {
                id: protocol::uuid(&hello.project_id).map_err(anyhow::Error::msg)?,
                generation: hello.project_generation,
            },
        };
        let send_frame = |seq: &mut u64,
                          panel_id: u64,
                          rev: u64,
                          in_reply_to: u64,
                          body: envelope::Body| {
            *seq += 1;
            let mut env = Envelope::new(identity, *seq, rev, body, OP_TIMEOUT).with_panel_id(panel_id);
            env.in_reply_to = in_reply_to;
            env
        };
        let mut out_seq = 0u64;
        let reg = send_frame(
            &mut out_seq,
            protocol::PANEL_ID,
            1,
            hello.request_id,
            envelope::Body::Register(protocol::Register {
                plugin_id: "lab.duplex-service".into(),
                plugin_version: "1.5.0".into(),
                required_components: vec![],
                panel: Some(make_duplex_panel_snapshot(1, "main")),
                supports_panel_multiplexing: true,
            }),
        );
        worker_tx.write_all(&protocol::encode_frame(&reg)?).await?;
        worker_tx.flush().await?;
        let _ready = read_mux_duplex_frame(&mut worker_rx).await?;

        let mut revs: HashMap<u64, u64> = HashMap::from([(protocol::PANEL_ID, 1)]);
        while let Ok(env) = read_mux_duplex_frame(&mut worker_rx).await {
            match env.body {
                    Some(envelope::Body::Shutdown(_)) => break,
                    Some(envelope::Body::OpenRequest(_))
                    | Some(envelope::Body::OpenResponse(_)) => {
                        let rev = revs.entry(env.panel_id).or_insert(0);
                        *rev += 1;
                        let snap = send_frame(
                            &mut out_seq,
                            env.panel_id,
                            *rev,
                            env.request_id,
                            envelope::Body::Snapshot(make_duplex_panel_snapshot(
                                *rev,
                                &format!("p-{}-bound", env.panel_id),
                            )),
                        );
                        worker_tx.write_all(&protocol::encode_frame(&snap)?).await?;
                        worker_tx.flush().await?;
                    }
                    Some(envelope::Body::CommitEditor(ref commit)) => {
                        let rid = protocol::uuid(&commit.resource_id).map_err(anyhow::Error::msg)?;
                        commit_reqs_w
                            .lock()
                            .unwrap()
                            .push((env.panel_id, rid, commit.message_revision));
                        let rev = *revs.get(&env.panel_id).unwrap_or(&1);
                        let committed = send_frame(
                            &mut out_seq,
                            env.panel_id,
                            rev,
                            env.request_id,
                            envelope::Body::Committed(Committed {
                                resource_id: commit.resource_id.clone(),
                                message_revision: commit.message_revision,
                                document_id: 1,
                                document_revision: commit.editor.as_ref().map_or(1, |e| e.revision),
                                request: commit.request.clone(),
                            }),
                        );
                        worker_tx
                            .write_all(&protocol::encode_frame(&committed)?)
                            .await?;
                        worker_tx.flush().await?;
                    }
                    Some(envelope::Body::CommitResponseEditor(ref commit)) => {
                        let rid = protocol::uuid(&commit.resource_id).map_err(anyhow::Error::msg)?;
                        commit_resps_w
                            .lock()
                            .unwrap()
                            .push((env.panel_id, rid, commit.message_revision));
                        let rev = *revs.get(&env.panel_id).unwrap_or(&1);
                        let committed = send_frame(
                            &mut out_seq,
                            env.panel_id,
                            rev,
                            env.request_id,
                            envelope::Body::ResponseCommitted(ResponseCommitted {
                                resource_id: commit.resource_id.clone(),
                                message_revision: commit.message_revision,
                                document_id: 1,
                                document_revision: commit.editor.as_ref().map_or(1, |e| e.revision),
                                response: commit.response.clone(),
                            }),
                        );
                        worker_tx
                            .write_all(&protocol::encode_frame(&committed)?)
                            .await?;
                        worker_tx.flush().await?;
                    }
                    Some(envelope::Body::CloseRequest(_)) => {
                        let rev = *revs.get(&env.panel_id).unwrap_or(&1);
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
        Ok::<_, anyhow::Error>(())
    });

    let (mut state, mut rx, _events_tx, parent_id, _dir) =
        setup_duplex_service_state(256, host_tx, host_rx).await;

    // Sub-scenario 1: main panel `CommitEditor` on genuinely bound `tab_main`, cancelled by tab close BEFORE first poll.
    let (tab_main, rev_main) = add_tab(&mut state, "main-bound", "http://127.0.0.1/main");
    let open_main = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            open_main,
            PluginAction::OpenRequest {
                instance_id: parent_id,
                tab_id: tab_main,
                draft_revision: rev_main,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        drain_op(&mut state, &mut rx, open_main).await.unwrap(),
        PluginOutcome::Opened { .. }
    ));

    let commit_main_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            commit_main_op,
            PluginAction::CommitEditor {
                instance_id: parent_id,
                tab_id: tab_main,
                draft_revision: rev_main,
                editor: EditorSnapshot {
                    document_id: 1,
                    revision: 2,
                    contents: b"unpolled-main".to_vec(),
                    ..Default::default()
                },
                            expected_panel: None,
            },
        )
        .await
        .unwrap();
    // Zero yield points before synchronous tab-close removal & waiter cancellation!
    let mut ws = state.store.workspace().unwrap();
    ws.tabs.retain(|t| t.id != tab_main);
    ws.active = None;
    ws.revision += 1;
    state.store.save_workspace(&ws).unwrap();
    state.plugin_workspace_changed(&ws).await;

    assert!(drain_op(&mut state, &mut rx, commit_main_op).await.is_err());
    while !state.plugins.operations.is_empty() {
        let sig = state.plugins.next_signal().await;
        state.plugin_signal(sig).await;
    }
    assert!(commit_requests.lock().unwrap().is_empty());

    // Sub-scenario 2: writable multiplexed `CommitContextEditor` on genuinely bound `ctx_req`,
    // cancelled by `Disable` BEFORE first poll.
    let req_ctx = parse_request(
        b"GET /ctx HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        "http://127.0.0.1/ctx",
        HttpVersion::Http11,
    )
    .unwrap();
    let ctx_req = Uuid::new_v4();
    let open_ctx = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            open_ctx,
            PluginAction::OpenContextViewer {
                source_instance: parent_id,
                context_id: ctx_req,
                revision: 1,
                read_only: false,
                request: req_ctx.clone(),
            },
        )
        .await
        .unwrap();
    let PluginOutcome::ContextOpened {
        info: ctx_info, ..
    } = drain_op(&mut state, &mut rx, open_ctx).await.unwrap()
    else {
        panic!("expected ContextOpened");
    };
    let ctx_inst = ctx_info.identity.instance_id;

    let commit_ctx_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            commit_ctx_op,
            PluginAction::CommitContextEditor {
                instance_id: ctx_inst,
                context_id: ctx_req,
                revision: 1,
                request: req_ctx,
                editor: EditorSnapshot {
                    document_id: 1,
                    revision: 2,
                    contents: b"unpolled-ctx".to_vec(),
                    ..Default::default()
                },
                            expected_panel: None,
            },
        )
        .await
        .unwrap();
    let disable_ctx_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            disable_ctx_op,
            PluginAction::Disable {
                instance_id: ctx_inst,
            },
        )
        .await
        .unwrap();
    assert!(drain_op(&mut state, &mut rx, commit_ctx_op).await.is_err());
    assert!(drain_op(&mut state, &mut rx, disable_ctx_op).await.is_ok());
    while !state.plugins.operations.is_empty() {
        let sig = state.plugins.next_signal().await;
        state.plugin_signal(sig).await;
    }
    assert!(commit_requests.lock().unwrap().is_empty());

    // Sub-scenario 3: writable multiplexed `CommitResponseEditor` on genuinely bound `ctx_resp`,
    // cancelled by `Disable` BEFORE first poll.
    let resp_fixture = HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: Some("OK".into()),
        headers: vec![],
        body: b"orig-resp".to_vec(),
        trailers: vec![],
    };
    let ctx_resp = Uuid::new_v4();
    let open_resp = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            open_resp,
            PluginAction::OpenResponseViewer {
                source_instance: parent_id,
                context_id: ctx_resp,
                revision: 1,
                read_only: false,
                response: resp_fixture.clone(),
            },
        )
        .await
        .unwrap();
    let PluginOutcome::ResponseOpened {
        info: resp_info, ..
    } = drain_op(&mut state, &mut rx, open_resp).await.unwrap()
    else {
        panic!("expected ResponseOpened");
    };
    let resp_inst = resp_info.identity.instance_id;

    let commit_resp_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            commit_resp_op,
            PluginAction::CommitResponseEditor {
                instance_id: resp_inst,
                context_id: ctx_resp,
                revision: 1,
                response: resp_fixture,
                editor: EditorSnapshot {
                    document_id: 1,
                    revision: 2,
                    contents: b"unpolled-resp".to_vec(),
                    ..Default::default()
                },
                            expected_panel: None,
            },
        )
        .await
        .unwrap();
    let disable_resp_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            disable_resp_op,
            PluginAction::Disable {
                instance_id: resp_inst,
            },
        )
        .await
        .unwrap();
    assert!(drain_op(&mut state, &mut rx, commit_resp_op).await.is_err());
    assert!(drain_op(&mut state, &mut rx, disable_resp_op).await.is_ok());
    while !state.plugins.operations.is_empty() {
        let sig = state.plugins.next_signal().await;
        state.plugin_signal(sig).await;
    }
    assert!(commit_responses.lock().unwrap().is_empty());

    // Sub-scenario 4 (positive control): genuinely bound `tab_live` on main panel commits cleanly.
    let (tab_live, rev_live) = add_tab(&mut state, "live", "http://127.0.0.1/live");
    let open_live = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            open_live,
            PluginAction::OpenRequest {
                instance_id: parent_id,
                tab_id: tab_live,
                draft_revision: rev_live,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        drain_op(&mut state, &mut rx, open_live).await.unwrap(),
        PluginOutcome::Opened { .. }
    ));

    let commit_live_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            commit_live_op,
            PluginAction::CommitEditor {
                instance_id: parent_id,
                tab_id: tab_live,
                draft_revision: rev_live,
                editor: EditorSnapshot {
                    document_id: 1,
                    revision: 2,
                    contents: b"live-commit".to_vec(),
                    ..Default::default()
                },
                            expected_panel: None,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        drain_op(&mut state, &mut rx, commit_live_op).await.unwrap(),
        PluginOutcome::Committed { tab_id, .. } if tab_id == tab_live
    ));
    assert_eq!(
        commit_requests.lock().unwrap().as_slice(),
        &[(protocol::PANEL_ID, tab_live, rev_live)]
    );

    let dis_parent = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            dis_parent,
            PluginAction::Disable {
                instance_id: parent_id,
            },
        )
        .await
        .unwrap();
    assert!(drain_op(&mut state, &mut rx, dis_parent).await.is_ok());
    tokio::time::timeout(OP_TIMEOUT, worker_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn service_workspace_close_two_tabs_on_same_panel_cancels_deferred_second_open_before_first_close_ack()
{
    use std::time::Duration;
    use mio_plugin_api::{
        panel,
        protocol::{self, Envelope, envelope},
    };
    use tokio::io::AsyncWriteExt;

    const OP_TIMEOUT: Duration = Duration::from_secs(5);

    let (host_tx, mut worker_rx) = tokio::io::duplex(64 * 1024);
    let (mut worker_tx, host_rx) = tokio::io::duplex(64 * 1024);

    let main_hold_id = Arc::new(std::sync::Mutex::new(None::<Uuid>));
    let child_hold_id = Arc::new(std::sync::Mutex::new(None::<Uuid>));
    let main_hold_w = main_hold_id.clone();
    let child_hold_w = child_hold_id.clone();
    let saw_main_hold = Arc::new(tokio::sync::Notify::new());
    let saw_child_hold = Arc::new(tokio::sync::Notify::new());
    let release_main_hold = Arc::new(tokio::sync::Notify::new());
    let release_child_hold = Arc::new(tokio::sync::Notify::new());
    let saw_main_hold_w = saw_main_hold.clone();
    let saw_child_hold_w = saw_child_hold.clone();
    let release_main_hold_w = release_main_hold.clone();
    let release_child_hold_w = release_child_hold.clone();

    let received_opens = Arc::new(std::sync::Mutex::new(Vec::<(u64, Uuid, u64)>::new()));
    let received_closes = Arc::new(std::sync::Mutex::new(Vec::<(u64, Uuid, bool)>::new()));
    let max_inflight_by_panel = Arc::new(std::sync::Mutex::new(HashMap::<u64, usize>::new()));
    let opens_w = received_opens.clone();
    let closes_w = received_closes.clone();
    let max_inflight_w = max_inflight_by_panel.clone();

    let worker_task = tokio::spawn(async move {
        let hello = read_mux_duplex_frame(&mut worker_rx).await?;
        let identity = ChannelIdentity {
            instance_id: protocol::uuid(&hello.instance_id).map_err(anyhow::Error::msg)?,
            project: mio_plugin_api::ProjectKey {
                id: protocol::uuid(&hello.project_id).map_err(anyhow::Error::msg)?,
                generation: hello.project_generation,
            },
        };
        let send_frame = |seq: &mut u64,
                          panel_id: u64,
                          rev: u64,
                          in_reply_to: u64,
                          body: envelope::Body| {
            *seq += 1;
            let mut env = Envelope::new(identity, *seq, rev, body, OP_TIMEOUT).with_panel_id(panel_id);
            env.in_reply_to = in_reply_to;
            env
        };
        let mut out_seq = 0u64;
        let reg = send_frame(
            &mut out_seq,
            protocol::PANEL_ID,
            1,
            hello.request_id,
            envelope::Body::Register(protocol::Register {
                plugin_id: "lab.duplex-service".into(),
                plugin_version: "1.5.0".into(),
                required_components: vec![],
                panel: Some(make_duplex_panel_snapshot(1, "main")),
                supports_panel_multiplexing: true,
            }),
        );
        worker_tx.write_all(&protocol::encode_frame(&reg)?).await?;
        worker_tx.flush().await?;
        let _ready = read_mux_duplex_frame(&mut worker_rx).await?;

        let mut revs: HashMap<u64, u64> = HashMap::from([(protocol::PANEL_ID, 1)]);
        let mut inflight: HashMap<u64, usize> = HashMap::new();
        let mut held_main: Option<(u64, u64, u64)> = None;
        let mut held_child: Option<(u64, u64, u64)> = None;
        loop {
            tokio::select! {
                biased;
                _ = release_main_hold_w.notified(), if held_main.is_some() => {
                    let (panel_id, req_id, rev) = held_main.take().unwrap();
                    let cur = inflight.entry(panel_id).or_insert(0);
                    *cur = cur.saturating_sub(1);
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
                _ = release_child_hold_w.notified(), if held_child.is_some() => {
                    let (panel_id, req_id, rev) = held_child.take().unwrap();
                    let cur = inflight.entry(panel_id).or_insert(0);
                    *cur = cur.saturating_sub(1);
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
                incoming = read_mux_duplex_frame(&mut worker_rx) => {
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
                                envelope::Body::Snapshot(make_duplex_panel_snapshot(
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
                            let cur = inflight.entry(env.panel_id).or_insert(0);
                            *cur += 1;
                            {
                                let mut max = max_inflight_w.lock().unwrap();
                                let entry = max.entry(env.panel_id).or_insert(0);
                                *entry = (*entry).max(*cur);
                            }
                            let rev = *revs.get(&env.panel_id).unwrap_or(&1);
                            if !close.destroy_panel && *main_hold_w.lock().unwrap() == Some(rid) {
                                held_main = Some((env.panel_id, env.request_id, rev));
                                saw_main_hold_w.notify_one();
                            } else if !close.destroy_panel && *child_hold_w.lock().unwrap() == Some(rid) {
                                held_child = Some((env.panel_id, env.request_id, rev));
                                saw_child_hold_w.notify_one();
                            } else {
                                *cur = cur.saturating_sub(1);
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

    let (mut state, mut rx, _events_tx, parent_id, _dir) =
        setup_duplex_service_state(256, host_tx, host_rx).await;
    let parent_handle = state.plugins.instances[&parent_id].worker.handle();

    // Part A: Main panel (`PANEL_ID`) has two closed resources `[tab_a, tab_b]`; older `Open(tab_b, rev=2)`
    // is deferred behind in-flight `CloseRequest(hold_tab)` when ONE `plugin_workspace_changed` closes both.
    let (tab_hold, rev_hold) = add_tab(&mut state, "m-hold", "http://127.0.0.1/mh");
    let (tab_a, rev_a) = add_tab(&mut state, "m-a", "http://127.0.0.1/ma");
    let (tab_b, rev_b) = add_tab(&mut state, "m-b", "http://127.0.0.1/mb");
    *main_hold_id.lock().unwrap() = Some(tab_hold);

    for (tab_id, rev) in [(tab_hold, rev_hold), (tab_a, rev_a), (tab_b, rev_b)] {
        let op = Uuid::new_v4();
        state
            .plugin_command(
                state.project,
                op,
                PluginAction::OpenRequest {
                    instance_id: parent_id,
                    tab_id,
                    draft_revision: rev,
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            drain_op(&mut state, &mut rx, op).await.unwrap(),
            PluginOutcome::Opened { .. }
        ));
    }

    // Hold CloseRequest(tab_hold) on main panel so Running::is_panel_command_barrier_active(PANEL_ID) is active
    // while `state` still allows submitting `PluginAction::OpenRequest` for `tab_b (rev = 2)`.
    let h_main = parent_handle.clone();
    let hold_main_task = tokio::spawn(async move {
        h_main
            .revoke_panel_resource(protocol::PANEL_ID, tab_hold, false)
            .await
    });
    tokio::time::timeout(OP_TIMEOUT, saw_main_hold.notified())
        .await
        .unwrap();

    // Bump `tab_b` draft revision to 2 and submit `OpenRequest(tab_b, rev=2)` via service layer.
    {
        let mut ws = state.store.workspace().unwrap();
        let tab = ws.tabs.iter_mut().find(|t| t.id == tab_b).unwrap();
        tab.draft.revision = 2;
        ws.revision += 1;
        state.store.save_workspace(&ws).unwrap();
    }
    let open_b_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            open_b_op,
            PluginAction::OpenRequest {
                instance_id: parent_id,
                tab_id: tab_b,
                draft_revision: 2,
            },
        )
        .await
        .unwrap();
    // Wait until Running pops Open(tab_b, rev=2) from `handle.control` and parks it in `panels[&PANEL_ID].deferred_commands`.
    tokio::time::sleep(Duration::from_millis(35)).await;
    assert!(
        received_opens
            .lock()
            .unwrap()
            .iter()
            .all(|(_, rid, rev)| !(*rid == tab_b && *rev == 2)),
        "Open(tab_b, rev=2) must be deferred in Running behind CloseRequest(tab_hold)"
    );

    // Close BOTH `tab_a` and `tab_b` in one `plugin_workspace_changed` call.
    {
        let mut ws = state.store.workspace().unwrap();
        ws.tabs.retain(|t| t.id != tab_a && t.id != tab_b);
        ws.active = Some(tab_hold);
        ws.revision += 1;
        state.store.save_workspace(&ws).unwrap();
        let ops_before = state.plugins.operations.len();
        state.plugin_workspace_changed(&ws).await;
        assert_eq!(state.plugins.operations.len(), ops_before + 1);
    }

    // `open_b_op` MUST fail IMMEDIATELY BEFORE `CloseRequest(tab_hold)` receives its Ack!
    assert!(
        tokio::time::timeout(OP_TIMEOUT, drain_op(&mut state, &mut rx, open_b_op))
            .await
            .unwrap()
            .is_err(),
        "batch revoke on main panel must cancel deferred Open(tab_b, rev=2) before CloseRequest(tab_hold) Ack"
    );

    release_main_hold.notify_one();
    tokio::time::timeout(OP_TIMEOUT, hold_main_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(OP_TIMEOUT, async {
        while !state.plugins.operations.is_empty() {
            let sig = state.plugins.next_signal().await;
            state.plugin_signal(sig).await;
        }
    })
    .await
    .unwrap();
    assert!(
        received_opens
            .lock()
            .unwrap()
            .iter()
            .all(|(_, rid, rev)| !(*rid == tab_b && *rev == 2))
    );

    // Part B: Live multiplexed child panel with two closed extra resources `[c_a, c_b]`;
    // older `Open(c_b, rev=2)` deferred behind `CloseRequest(c_hold)` when ONE `plugin_workspace_changed` closes both.
    let ctx_req = parse_request(
        b"GET /ctx HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        "http://127.0.0.1/ctx",
        HttpVersion::Http11,
    )
    .unwrap();
    let ctx_id = Uuid::new_v4();
    let open_child = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            open_child,
            PluginAction::OpenContextViewer {
                source_instance: parent_id,
                context_id: ctx_id,
                revision: 1,
                read_only: false,
                request: ctx_req,
            },
        )
        .await
        .unwrap();
    let PluginOutcome::ContextOpened {
        info: child_info, ..
    } = drain_op(&mut state, &mut rx, open_child).await.unwrap()
    else {
        panic!("expected ContextOpened");
    };
    let child_id = child_info.identity.instance_id;
    let child_panel = state.plugins.multiplexed[&child_id].panel_id;

    let (c_hold, c_rev_hold) = add_tab(&mut state, "c-hold", "http://127.0.0.1/ch");
    let (c_a, c_rev_a) = add_tab(&mut state, "c-a", "http://127.0.0.1/ca");
    let (c_b, c_rev_b) = add_tab(&mut state, "c-b", "http://127.0.0.1/cb");
    *child_hold_id.lock().unwrap() = Some(c_hold);

    for (tab_id, rev) in [(c_hold, c_rev_hold), (c_a, c_rev_a), (c_b, c_rev_b)] {
        let op = Uuid::new_v4();
        state
            .plugin_command(
                state.project,
                op,
                PluginAction::OpenRequest {
                    instance_id: child_id,
                    tab_id,
                    draft_revision: rev,
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            drain_op(&mut state, &mut rx, op).await.unwrap(),
            PluginOutcome::Opened { .. }
        ));
    }

    let h_child = parent_handle.clone();
    let hold_child_task = tokio::spawn(async move {
        h_child.revoke_panel_resource(child_panel, c_hold, false).await
    });
    tokio::time::timeout(OP_TIMEOUT, saw_child_hold.notified())
        .await
        .unwrap();

    {
        let mut ws = state.store.workspace().unwrap();
        let tab = ws.tabs.iter_mut().find(|t| t.id == c_b).unwrap();
        tab.draft.revision = 2;
        ws.revision += 1;
        state.store.save_workspace(&ws).unwrap();
    }
    let open_cb_op = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            open_cb_op,
            PluginAction::OpenRequest {
                instance_id: child_id,
                tab_id: c_b,
                draft_revision: 2,
            },
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(35)).await;
    assert!(
        received_opens
            .lock()
            .unwrap()
            .iter()
            .all(|(_, rid, rev)| !(*rid == c_b && *rev == 2)),
        "Open(c_b, rev=2) must be deferred in Running behind CloseRequest(c_hold)"
    );

    {
        let mut ws = state.store.workspace().unwrap();
        ws.tabs.retain(|t| t.id != c_a && t.id != c_b);
        ws.active = Some(tab_hold);
        ws.revision += 1;
        state.store.save_workspace(&ws).unwrap();
        let ops_before = state.plugins.operations.len();
        state.plugin_workspace_changed(&ws).await;
        assert_eq!(state.plugins.operations.len(), ops_before + 1);
    }

    assert!(
        tokio::time::timeout(OP_TIMEOUT, drain_op(&mut state, &mut rx, open_cb_op))
            .await
            .unwrap()
            .is_err(),
        "batch revoke on child panel must cancel deferred Open(c_b, rev=2) before CloseRequest(c_hold) Ack"
    );

    release_child_hold.notify_one();
    tokio::time::timeout(OP_TIMEOUT, hold_child_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(OP_TIMEOUT, async {
        while !state.plugins.operations.is_empty() {
            let sig = state.plugins.next_signal().await;
            state.plugin_signal(sig).await;
        }
    })
    .await
    .unwrap();

    assert!(state.plugins.multiplexed.contains_key(&child_id));
    assert!(
        received_opens
            .lock()
            .unwrap()
            .iter()
            .all(|(_, rid, rev)| !(*rid == c_b && *rev == 2))
    );
    {
        let max_inflight = max_inflight_by_panel.lock().unwrap();
        assert_eq!(max_inflight.get(&protocol::PANEL_ID), Some(&1));
        assert_eq!(max_inflight.get(&child_panel), Some(&1));
    }
    {
        let closes = received_closes.lock().unwrap();
        for expected in [
            (protocol::PANEL_ID, tab_hold, false),
            (protocol::PANEL_ID, tab_a, false),
            (protocol::PANEL_ID, tab_b, false),
            (child_panel, c_hold, false),
            (child_panel, c_a, false),
            (child_panel, c_b, false),
        ] {
            assert!(closes.contains(&expected), "missing close {expected:?} in {closes:?}");
        }
    }

    let dis_parent = Uuid::new_v4();
    state
        .plugin_command(
            state.project,
            dis_parent,
            PluginAction::Disable {
                instance_id: parent_id,
            },
        )
        .await
        .unwrap();
    assert!(drain_op(&mut state, &mut rx, dis_parent).await.is_ok());
    tokio::time::timeout(OP_TIMEOUT, worker_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
