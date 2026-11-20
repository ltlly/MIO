use super::*;

#[gpui_kit::test]
fn selection_only_change_while_saving_invalidates_a_selection_sensitive_commit(
    cx: &mut TestAppContext,
) {
    let tab = tab("1");
    let mut fixture = fixture(cx, vec![tab.clone()], 128);
    let info = install_editor(&fixture, &tab, cx);
    drain(&mut fixture.commands);
    input_plugin(&fixture, &raw("selection-sensitive"), cx);
    let (operation, draft, editor) = send_click(&mut fixture, cx);
    let saved = accept_commit(&mut fixture, &info, operation, &draft, &editor, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("plugin-node", 101u64), cx);
        window.press(crate::kbd!("a"), cx);
    })
    .unwrap();
    acknowledge(&fixture, saved, cx);
    assert!(
        !drain(&mut fixture.commands).iter().any(|command| matches!(
            command,
            Command::Send { .. }
                | Command::Plugin {
                    action: PluginAction::OpenRequest { .. },
                    ..
                }
        )),
        "editor selection is part of the generic Commit contract; matching bytes alone cannot authorize the old result"
    );
}

#[cfg(target_os = "macos")]
#[gpui_kit::test]
fn native_plugin_quit_cancel_and_deadline_preserve_edits(cx: &mut TestAppContext) {
    use crate::native_terminate::tests::Probe;
    use objc2_app_kit::NSApplicationTerminateReply as Reply;

    let tab = tab("原生插件退出确认");
    let mut fixture = fixture(cx, vec![tab.clone()], 64);
    install_editor(&fixture, &tab, cx);
    input_plugin(&fixture, &raw("原生退出期间保留的插件编辑"), cx);
    drain(&mut fixture.commands);
    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    cx.simulate_prompt_answer("返回保留内容");
    cx.run_until_parked();
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [false]);
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    probe.expire();
    probe.pump_modal_until_replies(2);
    cx.simulate_prompt_answer("放弃插件编辑并继续退出");
    cx.run_until_parked();
    assert_eq!(probe.replies(), [false, false]);
    assert!(!cx.has_pending_prompt());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.quitting);
        assert!(!view.shutdown.is_cancelled());
        assert!(!view.extensions.allow_quit);
        assert!(
            view.extensions.instances[0]
                .panel
                .read(cx)
                .has_unsaved_edits(cx)
        );
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn repeated_quit_with_plugin_edits_uses_one_confirmation(cx: &mut TestAppContext) {
    let tab = tab("插件退出确认");
    let mut fixture = fixture(cx, vec![tab.clone()], 64);
    install_editor(&fixture, &tab, cx);
    input_plugin(&fixture, &raw("尚未回写的编辑🙂"), cx);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(Quit), cx);
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("返回保留内容");
    pump_native(cx);
    assert!(!cx.has_pending_prompt());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.transition.is_none());
        assert!(!view.shutdown.is_cancelled());
        assert!(!view.extensions.allow_quit);
        assert!(
            view.extensions.instances[0]
                .panel
                .read(cx)
                .has_unsaved_edits(cx)
        );
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn joined_runtime_invalidates_plugin_quit_confirmation(cx: &mut TestAppContext) {
    let tab = tab("保留插件的最新文本");
    let mut fixture = fixture(cx, vec![tab.clone()], 64);
    install_editor(&fixture, &tab, cx);
    input_plugin(&fixture, &raw("后台回收后仍可复制"), cx);
    drain(&mut fixture.commands);
    let (finished, joined) = tokio::sync::oneshot::channel::<anyhow::Result<()>>();
    cx.update(|cx| {
        let joined = cx.background_spawn(async move { joined.await? });
        Workbench::observe_runtime(&fixture.view, fixture.window, joined, cx);
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(cx.has_pending_prompt());
    finished
        .send(Err(anyhow::anyhow!("确认期间的回收错误")))
        .unwrap();
    pump_native(cx);
    cx.simulate_prompt_answer("放弃插件编辑并继续退出");
    pump_native(cx);
    assert!(!cx.has_pending_prompt());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.transition.is_none());
        assert!(!view.extensions.allow_quit);
        assert!(!view.quitting);
        assert!(
            view.error
                .as_deref()
                .unwrap()
                .contains("确认期间的回收错误")
        );
        assert!(
            view.extensions.instances[0]
                .panel
                .read(cx)
                .has_unsaved_edits(cx)
        );
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn inspect_never_loads_and_native_trust_and_each_grant_are_explicit(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("1")], 64);
    drain(&mut fixture.commands);
    let directory = cx.read(|cx| fixture.view.read(cx).extensions.directory.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-extensions", cx);
        window.click("extensions-tab-settings", cx);
        window.render_frame(cx);
        window.click(("input", directory.entity_id()), cx);
        window.input("/tmp/mio-ui-generic-fixture", cx);
        window.click("plugin-inspect", cx);
    })
    .unwrap();
    let commands = drain(&mut fixture.commands);
    assert!(!commands.iter().any(|command| matches!(
        command,
        Command::Plugin {
            action: PluginAction::Load { .. },
            ..
        }
    )));
    let operation = commands
        .into_iter()
        .find_map(|command| match command {
            Command::Plugin {
                operation_id,
                action: PluginAction::Inspect { .. },
                ..
            } => Some(operation_id),
            _ => None,
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: operation,
            result: Ok(PluginOutcome::Inspected(PluginInspection {
                id: Uuid::new_v4(),
                directory: "/tmp/mio-ui-generic-fixture".into(),
                executable: "/tmp/mio-ui-generic-fixture/worker".into(),
                manifest: manifest(),
            })),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("plugin-load").disabled(), Some(true));
        window.click("plugin-load", cx);
        window.click("plugin-native-trust", cx);
        window.click(("plugin-grant", Permission::TrafficRead as usize), cx);
        window.click("plugin-load", cx);
    })
    .unwrap();
    let loads: Vec<_> = drain(&mut fixture.commands)
        .into_iter()
        .filter_map(|command| match command {
            Command::Plugin {
                action:
                    PluginAction::Load {
                        native_trusted,
                        grants,
                        ..
                    },
                ..
            } => Some((native_trusted, grants)),
            _ => None,
        })
        .collect();
    assert_eq!(loads, [(true, vec![Permission::TrafficRead])]);
}

#[gpui_kit::test]
fn custom_send_commits_latest_then_saves_acks_sends_and_rebinds_for_next_send(
    cx: &mut TestAppContext,
) {
    let tab = tab("1");
    let mut fixture = fixture(cx, vec![tab.clone()], 128);
    let info = install_editor(&fixture, &tab, cx);
    drain(&mut fixture.commands);
    let latest = raw("最新中文🙂");
    input_plugin(&fixture, &latest, cx);
    let (operation, draft, editor) = send_click(&mut fixture, cx);
    assert_eq!(editor.contents, latest.as_bytes());
    let saved = accept_commit(&mut fixture, &info, operation, &draft, &editor, cx);
    assert_eq!(saved.tabs[0].draft.raw, latest.as_bytes());
    acknowledge(&fixture, saved.clone(), cx);
    let commands = drain(&mut fixture.commands);
    assert!(commands.iter().any(|command| matches!(command, Command::Send { tab_id, draft, .. } if *tab_id == tab.id && *draft == saved.tabs[0].draft)));
    let reopen = commands
        .iter()
        .find_map(|command| match command {
            Command::Plugin {
                operation_id,
                action:
                    PluginAction::OpenRequest {
                        tab_id,
                        draft_revision,
                        ..
                    },
                ..
            } => {
                assert_eq!(
                    (*tab_id, *draft_revision),
                    (tab.id, saved.tabs[0].draft.revision)
                );
                Some(*operation_id)
            }
            _ => None,
        })
        .unwrap();
    cx.read(|cx| {
        assert_eq!(
            fixture
                .view
                .read(cx)
                .extensions
                .instance(info.identity.instance_id)
                .unwrap()
                .bound
                .as_ref(),
            Some(&tab.draft),
            "UI must not pretend the broker was rebound before ACK"
        )
    });
    fixture
        .events
        .try_send(Event::PluginOperation {
            project: fixture.project,
            operation_id: reopen,
            result: Ok(PluginOutcome::Opened {
                identity: info.identity,
                tab_id: tab.id,
                draft_revision: saved.tabs[0].draft.revision,
            }),
        })
        .unwrap();
    cx.run_until_parked();
    fixture.view.update(cx, |view, _| {
        view.sending.remove(&tab.id);
    });
    let (_, next_draft, _) = send_click(&mut fixture, cx);
    assert_eq!(next_draft.revision, saved.tabs[0].draft.revision);
}
