use super::*;

#[gpui_kit::test]
fn settings_save_as_failure_preserves_latest_draft_and_retries_snapshot(cx: &mut TestAppContext) {
    let (mut fixture, stored) = saved_repeater_fixture(cx);
    let id = stored.tabs[0].id;
    let source = cx.read(|cx| fixture.view.read(cx).project_path.clone().unwrap());
    let editor = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    let latest = "GET /settings-save-as HTTP/1.1\r\nHost: 127.0.0.1:9001\r\nX-Note: 保存失败仍保留🙂\r\n\r\n";
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(latest, cx);
    })
    .unwrap();
    let settings = open_settings(&fixture, cx);
    toggle(settings, false, cx);
    let old = saved(&mut fixture.commands);
    menu_action(settings, "项目", "另存为…", cx);
    assert!(cx.did_prompt_for_new_path());
    acknowledge(&fixture, old.clone(), cx);
    assert!(drain(&mut fixture.commands).is_empty());
    let copy = std::env::temp_dir().join(format!("mio-settings-retry-{}.mio", Uuid::new_v4()));
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
            workspace: Some(attempt),
        },
    ] = commands.as_slice()
    else {
        panic!("Save As must carry the latest draft and setting directly to its destination")
    };
    assert_eq!((*project, path), (fixture.project, &copy));
    assert_eq!(attempt.revision, old.revision + 1);
    assert!(!attempt.repeater_settings.unpack_compressed_responses);
    assert_eq!(attempt.tabs[0].draft.raw, latest.as_bytes());
    assert_eq!(attempt.tabs[0].draft.target, stored.tabs[0].draft.target);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert_eq!(view.project_path.as_ref(), Some(&source));
        assert!(view.saving_project_snapshot && !view.workspace_dirty);
        assert_eq!(view.pending_workspace, Some(attempt.revision));
        assert_eq!(view.project_saves_in_flight, 1);
        let status = rendered_status(window);
        assert!(status.starts_with("正在保存… · ") && !status.contains("项目已保存"));
    })
    .unwrap();
    fixture
        .events
        .try_send(Event::Error {
            project: Some(fixture.project),
            operation: "save_project",
            message: "设置目的盘已满".into(),
        })
        .unwrap();
    cx.run_until_parked();
    visible_error(settings, "设置目的盘已满", cx);
    checked(settings, false, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project_path.as_ref(), Some(&source));
        assert_eq!(view.workspace.tabs, attempt.tabs);
        assert_eq!(view.workspace.repeater_settings, attempt.repeater_settings);
        assert!(view.workspace_dirty && view.pending_workspace.is_none());
        assert!(!view.saving_project_snapshot);
        assert!(view.pending_project_save.is_none());
        assert_eq!(view.project_saves_in_flight, 0);
        assert_eq!(view.project_save_status, "项目保存失败");
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert_eq!(editor.read(cx).value().as_bytes(), latest.as_bytes());
    });

    menu_action(settings, "项目", "另存为…", cx);
    assert!(cx.did_prompt_for_new_path());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.simulate_new_path_selection(|_| Some(copy.clone()));
    cx.run_until_parked();
    let retry = drain(&mut fixture.commands);
    let [
        Command::SaveProject {
            websocket_notes: _,
            ext_http_notes: _,
            ext_http_highlights: _,
            project,
            path: Some(path),
            workspace: Some(retried),
        },
    ] = retry.as_slice()
    else {
        panic!("retry must save the retained snapshot without another source write or Send")
    };
    assert_eq!((*project, path), (fixture.project, &copy));
    assert_eq!(retried, attempt);
    acknowledge(&fixture, retried.clone(), cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let status = rendered_status(window);
        assert!(status.starts_with("正在保存… · ") && !status.contains("项目已保存"));
    })
    .unwrap();
    acknowledge_project_saved(&fixture, copy.clone(), cx);
    checked(settings, false, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert_eq!(&*view.workspace, retried);
        assert_eq!(view.project_path.as_ref(), Some(&copy));
        assert!(!view.workspace_dirty && view.pending_workspace.is_none());
        assert!(!view.saving_project_snapshot && view.pending_project_save.is_none());
        assert_eq!(view.project_saves_in_flight, 0);
        assert!(view.error.is_none());
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert_eq!(editor.read(cx).value().as_bytes(), latest.as_bytes());
        let status = rendered_status(window);
        assert!(status.starts_with("项目已保存（4096 字节，含 sidecar）"));
        assert!(
            !status.contains("未保存") && !status.contains("正在保存"),
            "acknowledged Settings Save As must not retain a saving footer: {status}"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn settings_save_as_success_sends_once_with_activation_settings(cx: &mut TestAppContext) {
    let (mut fixture, stored) = saved_repeater_fixture(cx);
    let id = stored.tabs[0].id;
    let settings_window = open_settings(&fixture, cx);
    toggle(settings_window, false, cx);
    let old = saved(&mut fixture.commands);
    toggle(settings_window, true, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-send", cx);
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    save_as_ack_while_choosing(&mut fixture, settings_window, old.clone(), cx);
    toggle(settings_window, false, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    let copy = std::env::temp_dir().join(format!("mio-settings-send-{}.mio", Uuid::new_v4()));
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
        panic!("the pending Send must wait for the accepted Save As snapshot")
    };
    assert_eq!((*project, path), (fixture.project, &copy));
    assert_eq!(snapshot.revision, old.revision + 1);
    assert_eq!(snapshot.tabs, stored.tabs);
    assert!(!snapshot.repeater_settings.unpack_compressed_responses);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.pending_workspace, Some(snapshot.revision));
        assert_eq!(view.pending_sends.len(), 1);
        assert!(view.pending_sends[&id].settings.unpack_compressed_responses);
        assert_eq!(
            view.pending_sends[&id].required_workspace_revision,
            snapshot.revision
        );
        assert!(view.sending.is_empty());
    });
    acknowledge(&fixture, snapshot.clone(), cx);
    let sent = drain(&mut fixture.commands);
    let [
        Command::Send {
            project,
            tab_id,
            run_id,
            draft,
            settings,
            cancel,
        },
    ] = sent.as_slice()
    else {
        panic!("the destination workspace ACK must release exactly one captured Send")
    };
    assert_eq!((*project, *tab_id), (fixture.project, id));
    assert_eq!(draft, &stored.tabs[0].draft);
    assert!(settings.unpack_compressed_responses);
    assert!(!cancel.is_cancelled());
    acknowledge_project_saved(&fixture, copy.clone(), cx);
    acknowledge(&fixture, old, cx);
    acknowledge(&fixture, snapshot.clone(), cx);
    assert!(drain(&mut fixture.commands).is_empty());
    checked(settings_window, false, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(&*view.workspace, snapshot);
        assert_eq!(view.project_path.as_ref(), Some(&copy));
        assert!(!view.workspace_dirty && view.pending_workspace.is_none());
        assert!(view.pending_sends.is_empty() && view.pending_project_save.is_none());
        assert_eq!(view.project_saves_in_flight, 0);
        assert_eq!(view.sending.len(), 1);
        assert_eq!(view.sending[&id].run_id, *run_id);
        assert!(!cancel.is_cancelled());
    });
}

#[gpui_kit::test]
fn settings_send_revision_overflow_preserves_setting_and_draft(cx: &mut TestAppContext) {
    let first = tab("revision 边界的未发送草稿");
    let mut fixture = fixture(cx, vec![first.clone()], 64);
    let mut opened = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
    opened.revision = u64::MAX - 1;
    opened.tab_sidebar_width = 180;
    opened.repeater_settings.unpack_compressed_responses = false;
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: fixture.project,
            path: None,
            workspace: opened.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);
    checked(settings, false, cx);
    toggle(settings, true, cx);
    let pending = saved(&mut fixture.commands);
    assert_eq!(pending.revision, u64::MAX);
    toggle(settings, false, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    activate_empty_repeater(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-send", cx);
    })
    .unwrap();
    cx.run_until_parked();
    visible_error(settings, "工作区 revision 已耗尽；未发送请求。", cx);
    checked(settings, false, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(*view.workspace, opened);
        assert_eq!(view.pending_workspace, Some(u64::MAX));
        assert!(view.workspace_dirty);
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert!(view.pending_project_save.is_none());
        assert_eq!(
            view.tab_editors[&first.id].request.bytes_lossless(cx),
            first.draft.raw
        );
    });
    acknowledge(&fixture, pending, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.revision, u64::MAX);
        assert_eq!(view.workspace.tabs, vec![first]);
        assert!(!view.workspace.repeater_settings.unpack_compressed_responses);
        assert!(view.workspace_dirty && view.pending_workspace.is_none());
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
    });
}
