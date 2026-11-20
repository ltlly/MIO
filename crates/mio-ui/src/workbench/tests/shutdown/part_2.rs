use super::*;

#[gpui_kit::test]
fn native_timeout_invalidates_late_prompt_without_overlapping_sheets(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("迟到确认")], 64);
    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    probe.expire();
    probe.pump_modal_until_replies(1);
    cx.run_until_parked();
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    probe.pump_modal_until_replies(2);
    assert_eq!(probe.replies(), [false, false]);
    cx.simulate_prompt_answer("放弃并继续");
    cx.run_until_parked();
    assert!(!cx.has_pending_prompt());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.shutdown.is_cancelled());
        assert!(!view.quitting);
        assert!(view.transition.is_none());
    });
}

#[gpui_kit::test]
fn native_save_picker_cancel_replies_no(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("保存选择取消")], 64);
    let probe = Probe::new();
    native_save_picker(&mut fixture, &probe, cx);
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [false]);
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::Resume { .. }]
    ));
    cx.read(|cx| assert!(!fixture.view.read(cx).shutdown.is_cancelled()));
}

#[gpui_kit::test]
fn native_save_failure_replies_no_and_keeps_edits(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("保存失败的编辑")], 64);
    let probe = Probe::new();
    native_save_picker(&mut fixture, &probe, cx);
    cx.simulate_new_path_selection(|_| {
        Some(PathBuf::from("/tmp/mio-native-save-failure-synthetic.mio"))
    });
    cx.run_until_parked();
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::SaveProject { .. }]
    ));
    fixture
        .events
        .try_send(Event::Error {
            project: Some(fixture.project),
            operation: "save_project",
            message: "synthetic disk error".into(),
        })
        .unwrap();
    cx.run_until_parked();
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [false]);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.shutdown.is_cancelled());
        assert_eq!(view.workspace.tabs[0].title, "保存失败的编辑");
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .contains("synthetic disk error")
        );
    });
}

#[gpui_kit::test]
fn native_timeout_prevents_late_saved_event_from_starting_shutdown(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("超时后保存完成")], 64);
    let probe = Probe::new();
    native_save_picker(&mut fixture, &probe, cx);
    let path = PathBuf::from("/tmp/mio-native-late-saved-synthetic.mio");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    cx.run_until_parked();
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::SaveProject { .. }]
    ));
    probe.expire();
    probe.pump_modal_until_replies(1);
    // Deliver the backend event before the timeout observer gets its turn.
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.on_event(
                Event::ProjectSaved {
                    websocket_notes: vec![],
                    ext_http_notes: Vec::new(),
                    ext_http_highlights: Vec::new(),
                    project: fixture.project,
                    path,
                    total_bytes: 4096,
                },
                window,
                cx,
            )
        });
    })
    .unwrap();
    cx.read(|cx| {
        assert!(
            !fixture.view.read(cx).shutdown.is_cancelled(),
            "a saved event from the expired quit must not start shutdown"
        )
    });
    cx.run_until_parked();
    assert_eq!(probe.replies(), [false]);
}

#[gpui_kit::test]
fn native_timeout_prevents_late_quiesced_event_from_opening_save_picker(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("超时后连接停止")], 64);
    let probe = Probe::new();
    drain(&mut fixture.commands);
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    cx.simulate_prompt_answer("保存");
    cx.run_until_parked();
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::Quiesce { .. }]
    ));
    probe.expire();
    probe.pump_modal_until_replies(1);
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.on_event(
                Event::Quiesced {
                    project: fixture.project,
                },
                window,
                cx,
            )
        });
    })
    .unwrap();
    assert!(
        !cx.did_prompt_for_new_path(),
        "an expired quit cannot schedule another native modal panel"
    );
    cx.run_until_parked();
    assert_eq!(probe.replies(), [false]);
}

#[gpui_kit::test]
fn native_timeout_invalidates_save_picker_before_gpui_notification(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("迟到保存位置")], 64);
    let probe = Probe::new();
    native_save_picker(&mut fixture, &probe, cx);
    probe.expire();
    probe.pump_modal_until_replies(1);
    cx.simulate_new_path_selection(|_| {
        Some(PathBuf::from("/tmp/mio-native-late-picker-synthetic.mio"))
    });
    cx.run_until_parked();
    assert_eq!(probe.replies(), [false]);
    assert!(
        drain(&mut fixture.commands)
            .iter()
            .all(|command| matches!(command, Command::Resume { .. }))
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.shutdown.is_cancelled());
        assert!(view.after_save.is_none());
    });
}

fn native_overdue_confirmation_preserves_edits(answer: &str, cx: &mut TestAppContext) {
    let first = tab("过期确认保留输入");
    let id = first.id;
    let mut fixture = fixture(cx, vec![first], 64);
    let raw = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    let latest = "POST /overdue-answer HTTP/1.1\r\nHost: localhost\r\n\r\n尚未保存的输入🙂";
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click(("input", raw.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(latest, cx);
    })
    .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);
    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    probe.expire();
    cx.simulate_prompt_answer(answer);
    cx.run_until_parked();
    // Observe the answer before the overdue AppKit timer can hide its side effects.
    let shutdown_started = cx.read(|cx| {
        let view = fixture.view.read(cx);
        view.shutdown_requested || view.shutdown.is_cancelled()
    });
    let commands_before_timer = drain(&mut fixture.commands);
    let picker_opened = cx.did_prompt_for_new_path();
    assert!(probe.replies().is_empty());
    probe.pump_modal_until_replies(1);
    cx.run_until_parked();
    assert!(
        !shutdown_started,
        "an overdue {answer} answer must not cancel Runtime before the native timer runs"
    );
    assert!(
        commands_before_timer.is_empty(),
        "an overdue {answer} answer must not quiesce or save before the native timer runs"
    );
    assert!(!picker_opened);
    assert_eq!(probe.replies(), [false]);
    assert!(!cx.has_pending_prompt());
    assert!(
        drain(&mut fixture.commands)
            .iter()
            .all(|command| matches!(command, Command::Resume { .. }))
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(fixture.project));
        assert_eq!(view.workspace.active, Some(id));
        assert_eq!(raw.read(cx).value().as_str(), latest);
        assert!(!view.shutdown_requested && !view.shutdown.is_cancelled() && !view.quitting);
        assert!(view.quit_attempt.is_none() && view.transition.is_none());
        assert!(view.after_save.is_none() && view.pending_project_save.is_none());
    });
}

#[gpui_kit::test]
fn native_overdue_discard_answer_cannot_cancel_runtime_before_timer(cx: &mut TestAppContext) {
    native_overdue_confirmation_preserves_edits("放弃并继续", cx);
}

#[gpui_kit::test]
fn native_overdue_save_answer_cannot_quiesce_before_timer(cx: &mut TestAppContext) {
    native_overdue_confirmation_preserves_edits("保存", cx);
}

#[gpui_kit::test]
fn native_overdue_save_picker_answer_cannot_save_before_timer(cx: &mut TestAppContext) {
    let first = tab("过期保存位置保留输入");
    let id = first.id;
    let mut fixture = fixture(cx, vec![first], 64);
    let raw = cx.read(|cx| fixture.view.read(cx).tab_editors[&id].request.raw.clone());
    let latest = "POST /overdue-picker HTTP/1.1\r\nHost: localhost\r\n\r\n保留保存前的输入🙂";
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-repeater", cx);
        window.click(("input", raw.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input(latest, cx);
    })
    .unwrap();
    cx.run_until_parked();
    let probe = Probe::new();
    native_save_picker(&mut fixture, &probe, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    probe.expire();
    cx.simulate_new_path_selection(|_| {
        Some(PathBuf::from(
            "/tmp/mio-native-overdue-picker-synthetic.mio",
        ))
    });
    cx.run_until_parked();
    let commands_before_timer = drain(&mut fixture.commands);
    assert!(probe.replies().is_empty());
    probe.pump_modal_until_replies(1);
    cx.run_until_parked();
    assert!(
        commands_before_timer
            .iter()
            .all(|command| matches!(command, Command::Resume { .. })),
        "an overdue picker answer must not enqueue SaveProject or SaveWorkspace before the native timer runs"
    );
    assert!(
        drain(&mut fixture.commands)
            .iter()
            .all(|command| matches!(command, Command::Resume { .. }))
    );
    assert_eq!(probe.replies(), [false]);
    assert!(!cx.did_prompt_for_new_path());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(fixture.project));
        assert_eq!(view.workspace.active, Some(id));
        assert_eq!(raw.read(cx).value().as_str(), latest);
        assert!(!view.shutdown_requested && !view.shutdown.is_cancelled() && !view.quitting);
        assert!(view.file_picker.is_none() && !view.choosing_save_as);
        assert!(view.quit_attempt.is_none() && view.transition.is_none());
        assert!(view.after_save.is_none() && view.pending_project_save.is_none());
    });
}

#[gpui_kit::test]
fn native_save_command_rejection_replies_no(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("队列满时拒绝保存退出")], 1);
    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    cx.simulate_prompt_answer("保存");
    cx.run_until_parked();
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [false]);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.shutdown.is_cancelled());
        assert!(view.quit_attempt.is_none());
        assert!(view.error.as_ref().unwrap().contains("队列已满"));
    });
}

#[gpui_kit::test]
fn lost_main_window_retains_join_error_and_edits(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("保留主窗口的编辑")], 64);
    let finished = observe_runtime(&fixture, cx);
    cx.update_window(fixture.window, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    finished
        .send(Err(anyhow::anyhow!("synthetic join failure")))
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            matches!(&view.runtime_result, Some(Err(error)) if error.contains("synthetic join failure")),
            "losing the main window must retain the actual join result rather than authorize exit"
        );
        assert_eq!(view.workspace.tabs[0].title, "保留主窗口的编辑");
        assert!(!view.quitting);
    });
}

#[gpui_kit::test]
fn native_delegate_exposes_cancellable_termination_before_quit_dispatch(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![], 64);
    let finished = observe_runtime(&fixture, cx);
    let delegate = objc2::runtime::AnyClass::get(c"GPUIApplicationDelegate")
        .expect("the linked GPUI backend registers its original delegate class");
    assert!(
        delegate
            .instance_method(objc2::sel!(applicationShouldTerminate:))
            .is_some(),
        "native terminate must enter the cancellable MIO exit flow before AppKit can close windows"
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    pump_native(cx);
    cx.read(|cx| assert!(!fixture.view.read(cx).quitting));
    finished.send(Ok(())).unwrap();
    pump_native(cx);
    cx.read(|cx| assert!(fixture.view.read(cx).quitting));
}
