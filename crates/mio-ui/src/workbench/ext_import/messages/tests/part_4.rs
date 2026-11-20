use super::*;

#[gpui_kit::test]
fn paging_focused_archive_fragment_keeps_quit_shortcut_routed_while_loading(
    cx: &mut TestAppContext,
) {
    let mut fixture = archive_quit_fixture(cx, FRAGMENT_BYTES as u64 + 4);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let raw = fixture
            .view
            .read(cx)
            .ext_import
            .messages
            .as_ref()
            .unwrap()
            .payload
            .raw
            .clone();
        window.click(("input", raw.entity_id()), cx);
        window.render_frame(cx);
        assert!(raw.focus_handle(cx).is_focused(window));
        assert!(window.is_action_available(&Quit, cx));
        window.click("ext-fragment-next", cx);
        window.render_frame(cx);
        assert!(
            fixture
                .view
                .read(cx)
                .ext_import
                .messages
                .as_ref()
                .unwrap()
                .detail
                .is_none()
        );
        window.press(crate::kbd!("q"), cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::ReadExtMessage { project, offset, limit: 4, .. }]
            if *project == fixture.project && *offset == FRAGMENT_BYTES as u64
    ));
    assert!(
        cx.has_pending_prompt(),
        "Cmd-Q must reach the exit confirmation while the focused editor is hidden for a fragment read"
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
fn disconnecting_backend_hides_focused_archive_message_without_losing_quit_shortcut(
    cx: &mut TestAppContext,
) {
    let mut fixture = archive_quit_fixture(cx, 4);
    click(&fixture, "ext-messages-report", cx);
    click(&fixture, "ext-choose-source", cx);
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files);
        assert!(!options.directories && !options.multiple);
        Some(vec![PathBuf::from("/tmp/mio-ui-ext-focus-synthetic.archive")])
    });
    pump_native(cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::InspectExternal {
            project, cancel, ..
        },
    ] = commands.as_slice()
    else {
        panic!("the native picker must start one read-only inspection")
    };
    assert_eq!(*project, fixture.project);
    assert!(!cancel.is_cancelled());
    click(&fixture, "ext-browse-messages", cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let import = &fixture.view.read(cx).ext_import;
        assert!(import.browser_visible && import.job.is_some());
        let raw = import.messages.as_ref().unwrap().payload.raw.clone();
        window.click(("input", raw.entity_id()), cx);
        window.render_frame(cx);
        assert!(raw.focus_handle(cx).is_focused(window));
        assert!(window.is_action_available(&Quit, cx));
    })
    .unwrap();

    let (replacement, _) = mpsc::channel(1);
    drop(std::mem::replace(&mut fixture.events, replacement));
    pump_native(cx);
    assert!(cancel.is_cancelled());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let import = &fixture.view.read(cx).ext_import;
        assert!(import.visible && !import.browser_visible);
        assert!(import.messages.is_some());
        assert!(!import.blocks_transition());
        assert!(matches!(import.result, Some(ExtJobOutcome::Failed { .. })));
        window.press(crate::kbd!("q"), cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(
        cx.has_pending_prompt(),
        "Cmd-Q must reach the exit confirmation after service loss hides the focused archive editor"
    );
    cx.simulate_prompt_answer("取消");
    assert!(
        !cx.has_pending_prompt(),
        "one key press must open only one confirmation"
    );
    pump_native(cx);
    assert!(drain(&mut fixture.commands).is_empty());
}
