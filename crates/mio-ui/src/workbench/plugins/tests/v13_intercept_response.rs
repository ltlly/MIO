use super::*;
use crate::workbench::tests::record;
use mio_core::{Header, HttpResponse, HttpVersion};

fn install_resp_suite(fixture: &Fixture, cx: &mut TestAppContext) -> LoadedPlugin {
    let info = LoadedPlugin {
        identity: ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: fixture.project,
        },
        directory: PathBuf::from("/tmp/mio-v13-resp-suite"),
        manifest: PluginManifest {
            id: "lab.protocol".into(),
            version: "1.3.0".into(),
            api_version: 1,
            engine: "native".into(),
            entry: PathBuf::from("worker"),
            permissions: vec![Permission::TrafficRead, Permission::DraftWrite],
            contributions: vec!["suite_panel".into(), "message_response_editor".into()],
        },
        grants: vec![Permission::TrafficRead, Permission::DraftWrite],
        tab_id: None,
    };
    let snapshot = Arc::new(PanelSnapshot {
        revision: 1,
        title: "lab.protocol".into(),
        root: Some(Node::new(
            1,
            "状态",
            node::Kind::Text(mio_core::panel::Text {
                text: "就绪".into(),
                tone: 0,
            }),
        )),
    });
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.install_plugin(info.clone(), snapshot, None, window, cx);
        });
        window.render_frame(cx);
    })
    .unwrap();
    info
}

#[gpui_kit::test]
fn intercept_response_stage_plugin_editor_commits_and_writes_back_response(
    cx: &mut TestAppContext,
) {
    let initial = tab("1");
    let mut fixture = fixture(cx, vec![initial], 64);
    let suite = install_resp_suite(&fixture, cx);
    drain(&mut fixture.commands);

    let intercept_resp = HttpResponse {
        version: HttpVersion::Http11,
        status: 200,
        reason_phrase: None,
        headers: vec![Header {
            name: "Content-Type".into(),
            value: b"text/plain".to_vec(),
        }],
        body: b"orig-resp".to_vec(),
        trailers: vec![],
    };
    let mut rec = record("http://127.0.0.1:9001/resp", "");
    let flow_id = rec.summary.id;
    rec.summary.status = Some(200);
    rec.response = Some(intercept_resp.clone());

    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            let mut ed = MessageEditor::new(window, cx);
            ed.load(&response_to_raw(&intercept_resp), window, cx);
            view.intercepts.push(PendingIntercept {
                target: InterceptTarget::Http {
                    stage: InterceptStage::Response,
                    response_head_only: false,
                    record: Box::new(rec),
                },
                editor: ed,
                deciding: false,
            });
            view.selected_intercept = Some(flow_id);
            view.tool = Tool::Proxy;
            view.proxy_page = ProxyPage::Intercept;
        });
        window.render_frame(cx);
        let btn = SharedString::from(format!(
            "intercept-plugin-response-tab-{}",
            suite.identity.instance_id
        ));
        window.click(btn, cx);
    })
    .unwrap();

    let (ic_open_op, ic_ctx_id, ic_rev) = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::Plugin {
                operation_id,
                action:
                    PluginAction::OpenResponseViewer {
                        read_only: false,
                        context_id,
                        revision,
                        ..
                    },
                ..
            } => Some((operation_id, context_id, revision)),
            _ => None,
        })
        .expect("intercept response stage must dispatch writable OpenResponseViewer");

    let ic_viewer_info = LoadedPlugin {
        identity: ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: fixture.project,
        },
        directory: suite.directory,
        manifest: suite.manifest,
        grants: suite.grants,
        tab_id: None,
    };
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: ic_open_op,
            result: Ok(PluginOutcome::ResponseOpened {
                info: ic_viewer_info.clone(),
                context_id: ic_ctx_id,
                revision: ic_rev,
                panel: panel(b"edited-by-plugin".to_vec()),
            }),
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("intercept-plugin-response-apply", cx);
    })
    .unwrap();
    let commit_op = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::Plugin {
                operation_id,
                action: PluginAction::CommitResponseEditor { instance_id, .. },
                ..
            } if instance_id == ic_viewer_info.identity.instance_id => Some(operation_id),
            _ => None,
        })
        .expect("clicking intercept-plugin-response-apply must dispatch CommitResponseEditor");

    let mut updated_resp = intercept_resp;
    updated_resp.body = b"edited-by-plugin".to_vec();
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: commit_op,
            result: Ok(PluginOutcome::ResponseCommitted {
                identity: ic_viewer_info.identity,
                context_id: ic_ctx_id,
                revision: ic_rev,
                response: updated_resp,
            }),
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            let bytes = view.intercepts[0].editor.bytes(window, cx).unwrap();
            assert!(String::from_utf8_lossy(&bytes).ends_with("edited-by-plugin"));
        });
    })
    .unwrap();
}

#[gpui_kit::test]
fn context_menu_replace_request_and_ext_imported_history_plugin_tabs_work(
    cx: &mut TestAppContext,
) {
    use mio_core::{ExtHistoryRow, ExtItemSummary, ExtMessageSummary, HistoryRow, HttpVersion};

    let initial = tab("1");
    let mut fixture = fixture(cx, vec![initial.clone()], 64);
    let suite = LoadedPlugin {
        identity: ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: fixture.project,
        },
        directory: PathBuf::from("/tmp/mio-ctx-replace-suite"),
        manifest: PluginManifest {
            id: "lab.protocol".into(),
            version: "1.3.0".into(),
            api_version: 1,
            engine: "native".into(),
            entry: PathBuf::from("worker"),
            permissions: vec![Permission::TrafficRead, Permission::DraftWrite],
            contributions: vec![
                "message_editor".into(),
                "message_response_editor".into(),
                "context_menu".into(),
            ],
        },
        grants: vec![Permission::TrafficRead, Permission::DraftWrite],
        tab_id: None,
    };
    let snap = Arc::new(PanelSnapshot {
        revision: 1,
        title: "lab.protocol".into(),
        root: Some(Node::new(
            1,
            "status",
            node::Kind::Text(mio_core::panel::Text {
                text: "ok".into(),
                tone: 0,
            }),
        )),
    });
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.install_plugin(suite.clone(), snap, None, window, cx);
            view.tool = Tool::Repeater;
            view.invoke_plugin_context_menu(suite.identity.instance_id, window, cx);
        });
    })
    .unwrap();
    let menu_op = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::Plugin {
                operation_id,
                action: PluginAction::InvokeContextMenu { .. },
                ..
            } => Some(operation_id),
            _ => None,
        })
        .expect("InvokeContextMenu dispatched");
    let replaced_bytes =
        b"POST /in-place HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 4\r\n\r\ndone".to_vec();
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: menu_op,
            result: Ok(PluginOutcome::ContextMenuInvoked {
                identity: suite.identity,
                status_message: "已原地替换 Repeater 请求".into(),
                log_line: "replaced".into(),
                open_repeater: None,
                replace_request: Some(replaced_bytes.clone()),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            assert_eq!(view.workspace.tabs.len(), 1);
            assert_eq!(view.workspace.tabs[0].draft.raw, replaced_bytes);
            assert_eq!(
                view.tab_editors[&initial.id]
                    .request
                    .bytes(window, cx)
                    .unwrap(),
                replaced_bytes
            );
        });
    })
    .unwrap();
    let _ = drain(&mut fixture.commands);

    let src_id = Uuid::new_v4();
    let item_id = Uuid::new_v4();
    let req_bytes = b"GET /archive-path HTTP/1.1\r\nHost: 127.0.0.1:8080\r\n\r\n";
    let archive_row = HistoryRow::ExtProxy(Box::new(ExtHistoryRow {
        notes_revision: 0,
        notes_preview: String::new(),
        notes_error: None,
        highlight: None,
        highlight_error: None,
        time_unix_ms: None,
        time_error: None,
        item: ExtItemSummary {
            id: item_id,
            source_id: src_id,
            collection: "root.f1.f0".into(),
            index: 0,
            object_address: None,
            state: "mapped".into(),
            message_count: 1,
        },
        request: Some(ExtMessageSummary {
            id: Uuid::new_v4(),
            source_id: src_id,
            item_id,
            collection: "root.f1.f0".into(),
            item_index: 0,
            role: "request".into(),
            field_id: 15,
            raw_offset: 0,
            raw_bytes: req_bytes.len() as u64,
            body_offset: None,
            declared_version: Some(HttpVersion::Http11),
            method: Some("GET".into()),
            request_target: Some("/archive-path".into()),
            status: None,
            diagnostic: None,
        }),
        edited_request: None,
        response: None,
        edited_response: None,
        origin: Some("http://127.0.0.1:8080".into()),
    }));
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.tool = Tool::Proxy;
            view.proxy_page = ProxyPage::History;
            view.selected_history = Some(archive_row.id());
            view.refresh_imported_history(&archive_row, window, cx);
        });
    })
    .unwrap();
    let read_cmd = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|c| match c {
            Command::ReadExtMessage {
                selection_revision,
                message_id,
                ..
            } => Some((selection_revision, message_id)),
            _ => None,
        })
        .unwrap();
    let HistoryRow::ExtProxy(b_row) = &archive_row else {
        unreachable!()
    };
    fixture
        .events
        .try_send(Event::ExtMessage {
            project: fixture.project,
            selection_revision: read_cmd.0,
            message_id: read_cmd.1,
            offset: 0,
            result: Ok(mio_core::ExtMessageFragment {
                summary: b_row.request.clone().unwrap(),
                offset: 0,
                bytes: req_bytes.to_vec(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let add_btn = SharedString::from(format!(
            "imported-history-plugin-add-editor-{}",
            suite.identity.instance_id
        ));
        window.click(add_btn, cx);
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).into_iter().any(|cmd| {
        matches!(
            cmd,
            Command::Plugin {
                action: PluginAction::OpenContextViewer { request, .. },
                ..
            } if request.url == "http://127.0.0.1:8080/archive-path"
        )
    }));
}
