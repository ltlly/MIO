use super::*;

#[gpui_kit::test]
fn batch_notes_cancel_is_blank_warns_and_keeps_all_existing_64_drafts(cx: &mut TestAppContext) {
    let (mut f, ids) = setup(cx);
    f.view.update(cx, |view, _| {
        for index in 0..MAX_DRAFTS {
            let id = if index == 0 {
                ids[0]
            } else {
                (ids[0].0, Uuid::new_v4())
            };
            view.http_notes.drafts.insert(
                id,
                Draft {
                    stored: ExtHttpNotes {
                        revision: 0,
                        text: String::new(),
                    },
                    text: format!("retained {index}"),
                    pending: None,
                    error: Some("unwritable source".into()),
                },
            );
        }
    });
    let before = cx.read(|cx| {
        f.view
            .read(cx)
            .http_notes
            .drafts
            .iter()
            .map(|(id, draft)| (*id, draft.text.clone()))
            .collect::<HashMap<_, _>>()
    });
    let (handle, editor) = open(&f, cx);
    load(&mut f, &ids, true, cx);
    cx.read(|cx| {
        assert!(editor.read(cx).warning);
        assert!(editor.read(cx).input.read(cx).value().is_empty());
        assert_eq!(f.view.read(cx).http_notes.drafts.len(), MAX_DRAFTS);
    });
    input(handle, &editor, "取消的批量文字", cx);
    click(handle, "http-notes-batch-cancel", cx);
    assert!(!cx.read(|cx| cx.windows().contains(&handle)));
    assert!(drain(&mut f.commands).is_empty());
    cx.read(|cx| {
        assert_eq!(
            f.view
                .read(cx)
                .http_notes
                .drafts
                .iter()
                .map(|(id, draft)| (*id, draft.text.clone()))
                .collect::<HashMap<_, _>>(),
            before
        )
    });
    drop(editor);
    cx.run_until_parked();
}

#[gpui_kit::test]
fn batch_notes_atomic_save_retains_newer_input_and_ignores_old_ack(cx: &mut TestAppContext) {
    let (mut f, ids) = setup(cx);
    let (handle, editor) = open(&f, cx);
    load(&mut f, &ids, false, cx);
    let first_revision = input(handle, &editor, "第一轮中文", cx);
    click(handle, "http-notes-batch-save", cx);
    let first = write(&mut f);
    assert_eq!(
        first.1,
        ids.iter()
            .map(|&(source, item)| (source, item, 0))
            .collect::<Vec<_>>()
    );
    assert_eq!(first.2, "第一轮中文");
    let latest_revision = input(handle, &editor, "较新的两行\n第二行🙂", cx);
    assert!(latest_revision > first_revision);
    ack(&f, first.0, &first.1, cx);
    assert!(cx.read(|cx| cx.windows().contains(&handle)));
    cx.read(|cx| {
        assert_eq!(
            editor.read(cx).input.read(cx).value().as_ref(),
            "较新的两行\n第二行🙂"
        );
        let view = f.view.read(cx);
        assert!(view.http_notes.drafts.is_empty());
        for row in &view.history.read(cx).delegate().rows {
            let HistoryRow::ExtProxy(row) = row else {
                unreachable!()
            };
            assert_eq!(row.notes_preview, "第一轮中文");
            assert_eq!(row.notes_revision, 1);
        }
    });
    click(handle, "http-notes-batch-save", cx);
    let second = write(&mut f);
    assert_eq!(second.2, "较新的两行\n第二行🙂");
    assert!(second.1.iter().all(|(_, _, revision)| *revision == 1));
    ack(&f, first.0, &first.1, cx);
    cx.read(|cx| assert!(f.view.read(cx).http_notes_pending()));
    ack(&f, second.0, &second.1, cx);
    assert!(!cx.read(|cx| cx.windows().contains(&handle)));
    drop(editor);
    cx.run_until_parked();
}

#[gpui_kit::test]
fn batch_notes_ime_blocks_save_and_cancel_waits_for_actual_write_result(cx: &mut TestAppContext) {
    let (mut f, ids) = setup(cx);
    let (handle, editor) = open(&f, cx);
    load(&mut f, &ids, false, cx);
    input(handle, &editor, "same", cx);
    handle
        .update(cx, |_, window, cx| {
            let input = editor.read(cx).input.clone();
            input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(Some(0..4), "same", Some(4..4), window, cx);
                assert_eq!(input.value().as_ref(), "same");
                assert!(input.marked_text_range(window, cx).is_some());
            });
        })
        .unwrap();
    click(handle, "http-notes-batch-save", cx);
    assert!(drain(&mut f.commands).is_empty());
    handle
        .update(cx, |_, window, cx| {
            editor.read(cx).input.clone().update(cx, |input, cx| {
                assert!(input.marked_text_range(window, cx).is_some());
                input.unmark_text(window, cx);
            });
        })
        .unwrap();
    click(handle, "http-notes-batch-save", cx);
    let pending = write(&mut f);
    click(handle, "http-notes-batch-cancel", cx);
    assert!(pending.3.is_cancelled());
    assert!(cx.read(|cx| cx.windows().contains(&handle)));
    f.events
        .try_send(Event::ExtHttpNotesBatchChanged {
            project: f.project,
            operation_id: pending.0,
            result: Err(mio_core::ExtHttpNotesBatchError {
                message: "cancelled before commit".into(),
                stored: Vec::new(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(!cx.read(|cx| cx.windows().contains(&handle)));
    cx.read(|cx| {
        for row in &f.view.read(cx).history.read(cx).delegate().rows {
            let HistoryRow::ExtProxy(row) = row else {
                unreachable!()
            };
            assert_eq!(row.notes_revision, 0);
            assert!(row.notes_preview.is_empty());
        }
    });
    drop(editor);
    cx.run_until_parked();
}
