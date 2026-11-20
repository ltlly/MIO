use super::*;

#[gpui_kit::test]
fn real_button_captures_latest_unicode_and_binary_without_worker_echo(cx: &mut TestAppContext) {
    let fixture = fixture(cx, model());
    let input = cx.read(|cx| fixture.view.read(cx).documents[&401].state.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", input.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("最新中文🙂", cx);
        window.click(("plugin-button", 2u64), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let events = fixture.events.borrow();
    let click = events
        .iter()
        .find(|event| matches!(event.event, Some(ui_event::Event::Click(_))))
        .unwrap();
    assert_eq!(click.editors.len(), 2);
    assert_eq!(
        click
            .editors
            .iter()
            .find(|editor| editor.document_id == 401)
            .unwrap()
            .contents,
        "最新中文🙂".as_bytes()
    );
    assert_eq!(
        click
            .editors
            .iter()
            .find(|editor| editor.document_id == 902)
            .unwrap()
            .contents,
        [0, 255]
    );
}

#[gpui_kit::test]
fn button_with_marked_text_or_invalid_hex_emits_no_partial_commit(cx: &mut TestAppContext) {
    let fixture = fixture(cx, model());
    let raw = cx.read(|cx| fixture.view.read(cx).documents[&401].state.clone());
    let hex = cx.read(|cx| fixture.view.read(cx).documents[&902].state.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", raw.entity_id()), cx);
        raw.update(cx, |state, cx| {
            state.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
        });
        window.render_frame(cx);
        window.click(("plugin-button", 2u64), cx);
        assert!(
            raw.update(cx, |state, cx| state.marked_text_range(window, cx))
                .is_some()
        );
        assert_eq!(
            fixture.view.read(cx).error.as_deref(),
            Some(editing::COMPOSITION_PENDING)
        );
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        !fixture
            .events
            .borrow()
            .iter()
            .any(|event| matches!(event.event, Some(ui_event::Event::Click(_))))
    );
    cx.update_window(fixture.window, |_, window, cx| {
        raw.update(cx, |state, cx| state.set_value("已确认", window, cx));
        window.click(("input", hex.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("0", cx);
        window.click(("plugin-button", 2u64), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        !fixture
            .events
            .borrow()
            .iter()
            .any(|event| matches!(event.event, Some(ui_event::Event::Click(_))))
    );
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).documents[&902].snapshot.contents,
            [0, 255]
        )
    });
}

#[gpui_kit::test]
fn late_panel_keeps_native_entity_and_newer_edit_then_disable_keeps_copyable_bytes(
    cx: &mut TestAppContext,
) {
    let snapshot = model();
    let fixture = fixture(cx, snapshot.clone());
    let raw = cx.read(|cx| fixture.view.read(cx).documents[&401].state.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", raw.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("local newest", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |panel, cx| {
            let mut late = snapshot;
            late.revision = 2;
            panel.replace(Arc::new(late), &[], window, cx).unwrap();
            assert_eq!(panel.documents[&401].state.entity_id(), raw.entity_id());
            assert_eq!(raw.read(cx).value().as_ref(), "local newest");
            panel.set_enabled(false, cx);
            assert!(panel.capture_editor(401, window, cx).is_err());
        });
        window.render_frame(cx);
        window.click(("input", raw.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.press(crate::kbd!("c"), cx);
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            "local newest"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn unrelated_panel_cannot_replace_harvested_unsaved_document(cx: &mut TestAppContext) {
    let fixture = fixture(cx, model());
    let raw = cx.read(|cx| fixture.view.read(cx).documents[&401].state.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", raw.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("local newest", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |panel, cx| {
            let document = &panel.documents[&401];
            assert!(document.unsaved);
            assert_eq!(document.snapshot.revision, 2);
            assert_eq!(document.snapshot.contents, b"local newest");
            let mut root = editor(
                4,
                401,
                b"older asynchronous worker result",
                EditorFormat::Utf8,
            );
            let node::Kind::Editor(editor) = root.kind.as_mut().unwrap() else {
                unreachable!()
            };
            editor.revision = 3;
            panel
                .replace(
                    Arc::new(PanelSnapshot {
                        revision: 2,
                        title: "无关联的面板更新".into(),
                        root: Some(root),
                    }),
                    &[],
                    window,
                    cx,
                )
                .unwrap();
            assert_eq!(raw.read(cx).value().as_ref(), "local newest");
            assert_eq!(panel.documents[&401].snapshot.revision, 4);
            assert_eq!(
                panel.capture_editor(401, window, cx).unwrap().contents,
                b"local newest"
            );
            assert!(panel.has_unsaved_edits(cx));
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(fixture.events.borrow().iter().zip(fixture.revisions.borrow().iter()).any(
        |(event, revision)| matches!(&event.event, Some(ui_event::Event::EditorChanged(editor))
            if *revision == 2 && editor.revision == 4 && editor.contents == b"local newest")
    ));
}

#[gpui_kit::test]
fn only_the_correlated_latest_editor_base_can_transform_dirty_input(cx: &mut TestAppContext) {
    let fixture = fixture(cx, model());
    let raw = cx.read(|cx| fixture.view.read(cx).documents[&401].state.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", raw.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("first native input", cx);
        window.click(("plugin-button", 2u64), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let bases = fixture
        .events
        .borrow()
        .iter()
        .find_map(|event| {
            matches!(event.event, Some(ui_event::Event::Click(_))).then(|| event.editors.clone())
        })
        .unwrap();
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |panel, cx| {
            panel
                .replace(
                    Arc::new(changed_raw(2, 3, b"transformed")),
                    &bases,
                    window,
                    cx,
                )
                .unwrap();
            assert_eq!(raw.read(cx).value().as_ref(), "transformed");
            assert!(
                panel.has_unsaved_edits(cx),
                "a transform is not a durable save acknowledgement"
            );
        });
        window.render_frame(cx);
        window.click(("input", raw.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("new input while waiting", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |panel, cx| {
            panel
                .replace(
                    Arc::new(changed_raw(3, 5, b"late transform")),
                    &bases,
                    window,
                    cx,
                )
                .unwrap();
            assert_eq!(raw.read(cx).value().as_ref(), "new input while waiting");
            let latest = panel.capture_editor(401, window, cx).unwrap();
            assert_eq!(latest.revision, 6);
            let mut mismatched = latest.clone();
            mismatched.contents = b"same revision, different branch".to_vec();
            panel
                .replace(
                    Arc::new(changed_raw(4, 7, b"wrong base contents")),
                    &[mismatched],
                    window,
                    cx,
                )
                .unwrap();
            assert_eq!(raw.read(cx).value().as_ref(), "new input while waiting");
            let latest = panel.capture_editor(401, window, cx).unwrap();
            assert_eq!(latest.revision, 8);
            panel
                .replace(
                    Arc::new(changed_raw(5, 9, b"latest correlated transform")),
                    &[latest],
                    window,
                    cx,
                )
                .unwrap();
            assert_eq!(raw.read(cx).value().as_ref(), "latest correlated transform");
            assert!(panel.has_unsaved_edits(cx));
        });
    })
    .unwrap();
}
