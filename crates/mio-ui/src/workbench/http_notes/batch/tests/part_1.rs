use super::*;

pub(super) fn click(
    handle: AnyWindowHandle,
    id: impl Into<gpui_kit::ElementId>,
    cx: &mut TestAppContext,
) {
    handle
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        })
        .unwrap();
    cx.run_until_parked();
}

pub(super) fn setup(cx: &mut TestAppContext) -> (Fixture, Vec<NoteId>) {
    let mut f = fixture(cx, vec![], 64);
    let commands = drain(&mut f.commands);
    let [
        Command::QueryHttpHistory {
            query: mio_core::HttpHistoryQuery { query_revision, .. },
        },
    ] = commands.as_slice()
    else {
        panic!("initial query")
    };
    let source = Uuid::new_v4();
    let ids = vec![(source, Uuid::new_v4()), (source, Uuid::new_v4())];
    let rows = ids
        .iter()
        .enumerate()
        .map(|(index, &(source_id, item_id))| {
            HistoryRow::ExtProxy(Box::new(mio_core::ExtHistoryRow {
                item: ExtItemSummary {
                    id: item_id,
                    source_id,
                    collection: "root.f1.f0".into(),
                    index: index as u64,
                    object_address: None,
                    state: "mapped".into(),
                    message_count: 0,
                },
                request: None,
                edited_request: None,
                response: None,
                edited_response: None,
                origin: None,
                notes_revision: 0,
                notes_preview: String::new(),
                notes_error: None,
                highlight: Some(ExtHttpHighlight {
                    revision: 0,
                    color: 0,
                }),
                highlight_error: None,
                time_unix_ms: None,
                time_error: None,
            }))
        })
        .collect();
    f.events
        .try_send(Event::HistoryPage {
            project: f.project,
            query_revision: *query_revision,
            offset: 0,
            total: 2,
            rows,
        })
        .unwrap();
    cx.run_until_parked();
    click(f.window, "proxy-history", cx);
    (f, ids)
}

pub(super) fn open(f: &Fixture, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<BatchEditor>) {
    click(f.window, "table-cell-0:0", cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.press(crate::kbd!("a"), cx);
            assert_eq!(
                f.view
                    .read(cx)
                    .history
                    .read(cx)
                    .delegate()
                    .selected_ids
                    .len(),
                2
            );
            window.right_click("table-cell-0:0", cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(
            view.history.read(cx).delegate().selected_ids.len(),
            2,
            "the selected set must still contain both rows after the right-click settles; primary={:?}, error={:?}, notes_pending={}",
            view.selected_history,
            view.error,
            view.http_notes_pending(),
        );
    });
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let position = window.find_by_label("添加备注").bounds().center();
            window.pointer_down(position, MouseButton::Left, cx);
            window.pointer_up(position, MouseButton::Left, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let handle = cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(
            view.history.read(cx).delegate().selected_ids.len(),
            2,
            "Add notes must not shrink its selected targets; error={:?}, notes_pending={}",
            view.error,
            view.http_notes_pending(),
        );
        view.http_notes.batch.as_ref().unwrap_or_else(|| {
            panic!(
                "Add notes did not open: selected={}, primary={:?}, error={:?}, notes_pending={}, project_saves={}, picker={}, transition={}",
                view.history.read(cx).delegate().selected_ids.len(),
                view.selected_history,
                view.error,
                view.http_notes_pending(),
                view.project_saves_in_flight,
                view.file_picker_pending(),
                view.transition.is_some(),
            )
        }).window.expect("the batch state must own its native window")
    });
    let editor = handle
        .update(cx, |_, window, cx| {
            window
                .root::<gpui_kit::base::Root>()
                .unwrap()
                .unwrap()
                .read(cx)
                .view()
                .clone()
                .downcast::<BatchEditor>()
                .unwrap()
        })
        .unwrap();
    (handle, editor)
}

pub(super) fn load(
    f: &mut Fixture,
    ids: &[NoteId],
    has_notes: bool,
    cx: &mut TestAppContext,
) -> Uuid {
    let commands = drain(&mut f.commands);
    let [
        Command::ReadExtHttpNotesBatch {
            operation_id,
            ids: requested,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("one bounded metadata read: {}", commands.len())
    };
    assert_eq!(requested, ids);
    let operation = *operation_id;
    f.events
        .try_send(Event::ExtHttpNotesBatch {
            project: f.project,
            operation_id: operation,
            result: Ok(mio_core::ExtHttpNotesBatch {
                states: ids
                    .iter()
                    .enumerate()
                    .map(|(index, &(source, item))| (source, item, 0, has_notes && index == 0))
                    .collect(),
                common_text: (!has_notes).then(String::new),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    operation
}

pub(super) fn input(
    handle: AnyWindowHandle,
    editor: &Entity<BatchEditor>,
    text: &str,
    cx: &mut TestAppContext,
) -> u64 {
    let revision = handle
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let input = editor.read(cx).input.clone();
            let before = input.read(cx).document_revision();
            window.click(("input", input.entity_id()), cx);
            window.press(crate::kbd!("a"), cx);
            window.input(text, cx);
            assert_eq!(input.read(cx).value().as_ref(), text);
            assert!(input.read(cx).document_revision() > before);
            input.read(cx).document_revision()
        })
        .unwrap();
    cx.run_until_parked();
    revision
}

pub(super) fn write(f: &mut Fixture) -> (Uuid, Vec<(Uuid, Uuid, u64)>, String, CancellationToken) {
    let commands: Vec<_> = drain(&mut f.commands)
        .into_iter()
        .filter(|command| !matches!(command, Command::QueryHttpHistory { .. }))
        .collect();
    let [
        Command::SetExtHttpNotesBatch {
            operation_id,
            changes,
            text,
            cancel,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("one atomic common-text write: {}", commands.len())
    };
    (*operation_id, changes.clone(), text.clone(), cancel.clone())
}

pub(super) fn ack(
    f: &Fixture,
    operation: Uuid,
    changes: &[(Uuid, Uuid, u64)],
    cx: &mut TestAppContext,
) {
    f.events
        .try_send(Event::ExtHttpNotesBatchChanged {
            project: f.project,
            operation_id: operation,
            result: Ok(changes
                .iter()
                .map(|&(source, item, revision)| (source, item, revision + 1))
                .collect()),
        })
        .unwrap();
    cx.run_until_parked();
}
