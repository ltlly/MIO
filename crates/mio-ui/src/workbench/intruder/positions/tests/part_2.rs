use super::*;

#[gpui_kit::test]
fn automatic_positions_roundtrip_uses_one_command_and_one_native_undo(cx: &mut TestAppContext) {
    let (mut fixture, id, raw) = automatic_fixture(cx);
    let (operation_id, positions) = auto_request(&mut fixture, cx);
    assert_eq!(positions.len(), 2);
    fixture
        .events
        .try_send(Event::IntruderPositions {
            project: fixture.project,
            operation_id,
            positions: Ok(positions.clone()),
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(snapshot(&fixture, id, cx), (raw.clone(), positions.clone()));
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("successful placement saves one complete draft")
    };
    assert_eq!(workspace.intruder.tabs[0].spec.positions, positions);
    assert_eq!(workspace.intruder.tabs[0].spec.draft.raw, raw);
    cx.update_window(fixture.window, |_, window, cx| {
        window.press(crate::kbd!("z"), cx);
        assert!(
            fixture.view.read(cx).intruder.editors[&id]
                .request
                .state()
                .read(cx)
                .tokens()
                .is_empty()
        );
        window.press(crate::kbd!("shift-z"), cx);
    })
    .unwrap();
    assert_eq!(snapshot(&fixture, id, cx), (raw, positions));
}

#[gpui_kit::test]
fn stale_automatic_reply_cannot_overwrite_edits_selection_or_another_operation(
    cx: &mut TestAppContext,
) {
    for change in 0..4 {
        let (mut fixture, id, raw) = automatic_fixture(cx);
        let (operation_id, positions) = auto_request(&mut fixture, cx);
        fixture
            .events
            .try_send(Event::IntruderPositions {
                project: fixture.project,
                operation_id: Uuid::new_v4(),
                positions: Ok(positions.clone()),
            })
            .unwrap();
        cx.run_until_parked();
        assert!(cx.read(|cx| fixture.view.read(cx).intruder.positions_request.is_some()));
        cx.update_window(fixture.window, |_, window, cx| {
            let input = fixture.view.read(cx).intruder.editors[&id]
                .request
                .raw
                .clone();
            window.click(("input", input.entity_id()), cx);
            window.press(crate::kbd!("up"), cx);
            match change {
                0 => window.input("edited", cx),
                1 => window.press("shift-right", cx),
                2 => {
                    window.input("edited", cx);
                    window.press(crate::kbd!("z"), cx);
                }
                _ => {
                    window.press(crate::kbd!("w"), cx);
                    window.press(crate::kbd!("shift-t"), cx);
                }
            }
        })
        .unwrap();
        cx.run_until_parked();
        drain(&mut fixture.commands);
        fixture
            .events
            .try_send(Event::IntruderPositions {
                project: fixture.project,
                operation_id,
                positions: Ok(positions),
            })
            .unwrap();
        cx.run_until_parked();
        assert!(cx.read(|cx| fixture.view.read(cx).intruder.positions_request.is_none()));
        let (latest, positions) = snapshot(&fixture, id, cx);
        assert!(positions.is_empty());
        assert_eq!(
            latest,
            if change == 0 {
                [b"edited".as_slice(), raw.as_slice()].concat()
            } else {
                raw
            }
        );
        assert!(
            drain(&mut fixture.commands).is_empty(),
            "obsolete completion cannot save or send"
        );
    }
}

#[gpui_kit::test]
fn im_preedit_blocks_marker_buttons_and_view_changes_without_discarding_composition(
    cx: &mut TestAppContext,
) {
    let original = attack();
    let id = original.id;
    let mut fixture = intruder_fixture(cx, vec![original], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        let input = fixture.view.read(cx).intruder.editors[&id]
            .request
            .raw
            .clone();
        window.click(("input", input.entity_id()), cx);
        input.update(cx, |state, cx| {
            state.replace_and_mark_text_in_range(Some(0..0), "zhong", Some(5..5), window, cx);
        });
        let content = input.read(cx).content();
        for control in [
            "intruder-position-add",
            "intruder-position-clear",
            "intruder-position-auto",
            "intruder-view-hex",
        ] {
            window.click(control, cx);
            assert_eq!(input.read(cx).content(), content);
            assert!(input.update(cx, |state, cx| {
                state.marked_text_range(window, cx).is_some()
            }));
            assert_eq!(
                fixture.view.read(cx).error.as_deref(),
                Some(editing::COMPOSITION_PENDING)
            );
        }
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn raw_hex_toolbar_roundtrip_is_lossless_and_binary_raw_rejection_retains_hex(
    cx: &mut TestAppContext,
) {
    let original = attack();
    let id = original.id;
    let expected = original.spec.clone();
    let fixture = intruder_fixture(cx, vec![original], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.click("intruder-view-hex", cx);
        assert_eq!(
            fixture.view.read(cx).intruder.editors[&id].request.view,
            MessageView::Hex
        );
        window.click("intruder-view-raw", cx);
        assert_eq!(
            fixture.view.read(cx).intruder.editors[&id].request.view,
            MessageView::Raw
        );
        window.click("intruder-view-hex", cx);
        let input = fixture.view.read(cx).intruder.editors[&id]
            .request
            .hex
            .clone();
        window.click(("input", input.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("00 ff c2 a7", cx);
        window.click("intruder-view-raw", cx);
        assert_eq!(
            fixture.view.read(cx).intruder.editors[&id].request.view,
            MessageView::Hex
        );
        assert_eq!(input.read(cx).value().as_ref(), "00 ff c2 a7");
        assert!(
            fixture
                .view
                .read(cx)
                .error
                .as_ref()
                .unwrap()
                .contains("非 UTF-8")
        );
        assert_eq!(
            fixture.view.read(cx).workspace.intruder.tabs[0].spec,
            expected
        );
    })
    .unwrap();
    assert_eq!(
        snapshot(&fixture, id, cx),
        (vec![0, 255, 0xc2, 0xa7], vec![])
    );
}
