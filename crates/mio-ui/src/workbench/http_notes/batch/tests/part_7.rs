use super::*;

#[gpui_kit::test]
fn batch_initial_common_read_never_replaces_text_typed_while_loading(cx: &mut TestAppContext) {
    let (mut f, ids) = setup(cx);
    let (handle, editor) = open(&f, cx);
    let entered_revision = input(handle, &editor, "读取期间输入的中文🙂", cx);
    load_common(&mut f, &ids, "MULTI-NOTES-229", cx);
    cx.read(|cx| {
        assert_eq!(
            editor.read(cx).input.read(cx).value().as_ref(),
            "读取期间输入的中文🙂"
        );
        assert_eq!(
            editor.read(cx).input.read(cx).document_revision(),
            entered_revision
        );
        assert!(!editor.read(cx).warning);
    });
    click(handle, "http-notes-batch-save", cx);
    let pending = write(&mut f);
    assert_eq!(pending.2, "读取期间输入的中文🙂");
    ack(&f, pending.0, &pending.1, cx);
    drop(editor);
    cx.run_until_parked();
}

#[gpui_kit::test]
fn batch_different_local_draft_prevents_source_common_prefill_and_warns_without_losing_draft(
    cx: &mut TestAppContext,
) {
    let (mut f, ids) = setup(cx);
    f.view.update(cx, |view, _| {
        view.http_notes.drafts.insert(
            ids[0],
            Draft {
                stored: ExtHttpNotes {
                    revision: 0,
                    text: "MULTI-NOTES-229".into(),
                },
                text: "未保存的不同备注".into(),
                pending: None,
                error: Some("source readonly".into()),
            },
        );
    });
    let (handle, editor) = open(&f, cx);
    load_common(&mut f, &ids, "MULTI-NOTES-229", cx);
    cx.read(|cx| {
        assert!(editor.read(cx).input.read(cx).value().is_empty());
        assert!(editor.read(cx).warning);
        assert_eq!(
            f.view.read(cx).http_notes.drafts[&ids[0]].text,
            "未保存的不同备注"
        );
    });
    click(handle, "http-notes-batch-cancel", cx);
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).http_notes.drafts[&ids[0]].text,
            "未保存的不同备注"
        )
    });
    drop(editor);
    cx.run_until_parked();
}
