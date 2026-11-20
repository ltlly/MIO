use super::*;

#[gpui_kit::test]
fn archive_load_waits_for_ack_preserves_late_input_and_only_appends_an_unsent_tab(
    cx: &mut TestAppContext,
) {
    let old = tab("现有稿");
    let old_id = old.id;
    let mut fixture = fixture(cx, vec![old], 64);
    let editor = cx.read(|cx| {
        fixture.view.read(cx).tab_editors[&old_id]
            .request
            .raw
            .clone()
    });
    click(&fixture, "tool-repeater", cx);
    let latest = "POST /最新 HTTP/1.1\r\nX-A: a\r\nX-A: b\r\n\r\n中文";
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(latest, cx);
    })
    .unwrap();
    let rows = browse(&mut fixture, cx);
    click(&fixture, "ext-open-repeater", cx);
    let before = saved(&mut fixture, cx);
    assert_eq!(before.tabs[0].draft.raw, latest.as_bytes());
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: before.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    let mut commands = drain(&mut fixture.commands);
    assert_eq!(commands.len(), 1);
    let command = commands.pop().unwrap();
    assert!(
        matches!(command, Command::OpenExtRepeater { workspace_revision, message_id, .. }
        if workspace_revision == before.revision && message_id == rows[0].id)
    );
    let later = "POST /稍后 HTTP/1.1\r\n\r\n继续输入";
    cx.update_window(fixture.window, |_, window, cx| {
        editor.update(cx, |editor, cx| editor.set_value(later, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    let tab = archived();
    fixture
        .events
        .try_send(proposal(&command, tab.clone()))
        .unwrap();
    cx.run_until_parked();
    let appended = saved(&mut fixture, cx);
    assert_eq!(appended.tabs.len(), 2);
    assert_eq!(appended.tabs[0].id, old_id);
    assert_eq!(appended.tabs[0].draft.raw, later.as_bytes());
    assert_eq!(appended.tabs[1], tab);
    assert_eq!(appended.active, Some(tab.id));
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: appended.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert!(crate::workbench::tests::acknowledge_history_query(&fixture, commands, cx).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.ext_import.visible);
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert_eq!(view.workspace.tabs, appended.tabs);
        assert!(view.status.contains("未发送"));
    });
}

#[gpui_kit::test]
fn archive_load_ignores_old_selection_and_wrong_envelopes_and_cannot_reappear_after_navigation(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    let rows = browse(&mut fixture, cx);
    let old = request(&mut fixture, cx);
    select(&mut fixture, &rows[1], 1, cx);
    select(&mut fixture, &rows[0], 0, cx);
    fixture.events.try_send(proposal(&old, archived())).unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| assert!(fixture.view.read(cx).workspace.tabs.is_empty()));
    let current = request(&mut fixture, cx);
    for mismatch in 0..6 {
        let mut event = proposal(&current, archived());
        let Event::ExtRepeaterOpened {
            project,
            operation_id,
            source_id,
            item_id,
            message_id,
            workspace_revision,
            ..
        } = &mut event
        else {
            unreachable!()
        };
        match mismatch {
            0 => project.generation += 1,
            1 => *operation_id = Uuid::new_v4(),
            2 => *source_id = Uuid::new_v4(),
            3 => *item_id = Uuid::new_v4(),
            4 => *message_id = Uuid::new_v4(),
            _ => *workspace_revision += 1,
        }
        fixture.events.try_send(event).unwrap();
        cx.run_until_parked();
        assert!(drain(&mut fixture.commands).is_empty());
        cx.read(|cx| assert!(fixture.view.read(cx).workspace.tabs.is_empty()));
    }
    click(&fixture, "ext-messages-report", cx);
    click(&fixture, "ext-browse-messages", cx);
    fixture
        .events
        .try_send(proposal(&current, archived()))
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| assert!(fixture.view.read(cx).workspace.tabs.is_empty()));
    let new = request(&mut fixture, cx);
    fixture.events.try_send(proposal(&new, archived())).unwrap();
    cx.run_until_parked();
    assert_eq!(saved(&mut fixture, cx).tabs.len(), 1);
}

#[gpui_kit::test]
fn archive_load_rejects_changed_bytes_target_and_duplicate_ids_without_partial_workspace_changes(
    cx: &mut TestAppContext,
) {
    let original = tab("保留");
    let mut fixture = fixture(cx, vec![original.clone()], 64);
    browse(&mut fixture, cx);
    for invalid in 0..5 {
        let command = request(&mut fixture, cx);
        let mut tab = archived();
        match invalid {
            0 => tab.draft.raw[0] = b'X',
            1 => tab.draft.target = "http://localhost:1/path".into(),
            2 => tab.id = original.id,
            3 => tab.draft.version = HttpVersion::Http10,
            _ => tab.draft.revision += 1,
        }
        fixture.events.try_send(proposal(&command, tab)).unwrap();
        cx.run_until_parked();
        assert!(drain(&mut fixture.commands).is_empty());
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(
                view.workspace.tabs.as_slice(),
                std::slice::from_ref(&original)
            );
            assert!(view.ext_import.repeater.error.is_some());
            assert!(view.pending_sends.is_empty());
        });
    }
}

#[gpui_kit::test]
fn archive_load_queue_or_save_failure_requires_explicit_retry_and_never_starts_network(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("保存失败仍保留")], 1);
    browse(&mut fixture, cx);
    fixture.view.update(cx, |view, cx| {
        assert!(view.enqueue(
            Command::QueryExtReport {
                project: fixture.project,
                query_revision: 90
            },
            cx
        ));
    });
    click(&fixture, "ext-open-repeater", cx);
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryExtReport {
            query_revision: 90,
            ..
        }]
    ));
    cx.read(|cx| assert!(fixture.view.read(cx).ext_import.repeater.pending.is_none()));
    fixture
        .view
        .update(cx, |view, _| view.workspace_dirty = true);
    click(&fixture, "ext-open-repeater", cx);
    let proposed = saved(&mut fixture, cx);
    fixture
        .events
        .try_send(Event::WorkspaceRejected {
            project: fixture.project,
            revision: proposed.revision,
            stored: Workspace {
                revision: proposed.revision - 1,
                ..proposed.clone()
            },
            message: "synthetic disk rejection".into(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.ext_import.repeater.pending.is_none());
        assert!(view.workspace_dirty && view.pending_sends.is_empty());
    });
    click(&fixture, "ext-open-repeater", cx);
    let retry = saved(&mut fixture, cx);
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: retry,
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert!(matches!(
        commands.as_slice(),
        [Command::OpenExtRepeater { .. }]
    ));
}

#[gpui_kit::test]
fn archive_load_preserves_unconfirmed_native_composition_without_loading_or_saving_a_partial_draft(
    cx: &mut TestAppContext,
) {
    let original = tab("输入法");
    let id = original.id;
    let mut fixture = fixture(cx, vec![original.clone()], 64);
    browse(&mut fixture, cx);
    let editor = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.replace_and_mark_text_in_range(None, "未确认", Some(0..3), window, cx);
        });
    })
    .unwrap();
    click(&fixture, "ext-open-repeater", cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            assert!(editor.marked_text_range(window, cx).is_some())
        });
    })
    .unwrap();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.tabs, [original]);
        assert!(view.ext_import.repeater.pending.is_none());
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("输入法"))
        );
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.press(crate::kbd!("q"), cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(
        cx.has_pending_prompt(),
        "the rejected load must not leave focus on a hidden editor"
    );
    cx.simulate_prompt_answer("取消");
    pump_native(cx);
}

#[gpui_kit::test]
fn archive_load_result_with_late_target_composition_keeps_quit_routed_and_does_not_append(
    cx: &mut TestAppContext,
) {
    let original = tab("目标输入法");
    let id = original.id;
    let mut fixture = fixture(cx, vec![original.clone()], 64);
    browse(&mut fixture, cx);
    let command = request(&mut fixture, cx);
    let target = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].target.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        target.update(cx, |state, cx| {
            state.replace_and_mark_text_in_range(None, "未确认", Some(0..3), window, cx);
        });
    })
    .unwrap();
    fixture
        .events
        .try_send(proposal(&command, archived()))
        .unwrap();
    pump_native(cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.tabs, [original]);
        assert!(view.ext_import.repeater.pending.is_none());
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("输入法"))
        );
    });
    cx.update_window(fixture.window, |_, window, cx| {
        target.update(cx, |state, cx| {
            assert!(state.marked_text_range(window, cx).is_some())
        });
        window.render_frame(cx);
        window.press(crate::kbd!("q"), cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(
        cx.has_pending_prompt(),
        "a late target IME rejection cannot focus a hidden input"
    );
    cx.simulate_prompt_answer("取消");
    pump_native(cx);
}
