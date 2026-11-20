use super::*;

#[gpui_kit::test]
fn edited_history_request_sent_without_later_edits_selects_its_new_result(cx: &mut TestAppContext) {
    let mut f = boot(cx);
    let id = cx.read(|cx| f.view.read(cx).workspace.active.unwrap());
    f.window
        .update(cx, |_, window, cx| {
            let editor = f.view.read(cx).tab_editors[&id].request.raw.clone();
            window.click(("input", editor.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input("GET /edited-and-sent HTTP/1.1\r\n\r\n", cx);
        })
        .unwrap();
    click(&f, "repeater-send", cx);
    let commands = drain(&mut f.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("edited request must save before Send")
    };
    assert!(
        workspace.tabs[0].draft.revision
            > workspace.tabs[0]
                .history
                .as_ref()
                .unwrap()
                .cursor
                .as_ref()
                .unwrap()
                .draft_revision
    );
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace: workspace.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [Command::Send { run_id, draft, .. }] = commands.as_slice() else {
        panic!("ACK must start only the requested Send")
    };
    let sent = (*run_id, draft.clone());
    let record = finish(&f, &sent, cx);
    let commands = drain(&mut f.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!(
            "an un-navigated terminal result must save directly and refresh history without another Select"
        )
    };
    assert_eq!(workspace.tabs[0].draft, sent.1);
    assert_eq!(workspace.tabs[0].last_flow, Some(sent.0));
    assert!(workspace.tabs[0].archive_response.is_none());
    assert_eq!(
        workspace.tabs[0].history.as_ref().unwrap().cursor,
        Some(RepeaterHistoryCursor {
            entry: RepeaterHistoryEntryId::Live(sent.0),
            draft_revision: sent.1.revision,
        })
    );
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.workspace.tabs[0], workspace.tabs[0]);
        assert_eq!(
            view.tab_editors[&id]
                .request
                .bytes_lossless(cx),
            sent.1.raw
        );
        assert_eq!(
            view.tab_editors[&id].response.bytes_lossless(cx),
            response_to_raw(record.response.as_ref().unwrap())
        );
        assert_eq!(
            view.tab_editors[&id].response_revision,
            Some(sent.1.revision)
        );
        assert!(view.sending.is_empty());
    });
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace: workspace.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(query(&mut f).1, 0);
}

#[gpui_kit::test]
fn history_split_menus_use_number_url_order_and_keyboard_select_same_tab(cx: &mut TestAppContext) {
    let mut f = boot(cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            for id in [
                "repeater-history-previous-menu",
                "repeater-history-next-menu",
            ] {
                let bounds = window.find(id).bounds();
                assert_eq!(bounds.size.width, px(17.));
                let text = gpui_kit::base::test_support::texts(window)
                    .into_iter()
                    .find(|text| {
                        text.text() == "历史下拉箭头"
                            && text.visible()
                            && text.is_alternative()
                            && text.path().contains(&gpui_kit::ElementId::from(id))
                    })
                    .expect("both narrow history dropdowns must paint their chevron");
                assert!(
                    text.bounds().origin.x >= bounds.origin.x
                        && text.bounds().right() <= bounds.right()
                );
                assert_eq!(text.bounds().size, gpui_kit::size(px(11.), px(11.)));
            }
        })
        .unwrap();
    click(&f, "repeater-history-previous-menu", cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let nearest = window.find_by_label("2. GET http://127.0.0.1:19081/history/1 [200]");
            let oldest = window.find_by_label("1. GET http://127.0.0.1:19081/history/0 [200]");
            assert!(nearest.bounds().origin.y < oldest.bounds().origin.y);
            assert!(
                window
                    .try_find_by_label("3. GET http://127.0.0.1:19081/history/2 [200]")
                    .is_none()
            );
            window.press("escape", cx);
        })
        .unwrap();
    click(&f, "repeater-history-next-menu", cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .find_by_label("4. GET http://127.0.0.1:19081/history/3 [200]")
                    .visible()
            );
            assert!(
                window
                    .try_find_by_label("3. GET http://127.0.0.1:19081/history/2 [200]")
                    .is_none()
            );
            window.press("down", cx);
            window.press("enter", cx);
        })
        .unwrap();
    cx.run_until_parked();
    let chosen = select(&mut f);
    assert_eq!(chosen.1, RepeaterHistoryEntryId::Archive(3));
    let next = proposal(&f, chosen, cx);
    selected(&f, chosen, next.clone(), cx);
    let commands = drain(&mut f.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("menu selection must save only its selected workspace")
    };
    assert_eq!(workspace.tabs, [next]);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("repeater-history-next").disabled(), Some(true));
            assert_eq!(
                window.find("repeater-history-next-menu").disabled(),
                Some(true)
            );
            assert!(
                !window
                    .find("repeater-history-previous")
                    .disabled()
                    .unwrap_or(false)
            );
        })
        .unwrap();
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace: workspace.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut f.commands).is_empty());
    click(&f, "repeater-history-previous", cx);
    assert_eq!(select(&mut f).1, RepeaterHistoryEntryId::Archive(2));
}

#[gpui_kit::test]
fn selecting_another_tab_saves_before_history_query_with_one_command_slot(cx: &mut TestAppContext) {
    let first = history_tab();
    let second = history_tab();
    let second_id = second.id;
    let mut f = fixture(cx, vec![first, second], 1);
    drain(&mut f.commands);
    click(&f, "tool-repeater", cx);
    let initial = query(&mut f);
    page(&f, initial, 2, 4, cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(format!("work-tab-{second_id}"), cx);
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("automatic history reads must leave the single queue slot for the selection save")
    };
    assert_eq!(workspace.active, Some(second_id));
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.pending_workspace.is_some());
        assert!(!view.repeater_history.querying);
        assert!(view.repeater_history.error.is_none());
    });
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace: workspace.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    let _ = query(&mut f);
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).repeater_history.owner,
            Some((f.project, second_id))
        )
    });
}

#[gpui_kit::test]
fn restoring_a_closed_tab_persists_it_before_querying_its_history(cx: &mut TestAppContext) {
    let saved_tab = history_tab();
    let id = saved_tab.id;
    let mut f = fixture(cx, vec![saved_tab], 1);
    drain(&mut f.commands);
    click(&f, "tool-repeater", cx);
    let initial = query(&mut f);
    page(&f, initial, 2, 4, cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(format!("close-tab-{id}"), cx);
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("closing must save its closed-tab record")
    };
    assert!(workspace.tabs.is_empty());
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
            window.press(crate::kbd!("shift-t"), cx);
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("the restored tab must be persisted before its first history query")
    };
    assert_eq!(workspace.tabs[0].id, id);
    assert!(workspace.closed.is_empty());
    cx.read(|cx| assert!(f.view.read(cx).repeater_history.error.is_none()));
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace: workspace.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    let _ = query(&mut f);
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).repeater_history.owner,
            Some((f.project, id))
        )
    });
}

#[gpui_kit::test]
fn saved_edit_during_send_keeps_new_input_and_displays_that_sends_response(
    cx: &mut TestAppContext,
) {
    let mut f = boot(cx);
    let sent = sending(&mut f, cx);
    let id = cx.read(|cx| f.view.read(cx).workspace.active.unwrap());
    let path = PathBuf::from("/tmp/mio-history-mid-send-edit.mio");
    f.view
        .update(cx, |view, _| view.project_path = Some(path.clone()));
    let newer = "GET /saved-while-sending HTTP/1.1\r\n\r\n";
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let editor = f.view.read(cx).tab_editors[&id].request.raw.clone();
            window.click(("input", editor.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input(newer, cx);
            window.press(crate::kbd!("s"), cx);
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("Cmd-S must save its newer draft while Send remains in flight")
    };
    assert!(workspace.tabs[0].draft.revision > sent.1.revision);
    assert_eq!(workspace.tabs[0].draft.raw, newer.as_bytes());
    f.events
        .try_send(Event::WorkspaceChanged {
            project: f.project,
            workspace: workspace.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(matches!(
        drain(&mut f.commands).as_slice(),
        [Command::SaveProject { path: None, .. }]
    ));
    f.events
        .try_send(Event::ProjectSaved {
            project: f.project,
            path,
            total_bytes: 4096,
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut f.commands).is_empty());
    let record = finish(&f, &sent, cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.workspace.tabs[0].draft.raw, newer.as_bytes());
        assert_eq!(view.tab_editors[&id].request.raw.read(cx).value().as_ref(), newer);
        assert_eq!(view.workspace.tabs[0].last_flow, Some(sent.0), "a newer saved edit is not history navigation and must retain the completed send linkage");
        assert_eq!(
            view.tab_editors[&id].response.bytes_lossless(cx),
            response_to_raw(record.response.as_ref().unwrap())
        );
        assert_eq!(view.tab_editors[&id].response_revision, Some(sent.1.revision));
        assert!(view.sending.is_empty());
    });
}
