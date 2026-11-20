use super::*;

#[gpui_kit::test]
fn ordinary_settings_save_as_cancel_preserves_rebind_after_rejected_native_send(
    cx: &mut TestAppContext,
) {
    use crate::workbench::tests::{record, settings};

    let pending_tab = tab("B：合法等待保存的原生请求");
    let tab = tab("A：Rebind 等待中的原生 Send");
    let mut fixture = fixture(cx, vec![tab.clone(), pending_tab.clone()], 128);
    let info = install_editor(&fixture, &tab, cx);
    drain(&mut fixture.commands);
    input_plugin(&fixture, &raw("先完成的插件请求"), cx);
    let (operation, draft, editor) = send_click(&mut fixture, cx);
    let saved = accept_commit(&mut fixture, &info, operation, &draft, &editor, cx);
    let admitted_draft = saved.tabs[0].draft.clone();
    acknowledge(&fixture, saved, cx);
    let commands = drain(&mut fixture.commands);
    let (run_id, cancel) = commands
        .iter()
        .find_map(|command| match command {
            Command::Send {
                project,
                tab_id,
                run_id,
                draft,
                cancel,
                ..
            } if *project == fixture.project && *tab_id == tab.id && *draft == admitted_draft => {
                Some((*run_id, cancel.clone()))
            }
            _ => None,
        })
        .expect("the plugin Commit and workspace ACK admitted the first Send");
    let rebind = commands
        .iter()
        .find_map(|command| match command {
            Command::Plugin {
                project,
                operation_id,
                action:
                    PluginAction::OpenRequest {
                        instance_id,
                        tab_id,
                        draft_revision,
                    },
            } if *project == fixture.project
                && *instance_id == info.identity.instance_id
                && *tab_id == tab.id
                && *draft_revision == admitted_draft.revision =>
            {
                Some(*operation_id)
            }
            _ => None,
        })
        .expect("the admitted Send has its own real Rebind operation");
    let binding = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.sending[&tab.id].run_id, run_id);
        assert!(view.pending_sends.is_empty() && view.extensions.saved_commits.is_empty());
        assert!(matches!(view.extensions.operations.get(&rebind),
            Some(Operation::Rebind { instance, tab_id, draft })
                if *instance == info.identity.instance_id && *tab_id == tab.id && *draft == admitted_draft));
        let instance = view.extensions.instance(info.identity.instance_id).unwrap();
        assert!(instance.live);
        assert_eq!(instance.bound.as_ref(), Some(&tab.draft));
        assert_ne!(instance.bound.as_ref(), Some(&admitted_draft));
        assert!(!cancel.is_cancelled());
        instance.bound.clone()
    });

    let mut terminal = record(&admitted_draft.target, "");
    terminal.request = parse_request(
        &admitted_draft.raw,
        &admitted_draft.target,
        admitted_draft.version,
    )
    .unwrap();
    let response = parse_response(
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK",
        HttpVersion::Http11,
    )
    .unwrap();
    terminal.summary.id = run_id;
    terminal.summary.revision = 2;
    terminal.summary.source = mio_core::TrafficSource::Repeater;
    terminal.summary.method = terminal.request.method.clone();
    terminal.summary.url = terminal.request.url.clone();
    terminal.summary.state = mio_core::FlowState::Complete;
    terminal.summary.status = Some(response.status);
    terminal.summary.request_bytes = terminal.request.body.len() as u64;
    terminal.summary.response_bytes = response.body.len() as u64;
    terminal.summary.started_unix_ms = 1;
    terminal.summary.elapsed_ms = 1;
    terminal.response = Some(response);
    let terminal_summary = terminal.summary.clone();
    fixture
        .events
        .try_send(Event::SendFinished {
            project: fixture.project,
            tab_id: tab.id,
            draft_revision: admitted_draft.revision,
            record: Box::new(terminal),
            persisted: true,
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveWorkspace {
            project,
            workspace: linked,
        },
    ] = commands.as_slice()
    else {
        panic!("SendFinished must first save linkage without reading unpersisted history");
    };
    assert_eq!(*project, fixture.project);
    let linked = linked.clone();
    assert!(drain(&mut fixture.commands).is_empty());
    assert_eq!(linked.tabs[0].last_flow, Some(run_id));
    acknowledge(&fixture, linked, cx);
    let commands = drain(&mut fixture.commands);
    acknowledge_linked_history(&fixture, tab.id, &terminal_summary, commands, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.sending.is_empty() && view.pending_sends.is_empty());
        assert!(view.plugin_waiting(tab.id));
        assert!(view.extensions.operations.contains_key(&rebind));
        let instance = view.extensions.instance(info.identity.instance_id).unwrap();
        assert_eq!(instance.bound, binding);
        assert!(!cancel.is_cancelled());
    });

    let native = cx.read(|cx| {
        fixture.view.read(cx).tab_editors[&tab.id]
            .request
            .raw
            .clone()
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("plugin-message-native", cx);
        window.click(("input", native.entity_id()), cx);
    })
    .unwrap();
    let settings_window = settings::open_settings(&fixture, cx);
    settings::toggle(settings_window, false, cx);
    let old = settings::saved(&mut fixture.commands);
    settings::toggle(settings_window, true, cx);
    for keyboard in [false, true] {
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            window.click(("input", native.entity_id()), cx);
            assert!(native.focus_handle(cx).is_focused(window));
            assert!(window.try_find("repeater-send").is_none());
            assert_eq!(window.find("repeater-cancel").disabled(), None);
            if keyboard {
                window.press("cmd-enter", cx);
            } else {
                assert!(window.try_find("repeater-send").is_none());
            }
        })
        .unwrap();
        cx.run_until_parked();
        assert!(drain(&mut fixture.commands).is_empty());
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert!(!view.extensions.active_messages.contains_key(&tab.id));
            assert!(view.extensions.saved_commits.is_empty());
            assert!(
                view.pending_sends.is_empty() && view.sending.is_empty(),
                "A's disabled click and native Cmd+Enter must not create a Send intent"
            );
            assert!(view.plugin_waiting(tab.id));
            assert!(matches!(view.extensions.operations.get(&rebind),
                Some(Operation::Rebind { instance, tab_id, draft })
                    if *instance == info.identity.instance_id && *tab_id == tab.id && *draft == admitted_draft));
            let instance = view.extensions.instance(info.identity.instance_id).unwrap();
            assert_eq!(instance.bound, binding);
            assert_eq!(view.workspace.tabs[0].draft, admitted_draft);
            assert_eq!(
                view.tab_editors[&tab.id].request.bytes_lossless(cx),
                admitted_draft.raw
            );
            assert!(!cancel.is_cancelled());
        });
    }

    let pending_raw = raw("B 的最新草稿；放弃另存为后仍保留🙂");
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(
            SharedString::from(format!("work-tab-{}", pending_tab.id)),
            cx,
        );
    })
    .unwrap();
    let pending_native = cx.read(|cx| {
        fixture.view.read(cx).tab_editors[&pending_tab.id]
            .request
            .raw
            .clone()
    });
    let commands = drain(&mut fixture.commands);
    // B is an unsent draft; selecting it reads an empty navigation history.
    assert!(crate::workbench::tests::acknowledge_history_query(&fixture, commands, cx).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", pending_native.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(&pending_raw, cx);
        assert_eq!(window.find("repeater-send").disabled(), None);
        window.click("repeater-send", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    let pending_draft = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.active, Some(pending_tab.id));
        assert_eq!(view.pending_sends.len(), 1);
        assert!(!view.pending_sends.contains_key(&tab.id));
        assert!(view.sending.is_empty() && view.extensions.saved_commits.is_empty());
        assert!(view.plugin_waiting(tab.id) && !view.plugin_waiting(pending_tab.id));
        assert!(
            view.workspace_dirty && view.workspace.repeater_settings.unpack_compressed_responses
        );
        assert_eq!(view.pending_workspace, Some(old.revision));
        let send = &view.pending_sends[&pending_tab.id];
        assert_eq!(send.draft.raw.as_slice(), pending_raw.as_bytes());
        assert_eq!(send.draft.revision, pending_tab.draft.revision + 1);
        assert_eq!(send.draft.target, pending_tab.draft.target);
        assert_eq!(send.draft.version, pending_tab.draft.version);
        assert_eq!(send.settings, view.workspace.repeater_settings);
        assert!(send.required_workspace_revision > old.revision);
        assert_eq!(view.workspace.tabs[1].draft, send.draft);
        send.draft.clone()
    });
    settings::menu_action(settings_window, "项目", "另存为…", cx);
    assert!(cx.did_prompt_for_new_path());
    acknowledge(&fixture, old, cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "the old ACK must not revive rejected A or admit B before its own save revision"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.choosing_save_as && view.workspace_dirty && view.pending_workspace.is_none());
        assert_eq!(view.pending_sends.len(), 1);
        assert!(!view.pending_sends.contains_key(&tab.id));
        assert_eq!(view.pending_sends[&pending_tab.id].draft, pending_draft);
        assert!(view.sending.is_empty() && view.extensions.saved_commits.is_empty());
        assert!(matches!(view.extensions.operations.get(&rebind),
            Some(Operation::Rebind { instance, tab_id, draft })
                if *instance == info.identity.instance_id && *tab_id == tab.id && *draft == admitted_draft));
        let instance = view.extensions.instance(info.identity.instance_id).unwrap();
        assert_eq!(instance.bound, binding);
    });

    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    let abandoned = drain(&mut fixture.commands);
    assert!(!cx.did_prompt_for_new_path());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.choosing_save_as);
        assert!(view.transition.is_none() && view.after_save.is_none());
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert!(view.extensions.saved_commits.is_empty());
        assert!(!cancel.is_cancelled());
        assert!(
            matches!(view.extensions.operations.get(&rebind),
                Some(Operation::Rebind { instance, tab_id, draft })
                    if *instance == info.identity.instance_id && *tab_id == tab.id && *draft == admitted_draft),
            "ordinary Save As cancellation must preserve the unrelated same-tab Rebind operation"
        );
        let instance = view.extensions.instance(info.identity.instance_id).unwrap();
        assert!(instance.live && view.plugin_waiting(tab.id));
        assert_eq!(instance.bound, binding);
        assert!(view.workspace_dirty && view.workspace.repeater_settings.unpack_compressed_responses);
        assert!(view.pending_workspace.is_none() && view.pending_project_save.is_none());
        assert_eq!(view.workspace.tabs[0].draft, admitted_draft);
        assert_eq!(
            view.tab_editors[&tab.id].request.bytes_lossless(cx),
            admitted_draft.raw
        );
        assert_eq!(view.workspace.tabs[1].draft, pending_draft);
        assert_eq!(
            view.tab_editors[&pending_tab.id].request.bytes_lossless(cx),
            pending_draft.raw
        );
    });
    assert!(
        !abandoned.iter().any(|command| matches!(
            command,
            Command::Plugin {
                action: PluginAction::Cancel { .. },
                ..
            }
        )),
        "abandoning the native pending Send must not cancel the plugin's earlier Rebind"
    );
    assert!(
        abandoned.is_empty(),
        "abandonment must not save, send, resume, or rebind"
    );
    settings::checked(settings_window, true, cx);

    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: rebind,
            result: Ok(PluginOutcome::Opened {
                identity: info.identity,
                tab_id: tab.id,
                draft_revision: admitted_draft.revision,
            }),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.extensions.operations.contains_key(&rebind));
        assert!(!view.plugin_waiting(tab.id));
        let instance = view.extensions.instance(info.identity.instance_id).unwrap();
        assert!(instance.live);
        assert_eq!(instance.bound.as_ref(), Some(&admitted_draft));
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert!(
            view.workspace_dirty && view.workspace.repeater_settings.unpack_compressed_responses
        );
        assert!(!cancel.is_cancelled());
    });

    settings::menu_action(settings_window, "项目", "保存项目", cx);
    assert!(cx.did_prompt_for_new_path());
    assert!(drain(&mut fixture.commands).is_empty());
    let path = PathBuf::from("/tmp/mio-ui-send-admission-after-save-as.mio");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveProject {
            websocket_notes: _,
            ext_http_notes: _,
            ext_http_highlights: _,
            project,
            path: Some(destination),
            workspace: Some(saved),
        },
    ] = commands.as_slice()
    else {
        panic!("the later explicit Save must persist both drafts and settings without sending")
    };
    assert_eq!((*project, destination), (fixture.project, &path));
    assert_eq!(saved.tabs[0].draft, admitted_draft);
    assert_eq!(saved.tabs[0].last_flow, Some(run_id));
    assert_eq!(saved.tabs[1].draft, pending_draft);
    assert!(saved.repeater_settings.unpack_compressed_responses);
    acknowledge(&fixture, saved.clone(), cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace.active, Some(pending_tab.id));
        assert!(view.workspace.tabs[1].history.is_none());
        assert!(view.workspace.tabs[1].last_flow.is_none());
    });
    let commands = drain(&mut fixture.commands);
    // B's cancelled Send was never admitted; the post-save query must target B, not live A.
    assert!(
        matches!(commands.as_slice(), [Command::QueryRepeaterHistory { project, tab_id, start: 0, limit: 64, .. }] if *project == fixture.project && *tab_id == pending_tab.id)
    );
    let commands = crate::workbench::tests::acknowledge_history_query(&fixture, commands, cx);
    assert!(
        commands.is_empty(),
        "a later save ACK must neither replay rejected A nor revive cancelled B"
    );
    assert!(drain(&mut fixture.commands).is_empty());
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
    assert!(drain(&mut fixture.commands).is_empty());
    settings::checked(settings_window, true, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.pending_sends.is_empty() && view.sending.is_empty());
        assert!(view.extensions.saved_commits.is_empty());
        assert!(!view.extensions.operations.contains_key(&rebind));
        assert!(!view.plugin_waiting(tab.id));
        let instance = view.extensions.instance(info.identity.instance_id).unwrap();
        assert!(instance.live);
        assert_eq!(instance.bound.as_ref(), Some(&admitted_draft));
        assert!(!cancel.is_cancelled());
        assert!(!view.workspace_dirty && view.pending_workspace.is_none());
        assert!(view.pending_project_save.is_none());
        assert_eq!(view.project_saves_in_flight, 0);
        assert_eq!(view.project_path.as_ref(), Some(&path));
        assert_eq!(view.workspace.tabs[0].draft, admitted_draft);
        assert_eq!(view.workspace.tabs[1].draft, pending_draft);
        assert_eq!(
            view.tab_editors[&tab.id].request.bytes_lossless(cx),
            admitted_draft.raw
        );
        assert_eq!(
            view.tab_editors[&pending_tab.id].request.bytes_lossless(cx),
            pending_draft.raw
        );
        assert!(view.workspace.repeater_settings.unpack_compressed_responses);
    });
}
