use super::*;

#[gpui_kit::test]
fn old_ack_cannot_send_a_request_waiting_for_new_checkbox_setting(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("1")], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);
    toggle(settings, false, cx);
    let old = saved(&mut fixture.commands);
    toggle(settings, true, cx);
    activate_empty_repeater(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-send", cx);
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    acknowledge(&fixture, old.clone(), cx);
    let commands = drain(&mut fixture.commands);
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, Command::Send { .. })),
        "old ACK must not send the new-setting request using the old saved setting"
    );
    let latest = commands
        .into_iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .expect("new checkbox setting is persisted after the old ACK");
    assert!(latest.revision > old.revision);
    assert!(latest.repeater_settings.unpack_compressed_responses);
    acknowledge(&fixture, latest, cx);
    assert_eq!(
        drain(&mut fixture.commands)
            .iter()
            .filter(|command| matches!(command, Command::Send { .. }))
            .count(),
        1
    );
}

#[gpui_kit::test]
fn settings_rejection_preserves_latest_draft_and_save_as_recovers_from_auxiliary_window(
    cx: &mut TestAppContext,
) {
    let first = tab("保留编辑");
    let id = first.id;
    let mut fixture = fixture(cx, vec![first], 64);
    drain(&mut fixture.commands);
    let stored = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let editor = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    let settings = open_settings(&fixture, cx);
    toggle(settings, false, cx);
    let rejected = saved(&mut fixture.commands);
    activate_empty_repeater(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("end"), cx);
        window.input("保留最新草稿🙂", cx);
        window.click("repeater-send", cx);
    })
    .unwrap();
    fixture
        .events
        .try_send(Event::WorkspaceRejected {
            project: fixture.project,
            revision: rejected.revision,
            stored,
            message: "只读测试磁盘".into(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    checked(settings, false, cx);
    visible_error(settings, "只读测试磁盘", cx);
    cx.simulate_keystrokes(settings, "escape");
    let reopened = open_settings(&fixture, cx);
    checked(reopened, false, cx);
    visible_error(reopened, "只读测试磁盘", cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.workspace_dirty);
        assert!(view.pending_sends.is_empty());
        assert!(
            view.workspace.tabs[0]
                .draft
                .raw
                .ends_with("保留最新草稿🙂".as_bytes())
        );
        assert!(editor.read(cx).value().ends_with("保留最新草稿🙂"));
    });
    menu_action(reopened, "项目", "另存为…", cx);
    assert!(cx.did_prompt_for_new_path());
    let destination = PathBuf::from("/tmp/mio-settings-recovery.mio");
    cx.simulate_new_path_selection(|_| Some(destination.clone()));
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert_eq!(
        commands.len(),
        1,
        "Save As bypasses the failed source write"
    );
    let Command::SaveProject {
        websocket_notes: _,
        ext_http_notes: _,
        ext_http_highlights: _,
        project,
        path: Some(path),
        workspace: Some(workspace),
    } = &commands[0]
    else {
        panic!("Save As carries the latest project snapshot");
    };
    assert_eq!((*project, path), (fixture.project, &destination));
    assert!(!workspace.repeater_settings.unpack_compressed_responses);
    assert!(
        workspace.tabs[0]
            .draft
            .raw
            .ends_with("保留最新草稿🙂".as_bytes())
    );
    acknowledge(&fixture, workspace.clone(), cx);
    fixture
        .events
        .try_send(Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path: destination,
            total_bytes: 4096,
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    // The recovered draft has no sent history; only acknowledge its post-save query.
    let commands = acknowledge_history_query(&fixture, commands, cx);
    assert!(commands.is_empty(), "recovery never retries Send");
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.workspace_dirty);
        assert!(view.pending_workspace.is_none());
    });
    checked(reopened, false, cx);
}

#[gpui_kit::test]
fn settings_event_disconnect_preserves_value_and_cancels_waiting_send(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("断连保留")], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);
    toggle(settings, false, cx);
    let pending = saved(&mut fixture.commands);
    activate_empty_repeater(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-send", cx);
    })
    .unwrap();
    let old_events = std::mem::replace(&mut fixture.events, mpsc::channel(1).0);
    drop(old_events);
    cx.run_until_parked();
    visible_error(settings, "后台服务已断开", cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.backend_disconnected);
        assert!(view.workspace_dirty);
        assert_eq!(view.pending_workspace, Some(pending.revision));
        assert!(view.workspace.revision < pending.revision);
        assert!(view.pending_sends.is_empty());
        assert_eq!(view.workspace.tabs[0].title, "断连保留");
    });
    cx.simulate_keystrokes(settings, "escape");
    let reopened = open_settings(&fixture, cx);
    checked(reopened, false, cx);
    cx.update_window(reopened, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("repeater-unpack-responses").disabled(),
            Some(true)
        );
        window.click("repeater-unpack-responses", cx);
    })
    .unwrap();
    checked(reopened, false, cx);
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn settings_closed_command_channel_preserves_checkbox_draft_and_visible_error(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);
    fixture.commands.close();
    toggle(settings, false, cx);
    visible_error(settings, "后台服务已停止", cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.workspace_dirty);
        assert!(view.pending_workspace.is_none());
    });
    cx.simulate_keystrokes(settings, "escape");
    checked(open_settings(&fixture, cx), false, cx);
}

#[gpui_kit::test]
fn full_command_queue_retains_settings_and_auxiliary_save_restores_main_shortcuts(
    cx: &mut TestAppContext,
) {
    let first = tab("队列恢复");
    let id = first.id;
    let original = first.draft.clone();
    let mut fixture = fixture(cx, vec![first], 1);
    drain(&mut fixture.commands);
    let path = PathBuf::from("/tmp/mio-settings-keyboard.mio");
    fixture
        .events
        .try_send(Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path: path.clone(),
            total_bytes: 4096,
        })
        .unwrap();
    cx.run_until_parked();
    let editor = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    activate_empty_repeater(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!(","), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 2);
    let settings_window = open_settings(&fixture, cx);
    cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .commands
            .try_send(Command::QueryHttpHistory {
                query: mio_core::HttpHistoryQuery {
                    project: fixture.project,
                    query_revision: u64::MAX,
                    offset: 0,
                    limit: 1,
                    url_filter: String::new(),
                    filter: Default::default(),
                    sort: Default::default(),
                    selected_ids: Vec::new(),
                },
            })
            .unwrap();
    });
    toggle(settings_window, false, cx);
    visible_error(settings_window, "命令队列已满", cx);
    cx.simulate_keystrokes(settings_window, "escape");
    cx.update_window(fixture.window, |_, window, cx| {
        window.press("cmd-enter", cx)
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.workspace_dirty && view.pending_workspace.is_none());
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert_eq!(view.tab_editors[&id].request.raw, editor);
        assert_eq!(view.tab_editors[&id].request.bytes_lossless(cx), original.raw);
    });
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryHttpHistory {
            query: mio_core::HttpHistoryQuery {
                query_revision: u64::MAX,
                ..
            }
        }]
    ));
    let settings_window = open_settings(&fixture, cx);
    checked(settings_window, false, cx);
    cx.simulate_keystrokes(settings_window, crate::kbd!("s"));
    cx.run_until_parked();
    let workspace = saved(&mut fixture.commands);
    assert!(!workspace.repeater_settings.unpack_compressed_responses);
    assert_eq!(workspace.tabs[0].draft, original);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let status = rendered_status(window);
        assert!(status.starts_with("正在保存… · ") && !status.contains("项目已保存"));
    })
    .unwrap();
    cx.simulate_keystrokes(settings_window, crate::kbd!("w"));
    acknowledge(&fixture, workspace, cx);
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::SaveProject {
            path: None,
            workspace: None,
            ..
        }]
    ));
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let status = rendered_status(window);
        assert!(status.starts_with("正在保存… · ") && !status.contains("项目已保存"));
    })
    .unwrap();
    fixture
        .events
        .try_send(Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path,
            total_bytes: 4096,
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let status = rendered_status(window);
        assert!(status.starts_with("项目已保存（4096 字节，含 sidecar）"));
        assert!(
            !status.contains("未保存") && !status.contains("正在保存"),
            "acknowledged Settings Save must not retain a saving footer: {status}"
        );
        window.press("cmd-enter", cx)
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::Send {
            settings, cancel, ..
        },
    ] = commands.as_slice()
    else {
        panic!("main Send shortcut uses the recovered settings exactly once");
    };
    assert!(!settings.unpack_compressed_responses);
    assert!(!cancel.is_cancelled());
    cx.simulate_keystrokes(fixture.window, crate::kbd!("s"));
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::SaveProject {
            path: None,
            workspace: None,
            ..
        }]
    ));
    cx.simulate_keystrokes(fixture.window, crate::kbd!("period"));
    assert!(cancel.is_cancelled());
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::CancelSend { .. }]
    ));
    assert_eq!(cx.windows(), vec![fixture.window]);
}

#[gpui_kit::test]
fn settings_window_rebinds_two_projects_and_ignores_old_project_ack(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("A")], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);
    toggle(settings, false, cx);
    let saved_a = saved(&mut fixture.commands);
    activate_empty_repeater(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-send", cx);
    })
    .unwrap();
    project_menu(&fixture, "新建项目", cx);
    cx.simulate_prompt_answer("放弃并继续");
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands)
            .iter()
            .any(|command| matches!(command, Command::NewProject { .. }))
    );
    let project_a = fixture.project;
    let project_b = ProjectKey {
        id: Uuid::new_v4(),
        generation: project_a.generation + 1,
    };
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: project_b,
            path: None,
            workspace: Workspace::default(),
        })
        .unwrap();
    cx.run_until_parked();
    checked(settings, true, cx);
    acknowledge(&fixture, saved_a.clone(), cx);
    assert!(
        !drain(&mut fixture.commands)
            .iter()
            .any(|command| matches!(command, Command::Send { .. }))
    );
    fixture.project = project_b;
    toggle(settings, false, cx);
    let saved_b = saved(&mut fixture.commands);
    acknowledge(&fixture, saved_b, cx);
    toggle(settings, true, cx);
    let saved_b = saved(&mut fixture.commands);
    acknowledge(&fixture, saved_b.clone(), cx);
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: ProjectKey {
                generation: project_b.generation + 1,
                ..project_a
            },
            path: None,
            workspace: saved_a,
        })
        .unwrap();
    cx.run_until_parked();
    checked(settings, false, cx);
    acknowledge(&fixture, saved_b, cx);
    checked(settings, false, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.tabs[0].title, "A");
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
    });
    assert_eq!(cx.windows().len(), 2);
}
