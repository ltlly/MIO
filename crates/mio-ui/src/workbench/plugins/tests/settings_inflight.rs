use super::*;

fn settings_abandonment_before_commit_reply(
    cx: &mut TestAppContext,
    save_as: bool,
    cancel_picker: bool,
) {
    use crate::workbench::tests::settings;

    let tab = tab("等待 Commit 回包时保存失败");
    let mut fixture = fixture(cx, vec![tab.clone()], 128);
    let info = install_editor(&fixture, &tab, cx);
    drain(&mut fixture.commands);
    let source = std::env::temp_dir().join(format!("mio-commit-wait-{}.mio", Uuid::new_v4()));
    let destination = source.with_extension("copy.mio");
    settings::acknowledge_project_saved(&fixture, source.clone(), cx);
    let latest = raw("失败后仍保留的插件编辑🙂");
    input_plugin(&fixture, &latest, cx);
    let settings_window = settings::open_settings(&fixture, cx);
    settings::toggle(settings_window, false, cx);
    let old = settings::saved(&mut fixture.commands);
    settings::toggle(settings_window, true, cx);
    let (operation, draft, editor) = send_click(&mut fixture, cx);
    assert_eq!(editor.contents, latest.as_bytes());
    assert_eq!(draft, tab.draft);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let Some(Operation::Commit {
            instance,
            tab_id,
            send,
            editor: captured,
        }) = view.extensions.operations.get(&operation)
        else {
            panic!("the real Send click must leave its Commit operation awaiting a reply")
        };
        assert_eq!((*instance, *tab_id), (info.identity.instance_id, tab.id));
        assert_eq!(send.draft, draft);
        assert_eq!(captured, &editor);
        assert!(send.settings.unpack_compressed_responses);
        assert_eq!(send.required_workspace_revision, old.revision + 1);
        assert_eq!(view.pending_workspace, Some(old.revision));
        assert!(view.workspace_dirty && view.plugin_waiting(tab.id));
        assert!(view.pending_sends.is_empty() && view.extensions.saved_commits.is_empty());
        assert!(view.sending.is_empty() && view.error.is_none());
    });

    let owner = fixture.view.clone();
    let owner_window = fixture.window;
    let assert_editor_preserved = |cx: &mut TestAppContext| {
        cx.update_window(owner_window, |_, window, cx| {
            let view = owner.read(cx);
            assert_eq!(view.workspace.tabs[0].draft, tab.draft);
            let native = &view.tab_editors[&tab.id];
            assert_eq!(native.request.bytes_lossless(cx), tab.draft.raw);
            assert_eq!(native.target.read(cx).value().as_ref(), tab.draft.target);
            let instance = view.extensions.instance(info.identity.instance_id).unwrap();
            assert!(instance.live && instance.panel.read(cx).has_unsaved_edits(cx));
            assert_eq!(instance.bound.as_ref(), Some(&tab.draft));
            let panel = instance.panel.clone();
            assert_eq!(
                panel
                    .update(cx, |panel, cx| panel.capture_editor(401, window, cx))
                    .unwrap(),
                editor
            );
        })
        .unwrap();
    };
    assert_editor_preserved(cx);

    if save_as {
        settings::menu_action(settings_window, "项目", "另存为…", cx);
        assert!(cx.did_prompt_for_new_path());
        acknowledge(&fixture, old.clone(), cx);
        assert!(drain(&mut fixture.commands).is_empty());
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert!(view.choosing_save_as && view.pending_workspace.is_none());
            assert!(view.workspace_dirty && view.plugin_waiting(tab.id));
            assert!(view.pending_sends.is_empty() && view.extensions.saved_commits.is_empty());
            assert!(view.transition.is_none() && view.after_save.is_none());
        });
    }
    let filled = if cancel_picker {
        0
    } else {
        settings::fill_command_queue(&fixture, cx)
    };
    if save_as {
        cx.simulate_new_path_selection(|_| (!cancel_picker).then(|| destination.clone()));
        cx.run_until_parked();
    } else {
        acknowledge(&fixture, old.clone(), cx);
    }
    assert!(!cx.did_prompt_for_new_path());
    settings::checked(settings_window, true, cx);
    if !cancel_picker {
        settings::visible_error(settings_window, "命令队列已满", cx);
    }
    let (retained, commit_cancelled, error) = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.revision, old.revision);
        assert_eq!(view.project_path.as_ref(), Some(&source));
        assert_ne!(view.project_path.as_ref(), Some(&destination));
        assert!(view.workspace.repeater_settings.unpack_compressed_responses);
        assert!(view.workspace_dirty && view.pending_workspace.is_none());
        assert!(!view.choosing_save_as && !view.saving_project_snapshot);
        assert!(view.pending_project_save.is_none());
        assert_eq!(view.project_saves_in_flight, 0);
        assert!(view.transition.is_none() && view.after_save.is_none());
        assert!(view.pending_sends.is_empty() && view.extensions.saved_commits.is_empty());
        assert!(view.sending.is_empty());
        if cancel_picker {
            assert!(view.error.is_none());
        }
        (
            (*view.workspace).clone(),
            !view.extensions.operations.contains_key(&operation) && !view.plugin_waiting(tab.id),
            view.error.clone(),
        )
    });
    assert_editor_preserved(cx);

    let project_key = fixture.project;
    let is_cancel = |command: &Command| {
        matches!(command,
            Command::Plugin {
                project,
                action: PluginAction::Cancel { operation_id },
                ..
            } if *project == project_key && *operation_id == operation)
    };
    let is_filler = |command: &Command| {
        matches!(command,
            Command::QueryHttpHistory { query: mio_core::HttpHistoryQuery { project, query_revision: u64::MAX, .. } }
                if *project == project_key)
    };
    let mut commands = drain(&mut fixture.commands);
    // Cancellation control waits for capacity when the data channel is full.
    cx.run_until_parked();
    commands.extend(drain(&mut fixture.commands));
    assert_eq!(
        commands.iter().filter(|command| is_filler(command)).count(),
        filled
    );
    assert_eq!(
        commands.iter().filter(|command| is_cancel(command)).count(),
        1,
        "the abandoned Commit must receive one Cancel after queue capacity returns"
    );
    assert!(
        commands
            .iter()
            .all(|command| is_filler(command) || is_cancel(command)),
        "failed or cancelled Settings save may only cancel its Commit, never save, Send, or Rebind"
    );
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: operation,
            result: Ok(PluginOutcome::Committed {
                identity: info.identity,
                tab_id: tab.id,
                draft_revision: draft.revision,
                request: parse_request(&editor.contents, &draft.target, draft.version).unwrap(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).iter().all(is_cancel),
        "a late Commit reply after failed or cancelled Settings save must not create SaveWorkspace, Send, or Rebind"
    );
    assert!(
        commit_cancelled,
        "Settings save failure or cancellation must release the in-flight Commit immediately, before its reply or queue capacity returns"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(*view.workspace, retained);
        assert_eq!(view.error, error);
        assert!(view.workspace_dirty && view.pending_workspace.is_none());
        assert!(view.pending_sends.is_empty() && view.extensions.saved_commits.is_empty());
        assert!(view.sending.is_empty() && !view.plugin_waiting(tab.id));
    });
    assert_editor_preserved(cx);
    if !cancel_picker {
        settings::visible_error(settings_window, "命令队列已满", cx);
    }

    settings::menu_action(settings_window, "项目", "保存项目", cx);
    assert!(!cx.did_prompt_for_new_path());
    let commands: Vec<_> = drain(&mut fixture.commands)
        .into_iter()
        .filter(|command| !is_cancel(command))
        .collect();
    let [
        Command::SaveWorkspace {
            project,
            workspace: retry,
        },
    ] = commands.as_slice()
    else {
        panic!("explicit Save must only retry the retained workspace, never the cancelled Commit")
    };
    assert_eq!(*project, fixture.project);
    let mut expected = retained;
    expected.revision = old.revision + 1;
    assert_eq!(retry, &expected);
    acknowledge(&fixture, retry.clone(), cx);
    let commands: Vec<_> = drain(&mut fixture.commands)
        .into_iter()
        .filter(|command| !is_cancel(command))
        .collect();
    // The cancelled Commit never created live history; acknowledge only its empty page.
    let commands = crate::workbench::tests::acknowledge_history_query(&fixture, commands, cx);
    assert!(
        matches!(commands.as_slice(),
            [Command::SaveProject { websocket_notes: _, ext_http_notes: _, ext_http_highlights: _, project, path: None, workspace: None }]
                if *project == fixture.project),
        "a later explicit Save ACK must not revive the abandoned Send or Rebind"
    );
    settings::acknowledge_project_saved(&fixture, source.clone(), cx);
    assert!(drain(&mut fixture.commands).iter().all(is_cancel));
    settings::checked(settings_window, true, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        let view = fixture.view.read(cx);
        assert_eq!(*view.workspace, expected);
        assert_eq!(view.project_path.as_ref(), Some(&source));
        assert!(!view.workspace_dirty && view.pending_workspace.is_none());
        assert!(view.pending_project_save.is_none() && !view.saving_project_snapshot);
        assert_eq!(view.project_saves_in_flight, 0);
        assert!(view.pending_sends.is_empty() && view.extensions.saved_commits.is_empty());
        assert!(view.sending.is_empty() && !view.plugin_waiting(tab.id));
        assert!(!view.extensions.operations.contains_key(&operation));
        assert!(view.error.is_none());
        window.render_frame(cx);
        assert_eq!(window.find("repeater-send").disabled(), None);
        assert!(window.try_find("repeater-cancel").is_none());
    })
    .unwrap();
    assert_editor_preserved(cx);
}

#[gpui_kit::test]
fn settings_coalesced_workspace_enqueue_failure_cancels_inflight_commit(cx: &mut TestAppContext) {
    settings_abandonment_before_commit_reply(cx, false, false);
}

#[gpui_kit::test]
fn settings_save_as_enqueue_failure_cancels_inflight_commit(cx: &mut TestAppContext) {
    settings_abandonment_before_commit_reply(cx, true, false);
}

#[gpui_kit::test]
fn settings_save_as_cancel_cancels_inflight_commit(cx: &mut TestAppContext) {
    settings_abandonment_before_commit_reply(cx, true, true);
}
