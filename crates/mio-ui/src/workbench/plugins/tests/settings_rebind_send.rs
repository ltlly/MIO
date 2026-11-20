use super::*;

fn message_editor_switch_preserves_settings_action(cx: &mut TestAppContext, to_plugin: bool) {
    use crate::workbench::tests::settings;

    let tab = tab("切换编辑器后的项目操作");
    let mut fixture = fixture(cx, vec![tab.clone()], 128);
    let info = install_editor(&fixture, &tab, cx);
    let plugin_text = raw("保留插件输入🙂");
    let native_text = raw("保留原生输入🙂");
    input_plugin(&fixture, &plugin_text, cx);
    let native = cx.read(|cx| {
        fixture.view.read(cx).tab_editors[&tab.id]
            .request
            .raw
            .clone()
    });
    let plugin_button = SharedString::from(format!("plugin-message-{}", info.identity.instance_id));
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("plugin-message-native", cx);
        window.click(("input", native.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(&native_text, cx);
        if to_plugin {
            window.click(plugin_button, cx);
        } else {
            window.click(plugin_button, cx);
            window.click(("plugin-node", 101u64), cx);
            window.click("plugin-message-native", cx);
        }
        window.render_frame(cx);
        assert!(
            window.is_action_available(&crate::OpenSettings, cx),
            "switching the focused message editor must preserve the installed Settings action"
        );
        assert!(window.is_action_available(&SaveProject, cx));
        assert!(window.is_action_available(&Quit, cx));
    })
    .unwrap();
    let settings = settings::open_settings(&fixture, cx);
    settings::checked(settings, true, cx);
    cx.simulate_keystrokes(settings, crate::kbd!("w"));
    cx.update_window(fixture.window, |_, window, cx| {
        let view = fixture.view.read(cx);
        assert_eq!(native.read(cx).value().as_ref(), native_text);
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
            plugin_text.as_bytes()
        );
    })
    .unwrap();
    assert!(!drain(&mut fixture.commands).iter().any(|command| matches!(
        command,
        Command::Send { .. } | Command::SaveWorkspace { .. } | Command::SaveProject { .. }
    )));
}

#[gpui_kit::test]
fn native_message_switch_preserves_settings_action(cx: &mut TestAppContext) {
    message_editor_switch_preserves_settings_action(cx, false);
}

#[gpui_kit::test]
fn plugin_message_switch_preserves_settings_action(cx: &mut TestAppContext) {
    message_editor_switch_preserves_settings_action(cx, true);
}

#[gpui_kit::test]
fn same_tab_rebind_blocks_native_send_click_and_key_until_ack(cx: &mut TestAppContext) {
    use crate::workbench::tests::record;

    for after_ack_keyboard in [false, true] {
        let tab = tab("同标签 Rebind 的统一 Send 准入");
        let mut fixture = fixture(cx, vec![tab.clone()], 128);
        let info = install_editor(&fixture, &tab, cx);
        drain(&mut fixture.commands);
        input_plugin(&fixture, &raw("先前的插件请求"), cx);
        let (operation, draft, editor) = send_click(&mut fixture, cx);
        let saved = accept_commit(&mut fixture, &info, operation, &draft, &editor, cx);
        let admitted_draft = saved.tabs[0].draft.clone();
        acknowledge(&fixture, saved, cx);
        let commands = drain(&mut fixture.commands);
        // This first send has no earlier live history; the query follows its workspace ACK.
        let commands = crate::workbench::tests::acknowledge_history_query(&fixture, commands, cx);
        let [
            Command::Plugin {
                project: rebind_project,
                operation_id: rebind,
                action:
                    PluginAction::OpenRequest {
                        instance_id,
                        tab_id: rebound_tab,
                        draft_revision,
                    },
            },
            Command::Send {
                project,
                tab_id: sent_tab,
                run_id,
                draft: sent_draft,
                cancel,
                ..
            },
        ] = commands.as_slice()
        else {
            panic!("the accepted plugin Commit must rebind and send after its workspace ACK")
        };
        assert_eq!(
            (*rebind_project, *project),
            (fixture.project, fixture.project)
        );
        assert_eq!(
            (*instance_id, *rebound_tab, *draft_revision),
            (info.identity.instance_id, tab.id, admitted_draft.revision)
        );
        assert_eq!((*sent_tab, sent_draft), (tab.id, &admitted_draft));
        let (rebind, first_run, first_cancel) = (*rebind, *run_id, cancel.clone());
        assert!(!first_cancel.is_cancelled());

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
        terminal.summary.id = first_run;
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
            panic!("SendFinished must save linkage before its history query");
        };
        assert_eq!(*project, fixture.project);
        let linked = linked.clone();
        assert_eq!(linked.tabs[0].last_flow, Some(first_run));
        assert_eq!(linked.tabs[0].draft, admitted_draft);
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
        cx.run_until_parked();
        assert!(drain(&mut fixture.commands).is_empty());

        for workspace_saved in [false, true] {
            if workspace_saved {
                acknowledge(&fixture, linked.clone(), cx);
                let commands = drain(&mut fixture.commands);
                acknowledge_linked_history(&fixture, tab.id, &terminal_summary, commands, cx);
                assert!(
                    drain(&mut fixture.commands).is_empty(),
                    "a workspace ACK must not replay a blocked Send attempt"
                );
            }
            for keyboard in [false, true] {
                cx.update_window(fixture.window, |_, window, cx| {
                    window.render_frame(cx);
                    assert!(window.try_find("repeater-send").is_none());
                    assert_eq!(window.find("repeater-cancel").disabled(), None);
                    window.click(("input", native.entity_id()), cx);
                    assert!(native.focus_handle(cx).is_focused(window));
                    if keyboard {
                        window.press("cmd-enter", cx);
                    } else {
                        assert!(window.try_find("repeater-send").is_none());
                    }
                })
                .unwrap();
                cx.run_until_parked();
                assert!(
                    drain(&mut fixture.commands).is_empty(),
                    "Rebind must block click and native key-down without Send, save or plugin operations"
                );
                cx.read(|cx| {
                    let view = fixture.view.read(cx);
                    assert!(view.sending.is_empty());
                    assert!(
                        view.pending_sends.is_empty(),
                        "a disabled Send must not create a deferred native send during Rebind"
                    );
                    assert_eq!(
                        view.pending_workspace,
                        (!workspace_saved).then_some(linked.revision)
                    );
                    assert!(!view.workspace_dirty);
                    assert!(view.extensions.saved_commits.is_empty());
                    assert!(!view.extensions.active_messages.contains_key(&tab.id));
                    assert!(view.plugin_waiting(tab.id));
                    assert!(matches!(view.extensions.operations.get(&rebind),
                        Some(Operation::Rebind { instance, tab_id, draft })
                            if *instance == info.identity.instance_id
                                && *tab_id == tab.id && *draft == admitted_draft));
                    let instance = view.extensions.instance(info.identity.instance_id).unwrap();
                    assert!(instance.live);
                    assert_eq!(instance.bound.as_ref(), Some(&tab.draft));
                    assert_ne!(instance.bound.as_ref(), Some(&admitted_draft));
                    assert_eq!(
                        view.tab_editors[&tab.id].request.bytes_lossless(cx),
                        admitted_draft.raw
                    );
                    assert!(!first_cancel.is_cancelled());
                });
            }
        }

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
        assert!(
            drain(&mut fixture.commands).is_empty(),
            "Rebind ACK alone must not replay the blocked click or key-down"
        );
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            let view = fixture.view.read(cx);
            assert!(view.pending_sends.is_empty() && view.sending.is_empty());
            assert!(!view.extensions.operations.contains_key(&rebind));
            assert!(!view.plugin_waiting(tab.id));
            let instance = view.extensions.instance(info.identity.instance_id).unwrap();
            assert!(instance.live);
            assert_eq!(instance.bound.as_ref(), Some(&admitted_draft));
            assert!(window.try_find("repeater-cancel").is_none());
            assert_eq!(window.find("repeater-send").disabled(), None);
            window.click(("input", native.entity_id()), cx);
            assert!(native.focus_handle(cx).is_focused(window));
            if after_ack_keyboard {
                window.press("cmd-enter", cx);
            } else {
                window.click("repeater-send", cx);
            }
        })
        .unwrap();
        cx.run_until_parked();
        let commands = drain(&mut fixture.commands);
        let [
            Command::Send {
                project,
                tab_id,
                run_id,
                draft,
                cancel,
                ..
            },
        ] = commands.as_slice()
        else {
            panic!("after both ACKs, a fresh click or native Cmd+Enter must send exactly once")
        };
        assert_eq!((*project, *tab_id), (fixture.project, tab.id));
        assert_eq!(draft, &admitted_draft);
        assert_ne!(*run_id, first_run);
        assert!(!cancel.is_cancelled() && !first_cancel.is_cancelled());
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert!(view.pending_sends.is_empty());
            assert_eq!(view.sending.len(), 1);
            assert_eq!(view.sending[&tab.id].run_id, *run_id);
        });
    }
}
