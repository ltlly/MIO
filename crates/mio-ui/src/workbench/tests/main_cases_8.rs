use super::*;

#[gpui_kit::test]
fn saved_project_transition_waits_for_quiesce_terminal_flow_and_its_own_save(
    cx: &mut TestAppContext,
) {
    let first = tab("切换前的响应");
    let id = first.id;
    let mut fixture = fixture(cx, vec![first], 64);
    let path = PathBuf::from("/tmp/mio-ui-transition-source.mio");
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
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click("repeater-send", cx);
        window.press(crate::kbd!("s"), cx);
    })
    .unwrap();
    let ordinary = drain(&mut fixture.commands);
    assert!(
        ordinary
            .iter()
            .any(|command| matches!(command, Command::Send { .. }))
    );
    assert!(
        ordinary
            .iter()
            .any(|command| matches!(command, Command::SaveProject { .. }))
    );
    assert!(
        !ordinary
            .iter()
            .any(|command| matches!(command, Command::Quiesce { .. }))
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.press(crate::kbd!("n"), cx);
    })
    .unwrap();
    let transition = drain(&mut fixture.commands);
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
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "an ordinary Save acknowledgement cannot switch projects before terminal events"
    );
    assert_eq!(transition.len(), 1);
    assert!(matches!(transition[0], Command::Quiesce { project } if project == fixture.project));

    let editor = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("end"), cx);
        window.input("停止期间的最新草稿", cx);
    })
    .unwrap();
    let mut terminal = record("http://127.0.0.1:9001/", "最后响应");
    terminal.summary.state = FlowState::Cancelled;
    terminal.summary.id = ordinary
        .iter()
        .find_map(|command| match command {
            Command::Send { run_id, .. } => Some(*run_id),
            _ => None,
        })
        .unwrap();
    let flow_id = terminal.summary.id;
    fixture
        .events
        .try_send(Event::SendFinished {
            project: fixture.project,
            tab_id: id,
            draft_revision: 1,
            record: Box::new(terminal),
            persisted: true,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::Quiesced {
            project: fixture.project,
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert!(!commands.iter().any(|command| matches!(
        command,
        Command::NewProject { .. } | Command::SaveProject { .. }
    )));
    let saved = commands
        .into_iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .expect("post-quiesce workspace must be durable before project save");
    assert_eq!(saved.tabs[0].last_flow, Some(flow_id));
    assert!(
        saved.tabs[0]
            .draft
            .raw
            .ends_with("停止期间的最新草稿".as_bytes())
    );
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: saved,
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert_eq!(commands.len(), 1);
    assert!(
        matches!(commands[0], Command::SaveProject { project, path: None, .. } if project == fixture.project)
    );
    fixture
        .events
        .try_send(Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path: PathBuf::from("/tmp/not-the-selected-project.mio"),
            total_bytes: 4096,
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "an unrelated save cannot finish the transition"
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let status = rendered_status(window);
        assert!(
            status.contains("正在保存") && !status.contains("项目已保存"),
            "an ignored save acknowledgement must not clear the visible pending state: {status}"
        );
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
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::NewProject { path: None }]
    ));
}

#[gpui_kit::test]
fn failed_terminal_storage_aborts_quit_and_preserves_response_for_save_as(cx: &mut TestAppContext) {
    let first = tab("写盘失败时保留响应");
    let id = first.id;
    let mut fixture = fixture(cx, vec![first], 64);
    drain(&mut fixture.commands);
    let shutdown = cx.read(|cx| fixture.view.read(cx).shutdown.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click("repeater-send", cx);
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    cx.simulate_prompt_answer("保存");
    pump_native(cx);
    let commands = drain(&mut fixture.commands);
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, Command::Quiesce { .. }))
    );
    assert!(
        !cx.did_prompt_for_new_path(),
        "the save picker must wait for terminal events"
    );
    let mut terminal = record("http://127.0.0.1:9001/", "最后响应");
    terminal.summary.state = FlowState::Complete;
    terminal.response = Some(
        parse_response(
            "HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\n最后响应".as_bytes(),
            HttpVersion::Http11,
        )
        .unwrap(),
    );
    terminal.summary.id = commands
        .iter()
        .find_map(|command| match command {
            Command::Send { run_id, .. } => Some(*run_id),
            _ => None,
        })
        .unwrap();
    let flow_id = terminal.summary.id;
    fixture
        .events
        .try_send(Event::SendFinished {
            project: fixture.project,
            tab_id: id,
            draft_revision: 1,
            record: Box::new(terminal),
            persisted: false,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::StorageState {
            project: fixture.project,
            pending_records: 1,
            message: Some("测试磁盘已满".into()),
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::Quiesced {
            project: fixture.project,
        })
        .unwrap();
    pump_native(cx);
    assert!(!shutdown.is_cancelled());
    let commands = drain(&mut fixture.commands);
    assert!(
        matches!(commands.as_slice(), [Command::Resume { project }] if *project == fixture.project)
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.after_save.is_none());
        assert!(view.workspace_dirty);
        assert_eq!(view.workspace.tabs[0].last_flow, Some(flow_id));
        assert!(
            view.tab_editors[&id]
                .response
                .raw
                .read(cx)
                .value()
                .ends_with("最后响应")
        );
        assert!(view.storage_error.is_some());
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("storage-save-as", cx);
    })
    .unwrap();
    let destination = PathBuf::from("/tmp/mio-ui-terminal-recovery.mio");
    cx.simulate_new_path_selection(|_| Some(destination));
    pump_native(cx);
    let commands = drain(&mut fixture.commands);
    assert!(
        matches!(commands.as_slice(), [Command::SaveProject { workspace: Some(workspace), .. }] if workspace.tabs[0].last_flow == Some(flow_id))
    );
    assert!(!shutdown.is_cancelled());
}

#[gpui_kit::test]
fn cancelling_transition_save_picker_resumes_without_restarting_network(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("取消退出")], 64);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(Quit), cx)
    })
    .unwrap();
    pump_native(cx);
    cx.simulate_prompt_answer("保存");
    pump_native(cx);
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::Quiesce { .. }]
    ));
    fixture
        .events
        .try_send(Event::Quiesced {
            project: fixture.project,
        })
        .unwrap();
    pump_native(cx);
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| None);
    pump_native(cx);
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::Resume { .. }]
    ));
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.after_save.is_none());
        assert!(!view.quitting);
        assert!(!view.shutdown.is_cancelled());
    });
}

#[gpui_kit::test]
fn cancel_transition_survives_a_full_queue_and_ignores_its_late_quiesced_event(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("取消排队")], 1);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    cx.simulate_prompt_answer("保存");
    pump_native(cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("cancel-transition", cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(matches!(
        fixture.commands.try_recv().unwrap(),
        Command::Quiesce { .. }
    ));
    pump_native(cx);
    assert!(matches!(
        fixture.commands.try_recv().unwrap(),
        Command::Resume { .. }
    ));
    fixture
        .events
        .try_send(Event::Quiesced {
            project: fixture.project,
        })
        .unwrap();
    pump_native(cx);
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(!cx.did_prompt_for_new_path());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.after_save.is_none());
        assert!(view.transition.is_none());
        assert!(!view.shutdown.is_cancelled());
    });
}
