use super::*;

#[gpui_kit::test]
fn batch_notes_cancel_after_receiver_disconnect_waits_for_join_then_retains_unknown_input(
    cx: &mut TestAppContext,
) {
    let (mut f, ids) = setup(cx);
    let (finished, joined) = tokio::sync::oneshot::channel::<anyhow::Result<()>>();
    cx.update(|cx| {
        let joined = cx.background_spawn(async move { joined.await? });
        Workbench::observe_runtime(&f.view, f.window, joined, cx);
    });
    let (handle, editor) = open(&f, cx);
    load(&mut f, &ids, false, cx);
    input(handle, &editor, "保存结果未知也要保留", cx);
    click(handle, "http-notes-batch-save", cx);
    let pending = write(&mut f);
    let (unrelated, _) = mpsc::channel(1);
    drop(std::mem::replace(&mut f.events, unrelated));
    cx.run_until_parked();
    assert!(pending.3.is_cancelled());
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.backend_disconnected);
        assert!(view.runtime_result.is_none());
        assert!(
            view.http_notes_pending(),
            "channel closure does not prove Runtime has joined"
        );
    });
    click(handle, "http-notes-batch-cancel", cx);
    assert!(cx.read(|cx| cx.windows().contains(&handle)));
    finished
        .send(Err(anyhow::anyhow!("synthetic terminal storage failure")))
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(matches!(view.runtime_result, Some(Err(_))));
        assert!(
            !view.http_notes_pending(),
            "a joined Runtime can never emit the missing batch receipt"
        );
        let batch = view.http_notes.batch.as_ref().unwrap();
        assert!(
            batch
                .error
                .as_deref()
                .is_some_and(|error| error.contains("未确认"))
        );
        assert!(
            cx.windows().contains(&handle),
            "unknown commit outcome must not silently discard the common text"
        );
        assert_eq!(
            editor.read(cx).input.read(cx).value().as_ref(),
            "保存结果未知也要保留"
        );
    });
    click(handle, "http-notes-batch-cancel", cx);
    assert!(!cx.read(|cx| cx.windows().contains(&handle)));
    drop(editor);
    cx.run_until_parked();
}

#[gpui_kit::test]
fn batch_notes_read_deadline_cancels_but_waits_for_its_actual_receipt(cx: &mut TestAppContext) {
    let (mut f, _) = setup(cx);
    let (handle, editor) = open(&f, cx);
    let commands = drain(&mut f.commands);
    let [
        Command::ReadExtHttpNotesBatch {
            operation_id,
            cancel,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("one pending metadata read")
    };
    let operation = *operation_id;
    cx.background_executor
        .advance_clock(std::time::Duration::from_secs(31));
    cx.run_until_parked();
    assert!(cancel.is_cancelled());
    cx.read(|cx| assert!(f.view.read(cx).http_notes_pending()));
    assert!(cx.read(|cx| cx.windows().contains(&handle)));
    input(handle, &editor, "读取超时后的草稿", cx);
    click(handle, "http-notes-batch-cancel", cx);
    assert!(cx.read(|cx| cx.windows().contains(&handle)));
    f.events
        .try_send(Event::ExtHttpNotesBatch {
            project: f.project,
            operation_id: operation,
            result: Err("cancelled metadata read".into()),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(!cx.read(|cx| cx.windows().contains(&handle)));
    drop(editor);
    cx.run_until_parked();
}
