use super::*;

#[gpui_kit::test]
fn notes_failed_autosave_can_join_atomic_save_as_batch_without_losing_newer_input(
    cx: &mut TestAppContext,
) {
    let (mut f, first, _) = opened(cx);
    input(&f, "recover through Save As", cx);
    let save = set(&mut f);
    f.events
        .try_send(Event::WebSocketNotesChanged {
            project: f.project,
            operation_id: save.0,
            message_id: first.id,
            result: Err(WebSocketNotesError {
                message: "source read only".into(),
                stored: Some(WebSocketNotes::default()),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    let batch = f
        .window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                assert!(!view.websocket_notes_pending());
                view.snapshot_websocket_notes(window, cx).unwrap()
            })
        })
        .unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].message_id, first.id);
    assert_eq!(batch[0].text, "recover through Save As");
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.accept_saved_websocket_notes(&batch, &[(first.id, 1)], window, cx)
                    .unwrap();
                assert!(!view.websocket_notes_unsaved(cx));
            });
        })
        .unwrap();
    assert!(drain(&mut f.commands).is_empty());
    cx.read(|cx| {
        assert_eq!(
            f.view
                .read(cx)
                .websocket
                .notes
                .editor
                .read(cx)
                .value()
                .as_ref(),
            "recover through Save As"
        )
    });
}

#[gpui_kit::test]
fn save_as_receipt_keeps_clear_note_intent_after_switching_message(cx: &mut TestAppContext) {
    let (mut f, first, second) = opened(cx);
    input(&f, "submitted note", cx);
    let autosave = set(&mut f);
    f.events
        .try_send(Event::WebSocketNotesChanged {
            project: f.project,
            operation_id: autosave.0,
            message_id: first.id,
            result: Err(WebSocketNotesError {
                message: "source read only".into(),
                stored: Some(WebSocketNotes::default()),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.press(crate::kbd!("shift-s"), cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path());
    let destination = PathBuf::from("/tmp/mio-notes-clear-after-save-as.mio");
    cx.simulate_new_path_selection(|_| Some(destination.clone()));
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [
        Command::SaveProject {
            websocket_notes,
            path: Some(path),
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("Save As must submit one atomic notes batch")
    };
    assert_eq!(path, &destination);
    assert_eq!(websocket_notes.len(), 1);
    assert_eq!(websocket_notes[0].message_id, first.id);
    assert_eq!(websocket_notes[0].text, "submitted note");
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let editor = f.view.read(cx).websocket.notes.editor.clone();
            window.click(("input", editor.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.press("backspace", cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut f.commands).is_empty(),
        "autosave must not race a project save batch"
    );
    click_row(&f, 1, cx);
    let commands = drain(&mut f.commands);
    let read_revision = commands
        .iter()
        .find_map(|command| match command {
            Command::ReadWebSocketNotes {
                message_id,
                selection_revision,
                ..
            } if *message_id == second.id => Some(*selection_revision),
            _ => None,
        })
        .unwrap();
    f.events
        .try_send(Event::WebSocketNotes {
            project: f.project,
            selection_revision: read_revision,
            message_id: second.id,
            result: Ok(WebSocketNotes {
                revision: 0,
                text: "second untouched".into(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    f.events
        .try_send(Event::ProjectSaved {
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: f.project,
            path: destination,
            total_bytes: 4096,
            websocket_notes: vec![(first.id, 1)],
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(view.websocket.selected, Some(second.id));
        assert_eq!(
            view.websocket.notes.editor.read(cx).value().as_ref(),
            "second untouched"
        );
        assert!(
            view.websocket_notes_unsaved(cx),
            "clearing A after Save As began must remain an unsaved intent even while B is selected"
        );
        let draft = view
            .websocket
            .notes
            .drafts
            .get(&first.id)
            .expect("A's newer clear must remain associated with A");
        assert_eq!(draft.text, "");
        assert_eq!(draft.stored.revision, 1);
        assert_eq!(draft.stored.text, "submitted note");
    });
    let commands = drain(&mut f.commands);
    assert!(commands.iter().any(|command| matches!(command, Command::SetWebSocketNotes { message_id, expected_notes_revision: 1, text, .. } if *message_id == first.id && text.is_empty())), "the retained clear must be written against the committed batch revision");
}

#[gpui_kit::test]
fn notes_accepting_current_stored_text_after_conflict_does_not_recreate_stale_draft(
    cx: &mut TestAppContext,
) {
    let (mut f, first, _) = opened(cx);
    input(&f, "my draft", cx);
    let save = set(&mut f);
    let stored = WebSocketNotes {
        revision: 1,
        text: "stored elsewhere".into(),
    };
    f.events
        .try_send(Event::WebSocketNotesChanged {
            project: f.project,
            operation_id: save.0,
            message_id: first.id,
            result: Err(WebSocketNotesError {
                message: "notes revision changed".into(),
                stored: Some(stored.clone()),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            f.view
                .read(cx)
                .websocket
                .notes
                .editor
                .read(cx)
                .value()
                .as_ref(),
            "my draft"
        )
    });
    input(&f, &stored.text, cx);
    assert!(
        drain(&mut f.commands).is_empty(),
        "accepting the already stored text must not issue another write"
    );
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(
            !view.websocket_notes_unsaved(cx),
            "the accepted stored text must use the newer conflict baseline"
        );
        assert!(view.websocket.notes.drafts.is_empty());
        assert_eq!(view.websocket.notes.stored.as_ref(), Some(&stored));
    });
    click(&f, "websocket-notes-close", cx);
    click(&f, "websocket-notes-toggle", cx);
    assert!(
        drain(&mut f.commands).is_empty(),
        "closing and reopening Notes must not rebuild an obsolete base-0 draft"
    );
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(!view.websocket_notes_unsaved(cx));
        assert_eq!(
            view.websocket.notes.editor.read(cx).value().as_ref(),
            stored.text
        );
    });
}

#[gpui_kit::test]
fn notes_preview_collapses_whitespace_in_table_but_preserves_full_editor_text(
    cx: &mut TestAppContext,
) {
    let (mut f, first, _) = opened(cx);
    let text = "  首行🙂  原空格\n第二行\r\n第三行\t尾部  ";
    input(&f, text, cx);
    let save = set(&mut f);
    assert_eq!(save.2, text);
    ack(&f, first.id, save.0, 1, text, cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let view = f.view.read(cx);
            assert_eq!(view.websocket.notes.stored.as_ref().unwrap().text, text);
            assert_eq!(view.websocket.notes.editor.read(cx).value().as_ref(), text);
            assert_eq!(
                view.websocket.table.read(cx).delegate().rows[0].notes_preview,
                text
            );
            let cell = window.find("table-cell-0:5");
            assert!(cell.visible());
            assert_eq!(
                cell.label(),
                Some("  首行🙂  原空格 第二行 第三行 尾部  "),
                "Notes table preview must expose every line in a single row"
            );
        })
        .unwrap();
    assert!(
        drain(&mut f.commands)
            .iter()
            .all(|command| matches!(command, Command::QueryWebSocketHistory { .. }))
    );
}
