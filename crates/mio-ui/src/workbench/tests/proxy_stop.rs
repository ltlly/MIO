use super::*;

const ADDRESS: &str = "127.0.0.1:9002";

mod part_1;
mod part_2;

fn proxy_state(
    fixture: &Fixture,
    project: ProjectKey,
    listener: Option<(&str, &CancellationToken)>,
    cx: &mut TestAppContext,
) {
    fixture
        .events
        .try_send(Event::ProxyState {
            project,
            address: listener.map(|(address, _)| address.parse().unwrap()),
            cancel: listener.map(|(_, cancel)| cancel.clone()),
            intercept_requests: false,
            intercept_responses: false,
        })
        .unwrap();
    cx.run_until_parked();
}

fn ensure_proxy_settings(fixture: &Fixture, cx: &mut TestAppContext) -> AnyWindowHandle {
    if let Some(settings) = cx.read(|cx| fixture.view.read(cx).settings_window)
        && cx.windows().contains(&settings)
    {
        return settings;
    }
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-settings", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .settings_window
            .expect("Proxy settings opens Settings window")
    })
}

fn listening_fixture(cx: &mut TestAppContext, capacity: usize) -> (Fixture, CancellationToken) {
    let fixture = fixture(cx, vec![], capacity);
    let cancel = CancellationToken::new();
    proxy_state(&fixture, fixture.project, Some((ADDRESS, &cancel)), cx);
    let settings = ensure_proxy_settings(&fixture, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    (fixture, cancel)
}

fn assert_stopping(fixture: &Fixture, cx: &mut TestAppContext) {
    let settings = ensure_proxy_settings(fixture, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("settings-listener-toggle", 0usize)).disabled(),
            Some(true)
        );
    })
    .unwrap();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert!(view.proxy_busy, "only the matching stopped ACK ends Stop");
        assert!(view.proxy_address.is_some());
        let status = rendered_status(window);
        assert!(
            status.contains("停止") && status.contains("确认"),
            "{status}"
        );
        assert!(
            !status.contains("已停止") && !status.contains("正在监听"),
            "{status}"
        );
    })
    .unwrap();
}

fn assert_stopped(fixture: &Fixture, cx: &mut TestAppContext) {
    let settings = ensure_proxy_settings(fixture, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find(("settings-listener-toggle", 0usize)).visible());
        assert_eq!(
            window.find(("settings-listener-toggle", 0usize)).disabled(),
            None
        );
    })
    .unwrap();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let view = fixture.view.read(cx);
        assert!(view.proxy_address.is_none());
        assert!(!view.proxy_busy);
        assert_eq!(rendered_status(window), "Proxy 已停止");
    })
    .unwrap();
}

fn assert_stop_command(fixture: &mut Fixture) {
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::StopProxy { project }] if *project == fixture.project
    ));
}
