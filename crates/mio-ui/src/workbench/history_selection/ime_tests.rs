use super::*;
use crate::workbench::tests::{Fixture, drain, fixture};
use gpui_kit::TestAppContext;
use gpui_kit::test::{TestEventExt, TestWindowExt};
use mio_core::{ExtHttpHighlight, ExtHttpNotes, ExtItemSummary, Uuid};

fn click_row(f: &Fixture, row: usize, modifiers: Modifiers, cx: &mut TestAppContext) {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let position = window.find(format!("table-cell-{row}:0")).bounds().center();
            window.change_modifiers(modifiers, cx);
            window.pointer_down(position, MouseButton::Left, cx);
            window.pointer_up(position, MouseButton::Left, cx);
            window.change_modifiers(Modifiers::default(), cx);
        })
        .unwrap();
    cx.run_until_parked();
}

fn assert_retained(f: &Fixture, rows: &[HistoryRow], cx: &mut TestAppContext) {
    f.window
        .update(cx, |_, window, cx| {
            let view = f.view.read(cx);
            let delegate = view.history.read(cx).delegate();
            assert_eq!(view.selected_history, Some(rows[1].id()));
            assert_eq!(
                delegate.selected_ids,
                HashSet::from([rows[0].id(), rows[1].id()])
            );
            assert_eq!(delegate.selection_anchor, Some(rows[1].id()));
            assert_eq!(delegate.selection_lead, Some(rows[1].id()));
            assert_eq!(view.history.read(cx).selected_row(), Some(1));
            assert_eq!(
                delegate.rows.iter().map(HistoryRow::id).collect::<Vec<_>>(),
                rows.iter().map(HistoryRow::id).collect::<Vec<_>>()
            );
            let editor = view.http_notes.editor.clone();
            editor.update(cx, |editor, cx| {
                assert_eq!(editor.value().as_ref(), "正在组词");
                assert!(editor.marked_text_range(window, cx).is_some());
            });
        })
        .unwrap();
}

#[gpui_kit::test]
fn notes_ime_rejects_cmd_shift_primary_changes_and_page_removal_without_changing_selection(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, vec![], 64);
    let commands = drain(&mut f.commands);
    let [
        Command::QueryHttpHistory {
            query: mio_core::HttpHistoryQuery { query_revision, .. },
        },
    ] = commands.as_slice()
    else {
        panic!("initial history")
    };
    let source = Uuid::new_v4();
    let rows: Vec<_> = (0..3)
        .map(|index| {
            HistoryRow::ExtProxy(Box::new(mio_core::ExtHistoryRow {
                item: ExtItemSummary {
                    id: Uuid::new_v4(),
                    source_id: source,
                    collection: "root.f1.f0".into(),
                    index,
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
            total: 3,
            rows: rows.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("proxy-history", cx);
        })
        .unwrap();
    cx.run_until_parked();
    click_row(&f, 0, Modifiers::default(), cx);
    click_row(&f, 1, crate::secondary_modifiers(true), cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("http-notes-toggle", cx);
        })
        .unwrap();
    cx.run_until_parked();
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
        panic!("read active Notes")
    };
    assert_eq!(
        HistoryEntryId::External {
            source_id: *source_id,
            item_id: *item_id
        },
        rows[1].id()
    );
    f.events
        .try_send(Event::ExtHttpNotes {
            project: f.project,
            selection_revision: *selection_revision,
            source_id: *source_id,
            item_id: *item_id,
            result: Ok(ExtHttpNotes {
                revision: 0,
                text: "stable".into(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let editor = f.view.read(cx).http_notes.editor.clone();
            window.click(("input", editor.entity_id()), cx);
            editor.update(cx, |editor, cx| {
                let before = editor.document_revision();
                assert_eq!(editor.value().as_ref(), "stable");
                editor.replace_and_mark_text_in_range(
                    Some(0..6),
                    "正在组词",
                    Some(4..4),
                    window,
                    cx,
                );
                assert!(editor.document_revision() > before);
                assert_eq!(editor.value().as_ref(), "正在组词");
                assert!(editor.marked_text_range(window, cx).is_some());
            });
        })
        .unwrap();
    cx.run_until_parked();
    for modifiers in [
        crate::secondary_modifiers(true),
        Modifiers {
            shift: true,
            ..Default::default()
        },
    ] {
        assert_retained(&f, &rows, cx);
        click_row(&f, 2, modifiers, cx);
        assert_retained(&f, &rows, cx);
        assert!(
            drain(&mut f.commands).is_empty(),
            "rejected selection must not read or save a different record"
        );
    }
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("history-filter", cx);
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [
        Command::QueryHttpHistory {
            query: mio_core::HttpHistoryQuery { query_revision, .. },
        },
    ] = commands.as_slice()
    else {
        panic!("one refresh query")
    };
    f.events
        .try_send(Event::HistoryPage {
            project: f.project,
            query_revision: *query_revision,
            offset: 0,
            total: 1,
            rows: vec![rows[2].clone()],
        })
        .unwrap();
    cx.run_until_parked();
    assert_retained(&f, &rows, cx);
    cx.read(|cx| assert!(!f.view.read(cx).history.read(cx).delegate().loading));
    assert!(drain(&mut f.commands).is_empty());
}
