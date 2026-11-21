use super::*;

pub(super) struct PickerErrorRebindFixture {
    pub(super) fixture: Fixture,
    pub(super) a: WorkTab,
    pub(super) b: WorkTab,
    pub(super) source: PathBuf,
    pub(super) copy: PathBuf,
    pub(super) child_identity: mio_core::protocol::ChannelIdentity,
    pub(super) committed: Workspace,
    pub(super) rebind: Uuid,
    pub(super) cancel: tokio_util::sync::CancellationToken,
    pub(super) native_a: Entity<gpui_kit::component::input::EditorState>,
}

pub(super) fn setup_picker_error_rebind_fixture(
    cx: &mut TestAppContext,
) -> PickerErrorRebindFixture {
    use mio_core::{
        LoadedPlugin, PluginAction, PluginInspection, PluginOutcome,
        panel::{Editor as PluginEditor, EditorFormat, Node, PanelSnapshot, node},
        plugins::PluginManifest,
        protocol::{ChannelIdentity, Permission},
    };

    let a = tab("A：保留同标签 Rebind");
    let b = tab("B：picker 错误前等待保存的 native Send");
    let mut fixture = fixture(cx, vec![a.clone(), b.clone()], 64);
    drain(&mut fixture.commands);
    let source = std::env::temp_dir().join(format!("mio-settings-picker-{}.mio", Uuid::new_v4()));
    let copy = source.with_extension("copy.mio");
    menu_action(fixture.window, "项目", "保存项目", cx);
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| Some(source.clone()));
    cx.run_until_parked();
    assert!(matches!(drain(&mut fixture.commands).as_slice(),
        [Command::SaveProject { websocket_notes: _, ext_http_notes: _, ext_http_highlights: _, project, path: Some(path), workspace: None }]
            if *project == fixture.project && path == &source));
    acknowledge_project_saved(&fixture, source.clone(), cx);
    assert!(drain(&mut fixture.commands).is_empty());

    let project = fixture.project;
    let events = fixture.events.clone();
    let next_plugin = |commands: &mut mpsc::Receiver<Command>| {
        let mut commands = drain(commands);
        assert_eq!(commands.len(), 1);
        let Command::Plugin {
            project: owner,
            operation_id,
            action,
        } = commands.pop().unwrap()
        else {
            panic!("the fixture action must enter the shipped plugin handler")
        };
        assert_eq!(owner, project);
        (operation_id, action)
    };
    let reply = |operation_id, outcome, cx: &mut TestAppContext| {
        events
            .try_send(Event::PluginOperation {
                project,
                operation_id,
                result: Ok(outcome),
            })
            .unwrap();
        cx.run_until_parked();
    };
    let factory = LoadedPlugin {
        identity: ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project,
        },
        directory: std::env::temp_dir().join("mio-settings-picker-plugin-fixture"),
        manifest: PluginManifest {
            id: "test.settings-picker".into(),
            version: "1.0".into(),
            api_version: 1,
            engine: "native".into(),
            entry: "worker".into(),
            permissions: vec![Permission::TrafficRead, Permission::DraftWrite],
            contributions: vec!["message_editor".into()],
        },
        grants: vec![Permission::TrafficRead, Permission::DraftWrite],
        tab_id: None,
    };
    let panel = std::sync::Arc::new(PanelSnapshot {
        revision: 1,
        title: "Picker Rebind fixture".into(),
        root: Some(Node::new(
            101,
            "请求",
            node::Kind::Editor(PluginEditor {
                document_id: 401,
                revision: 1,
                contents: a.draft.raw.clone(),
                format: EditorFormat::Utf8 as i32,
                read_only: false,
            }),
        )),
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-extensions", cx);
        window.click("plugin-choose", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|_| Some(vec![factory.directory.clone()]));
    cx.run_until_parked();
    let (operation, action) = next_plugin(&mut fixture.commands);
    assert!(
        matches!(action, PluginAction::Inspect { directory } if directory == factory.directory)
    );
    let inspection = Uuid::new_v4();
    reply(
        operation,
        PluginOutcome::Inspected(PluginInspection {
            id: inspection,
            directory: factory.directory.clone(),
            executable: factory.directory.join("worker"),
            manifest: factory.manifest.clone(),
        }),
        cx,
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("plugin-native-trust", cx);
        for grant in &factory.grants {
            window.click(("plugin-grant", *grant as usize), cx);
        }
        window.click("plugin-load", cx);
    })
    .unwrap();
    let (operation, action) = next_plugin(&mut fixture.commands);
    assert!(matches!(action,
        PluginAction::Load { inspection_id, native_trusted: true, grants, replace_instance: None }
            if inspection_id == inspection && grants.len() == factory.grants.len()
                && factory.grants.iter().all(|grant| grants.contains(grant))));
    reply(
        operation,
        PluginOutcome::Loaded {
            info: factory.clone(),
            panel: panel.clone(),
        },
        cx,
    );
    activate_empty_repeater(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(
            SharedString::from(format!(
                "plugin-add-editor-{}",
                factory.identity.instance_id
            )),
            cx,
        );
    })
    .unwrap();
    let (operation, action) = next_plugin(&mut fixture.commands);
    assert!(matches!(action,
        PluginAction::CreateEditor { source_instance, tab_id, draft_revision }
            if source_instance == factory.identity.instance_id
                && tab_id == a.id && draft_revision == a.draft.revision));
    let child = LoadedPlugin {
        identity: ChannelIdentity {
            instance_id: Uuid::new_v4(),
            project,
        },
        tab_id: Some(a.id),
        ..factory
    };
    reply(
        operation,
        PluginOutcome::Loaded {
            info: child.clone(),
            panel,
        },
        cx,
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-send", cx)
    })
    .unwrap();
    let (operation, action) = next_plugin(&mut fixture.commands);
    let PluginAction::CommitEditor {
        instance_id,
        tab_id,
        draft_revision,
        editor,
        ..
    } = action
    else {
        panic!("A's Send must capture a real plugin Commit")
    };
    assert_eq!(
        (instance_id, tab_id, draft_revision),
        (child.identity.instance_id, a.id, a.draft.revision)
    );
    assert_eq!(editor.contents, a.draft.raw);
    reply(
        operation,
        PluginOutcome::Committed {
            identity: child.identity,
            tab_id: a.id,
            draft_revision,
            request: parse_request(&editor.contents, &a.draft.target, a.draft.version).unwrap(),
        },
        cx,
    );
    let committed = saved(&mut fixture.commands);
    acknowledge(&fixture, committed.clone(), cx);
    let admitted = drain(&mut fixture.commands);
    let [
        Command::Plugin {
            project: plugin_project,
            operation_id: rebind,
            action:
                PluginAction::OpenRequest {
                    instance_id,
                    tab_id: rebind_tab,
                    draft_revision,
                },
        },
        Command::Send {
            project: send_project,
            tab_id: sent_tab,
            run_id,
            draft,
            cancel,
            ..
        },
    ] = admitted.as_slice()
    else {
        panic!("the acknowledged plugin Commit must produce its Rebind and exactly one Send")
    };
    assert_eq!((*plugin_project, *send_project), (project, project));
    assert_eq!(
        (*instance_id, *rebind_tab, *draft_revision),
        (
            child.identity.instance_id,
            a.id,
            committed.tabs[0].draft.revision
        )
    );
    assert_eq!((*sent_tab, draft), (a.id, &committed.tabs[0].draft));
    fixture
        .events
        .try_send(Event::SendRejected {
            project,
            tab_id: a.id,
            run_id: *run_id,
            draft_revision: draft.revision,
            message: "夹具结束 A 的发送；Rebind 仍等待原始确认".into(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    let native_a = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.sending.is_empty() && view.pending_sends.is_empty());
        assert!(view.plugin_waiting(a.id));
        view.tab_editors[&a.id].request.raw.clone()
    });
    let rebind = *rebind;
    let cancel = cancel.clone();
    PickerErrorRebindFixture {
        fixture,
        a,
        b,
        source,
        copy,
        child_identity: child.identity,
        committed,
        rebind,
        cancel,
        native_a,
    }
}
