use super::*;

#[gpui_kit::test]
fn history_clicks_use_stable_ids_and_ignore_late_detail_and_old_project(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let revision = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::QueryHttpHistory {
                query: mio_core::HttpHistoryQuery { query_revision, .. },
            } => Some(query_revision),
            _ => None,
        })
        .unwrap();
    let first = record("http://127.0.0.1:9001/one", "一");
    let second = record("http://127.0.0.1:9001/two", "二");
    fixture
        .events
        .try_send(Event::HistoryPage {
            project: fixture.project,
            query_revision: revision,
            offset: 0,
            total: 2,
            rows: vec![first.summary.clone().into(), second.summary.clone().into()],
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-history", cx);
        window.render_frame(cx);
        assert!(window.try_find("history-request-version").is_none());
        window.click("table-cell-0:0", cx);
    })
    .unwrap();
    let first_read = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::ReadFlow {
                selection_revision,
                flow_id,
                ..
            } => Some((selection_revision, flow_id)),
            _ => None,
        })
        .unwrap();
    assert_eq!(first_read.1, first.summary.id);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("table-cell-1:0", cx);
    })
    .unwrap();
    let second_read = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::ReadFlow {
                selection_revision,
                flow_id,
                ..
            } => Some((selection_revision, flow_id)),
            _ => None,
        })
        .unwrap();
    assert_eq!(second_read.1, second.summary.id);
    fixture
        .events
        .try_send(Event::FlowDetail {
            project: fixture.project,
            selection_revision: second_read.0,
            record: Box::new(second.clone()),
            original: Box::new(OriginalMessages {
                request: Some(record("http://127.0.0.1:9001/two", "修改前").request),
                response: None,
                ..OriginalMessages::default()
            }),
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::FlowDetail {
            project: fixture.project,
            selection_revision: first_read.0,
            record: Box::new(first),
            original: Box::default(),
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::ProxyState {
            project: ProjectKey {
                generation: 0,
                ..fixture.project
            },
            address: Some("127.0.0.1:9999".parse().unwrap()),
            cancel: Some(CancellationToken::new()),
            intercept_requests: true,
            intercept_responses: true,
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.detail.as_ref().unwrap().summary.id, second.summary.id);
        assert!(view.proxy_address.is_none());
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("history-request-version", cx);
        window.render_frame(cx);
        let orig_pos = window.find_by_label("原始请求").bounds().center();
        window.pointer_down(orig_pos, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(orig_pos, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        let original_editor = fixture.view.read(cx).history_request.raw.clone();
        assert!(original_editor.read(cx).value().ends_with("修改前"));
        window.click(("input", original_editor.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("不应改写历史记录", cx);
        assert!(original_editor.read(cx).value().ends_with("修改前"));
        window.render_frame(cx);
        window.click("history-request-version", cx);
        window.render_frame(cx);
        let final_pos = window.find_by_label("最终请求").bounds().center();
        window.pointer_down(final_pos, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(final_pos, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        let original_editor = fixture.view.read(cx).history_request.raw.clone();
        assert!(original_editor.read(cx).value().ends_with("二"));
        window.render_frame(cx);
        window.click("history-request-message-actions", cx);
        window.render_frame(cx);
        window.click("history-request-to-repeater", cx);
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).iter().any(|command| matches!(command, Command::OpenRepeater { flow_id, .. } if *flow_id == second.summary.id)));
}

#[gpui_kit::test]
fn history_refresh_keeps_highlight_on_the_selected_flow_not_its_old_index(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let revision = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::QueryHttpHistory {
                query: mio_core::HttpHistoryQuery { query_revision, .. },
            } => Some(query_revision),
            _ => None,
        })
        .unwrap();
    let selected = record("http://127.0.0.1:9001/selected", "已选择");
    let inserted = record("http://127.0.0.1:9001/new", "新记录");
    fixture
        .events
        .try_send(Event::HistoryPage {
            project: fixture.project,
            query_revision: revision,
            offset: 0,
            total: 1,
            rows: vec![selected.summary.clone().into()],
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-history", cx);
        window.render_frame(cx);
        window.click("table-cell-0:0", cx);
    })
    .unwrap();
    let selection_revision = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::ReadFlow {
                selection_revision, ..
            } => Some(selection_revision),
            _ => None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::FlowDetail {
            project: fixture.project,
            selection_revision,
            record: Box::new(selected.clone()),
            original: Box::default(),
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::HistoryPage {
            project: fixture.project,
            query_revision: revision,
            offset: 0,
            total: 2,
            rows: vec![
                inserted.summary.clone().into(),
                selected.summary.clone().into(),
            ],
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.history.read(cx).selected_row(), Some(1));
        assert_eq!(view.selected_flow, Some(selected.summary.id));
        assert_eq!(
            view.detail.as_ref().unwrap().summary.id,
            selected.summary.id
        );
    });
    fixture
        .events
        .try_send(Event::HistoryPage {
            project: fixture.project,
            query_revision: revision,
            offset: 0,
            total: 1,
            rows: vec![inserted.summary.into()],
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.history.read(cx).selected_row().is_none());
        assert!(view.selected_flow.is_none());
        assert!(view.detail.is_none());
    });
}

#[gpui_kit::test]
fn orientation_drag_rename_close_and_reopen_keep_the_same_draft(cx: &mut TestAppContext) {
    let first = tab("1");
    let first_id = first.id;
    let second = tab("2");
    let second_id = second.id;
    let mut fixture = fixture(cx, vec![first, second], 64);
    drain(&mut fixture.commands);
    let editor = cx.read(|cx| {
        fixture.view.read(cx).tab_editors[&first_id]
            .request
            .raw
            .clone()
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("end"), cx);
        window.input("保留草稿", cx);
    })
    .unwrap();
    choose_tab_orientation(&fixture, Orientation::Vertical, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        assert_eq!(
            fixture.view.read(cx).tab_editors[&first_id]
                .request
                .raw
                .entity_id(),
            editor.entity_id()
        );
        window.drag_to(
            SharedString::from(format!("work-tab-{second_id}")),
            SharedString::from(format!("work-tab-{first_id}")),
            cx,
        );
        assert_eq!(fixture.view.read(cx).workspace.tabs[0].id, second_id);
        window.right_click(SharedString::from(format!("work-tab-{first_id}")), cx);
    })
    .unwrap();
    // The native menu is constructed in the deferred pointer callback.
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let rename = window.find_by_label("重命名标签");
        let popup = window.find("popup-menu");
        window.click_at(
            "popup-menu",
            rename.bounds().center() - popup.bounds().origin,
            cx,
        );
        let rename_input = fixture.view.read(cx).rename.as_ref().unwrap().input.clone();
        window.click(("input", rename_input.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("中文标签", cx);
        window.click("rename-apply", cx);
        window.click(SharedString::from(format!("close-tab-{first_id}")), cx);
        assert!(
            fixture
                .view
                .read(cx)
                .workspace
                .tabs
                .iter()
                .all(|tab| tab.id != first_id)
        );
        window.press(crate::kbd!("shift-t"), cx);
        let restored = fixture
            .view
            .read(cx)
            .workspace
            .tabs
            .iter()
            .find(|tab| tab.id == first_id)
            .unwrap();
        assert_eq!(restored.title, "中文标签");
        assert!(restored.draft.raw.ends_with("保留草稿".as_bytes()));
        assert_eq!(
            fixture.view.read(cx).workspace.orientation,
            Orientation::Vertical
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn closing_the_final_tab_keeps_restore_and_project_shortcuts_routed(cx: &mut TestAppContext) {
    let original = tab("最后一个标签");
    let id = original.id;
    let fixture = fixture(cx, vec![original.clone()], 64);
    let editor = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click(("input", editor.entity_id()), cx);
        window.click(SharedString::from(format!("close-tab-{id}")), cx);
        assert!(fixture.view.read(cx).workspace.tabs.is_empty());
        assert!(window.is_action_available(&RestoreTab, cx));
        window.press(crate::kbd!("shift-t"), cx);
        assert_eq!(fixture.view.read(cx).workspace.tabs, vec![original]);
        assert_eq!(fixture.view.read(cx).workspace.active, Some(id));
        window.press(crate::kbd!("s"), cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
}

#[gpui_kit::test]
fn full_command_queue_preserves_latest_draft_and_allows_retry(cx: &mut TestAppContext) {
    let first = tab("队列");
    let id = first.id;
    let mut fixture = fixture(cx, vec![first], 1);
    let editor = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("end"), cx);
        window.input("待保存稿", cx);
        window.click("repeater-send", cx);
        let view = fixture.view.read(cx);
        assert!(view.workspace_dirty);
        assert!(view.sending.is_empty());
        assert!(view.error.as_deref().unwrap().contains("队列已满"));
        assert!(
            view.workspace.tabs[0]
                .draft
                .raw
                .ends_with("待保存稿".as_bytes())
        );
    })
    .unwrap();
    assert!(matches!(
        fixture.commands.try_recv().unwrap(),
        Command::QueryHttpHistory { .. }
    ));
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-send", cx)
    })
    .unwrap();
    let saved = match fixture.commands.try_recv().unwrap() {
        Command::SaveWorkspace { workspace, .. } => workspace,
        _ => panic!("expected retry to persist current draft"),
    };
    assert!(saved.tabs[0].draft.raw.ends_with("待保存稿".as_bytes()));
    cx.read(|cx| {
        assert!(
            fixture.view.read(cx).sending.is_empty(),
            "full queue never pretends to send"
        )
    });
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: saved.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    match fixture.commands.try_recv().unwrap() {
        Command::Send { tab_id, draft, .. } => {
            assert_eq!(tab_id, id);
            assert_eq!(draft, saved.tabs[0].draft);
        }
        _ => panic!("expected Send after capacity becomes available"),
    }
}

#[gpui_kit::test]
fn filtered_empty_history_never_discards_unsaved_traffic_without_prompt(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let initial_revision = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::QueryHttpHistory {
                query: mio_core::HttpHistoryQuery { query_revision, .. },
            } => Some(query_revision),
            _ => None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::HistoryPage {
            project: fixture.project,
            query_revision: initial_revision,
            offset: 0,
            total: 0,
            rows: vec![],
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::HistoryChanged {
            project: fixture.project,
        })
        .unwrap();
    cx.run_until_parked();
    let filtered_revision = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::QueryHttpHistory {
                query: mio_core::HttpHistoryQuery { query_revision, .. },
            } => Some(query_revision),
            _ => None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::HistoryPage {
            project: fixture.project,
            query_revision: filtered_revision,
            offset: 0,
            total: 0,
            rows: vec![],
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.press(crate::kbd!("n"), cx);
    })
    .unwrap();
    assert!(
        cx.has_pending_prompt(),
        "a filtered zero-row result is not an empty project"
    );
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "do not transition before explicit save/discard choice"
    );
    cx.simulate_prompt_answer("取消");
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
}
