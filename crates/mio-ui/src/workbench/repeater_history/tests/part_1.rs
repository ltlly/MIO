use super::*;

#[gpui_kit::test]
fn finished_send_returns_from_clean_older_history_to_its_new_live_result(cx: &mut TestAppContext) {
    let mut f = boot(cx);
    let sent = sending(&mut f, cx);
    click(&f, "repeater-history-previous", cx);
    let back = select(&mut f);
    let older = proposal(&f, back, cx);
    selected(&f, back, older, cx);
    let commands = drain(&mut f.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("Back saves its selected cursor")
    };
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace: workspace.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut f.commands).is_empty());
    let record = finish(&f, &sent, cx);
    let automatic = select(&mut f);
    assert_eq!(automatic.1, RepeaterHistoryEntryId::Live(sent.0));
    let mut newest = proposal(&f, automatic, cx);
    newest.draft.raw = sent.1.raw.clone();
    newest.draft.target = sent.1.target.clone();
    newest.last_flow = Some(sent.0);
    newest.archive_response = None;
    selected(&f, automatic, newest.clone(), cx);
    let commands = drain(&mut f.commands);
    let response_read = commands
        .iter()
        .find_map(|command| match command {
            Command::ReadFlow {
                selection_revision,
                flow_id,
                ..
            } if *flow_id == sent.0 => Some(*selection_revision),
            _ => None,
        })
        .expect("new result response is read from its actual flow");
    assert!(commands.iter().all(|command| matches!(
        command,
        Command::ReadFlow { .. }
            | Command::SaveWorkspace { .. }
            | Command::QueryRepeaterHistory { .. }
    )));
    f.events
        .try_send(Event::FlowDetail {
            project: f.project,
            selection_revision: response_read,
            record: Box::new(record.clone()),
            original: Box::default(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.workspace.tabs[0], newest);
        assert_eq!(
            view.tab_editors[&newest.id]
                .request
                .bytes_lossless(cx),
            sent.1.raw
        );
        assert_eq!(
            view.tab_editors[&newest.id]
                .response
                .bytes_lossless(cx),
            response_to_raw(record.response.as_ref().unwrap())
        );
        assert!(view.sending.is_empty());
    });
}

#[gpui_kit::test]
fn finished_send_preserves_newer_text_or_ime_and_does_not_apply_pending_history(
    cx: &mut TestAppContext,
) {
    for composing in [false, true] {
        let mut f = boot(cx);
        let sent = sending(&mut f, cx);
        click(&f, "repeater-history-previous", cx);
        let back = select(&mut f);
        let older = proposal(&f, back, cx);
        selected(&f, back, older, cx);
        let commands = drain(&mut f.commands);
        let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
            panic!("Back must become the selected history before editing it")
        };
        assert!(workspace.tabs[0].draft.revision > sent.1.revision);
        f.events
            .try_send(Event::WorkspaceChanged {
                project: f.project,
                workspace: workspace.clone(),
            })
            .unwrap();
        cx.run_until_parked();
        assert!(drain(&mut f.commands).is_empty());
        click(&f, "repeater-history-previous", cx);
        let pending = select(&mut f);
        assert_eq!(pending.1, RepeaterHistoryEntryId::Archive(0));
        let old_proposal = proposal(&f, pending, cx);
        let id = cx.read(|cx| f.view.read(cx).workspace.active.unwrap());
        f.window
            .update(cx, |_, window, cx| {
                let editor = f.view.read(cx).tab_editors[&id].request.raw.clone();
                window.click(("input", editor.entity_id()), cx);
                if composing {
                    editor.update(cx, |editor, cx| {
                        editor.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
                    });
                } else {
                    window.press(crate::kbd!("a"), cx);
                    window.input("GET /typed-after-send HTTP/1.1\r\n\r\n", cx);
                }
            })
            .unwrap();
        finish(&f, &sent, cx);
        let commands = drain(&mut f.commands);
        assert!(
            matches!(commands.as_slice(), [Command::QueryRepeaterHistory { .. }]),
            "new result is recorded but must not select over editing"
        );
        selected(&f, pending, old_proposal, cx);
        assert!(drain(&mut f.commands).is_empty());
        f.window
            .update(cx, |_, window, cx| {
                let editor = f.view.read(cx).tab_editors[&id].request.raw.clone();
                if composing {
                    assert!(editor.update(cx, |editor, cx| {
                        editor.marked_text_range(window, cx).is_some()
                    }));
                } else {
                    assert_eq!(
                        editor.read(cx).value().as_ref(),
                        "GET /typed-after-send HTTP/1.1\r\n\r\n"
                    );
                }
                let view = f.view.read(cx);
                assert_eq!(
                    view.workspace.tabs[0]
                        .history
                        .as_ref()
                        .unwrap()
                        .cursor
                        .as_ref()
                        .unwrap()
                        .entry,
                    RepeaterHistoryEntryId::Archive(1)
                );
                assert!(
                    view.tab_editors[&id]
                        .result
                        .as_ref()
                        .unwrap()
                        .contains("当前消息保留")
                );
                assert!(view.sending.is_empty());
            })
            .unwrap();
    }
}

#[gpui_kit::test]
fn background_send_updates_its_saved_history_without_stealing_active_navigation_or_focus(
    cx: &mut TestAppContext,
) {
    let first = history_tab();
    let first_id = first.id;
    let mut second = history_tab();
    second.title = "前台继续查看".into();
    let second_id = second.id;
    let mut f = fixture(cx, vec![first.clone(), second.clone()], 64);
    drain(&mut f.commands);
    click(&f, "tool-repeater", cx);
    let initial = query(&mut f);
    page(&f, initial, 2, 4, cx);
    let sent = sending(&mut f, cx);
    click(&f, "repeater-history-previous", cx);
    let back = select(&mut f);
    let older = proposal(&f, back, cx);
    selected(&f, back, older, cx);
    let commands = drain(&mut f.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("A must acknowledge its older historical selection before becoming inactive")
    };
    assert!(workspace.tabs[0].draft.revision > sent.1.revision);
    assert_eq!(
        workspace.tabs[0]
            .history
            .as_ref()
            .unwrap()
            .cursor
            .as_ref()
            .unwrap()
            .entry,
        RepeaterHistoryEntryId::Archive(1)
    );
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace: workspace.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut f.commands).is_empty());
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(format!("work-tab-{second_id}"), cx);
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let switched = commands
        .iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace.clone()),
            _ => None,
        })
        .unwrap();
    assert!(matches!(
        commands.as_slice(),
        [Command::SaveWorkspace { .. }]
    ));
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace: switched,
        })
        .unwrap();
    cx.run_until_parked();
    let active_query = query(&mut f);
    page(&f, active_query, 2, 4, cx);
    assert!(drain(&mut f.commands).is_empty());
    let active_editor = cx.read(|cx| f.view.read(cx).tab_editors[&second_id].request.raw.clone());
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(("input", active_editor.entity_id()), cx);
        })
        .unwrap();
    let mut record = crate::workbench::tests::record(&sent.1.target, "background result");
    record.summary.id = sent.0;
    f.events
        .try_send(Event::SendFinished {
            project: f.project,
            tab_id: first_id,
            draft_revision: sent.1.revision,
            record: Box::new(record),
            persisted: true,
        })
        .unwrap();
    cx.run_until_parked();
    let automatic = select(&mut f);
    assert_eq!(automatic.1, RepeaterHistoryEntryId::Live(sent.0));
    click(&f, "repeater-history-previous", cx);
    assert!(
        drain(&mut f.commands).is_empty(),
        "another tab's navigation waits for the shared workspace revision without cancelling the background selection"
    );
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(("input", active_editor.entity_id()), cx);
        })
        .unwrap();
    let mut restored = first;
    restored.draft.revision = automatic.3 + 1;
    restored.last_flow = Some(sent.0);
    restored.archive_response = None;
    restored.history.as_mut().unwrap().cursor = Some(RepeaterHistoryCursor {
        entry: automatic.1,
        draft_revision: restored.draft.revision,
    });
    selected(&f, automatic, restored.clone(), cx);
    let commands = drain(&mut f.commands);
    let workspace = commands
        .iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace.clone()),
            _ => None,
        })
        .expect("background result selection must be persisted");
    assert_eq!(workspace.active, Some(second_id));
    assert_eq!(workspace.tabs[0], restored);
    assert_eq!(workspace.tabs[1], second);
    f.window
        .update(cx, |_, window, cx| {
            assert!(active_editor.focus_handle(cx).is_focused(window));
            let view = f.view.read(cx);
            assert!(view.tool == Tool::Repeater);
            assert_eq!(view.workspace.active, Some(second_id));
            assert!(view.repeater_history.pending.contains_key(&second_id));
            assert!(!view.repeater_history.pending.contains_key(&first_id));
        })
        .unwrap();
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace,
        })
        .unwrap();
    cx.run_until_parked();
    let next = select(&mut f);
    assert_eq!(next.1, RepeaterHistoryEntryId::Archive(1));
    assert!(next.2 > automatic.2);
}
