use base64::{Engine, engine::general_purpose::STANDARD};
use tokio::io::AsyncReadExt;

use super::*;

#[tokio::test]
async fn application_workspace_proposal_is_atomic_and_cannot_edit_other_tabs() {
    let directory = package();
    let mut app = App::start().await;
    let tab_id = app.add_tab("http://127.0.0.1:9/plugin", "原稿").await;
    app.add_tab("http://127.0.0.1:9/plugin", "不能更改").await;
    let source = app.load(&directory, None).await;
    let id = source.identity.instance_id;
    let mut panel = app.open(id, tab_id).await;
    for malicious in [true, false] {
        let mut editors = snapshots(&panel);
        let raw = editors
            .iter_mut()
            .find(|editor| editor.document_id == 100)
            .unwrap();
        raw.revision += 1;
        raw.contents = STANDARD.encode("回写中文").into_bytes();
        app.click(id, &panel, 15, editors).await;
        let call = loop {
            if let Event::PluginWorkspaceRequest(call) = app.next().await {
                break call;
            }
        };
        let PluginWorkspaceEdit::UpdateDraft {
            tab_id: actual,
            expected_revision,
            request,
        } = call.edit
        else {
            panic!("update draft");
        };
        assert_eq!(actual, tab_id);
        let before = app.workspace.clone();
        let mut proposal = before.clone();
        proposal.revision += 1;
        let tab = proposal
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .unwrap();
        tab.draft = Draft {
            revision: expected_revision + 1,
            target: request.url.clone(),
            version: request.version,
            raw: mio_core::request_to_raw(&request),
        };
        if malicious {
            proposal.tabs[1].title = "not authorized by this host call".into();
        }
        let op = app
            .action(PluginAction::ApplyWorkspace {
                instance_id: id,
                request_id: call.request_id,
                workspace: proposal.clone(),
                tab_id,
            })
            .await;
        let result = app.outcome(op).await;
        if malicious {
            assert!(result.unwrap_err().contains("未经请求"));
            assert_eq!(app.workspace, before);
        } else {
            assert!(matches!(result.unwrap(), PluginOutcome::WorkspaceApplied));
            assert_eq!(
                app.workspace, proposal,
                "WorkspaceChanged ACK must precede completion"
            );
        }
        let prior = panel.revision;
        app.panel(id, |panel| {
            panel.revision > prior && !panel.title.is_empty()
        })
        .await;
        // Wait for the actual host-result status, not the earlier busy patch.
        panel = app
            .panel(id, |panel| {
                let Some(node::Kind::Text(status)) = panel
                    .root
                    .as_ref()
                    .unwrap()
                    .find(4)
                    .and_then(|node| node.kind.as_ref())
                else {
                    return false;
                };
                if malicious {
                    status.text.contains("未经请求")
                } else {
                    status.text.contains("已回写实际草稿")
                }
            })
            .await;
    }
    app.shutdown();
}

#[tokio::test]
async fn application_reload_and_disable_join_actual_network_and_persist_terminal_records() {
    let directory = package();
    let mut app = App::start().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tab = app
        .add_tab(
            &format!("http://{}/plugin", listener.local_addr().unwrap()),
            "原稿",
        )
        .await;
    let mut plugin = app.load(&directory, None).await;
    for reload in [true, false] {
        let panel = app.open(plugin.identity.instance_id, tab).await;
        let (received, request_received) = oneshot::channel();
        let (released, peer_released) = oneshot::channel();
        let accept = listener.accept();
        app.click(plugin.identity.instance_id, &panel, 12, snapshots(&panel))
            .await;
        let (mut socket, _) = timeout(BOUND, accept).await.unwrap().unwrap();
        let origin = tokio::spawn(async move {
            let mut data = Vec::new();
            let mut received = Some(received);
            loop {
                let count = socket.read_buf(&mut data).await.unwrap();
                if count == 0 {
                    released.send(()).unwrap();
                    return data;
                }
                assert!(data.len() < 8192);
                if data.windows(4).any(|bytes| bytes == b"\r\n\r\n")
                    && let Some(received) = received.take()
                {
                    received.send(()).unwrap();
                }
            }
        });
        timeout(BOUND, request_received).await.unwrap().unwrap();
        let old = plugin.identity.instance_id;
        if reload {
            plugin = app.load(&directory, Some(old)).await;
            assert_ne!(plugin.identity.instance_id, old);
        } else {
            let op = app.action(PluginAction::Disable { instance_id: old }).await;
            assert!(matches!(
                app.outcome(op).await.unwrap(),
                PluginOutcome::Disabled { .. }
            ));
        }
        assert!(
            app.stopped.contains(&old),
            "worker retirement precedes visible lifecycle completion"
        );
        timeout(BOUND, peer_released)
            .await
            .expect("real connection closed after lifecycle ACK")
            .unwrap();
        let received = origin.await.unwrap();
        assert!(received.starts_with(b"POST /plugin HTTP/1.1\r\n"));
    }
    app.client
        .commands
        .send(Command::QueryHistory {
            project: app.project,
            query_revision: 1,
            offset: 0,
            limit: 10,
            filter: String::new(),
        })
        .await
        .unwrap();
    let rows = loop {
        if let Event::HistoryPage { rows, .. } = app.next().await {
            break rows;
        }
    };
    assert_eq!(rows.len(), 2);
    for row in &rows {
        let mio_core::HistoryRow::Live(summary) = row else {
            panic!("plugin-generated requests must remain live history rows");
        };
        assert_eq!(summary.source, TrafficSource::Plugin("lab.protocol".into()));
        assert_eq!(summary.state, FlowState::Cancelled);
    }
    app.shutdown();
}

fn multiplex_e2e_package() -> TempDir {
    let directory = tempfile::tempdir().unwrap();
    let bin = std::env::var("MIO_TEST_PROTOCOL_LAB_WORKER_BIN")
        .unwrap_or_else(|_| env!("CARGO_BIN_EXE_protocol-lab-test-worker").into());
    let entry = if cfg!(windows) {
        "multiplex-e2e.exe"
    } else {
        "multiplex-e2e"
    };
    fs::copy(&bin, directory.path().join(entry)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            directory.path().join(entry),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }
    fs::write(
        directory.path().join("manifest.json"),
        serde_json::to_vec(&serde_json::json!({
            "id": "lab.multiplex-e2e",
            "version": "1.5.0",
            "api_version": 1,
            "engine": "native",
            "entry": entry,
            "permissions": ["traffic.read", "draft.write", "repeater.open"],
            "contributions": ["suite_panel", "message_editor", "message_response_editor"]
        }))
        .unwrap(),
    )
    .unwrap();
    directory
}

#[tokio::test]
async fn application_real_process_multiplex_open_repeater_and_close_scope() {
    let directory = multiplex_e2e_package();
    let mut app = App::start().await;
    let tab1 = app.add_tab("http://127.0.0.1:9/p1", "one").await;
    let tab2 = app.add_tab("http://127.0.0.1:9/p2", "two").await;

    let op = app
        .action(PluginAction::Inspect {
            directory: directory.path().to_owned(),
        })
        .await;
    let PluginOutcome::Inspected(inspection) = app.outcome(op).await.unwrap() else {
        panic!("expected Inspected");
    };
    let op = app
        .action(PluginAction::Load {
            inspection_id: inspection.id,
            native_trusted: true,
            grants: vec![
                Permission::TrafficRead,
                Permission::DraftWrite,
                Permission::RepeaterOpen,
            ],
            replace_instance: None,
        })
        .await;
    let PluginOutcome::Loaded {
        info: parent_info,
        panel: parent_panel,
    } = app.outcome(op).await.unwrap()
    else {
        panic!("expected parent Loaded");
    };
    let parent_id = parent_info.identity.instance_id;
    let Some(node::Kind::Text(pid_text)) = parent_panel
        .root
        .as_ref()
        .and_then(|r| r.find(1))
        .and_then(|n| n.kind.as_ref())
    else {
        panic!("expected root pid text node");
    };
    let main_pid = pid_text.text.strip_prefix("pid:").unwrap().to_owned();

    // Open two child repeater editors + one standalone context viewer inside the SAME OS process.
    let open_child = |ws: &Workspace, tab_id: Uuid| {
        let rev = ws
            .tabs
            .iter()
            .find(|t| t.id == tab_id)
            .unwrap()
            .draft
            .revision;
        PluginAction::CreateEditor {
            source_instance: parent_id,
            tab_id,
            draft_revision: rev,
        }
    };
    let op1 = app.action(open_child(&app.workspace, tab1)).await;
    let PluginOutcome::Loaded {
        info: child1_info,
        panel: child1_panel,
    } = app.outcome(op1).await.unwrap()
    else {
        panic!("expected child1 Loaded");
    };
    let child1_id = child1_info.identity.instance_id;
    assert_eq!(
        String::from_utf8_lossy(&editor(&child1_panel, 2).contents),
        format!("pid:{main_pid}:panel:2")
    );

    let op2 = app.action(open_child(&app.workspace, tab2)).await;
    let PluginOutcome::Loaded {
        info: child2_info,
        panel: child2_panel,
    } = app.outcome(op2).await.unwrap()
    else {
        panic!("expected child2 Loaded");
    };
    let child2_id = child2_info.identity.instance_id;
    assert_eq!(
        String::from_utf8_lossy(&editor(&child2_panel, 2).contents),
        format!("pid:{main_pid}:panel:3")
    );

    let ctx_id = Uuid::new_v4();
    let ctx_req = mio_core::parse_request(
        b"GET /ctx HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        "http://127.0.0.1/ctx",
        mio_core::HttpVersion::Http11,
    )
    .unwrap();
    let op_ctx = app
        .action(PluginAction::OpenContextViewer {
            source_instance: parent_id,
            context_id: ctx_id,
            revision: 1,
            read_only: false,
            request: ctx_req,
        })
        .await;
    let PluginOutcome::ContextOpened {
        info: ctx_info,
        panel: ctx_panel,
        ..
    } = app.outcome(op_ctx).await.unwrap()
    else {
        panic!("expected ContextOpened");
    };
    let ctx_inst = ctx_info.identity.instance_id;
    assert_eq!(
        String::from_utf8_lossy(&editor(&ctx_panel, 2).contents),
        format!("pid:{main_pid}:panel:4")
    );

    // Trigger OpenRepeater from child1 over real OS stdio -> bind extra_tab end-to-end!
    app.click(child1_id, &child1_panel, 10, vec![]).await;
    let ws_call = timeout(BOUND, async {
        loop {
            if let Event::PluginWorkspaceRequest(req) = app.next().await {
                return req;
            }
        }
    })
    .await
    .expect("OpenRepeater workspace request");
    let PluginWorkspaceEdit::OpenRepeater { request: open_req } = ws_call.edit else {
        panic!("expected OpenRepeater workspace request");
    };
    let extra_tab = Uuid::new_v4();
    let extra_tab_struct = mio_core::WorkTab {
        id: extra_tab,
        title: "child-extra".into(),
        group: None,
        draft: Draft {
            revision: 1,
            target: open_req.url.clone(),
            raw: mio_core::request_to_raw(&open_req),
            version: open_req.version,
        },
        last_flow: None,
        archive_response: None,
        history: None,
    };
    let mut proposal = app.workspace.clone();
    proposal.revision += 1;
    proposal.tabs.push(extra_tab_struct);
    proposal.active = Some(extra_tab);
    let apply_op = app
        .action(PluginAction::ApplyWorkspace {
            instance_id: parent_id,
            request_id: ws_call.request_id,
            workspace: proposal.clone(),
            tab_id: extra_tab,
        })
        .await;
    assert!(matches!(
        app.outcome(apply_op).await.unwrap(),
        PluginOutcome::WorkspaceApplied
    ));
    let opened_text = format!("opened:{extra_tab}");
    app.panel(child1_id, |panel| {
        matches!(
            panel.root.as_ref().and_then(|r| r.find(11)).and_then(|n| n.kind.as_ref()),
            Some(node::Kind::Text(t)) if t.text == opened_text
        )
    })
    .await;

    // Closing extra_tab revokes only extra_tab (destroy_panel = false) while keeping child1 live & committable.
    let mut after_close = app.workspace.clone();
    after_close.tabs.retain(|t| t.id != extra_tab);
    after_close.active = Some(tab1);
    app.save(after_close).await;
    let revoked_text = format!("revoked:{extra_tab}");
    let updated_child1 = app
        .panel(child1_id, |panel| {
            matches!(
                panel.root.as_ref().and_then(|r| r.find(11)).and_then(|n| n.kind.as_ref()),
                Some(node::Kind::Text(t)) if t.text == revoked_text
            )
        })
        .await;
    let rev1 = app
        .workspace
        .tabs
        .iter()
        .find(|t| t.id == tab1)
        .unwrap()
        .draft
        .revision;
    let commit1 = app
        .action(PluginAction::CommitEditor {
            instance_id: child1_id,
            tab_id: tab1,
            draft_revision: rev1,
            editor: snapshots(&updated_child1).into_iter().next().unwrap(),
            expected_panel: None,
        })
        .await;
    let PluginOutcome::Committed {
        request: committed1,
        ..
    } = app.outcome(commit1).await.unwrap()
    else {
        panic!("expected child1 Committed");
    };
    assert!(committed1
        .headers
        .iter()
        .any(|h| h.name == "x-worker-pid" && h.value == main_pid.as_bytes()));

    // Disable Context Viewer (destroy_panel = true), while child2 stays alive and committable.
    let dis_ctx = app
        .action(PluginAction::Disable {
            instance_id: ctx_inst,
        })
        .await;
    assert!(matches!(
        app.outcome(dis_ctx).await.unwrap(),
        PluginOutcome::Disabled { .. }
    ));
    let rev2 = app
        .workspace
        .tabs
        .iter()
        .find(|t| t.id == tab2)
        .unwrap()
        .draft
        .revision;
    let commit2 = app
        .action(PluginAction::CommitEditor {
            instance_id: child2_id,
            tab_id: tab2,
            draft_revision: rev2,
            editor: snapshots(&child2_panel).into_iter().next().unwrap(),
            expected_panel: None,
        })
        .await;
    assert!(matches!(
        app.outcome(commit2).await.unwrap(),
        PluginOutcome::Committed { .. }
    ));

    // Disable parent plugin tears down the single OS worker cleanly.
    let dis_parent = app
        .action(PluginAction::Disable {
            instance_id: parent_id,
        })
        .await;
    assert!(matches!(
        app.outcome(dis_parent).await.unwrap(),
        PluginOutcome::Disabled { .. }
    ));
    assert!(app.stopped.contains(&parent_id));
    app.shutdown();
}
