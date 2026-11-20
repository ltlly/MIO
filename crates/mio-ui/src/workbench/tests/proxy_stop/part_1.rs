use super::*;

#[gpui_kit::test]
fn global_intercept_toggle_controls_both_stages_without_changing_rule_groups(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 16);
    drain(&mut fixture.commands);
    let rules = cx.read(|cx| fixture.view.read(cx).workspace.proxy_interception.clone());
    assert!(rules.request.enabled);
    assert!(!rules.response.enabled);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("proxy-intercept", cx);
        window.click("intercept-toggle", cx);
    })
    .unwrap();
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::SetIntercept { project, requests: true, responses: true }]
            if *project == fixture.project
    ));
    assert_eq!(
        cx.read(|cx| fixture.view.read(cx).workspace.proxy_interception.clone()),
        rules,
        "global admission must not silently enable the default-disabled response rules"
    );
    fixture
        .events
        .try_send(Event::ProxyState {
            project: fixture.project,
            address: None,
            cancel: None,
            intercept_requests: true,
            intercept_responses: true,
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("intercept-responses").is_none());
        window.click("intercept-toggle", cx);
    })
    .unwrap();
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::SetIntercept { project, requests: false, responses: false }]
            if *project == fixture.project
    ));
}

#[gpui_kit::test]
fn stop_click_cancels_synchronously_and_waits_for_ack_before_restart(cx: &mut TestAppContext) {
    let (mut fixture, cancel) = listening_fixture(cx, 4);
    let settings = ensure_proxy_settings(&fixture, cx);
    drain(&mut fixture.commands);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-listener-toggle", 0usize), cx);
        assert!(
            cancel.is_cancelled(),
            "Stop must cancel before returning to the event loop"
        );
    })
    .unwrap();
    assert_stop_command(&mut fixture);
    assert_stopping(&fixture, cx);
    cx.update_window(settings, |_, window, cx| {
        window.click(("settings-listener-toggle", 0usize), cx);
        window.click(("settings-listener-toggle", 0usize), cx);
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());

    proxy_state(&fixture, fixture.project, None, cx);
    assert_stopped(&fixture, cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "ACK must not restart the listener"
    );
    cx.update_window(settings, |_, window, cx| {
        window.click(("settings-listener-toggle", 0usize), cx)
    })
    .unwrap();
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::StartProxy { project, listen }]
            if *project == fixture.project && *listen == "127.0.0.1:8080".parse::<SocketAddr>().unwrap()
    ));
    let next_cancel = CancellationToken::new();
    proxy_state(&fixture, fixture.project, Some((ADDRESS, &next_cancel)), cx);
    assert!(
        !next_cancel.is_cancelled(),
        "a fresh listener cannot inherit cancellation"
    );
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert!(!fixture.view.read(cx).proxy_busy);
        window.click(("settings-listener-toggle", 0usize), cx);
        assert!(next_cancel.is_cancelled());
    })
    .unwrap();
    assert_stop_command(&mut fixture);
    proxy_state(&fixture, fixture.project, None, cx);
    assert_stopped(&fixture, cx);
}

#[gpui_kit::test]
fn stop_click_cancels_with_a_full_command_queue_without_retry_or_false_ack(
    cx: &mut TestAppContext,
) {
    let (mut fixture, cancel) = listening_fixture(cx, 1);
    let settings = ensure_proxy_settings(&fixture, cx);
    assert_eq!(
        fixture.commands.len(),
        1,
        "the initial history query fills the only slot"
    );
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-listener-toggle", 0usize), cx);
        assert!(
            cancel.is_cancelled(),
            "a full command queue cannot prevent Stop"
        );
        assert!(
            fixture
                .view
                .read(cx)
                .error
                .as_deref()
                .unwrap()
                .contains("队列已满")
        );
    })
    .unwrap();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("dismiss-error").visible());
    })
    .unwrap();
    assert_stopping(&fixture, cx);
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryHttpHistory { query: mio_core::HttpHistoryQuery { project, .. } }] if *project == fixture.project
    ));
    cx.update_window(settings, |_, window, cx| {
        window.click(("settings-listener-toggle", 0usize), cx);
        window.click(("settings-listener-toggle", 0usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "free capacity must not resend Stop or Start"
    );
    assert_stopping(&fixture, cx);
    proxy_state(&fixture, fixture.project, None, cx);
    assert_stopped(&fixture, cx);
}

#[gpui_kit::test]
fn stop_click_cancels_with_a_closed_command_queue_and_disconnect_is_not_an_ack(
    cx: &mut TestAppContext,
) {
    let (mut fixture, cancel) = listening_fixture(cx, 4);
    let settings = ensure_proxy_settings(&fixture, cx);
    drain(&mut fixture.commands);
    fixture.commands.close();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-listener-toggle", 0usize), cx);
        assert!(
            cancel.is_cancelled(),
            "a closed command queue cannot prevent cancellation"
        );
        assert!(
            fixture
                .view
                .read(cx)
                .error
                .as_deref()
                .unwrap()
                .contains("后台服务")
        );
    })
    .unwrap();
    assert_stopping(&fixture, cx);
    assert!(matches!(
        fixture.commands.try_recv(),
        Err(mpsc::error::TryRecvError::Disconnected)
    ));
    proxy_state(&fixture, fixture.project, Some((ADDRESS, &cancel)), cx);
    assert_stopping(&fixture, cx);

    let (replacement, _) = mpsc::channel(1);
    drop(std::mem::replace(&mut fixture.events, replacement));
    cx.run_until_parked();
    assert_stopping(&fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(fixture.view.read(cx).backend_disconnected);
        assert!(
            fixture
                .view
                .read(cx)
                .error
                .as_deref()
                .unwrap()
                .contains("断开")
        );
        assert!(rendered_status(window).contains("后台已断开"));
        window.click("dismiss-error", cx);
    })
    .unwrap();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-listener-toggle", 0usize), cx);
    })
    .unwrap();
    assert_stopping(&fixture, cx);
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn cancelled_listener_states_and_errors_keep_stop_pending_until_the_real_ack(
    cx: &mut TestAppContext,
) {
    let (mut fixture, cancel) = listening_fixture(cx, 4);
    let settings = ensure_proxy_settings(&fixture, cx);
    drain(&mut fixture.commands);
    cx.update_window(settings, |_, window, cx| {
        window.click(("settings-listener-toggle", 0usize), cx)
    })
    .unwrap();
    assert_stop_command(&mut fixture);
    // The backend also cancels after consuming StopProxy.
    cancel.cancel();
    proxy_state(&fixture, fixture.project, Some((ADDRESS, &cancel)), cx);
    assert_stopping(&fixture, cx);

    for operation in ["stop_proxy", "set_intercept", "start_proxy"] {
        fixture
            .events
            .try_send(Event::Error {
                project: Some(fixture.project),
                operation,
                message: "synthetic proxy operation failure before Stop ACK".into(),
            })
            .unwrap();
        cx.run_until_parked();
        assert_stopping(&fixture, cx);
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                fixture
                    .view
                    .read(cx)
                    .error
                    .as_deref()
                    .unwrap()
                    .contains(operation)
            );
            assert!(window.find("dismiss-error").visible());
        })
        .unwrap();
        cx.update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            window.click(("settings-listener-toggle", 0usize), cx);
        })
        .unwrap();
        assert!(drain(&mut fixture.commands).is_empty());
        proxy_state(&fixture, fixture.project, Some((ADDRESS, &cancel)), cx);
        assert_stopping(&fixture, cx);
        cx.read(|cx| {
            assert!(
                fixture.view.read(cx).error.is_some(),
                "state updates must preserve the real error"
            )
        });
    }
    proxy_state(&fixture, fixture.project, None, cx);
    assert_stopped(&fixture, cx);
    cx.read(|cx| {
        assert!(
            fixture.view.read(cx).error.is_some(),
            "a stopped ACK does not undo an operation failure"
        )
    });

    cx.update_window(settings, |_, window, cx| {
        window.click(("settings-listener-toggle", 0usize), cx)
    })
    .unwrap();
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::StartProxy { project, .. }] if *project == fixture.project
    ));
    fixture
        .events
        .try_send(Event::Error {
            project: Some(fixture.project),
            operation: "start_proxy",
            message: "synthetic address already in use".into(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert!(!view.proxy_busy, "a failed Start is not a pending Stop");
        assert!(view.proxy_address.is_none());
        assert!(
            view.error
                .as_deref()
                .unwrap()
                .contains("address already in use")
        );
    })
    .unwrap();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find(("settings-listener-toggle", 0usize)).visible());
        assert_eq!(
            window.find(("settings-listener-toggle", 0usize)).disabled(),
            None
        );
    })
    .unwrap();
}
