use super::*;

#[gpui_kit::test]
fn native_entry_mio_quit_menu_creates_request(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("菜单退出保留编辑")], 64);
    let requests = cx.read(|cx| cx.global::<NativeTerminate>().subscribe());
    settings::menu_action(fixture.window, "MIO", "退出 MIO", cx);
    pump_native(cx);
    let request_before_cancel = *requests.borrow();
    if cx.has_pending_prompt() {
        cx.simulate_prompt_answer("取消");
        pump_native(cx);
    }
    assert!(
        matches!(request_before_cancel, Some(NativeRequest::Requested(_))),
        "the installed MIO Quit menu action must create a native termination request"
    );
}

#[gpui_kit::test]
fn native_entry_main_cmd_q_creates_request(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("快捷键退出保留编辑")], 64);
    let requests = cx.read(|cx| cx.global::<NativeTerminate>().subscribe());
    cx.simulate_keystrokes(fixture.window, crate::kbd!("q"));
    pump_native(cx);
    let request_before_cancel = *requests.borrow();
    if cx.has_pending_prompt() {
        cx.simulate_prompt_answer("取消");
        pump_native(cx);
    }
    assert!(
        matches!(request_before_cancel, Some(NativeRequest::Requested(_))),
        "Cmd+Q in the main window must create a native termination request"
    );
}

#[gpui_kit::test]
fn native_entry_settings_quit_menu_creates_request(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("设置窗口退出保留编辑")], 64);
    let settings_window = settings::open_settings(&fixture, cx);
    let requests = cx.read(|cx| cx.global::<NativeTerminate>().subscribe());
    settings::menu_action(settings_window, "MIO", "退出 MIO", cx);
    pump_native(cx);
    let request_before_cancel = *requests.borrow();
    if cx.has_pending_prompt() {
        cx.simulate_prompt_answer("取消");
        pump_native(cx);
    }
    cx.simulate_keystrokes(settings_window, "escape");
    pump_native(cx);
    assert!(
        matches!(request_before_cancel, Some(NativeRequest::Requested(_))),
        "the installed Quit menu action in Settings must create a native termination request"
    );
}

#[gpui_kit::test]
fn native_entry_main_window_close_creates_request(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("关闭主窗口保留编辑")], 64);
    let requests = cx.read(|cx| cx.global::<NativeTerminate>().subscribe());
    let mut native = gpui_kit::VisualTestContext::from_window(fixture.window, cx);
    let closed = native.simulate_close();
    pump_native(cx);
    let request_before_cancel = *requests.borrow();
    if cx.has_pending_prompt() {
        cx.simulate_prompt_answer("取消");
        pump_native(cx);
    }
    assert!(!closed, "the main window must wait for the exit decision");
    assert!(
        matches!(request_before_cancel, Some(NativeRequest::Requested(_))),
        "closing the main window must create a native termination request"
    );
}

#[gpui_kit::test]
fn native_entry_queued_quit_actions_share_one_request(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("同轮退出保留编辑")], 64);
    let requests = cx.read(|cx| cx.global::<NativeTerminate>().subscribe());
    let probe = Probe::installed();
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(Quit), cx);
        window.dispatch_action(Box::new(Quit), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(requests.borrow().is_none());
    assert!(probe.terminations().is_empty());
    assert!(!cx.has_pending_prompt());
    pump_native(cx);
    let Some(NativeRequest::Requested(first)) = *requests.borrow() else {
        panic!("the queued Quit actions must reach the native delegate");
    };
    assert_eq!(probe.terminations(), [Reply::TerminateLater]);
    assert!(cx.has_pending_prompt());
    cx.simulate_keystrokes(fixture.window, crate::kbd!("q"));
    pump_native(cx);
    assert!(matches!(*requests.borrow(), Some(NativeRequest::Requested(id)) if id == first));
    assert_eq!(probe.terminations(), [Reply::TerminateLater]);
    cx.simulate_prompt_answer("取消");
    pump_native(cx);
    assert_eq!(probe.replies(), [false]);
    assert!(!cx.has_pending_prompt());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.quit_attempt.is_none() && view.transition.is_none());
        assert!(!view.shutdown.is_cancelled() && !view.quitting);
    });
}

#[gpui_kit::test]
fn native_entry_settings_cmd_q_creates_request(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("设置快捷键退出保留编辑")], 64);
    let settings_window = settings::open_settings(&fixture, cx);
    let requests = cx.read(|cx| cx.global::<NativeTerminate>().subscribe());
    let probe = Probe::installed();
    cx.simulate_keystrokes(settings_window, crate::kbd!("q"));
    pump_native(cx);
    let request_before_cancel = *requests.borrow();
    if cx.has_pending_prompt() {
        cx.simulate_prompt_answer("取消");
        pump_native(cx);
    }
    cx.simulate_keystrokes(settings_window, "escape");
    pump_native(cx);
    assert!(matches!(
        request_before_cancel,
        Some(NativeRequest::Requested(_))
    ));
    assert_eq!(probe.terminations(), [Reply::TerminateLater]);
    assert_eq!(probe.replies(), [false]);
    assert_eq!(cx.windows(), vec![fixture.window]);
}

#[gpui_kit::test]
fn native_entry_settings_close_does_not_request_termination(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("关闭设置仍保留主窗")], 64);
    let settings_window = settings::open_settings(&fixture, cx);
    let requests = cx.read(|cx| cx.global::<NativeTerminate>().subscribe());
    let probe = Probe::installed();
    let mut native = gpui_kit::VisualTestContext::from_window(settings_window, cx);
    // This GPUI helper invokes should-close only; Escape performs window cleanup.
    let close_allowed = native.simulate_close();
    pump_native(cx);
    let request_before_cleanup = *requests.borrow();
    cx.simulate_keystrokes(settings_window, "escape");
    pump_native(cx);
    assert!(close_allowed, "Settings must allow its own window to close");
    assert!(request_before_cleanup.is_none());
    assert!(requests.borrow().is_none());
    assert!(probe.terminations().is_empty() && probe.replies().is_empty());
    assert!(!cx.has_pending_prompt());
    assert_eq!(cx.windows(), vec![fixture.window]);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.quit_attempt.is_none() && view.transition.is_none());
        assert!(!view.shutdown.is_cancelled() && !view.quitting);
    });
}

#[gpui_kit::test]
fn native_overdue_request_cannot_open_confirmation_before_timer(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("延迟原生请求保留编辑")], 64);
    drain(&mut fixture.commands);
    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    probe.expire();
    cx.run_until_parked();
    assert!(
        !cx.has_pending_prompt(),
        "an overdue native request must not open a confirmation before its timer runs"
    );
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(probe.replies().is_empty());
    probe.pump_modal_until_replies(1);
    cx.run_until_parked();
    assert_eq!(probe.replies(), [false]);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.quit_attempt.is_none() && view.transition.is_none());
        assert!(!view.shutdown_requested && !view.shutdown.is_cancelled() && !view.quitting);
        assert_eq!(view.workspace.tabs[0].title, "延迟原生请求保留编辑");
    });
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("取消");
    cx.run_until_parked();
    probe.pump_modal_until_replies(2);
    assert_eq!(probe.replies(), [false, false]);
    assert!(!cx.has_pending_prompt());
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn native_observer_releases_its_guard_when_app_global_is_removed(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("全局退出后释放观察者")], 64);
    drop(cx.update(|cx| cx.remove_global::<crate::native_terminate::NativeTerminate>()));
    cx.run_until_parked();
    cx.update(|cx| cx.set_global(crate::native_terminate::NativeTerminate::for_test()));
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).workspace.tabs[0].title,
            "全局退出后释放观察者"
        )
    });
}

#[gpui_kit::test]
fn native_save_success_waits_for_join_then_replies_yes(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("保存后退出")], 64);
    let finished = observe_runtime(&fixture, cx);
    let probe = Probe::new();
    native_save_picker(&mut fixture, &probe, cx);
    let path = PathBuf::from("/tmp/mio-native-save-success-synthetic.mio");
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    cx.run_until_parked();
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::SaveProject { .. }]
    ));
    fixture
        .events
        .try_send(Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path,
            total_bytes: 4096,
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.shutdown.is_cancelled());
        assert!(!view.quitting);
        assert_eq!(view.workspace.tabs[0].title, "保存后退出");
    });
    assert!(probe.replies().is_empty());
    finished.send(Ok(())).unwrap();
    cx.run_until_parked();
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [true]);
}

#[gpui_kit::test]
fn native_request_while_an_unrelated_save_picker_is_open_replies_no(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("保留另存为对话框")], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(SaveProjectAs), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path());
    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [false]);
    assert!(cx.did_prompt_for_new_path());
    assert!(!cx.has_pending_prompt());
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
}

#[gpui_kit::test]
fn native_deadline_invalidates_late_join_error_confirmation(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![], 64);
    let finished = observe_runtime(&fixture, cx);
    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    finished
        .send(Err(anyhow::anyhow!("synthetic joined error")))
        .unwrap();
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    probe.expire();
    probe.pump_modal_until_replies(1);
    cx.simulate_prompt_answer("退出");
    cx.run_until_parked();
    assert_eq!(probe.replies(), [false]);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.quitting);
        assert!(matches!(view.runtime_result, Some(Err(_))));
        assert!(view.transition.is_none());
    });
}

#[gpui_kit::test]
fn native_request_is_deferred_coalesced_and_cancel_replies_no(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("原生退出确认")], 64);
    let probe = Probe::new();
    cx.update(|_| {
        assert_eq!(probe.request(), Reply::TerminateLater);
        assert_eq!(probe.request(), Reply::TerminateLater);
        assert!(probe.replies().is_empty());
    });
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("取消");
    cx.run_until_parked();
    assert!(!cx.has_pending_prompt());
    assert!(
        probe.replies().is_empty(),
        "reply cannot run inside the GPUI borrow"
    );
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [false]);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.shutdown.is_cancelled());
        assert!(view.quit_attempt.is_none());
    });
}

#[gpui_kit::test]
fn native_request_joins_existing_menu_confirmation(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("共用退出轮次")], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.dispatch_action(Box::new(Quit), cx)
    })
    .unwrap();
    pump_native(cx);
    let probe = Probe::installed();
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    cx.simulate_prompt_answer("取消");
    cx.run_until_parked();
    assert!(!cx.has_pending_prompt());
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [false]);
}

#[gpui_kit::test]
fn native_reply_yes_requires_join_after_stopped_and_channel_close(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![], 64);
    let finished = observe_runtime(&fixture, cx);
    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    fixture.events.try_send(Event::Stopped).unwrap();
    drop(fixture.events);
    cx.run_until_parked();
    assert!(probe.replies().is_empty());
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.read(|cx| assert!(!fixture.view.read(cx).quitting));
    finished.send(Ok(())).unwrap();
    cx.run_until_parked();
    assert!(probe.replies().is_empty());
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [true]);
    assert_eq!(probe.request(), Reply::TerminateNow);
}

#[gpui_kit::test]
fn native_join_error_keeps_edits_and_requires_explicit_answer(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("join 错误中的编辑")], 64);
    let finished = observe_runtime(&fixture, cx);
    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    cx.simulate_prompt_answer("放弃并继续");
    cx.run_until_parked();
    finished
        .send(Err(anyhow::anyhow!("synthetic cleanup failure")))
        .unwrap();
    cx.run_until_parked();
    assert!(probe.replies().is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.quitting);
        assert_eq!(view.workspace.tabs[0].title, "join 错误中的编辑");
        assert!(
            view.error
                .as_ref()
                .unwrap()
                .contains("synthetic cleanup failure")
        );
    });
    cx.simulate_prompt_answer("保留窗口");
    cx.run_until_parked();
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [false]);
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    cx.simulate_prompt_answer("退出");
    cx.run_until_parked();
    probe.pump_modal_until_replies(2);
    assert_eq!(probe.replies(), [false, true]);
}

#[gpui_kit::test]
fn native_timeout_replies_no_retains_join_and_requires_fresh_exit(cx: &mut TestAppContext) {
    let fixture = fixture(cx, vec![tab("超时后保留编辑")], 64);
    let finished = observe_runtime(&fixture, cx);
    let probe = Probe::new();
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    cx.simulate_prompt_answer("放弃并继续");
    cx.run_until_parked();
    probe.expire();
    probe.pump_modal_until_replies(1);
    assert_eq!(probe.replies(), [false]);
    assert!(
        !finished.is_closed(),
        "the deadline must retain the join observer"
    );
    finished.send(Ok(())).unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(matches!(view.runtime_result, Some(Ok(()))));
        assert!(!view.quitting);
        assert!(!view.shutdown_requested);
        assert_eq!(view.workspace.tabs[0].title, "超时后保留编辑");
    });
    assert_eq!(probe.replies(), [false]);
    assert_eq!(probe.request(), Reply::TerminateLater);
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("退出");
    cx.run_until_parked();
    probe.pump_modal_until_replies(2);
    assert_eq!(probe.replies(), [false, true]);
}
