use super::*;

#[gpui_kit::test]
fn archive_response_proposal_rejects_wrong_owner_revision_or_request_message(
    cx: &mut TestAppContext,
) {
    let original = tab("原有草稿保持");
    let mut fixture = fixture(cx, vec![original.clone()], 64);
    let selected = browse_history_request(&mut fixture, cx);
    for kind in 0..5 {
        let command = request(&mut fixture, cx);
        let mut reference = mio_core::ExtResponseRef {
            source_id: selected.source_id,
            item_id: selected.item_id,
            message_id: Uuid::new_v4(),
            draft_revision: 1,
        };
        match kind {
            0 => reference.source_id = Uuid::new_v4(),
            1 => reference.item_id = Uuid::new_v4(),
            2 => reference.draft_revision = 0,
            3 => reference.draft_revision = 2,
            4 => reference.message_id = selected.id,
            _ => unreachable!(),
        }
        let mut added = archived();
        added.archive_response = Some(reference);
        fixture.events.try_send(proposal(&command, added)).unwrap();
        cx.run_until_parked();
        assert!(
            drain(&mut fixture.commands).is_empty(),
            "forged archive response must not save, read, or send: variant {kind}"
        );
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(
                view.workspace.tabs.as_slice(),
                std::slice::from_ref(&original)
            );
            assert!(view.pending_workspace.is_none() && !view.workspace_dirty);
            assert!(view.ext_import.repeater.pending.is_none());
            assert!(view.ext_import.repeater.error.is_some());
            assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        });
    }
}

#[gpui_kit::test]
fn archive_response_proposal_saves_bound_reference_without_sending_request(
    cx: &mut TestAppContext,
) {
    let original = tab("保留现有工作区");
    let mut fixture = fixture(cx, vec![original.clone()], 64);
    let selected = browse_history_request(&mut fixture, cx);
    let command = request(&mut fixture, cx);
    let mut added = archived();
    let reference = mio_core::ExtResponseRef {
        source_id: selected.source_id,
        item_id: selected.item_id,
        message_id: Uuid::new_v4(),
        draft_revision: added.draft.revision,
    };
    added.archive_response = Some(reference.clone());
    fixture
        .events
        .try_send(proposal(&command, added.clone()))
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let saves = commands
        .iter()
        .filter_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(saves.len(), 1);
    assert_eq!(saves[0].tabs, [original.clone(), added.clone()]);
    assert_eq!(saves[0].active, Some(added.id));
    assert!(
        commands.iter().all(|command| match command {
            Command::SaveWorkspace { .. } => true,
            Command::ReadExtResponse {
                project,
                reference: actual,
                offset,
                limit,
                ..
            } =>
                *project == fixture.project
                    && actual == &reference
                    && *offset == 0
                    && *limit <= 64 * 1024,
            _ => false,
        }),
        "only workspace persistence and bounded archive reading are allowed"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.tabs, [original, added.clone()]);
        assert_eq!(view.workspace.tabs[1].draft.raw, RAW);
        assert_eq!(view.workspace.tabs[1].last_flow, None);
        assert_eq!(
            view.workspace.tabs[1].archive_response.as_ref(),
            Some(&reference)
        );
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert!(view.ext_import.repeater.error.is_none());
    });
}

#[gpui_kit::test]
fn a_new_project_open_event_revokes_an_inflight_archive_proposal_without_polluting_its_successor(
    cx: &mut TestAppContext,
) {
    let old_tab = tab("旧项目草稿");
    let old_id = old_tab.id;
    let mut fixture = fixture(cx, vec![old_tab], 64);
    browse(&mut fixture, cx);
    let old_command = request(&mut fixture, cx);
    cx.read(|cx| {
        assert!(
            fixture
                .view
                .read(cx)
                .ext_import
                .repeater
                .pending
                .as_ref()
                .unwrap()
                .workspace_revision
                .is_some()
        );
    });

    let mut next_project = fixture.project;
    next_project.id = Uuid::new_v4();
    next_project.generation += 1;
    let next_tab = tab("新项目原有草稿");
    let next_workspace = Workspace {
        revision: 7,
        active: Some(next_tab.id),
        tabs: vec![next_tab.clone()],
        ..Workspace::default()
    };
    // Deliver the real backend event while the old proposal is still in flight.
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: next_project,
            path: None,
            workspace: next_workspace.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(matches!(drain(&mut fixture.commands).as_slice(),
        [Command::QueryHttpHistory { query: mio_core::HttpHistoryQuery { project, .. } }] if *project == next_project));
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(next_project));
        assert_eq!(*view.workspace, next_workspace);
        assert!(view.ext_import.messages.is_none());
        assert!(view.ext_import.report.is_none());
        assert!(view.ext_import.repeater.pending.is_none());
        assert!(view.selected_flow.is_none() && view.selected_intercept.is_none());
        assert!(!view.tab_editors.contains_key(&old_id));
        assert!(view.tab_editors.contains_key(&next_tab.id));
    });

    fixture.project = next_project;
    let next_rows = browse(&mut fixture, cx);
    let next_command = request(&mut fixture, cx);
    let Command::OpenExtRepeater { operation_id, .. } = next_command else {
        unreachable!()
    };
    fixture
        .events
        .try_send(proposal(&old_command, archived()))
        .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "a stale project proposal must neither reopen, save nor send in the new project"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(next_project));
        assert_eq!(*view.workspace, next_workspace);
        assert_eq!(
            view.ext_repeater_selection().unwrap().message,
            next_rows[0].id
        );
        let pending = view.ext_import.repeater.pending.as_ref().unwrap();
        assert_eq!(pending.project, next_project);
        assert_eq!(pending.operation, operation_id);
        assert_eq!(pending.selection.source, next_rows[0].source_id);
        assert_eq!(pending.selection.item, next_rows[0].item_id);
        assert_eq!(pending.selection.message, next_rows[0].id);
        assert!(view.ext_import.repeater.error.is_none());
        assert!(!view.workspace_dirty && view.pending_workspace.is_none());
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
    });
}

#[gpui_kit::test]
fn archive_load_waits_for_the_second_ack_when_native_input_arrives_during_the_first_save(
    cx: &mut TestAppContext,
) {
    let original = tab("两次保存间的输入");
    let id = original.id;
    let mut fixture = fixture(cx, vec![original], 64);
    let editor = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    let first_text = "POST /first HTTP/1.1\r\n\r\n第一稿";
    let latest_text = "POST /latest HTTP/1.1\r\n\r\n稍后提交的中文🙂";
    click(&fixture, "tool-repeater", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", editor.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(first_text, cx);
    })
    .unwrap();
    let rows = browse(&mut fixture, cx);
    click(&fixture, "ext-open-repeater", cx);
    let first = saved(&mut fixture, cx);
    assert_eq!(first.tabs[0].draft.raw, first_text.as_bytes());

    // An already queued native commit can arrive after navigation hides the old editor.
    cx.update_window(fixture.window, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.replace_text_in_range(
                Some(0..first_text.encode_utf16().count()),
                latest_text,
                window,
                cx,
            );
        });
    })
    .unwrap();
    cx.run_until_parked();
    click(&fixture, "ext-open-repeater", cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "neither a second save nor archive read may overtake the first ACK"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.pending_workspace, Some(first.revision));
        assert_eq!(
            view.ext_import
                .repeater
                .pending
                .as_ref()
                .unwrap()
                .workspace_revision,
            None
        );
    });
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: first.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    let second = saved(&mut fixture, cx);
    assert_eq!(second.revision, first.revision + 1);
    assert_eq!(second.tabs[0].id, id);
    assert_eq!(second.tabs[0].draft.raw, latest_text.as_bytes());
    assert_eq!(
        second.tabs[0].draft.revision,
        first.tabs[0].draft.revision + 1
    );
    click(&fixture, "ext-open-repeater", cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.pending_workspace, Some(second.revision));
        assert_eq!(
            view.ext_import
                .repeater
                .pending
                .as_ref()
                .unwrap()
                .workspace_revision,
            None
        );
    });
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: second.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::OpenExtRepeater { project, message_id, workspace_revision, .. }]
            if *project == fixture.project && *message_id == rows[0].id
                && *workspace_revision == second.revision
    ));
    fixture
        .events
        .try_send(Event::WorkspaceChanged {
            project: fixture.project,
            workspace: second.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    click(&fixture, "ext-open-repeater", cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "a duplicate ACK or click must not repeat the accepted archive read"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.pending_workspace.is_none());
        assert_eq!(view.workspace.tabs, second.tabs);
        assert_eq!(editor.read(cx).value().as_ref(), latest_text);
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
    });
}
