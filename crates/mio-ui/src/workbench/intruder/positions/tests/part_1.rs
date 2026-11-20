use super::*;

#[test]
fn raw_and_hex_snapshots_remove_only_identified_tokens_and_keep_every_byte() {
    let text = "字面§🙂\r\n二进制之外";
    let marks = vec![
        mark("first", 0, "字面".len()),
        mark("insert", text.len(), text.len()),
    ];
    for view in [MessageView::Raw, MessageView::Hex] {
        let content = content_for(text.as_bytes(), &marks, view).unwrap();
        assert_eq!(content.tokens().len(), 4);
        let (bytes, restored) = snapshot_content(&content, view).unwrap();
        assert_eq!(bytes, text.as_bytes());
        assert_eq!(restored, marks);
    }
    let mut bytes: Vec<u8> = (0..=255).collect();
    bytes.extend_from_slice("§".as_bytes());
    let marks = vec![
        mark("all", 0, bytes.len()),
        mark("at-end", bytes.len(), bytes.len()),
    ];
    let content = content_for(&bytes, &marks, MessageView::Hex).unwrap();
    assert_eq!(
        snapshot_content(&content, MessageView::Hex).unwrap(),
        (bytes.clone(), marks)
    );
    assert!(content_for(&bytes, &[], MessageView::Raw).is_err());
    let literal = InputContent::new("§payload§");
    assert_eq!(
        snapshot_content(&literal, MessageView::Raw).unwrap(),
        ("§payload§".as_bytes().to_vec(), vec![])
    );
}

#[test]
fn invalid_or_crossing_token_pairs_and_half_hex_bytes_are_not_runnable_snapshots() {
    let unmatched = InputContent::new("§value")
        .with_token(0..2, marker("a", true))
        .unwrap();
    assert!(
        snapshot_content(&unmatched, MessageView::Raw)
            .unwrap_err()
            .contains("未配对")
    );
    let crossing = InputContent::new("§a§b§c§")
        .with_token(0..2, marker("a", true))
        .unwrap()
        .with_token(3..5, marker("b", true))
        .unwrap()
        .with_token(6..8, marker("a", false))
        .unwrap()
        .with_token(9..11, marker("b", false))
        .unwrap();
    assert!(
        snapshot_content(&crossing, MessageView::Raw)
            .unwrap_err()
            .contains("未配对")
    );
    let partial_hex = pair_content("f", "one").unwrap();
    assert!(
        snapshot_content(&partial_hex, MessageView::Hex)
            .unwrap_err()
            .contains("完整 Hex 字节")
    );
}

#[gpui_kit::test]
fn add_is_one_native_undo_redo_and_literal_section_signs_survive_transfer(cx: &mut TestAppContext) {
    let text = "POST /?literal=§ HTTP/1.1\r\nHost: fixture.invalid\r\n\r\n中文🙂";
    let mut original = attack();
    original.spec.draft.raw = text.as_bytes().to_vec();
    original.spec.positions.clear();
    let id = original.id;
    let fixture = intruder_fixture(cx, vec![original], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        let input = fixture.view.read(cx).intruder.editors[&id]
            .request
            .raw
            .clone();
        window.click(("input", input.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.click("intruder-position-add", cx);
        assert_eq!(input.read(cx).value().as_ref(), format!("§{text}§"));
        assert_eq!(input.read(cx).tokens().len(), 2);
        window.press(crate::kbd!("z"), cx);
        assert_eq!(input.read(cx).value().as_ref(), text);
        assert!(input.read(cx).tokens().is_empty());
        window.press(crate::kbd!("shift-z"), cx);
        assert_eq!(input.read(cx).value().as_ref(), format!("§{text}§"));
        assert_eq!(input.read(cx).tokens().len(), 2);
        window.press(crate::kbd!("r"), cx);
        assert!(fixture.view.read(cx).tool == Tool::Repeater);
        assert_eq!(
            fixture
                .view
                .read(cx)
                .workspace
                .tabs
                .last()
                .unwrap()
                .draft
                .raw,
            text.as_bytes()
        );
    })
    .unwrap();
    assert_eq!(
        snapshot(&fixture, id, cx),
        (
            text.as_bytes().to_vec(),
            vec![Position {
                start: 0,
                end: text.len()
            }]
        )
    );
}

#[gpui_kit::test]
fn prefix_edits_move_markers_by_utf8_bytes_and_clear_is_one_undo(cx: &mut TestAppContext) {
    let mut original = attack();
    let id = original.id;
    let raw = b"one two".to_vec();
    let positions = vec![Position { start: 0, end: 3 }, Position { start: 4, end: 7 }];
    original.spec.draft.raw = raw.clone();
    original.spec.positions = positions.clone();
    let fixture = intruder_fixture(cx, vec![original], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        let input = fixture.view.read(cx).intruder.editors[&id]
            .request
            .raw
            .clone();
        window.click(("input", input.entity_id()), cx);
        window.press(crate::kbd!("up"), cx);
        cx.write_to_clipboard(ClipboardItem::new_string("前🙂§".into()));
        window.press(crate::kbd!("v"), cx);
        assert_eq!(
            input.read(cx).tokens().len(),
            4,
            "pasted literal § has no token identity"
        );
        window.click("intruder-position-clear", cx);
        assert_eq!(input.read(cx).value().as_ref(), "前🙂§one two");
        assert!(input.read(cx).tokens().is_empty());
        window.press(crate::kbd!("z"), cx);
        assert_eq!(input.read(cx).value().as_ref(), "前🙂§§one§ §two§");
        assert_eq!(input.read(cx).tokens().len(), 4);
    })
    .unwrap();
    let shift = "前🙂§".len();
    assert_eq!(
        snapshot(&fixture, id, cx),
        (
            "前🙂§one two".as_bytes().to_vec(),
            positions
                .into_iter()
                .map(|position| Position {
                    start: position.start + shift,
                    end: position.end + shift
                })
                .collect(),
        )
    );
}

#[gpui_kit::test]
fn binary_hex_selection_add_clear_and_undo_preserve_the_complete_request(cx: &mut TestAppContext) {
    let mut original = attack();
    let id = original.id;
    let raw = vec![0, 255, 0xc2, 0xa7, b'\r', b'\n'];
    original.spec.draft.raw = raw.clone();
    original.spec.positions.clear();
    let fixture = intruder_fixture(cx, vec![original], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        let input = fixture.view.read(cx).intruder.editors[&id]
            .request
            .hex
            .clone();
        window.click(("input", input.entity_id()), cx);
        window.press(crate::kbd!("up"), cx);
        window.press("shift-right", cx);
        window.press("shift-right", cx);
        window.click("intruder-position-add", cx);
        assert_eq!(input.read(cx).value().as_ref(), "§00§ ff c2 a7 0d 0a");
        window.click("intruder-position-clear", cx);
        assert_eq!(input.read(cx).value().as_ref(), "00 ff c2 a7 0d 0a");
        window.press(crate::kbd!("z"), cx);
        assert_eq!(input.read(cx).tokens().len(), 2);
    })
    .unwrap();
    assert_eq!(
        snapshot(&fixture, id, cx),
        (raw, vec![Position { start: 0, end: 1 }])
    );
}

#[gpui_kit::test]
fn view_roundtrip_preserves_markers_and_the_unchanged_raw_native_history(cx: &mut TestAppContext) {
    let original = attack();
    let id = original.id;
    let fixture = intruder_fixture(cx, vec![original], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        let input = fixture.view.read(cx).intruder.editors[&id]
            .request
            .raw
            .clone();
        window.click(("input", input.entity_id()), cx);
        window.press(crate::kbd!("up"), cx);
        window.input("§", cx);
        let before = input.read(cx).content();
        fixture.view.update(cx, |view, cx| {
            let request = &mut view.intruder.editors.get_mut(&id).unwrap().request;
            request.switch(MessageView::Hex, window, cx).unwrap();
            assert!(!request.hex.read(cx).tokens().is_empty());
            request.switch(MessageView::Raw, window, cx).unwrap();
        });
        assert_eq!(input.read(cx).content(), before);
        window.press(crate::kbd!("z"), cx);
        assert_eq!(input.read(cx).tokens().len(), 2);
        assert!(!input.read(cx).value().starts_with('§'));
    })
    .unwrap();
}

#[gpui_kit::test]
fn invalid_saved_ranges_survive_save_until_explicit_clear_without_changing_raw(
    cx: &mut TestAppContext,
) {
    let mut original = attack();
    let id = original.id;
    original.spec.positions = vec![Position { start: 900, end: 1 }];
    let expected = original.spec.clone();
    let mut fixture = intruder_fixture(cx, vec![original], 64);
    assert_eq!(
        snapshot(&fixture, id, cx),
        (expected.draft.raw.clone(), expected.positions)
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.click("intruder-position-add", cx);
        assert!(
            fixture
                .view
                .read(cx)
                .error
                .as_ref()
                .unwrap()
                .contains("范围无效")
        );
        window.click("intruder-position-clear", cx);
    })
    .unwrap();
    assert_eq!(
        snapshot(&fixture, id, cx),
        (expected.draft.raw.clone(), vec![])
    );
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("explicit repair saves once")
    };
    assert_eq!(
        workspace.intruder.tabs[0].spec.draft.raw,
        expected.draft.raw
    );
    assert!(workspace.intruder.tabs[0].spec.positions.is_empty());
}
