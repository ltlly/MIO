use super::*;

#[gpui_kit::test]
fn plugin_send_waits_for_its_checkbox_setting_ack_before_send_and_rebind(cx: &mut TestAppContext) {
    use crate::workbench::tests::settings;

    let mut tab = tab("插件设置快照");
    let request = parse_request(&tab.draft.raw, &tab.draft.target, tab.draft.version).unwrap();
    tab.draft.raw = request_to_raw(&request);
    tab.draft.target = request.url;
    let mut fixture = fixture(cx, vec![tab.clone()], 128);
    let info = install_editor(&fixture, &tab, cx);
    drain(&mut fixture.commands);
    let settings_window = settings::open_settings(&fixture, cx);
    settings::toggle(settings_window, false, cx);
    let old = settings::saved(&mut fixture.commands);
    settings::toggle(settings_window, true, cx);
    let (operation, draft, editor) = send_click(&mut fixture, cx);
    settings::toggle(settings_window, false, cx);
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
    assert!(drain(&mut fixture.commands).is_empty());
    acknowledge(&fixture, old, cx);
    let commands = drain(&mut fixture.commands);
    assert!(
        !commands.iter().any(|command| matches!(
            command,
            Command::Send { .. }
                | Command::Plugin {
                    action: PluginAction::OpenRequest { .. },
                    ..
                }
        )),
        "old ACK must keep the plugin Commit guard until this Send's settings are acknowledged"
    );
    let latest = commands
        .into_iter()
        .find_map(|command| match command {
            Command::SaveWorkspace { workspace, .. } => Some(workspace),
            _ => None,
        })
        .unwrap();
    assert!(!latest.repeater_settings.unpack_compressed_responses);
    assert_eq!(latest.tabs[0].draft, tab.draft);
    acknowledge(&fixture, latest, cx);
    let commands = drain(&mut fixture.commands);
    let captured = commands
        .iter()
        .find_map(|command| match command {
            Command::Send { settings, .. } => Some(settings),
            _ => None,
        })
        .expect("plugin Send carries its activation settings");
    assert!(captured.unpack_compressed_responses);
    settings::toggle(settings_window, true, cx);
    assert!(captured.unpack_compressed_responses);
    assert_eq!(
        commands
            .iter()
            .filter(|command| matches!(command, Command::Send { tab_id, .. } if *tab_id == tab.id))
            .count(),
        1
    );
    assert_eq!(commands.iter().filter(|command| matches!(command, Command::Plugin { action: PluginAction::OpenRequest { tab_id, .. }, .. } if *tab_id == tab.id)).count(), 1);
}

fn ordinary_settings_save_as_abandons_plugin_commit(
    cx: &mut TestAppContext,
    picker_disconnect: bool,
) {
    use crate::workbench::tests::settings;

    let tab = tab("保留插件草稿与设置");
    let mut fixture = fixture(cx, vec![tab.clone()], 128);
    fixture
        .events
        .try_send(Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("settings-saveas-source.mio"),
            total_bytes: 4096,
        })
        .unwrap();
    cx.run_until_parked();
    let info = install_editor(&fixture, &tab, cx);
    drain(&mut fixture.commands);
    let latest = raw("取消另存为后仍保留的插件编辑🙂");
    input_plugin(&fixture, &latest, cx);
    let settings_window = settings::open_settings(&fixture, cx);
    settings::toggle(settings_window, false, cx);
    let old = settings::saved(&mut fixture.commands);
    settings::toggle(settings_window, true, cx);
    let (operation, draft, editor) = send_click(&mut fixture, cx);
    assert_eq!(editor.contents, latest.as_bytes());
    let request = parse_request(&editor.contents, &draft.target, draft.version).unwrap();
    let committed = Draft {
        revision: draft.revision + 1,
        target: request.url.clone(),
        raw: request_to_raw(&request),
        version: request.version,
    };
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: operation,
            result: Ok(PluginOutcome::Committed {
                identity: info.identity,
                tab_id: tab.id,
                draft_revision: draft.revision,
                request,
            }),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    settings::save_as_ack_while_choosing(&mut fixture, settings_window, old, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let guard = &view.extensions.saved_commits[&tab.id];
        assert_eq!(guard.instance, info.identity.instance_id);
        assert_eq!(guard.draft, committed);
        assert_eq!(guard.editor, editor);
        assert!(view.plugin_waiting(tab.id));
    });
    if picker_disconnect {
        settings::disconnect_save_picker(cx);
        cx.read(|cx| {
            assert!(
                fixture
                    .view
                    .read(cx)
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("文件选择器已意外关闭")
            );
        });
    } else {
        cx.simulate_new_path_selection(|_| None);
        cx.run_until_parked();
    }
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            view.extensions.saved_commits.is_empty(),
            "abandoning ordinary Save As must remove the real plugin Commit guard as well as Send"
        );
        assert!(!view.plugin_waiting(tab.id));
        let instance = view.extensions.instance(info.identity.instance_id).unwrap();
        assert!(instance.live);
        assert_eq!(instance.bound.as_ref(), Some(&tab.draft));
        assert!(instance.panel.read(cx).has_unsaved_edits(cx));
    });
    settings::abandoned_settings_send_stays_unsent(&mut fixture, settings_window, &committed, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        let view = fixture.view.read(cx);
        assert!(view.extensions.saved_commits.is_empty());
        assert!(!view.plugin_waiting(tab.id));
        let panel = view
            .extensions
            .instance(info.identity.instance_id)
            .unwrap()
            .panel
            .clone();
        assert!(panel.read(cx).has_unsaved_edits(cx));
        assert_eq!(
            panel
                .update(cx, |panel, cx| panel.capture_editor(401, window, cx))
                .unwrap(),
            editor,
            "a later unrelated Save cannot mark, rebind, or replace the abandoned plugin editor"
        );
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    let edited_again = raw("取消后仍可继续编辑🙂");
    input_plugin(&fixture, &edited_again, cx);
    cx.run_until_parked();
    let notifications: Vec<_> = drain(&mut fixture.commands)
        .into_iter()
        .map(|command| match command {
            Command::Plugin {
                project,
                action:
                    PluginAction::Ui {
                        instance_id,
                        panel_revision,
                        event,
                    },
                ..
            } => {
                assert_eq!(project, fixture.project);
                assert_eq!(instance_id, info.identity.instance_id);
                assert_eq!(panel_revision, 1);
                assert_eq!(event.node_id, 101);
                assert!(event.editors.is_empty());
                // Native editor roots forward key-down events as well as focus and edits.
                assert!(
                    matches!(
                        event.event,
                        Some(
                            ui_event::Event::Focus(_)
                                | ui_event::Event::EditorChanged(_)
                                | ui_event::Event::Key(_)
                        )
                    ),
                    "unexpected native editor notification: {event:?}"
                );
                event.event.unwrap()
            }
            _ => panic!(
                "editing retained text may only notify its plugin, never Send, rebind, or save"
            ),
        })
        .collect();
    assert!(notifications.iter().any(|event| matches!(event,
        ui_event::Event::EditorChanged(changed)
            if changed.document_id == 401 && changed.revision > editor.revision
                && changed.contents == edited_again.as_bytes())));
    cx.update_window(fixture.window, |_, window, cx| {
        let panel = fixture
            .view
            .read(cx)
            .extensions
            .instance(info.identity.instance_id)
            .unwrap()
            .panel
            .clone();
        assert!(panel.read(cx).has_unsaved_edits(cx));
        assert_eq!(
            panel
                .update(cx, |panel, cx| panel.capture_editor(401, window, cx))
                .unwrap()
                .contents,
            edited_again.as_bytes()
        );
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn ordinary_settings_save_as_cancel_clears_plugin_commit_guard(cx: &mut TestAppContext) {
    ordinary_settings_save_as_abandons_plugin_commit(cx, false);
}

#[gpui_kit::test]
fn ordinary_settings_save_as_picker_disconnect_clears_plugin_commit_guard(cx: &mut TestAppContext) {
    ordinary_settings_save_as_abandons_plugin_commit(cx, true);
}

#[gpui_kit::test]
fn settings_save_as_abandonment_preserves_admitted_send_and_other_plugin_rebind(
    cx: &mut TestAppContext,
) {
    use crate::workbench::tests::settings;

    let first = tab("已准入的插件请求");
    let second = tab("仅取消尚未发送的请求");
    let mut fixture = fixture(cx, vec![first.clone(), second.clone()], 128);
    let first_info = install_editor(&fixture, &first, cx);
    drain(&mut fixture.commands);
    input_plugin(&fixture, &raw("已经准入"), cx);
    let (operation, draft, editor) = send_click(&mut fixture, cx);
    let saved = accept_commit(&mut fixture, &first_info, operation, &draft, &editor, cx);
    let admitted_draft = saved.tabs[0].draft.clone();
    acknowledge(&fixture, saved, cx);
    let commands = drain(&mut fixture.commands);
    let (run_id, cancel) = commands
        .iter()
        .find_map(|command| match command {
            Command::Send {
                tab_id,
                run_id,
                cancel,
                ..
            } if *tab_id == first.id => Some((*run_id, cancel.clone())),
            _ => None,
        })
        .expect("the first plugin request was admitted before Save As");
    let rebind = commands
        .iter()
        .find_map(|command| match command {
            Command::Plugin {
                operation_id,
                action: PluginAction::OpenRequest { tab_id, .. },
                ..
            } if *tab_id == first.id => Some(*operation_id),
            _ => None,
        })
        .expect("the admitted plugin's real rebind is still in flight");
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(SharedString::from(format!("work-tab-{}", second.id)), cx);
    })
    .unwrap();
    let selected = settings::saved(&mut fixture.commands);
    acknowledge(&fixture, selected, cx);
    let second_info = install_editor(&fixture, &second, cx);
    drain(&mut fixture.commands);
    input_plugin(&fixture, &raw("保留未发送的插件内容🙂"), cx);
    let settings_window = settings::open_settings(&fixture, cx);
    settings::toggle(settings_window, false, cx);
    let old = settings::saved(&mut fixture.commands);
    settings::toggle(settings_window, true, cx);
    let (operation, draft, editor) = send_click(&mut fixture, cx);
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: operation,
            result: Ok(PluginOutcome::Committed {
                identity: second_info.identity,
                tab_id: second.id,
                draft_revision: draft.revision,
                request: parse_request(&editor.contents, &draft.target, draft.version).unwrap(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    settings::menu_action(settings_window, "项目", "另存为…", cx);
    assert!(cx.did_prompt_for_new_path());
    acknowledge(&fixture, old, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.choosing_save_as && view.workspace_dirty && view.pending_workspace.is_none());
        assert_eq!(view.pending_sends.len(), 1);
        assert_eq!(view.extensions.saved_commits.len(), 1);
        assert!(view.pending_sends.contains_key(&second.id));
        assert!(view.plugin_waiting(first.id) && view.plugin_waiting(second.id));
    });
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        let view = fixture.view.read(cx);
        assert!(view.pending_sends.is_empty(), "only deferred unsent requests are discarded");
        assert!(view.extensions.saved_commits.is_empty());
        assert_eq!(view.sending.len(), 1);
        assert_eq!(view.sending[&first.id].run_id, run_id);
        assert!(!cancel.is_cancelled());
        assert!(matches!(view.extensions.operations.get(&rebind),
            Some(Operation::Rebind { instance, tab_id, draft })
                if *instance == first_info.identity.instance_id && *tab_id == first.id && *draft == admitted_draft));
        assert!(view.plugin_waiting(first.id) && !view.plugin_waiting(second.id));
        assert!(view.workspace_dirty && view.workspace.repeater_settings.unpack_compressed_responses);
        let instance = view.extensions.instance(second_info.identity.instance_id).unwrap();
        assert!(instance.live && instance.panel.read(cx).has_unsaved_edits(cx));
        assert_eq!(instance.bound.as_ref(), Some(&second.draft));
        let panel = instance.panel.clone();
        assert_eq!(panel.update(cx, |panel, cx| panel.capture_editor(401, window, cx)).unwrap(), editor);
    }).unwrap();
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: rebind,
            result: Ok(PluginOutcome::Opened {
                identity: first_info.identity,
                tab_id: first.id,
                draft_revision: admitted_draft.revision,
            }),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.plugin_waiting(first.id));
        let instance = view
            .extensions
            .instance(first_info.identity.instance_id)
            .unwrap();
        assert!(instance.live);
        assert_eq!(instance.bound.as_ref(), Some(&admitted_draft));
        assert_eq!(view.sending[&first.id].run_id, run_id);
        assert!(!cancel.is_cancelled());
    });
}
