use super::*;

pub(super) fn acknowledge_linked_history(
    fixture: &Fixture,
    tab: TabId,
    terminal: &mio_core::FlowSummary,
    commands: Vec<Command>,
    cx: &mut TestAppContext,
) {
    let [
        Command::QueryRepeaterHistory {
            project: history_project,
            query_revision,
            tab_id,
            start,
            limit,
        },
    ] = commands.as_slice()
    else {
        panic!("the saved linkage ACK must release exactly that tab's completed history query");
    };
    assert_eq!((*history_project, *tab_id), (fixture.project, tab));
    assert_eq!((*start, *limit), (0, 64));
    assert_eq!(terminal.status, Some(200));
    assert!(matches!(terminal.state, mio_core::FlowState::Complete));
    // These two fixtures have one completed live send; neither may use the empty-history reply.
    fixture
        .events
        .try_send(Event::RepeaterHistoryPage {
            project: *history_project,
            query_revision: *query_revision,
            tab_id: *tab_id,
            result: Ok(mio_core::RepeaterHistoryPage {
                total: 1,
                start: 0,
                current_position: Some(0),
                rows: vec![mio_core::RepeaterHistoryEntry {
                    id: mio_core::RepeaterHistoryEntryId::Live(terminal.id),
                    position: 0,
                    method: Some(terminal.method.clone()),
                    target: Some(terminal.url.clone()),
                    status: terminal.status,
                    diagnostic: None,
                }],
                next_offset: None,
            }),
        })
        .unwrap();
    cx.run_until_parked();
}

pub(super) fn manifest() -> PluginManifest {
    PluginManifest {
        id: "test.native-editor".into(),
        version: "1.0".into(),
        api_version: 1,
        engine: "native".into(),
        entry: PathBuf::from("worker"),
        permissions: vec![Permission::TrafficRead, Permission::DraftWrite],
        contributions: vec!["message_editor".into(), "tool_panel".into()],
    }
}

pub(super) fn panel(bytes: Vec<u8>) -> Arc<PanelSnapshot> {
    Arc::new(PanelSnapshot {
        revision: 1,
        title: "Generic test editor".into(),
        root: Some(Node::new(
            101,
            "发送消息编辑器",
            node::Kind::Editor(Editor {
                document_id: 401,
                revision: 1,
                contents: bytes,
                format: EditorFormat::Utf8 as i32,
                read_only: false,
            }),
        )),
    })
}

pub(super) fn install_editor(
    fixture: &Fixture,
    tab: &WorkTab,
    cx: &mut TestAppContext,
) -> LoadedPlugin {
    let info = LoadedPlugin {
        identity: ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project: fixture.project,
        },
        directory: PathBuf::from("/tmp/mio-ui-generic-fixture"),
        manifest: manifest(),
        grants: vec![Permission::TrafficRead, Permission::DraftWrite],
        tab_id: Some(tab.id),
    };
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.install_plugin(
                info.clone(),
                panel(tab.draft.raw.clone()),
                Some(tab.draft.clone()),
                window,
                cx,
            );
            view.extensions
                .active_messages
                .insert(tab.id, info.identity.instance_id);
            view.tool = Tool::Repeater;
        });
        window.render_frame(cx);
    })
    .unwrap();
    info
}

pub(super) fn raw(body: &str) -> String {
    format!(
        "POST /plugin HTTP/1.1\r\nHost: 127.0.0.1:9001\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

pub(super) fn input_plugin(fixture: &Fixture, text: &str, cx: &mut TestAppContext) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("plugin-node", 101u64), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(text, cx);
    })
    .unwrap();
}

pub(super) fn send_click(
    fixture: &mut Fixture,
    cx: &mut TestAppContext,
) -> (Uuid, Draft, EditorSnapshot) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-send", cx)
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, Command::Send { .. })),
        "Commit validation and durable ACK must precede Send"
    );
    commands
        .into_iter()
        .find_map(|command| match command {
            Command::Plugin {
                operation_id,
                action: PluginAction::CommitEditor { tab_id, editor, .. },
                ..
            } => {
                let draft = cx.read(|cx| {
                    fixture
                        .view
                        .read(cx)
                        .workspace
                        .tabs
                        .iter()
                        .find(|tab| tab.id == tab_id)
                        .unwrap()
                        .draft
                        .clone()
                });
                Some((operation_id, draft, editor))
            }
            _ => None,
        })
        .expect("native Send click commits the custom editor")
}

pub(super) fn accept_commit(
    fixture: &mut Fixture,
    info: &LoadedPlugin,
    operation: Uuid,
    draft: &Draft,
    editor: &EditorSnapshot,
    cx: &mut TestAppContext,
) -> Workspace {
    let request = parse_request(&editor.contents, &draft.target, draft.version).unwrap();
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: operation,
            result: Ok(PluginOutcome::Committed {
                identity: info.identity,
                tab_id: info.tab_id.unwrap(),
                draft_revision: draft.revision,
                request,
            }),
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, Command::Send { .. }))
    );
    commands
        .into_iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .expect("validated custom message is saved")
}

pub(super) fn acknowledge(fixture: &Fixture, workspace: Workspace, cx: &mut TestAppContext) {
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace,
        })
        .unwrap();
    cx.run_until_parked();
}
