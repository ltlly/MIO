use super::*;

#[gpui_kit::test]
fn switching_focused_archive_raw_to_hex_keeps_quit_shortcut_routed(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("退出前保留工作区")], 64);
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
    pump_native(cx);
    select(&fixture, 0, cx);
    let selected = read(&mut fixture);
    fragment(&fixture, selected, message, b"body".to_vec());
    pump_native(cx);
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
        window.click("ext-message-hex", cx);
        window.render_frame(cx);
        let messages = fixture.view.read(cx).ext_import.messages.as_ref().unwrap();
        assert_eq!(messages.payload.view(), MessageView::Hex);
        assert_eq!(
            messages.payload.hex.read(cx).value().as_ref(),
            "62 6f 64 79"
        );
        window.press(crate::kbd!("q"), cx);
    })
    .unwrap();
    pump_native(cx);
    assert!(
        cx.has_pending_prompt(),
        "a real Cmd-Q after switching the focused read-only editor must reach the exit confirmation"
    );
    cx.simulate_prompt_answer("保存");
    assert!(
        !cx.has_pending_prompt(),
        "one key press must open only one confirmation"
    );
    pump_native(cx);
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::Quiesce { project }] if *project == fixture.project
    ));
}

pub(in crate::workbench) fn archive_quit_fixture(
    cx: &mut TestAppContext,
    raw_bytes: u64,
) -> Fixture {
    let mut fixture = fixture(cx, vec![tab("退出前保留工作区")], 64);
    let (source, revision) = open(&mut fixture, cx);
    let message = summary(source, 0, raw_bytes);
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
    let bytes = vec![b'a'; selected.3];
    fragment(&fixture, selected, message, bytes);
    cx.run_until_parked();
    fixture
}

#[gpui_kit::test]
fn leaving_focused_archive_message_keeps_quit_shortcut_routed(cx: &mut TestAppContext) {
    let mut fixture = archive_quit_fixture(cx, 4);
    let raw = cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .ext_import
            .messages
            .as_ref()
            .unwrap()
            .payload
            .raw
            .clone()
    });
    for target in ["ext-messages-report", "ext-messages-dismiss"] {
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            window.click(("input", raw.entity_id()), cx);
            window.render_frame(cx);
            assert!(raw.focus_handle(cx).is_focused(window));
            assert!(window.is_action_available(&Quit, cx));
            window.click(target, cx);
            window.render_frame(cx);
            window.press(crate::kbd!("q"), cx);
        })
        .unwrap();
        pump_native(cx);
        assert!(
            cx.has_pending_prompt(),
            "Cmd-Q must reach the exit confirmation after {target} hides the focused editor"
        );
        cx.simulate_prompt_answer("取消");
        assert!(
            !cx.has_pending_prompt(),
            "one key press must open only one confirmation"
        );
        pump_native(cx);
        assert!(drain(&mut fixture.commands).is_empty());
        if target == "ext-messages-report" {
            click(&fixture, "ext-browse-messages", cx);
        }
    }
}
