use super::*;

pub(super) fn proxy_rule_settings(cx: &mut TestAppContext) -> (Fixture, AnyWindowHandle) {
    let mut fixture = fixture(cx, Vec::new(), 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);
    proxy_rule_click(settings, "settings-proxy", cx);
    (fixture, settings)
}

pub(super) fn proxy_rule_click(
    window: AnyWindowHandle,
    id: impl Into<gpui_kit::ElementId>,
    cx: &mut TestAppContext,
) {
    let id = id.into();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        if window.try_find(id.clone()).is_some_and(|el| !el.visible())
            && let Some(anchor) = [
                "proxy-listeners-heading",
                "proxy-request-rules-heading",
                "proxy-response-rules-heading",
            ]
            .into_iter()
            .find(|&a| window.try_find(a).is_some_and(|n| n.visible()))
        {
            window.scroll(
                anchor,
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-320.))),
                cx,
            );
            window.render_frame(cx);
        }
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

pub(super) fn proxy_rule_dialog(
    settings: AnyWindowHandle,
    fixture: &Fixture,
    cx: &TestAppContext,
) -> AnyWindowHandle {
    let windows: Vec<_> = cx
        .windows()
        .into_iter()
        .filter(|window| *window != settings && *window != fixture.window)
        .collect();
    assert_eq!(windows.len(), 1);
    windows[0]
}

pub(super) fn proxy_rule_condition(
    window: AnyWindowHandle,
    condition: &str,
    cx: &mut TestAppContext,
) {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-rule-editor-condition", cx);
        window.press(crate::kbd!("a"), cx);
        window.input(condition, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

pub(super) fn proxy_rule_open_dropdown(
    dialog: AnyWindowHandle,
    id: &'static str,
    cx: &mut TestAppContext,
) {
    proxy_rule_click(dialog, id, cx);
    let open = cx
        .update_window(dialog, |_, window, cx| {
            window.render_frame(cx);
            let select = window.find(id);
            assert_eq!(select.focused(), Some(true));
            select.expanded() == Some(true)
        })
        .unwrap();
    if !open {
        cx.simulate_keystrokes(dialog, "enter");
    }
    cx.run_until_parked();
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(id).expanded(), Some(true));
    })
    .unwrap();
}

pub(super) fn proxy_rule_choose_type(
    dialog: AnyWindowHandle,
    expected: &str,
    response: bool,
    cx: &mut TestAppContext,
) {
    let mut types = vec![
        "Domain name",
        "IP address",
        "Protocol",
        "HTTP method",
        "URL",
        "File extension",
        "Request",
        "Cookie name",
        "Cookie value",
        "Any header",
        "Body",
        "Param name",
        "Param value",
    ];
    if response {
        types.extend([
            "Status code",
            "Content type header",
            "Mime type",
            "Page title",
        ]);
    }
    types.push("Listener port");
    proxy_rule_open_dropdown(dialog, "proxy-rule-editor-type", cx);
    let current = cx
        .update_window(dialog, |_, window, _| {
            let select = window.find("proxy-rule-editor-type");
            let label = select.value().unwrap();
            types
                .iter()
                .position(|expected| *expected == label)
                .unwrap()
        })
        .unwrap();
    let destination = types.iter().position(|label| *label == expected).unwrap();
    let direction = if destination < current {
        "up "
    } else {
        "down "
    };
    cx.simulate_keystrokes(
        dialog,
        &format!("{}enter", direction.repeat(destination.abs_diff(current))),
    );
    cx.run_until_parked();
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("proxy-rule-editor-type").value(),
            Some(expected)
        );
    })
    .unwrap();
}

pub(super) fn proxy_message_show_controls(settings: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        if !window.find("proxy-unpack-responses").visible() {
            let anchor = if window
                .try_find("proxy-listeners-heading")
                .is_some_and(|n| n.visible())
            {
                "proxy-listeners-heading"
            } else {
                "proxy-request-rules-heading"
            };
            window.scroll(
                anchor,
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-950.))),
                cx,
            );
        }
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        for id in ["proxy-unpack-requests", "proxy-unpack-responses"] {
            assert!(
                window.find(id).visible(),
                "{id} must be reachable on the Proxy page"
            );
        }
    })
    .unwrap();
}

pub(super) fn proxy_ws_show_controls(settings: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        if !window.find("proxy-ws-intercept-in-scope-only").visible() {
            let anchor = if window
                .try_find("proxy-listeners-heading")
                .is_some_and(|n| n.visible())
            {
                "proxy-listeners-heading"
            } else {
                "proxy-request-rules-heading"
            };
            window.scroll(
                anchor,
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-950.))),
                cx,
            );
        }
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        for id in [
            "proxy-ws-intercept-client-to-server",
            "proxy-ws-intercept-server-to-client",
            "proxy-ws-intercept-in-scope-only",
        ] {
            assert!(
                window.find(id).visible(),
                "{id} must be reachable on the Proxy page"
            );
        }
    })
    .unwrap();
}

pub(super) fn assert_proxy_rule_condition_disabled(
    dialog: AnyWindowHandle,
    cx: &mut TestAppContext,
) {
    let (before, revision) = cx
        .update_window(dialog, |_, window, cx| {
            window.render_frame(cx);
            let editor = window
                .root::<gpui_kit::base::Root>()
                .unwrap()
                .unwrap()
                .read(cx)
                .view()
                .clone()
                .downcast::<crate::workbench::settings::InterceptionEditor>()
                .unwrap();
            assert!(
                editor
                    .read(cx)
                    .condition
                    .read(cx)
                    .presentation()
                    .is_disabled()
            );
            let revision = editor.read(cx).condition.read(cx).document_revision();
            let before = window
                .find("proxy-rule-editor-condition")
                .value()
                .unwrap_or_default()
                .to_owned();
            window.click("proxy-rule-editor-condition", cx);
            window.input("must-not-edit-disabled-condition", cx);
            (before, revision)
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("proxy-rule-editor-condition").value(),
            Some(before.as_str())
        );
        let editor = window
            .root::<gpui_kit::base::Root>()
            .unwrap()
            .unwrap()
            .read(cx)
            .view()
            .clone()
            .downcast::<crate::workbench::settings::InterceptionEditor>()
            .unwrap();
        assert_eq!(
            editor.read(cx).condition.read(cx).document_revision(),
            revision,
            "disabled condition input must not advance its document revision"
        );
    })
    .unwrap();
}
