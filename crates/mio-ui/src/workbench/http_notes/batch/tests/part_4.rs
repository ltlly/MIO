use super::*;

#[gpui_kit::test]
fn batch_notes_cas_retry_preserves_local_draft_and_rebases_every_selected_item(
    cx: &mut TestAppContext,
) {
    let (mut f, ids) = setup(cx);
    f.view.update(cx, |view, _| {
        view.http_notes.drafts.insert(
            ids[0],
            Draft {
                stored: ExtHttpNotes {
                    revision: 0,
                    text: "source note".into(),
                },
                text: "retained failed local draft".into(),
                pending: None,
                error: Some("source readonly".into()),
            },
        );
    });
    let (handle, editor) = open(&f, cx);
    load(&mut f, &ids, true, cx);
    input(handle, &editor, "批量覆盖中文", cx);
    click(handle, "http-notes-batch-save", cx);
    let first = write(&mut f);
    f.events
        .try_send(Event::ExtHttpNotesBatchChanged {
            project: f.project,
            operation_id: first.0,
            result: Err(mio_core::ExtHttpNotesBatchError {
                message: "CAS conflict".into(),
                stored: ids
                    .iter()
                    .map(|&(source, item)| (source, item, 7, true))
                    .collect(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).http_notes.drafts[&ids[0]].text,
            "retained failed local draft"
        );
        assert_eq!(
            editor.read(cx).input.read(cx).value().as_ref(),
            "批量覆盖中文"
        );
        assert!(!f.view.read(cx).http_notes_pending());
    });
    click(handle, "http-notes-batch-save", cx);
    let retry = write(&mut f);
    assert_eq!(retry.2, "批量覆盖中文");
    assert!(retry.1.iter().all(|(_, _, revision)| *revision == 7));
    ack(&f, first.0, &first.1, cx);
    cx.read(|cx| assert!(f.view.read(cx).http_notes_pending()));
    ack(&f, retry.0, &retry.1, cx);
    cx.read(|cx| assert!(!f.view.read(cx).http_notes.drafts.contains_key(&ids[0])));
    assert!(!cx.read(|cx| cx.windows().contains(&handle)));
    drop(editor);
    cx.run_until_parked();
}
