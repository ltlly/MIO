use super::*;

#[gpui_kit::test]
fn batch_notes_failed_write_keeps_common_text_across_save_as_and_refreshes_revisions(
    cx: &mut TestAppContext,
) {
    let (mut f, ids) = setup(cx);
    let (handle, editor) = open(&f, cx);
    load(&mut f, &ids, false, cx);
    input(handle, &editor, "副本中重试", cx);
    click(handle, "http-notes-batch-save", cx);
    let first = write(&mut f);
    f.events
        .try_send(Event::ExtHttpNotesBatchChanged {
            project: f.project,
            operation_id: first.0,
            result: Err(mio_core::ExtHttpNotesBatchError {
                message: "source readonly".into(),
                stored: ids
                    .iter()
                    .map(|&(source, item)| (source, item, 0, false))
                    .collect(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("table-cell-0:0", cx);
            window.press(crate::kbd!("shift-s"), cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path());
    input(handle, &editor, "副本确认期间的新中文🙂", cx);
    let destination = PathBuf::from("/tmp/mio-http-batch-notes-recovery.mio");
    cx.simulate_new_path_selection(|_| Some(destination.clone()));
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [
        Command::SaveProject {
            path: Some(path),
            websocket_notes,
            ext_http_notes,
            ext_http_highlights,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("Save As must preserve the separate batch without expanding ordinary drafts")
    };
    assert_eq!(path, &destination);
    assert!(
        websocket_notes.is_empty() && ext_http_notes.is_empty() && ext_http_highlights.is_empty()
    );
    f.events
        .try_send(Event::ProjectSaved {
            project: f.project,
            path: destination,
            total_bytes: 4096,
            websocket_notes: vec![],
            ext_http_notes: vec![],
            ext_http_highlights: vec![],
        })
        .unwrap();
    cx.run_until_parked();
    let commands: Vec<_> = drain(&mut f.commands)
        .into_iter()
        .filter(|command| !matches!(command, Command::QueryHttpHistory { .. }))
        .collect();
    let [
        Command::ReadExtHttpNotesBatch {
            operation_id,
            ids: reread,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("saved copy must refresh all batch CAS baselines")
    };
    assert_eq!(reread, &ids);
    f.events
        .try_send(Event::ExtHttpNotesBatch {
            project: f.project,
            operation_id: *operation_id,
            result: Ok(mio_core::ExtHttpNotesBatch {
                states: ids
                    .iter()
                    .map(|&(source, item)| (source, item, 2, true))
                    .collect(),
                common_text: Some("copied existing common note".into()),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert!(cx.windows().contains(&handle));
        assert_eq!(
            editor.read(cx).input.read(cx).value().as_ref(),
            "副本确认期间的新中文🙂"
        );
        assert!(f.view.read(cx).http_notes.drafts.is_empty());
    });
    click(handle, "http-notes-batch-save", cx);
    let retry = write(&mut f);
    assert_eq!(retry.2, "副本确认期间的新中文🙂");
    assert!(retry.1.iter().all(|(_, _, revision)| *revision == 2));
    ack(&f, retry.0, &retry.1, cx);
    assert!(!cx.read(|cx| cx.windows().contains(&handle)));
    drop(editor);
    cx.run_until_parked();
}

pub(super) fn sidebar(f: &mut Fixture, text: &str, cx: &mut TestAppContext) {
    click(f.window, "table-cell-0:0", cx);
    click(f.window, "http-notes-toggle", cx);
    let commands = drain(&mut f.commands);
    let [
        Command::ReadExtHttpNotes {
            selection_revision,
            source_id,
            item_id,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("sidebar reads selected note")
    };
    f.events
        .try_send(Event::ExtHttpNotes {
            project: f.project,
            selection_revision: *selection_revision,
            source_id: *source_id,
            item_id: *item_id,
            result: Ok(ExtHttpNotes {
                revision: 0,
                text: text.into(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
}
