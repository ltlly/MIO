use super::*;

#[gpui_kit::test]
fn settings_save_as_picker_error_clears_native_send_preserves_rebind_and_retries(
    cx: &mut TestAppContext,
) {
    let super::part_12::PickerErrorRebindFixture {
        mut fixture,
        a,
        b,
        source,
        copy,
        child_identity,
        committed,
        rebind,
        cancel,
        native_a,
    } = super::part_12::setup_picker_error_rebind_fixture(cx);
    let project = fixture.project;
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("plugin-message-native", cx);
        window.click(("input", native_a.entity_id()), cx);
        assert!(window.try_find("repeater-send").is_none());
        assert_eq!(window.find("repeater-cancel").disabled(), None);
        window.press("cmd-enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(SharedString::from(format!("work-tab-{}", b.id)), cx);
    })
    .unwrap();
    let native_b = cx.read(|cx| fixture.view.read(cx).tab_editors[&b.id].request.raw.clone());
    let old = saved(&mut fixture.commands);
    assert!(old.repeater_settings.unpack_compressed_responses);
    let settings = open_settings(&fixture, cx);
    toggle(settings, false, cx);
    let body = "选择保存位置失败后保留中文草稿🙂";
    let latest = format!(
        "POST /picker-error HTTP/1.1\r\nHost: 127.0.0.1:9001\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", native_b.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(&latest, cx);
        window.click("repeater-send", cx);
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.pending_workspace, Some(old.revision));
        assert!(
            view.workspace_dirty && !view.workspace.repeater_settings.unpack_compressed_responses
        );
        assert_eq!(view.pending_sends.len(), 1);
        let send = &view.pending_sends[&b.id];
        assert_eq!(send.required_workspace_revision, old.revision + 1);
        assert!(!send.settings.unpack_compressed_responses);
        assert_eq!(send.draft, view.workspace.tabs[1].draft);
        assert_eq!(send.draft.raw, latest.as_bytes());
        assert_eq!(send.draft.revision, b.draft.revision + 1);
        assert_eq!(
            (&send.draft.target, send.draft.version),
            (&b.draft.target, b.draft.version)
        );
        assert!(view.sending.is_empty());
        assert!(view.plugin_waiting(a.id) && !view.plugin_waiting(b.id));
    });

    let (picker, receiver) = tokio::sync::oneshot::channel();
    fixture.view.update(cx, |view, _| {
        assert!(view.save_picker_override.is_none());
        view.save_picker_override = Some(receiver);
    });
    menu_action(settings, "项目", "另存为…", cx);
    assert!(!cx.did_prompt_for_new_path());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.save_picker_override.is_none());
        assert!(view.choosing_save_as && view.file_picker.is_some());
        assert!(view.transition.is_none() && view.after_save.is_none());
    });
    acknowledge(&fixture, old.clone(), cx);
    assert!(drain(&mut fixture.commands).is_empty());
    let retained = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.choosing_save_as && view.workspace_dirty && view.pending_workspace.is_none());
        assert_eq!(view.workspace.revision, old.revision);
        assert!(!view.workspace.repeater_settings.unpack_compressed_responses);
        assert_eq!(view.pending_sends.len(), 1);
        assert_eq!(view.pending_sends[&b.id].draft.raw, latest.as_bytes());
        assert!(view.sending.is_empty() && view.plugin_waiting(a.id));
        (*view.workspace).clone()
    });
    picker
        .send(Err(anyhow::anyhow!("picker 返回错误🙂")))
        .unwrap();
    cx.run_until_parked();
    let error = "无法选择保存位置：picker 返回错误🙂";
    visible_error(settings, error, cx);
    checked(settings, false, cx);
    assert!(!cx.did_prompt_for_new_path());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert_eq!(*view.workspace, retained);
        assert_eq!(view.project_path.as_ref(), Some(&source));
        assert_eq!(view.error.as_deref(), Some(error));
        assert!(!view.choosing_save_as && view.file_picker.is_none());
        assert!(view.transition.is_none() && view.after_save.is_none());
        assert!(view.workspace_dirty && view.pending_workspace.is_none());
        assert!(!view.saving_project_snapshot && view.pending_project_save.is_none());
        assert_eq!(view.project_saves_in_flight, 0);
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert!(view.plugin_waiting(a.id) && !view.plugin_waiting(b.id));
        assert!(!cancel.is_cancelled());
        let _ = &native_a;
        assert_eq!(
            view.tab_editors[&a.id].request.bytes_lossless(cx),
            retained.tabs[0].draft.raw
        );
        assert_eq!(native_b.read(cx).value().as_bytes(), latest.as_bytes());
        assert_eq!(
            view.tab_editors[&b.id].target.read(cx).value().as_ref(),
            b.draft.target
        );
        assert_eq!(window.find("repeater-send").disabled(), None);
        assert!(window.try_find("repeater-cancel").is_none());
        let status = rendered_status(window);
        assert!(status.contains("未保存") && status.contains("尚未建立网络连接"));
    })
    .unwrap();

    fixture
        .events
        .try_send(Event::PluginOperation {
            project,
            operation_id: rebind,
            result: Ok(mio_core::PluginOutcome::Opened {
                identity: child_identity,
                tab_id: a.id,
                draft_revision: committed.tabs[0].draft.revision,
            }),
        })
        .unwrap();
    cx.run_until_parked();
    acknowledge(&fixture, old.clone(), cx);
    assert!(drain(&mut fixture.commands).is_empty());
    visible_error(settings, error, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.plugin_waiting(a.id));
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert_eq!(*view.workspace, retained);
        assert!(view.workspace_dirty && view.pending_workspace.is_none());
        assert_eq!(view.project_path.as_ref(), Some(&source));
        assert_eq!(view.error.as_deref(), Some(error));
    });

    menu_action(settings, "项目", "另存为…", cx);
    assert!(cx.did_prompt_for_new_path());
    assert!(drain(&mut fixture.commands).is_empty());
    visible_error(settings, error, cx);
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
        panic!(
            "ordinary Save As retry must persist the retained snapshot without reviving requests"
        )
    };
    assert_eq!((*project, path), (fixture.project, &copy));
    let mut expected = retained;
    expected.revision = old.revision + 1;
    assert_eq!(snapshot, &expected);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project_path.as_ref(), Some(&source));
        assert_eq!(view.pending_workspace, Some(snapshot.revision));
        assert!(view.saving_project_snapshot && !view.workspace_dirty);
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
    });
    acknowledge(&fixture, snapshot.clone(), cx);
    let commands = drain(&mut fixture.commands);
    // Save As persisted draft-only tabs, so the newly released history query is empty.
    assert!(acknowledge_history_query(&fixture, commands, cx).is_empty());
    acknowledge_project_saved(&fixture, copy.clone(), cx);
    acknowledge(&fixture, old, cx);
    acknowledge(&fixture, snapshot.clone(), cx);
    assert!(drain(&mut fixture.commands).is_empty());
    checked(settings, false, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert_eq!(&*view.workspace, snapshot);
        assert_eq!(view.project_path.as_ref(), Some(&copy));
        assert!(!view.workspace_dirty && view.pending_workspace.is_none());
        assert!(!view.choosing_save_as && view.file_picker.is_none());
        assert!(!view.saving_project_snapshot && view.pending_project_save.is_none());
        assert_eq!(view.project_saves_in_flight, 0);
        assert!(view.error.is_none());
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert!(!view.plugin_waiting(a.id));
        assert!(!cancel.is_cancelled());
        assert_eq!(
            view.tab_editors[&a.id].request.bytes_lossless(cx),
            snapshot.tabs[0].draft.raw
        );
        assert_eq!(native_b.read(cx).value().as_bytes(), latest.as_bytes());
        assert!(rendered_status(window).starts_with("项目已保存"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn settings_save_as_enqueue_failure_does_not_revive_send_on_save(cx: &mut TestAppContext) {
    let (mut fixture, stored) = saved_repeater_fixture(cx);
    let id = stored.tabs[0].id;
    let source = cx.read(|cx| fixture.view.read(cx).project_path.clone().unwrap());
    let destination =
        std::env::temp_dir().join(format!("mio-settings-full-{}.mio", Uuid::new_v4()));
    let editor = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    let body = "另存为入队失败仍保留中文草稿🙂";
    let latest = format!(
        "POST /queue-full HTTP/1.1\r\nHost: 127.0.0.1:9001\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(&latest, cx);
    })
    .unwrap();
    let settings = open_settings(&fixture, cx);
    toggle(settings, false, cx);
    let old = saved(&mut fixture.commands);
    toggle(settings, true, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-send", cx)
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    let mut expected_draft = stored.tabs[0].draft.clone();
    expected_draft.raw = latest.as_bytes().to_vec();
    expected_draft.revision += 1;
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.pending_sends[&id].draft, expected_draft);
        assert_eq!(view.workspace.tabs[0].draft, expected_draft);
    });
    save_as_ack_while_choosing(&mut fixture, settings, old.clone(), cx);

    let filled = fill_command_queue(&fixture, cx);
    cx.simulate_new_path_selection(|_| Some(destination.clone()));
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_new_path());
    visible_error(settings, "命令队列已满", cx);
    checked(settings, true, cx);
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
    let (retained, send_cancelled) = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.revision, old.revision);
        assert_eq!(view.workspace.tabs[0].draft, expected_draft);
        assert_eq!(view.project_path.as_ref(), Some(&source));
        assert!(view.workspace.repeater_settings.unpack_compressed_responses);
        assert!(view.workspace_dirty && view.pending_workspace.is_none());
        assert!(!view.choosing_save_as && !view.saving_project_snapshot);
        assert!(view.pending_project_save.is_none());
        assert_eq!(view.project_saves_in_flight, 0);
        assert!(view.transition.is_none() && view.after_save.is_none());
        assert!(view.sending.is_empty());
        assert_eq!(editor.read(cx).value().as_bytes(), latest.as_bytes());
        assert_eq!(
            view.tab_editors[&id].target.read(cx).value().as_ref(),
            expected_draft.target
        );
        ((*view.workspace).clone(), view.pending_sends.is_empty())
    });

    menu_action(settings, "项目", "保存项目", cx);
    assert!(!cx.did_prompt_for_new_path());
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveWorkspace {
            project,
            workspace: retry,
        },
    ] = commands.as_slice()
    else {
        panic!("ordinary Save must retry only the retained workspace after Save As enqueue failure")
    };
    assert_eq!(*project, fixture.project);
    let mut expected = retained;
    expected.revision = old.revision + 1;
    assert_eq!(retry, &expected);
    acknowledge(&fixture, retry.clone(), cx);
    let commands = drain(&mut fixture.commands);
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, Command::Send { .. })),
        "a later Save ACK must not revive the Send abandoned by Save As enqueue failure"
    );
    assert!(matches!(commands.as_slice(),
        [Command::SaveProject { websocket_notes: _, ext_http_notes: _, ext_http_highlights: _, project, path: None, workspace: None }]
            if *project == fixture.project));
    acknowledge_project_saved(&fixture, source.clone(), cx);
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(
        send_cancelled,
        "failed Save As enqueue must immediately discard its deferred Send"
    );
    checked(settings, true, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(*view.workspace, expected);
        assert_eq!(view.project_path.as_ref(), Some(&source));
        assert_ne!(view.project_path.as_ref(), Some(&destination));
        assert!(!view.workspace_dirty && view.pending_workspace.is_none());
        assert!(view.pending_project_save.is_none() && !view.saving_project_snapshot);
        assert_eq!(view.project_saves_in_flight, 0);
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert!(view.error.is_none());
        assert_eq!(editor.read(cx).value().as_bytes(), latest.as_bytes());
    });
}
