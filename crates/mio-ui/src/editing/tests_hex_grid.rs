use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Bounds, ClipboardItem, Context, Entity, IntoElement,
    KeyDownEvent, Keystroke, Modifiers, ParentElement, Point, Render, Styled, TestAppContext,
    Window, WindowBounds, WindowOptions, div, px, size,
};

use super::*;

struct HexEditorHarness {
    editor: MessageEditor,
}

impl Render for HexEditorHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("hex-harness-root")
            .size_full()
            .child(message_editor(self.editor.state()).size_full())
    }
}

fn open_hex_harness(
    initial: &[u8],
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<HexEditorHarness>) {
    cx.update(gpui_kit::init);
    let (handle, entity) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(800.), px(600.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    let mut editor = MessageEditor::new(window, cx);
                    editor.load(initial, window, cx);
                    editor.switch(MessageView::Hex, window, cx).unwrap();
                    HexEditorHarness { editor }
                })
            },
        )
        .unwrap()
    });
    cx.run_until_parked();
    (handle, entity)
}

fn simulate_key(
    key: &str,
    key_char: Option<&str>,
    modifiers: Modifiers,
    grid: &Entity<HexGridView>,
    window: &mut Window,
    cx: &mut App,
) {
    let event = KeyDownEvent {
        keystroke: Keystroke {
            modifiers,
            key: key.into(),
            key_char: key_char.map(Into::into),
        },
        is_held: false,
        prefer_character_input: false,
    };
    grid.update(cx, |g, cx| {
        g.handle_key_down_for_test(&event, window, cx);
    });
}

#[gpui_kit::test]
fn hex_grid_in_place_nibble_ascii_insert_undo_redo_copy_paste_preserve_binary_bytes(
    cx: &mut TestAppContext,
) {
    let (handle, harness) = open_hex_harness(b"\x00\x41\xFF\x80", cx);

    // Draw initial frame so MessageEditorElement mounts HexGridView and wires readonly/menu.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let grid = harness.read(cx).editor.hex_grid.clone().unwrap();
        grid.update(cx, |g, cx| {
            g.select_byte(0, false, false, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();

    // Two-nibble hex typing at index 0: 0x00 -> type 'd' (0xD0) then 'e' (0xDE).
    cx.update_window(handle, |_, window, cx| {
        let grid = harness.read(cx).editor.hex_grid.clone().unwrap();
        simulate_key("d", Some("d"), Modifiers::none(), &grid, window, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        let grid = harness.read(cx).editor.hex_grid.clone().unwrap();
        assert!(
            grid.read(cx).low_nibble_pending,
            "observer must not clear low_nibble_pending after high nibble"
        );
        assert_eq!(grid.read(cx).active_cursor, Some(0));
        simulate_key("e", Some("e"), Modifiers::none(), &grid, window, cx);
    })
    .unwrap();
    cx.run_until_parked();

    let after_hex_nibbles = cx
        .update_window(handle, |_, window, cx| {
            harness.update(cx, |v, cx| v.editor.bytes(window, cx).unwrap())
        })
        .unwrap();
    assert_eq!(after_hex_nibbles, vec![0xDE, 0x41, 0xFF, 0x80]);

    // Tab switches to ASCII column; overwrite cursor (byte 1) with 'Z'.
    cx.update_window(handle, |_, window, cx| {
        let grid = harness.read(cx).editor.hex_grid.clone().unwrap();
        simulate_key("tab", None, Modifiers::none(), &grid, window, cx);
        assert!(grid.read(cx).editing_ascii);
        simulate_key("Z", Some("Z"), Modifiers::none(), &grid, window, cx);
    })
    .unwrap();
    cx.run_until_parked();
    let after_ascii = cx
        .update_window(handle, |_, window, cx| {
            harness.update(cx, |v, cx| v.editor.bytes(window, cx).unwrap())
        })
        .unwrap();
    assert_eq!(after_ascii, vec![0xDE, b'Z', 0xFF, 0x80]);

    // Toggle insert_mode and type '!' at index 2 -> inserts 0x21 before 0xFF.
    cx.update_window(handle, |_, window, cx| {
        let grid = harness.read(cx).editor.hex_grid.clone().unwrap();
        grid.update(cx, |g, _| g.insert_mode = true);
        simulate_key("!", Some("!"), Modifiers::none(), &grid, window, cx);
    })
    .unwrap();
    cx.run_until_parked();
    let after_insert = cx
        .update_window(handle, |_, window, cx| {
            harness.update(cx, |v, cx| v.editor.bytes(window, cx).unwrap())
        })
        .unwrap();
    assert_eq!(after_insert, vec![0xDE, b'Z', b'!', 0xFF, 0x80]);

    // Undo restores before insert, redo re-applies insert.
    cx.update_window(handle, |_, window, cx| {
        let grid = harness.read(cx).editor.hex_grid.clone().unwrap();
        simulate_key(
            "z",
            None,
            Modifiers {
                control: true,
                ..Modifiers::none()
            },
            &grid,
            window,
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    let after_undo = cx
        .update_window(handle, |_, window, cx| {
            harness.update(cx, |v, cx| v.editor.bytes(window, cx).unwrap())
        })
        .unwrap();
    assert_eq!(after_undo, vec![0xDE, b'Z', 0xFF, 0x80]);

    cx.update_window(handle, |_, window, cx| {
        let grid = harness.read(cx).editor.hex_grid.clone().unwrap();
        simulate_key(
            "z",
            None,
            Modifiers {
                control: true,
                shift: true,
                ..Modifiers::none()
            },
            &grid,
            window,
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    let after_redo = cx
        .update_window(handle, |_, window, cx| {
            harness.update(cx, |v, cx| v.editor.bytes(window, cx).unwrap())
        })
        .unwrap();
    assert_eq!(after_redo, vec![0xDE, b'Z', b'!', 0xFF, 0x80]);

    // Select range 0..2, switch back to hex mode via Tab, copy Hex -> clipboard gets "de 5a".
    cx.update_window(handle, |_, window, cx| {
        let grid = harness.read(cx).editor.hex_grid.clone().unwrap();
        simulate_key("tab", None, Modifiers::none(), &grid, window, cx);
        grid.update(cx, |g, cx| {
            g.select_byte(0, false, false, window, cx);
            g.select_byte(1, true, false, window, cx);
            g.copy_selection(None, cx);
        });
    })
    .unwrap();
    let copied = cx.read(|cx| cx.read_from_clipboard().and_then(|i| i.text()));
    assert_eq!(copied.as_deref(), Some("de 5a"));

    // Write hex payload to clipboard and paste over selected 0..2.
    cx.update(|cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("CA FE BA BE".into()));
    });
    cx.update_window(handle, |_, window, cx| {
        let grid = harness.read(cx).editor.hex_grid.clone().unwrap();
        simulate_key(
            "v",
            None,
            Modifiers {
                control: true,
                ..Modifiers::none()
            },
            &grid,
            window,
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    let after_paste = cx
        .update_window(handle, |_, window, cx| {
            harness.update(cx, |v, cx| v.editor.bytes(window, cx).unwrap())
        })
        .unwrap();
    assert_eq!(
        after_paste,
        vec![0xCA, 0xFE, 0xBA, 0xBE, b'!', 0xFF, 0x80]
    );
}

#[gpui_kit::test]
fn hex_grid_empty_buffer_typing_and_raw_switch_preserve_utf8(cx: &mut TestAppContext) {
    let (handle, harness) = open_hex_harness(b"", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let grid = harness.read(cx).editor.hex_grid.clone().unwrap();
        // Enable insert mode from empty, type "4f" -> 'O' (advances cursor to end), then ASCII "K" -> "OK".
        grid.update(cx, |g, _| g.insert_mode = true);
        simulate_key("4", Some("4"), Modifiers::none(), &grid, window, cx);
        simulate_key("f", Some("f"), Modifiers::none(), &grid, window, cx);
        grid.update(cx, |g, _| g.editing_ascii = true);
        simulate_key("K", Some("K"), Modifiers::none(), &grid, window, cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(handle, |_, window, cx| {
        harness.update(cx, |view, cx| {
            assert_eq!(view.editor.bytes(window, cx).unwrap(), b"OK");
            view.editor.switch(MessageView::Raw, window, cx).unwrap();
            assert_eq!(view.editor.raw.read(cx).value().as_ref(), "OK");
        });
    })
    .unwrap();
}

#[gpui_kit::test]
fn hex_grid_emits_change_guards_end_cursor_preserves_lossless_ascii_cut_and_coalesces_undo(
    cx: &mut TestAppContext,
) {
    let (handle, harness) = open_hex_harness(&[0x41, 0xFF, 0xFE, 0x42], cx);
    let change_count = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let count_clone = change_count.clone();
    cx.update_window(handle, |_, _, cx| {
        let hex_entity = harness.read(cx).editor.hex.clone();
        cx.subscribe(&hex_entity, move |_, ev: &InputEvent, _| {
            if matches!(ev, InputEvent::Change) {
                count_clone.set(count_clone.get() + 1);
            }
        })
        .detach();
    })
    .unwrap();

    // 1. Insert byte via two nibbles at end -> 1 Undo reverts full byte; Change fires.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let grid = harness.read(cx).editor.hex_grid.clone().unwrap();
        grid.update(cx, |g, cx| {
            g.insert_mode = true;
            g.select_byte(3, false, false, window, cx);
        });
        simulate_key("end", None, Modifiers::none(), &grid, window, cx);
        // Move cursor to end-of-buffer insert slot (len = 4), type 0x7A.
        grid.update(cx, |g, _| g.active_cursor = Some(g.bytes.len()));
        simulate_key("7", Some("7"), Modifiers::none(), &grid, window, cx);
        let after_hi = grid.read(cx).bytes.clone();
        assert_eq!(after_hi, vec![0x41, 0xFF, 0xFE, 0x42, 0x70]);
        simulate_key("a", Some("a"), Modifiers::none(), &grid, window, cx);
        assert_eq!(
            grid.read(cx).bytes,
            vec![0x41, 0xFF, 0xFE, 0x42, 0x7A]
        );
        // Cursor is now at len (5) and byte_input is empty; edit_selected_byte must not panic.
        grid.update(cx, |g, cx| g.edit_selected_byte(window, cx));
        assert_eq!(
            grid.read(cx).bytes,
            vec![0x41, 0xFF, 0xFE, 0x42, 0x7B]
        );
        // Undo once reverts edit_selected_byte; second Undo reverts BOTH nibbles (0x7A) back to len 4!
        grid.update(cx, |g, cx| g.undo(window, cx));
        grid.update(cx, |g, cx| g.undo(window, cx));
        assert_eq!(grid.read(cx).bytes, vec![0x41, 0xFF, 0xFE, 0x42]);
        // Verify home/end and toggle overwrite clamp cursor safely when cursor == bytes.len().
        grid.update(cx, |g, _| g.active_cursor = Some(g.bytes.len()));
        simulate_key("home", None, Modifiers::none(), &grid, window, cx);
        assert_eq!(grid.read(cx).active_cursor, Some(0));
        grid.update(cx, |g, _| g.active_cursor = Some(g.bytes.len()));
        simulate_key("end", None, Modifiers::none(), &grid, window, cx);
        assert_eq!(grid.read(cx).active_cursor, Some(3));
        grid.update(cx, |g, cx| {
            g.active_cursor = Some(g.bytes.len());
            g.toggle_insert_mode(cx);
            assert_eq!(g.active_cursor, Some(3));
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        change_count.get() >= 1,
        "HexGridView commits must emit InputEvent::Change on underlying hex_editor"
    );

    // 2. ASCII Cut + Paste of non-UTF-8 [0xFF, 0xFE] preserves exact raw bytes.
    cx.update_window(handle, |_, window, cx| {
        let grid = harness.read(cx).editor.hex_grid.clone().unwrap();
        grid.update(cx, |g, cx| {
            g.select_byte(1, false, true, window, cx);
            g.select_byte(2, true, true, window, cx);
            g.cut_selection(window, cx);
            assert_eq!(g.bytes, vec![0x41, 0x42]);
            g.select_byte(1, false, true, window, cx);
            g.selected_range = None;
            g.active_cursor = Some(1);
            g.paste_clipboard(window, cx);
            assert_eq!(g.bytes, vec![0x41, 0xFF, 0xFE, 0x42]);
        });
    })
    .unwrap();

    // 3. External empty clear via hex_editor Change must not underflow when active_cursor = Some(0).
    cx.update_window(handle, |_, window, cx| {
        let grid = harness.read(cx).editor.hex_grid.clone().unwrap();
        grid.update(cx, |g, _| g.active_cursor = Some(0));
        let hex_entity = harness.read(cx).editor.hex.clone();
        hex_entity.update(cx, |state, cx| {
            state.replace_all("", window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let grid = harness.read(cx).editor.hex_grid.clone().unwrap();
        assert!(grid.read(cx).bytes.is_empty());
        assert_eq!(grid.read(cx).active_cursor, None);
    });
}

#[gpui_kit::test]
fn hex_grid_search_scrolls_to_matched_offset(cx: &mut TestAppContext) {
    assert_eq!(hex_text_range_to_byte_range(0..1, 4), Some(0..1));
    assert_eq!(hex_text_range_to_byte_range(3..4, 4), Some(1..2));
    assert_eq!(hex_text_range_to_byte_range(1..4, 4), Some(0..2));
    assert_eq!(hex_text_range_to_byte_range(2..3, 4), None);
    assert_eq!(
        hex_text_range_to_byte_range_in_str("41   42\n43", 5..7, 3),
        Some(1..2)
    );

    let mut payload = vec![b'A'; 4096];
    payload[3072..3076].copy_from_slice(b"FIND");
    let (handle, harness) = open_hex_harness(&payload, cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let grid = harness.read(cx).editor.hex_grid.clone().unwrap();
        // Start mid-byte edit at byte 0 (high nibble '5'); search must NOT overwrite edit cursor/nibble.
        grid.update(cx, |g, cx| {
            g.select_byte(0, false, false, window, cx);
        });
        simulate_key("5", Some("5"), Modifiers::none(), &grid, window, cx);
        assert!(grid.read(cx).low_nibble_pending);
        harness.update(cx, |view, cx| {
            view.editor
                .search
                .update(cx, |s, cx| s.set_value("46 49 4e 44", window, cx));
            view.editor.update_search(Some(true), window, cx).unwrap();
        });
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        let grid = harness.read(cx).editor.hex_grid.clone().unwrap();
        assert_eq!(grid.read(cx).search_match_range, Some(3072..3076));
        assert_eq!(grid.read(cx).active_cursor, Some(0));
        assert!(grid.read(cx).low_nibble_pending);
        simulate_key("a", Some("a"), Modifiers::none(), &grid, window, cx);
        assert_eq!(grid.read(cx).bytes[0], 0x5A);
    })
    .unwrap();
}

#[gpui_kit::test]
fn hex_grid_cut_at_end_clipboard_metadata_and_replace_unified_undo(cx: &mut TestAppContext) {
    let (handle, harness) = open_hex_harness(&[0x00, 0xFF, 0x41, 0x42], cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let grid = harness.read(cx).editor.hex_grid.clone().unwrap();
        // 1. Cut at end-of-buffer insert slot (active_cursor == bytes.len(), no selection) must be a no-op.
        cx.write_to_clipboard(ClipboardItem::new_string("KEEP".into()));
        grid.update(cx, |g, cx| {
            g.insert_mode = true;
            g.selected_range = None;
            g.active_cursor = Some(g.bytes.len());
            g.cut_selection(window, cx);
            assert_eq!(g.bytes, vec![0x00, 0xFF, 0x41, 0x42]);
        });
        assert_eq!(
            cx.read_from_clipboard().and_then(|i| i.text()).as_deref(),
            Some("KEEP")
        );

        // 2. Copy non-UTF-8 [0xFF, 0x41] in ASCII mode, overwrite clipboard with same visible text (no MIO meta),
        // then paste in ASCII mode -> inserts literal UTF-8 U+FFFD + 'A', NOT stale [0xFF, 0x41].
        grid.update(cx, |g, cx| {
            g.select_byte(1, false, true, window, cx);
            g.copy_selection(Some(true), cx);
        });
        let copied_lossy = cx.read_from_clipboard().and_then(|i| i.text()).unwrap();
        assert_eq!(copied_lossy, "\u{FFFD}");
        cx.write_to_clipboard(ClipboardItem::new_string(copied_lossy));
        grid.update(cx, |g, cx| {
            g.selected_range = None;
            g.active_cursor = Some(0);
            g.editing_ascii = true;
            g.paste_clipboard(window, cx);
            assert_eq!(&g.bytes[..3], "\u{FFFD}".as_bytes());
            g.undo(window, cx);
            assert_eq!(g.bytes, vec![0x00, 0xFF, 0x41, 0x42]);
        });
    })
    .unwrap();
    cx.run_until_parked();

    // 3. Find & Replace in Hex view with extra spaces canonicalizes hex text and pushes unified delta undo.
    cx.update_window(handle, |_, window, cx| {
        harness.update(cx, |view, cx| {
            view.editor
                .search
                .update(cx, |s, cx| s.set_value("41 42", window, cx));
            view.editor
                .replace
                .update(cx, |s, cx| s.set_value("43   44", window, cx));
            view.editor.update_search(None, window, cx).unwrap();
            view.editor.replace_current(false, window, cx).unwrap();
            assert_eq!(view.editor.hex.read(cx).value().as_ref(), "00 ff 43 44");
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        let grid = harness.read(cx).editor.hex_grid.clone().unwrap();
        grid.update(cx, |g, cx| {
            assert_eq!(g.bytes, vec![0x00, 0xFF, 0x43, 0x44]);
            g.undo(window, cx);
            assert_eq!(g.bytes, vec![0x00, 0xFF, 0x41, 0x42]);
        });

        // 4. Searching a non-empty query with 0 matches clears stale search_match_range.
        harness.update(cx, |view, cx| {
            view.editor
                .search
                .update(cx, |s, cx| s.set_value("ff 41", window, cx));
            view.editor.update_search(Some(true), window, cx).unwrap();
            assert_eq!(
                view.editor.hex_grid.as_ref().unwrap().read(cx).search_match_range,
                Some(1..3)
            );
            view.editor
                .search
                .update(cx, |s, cx| s.set_value("de ad be ef", window, cx));
            view.editor.update_search(Some(true), window, cx).unwrap();
            assert_eq!(
                view.editor.hex_grid.as_ref().unwrap().read(cx).search_match_range,
                None
            );
        });

        // 5. Inspector Hex writeback (replace_hex_byte_range) preserves prior undo stack and is undoable.
        harness.update(cx, |view, cx| {
            view.editor
                .replace_hex_byte_range(1..3, &[0xAA, 0xBB], window, cx)
                .unwrap();
        });
        grid.update(cx, |g, cx| {
            assert_eq!(g.bytes, vec![0x00, 0xAA, 0xBB, 0x42]);
            g.undo(window, cx);
            assert_eq!(g.bytes, vec![0x00, 0xFF, 0x41, 0x42]);
        });
    })
    .unwrap();
}

#[gpui_kit::test]
fn hex_grid_full_delete_via_inspector_and_ui_undo_redo(cx: &mut TestAppContext) {
    let (handle, harness) = open_hex_harness(&[0x00, 0xFF, 0x41, 0x42], cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let grid = harness.read(cx).editor.hex_grid.clone().unwrap();

        // Part A: Inspector replacing the entire Hex buffer with empty bytes must not underflow.
        harness.update(cx, |view, cx| {
            view.editor
                .replace_hex_byte_range(0..4, &[], window, cx)
                .unwrap();
        });
        grid.update(cx, |g, _cx| {
            assert!(g.bytes.is_empty());
            assert_eq!(g.active_cursor, None);
            assert_eq!(g.selected_range, None);
        });
        assert!(harness.read(cx).editor.hex.read(cx).value().is_empty());

        grid.update(cx, |g, cx| {
            g.undo(window, cx);
            assert_eq!(g.bytes, vec![0x00, 0xFF, 0x41, 0x42]);
        });
        assert_eq!(
            harness.read(cx).editor.hex.read(cx).value().as_ref(),
            "00 ff 41 42"
        );

        grid.update(cx, |g, cx| {
            g.redo(window, cx);
            assert!(g.bytes.is_empty());
            assert_eq!(g.active_cursor, None);
            g.undo(window, cx);
            assert_eq!(g.bytes, vec![0x00, 0xFF, 0x41, 0x42]);
        });

        // Part B: UI Select All + Backspace + Undo + Redo works cleanly on full delete.
        grid.update(cx, |g, cx| {
            g.select_all(cx);
            assert_eq!(g.selected_range, Some(0..4));
        });
        simulate_key("backspace", None, Modifiers::none(), &grid, window, cx);
        grid.update(cx, |g, cx| {
            assert!(g.bytes.is_empty());
            assert_eq!(g.active_cursor, None);
            g.undo(window, cx);
            assert_eq!(g.bytes, vec![0x00, 0xFF, 0x41, 0x42]);
            g.redo(window, cx);
            assert!(g.bytes.is_empty());
        });
    })
    .unwrap();
}
