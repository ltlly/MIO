use super::*;

#[gpui_kit::test]
fn project_switch_releases_old_listener_and_stale_events_cannot_cancel_the_new_one(
    cx: &mut TestAppContext,
) {
    for stop_before_switch in [false, true] {
        let mut fixture = fixture(cx, vec![], 4);
        let settings = ensure_proxy_settings(&fixture, cx);
        drain(&mut fixture.commands);
        cx.update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            window.click(("settings-listener-toggle", 0usize), cx)
        })
        .unwrap();
        assert!(matches!(
            drain(&mut fixture.commands).as_slice(),
            [Command::StartProxy { project, .. }] if *project == fixture.project
        ));
        let previous = fixture.project;
        let previous_cancel = CancellationToken::new();
        proxy_state(&fixture, previous, Some((ADDRESS, &previous_cancel)), cx);
        if stop_before_switch {
            cx.update_window(settings, |_, window, cx| {
                window.render_frame(cx);
                window.click(("settings-listener-toggle", 0usize), cx)
            })
            .unwrap();
            assert_stop_command(&mut fixture);
        }
        let current = ProjectKey {
            generation: previous.generation + 1,
            ..previous
        };
        fixture
            .events
            .try_send(Event::ProjectOpened {
                project: current,
                path: None,
                workspace: Workspace::default(),
            })
            .unwrap();
        cx.run_until_parked();
        fixture.project = current;
        drain(&mut fixture.commands);
        assert!(
            previous_cancel.is_cancelled(),
            "project replacement must release the previous token"
        );
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            let view = fixture.view.read(cx);
            assert_eq!(view.project, Some(current));
            assert!(
                view.proxy_address.is_none(),
                "old listener state cannot belong to the new project"
            );
            assert!(!view.proxy_busy);
        })
        .unwrap();
        cx.update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            window.click(("settings-listener-toggle", 0usize), cx);
        })
        .unwrap();
        assert!(matches!(
            drain(&mut fixture.commands).as_slice(),
            [Command::StartProxy { project, .. }] if *project == current
        ));
        let current_cancel = CancellationToken::new();
        proxy_state(
            &fixture,
            current,
            Some(("127.0.0.1:9003", &current_cancel)),
            cx,
        );
        let stale_cancel = CancellationToken::new();
        fixture
            .events
            .try_send(Event::ProxyState {
                project: previous,
                address: Some(ADDRESS.parse().unwrap()),
                cancel: Some(stale_cancel.clone()),
                intercept_requests: true,
                intercept_responses: true,
            })
            .unwrap();
        fixture
            .events
            .try_send(Event::Error {
                project: Some(previous),
                operation: "stop_proxy",
                message: "stale project error".into(),
            })
            .unwrap();
        proxy_state(&fixture, previous, None, cx);
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            let view = fixture.view.read(cx);
            assert_eq!(view.proxy_address, Some("127.0.0.1:9003".parse().unwrap()));
            assert!(!view.proxy_busy);
            assert!(!view.intercept_requests && !view.intercept_responses);
            assert!(view.error.is_none());
            assert!(!current_cancel.is_cancelled());
            assert!(!stale_cancel.is_cancelled());
        })
        .unwrap();
        cx.update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            window.click(("settings-listener-toggle", 0usize), cx);
            assert!(
                current_cancel.is_cancelled(),
                "Stop must retain the current listener token"
            );
            assert!(
                !stale_cancel.is_cancelled(),
                "an ignored old event conveys no cancellation authority"
            );
        })
        .unwrap();
        assert_stop_command(&mut fixture);
        proxy_state(&fixture, previous, None, cx);
        assert_stopping(&fixture, cx);
        proxy_state(&fixture, current, None, cx);
        assert_stopped(&fixture, cx);
    }
}

#[gpui_kit::test]
fn backend_disconnect_cancels_the_live_listener_without_claiming_it_has_stopped(
    cx: &mut TestAppContext,
) {
    for stopped_ack in [false, true] {
        let mut fixture = fixture(cx, vec![], 4);
        let settings = ensure_proxy_settings(&fixture, cx);
        drain(&mut fixture.commands);
        cx.update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            window.click(("settings-listener-toggle", 0usize), cx)
        })
        .unwrap();
        assert!(matches!(
            drain(&mut fixture.commands).as_slice(),
            [Command::StartProxy { project, .. }] if *project == fixture.project
        ));
        let cancel = CancellationToken::new();
        proxy_state(&fixture, fixture.project, Some((ADDRESS, &cancel)), cx);
        assert!(!cancel.is_cancelled());
        if stopped_ack {
            cx.update_window(settings, |_, window, cx| {
                window.render_frame(cx);
                window.click(("settings-listener-toggle", 0usize), cx)
            })
            .unwrap();
            assert_stop_command(&mut fixture);
            proxy_state(&fixture, fixture.project, None, cx);
            assert_stopped(&fixture, cx);
        }
        let stale_cancel = CancellationToken::new();
        proxy_state(
            &fixture,
            ProjectKey {
                generation: 0,
                ..fixture.project
            },
            Some(("127.0.0.1:9999", &stale_cancel)),
            cx,
        );
        let (replacement, _) = mpsc::channel(1);
        drop(std::mem::replace(&mut fixture.events, replacement));
        cx.run_until_parked();
        assert!(
            cancel.is_cancelled(),
            "service loss must cancel the current listener token"
        );
        assert!(!stale_cancel.is_cancelled());
        if !stopped_ack {
            assert_stopping(&fixture, cx);
        }
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            let view = fixture.view.read(cx);
            assert!(view.backend_disconnected);
            assert!(view.error.as_deref().unwrap().contains("后台服务已断开"));
            assert_eq!(view.proxy_address.is_none(), stopped_ack);
            assert_eq!(view.proxy_busy, !stopped_ack);
            assert!(rendered_status(window).contains("后台已断开"));
            window.click("dismiss-error", cx);
        })
        .unwrap();
        cx.update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find(("settings-listener-toggle", 0usize)).disabled(),
                Some(true)
            );
            window.click(("settings-listener-toggle", 0usize), cx);
        })
        .unwrap();
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(fixture.view.read(cx).proxy_busy, !stopped_ack);
            assert!(rendered_status(window).contains("后台已断开"));
        })
        .unwrap();
        if !stopped_ack {
            assert_stopping(&fixture, cx);
        }
        assert!(
            drain(&mut fixture.commands).is_empty(),
            "service loss must not enqueue or retry requests"
        );
    }
}

#[gpui_kit::test]
fn automatic_history_refresh_preserves_stop_errors_and_selected_details(cx: &mut TestAppContext) {
    for stopped_ack in [false, true] {
        let (mut fixture, cancel) = listening_fixture(cx, 8);
        let settings = ensure_proxy_settings(&fixture, cx);
        let [
            Command::QueryHttpHistory {
                query: mio_core::HttpHistoryQuery { query_revision, .. },
            },
        ]: [Command; 1] = drain(&mut fixture.commands)
            .try_into()
            .unwrap_or_else(|_| panic!("expected exactly one command"))
        else {
            panic!("initial history query")
        };
        let mut captured = record("http://127.0.0.1:9001/once", "retained response");
        fixture
            .events
            .try_send(Event::HistoryPage {
                project: fixture.project,
                query_revision,
                offset: 0,
                total: 1,
                rows: vec![captured.summary.clone().into()],
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            assert!(!fixture.view.read(cx).history_loading);
            window.click("proxy-history", cx);
            window.click_at("table", point(px(90.), px(theme::ROW_HEIGHT * 1.5)), cx);
        })
        .unwrap();
        let [
            Command::ReadFlow {
                selection_revision,
                flow_id,
                ..
            },
        ]: [Command; 1] = drain(&mut fixture.commands)
            .try_into()
            .unwrap_or_else(|_| panic!("expected exactly one command"))
        else {
            panic!("selecting the actual row requests its detail")
        };
        assert_eq!(flow_id, captured.summary.id);
        fixture
            .events
            .try_send(Event::FlowDetail {
                project: fixture.project,
                selection_revision,
                record: Box::new(captured.clone()),
                original: Box::default(),
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            window.click(("settings-listener-toggle", 0usize), cx);
        })
        .unwrap();
        assert_stop_command(&mut fixture);
        fixture
            .events
            .try_send(Event::HistoryChanged {
                project: fixture.project,
            })
            .unwrap();
        fixture
            .events
            .try_send(Event::ProxyState {
                project: fixture.project,
                address: (!stopped_ack).then(|| ADDRESS.parse().unwrap()),
                cancel: (!stopped_ack).then(|| cancel.clone()),
                intercept_requests: false,
                intercept_responses: false,
            })
            .unwrap();
        fixture
            .events
            .try_send(Event::Error {
                project: Some(fixture.project),
                operation: "stop_proxy",
                message: "synthetic cleanup failure".into(),
            })
            .unwrap();
        cx.run_until_parked();
        let expected = "stop_proxy：synthetic cleanup failure";
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            let view = fixture.view.read(cx);
            assert_eq!(view.error.as_deref(), Some(expected));
            assert_eq!(view.proxy_busy, !stopped_ack);
            assert_eq!(view.proxy_address.is_none(), stopped_ack);
            assert!(window.find("dismiss-error").visible());
        })
        .unwrap();
        let [
            Command::QueryHttpHistory {
                query:
                    mio_core::HttpHistoryQuery {
                        query_revision: refresh,
                        ..
                    },
            },
        ]: [Command; 1] = drain(&mut fixture.commands)
            .try_into()
            .unwrap_or_else(|_| panic!("expected exactly one command"))
        else {
            panic!("the automatic refresh must still be sent")
        };
        assert!(refresh > query_revision);
        captured.summary.revision += 1;
        captured.summary.state = FlowState::Cancelled;
        fixture
            .events
            .try_send(Event::HistoryPage {
                project: fixture.project,
                query_revision: refresh,
                offset: 0,
                total: 1,
                rows: vec![captured.summary.clone().into()],
            })
            .unwrap();
        cx.run_until_parked();
        let [
            Command::ReadFlow {
                selection_revision: refreshed_selection,
                flow_id,
                ..
            },
        ]: [Command; 1] = drain(&mut fixture.commands)
            .try_into()
            .unwrap_or_else(|_| panic!("expected exactly one command"))
        else {
            panic!("the changed selected flow must refresh its detail")
        };
        assert_eq!(flow_id, captured.summary.id);
        assert!(refreshed_selection > selection_revision);
        cx.read(|cx| assert_eq!(fixture.view.read(cx).error.as_deref(), Some(expected)));
        fixture
            .events
            .try_send(Event::FlowDetail {
                project: fixture.project,
                selection_revision: refreshed_selection,
                record: Box::new(captured.clone()),
                original: Box::default(),
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            let view = fixture.view.read(cx);
            assert_eq!(view.error.as_deref(), Some(expected));
            assert_eq!(view.detail.as_deref(), Some(&captured));
            assert!(window.find("dismiss-error").visible());
            window.click("dismiss-error", cx);
            assert!(fixture.view.read(cx).error.is_none());
        })
        .unwrap();
        assert!(drain(&mut fixture.commands).is_empty());
    }
}
