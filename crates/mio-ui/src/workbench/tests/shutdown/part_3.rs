use super::*;

#[gpui_kit::test]
fn startup_initial_project_event_preserves_native_quit_until_runtime_join(cx: &mut TestAppContext) {
    let mut fixture = uninitialized_startup_fixture(cx);
    let finished = observe_runtime(&fixture, cx);
    drain(&mut fixture.commands);
    let probe = Probe::installed();
    cx.simulate_keystrokes(fixture.window, crate::kbd!("q"));
    pump_native(cx);
    let attempt = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.project.is_none());
        assert!(view.startup_wizard_active());
        assert!(view.startup_wizard_picker_pending());
        assert_eq!(probe.terminations(), [Reply::TerminateLater]);
        view.quit_attempt
            .expect("Quit must have a stable confirmation identity")
    });
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: fixture.project,
            path: None,
            workspace: Workspace::default(),
        })
        .unwrap();
    pump_native(cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.startup_wizard_active() && view.startup_wizard_picker_pending());
        assert_eq!(
            view.quit_attempt,
            Some(attempt),
            "the initial empty service project cannot revoke the pending wizard confirmation"
        );
        assert!(probe.replies().is_empty());
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("wizard-cancel-yes", cx);
    })
    .unwrap();
    pump_native(cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.shutdown_requested && view.shutdown.is_cancelled());
        assert!(!view.quitting && view.runtime_result.is_none());
        assert!(
            probe.replies().is_empty(),
            "native exit still waits for actual join"
        );
    });
    finished.send(Ok(())).unwrap();
    pump_native(cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.quitting);
        assert!(matches!(view.runtime_result, Some(Ok(()))));
    });
    assert_eq!(probe.replies(), [true]);
    let commands = drain(&mut fixture.commands);
    assert!(
        commands.iter().all(|command| matches!(
            command,
            Command::QueryHttpHistory { .. } | Command::QueryIntruderRuns { .. }
        )),
        "initial project refresh may only query, never send, save or start a task"
    );
}

#[gpui_kit::test]
fn startup_successor_project_event_still_revokes_the_old_native_quit(cx: &mut TestAppContext) {
    let mut fixture = uninitialized_startup_fixture(cx);
    drain(&mut fixture.commands);
    let probe = Probe::installed();
    cx.simulate_keystrokes(fixture.window, crate::kbd!("q"));
    pump_native(cx);
    let initial_attempt = cx.read(|cx| fixture.view.read(cx).quit_attempt.unwrap());
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: fixture.project,
            path: None,
            workspace: Workspace::default(),
        })
        .unwrap();
    pump_native(cx);
    cx.read(|cx| assert_eq!(fixture.view.read(cx).quit_attempt, Some(initial_attempt)));
    let successor = ProjectKey {
        id: Uuid::new_v4(),
        generation: 2,
    };
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: successor,
            path: None,
            workspace: Workspace::default(),
        })
        .unwrap();
    pump_native(cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(successor));
        assert!(view.quit_attempt.is_none());
        assert!(!view.shutdown.is_cancelled() && !view.quitting);
    });
    assert_eq!(
        probe.replies(),
        [false],
        "a real successor project must revoke the old exit authorization"
    );
}

#[gpui_kit::test]
fn quit_requests_cancellation_without_terminating_before_runtime_join(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let finished = observe_runtime(&fixture, cx);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.shutdown.is_cancelled());
        assert!(
            !view.quitting,
            "the native terminate path must wait for the service thread to be joined"
        );
    });
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("runtime-shutdown").visible());
        assert!(window.try_find("cancel-transition").is_none());
        window.dispatch_action(Box::new(Quit), cx);
        window.dispatch_action(Box::new(SaveProject), cx);
    })
    .unwrap();
    fixture.events.try_send(Event::Stopped).unwrap();
    pump_native(cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| assert!(!fixture.view.read(cx).quitting));
    finished.send(Ok(())).unwrap();
    pump_native(cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.quitting);
        assert!(matches!(view.runtime_result, Some(Ok(()))));
    });
}

#[gpui_kit::test]
fn native_close_callback_waits_for_the_runtime_join(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("关闭窗口前确认")], 64);
    let finished = observe_runtime(&fixture, cx);
    drain(&mut fixture.commands);
    let mut native = gpui_kit::VisualTestContext::from_window(fixture.window, cx);
    assert!(!native.simulate_close());
    pump_native(cx);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("取消");
    pump_native(cx);
    cx.read(|cx| assert!(!fixture.view.read(cx).shutdown.is_cancelled()));
    assert!(!native.simulate_close());
    pump_native(cx);
    cx.simulate_prompt_answer("放弃并继续");
    pump_native(cx);
    assert!(!native.simulate_close());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.shutdown_requested);
        assert!(!view.quitting);
    });
    assert!(drain(&mut fixture.commands).is_empty());
    finished.send(Ok(())).unwrap();
    pump_native(cx);
    cx.read(|cx| assert!(fixture.view.read(cx).quitting));
}

#[gpui_kit::test]
fn stopped_event_cannot_authorize_native_termination(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("保留尚未回收的工作区")], 64);
    fixture.events.try_send(Event::Stopped).unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            !view.quitting,
            "State emits Stopped before its resources are dropped and its thread is joined"
        );
        assert_eq!(view.workspace.tabs[0].title, "保留尚未回收的工作区");
    });
}

#[gpui_kit::test]
fn discard_quit_keeps_the_window_while_a_full_queue_prevents_shutdown_completion(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("队列满时退出")], 1);
    assert_eq!(fixture.commands.len(), 1);
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    cx.simulate_prompt_answer("放弃并继续");
    pump_native(cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.shutdown.is_cancelled());
        assert!(
            !view.quitting,
            "out-of-band cancellation is a request; it does not prove cleanup completed"
        );
    });
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryHttpHistory { .. }]
    ));
}

#[gpui_kit::test]
fn channel_close_still_waits_for_join_and_cleanup_error_requires_explicit_exit(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("回收失败时保留草稿")], 64);
    let finished = observe_runtime(&fixture, cx);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    cx.simulate_prompt_answer("放弃并继续");
    pump_native(cx);
    drop(fixture.events);
    pump_native(cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.quitting);
        assert!(view.shutdown_requested);
        assert!(view.runtime_result.is_none());
    });
    finished
        .send(Err(anyhow::anyhow!("测试 checkpoint 失败")))
        .unwrap();
    pump_native(cx);
    assert!(cx.has_pending_prompt());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.quitting);
        assert!(!view.shutdown_requested);
        assert!(
            view.error
                .as_deref()
                .unwrap()
                .contains("测试 checkpoint 失败")
        );
        assert_eq!(view.workspace.tabs[0].title, "回收失败时保留草稿");
    });
    cx.simulate_prompt_answer("保留窗口");
    pump_native(cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.transition.is_none());
        assert!(!view.quitting);
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    cx.simulate_prompt_answer("退出");
    pump_native(cx);
    cx.read(|cx| assert!(fixture.view.read(cx).quitting));
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn quit_from_auxiliary_window_uses_main_project_and_waits_for_join(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("主窗口草稿")], 64);
    let finished = observe_runtime(&fixture, cx);
    drain(&mut fixture.commands);
    let (auxiliary, _) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
            cx.new(|_| AuxiliaryWindow)
        })
        .unwrap()
    });
    cx.update_window(auxiliary, |_, window, cx| {
        window.render_frame(cx);
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(
        cx.has_pending_prompt(),
        "the application Quit action must reach the owning project from another window"
    );
    cx.simulate_prompt_answer("放弃并继续");
    pump_native(cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.shutdown_requested);
        assert!(view.shutdown.is_cancelled());
        assert!(!view.quitting);
        assert_eq!(view.workspace.tabs[0].title, "主窗口草稿");
    });
    finished.send(Ok(())).unwrap();
    pump_native(cx);
    cx.read(|cx| assert!(fixture.view.read(cx).quitting));
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn joined_error_survives_buffered_events_and_receiver_close(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("回收后仍保留此草稿")], 64);
    let finished = observe_runtime(&fixture, cx);
    finished
        .send(Err(anyhow::anyhow!("测试最终清理失败")))
        .unwrap();
    cx.run_until_parked();
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: fixture.project,
            path: None,
            workspace: Workspace::default(),
        })
        .unwrap();
    drop(fixture.events);
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            view.error.as_deref().unwrap().contains("测试最终清理失败"),
            "channel closure must preserve the actual joined result"
        );
        assert_eq!(view.workspace.tabs.len(), 1);
        assert_eq!(view.workspace.tabs[0].title, "回收后仍保留此草稿");
        assert!(!view.quitting);
    });
}

#[gpui_kit::test]
fn repeated_quit_opens_only_one_confirmation(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("只确认一次")], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(Quit), cx);
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("取消");
    pump_native(cx);
    assert!(
        !cx.has_pending_prompt(),
        "one user cancellation must dismiss the only pending Quit confirmation"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.transition.is_none());
        assert!(!view.shutdown.is_cancelled());
    });
}

#[gpui_kit::test]
fn completed_runtime_invalidates_a_pending_quit_answer(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("迟到的退出答案")], 64);
    let finished = observe_runtime(&fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(cx.has_pending_prompt());
    finished
        .send(Err(anyhow::anyhow!("确认期间后台失败")))
        .unwrap();
    pump_native(cx);
    cx.simulate_prompt_answer("放弃并继续");
    pump_native(cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.runtime_result.is_some());
        assert!(
            !view.shutdown_requested,
            "a joined Runtime cannot emit another completion"
        );
        assert!(view.transition.is_none());
        assert!(!view.quitting);
        assert_eq!(view.workspace.tabs[0].title, "迟到的退出答案");
        assert!(view.error.as_deref().unwrap().contains("确认期间后台失败"));
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("保留窗口");
    pump_native(cx);
}

#[gpui_kit::test]
fn pending_shutdown_cannot_open_file_dialogs(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let finished = observe_runtime(&fixture, cx);
    drain(&mut fixture.commands);
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.dispatch_action(Box::new(ExportCa), cx);
        window.dispatch_action(Box::new(SaveProjectAs), cx);
        window.dispatch_action(Box::new(OpenProject), cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(
        !cx.did_prompt_for_new_path(),
        "shutdown cannot start another save dialog"
    );
    assert!(!cx.did_prompt_for_paths());
    assert!(drain(&mut fixture.commands).is_empty());
    finished.send(Ok(())).unwrap();
    pump_native(cx);
    cx.read(|cx| assert!(fixture.view.read(cx).quitting));
}
