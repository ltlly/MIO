use super::*;

#[gpui_kit::test]
fn pointer_drag_can_insert_after_the_last_tab(cx: &mut TestAppContext) {
    let first = tab("1");
    let first_id = first.id;
    let second = tab("2");
    let second_id = second.id;
    let mut fixture = fixture(cx, vec![first, second], 64);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
    })
    .unwrap();
    choose_tab_orientation(&fixture, Orientation::Vertical, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        let source = window
            .find(SharedString::from(format!("work-tab-{first_id}")))
            .bounds();
        let destination = window
            .find(SharedString::from(format!("work-tab-{second_id}")))
            .bounds();
        let start = source.center();
        let end = point(destination.center().x, destination.bottom() - px(4.));
        window.pointer_down(start, MouseButton::Left, cx);
        window.pointer_move(start + point(px(12.), px(0.)), Some(MouseButton::Left), cx);
        window.pointer_move(end, Some(MouseButton::Left), cx);
        window.pointer_up(end, MouseButton::Left, cx);
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.tabs[0].id, second_id);
        assert_eq!(view.workspace.tabs[1].id, first_id);
    })
    .unwrap();
}

#[gpui_kit::test]
fn new_tab_reply_merges_with_local_layout_while_one_save_is_in_flight(cx: &mut TestAppContext) {
    let first = tab("原有");
    let first_id = first.id;
    let second = tab("新建");
    let second_id = second.id;
    let mut fixture = fixture(cx, vec![first], 64);
    drain(&mut fixture.commands);
    let editor = cx.read(|cx| {
        fixture.view.read(cx).tab_editors[&first_id]
            .request
            .raw
            .entity_id()
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click("repeater-new", cx);
    })
    .unwrap();
    choose_tab_orientation(&fixture, Orientation::Vertical, cx);
    let commands = drain(&mut fixture.commands);
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, Command::NewRepeater { .. }))
    );
    let first_save = commands
        .into_iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::RepeaterOpened {
            project: fixture.project,
            tab: second,
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let commands = acknowledge_history_query(&fixture, commands, cx);
    assert!(
        commands.is_empty(),
        "only one workspace save may be in flight"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.orientation, Orientation::Vertical);
        assert_eq!(view.workspace.tabs.len(), 2);
        assert_eq!(view.workspace.active, Some(second_id));
        assert_eq!(view.tab_editors[&first_id].request.raw.entity_id(), editor);
    });
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: first_save.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    let next = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .unwrap();
    assert_eq!(next.revision, first_save.revision + 1);
    assert_eq!(next.orientation, Orientation::Vertical);
    assert_eq!(next.tabs.len(), 2);
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: next,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: first_save,
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert!(acknowledge_history_query(&fixture, commands, cx).is_empty());
    cx.read(|cx| assert_eq!(fixture.view.read(cx).workspace.tabs.len(), 2));
}

#[gpui_kit::test]
fn rejected_save_keeps_latest_draft_and_never_automatically_sends(cx: &mut TestAppContext) {
    let first = tab("保存错误");
    let id = first.id;
    let mut fixture = fixture(cx, vec![first], 64);
    drain(&mut fixture.commands);
    let stored = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let editor = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("end"), cx);
        window.input("不能丢", cx);
        window.click("repeater-send", cx);
    })
    .unwrap();
    let rejected = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
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
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.workspace_dirty);
        assert!(view.pending_sends.is_empty());
        assert!(view.sending.is_empty());
        assert!(
            view.workspace.tabs[0]
                .draft
                .raw
                .ends_with("不能丢".as_bytes())
        );
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("repeater-send", cx);
    })
    .unwrap();
    let retry = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        retry.revision, rejected.revision,
        "failed saves do not consume a stored revision"
    );
    assert_eq!(retry.tabs[0].draft, rejected.tabs[0].draft);
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: retry.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    let sent = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::Send { draft, .. } => Some(draft),
            _ => None,
        })
        .unwrap();
    assert_eq!(sent, retry.tabs[0].draft);
}

#[gpui_kit::test]
fn cancelling_before_save_ack_never_opens_a_network_request(cx: &mut TestAppContext) {
    let first = tab("取消保存中的发送");
    let mut fixture = fixture(cx, vec![first], 64);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
    })
    .unwrap();
    choose_tab_orientation(&fixture, Orientation::Vertical, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("repeater-cancel").is_none());
        let send_bounds = window.find("repeater-send").bounds();
        window.click("repeater-send", cx);
        window.render_frame(cx);
        assert!(window.try_find("repeater-send").is_none());
        let cancel_bounds = window.find("repeater-cancel").bounds();
        assert_eq!(cancel_bounds.origin, send_bounds.origin);
        assert_eq!(cancel_bounds.size, send_bounds.size);
        window.click("repeater-cancel", cx);
        window.render_frame(cx);
        assert!(window.try_find("repeater-cancel").is_none());
        let send = window.find("repeater-send");
        assert_eq!(send.disabled(), None);
        assert_eq!(send.bounds().origin, send_bounds.origin);
        assert_eq!(send.bounds().size, send_bounds.size);
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, Command::Send { .. } | Command::CancelSend { .. }))
    );
    let saved = commands
        .into_iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: saved,
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn save_as_captures_late_changes_without_requiring_a_source_write(cx: &mut TestAppContext) {
    let first = tab("待另存为");
    let id = first.id;
    let second = tab("稍后到达");
    let second_id = second.id;
    let mut fixture = fixture(cx, vec![first], 64);
    drain(&mut fixture.commands);
    let editor = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click("repeater-new", cx);
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("end"), cx);
        window.input("最新未写入", cx);
        window.press(crate::kbd!("shift-s"), cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_new_path());
    let commands = drain(&mut fixture.commands);
    let commands = acknowledge_history_query(&fixture, commands, cx);
    assert_eq!(commands.len(), 1);
    assert!(matches!(commands[0], Command::NewRepeater { .. }));
    fixture
        .events
        .try_send(Event::RepeaterOpened {
            project: fixture.project,
            tab: second,
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "late updates while choosing Save As must not write to the failed source"
    );
    let destination = PathBuf::from("/tmp/mio-ui-save-as-test.mio");
    cx.simulate_new_path_selection(|_| Some(destination.clone()));
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert_eq!(commands.len(), 1);
    let saved = match &commands[0] {
        Command::SaveProject {
            path: Some(path),
            workspace: Some(workspace),
            ..
        } => {
            assert_eq!(*path, destination);
            workspace.clone()
        }
        _ => panic!("Save As must atomically carry the latest workspace to the destination"),
    };
    assert_eq!(saved.tabs.len(), 2);
    assert_eq!(saved.active, Some(second_id));
    assert!(saved.tabs[0].draft.raw.ends_with("最新未写入".as_bytes()));
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: saved.clone(),
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path: destination.clone(),
            total_bytes: 4096,
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project_path, Some(destination));
        assert_eq!(view.workspace, saved);
        assert!(!view.workspace_dirty);
        assert!(view.pending_workspace.is_none());
    });
}

#[gpui_kit::test]
fn save_as_recovers_after_an_already_queued_source_save_is_rejected(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("源项目不可写")], 64);
    drain(&mut fixture.commands);
    let stored = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
    })
    .unwrap();
    choose_tab_orientation(&fixture, Orientation::Vertical, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.press(crate::kbd!("shift-s"), cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_new_path());
    let rejected = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .unwrap();
    let destination = PathBuf::from("/tmp/mio-ui-save-as-recovery-test.mio");
    cx.simulate_new_path_selection(|_| Some(destination.clone()));
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    fixture
        .events
        .try_send(Event::WorkspaceRejected {
            project: fixture.project,
            revision: rejected.revision,
            stored,
            message: "源项目只读".into(),
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert_eq!(commands.len(), 1);
    let saved = match &commands[0] {
        Command::SaveProject {
            path: Some(path),
            workspace: Some(workspace),
            ..
        } => {
            assert_eq!(*path, destination);
            workspace
        }
        _ => panic!("source rejection must not discard the explicit Save As intent"),
    };
    assert_eq!(saved.revision, rejected.revision);
    assert_eq!(saved.orientation, Orientation::Vertical);
    fixture
        .events
        .try_send(Event::Error {
            project: Some(fixture.project),
            operation: "save_project",
            message: "目的盘已满".into(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.project_path.is_none());
        assert!(view.pending_workspace.is_none());
        assert!(view.workspace_dirty);
        assert_eq!(view.workspace.orientation, Orientation::Vertical);
    });
}
