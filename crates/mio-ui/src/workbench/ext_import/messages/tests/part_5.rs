use super::*;

#[gpui_kit::test]
fn replacing_report_source_hides_focused_archive_message_without_losing_quit_shortcut(
    cx: &mut TestAppContext,
) {
    let mut fixture = archive_quit_fixture(cx, 4);
    click(&fixture, "ext-messages-report", cx);
    click(&fixture, "ext-report-refresh", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryExtReport {
            project,
            query_revision,
        },
    ] = commands.as_slice()
    else {
        panic!("the real refresh click must query the current project report")
    };
    assert_eq!(*project, fixture.project);
    click(&fixture, "ext-browse-messages", cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let import = &fixture.view.read(cx).ext_import;
        assert!(import.browser_visible && import.report_loading);
        let raw = import.messages.as_ref().unwrap().payload.raw.clone();
        window.click(("input", raw.entity_id()), cx);
        window.render_frame(cx);
        assert!(raw.focus_handle(cx).is_focused(window));
        assert!(window.is_action_available(&Quit, cx));
    })
    .unwrap();

    let source = Uuid::new_v4();
    fixture
        .events
        .try_send(Event::ExtReport {
            project: fixture.project,
            query_revision: *query_revision,
            result: Ok(Some(ExtSourceInfo {
                id: source,
                report: report(),
            })),
        })
        .unwrap();
    pump_native(cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let import = &fixture.view.read(cx).ext_import;
        assert!(import.visible && !import.browser_visible);
        assert!(import.messages.is_none());
        assert_eq!(import.report.as_ref().unwrap().id, source);
        assert!(!import.report_loading);
        window.press("cmd-q", cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(
        cx.has_pending_prompt(),
        "Cmd-Q must reach the exit confirmation after a refreshed source removes the focused archive editor"
    );
    cx.simulate_prompt_answer("取消");
    assert!(
        !cx.has_pending_prompt(),
        "one key press must open only one confirmation"
    );
    pump_native(cx);
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn archive_browser_actions_cannot_send_or_mutate_a_hidden_repeater(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("隐藏但必须保留")], 64);
    cx.update(|cx| {
        cx.bind_keys([
            KeyBinding::new("cmd-enter", SendRequest, Some("MioWorkspace")),
            KeyBinding::new("cmd-t", NewTab, Some("MioWorkspace")),
            KeyBinding::new("cmd-w", CloseTab, Some("MioWorkspace")),
        ])
    });
    click(&fixture, "tool-repeater", cx);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let (source, revision) = open(&mut fixture, cx);
    let message = summary(source, 0, 4);
    page(
        &fixture,
        source,
        revision,
        None,
        0,
        1,
        vec![message.clone()],
    );
    cx.run_until_parked();
    select(&fixture, 0, cx);
    let selected = read(&mut fixture);
    assert_eq!(selected.3, 4);
    fragment(&fixture, selected, message, b"body".to_vec());
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let editor = fixture
            .view
            .read(cx)
            .ext_import
            .messages
            .as_ref()
            .unwrap()
            .payload
            .raw
            .clone();
        window.click(("input", editor.entity_id()), cx);
        window.press("cmd-enter", cx);
        window.press("cmd-t", cx);
        window.press("cmd-w", cx);
    })
    .unwrap();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "archive-only view must not send or change an invisible work tab"
    );
    cx.read(|cx| assert_eq!(fixture.view.read(cx).workspace, original));
}
