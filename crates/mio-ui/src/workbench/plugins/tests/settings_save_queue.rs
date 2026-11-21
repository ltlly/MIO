use super::*;

#[gpui_kit::test]
fn settings_save_as_success_preserves_plugin_commit_and_sends_once(cx: &mut TestAppContext) {
    use crate::workbench::tests::settings;

    let tab = tab("另存为成功的插件请求");
    let mut fixture = fixture(cx, vec![tab.clone()], 128);
    let info = install_editor(&fixture, &tab, cx);
    drain(&mut fixture.commands);
    let latest = raw("另存为期间保留插件草稿🙂");
    input_plugin(&fixture, &latest, cx);
    let settings_window = settings::open_settings(&fixture, cx);
    settings::toggle(settings_window, false, cx);
    let old = settings::saved(&mut fixture.commands);
    settings::toggle(settings_window, true, cx);
    let (operation, draft, editor) = send_click(&mut fixture, cx);
    assert_eq!(editor.contents, latest.as_bytes());
    let request = parse_request(&editor.contents, &draft.target, draft.version).unwrap();
    let committed = Draft {
        revision: draft.revision + 1,
        target: request.url.clone(),
        raw: request_to_raw(&request),
        version: request.version,
    };
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: operation,
            result: Ok(PluginOutcome::Committed {
                identity: info.identity,
                tab_id: tab.id,
                draft_revision: draft.revision,
                request,
            }),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    settings::save_as_ack_while_choosing(&mut fixture, settings_window, old.clone(), cx);
    settings::toggle(settings_window, false, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    let copy = std::env::temp_dir().join(format!("mio-settings-plugin-{}.mio", Uuid::new_v4()));
    cx.simulate_new_path_selection(|_| Some(copy.clone()));
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveProject {
            websocket_notes: _,
            ext_http_notes: _,
            ext_http_highlights: _,
            project,
            path: Some(path),
            workspace: Some(snapshot),
        },
    ] = commands.as_slice()
    else {
        panic!("Save As must carry the committed draft while Send and Rebind still await its ACK")
    };
    assert_eq!((*project, path), (fixture.project, &copy));
    assert_eq!(snapshot.revision, old.revision + 1);
    assert_eq!(snapshot.tabs[0].draft, committed);
    assert!(!snapshot.repeater_settings.unpack_compressed_responses);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let guard = &view.extensions.saved_commits[&tab.id];
        assert_eq!(guard.instance, info.identity.instance_id);
        assert_eq!(guard.draft, committed);
        assert_eq!(guard.editor, editor);
        assert!(view.plugin_waiting(tab.id));
        assert_eq!(view.pending_sends.len(), 1);
        assert!(
            view.pending_sends[&tab.id]
                .settings
                .unpack_compressed_responses
        );
        assert_eq!(
            view.pending_sends[&tab.id].required_workspace_revision,
            snapshot.revision
        );
        assert_eq!(view.pending_workspace, Some(snapshot.revision));
        assert!(view.sending.is_empty());
        let instance = view.extensions.instance(info.identity.instance_id).unwrap();
        assert_eq!(instance.bound.as_ref(), Some(&tab.draft));
        assert!(instance.panel.read(cx).has_unsaved_edits(cx));
    });
    acknowledge(&fixture, snapshot.clone(), cx);
    let released = drain(&mut fixture.commands);
    // The destination contains only the unsent draft before this Send is admitted.
    let released = crate::workbench::tests::acknowledge_history_query(&fixture, released, cx);
    let [
        Command::Plugin {
            project: plugin_project,
            operation_id: rebind,
            action:
                PluginAction::OpenRequest {
                    instance_id,
                    tab_id: rebound_tab,
                    draft_revision,
                },
        },
        Command::Send {
            project,
            tab_id,
            run_id,
            draft,
            settings: sent_settings,
            group_send: _,
            cancel,
        },
    ] = released.as_slice()
    else {
        panic!("the destination workspace ACK must rebind and send the captured request once")
    };
    assert_eq!(
        (*plugin_project, *project),
        (fixture.project, fixture.project)
    );
    assert_eq!(
        (*instance_id, *rebound_tab, *draft_revision),
        (info.identity.instance_id, tab.id, committed.revision)
    );
    assert_eq!((*tab_id, draft), (tab.id, &committed));
    assert!(sent_settings.unpack_compressed_responses);
    assert!(!cancel.is_cancelled());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.extensions.saved_commits.is_empty() && view.pending_sends.is_empty());
        assert!(matches!(view.extensions.operations.get(rebind),
            Some(Operation::Rebind { instance, tab_id, draft })
                if *instance == info.identity.instance_id && *tab_id == tab.id && draft == &committed));
        let instance = view.extensions.instance(info.identity.instance_id).unwrap();
        assert!(!instance.panel.read(cx).has_unsaved_edits(cx));
    });
    settings::acknowledge_project_saved(&fixture, copy.clone(), cx);
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: *rebind,
            result: Ok(PluginOutcome::Opened {
                identity: info.identity,
                tab_id: tab.id,
                draft_revision: committed.revision,
            }),
        })
        .unwrap();
    cx.run_until_parked();
    acknowledge(&fixture, old, cx);
    acknowledge(&fixture, snapshot.clone(), cx);
    assert!(drain(&mut fixture.commands).is_empty());
    settings::checked(settings_window, false, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(&*view.workspace, snapshot);
        assert_eq!(view.project_path.as_ref(), Some(&copy));
        assert!(!view.workspace_dirty && view.pending_workspace.is_none());
        assert!(view.pending_project_save.is_none());
        assert_eq!(view.project_saves_in_flight, 0);
        assert!(view.extensions.saved_commits.is_empty() && view.pending_sends.is_empty());
        assert!(!view.extensions.operations.contains_key(rebind));
        assert!(!view.plugin_waiting(tab.id));
        let instance = view.extensions.instance(info.identity.instance_id).unwrap();
        assert!(instance.live);
        assert_eq!(instance.bound.as_ref(), Some(&committed));
        assert!(!instance.panel.read(cx).has_unsaved_edits(cx));
        assert_eq!(view.sending.len(), 1);
        assert_eq!(view.sending[&tab.id].run_id, *run_id);
        assert!(!cancel.is_cancelled());
    });
}

#[gpui_kit::test]
fn settings_coalesced_workspace_enqueue_failure_discards_plugin_commit(cx: &mut TestAppContext) {
    use crate::workbench::tests::settings;

    let tab = tab("队列满时保留插件草稿");
    let mut fixture = fixture(cx, vec![tab.clone()], 128);
    let info = install_editor(&fixture, &tab, cx);
    drain(&mut fixture.commands);
    let source = std::env::temp_dir().join(format!("mio-settings-queue-{}.mio", Uuid::new_v4()));
    settings::acknowledge_project_saved(&fixture, source.clone(), cx);
    let latest = raw("合并保存入队失败仍保留插件草稿🙂");
    input_plugin(&fixture, &latest, cx);
    let settings_window = settings::open_settings(&fixture, cx);
    settings::toggle(settings_window, false, cx);
    let old = settings::saved(&mut fixture.commands);
    settings::toggle(settings_window, true, cx);
    let (operation, draft, editor) = send_click(&mut fixture, cx);
    assert_eq!(editor.contents, latest.as_bytes());
    let request = parse_request(&editor.contents, &draft.target, draft.version).unwrap();
    let committed = Draft {
        revision: draft.revision + 1,
        target: request.url.clone(),
        raw: request_to_raw(&request),
        version: request.version,
    };
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: operation,
            result: Ok(PluginOutcome::Committed {
                identity: info.identity,
                tab_id: tab.id,
                draft_revision: draft.revision,
                request,
            }),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.pending_workspace, Some(old.revision));
        assert!(view.workspace_dirty && view.sending.is_empty());
        let send = &view.pending_sends[&tab.id];
        assert_eq!(send.draft, committed);
        assert!(send.settings.unpack_compressed_responses);
        assert_eq!(send.required_workspace_revision, old.revision + 1);
        let guard = &view.extensions.saved_commits[&tab.id];
        assert_eq!(guard.instance, info.identity.instance_id);
        assert_eq!(guard.draft, committed);
        assert_eq!(guard.editor, editor);
    });

    let filled = settings::fill_command_queue(&fixture, cx);
    acknowledge(&fixture, old.clone(), cx);
    settings::visible_error(settings_window, "命令队列已满", cx);
    settings::checked(settings_window, true, cx);
    let queued = drain(&mut fixture.commands);
    assert_eq!(queued.len(), filled);
    assert!(queued.iter().all(|command| matches!(
        command,
        Command::QueryHttpHistory {
            query: mio_core::HttpHistoryQuery {
                query_revision: u64::MAX,
                ..
            }
        }
    )));
    let (retained, commit_cancelled) = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.revision, old.revision);
        assert_eq!(view.workspace.tabs[0].draft, committed);
        assert_eq!(view.project_path.as_ref(), Some(&source));
        assert!(view.workspace.repeater_settings.unpack_compressed_responses);
        assert!(view.workspace_dirty && view.pending_workspace.is_none());
        assert!(view.pending_project_save.is_none() && !view.saving_project_snapshot);
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert!(view.transition.is_none() && view.after_save.is_none());
        assert_eq!(
            view.tab_editors[&tab.id].request.bytes_lossless(cx),
            committed.raw
        );
        let instance = view.extensions.instance(info.identity.instance_id).unwrap();
        assert!(instance.live && instance.panel.read(cx).has_unsaved_edits(cx));
        assert_eq!(instance.bound.as_ref(), Some(&tab.draft));
        (
            (*view.workspace).clone(),
            !view.extensions.saved_commits.contains_key(&tab.id),
        )
    });

    settings::menu_action(settings_window, "项目", "保存项目", cx);
    assert!(!cx.did_prompt_for_new_path());
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveWorkspace {
            project,
            workspace: retry,
        },
    ] = commands.as_slice()
    else {
        panic!("explicit Save must retry the retained workspace without Send or plugin operations")
    };
    assert_eq!(*project, fixture.project);
    let mut expected = retained;
    expected.revision = old.revision + 1;
    assert_eq!(retry, &expected);
    acknowledge(&fixture, retry.clone(), cx);
    let commands = drain(&mut fixture.commands);
    // Retrying the draft save does not register a sent history entry.
    let commands = crate::workbench::tests::acknowledge_history_query(&fixture, commands, cx);
    assert!(
        matches!(commands.as_slice(),
        [Command::SaveProject { websocket_notes: _, ext_http_notes: _, ext_http_highlights: _, project, path: None, workspace: None }]
            if *project == fixture.project),
        "the retry ACK must only save the project, never Send or rebind the failed Commit"
    );
    settings::acknowledge_project_saved(&fixture, source.clone(), cx);
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(
        commit_cancelled,
        "coalesced SaveWorkspace enqueue failure must discard its saved Commit guard immediately"
    );
    cx.update_window(fixture.window, |_, window, cx| {
        let view = fixture.view.read(cx);
        assert_eq!(*view.workspace, expected);
        assert_eq!(view.project_path.as_ref(), Some(&source));
        assert!(!view.workspace_dirty && view.pending_workspace.is_none());
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert!(!view.extensions.saved_commits.contains_key(&tab.id));
        assert!(!view.plugin_waiting(tab.id));
        assert!(view.error.is_none());
        let panel = view
            .extensions
            .instance(info.identity.instance_id)
            .unwrap()
            .panel
            .clone();
        assert!(panel.read(cx).has_unsaved_edits(cx));
        assert_eq!(
            panel
                .update(cx, |panel, cx| panel.capture_editor(401, window, cx))
                .unwrap(),
            editor
        );
        window.render_frame(cx);
        assert_eq!(window.find("repeater-send").disabled(), None);
        assert!(window.try_find("repeater-cancel").is_none());
        window.click("plugin-message-native", cx);
        assert_eq!(window.find("repeater-send").disabled(), None);
    })
    .unwrap();
    assert!(!drain(&mut fixture.commands).iter().any(|command| matches!(
        command,
        Command::Send { .. }
            | Command::SaveWorkspace { .. }
            | Command::SaveProject { .. }
            | Command::Plugin {
                action: PluginAction::CommitEditor { .. } | PluginAction::OpenRequest { .. },
                ..
            }
    )));
}
