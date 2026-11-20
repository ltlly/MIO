use super::*;

#[gpui_kit::test]
fn batch_mixed_source_with_local_override_reads_only_uncovered_item_and_prefills_effective_common(
    cx: &mut TestAppContext,
) {
    let (mut f, ids) = setup(cx);
    f.view.update(cx, |view, _| {
        view.http_notes.drafts.insert(
            ids[0],
            Draft {
                stored: ExtHttpNotes {
                    revision: 0,
                    text: "A".into(),
                },
                text: "B".into(),
                pending: None,
                error: Some("source readonly".into()),
            },
        );
    });
    let (handle, editor) = open(&f, cx);
    let commands = drain(&mut f.commands);
    let [
        Command::ReadExtHttpNotesBatch {
            operation_id,
            ids: requested,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("initial full metadata read")
    };
    assert_eq!(requested, &ids);
    f.events
        .try_send(Event::ExtHttpNotesBatch {
            project: f.project,
            operation_id: *operation_id,
            result: Ok(mio_core::ExtHttpNotesBatch {
                states: vec![(ids[0].0, ids[0].1, 0, true), (ids[1].0, ids[1].1, 2, true)],
                common_text: None,
            }),
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [
        Command::ReadExtHttpNotesBatch {
            operation_id,
            ids: uncovered,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("mixed source plus local override must resolve only the remaining source item")
    };
    assert_eq!(uncovered, &vec![ids[1]]);
    cx.read(|cx| {
        assert!(f.view.read(cx).http_notes_pending());
        assert!(
            editor.read(cx).input.read(cx).value().is_empty(),
            "do not guess common text before the subset reply"
        );
        assert_eq!(f.view.read(cx).http_notes.drafts[&ids[0]].text, "B");
    });
    f.events
        .try_send(Event::ExtHttpNotesBatch {
            project: f.project,
            operation_id: *operation_id,
            result: Ok(mio_core::ExtHttpNotesBatch {
                states: vec![(ids[1].0, ids[1].1, 3, true)],
                common_text: Some("B".into()),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(editor.read(cx).input.read(cx).value().as_ref(), "B");
        assert!(!editor.read(cx).warning);
        assert_eq!(f.view.read(cx).http_notes.drafts[&ids[0]].text, "B");
    });
    click(handle, "http-notes-batch-save", cx);
    let pending = write(&mut f);
    assert_eq!(pending.2, "B");
    assert_eq!(
        pending.1,
        vec![(ids[0].0, ids[0].1, 0), (ids[1].0, ids[1].1, 3)]
    );
    f.events
        .try_send(Event::ExtHttpNotesBatchChanged {
            project: f.project,
            operation_id: pending.0,
            result: Ok(vec![(ids[0].0, ids[0].1, 1), (ids[1].0, ids[1].1, 3)]),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(!cx.read(|cx| cx.windows().contains(&handle)));
    drop(editor);
    cx.run_until_parked();
}

#[gpui_kit::test]
fn batch_subset_reply_rechecks_local_coverage_and_preserves_newly_typed_common_input(
    cx: &mut TestAppContext,
) {
    let (mut f, ids) = setup(cx);
    sidebar(&mut f, "A", cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let editor = f.view.read(cx).http_notes.editor.clone();
            window.click(("input", editor.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input("B", cx);
            assert_eq!(editor.read(cx).value().as_ref(), "B");
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [
        Command::SetExtHttpNotes {
            operation_id,
            change,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("initial local note write")
    };
    f.events
        .try_send(Event::ExtHttpNotesChanged {
            project: f.project,
            operation_id: *operation_id,
            source_id: change.source_id,
            item_id: change.item_id,
            result: Err(ExtHttpNotesError {
                message: "source readonly".into(),
                stored: Some(ExtHttpNotes {
                    revision: 0,
                    text: "A".into(),
                }),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    let (handle, editor) = open(&f, cx);
    let commands = drain(&mut f.commands);
    let [Command::ReadExtHttpNotesBatch { operation_id, .. }] = commands.as_slice() else {
        panic!("initial metadata")
    };
    f.events
        .try_send(Event::ExtHttpNotesBatch {
            project: f.project,
            operation_id: *operation_id,
            result: Ok(mio_core::ExtHttpNotesBatch {
                states: ids
                    .iter()
                    .map(|&(source, item)| (source, item, 0, true))
                    .collect(),
                common_text: None,
            }),
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [
        Command::ReadExtHttpNotesBatch {
            operation_id,
            ids: requested,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("uncovered metadata")
    };
    assert_eq!(requested, &vec![ids[1]]);
    let subset_operation = *operation_id;
    click(f.window, "table-cell-0:0", cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let input = f.view.read(cx).http_notes.editor.clone();
            let previous_revision = input.read(cx).document_revision();
            window.click(("input", input.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input("A", cx);
            assert_eq!(input.read(cx).value().as_ref(), "A");
            assert!(input.read(cx).document_revision() > previous_revision);
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| assert!(!f.view.read(cx).http_notes.drafts.contains_key(&ids[0])));
    assert!(drain(&mut f.commands).is_empty());
    let input_revision = input(handle, &editor, "补读期间的新输入", cx);
    f.events
        .try_send(Event::ExtHttpNotesBatch {
            project: f.project,
            operation_id: subset_operation,
            result: Ok(mio_core::ExtHttpNotesBatch {
                states: vec![(ids[1].0, ids[1].1, 1, true)],
                common_text: Some("B".into()),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [
        Command::ReadExtHttpNotesBatch {
            operation_id,
            ids: requested,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("changed local coverage requires fresh metadata, never the stale common value")
    };
    assert_eq!(requested, &ids);
    cx.read(|cx| {
        assert!(f.view.read(cx).http_notes_pending());
        assert_eq!(
            editor.read(cx).input.read(cx).value().as_ref(),
            "补读期间的新输入"
        );
        assert_eq!(
            editor.read(cx).input.read(cx).document_revision(),
            input_revision
        );
    });
    f.events
        .try_send(Event::ExtHttpNotesBatch {
            project: f.project,
            operation_id: *operation_id,
            result: Ok(mio_core::ExtHttpNotesBatch {
                states: vec![(ids[0].0, ids[0].1, 0, true), (ids[1].0, ids[1].1, 1, true)],
                common_text: None,
            }),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert!(!f.view.read(cx).http_notes_pending());
        assert!(editor.read(cx).warning);
        assert_eq!(
            editor.read(cx).input.read(cx).value().as_ref(),
            "补读期间的新输入"
        );
        assert_eq!(
            editor.read(cx).input.read(cx).document_revision(),
            input_revision
        );
    });
    click(handle, "http-notes-batch-cancel", cx);
    assert!(drain(&mut f.commands).is_empty());
    drop(editor);
    cx.run_until_parked();
}
