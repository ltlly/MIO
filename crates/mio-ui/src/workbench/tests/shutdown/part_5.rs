use super::*;

#[gpui_kit::test]
fn busy_file_picker_blocks_quit_close_and_other_dialogs_until_cancel(cx: &mut TestAppContext) {
    let first = tab("选择器期间保留编辑");
    let id = first.id;
    let mut fixture = fixture(cx, vec![first], 64);
    let raw = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    let latest = "POST /保留 HTTP/1.1\r\nHost: localhost\r\n\r\n未保存输入🙂";
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click(("input", raw.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(latest, cx);
    })
    .unwrap();
    pump_native(cx);
    drain(&mut fixture.commands);
    let (auxiliary, _) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
            cx.new(|_| AuxiliaryWindow)
        })
        .unwrap()
    });
    for menu in ["打开项目…", "导出 Proxy CA…", "另存为…"] {
        project_menu(&fixture, menu, cx);
        let picker = cx.read(|cx| fixture.view.read(cx).file_picker.unwrap());
        let mut native = gpui_kit::VisualTestContext::from_window(fixture.window, cx);
        assert!(!native.simulate_close());
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            window.dispatch_action(Box::new(OpenProject), cx);
            window.dispatch_action(Box::new(ExportCa), cx);
            window.dispatch_action(Box::new(SaveProjectAs), cx);
            window.dispatch_action(Box::new(NewProject), cx);
            window.press(crate::kbd!("q"), cx);
        })
        .unwrap();
        cx.update_window(auxiliary, |_, window, cx| {
            window.dispatch_action(Box::new(Quit), cx)
        })
        .unwrap();
        pump_native(cx);
        assert!(!cx.has_pending_prompt(), "{menu} retains the only dialog");
        assert_eq!(cx.did_prompt_for_paths(), menu == "打开项目…");
        assert_eq!(cx.did_prompt_for_new_path(), menu != "打开项目…");
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(view.file_picker, Some(picker));
            assert!(view.quit_attempt.is_none());
            assert!(!view.shutdown_requested && !view.shutdown.is_cancelled());
            assert_eq!(raw.read(cx).value().as_str(), latest);
        });
        answer_file_picker(menu, None, cx);
        cx.update_window(auxiliary, |_, window, cx| {
            window.dispatch_action(Box::new(Quit), cx)
        })
        .unwrap();
        pump_native(cx);
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("取消");
        pump_native(cx);
        assert!(!cx.has_pending_prompt());
        assert!(drain(&mut fixture.commands).is_empty());
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert!(view.file_picker.is_none());
            assert!(!view.choosing_save_as && !view.quitting);
            assert!(view.quit_attempt.is_none() && view.transition.is_none());
            assert_eq!(raw.read(cx).value().as_str(), latest);
        });
    }
}

#[gpui_kit::test]
fn busy_file_picker_selection_after_native_rejection_runs_only_its_action(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    for menu in ["打开项目…", "导出 Proxy CA…"] {
        project_menu(&fixture, menu, cx);
        let probe = Probe::new();
        assert_eq!(probe.request(), Reply::TerminateLater);
        cx.run_until_parked();
        probe.pump_modal_until_replies(1);
        assert_eq!(probe.replies(), [false]);
        assert!(!cx.has_pending_prompt());
        let selected = std::env::temp_dir().join("mio-picker-synthetic-selection");
        answer_file_picker(menu, Some(selected.clone()), cx);
        let commands = drain(&mut fixture.commands);
        if menu == "打开项目…" {
            assert!(
                matches!(commands.as_slice(), [Command::OpenProject { path }] if *path == selected)
            );
            fixture.project.generation += 1;
            fixture
                .events
                .try_send(Event::ProjectOpened {
                    project: fixture.project,
                    path: None,
                    workspace: Workspace::default(),
                })
                .unwrap();
            cx.run_until_parked();
            drain(&mut fixture.commands);
        } else {
            assert!(
                matches!(commands.as_slice(), [Command::ExportCa { path }] if *path == selected)
            );
        }
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert!(view.file_picker.is_none() && view.quit_attempt.is_none());
            assert!(!view.shutdown_requested && !view.shutdown.is_cancelled());
        });
    }
}

#[gpui_kit::test]
fn busy_file_picker_project_change_retains_dialog_and_ignores_stale_answer(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("旧项目")], 64);
    drain(&mut fixture.commands);
    for menu in ["打开项目…", "导出 Proxy CA…", "另存为…"] {
        project_menu(&fixture, menu, cx);
        let picker = cx.read(|cx| fixture.view.read(cx).file_picker.unwrap());
        let next = tab("保留新项目");
        let next_id = next.id;
        fixture.project.generation += 1;
        fixture
            .events
            .try_send(Event::ProjectOpened {
                project: fixture.project,
                path: None,
                workspace: Workspace {
                    tabs: vec![next],
                    active: Some(next_id),
                    ..Workspace::default()
                },
            })
            .unwrap();
        pump_native(cx);
        drain(&mut fixture.commands);
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            window.dispatch_action(Box::new(Quit), cx);
            window.dispatch_action(Box::new(OpenProject), cx);
            window.dispatch_action(Box::new(ExportCa), cx);
            window.dispatch_action(Box::new(SaveProjectAs), cx);
        })
        .unwrap();
        pump_native(cx);
        assert!(!cx.has_pending_prompt());
        cx.read(|cx| assert_eq!(fixture.view.read(cx).file_picker, Some(picker)));
        answer_file_picker(
            menu,
            Some(std::env::temp_dir().join("mio-picker-stale-synthetic-selection")),
            cx,
        );
        assert!(drain(&mut fixture.commands).is_empty());
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert!(view.file_picker.is_none() && !view.choosing_save_as);
            assert_eq!(view.project, Some(fixture.project));
            assert_eq!(view.workspace.active, Some(next_id));
            assert_eq!(view.workspace.tabs[0].title, "保留新项目");
            assert!(!view.shutdown.is_cancelled());
        });
        cx.simulate_keystrokes(fixture.window, crate::kbd!("q"));
        pump_native(cx);
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("取消");
        pump_native(cx);
        assert!(!cx.has_pending_prompt());
    }
}

fn busy_picker_backend_close(menu: &str, cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("后端断开仍保留编辑")], 64);
    drain(&mut fixture.commands);
    project_menu(&fixture, menu, cx);
    drop(fixture.events);
    pump_native(cx);
    answer_file_picker(
        menu,
        Some(std::env::temp_dir().join("mio-picker-disconnected-synthetic-selection")),
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.backend_disconnected && view.file_picker.is_none());
        assert_eq!(view.workspace.tabs[0].title, "后端断开仍保留编辑");
        assert!(!view.quitting && !view.shutdown_requested);
    });
    cx.simulate_keystrokes(fixture.window, crate::kbd!("q"));
    pump_native(cx);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("取消");
    pump_native(cx);
    assert!(!cx.has_pending_prompt());
}

#[gpui_kit::test]
fn busy_file_picker_backend_close_discards_open_answer(cx: &mut TestAppContext) {
    busy_picker_backend_close("打开项目…", cx);
}

#[gpui_kit::test]
fn busy_file_picker_backend_close_discards_export_answer(cx: &mut TestAppContext) {
    busy_picker_backend_close("导出 Proxy CA…", cx);
}

fn settings_save_failure_preserves_quit_sheet(workspace_rejected: bool, cx: &mut TestAppContext) {
    let first = tab("保存失败后保留输入");
    let id = first.id;
    let mut fixture = fixture(cx, vec![first], 64);
    drain(&mut fixture.commands);
    let stored = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
    let raw = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    let settings_window = settings::open_settings(&fixture, cx);
    settings::toggle(settings_window, false, cx);
    let pending = settings::saved(&mut fixture.commands);
    let latest = "POST /pending-save HTTP/1.1\r\nHost: localhost\r\n\r\n尚未保存🙂";
    settings::activate_empty_repeater(&mut fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", raw.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(latest, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.pending_workspace, Some(pending.revision));
        assert!(!view.workspace.repeater_settings.unpack_compressed_responses);
        assert_eq!(raw.read(cx).value().as_str(), latest);
        assert!(!view.shutdown_requested && !view.shutdown.is_cancelled());
    });
    assert!(drain(&mut fixture.commands).is_empty());

    settings::menu_action(settings_window, "MIO", "退出 MIO", cx);
    let prompt_a = cx.pending_prompt().expect("Quit opens sheet A");
    let attempt_a = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.transition == Some(TransitionPhase::Confirming));
        assert_eq!(view.pending_workspace, Some(pending.revision));
        view.quit_attempt.expect("Quit owns attempt A")
    });
    let message = "synthetic pending Settings save failed";
    let failure = if workspace_rejected {
        Event::WorkspaceRejected {
            project: fixture.project,
            revision: pending.revision,
            stored,
            message: message.into(),
        }
    } else {
        Event::Error {
            project: Some(fixture.project),
            operation: "save_workspace",
            message: message.into(),
        }
    };
    fixture.events.try_send(failure).unwrap();
    pump_native(cx);
    assert_eq!(cx.pending_prompt(), Some(prompt_a.clone()));
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.pending_workspace.is_none() && view.workspace_dirty);
        assert!(view.error.as_ref().unwrap().contains(message));
        assert!(view.quit_attempt.is_none());
        assert!(!view.quitting && !view.shutdown_requested && !view.shutdown.is_cancelled());
        assert!(!view.workspace.repeater_settings.unpack_compressed_responses);
        assert_eq!(raw.read(cx).value().as_str(), latest);
        assert!(
            view.transition == Some(TransitionPhase::Confirming),
            "save failure must keep sheet A occupied until its answer, before any native retry"
        );
    });
    assert!(
        drain(&mut fixture.commands).iter().all(|command| {
            matches!(command, Command::Resume { project } if *project == fixture.project)
        }),
        "a failed save cannot authorize saving, sending or switching projects"
    );

    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [false]);
    assert_eq!(cx.pending_prompt(), Some(prompt_a));
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.transition == Some(TransitionPhase::Confirming));
        assert!(view.quit_attempt.is_none());
        assert!(!view.quitting && !view.shutdown_requested && !view.shutdown.is_cancelled());
    });

    cx.simulate_prompt_answer("放弃并继续");
    cx.run_until_parked();
    assert!(
        !cx.has_pending_prompt(),
        "answering expired sheet A must leave no queued duplicate quit sheet"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.transition.is_none() && view.quit_attempt.is_none());
        assert!(!view.quitting && !view.shutdown_requested && !view.shutdown.is_cancelled());
        assert_eq!(raw.read(cx).value().as_str(), latest);
    });
    assert!(drain(&mut fixture.commands).is_empty());
    assert_eq!(probe.replies(), [false]);

    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let attempt_b = view
            .quit_attempt
            .expect("fresh native request owns attempt B");
        assert_ne!(attempt_a, attempt_b);
        assert!(view.transition == Some(TransitionPhase::Confirming));
        assert!(!view.quitting && !view.shutdown_requested && !view.shutdown.is_cancelled());
        assert_eq!(raw.read(cx).value().as_str(), latest);
    });
    assert_eq!(probe.replies(), [false]);
    cx.simulate_prompt_answer("取消");
    cx.run_until_parked();
    probe.pump_modal_until_replies(2);
    assert_eq!(probe.replies(), [false, false]);
    assert!(!cx.has_pending_prompt());
    assert!(drain(&mut fixture.commands).is_empty());
    settings::checked(settings_window, false, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(fixture.project));
        assert!(view.transition.is_none() && view.quit_attempt.is_none());
        assert!(!view.quitting && !view.shutdown_requested && !view.shutdown.is_cancelled());
        assert!(view.workspace_dirty && view.pending_workspace.is_none());
        assert_eq!(raw.read(cx).value().as_str(), latest);
    });
}

#[gpui_kit::test]
fn workspace_rejection_keeps_pending_quit_sheet_exclusive(cx: &mut TestAppContext) {
    settings_save_failure_preserves_quit_sheet(true, cx);
}

#[gpui_kit::test]
fn workspace_save_error_keeps_pending_quit_sheet_exclusive(cx: &mut TestAppContext) {
    settings_save_failure_preserves_quit_sheet(false, cx);
}
