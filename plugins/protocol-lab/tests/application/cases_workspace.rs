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
