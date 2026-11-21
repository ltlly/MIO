use super::*;

#[gpui_kit::test]
fn higher_worker_revision_does_not_make_unharvested_input_uncommittable(cx: &mut TestAppContext) {
    let fixture = fixture(cx, model());
    let raw = cx.read(|cx| fixture.view.read(cx).documents[&401].state.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", raw.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("同帧最新输入", cx);
        fixture.view.update(cx, |panel, cx| {
            assert_eq!(panel.documents[&401].snapshot.contents, b"initial");
            panel
                .replace(Arc::new(changed_raw(2, 5, b"late value")), &[], window, cx)
                .unwrap();
            assert!(panel.documents[&401].pending_rebase);
            assert_eq!(raw.read(cx).value().as_ref(), "同帧最新输入");
        });
        window.click(("plugin-button", 2u64), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let events = fixture.events.borrow();
    let captured = events
        .iter()
        .find_map(|event| {
            matches!(event.event, Some(ui_event::Event::Click(_))).then(|| {
                event
                    .editors
                    .iter()
                    .find(|editor| editor.document_id == 401)
                    .unwrap()
            })
        })
        .unwrap();
    assert_eq!(captured.revision, 6);
    assert_eq!(captured.contents, "同帧最新输入".as_bytes());
    cx.read(|cx| assert!(!fixture.view.read(cx).documents[&401].pending_rebase));
}

#[gpui_kit::test]
fn layout_removal_during_native_change_keeps_a_copyable_recovery_editor(cx: &mut TestAppContext) {
    let fixture = fixture(cx, model());
    let raw = cx.read(|cx| fixture.view.read(cx).documents[&401].state.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.click(("input", raw.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.input("尚未派送的中文🙂", cx);
        fixture.view.update(cx, |panel, cx| {
            assert_eq!(panel.documents[&401].snapshot.contents, b"initial");
            panel
                .replace(
                    Arc::new(PanelSnapshot {
                        revision: 2,
                        title: "编辑器已移除".into(),
                        root: Some(editor(5, 902, &[0, 255], EditorFormat::Binary)),
                    }),
                    &[],
                    window,
                    cx,
                )
                .unwrap();
            assert!(!panel.documents[&401].live);
            assert!(panel.has_unsaved_edits(cx));
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(("plugin-recover", 401u64), cx);
        window.click(("input", raw.entity_id()), cx);
        window.press(crate::kbd!("a"), cx);
        window.press(crate::kbd!("c"), cx);
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            "尚未派送的中文🙂"
        );
        window.input("不能修改恢复内容", cx);
        assert_eq!(raw.read(cx).value().as_ref(), "尚未派送的中文🙂");
    })
    .unwrap();
}

#[gpui_kit::test]
fn removed_marked_text_and_incomplete_hex_keep_the_native_recovery_buffer(cx: &mut TestAppContext) {
    for (id, value, marked) in [(401, "zhong", true), (902, "f", false)] {
        let fixture = fixture(cx, model());
        let state = cx.read(|cx| fixture.view.read(cx).documents[&id].state.clone());
        cx.update_window(fixture.window, |_, window, cx| {
            window.click(("input", state.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            if marked {
                state.update(cx, |state, cx| {
                    state.replace_and_mark_text_in_range(
                        None,
                        value,
                        Some(value.len()..value.len()),
                        window,
                        cx,
                    )
                });
            } else {
                window.input(value, cx);
            }
            fixture.view.update(cx, |panel, cx| {
                panel
                    .replace(
                        Arc::new(PanelSnapshot {
                            revision: 2,
                            title: "已移除的文档".into(),
                            root: Some(Node::new(
                                1,
                                "状态",
                                node::Kind::Text(panel::Text {
                                    text: "原编辑器已解除绑定".into(),
                                    tone: panel::Tone::Foreground as i32,
                                }),
                            )),
                        }),
                        &[],
                        window,
                        cx,
                    )
                    .unwrap();
                assert!(!panel.documents[&id].live);
                assert!(panel.documents[&id].unsaved);
                assert_eq!(state.read(cx).value().as_ref(), value);
                if marked {
                    assert!(
                        state
                            .update(cx, |state, cx| state.marked_text_range(window, cx))
                            .is_some()
                    );
                }
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(fixture.window, |_, window, cx| {
            window.render_frame(cx);
            window.click(("plugin-recover", id), cx);
            assert_eq!(state.read(cx).value().as_ref(), value);
            if marked {
                assert!(
                    state
                        .update(cx, |state, cx| state.marked_text_range(window, cx))
                        .is_some()
                );
            } else {
                window.click(("input", state.entity_id()), cx);
                window.press(crate::kbd!("a"), cx);
                window.press(crate::kbd!("c"), cx);
                assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), value);
            }
        })
        .unwrap();
    }
}
