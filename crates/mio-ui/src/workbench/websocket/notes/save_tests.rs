use super::*;

fn key(f: &Fixture, key: &str, cx: &mut TestAppContext) {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        })
        .unwrap();
    cx.run_until_parked();
}

fn reject(f: &Fixture, id: Uuid, operation: Uuid, stored: WebSocketNotes, cx: &mut TestAppContext) {
    f.events
        .try_send(Event::WebSocketNotesChanged {
            project: f.project,
            operation_id: operation,
            message_id: id,
            result: Err(WebSocketNotesError {
                message: "source write rejected".into(),
                stored: Some(stored),
            }),
        })
        .unwrap();
    cx.run_until_parked();
}

fn saved(f: &Fixture, path: PathBuf, id: Uuid, revision: u64, cx: &mut TestAppContext) {
    f.events
        .try_send(Event::ProjectSaved {
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: f.project,
            path,
            total_bytes: 4096,
            websocket_notes: vec![(id, revision)],
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn failed_notes_autosave_recovers_through_real_save_as_picker_and_project_receipt(
    cx: &mut TestAppContext,
) {
    let (mut f, first, second) = opened(cx);
    let original = cx.read(|cx| f.view.read(cx).workspace.clone());
    let source = PathBuf::from("/tmp/mio-notes-ui-source.mio");
    f.view
        .update(cx, |view, _| view.project_path = Some(source.clone()));
    input(&f, "另存恢复🙂\n完整备注", cx);
    let autosave = set(&mut f);
    reject(&f, first.id, autosave.0, WebSocketNotes::default(), cx);
    drain(&mut f.commands);
    key(&f, crate::kbd!("shift-s"), cx);
    assert!(cx.did_prompt_for_new_path());
    assert!(drain(&mut f.commands).is_empty());
    let destination = PathBuf::from("/tmp/mio-notes-ui-recovered.mio");
    cx.simulate_new_path_selection(|_| Some(destination.clone()));
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [
        Command::SaveProject {
            project,
            path: Some(path),
            workspace,
            websocket_notes,
            ext_http_notes: _,
            ext_http_highlights: _,
        },
    ] = commands.as_slice()
    else {
        panic!("Save As must send one atomic notes batch without retrying the failed source write");
    };
    assert_eq!((*project, path), (f.project, &destination));
    assert!(workspace.is_none());
    assert_eq!(
        websocket_notes,
        &[WebSocketNotesChange {
            message_id: first.id,
            expected_notes_revision: 0,
            text: autosave.2.clone(),
        }]
    );
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.websocket_notes_unsaved(cx));
        assert_eq!(view.project_path.as_ref(), Some(&source));
        assert_eq!(view.workspace, original);
        assert_eq!(view.websocket.table.read(cx).delegate().rows[1], second);
    });
    saved(&f, destination.clone(), first.id, 1, cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(!view.websocket_notes_unsaved(cx));
        assert!(view.project_notes_save.is_none());
        assert_eq!(view.project_path.as_ref(), Some(&destination));
        assert_eq!(view.workspace, original);
        assert_eq!(
            view.websocket.notes.editor.read(cx).value().as_ref(),
            autosave.2
        );
        assert_eq!(view.websocket.table.read(cx).delegate().rows[1], second);
    });
    assert!(
        drain(&mut f.commands)
            .iter()
            .all(|command| matches!(command, Command::QueryWebSocketHistory { .. }))
    );
}

#[gpui_kit::test]
fn cmd_s_waits_for_autosave_ack_and_late_project_receipt_keeps_newer_notes(
    cx: &mut TestAppContext,
) {
    let (mut f, first, _) = opened(cx);
    let source = PathBuf::from("/tmp/mio-notes-ui-inflight.mio");
    f.view
        .update(cx, |view, _| view.project_path = Some(source.clone()));
    let original = cx.read(|cx| f.view.read(cx).workspace.clone());
    input(&f, "第一版", cx);
    let first_save = set(&mut f);
    input(&f, "第二版等待项目保存", cx);
    key(&f, crate::kbd!("s"), cx);
    assert!(
        drain(&mut f.commands).is_empty(),
        "project save must wait for notes ownership to resolve"
    );
    ack(&f, first.id, first_save.0, 1, &first_save.2, cx);
    let commands = drain(&mut f.commands);
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, Command::SetWebSocketNotes { .. })),
        "the waiting project save owns the latest note; do not start another autosave"
    );
    let batches: Vec<_> = commands
        .iter()
        .filter_map(|command| match command {
            Command::SaveProject {
                project,
                path: None,
                workspace: None,
                websocket_notes,
                ext_http_notes: _,
                ext_http_highlights: _,
            } if *project == f.project => Some(websocket_notes),
            _ => None,
        })
        .collect();
    assert_eq!(batches.len(), 1);
    assert_eq!(
        batches[0],
        &[WebSocketNotesChange {
            message_id: first.id,
            expected_notes_revision: 1,
            text: "第二版等待项目保存".into()
        }]
    );
    assert!(commands.iter().all(|command| matches!(
        command,
        Command::SaveProject { .. } | Command::QueryWebSocketHistory { .. }
    )));
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.notes_save_wait.is_none());
        assert!(!view.websocket_notes_pending());
        assert_eq!(view.project_notes_save.as_ref().unwrap(), batches[0]);
        assert!(view.websocket_notes_unsaved(cx));
        assert_eq!(view.websocket.notes.stored.as_ref().unwrap().revision, 1);
    });
    input(&f, "第三版比项目回执更新🙂", cx);
    assert!(
        drain(&mut f.commands).is_empty(),
        "newer input must wait while the project batch owns the note revision"
    );
    saved(&f, source, first.id, 2, cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(
            view.websocket_notes_unsaved(cx),
            "an older project receipt must not mark new editor content saved"
        );
        assert_eq!(
            view.websocket.notes.editor.read(cx).value().as_ref(),
            "第三版比项目回执更新🙂"
        );
        assert_eq!(
            view.websocket.notes.drafts[&first.id].text,
            "第三版比项目回执更新🙂"
        );
        assert_eq!(view.websocket.notes.stored.as_ref().unwrap().revision, 2);
        assert_eq!(view.workspace, original);
        assert!(!view.quitting);
        assert!(view.transition.is_none());
    });
}

#[gpui_kit::test]
fn save_quit_receipt_cannot_discard_equal_text_notes_composition(cx: &mut TestAppContext) {
    use crate::workbench::tests::pump_native;

    let (mut f, first, _) = opened(cx);
    let source = PathBuf::from("/tmp/mio-notes-equal-composition.mio");
    f.view
        .update(cx, |view, _| view.project_path = Some(source.clone()));
    let text = "same pending note";
    input(&f, text, cx);
    let autosave = set(&mut f);
    reject(&f, first.id, autosave.0, WebSocketNotes::default(), cx);
    drain(&mut f.commands);
    key(&f, crate::kbd!("q"), cx);
    pump_native(cx);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("保存");
    pump_native(cx);
    assert!(
        matches!(drain(&mut f.commands).as_slice(), [Command::Quiesce { project }] if *project == f.project)
    );
    f.events
        .try_send(Event::Quiesced { project: f.project })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [
        Command::SaveProject {
            project,
            path: None,
            workspace: None,
            websocket_notes,
            ext_http_notes: _,
            ext_http_highlights: _,
        },
    ] = commands.as_slice()
    else {
        panic!("save-before-quit must submit the failed autosave as an atomic project batch");
    };
    assert_eq!(*project, f.project);
    assert_eq!(
        websocket_notes,
        &[WebSocketNotesChange {
            message_id: first.id,
            expected_notes_revision: 0,
            text: text.into()
        }]
    );
    f.window
        .update(cx, |_, window, cx| {
            let editor = f.view.read(cx).websocket.notes.editor.clone();
            editor.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(
                    Some(0..text.len()),
                    text,
                    Some(text.len()..text.len()),
                    window,
                    cx,
                );
                assert_eq!(input.value().as_ref(), text);
                assert!(input.marked_text_range(window, cx).is_some());
            });
            let view = f.view.read(cx);
            assert!(matches!(view.after_save, Some(AfterSave::Quit)));
            assert!(
                matches!(&view.transition, Some(TransitionPhase::Saving(path)) if path == &source)
            );
        })
        .unwrap();
    saved(&f, source, first.id, 1, cx);
    pump_native(cx);
    f.window
        .update(cx, |_, window, cx| {
            let view = f.view.read(cx);
            assert!(
                !view.shutdown.is_cancelled(),
                "equal note text does not make active IME composition safe to discard on quit"
            );
            assert!(!view.shutdown_requested && !view.quitting);
            assert!(view.after_save.is_none());
            assert!(view.transition.is_none());
            let editor = view.websocket.notes.editor.clone();
            editor.update(cx, |input, cx| {
                assert_eq!(input.value().as_ref(), text);
                assert!(input.marked_text_range(window, cx).is_some());
            });
        })
        .unwrap();
    assert!(
        drain(&mut f.commands).iter().all(|command| matches!(
            command,
            Command::Resume { .. } | Command::QueryWebSocketHistory { .. }
        )),
        "rejecting composition must not issue another save, send, or project close"
    );
}
