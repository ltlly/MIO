use super::*;

#[gpui_kit::test]
fn correlated_panel_event_reaches_the_native_editor_before_outer_send(cx: &mut TestAppContext) {
    let tab = tab("1");
    let mut fixture = fixture(cx, vec![tab.clone()], 128);
    let info = install_editor(&fixture, &tab, cx);
    drain(&mut fixture.commands);
    input_plugin(&fixture, &raw("用户最新输入"), cx);
    cx.run_until_parked();
    let base = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::Plugin {
                action: PluginAction::Ui { event, .. },
                ..
            } => match event.event {
                Some(ui_event::Event::EditorChanged(editor)) => Some(editor),
                _ => None,
            },
            _ => None,
        })
        .expect("native input is sent as a correlated editor snapshot");
    let transformed = raw("关联转换后的最新内容");
    let mut snapshot = panel(transformed.as_bytes().to_vec());
    let model = Arc::make_mut(&mut snapshot);
    model.revision = 2;
    let node::Kind::Editor(editor) = model.root.as_mut().unwrap().kind.as_mut().unwrap() else {
        unreachable!()
    };
    editor.revision = base.revision + 1;
    let revision = editor.revision;
    fixture
        .events
        .try_send(Event::PluginPanel {
            identity: info.identity,
            snapshot,
            editor_bases: vec![base],
        })
        .unwrap();
    cx.run_until_parked();
    let (_, _, editor) = send_click(&mut fixture, cx);
    assert_eq!(editor.contents, transformed.as_bytes());
    assert_eq!(editor.revision, revision);
}

#[gpui_kit::test]
fn input_during_save_ack_cancels_old_custom_send_and_keeps_new_text(cx: &mut TestAppContext) {
    let tab = tab("1");
    let mut fixture = fixture(cx, vec![tab.clone()], 128);
    let info = install_editor(&fixture, &tab, cx);
    drain(&mut fixture.commands);
    input_plugin(&fixture, &raw("first"), cx);
    let (operation, draft, editor) = send_click(&mut fixture, cx);
    let saved = accept_commit(&mut fixture, &info, operation, &draft, &editor, cx);
    let newest = raw("不能丢弃的新输入");
    input_plugin(&fixture, &newest, cx);
    acknowledge(&fixture, saved, cx);
    assert!(!drain(&mut fixture.commands).iter().any(|command| matches!(
        command,
        Command::Send { .. }
            | Command::Plugin {
                action: PluginAction::OpenRequest { .. },
                ..
            }
    )));
    cx.update_window(fixture.window, |_, window, cx| {
        let view = fixture.view.read(cx);
        assert!(!view.plugin_waiting(tab.id));
        let panel = view
            .extensions
            .instance(info.identity.instance_id)
            .unwrap()
            .panel
            .clone();
        assert_eq!(
            panel
                .update(cx, |panel, cx| panel.capture_editor(401, window, cx))
                .unwrap()
                .contents,
            newest.as_bytes()
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn native_target_edited_while_commit_is_pending_is_preserved_as_unsaved(cx: &mut TestAppContext) {
    let tab = tab("1");
    let mut fixture = fixture(cx, vec![tab.clone()], 128);
    let info = install_editor(&fixture, &tab, cx);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("repeater-edit-target", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let dialog = cx.read(|cx| {
        *cx.windows()
            .iter()
            .find(|&&w| w != fixture.window)
            .expect("Repeater target dialog window")
    });
    let (base_target, dialog_target) = cx.read(|cx| {
        let view = fixture.view.read(cx);
        let base_target = view.tab_editors[&tab.id].target.clone();
        let dialog_target = view
            .open_repeater_target_dialog_for_tab(tab.id, cx)
            .expect("Repeater target dialog entity")
            .read(cx)
            .target
            .clone();
        (base_target, dialog_target)
    });
    assert_ne!(dialog_target.entity_id(), base_target.entity_id());
    input_plugin(&fixture, &raw("plugin request"), cx);
    let (operation, draft, editor) = send_click(&mut fixture, cx);
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("repeater-target-ok").disabled(), Some(true));
        window.click("repeater-target-input", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("http://127.0.0.1:9002", cx);
        dialog_target.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx);
            assert!(input.marked_text_range(window, cx).is_some());
        });
    })
    .unwrap();
    cx.run_until_parked();
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: operation,
            result: Ok(PluginOutcome::Committed {
                identity: info.identity,
                tab_id: tab.id,
                draft_revision: draft.revision,
                request: parse_request(&editor.contents, &draft.target, draft.version).unwrap(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(!drain(&mut fixture.commands).iter().any(|command| matches!(
        command,
        Command::SaveWorkspace { .. } | Command::Send { .. }
    )));
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.tabs[0].draft, tab.draft);
        assert!(!view.plugin_waiting(tab.id));
        assert!(
            view.error
                .as_deref()
                .unwrap()
                .contains("插件提交结果已过期或输入仍在变化")
        );
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("plugin-message-native", cx);
    })
    .unwrap();
    cx.update_window(dialog, |_, window, cx| {
        dialog_target.update(cx, |state, cx| {
            state.unmark_text(window, cx);
            state.set_value("http://127.0.0.1:9002", window, cx);
        });
        window.render_frame(cx);
        window.click("repeater-target-ok", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let saved_target = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .expect("confirming target dialog saves the new target");
    assert_eq!(saved_target.tabs[0].draft.target, "http://127.0.0.1:9002");
    acknowledge(&fixture, saved_target, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.tabs[0].draft.target, "http://127.0.0.1:9002");
        assert!(!cx.windows().contains(&dialog));
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-send", cx);
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).iter().any(|command| matches!(
        command,
        Command::Send { draft, .. } if draft.target == "http://127.0.0.1:9002"
    )));
}

#[gpui_kit::test]
fn cancel_then_late_commit_cannot_mutate_or_send_and_save_rejection_clears_wait(
    cx: &mut TestAppContext,
) {
    let tab = tab("1");
    let mut fixture = fixture(cx, vec![tab.clone()], 128);
    let info = install_editor(&fixture, &tab, cx);
    drain(&mut fixture.commands);
    input_plugin(&fixture, &raw("cancelled"), cx);
    let (operation, draft, editor) = send_click(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("repeater-cancel", cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).iter().any(|command| matches!(command, Command::Plugin { action: PluginAction::Cancel { operation_id }, .. } if *operation_id == operation)));
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: operation,
            result: Ok(PluginOutcome::Committed {
                identity: info.identity,
                tab_id: tab.id,
                draft_revision: draft.revision,
                request: parse_request(&editor.contents, &draft.target, draft.version).unwrap(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(!drain(&mut fixture.commands).iter().any(|command| matches!(
        command,
        Command::SaveWorkspace { .. } | Command::Send { .. }
    )));
    cx.read(|cx| assert_eq!(fixture.view.read(cx).workspace.tabs[0].draft, tab.draft));
    let (operation, draft, editor) = send_click(&mut fixture, cx);
    let saved = accept_commit(&mut fixture, &info, operation, &draft, &editor, cx);
    fixture
        .events
        .try_send(Event::WorkspaceRejected {
            project: fixture.project,
            revision: saved.revision,
            stored: Workspace {
                tabs: vec![tab.clone()],
                active: Some(tab.id),
                ..Workspace::default()
            },
            message: "test rejected".into(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.plugin_waiting(tab.id));
        assert!(view.pending_sends.is_empty());
    });
}

#[gpui_kit::test]
fn plugin_workspace_waits_for_existing_local_save_then_proposes_only_its_mutation(
    cx: &mut TestAppContext,
) {
    let tab = tab("1");
    let mut fixture = fixture(cx, vec![tab.clone()], 128);
    let info = install_editor(&fixture, &tab, cx);
    drain(&mut fixture.commands);
    fixture.view.update(cx, |view, _| {
        view.workspace.tabs[0].title = "local rename".into();
        view.workspace_dirty = true;
    });
    let request = PluginWorkspaceRequest {
        identity: info.identity,
        request_id: 23,
        deadline: Instant::now() + Duration::from_secs(5),
        cancel: CancellationToken::new(),
        edit: PluginWorkspaceEdit::UpdateDraft {
            tab_id: tab.id,
            expected_revision: tab.draft.revision,
            request: parse_request(
                raw("plugin body").as_bytes(),
                &tab.draft.target,
                tab.draft.version,
            )
            .unwrap(),
        },
    };
    fixture
        .events
        .try_send(Event::PluginWorkspaceRequest(request))
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert!(!commands.iter().any(|command| matches!(
        command,
        Command::Plugin {
            action: PluginAction::ApplyWorkspace { .. },
            ..
        }
    )));
    let local = commands
        .into_iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .unwrap();
    assert_eq!(local.tabs[0].title, "local rename");
    assert_eq!(local.tabs[0].draft, tab.draft);
    acknowledge(&fixture, local.clone(), cx);
    let proposal = drain(&mut fixture.commands)
        .into_iter()
        .find_map(|command| match command {
            Command::Plugin {
                action: PluginAction::ApplyWorkspace { workspace, .. },
                ..
            } => Some(workspace),
            _ => None,
        })
        .unwrap();
    let mut expected = local;
    expected.revision += 1;
    expected.tabs[0].draft = proposal.tabs[0].draft.clone();
    assert_eq!(
        proposal, expected,
        "no unrelated state may ride with plugin CAS"
    );
    assert_eq!(proposal.tabs[0].draft.revision, tab.draft.revision + 1);
    fixture.view.update(cx, |view, _| {
        view.workspace.tabs[0].title = "renamed while validating".into();
        view.workspace_dirty = true;
    });
    fixture
        .events
        .try_send(Event::WorkspaceRejected {
            project: fixture.project,
            revision: proposal.revision,
            stored: Workspace {
                revision: proposal.revision - 1,
                tabs: vec![WorkTab {
                    title: "local rename".into(),
                    ..tab.clone()
                }],
                active: Some(tab.id),
                ..Workspace::default()
            },
            message: "cancelled before persist".into(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.workspace.tabs[0].draft, tab.draft,
            "a rejected plugin proposal must not become a local user draft"
        );
        assert_eq!(
            view.workspace.tabs[0].title, "renamed while validating",
            "rolling back plugin draft bytes cannot discard an unrelated user rename"
        );
    });
}

#[gpui_kit::test]
fn two_tabs_keep_distinct_workers_and_close_then_restore_does_not_revive_old_worker(
    cx: &mut TestAppContext,
) {
    let first = tab("1");
    let second = tab("2");
    let mut fixture = fixture(cx, vec![first.clone(), second.clone()], 128);
    let one = install_editor(&fixture, &first, cx);
    let two = install_editor(&fixture, &second, cx);
    assert_ne!(one.identity.instance_id, two.identity.instance_id);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(SharedString::from(format!("work-tab-{}", first.id)), cx);
        window.click(SharedString::from(format!("close-tab-{}", first.id)), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).iter().any(|command| matches!(command, Command::Plugin { action: PluginAction::Disable { instance_id }, .. } if *instance_id == one.identity.instance_id)));
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            !view
                .extensions
                .instance(one.identity.instance_id)
                .unwrap()
                .live
        );
        assert!(
            view.extensions
                .instance(two.identity.instance_id)
                .unwrap()
                .live
        );
        assert!(!view.extensions.active_messages.contains_key(&first.id));
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.press(crate::kbd!("shift-t"), cx)
    })
    .unwrap();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.workspace.tabs.iter().any(|tab| tab.id == first.id));
        assert!(!view.extensions.active_messages.contains_key(&first.id));
        assert!(
            !view
                .extensions
                .instance(one.identity.instance_id)
                .unwrap()
                .live
        );
    });
}
