use super::*;
use crate::workbench::tests::{Fixture, drain, fixture};
use gpui_kit::TestAppContext;
use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use mio_core::ExtItemSummary;

fn row(source_id: Uuid, id: Uuid, color: Option<u8>) -> HistoryRow {
    HistoryRow::ExtProxy(Box::new(mio_core::ExtHistoryRow {
        item: ExtItemSummary {
            id,
            source_id,
            collection: "root.f1.f0".into(),
            index: 0,
            object_address: None,
            state: "mapped".into(),
            message_count: 0,
        },
        request: None,
        edited_request: None,
        response: None,
        edited_response: None,
        origin: None,
        notes_revision: 3,
        notes_preview: "retained note".into(),
        notes_error: Some("original note unavailable".into()),
        highlight: color.map(|color| ExtHttpHighlight { revision: 0, color }),
        highlight_error: color.is_none().then(|| "unknown original color".into()),
        time_unix_ms: None,
        time_error: None,
    }))
}

fn fixture_highlights(cx: &mut TestAppContext) -> (Fixture, HistoryEntryId, HistoryEntryId) {
    let mut f = fixture(cx, vec![], 64);
    let commands = drain(&mut f.commands);
    let [
        Command::QueryHttpHistory {
            query: mio_core::HttpHistoryQuery { query_revision, .. },
        },
    ] = commands.as_slice()
    else {
        panic!("initial history query")
    };
    let source = Uuid::new_v4();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    f.events
        .try_send(Event::HistoryPage {
            project: f.project,
            query_revision: *query_revision,
            offset: 0,
            total: 2,
            rows: vec![row(source, a, Some(0)), row(source, b, Some(0))],
        })
        .unwrap();
    cx.run_until_parked();
    (
        f,
        HistoryEntryId::External {
            source_id: source,
            item_id: a,
        },
        HistoryEntryId::External {
            source_id: source,
            item_id: b,
        },
    )
}

fn apply(f: &Fixture, ids: &[HistoryEntryId], color: u8, cx: &mut TestAppContext) {
    f.view
        .update(cx, |view, cx| view.set_http_highlights(ids, color, cx));
    cx.run_until_parked();
}

fn write(f: &mut Fixture) -> (Uuid, Vec<ExtHttpHighlightChange>, CancellationToken) {
    let commands: Vec<_> = drain(&mut f.commands)
        .into_iter()
        .filter(|command| {
            if let Command::QueryHttpHistory {
                query: mio_core::HttpHistoryQuery { project, .. },
            } = command
            {
                assert_eq!(*project, f.project);
                false
            } else {
                true
            }
        })
        .collect();
    let [
        Command::SetExtHttpHighlights {
            project,
            operation_id,
            changes,
            cancel,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("one atomic Highlight write: {} commands", commands.len())
    };
    assert_eq!(*project, f.project);
    (*operation_id, changes.clone(), cancel.clone())
}

fn ack(f: &Fixture, operation: Uuid, changes: &[ExtHttpHighlightChange], cx: &mut TestAppContext) {
    f.events
        .try_send(Event::ExtHttpHighlightsChanged {
            project: f.project,
            operation_id: operation,
            result: Ok(changes
                .iter()
                .map(|change| {
                    (
                        change.source_id,
                        change.item_id,
                        ExtHttpHighlight {
                            revision: change.expected_highlight_revision + 1,
                            color: change.color,
                        },
                    )
                })
                .collect()),
        })
        .unwrap();
    cx.run_until_parked();
}

fn assert_color(f: &Fixture, index: usize, color: u8, revision: u64, cx: &TestAppContext) {
    cx.read(|cx| {
        let view = f.view.read(cx);
        let HistoryRow::ExtProxy(row) = &view.history.read(cx).delegate().rows[index] else {
            unreachable!()
        };
        assert_eq!(row.highlight, Some(ExtHttpHighlight { color, revision }));
        assert_eq!(row.notes_preview, "retained note");
        assert_eq!(row.notes_revision, 3);
        assert_eq!(
            row.notes_error.as_deref(),
            Some("original note unavailable")
        );
    });
}

#[gpui_kit::test]
fn highlight_atomic_batch_ignores_notes_failure_and_preserves_newer_clear(cx: &mut TestAppContext) {
    let (mut f, first, second) = fixture_highlights(cx);
    apply(&f, &[first, second], 1, cx);
    let red = write(&mut f);
    assert_eq!(red.1.len(), 2);
    assert!(
        red.1
            .iter()
            .all(|change| change.color == 1 && change.expected_highlight_revision == 0)
    );
    apply(&f, &[first], 0, cx);
    assert!(drain(&mut f.commands).is_empty());
    assert_color(&f, 0, 0, 0, cx);
    assert_color(&f, 1, 1, 0, cx);
    ack(&f, red.0, &red.1, cx);
    let clear = write(&mut f);
    assert_eq!(clear.1.len(), 1);
    assert_eq!(clear.1[0].color, 0);
    assert_eq!(clear.1[0].expected_highlight_revision, 1);
    assert_color(&f, 0, 0, 1, cx);
    assert_color(&f, 1, 1, 1, cx);
    ack(&f, clear.0, &clear.1, cx);
    cx.read(|cx| assert!(!f.view.read(cx).http_highlights_unsaved()));
    assert_color(&f, 0, 0, 2, cx);
}

#[gpui_kit::test]
fn highlight_unknown_or_oversized_selection_never_partially_edits(cx: &mut TestAppContext) {
    let (mut f, first, second) = fixture_highlights(cx);
    f.view.update(cx, |view, cx| {
        view.history.update(cx, |table, _| {
            let HistoryRow::ExtProxy(row) = &mut table.delegate_mut().rows[1] else {
                unreachable!()
            };
            row.highlight = None;
            row.highlight_error = Some("unknown source color 42".into());
        })
    });
    apply(&f, &[first, second], 2, cx);
    assert!(drain(&mut f.commands).is_empty());
    assert_color(&f, 0, 0, 0, cx);
    cx.read(|cx| assert!(!f.view.read(cx).http_highlights_unsaved()));
    apply(&f, &vec![first; MAX_HIGHLIGHTS + 1], 2, cx);
    assert!(drain(&mut f.commands).is_empty());
    cx.read(|cx| assert!(!f.view.read(cx).http_highlights_unsaved()));
}

#[gpui_kit::test]
fn highlight_conflict_rebases_retry_and_stale_operation_cannot_settle_it(cx: &mut TestAppContext) {
    let (mut f, first, _) = fixture_highlights(cx);
    apply(&f, &[first], 3, cx);
    let failed = write(&mut f);
    f.events
        .try_send(Event::ExtHttpHighlightsChanged {
            project: f.project,
            operation_id: failed.0,
            result: Err(ExtHttpHighlightError {
                message: "CAS conflict".into(),
                stored: vec![(
                    failed.1[0].source_id,
                    failed.1[0].item_id,
                    ExtHttpHighlight {
                        revision: 7,
                        color: 4,
                    },
                )],
            }),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut f.commands).is_empty());
    assert_color(&f, 0, 3, 7, cx);
    apply(&f, &[first], 5, cx);
    let retry = write(&mut f);
    assert_eq!(retry.1[0].expected_highlight_revision, 7);
    ack(&f, failed.0, &failed.1, cx);
    cx.read(|cx| assert!(f.view.read(cx).http_highlights_pending()));
    assert_color(&f, 0, 5, 7, cx);
    ack(&f, retry.0, &retry.1, cx);
    assert_color(&f, 0, 5, 8, cx);
    cx.read(|cx| assert!(!f.view.read(cx).http_highlights_unsaved()));
}

#[gpui_kit::test]
fn highlight_save_receipt_retains_clear_to_previous_color_and_validates_all_keys(
    cx: &mut TestAppContext,
) {
    let (mut f, first, second) = fixture_highlights(cx);
    f.view.update(cx, |view, _| view.choosing_save_as = true);
    apply(&f, &[first, second], 6, cx);
    assert!(drain(&mut f.commands).is_empty());
    let submitted = f.view.update(cx, |view, _| {
        let submitted = view.snapshot_http_highlights().unwrap();
        view.choosing_save_as = false;
        view.project_saves_in_flight = 1;
        view.project_http_highlights_save = Some(submitted.clone());
        submitted
    });
    apply(&f, &[first], 0, cx);
    let invalid = vec![(submitted[0].source_id, submitted[0].item_id, 1); 2];
    f.view.update(cx, |view, cx| {
        assert!(
            view.accept_saved_http_highlights(&submitted, &invalid, cx)
                .is_err()
        );
        assert_eq!(view.http_highlights.drafts.len(), 2);
        assert!(
            view.http_highlights
                .drafts
                .values()
                .all(|draft| draft.stored.revision == 0)
        );
        view.project_saves_in_flight = 0;
        view.project_http_highlights_save = None;
        let receipts = submitted
            .iter()
            .map(|change| (change.source_id, change.item_id, 1))
            .collect::<Vec<_>>();
        view.accept_saved_http_highlights(&submitted, &receipts, cx)
            .unwrap();
        view.send_http_highlights(false, cx);
    });
    let clear = write(&mut f);
    assert_eq!(clear.1.len(), 1);
    assert_eq!(clear.1[0].color, 0);
    assert_eq!(clear.1[0].expected_highlight_revision, 1);
    assert_color(&f, 0, 0, 1, cx);
    assert_color(&f, 1, 6, 1, cx);
}

#[gpui_kit::test]
fn highlight_unsent_clear_and_disconnect_keep_visible_color_and_pending_ownership(
    cx: &mut TestAppContext,
) {
    let (mut f, first, _) = fixture_highlights(cx);
    f.view.update(cx, |view, _| view.choosing_save_as = true);
    apply(&f, &[first], 8, cx);
    apply(&f, &[first], 0, cx);
    assert_color(&f, 0, 0, 0, cx);
    cx.read(|cx| assert!(!f.view.read(cx).http_highlights_unsaved()));
    f.view.update(cx, |view, _| view.choosing_save_as = false);
    apply(&f, &[first], 9, cx);
    let pending = write(&mut f);
    f.view
        .update(cx, |view, cx| view.http_highlights_disconnected(cx));
    assert!(pending.2.is_cancelled());
    cx.read(|cx| {
        assert!(f.view.read(cx).http_highlights_pending());
        assert!(f.view.read(cx).http_highlights_unsaved());
    });
    assert_color(&f, 0, 9, 0, cx);
}

#[gpui_kit::test]
fn highlight_refresh_preserves_newer_revision_but_does_not_hide_unknown_source(
    cx: &mut TestAppContext,
) {
    let (mut f, first, _) = fixture_highlights(cx);
    apply(&f, &[first], 2, cx);
    let orange = write(&mut f);
    ack(&f, orange.0, &orange.1, cx);
    let source = orange.1[0].source_id;
    let item = orange.1[0].item_id;
    cx.read(|cx| {
        let mut rows = vec![row(source, item, Some(0))];
        f.view.read(cx).refresh_http_highlight_rows(&mut rows, cx);
        let HistoryRow::ExtProxy(restored) = &rows[0] else {
            unreachable!()
        };
        assert_eq!(
            restored.highlight,
            Some(ExtHttpHighlight {
                revision: 1,
                color: 2
            })
        );
        let mut rows = vec![row(source, item, None)];
        f.view.read(cx).refresh_http_highlight_rows(&mut rows, cx);
        let HistoryRow::ExtProxy(unknown) = &rows[0] else {
            unreachable!()
        };
        assert_eq!(unknown.highlight, None);
        assert_eq!(
            unknown.highlight_error.as_deref(),
            Some("unknown original color")
        );
    });
}

#[gpui_kit::test]
fn highlight_context_menu_applies_color_to_original_row_and_keeps_notes_error(
    cx: &mut TestAppContext,
) {
    let (mut f, first, _) = fixture_highlights(cx);
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click("proxy-history", cx);
            window.render_frame(cx);
            window.right_click("table-cell-0:0", cx);
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let position = window.find_by_label("Highlight").bounds().center();
            window.pointer_down(position, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(position, gpui_kit::MouseButton::Left, cx);
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click(("http-highlight-color", 3usize), cx);
        })
        .unwrap();
    cx.run_until_parked();
    let yellow = write(&mut f);
    let HistoryEntryId::External { source_id, item_id } = first else {
        unreachable!()
    };
    assert_eq!(
        (yellow.1[0].source_id, yellow.1[0].item_id),
        (source_id, item_id)
    );
    assert_eq!(yellow.1[0].color, 3);
    assert_color(&f, 0, 3, 0, cx);
}
