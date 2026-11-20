use super::*;

#[gpui_kit::test]
fn batch_notes_ack_keeps_later_sidebar_input_and_sends_it_against_new_revision(
    cx: &mut TestAppContext,
) {
    let (mut f, ids) = setup(cx);
    sidebar(&mut f, "original", cx);
    let (handle, editor) = open(&f, cx);
    load(&mut f, &ids, true, cx);
    input(handle, &editor, "batch submitted", cx);
    click(handle, "http-notes-batch-save", cx);
    let pending = write(&mut f);
    click(f.window, "table-cell-0:0", cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let input = f.view.read(cx).http_notes.editor.clone();
            let before = input.read(cx).document_revision();
            window.click(("input", input.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input("较新的侧栏输入🙂", cx);
            assert_eq!(input.read(cx).value().as_ref(), "较新的侧栏输入🙂");
            assert!(input.read(cx).document_revision() > before);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut f.commands).is_empty(),
        "batch owns selected revisions until its receipt"
    );
    ack(&f, pending.0, &pending.1, cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(
            view.http_notes.editor.read(cx).value().as_ref(),
            "较新的侧栏输入🙂"
        );
        assert_eq!(view.http_notes.drafts[&ids[0]].stored.revision, 1);
        assert_eq!(view.http_notes.drafts[&ids[0]].text, "较新的侧栏输入🙂");
    });
    drop(editor);
    cx.run_until_parked();
    let commands: Vec<_> = drain(&mut f.commands)
        .into_iter()
        .filter(|command| !matches!(command, Command::QueryHttpHistory { .. }))
        .collect();
    let [Command::SetExtHttpNotes { change, .. }] = commands.as_slice() else {
        panic!("newer sidebar input must resume autosave after batch closes")
    };
    assert_eq!((change.source_id, change.item_id), ids[0]);
    assert_eq!(change.expected_notes_revision, 1);
    assert_eq!(change.text, "较新的侧栏输入🙂");
}

#[gpui_kit::test]
fn batch_notes_ack_keeps_equal_text_sidebar_ime_until_user_commits(cx: &mut TestAppContext) {
    let (mut f, ids) = setup(cx);
    sidebar(&mut f, "same", cx);
    let (handle, editor) = open(&f, cx);
    load(&mut f, &ids, true, cx);
    input(handle, &editor, "batch submitted", cx);
    click(handle, "http-notes-batch-save", cx);
    let pending = write(&mut f);
    click(f.window, "table-cell-0:0", cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let input = f.view.read(cx).http_notes.editor.clone();
            window.click(("input", input.entity_id()), cx);
            input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(Some(0..4), "same", Some(4..4), window, cx);
                assert_eq!(input.value().as_ref(), "same");
                assert!(input.marked_text_range(window, cx).is_some());
            });
        })
        .unwrap();
    ack(&f, pending.0, &pending.1, cx);
    f.window
        .update(cx, |_, window, cx| {
            let input = f.view.read(cx).http_notes.editor.clone();
            input.update(cx, |input, cx| {
                assert_eq!(input.value().as_ref(), "same");
                assert!(input.marked_text_range(window, cx).is_some());
                input.unmark_text(window, cx);
            });
            assert_eq!(
                f.view.read(cx).http_notes.stored.as_ref().unwrap().revision,
                1
            );
            window.press(crate::kbd!("a"), cx);
            window.input("正式提交的中文", cx);
            assert_eq!(input.read(cx).value().as_ref(), "正式提交的中文");
        })
        .unwrap();
    cx.run_until_parked();
    drop(editor);
    cx.run_until_parked();
    let commands: Vec<_> = drain(&mut f.commands)
        .into_iter()
        .filter(|command| !matches!(command, Command::QueryHttpHistory { .. }))
        .collect();
    let [Command::SetExtHttpNotes { change, .. }] = commands.as_slice() else {
        panic!("committed IME text must resume autosave with batch revision")
    };
    assert_eq!(change.expected_notes_revision, 1);
    assert_eq!(change.text, "正式提交的中文");
}

pub(super) fn load_common(f: &mut Fixture, ids: &[NoteId], text: &str, cx: &mut TestAppContext) {
    let commands = drain(&mut f.commands);
    let [
        Command::ReadExtHttpNotesBatch {
            operation_id,
            ids: requested,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("one shared-text metadata read")
    };
    assert_eq!(requested, ids);
    f.events
        .try_send(Event::ExtHttpNotesBatch {
            project: f.project,
            operation_id: *operation_id,
            result: Ok(mio_core::ExtHttpNotesBatch {
                states: ids
                    .iter()
                    .map(|&(source, item)| (source, item, 0, !text.is_empty()))
                    .collect(),
                common_text: Some(text.into()),
            }),
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn batch_equal_source_notes_prefill_common_text_without_overwrite_warning(cx: &mut TestAppContext) {
    let (mut f, ids) = setup(cx);
    let (handle, editor) = open(&f, cx);
    load_common(&mut f, &ids, "MULTI-NOTES-229", cx);
    cx.read(|cx| {
        assert_eq!(
            editor.read(cx).input.read(cx).value().as_ref(),
            "MULTI-NOTES-229"
        );
        assert!(
            !editor.read(cx).warning,
            "equal notes have no overwrite warning in Ext"
        );
    });
    click(handle, "http-notes-batch-cancel", cx);
    assert!(drain(&mut f.commands).is_empty());
    drop(editor);
    cx.run_until_parked();
}
