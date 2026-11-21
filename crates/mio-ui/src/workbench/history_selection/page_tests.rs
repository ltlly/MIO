use super::*;
use crate::workbench::tests::{Fixture, drain, fixture, record};
use gpui_kit::test::{TestEventExt, TestWindowExt};
use gpui_kit::{AnyWindowHandle, TestAppContext};
use mio_core::{
    ExtHttpHighlight, ExtHttpHighlightChange, ExtItemSummary, ExtMessageFragment,
    ExtMessageSummary, HttpHistoryQuery, HttpHistoryResult, Uuid,
};

mod filter_and_live;

pub(super) const REQUEST: &[u8] = b"GET /retained HTTP/1.1\r\nHost: fixture.invalid\r\n\r\n\0\xff";

pub(super) fn row(source_id: Uuid, index: u64) -> HistoryRow {
    let item_id = Uuid::new_v4();
    HistoryRow::ExtProxy(Box::new(mio_core::ExtHistoryRow {
        item: ExtItemSummary {
            id: item_id,
            source_id,
            collection: "root.f1.f0".into(),
            index,
            object_address: None,
            state: "mapped".into(),
            message_count: 1,
        },
        request: Some(ExtMessageSummary {
            id: Uuid::new_v4(),
            source_id,
            item_id,
            collection: "root.f1.f0".into(),
            item_index: index,
            role: "request".into(),
            field_id: 15,
            raw_offset: 100,
            raw_bytes: REQUEST.len() as u64,
            body_offset: None,
            declared_version: Some(HttpVersion::Http11),
            method: Some("GET".into()),
            request_target: Some("/retained".into()),
            status: None,
            diagnostic: None,
        }),
        edited_request: None,
        response: None,
        edited_response: None,
        origin: Some("http://127.0.0.1:9001".into()),
        notes_revision: 2,
        notes_preview: "retained note".into(),
        notes_error: None,
        highlight: Some(ExtHttpHighlight {
            revision: 0,
            color: 0,
        }),
        highlight_error: None,
        time_unix_ms: None,
        time_error: None,
    }))
}

pub(super) fn query(f: &mut Fixture) -> HttpHistoryQuery {
    let commands = drain(&mut f.commands);
    let [Command::QueryHttpHistory { query }] = commands.as_slice() else {
        panic!("one HTTP page query, got {} commands", commands.len())
    };
    assert_eq!(query.project, f.project);
    query.clone()
}

pub(super) fn reply(
    f: &Fixture,
    query: &HttpHistoryQuery,
    rows: Vec<HistoryRow>,
    total: usize,
    selected_matches: Vec<(HistoryEntryId, bool)>,
    cx: &mut TestAppContext,
) {
    let selected_rows = cx.read(|cx| {
        let table = f.view.read(cx).history.read(cx).delegate();
        selected_matches
            .iter()
            .filter(|(id, matches)| *matches && !rows.iter().any(|row| row.id() == *id))
            .map(|(id, _)| {
                table
                    .row_by_id(*id)
                    .expect("known selected fixture row")
                    .clone()
            })
            .collect()
    });
    f.events
        .try_send(Event::HttpHistoryPage {
            query: query.clone(),
            result: Ok(HttpHistoryResult {
                total,
                selected_rows,
                rows,
                // Each fixture lists matching selections in their full-result rank order.
                selected_ranks: selected_matches
                    .iter()
                    .filter(|(_, matches)| *matches)
                    .enumerate()
                    .map(|(rank, (id, _))| (*id, rank))
                    .collect(),
                selected_matches,
                live_annotations: Vec::new(),
                live_numbers: Vec::new(),
                row_metadata: Vec::new(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
}

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

pub(super) fn setup(cx: &mut TestAppContext) -> (Fixture, Vec<HistoryRow>) {
    let mut f = fixture(cx, vec![], 64);
    let source = Uuid::new_v4();
    let rows: Vec<_> = (0..4).rev().map(|index| row(source, index)).collect();
    let initial = query(&mut f);
    reply(&f, &initial, rows[..2].to_vec(), 4, vec![], cx);
    click(f.window, "proxy-history", cx);
    if let Ok(command) = f.commands.try_recv() {
        let Command::QueryHttpHistory { query } = command else {
            panic!("opening HTTP history must not perform another operation")
        };
        reply(&f, &query, rows[..2].to_vec(), 4, vec![], cx);
    }
    assert!(drain(&mut f.commands).is_empty());
    (f, rows)
}

pub(super) fn select_and_load(
    f: &mut Fixture,
    visible_index: usize,
    expected: &HistoryRow,
    add: bool,
    cx: &mut TestAppContext,
) {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.click_with_modifiers(
                format!("table-cell-{visible_index}:0"),
                crate::secondary_modifiers(add),
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [
        Command::ReadExtMessage {
            selection_revision,
            message_id,
            offset,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!(
            "selecting a new row reads exactly its request, got {} commands",
            commands.len()
        )
    };
    let HistoryRow::ExtProxy(expected) = expected else {
        unreachable!()
    };
    let summary = expected.request.as_ref().unwrap();
    assert_eq!(*message_id, summary.id);
    assert_eq!(*offset, 0);
    f.events
        .try_send(Event::ExtMessage {
            project: f.project,
            selection_revision: *selection_revision,
            message_id: *message_id,
            offset: *offset,
            result: Ok(ExtMessageFragment {
                summary: summary.clone(),
                offset: *offset,
                bytes: REQUEST.to_vec(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut f.commands).is_empty());
}

pub(super) fn choose_color(
    f: &mut Fixture,
    visible_index: usize,
    color: usize,
    cx: &mut TestAppContext,
) -> (Uuid, Vec<ExtHttpHighlightChange>) {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.right_click(format!("table-cell-{visible_index}:0"), cx);
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let at = window
                .within("popup-menu")
                .find_by_label("Highlight")
                .bounds()
                .center();
            window.pointer_down(at, MouseButton::Left, cx);
            window.pointer_up(at, MouseButton::Left, cx);
        })
        .unwrap();
    cx.run_until_parked();
    click(f.window, ("http-highlight-color", color), cx);
    let commands = drain(&mut f.commands);
    let [
        Command::SetExtHttpHighlights {
            project,
            operation_id,
            changes,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!(
            "one whole selected-color batch, got {} commands",
            commands.len()
        )
    };
    assert_eq!(*project, f.project);
    (*operation_id, changes.clone())
}

pub(super) fn acknowledge_highlights(
    f: &Fixture,
    operation: Uuid,
    changes: &[ExtHttpHighlightChange],
    cx: &mut TestAppContext,
) {
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

pub(super) fn assert_selection(
    f: &Fixture,
    ids: &[HistoryEntryId],
    primary: HistoryEntryId,
    cx: &TestAppContext,
) {
    cx.read(|cx| {
        let view = f.view.read(cx);
        let table = view.history.read(cx).delegate();
        assert_eq!(view.selected_history, Some(primary));
        assert_eq!(table.selected_ids, ids.iter().copied().collect());
        assert_eq!(table.selected_rows.len(), ids.len());
        assert!(ids.iter().all(|id| table.selected_rows.contains_key(id)));
    });
}

#[gpui_kit::test]
fn offpage_selection_keeps_detail_and_late_highlight_receipts_for_the_whole_batch(
    cx: &mut TestAppContext,
) {
    let (mut f, rows) = setup(cx);
    select_and_load(&mut f, 0, &rows[0], false, cx);
    select_and_load(&mut f, 1, &rows[1], true, cx);
    let ids = [rows[0].id(), rows[1].id()];
    let (raw, revision, selection_revision) = cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.history_request.view() == MessageView::Hex);
        (
            view.history_request.hex.read(cx).value().to_string(),
            view.history_request.hex.read(cx).document_revision(),
            view.selection_revision,
        )
    });
    let red = choose_color(&mut f, 0, 1, cx);
    assert_eq!(red.1.len(), 2);
    assert!(
        red.1
            .iter()
            .all(|change| change.expected_highlight_revision == 0)
    );

    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.query_history(2, window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    let next = query(&mut f);
    assert_eq!(next.offset, 2);
    assert_eq!(
        next.selected_ids.iter().copied().collect::<HashSet<_>>(),
        ids.into_iter().collect()
    );
    reply(
        &f,
        &next,
        rows[2..].to_vec(),
        4,
        ids.map(|id| (id, true)).to_vec(),
        cx,
    );
    assert_selection(&f, &ids, rows[1].id(), cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.history.read(cx).selected_row().is_none());
        assert!(view.history_request.view() == MessageView::Hex);
        assert_eq!(view.history_request.hex.read(cx).value().as_ref(), raw);
        assert_eq!(
            view.history_request.hex.read(cx).document_revision(),
            revision
        );
        assert_eq!(view.selection_revision, selection_revision);
        assert_eq!(
            view.history.read(cx).delegate().selection_anchor,
            Some(rows[1].id())
        );
    });
    assert!(
        drain(&mut f.commands).is_empty(),
        "page movement must not reread the body"
    );

    acknowledge_highlights(&f, red.0, &red.1, cx);
    let refresh = query(&mut f);
    reply(
        &f,
        &refresh,
        rows[2..].to_vec(),
        4,
        ids.map(|id| (id, true)).to_vec(),
        cx,
    );
    for id in ids {
        cx.read(|cx| {
            let cache = &f.view.read(cx).history.read(cx).delegate().selected_rows[&id];
            let HistoryRow::ExtProxy(row) = cache else {
                unreachable!()
            };
            assert_eq!(
                row.highlight,
                Some(ExtHttpHighlight {
                    revision: 1,
                    color: 1
                })
            );
            assert_eq!(
                (row.notes_revision, row.notes_preview.as_str()),
                (2, "retained note")
            );
        });
    }

    select_and_load(&mut f, 0, &rows[2], true, cx);
    assert_selection(&f, &[ids[0], ids[1], rows[2].id()], rows[2].id(), cx);
    let green = choose_color(&mut f, 0, 4, cx);
    assert_eq!(
        green.1.len(),
        3,
        "off-page rows are part of the same menu operation"
    );
    for id in ids {
        let HistoryEntryId::External { source_id, item_id } = id else {
            unreachable!()
        };
        assert!(green.1.iter().any(|change| {
            (
                change.source_id,
                change.item_id,
                change.expected_highlight_revision,
                change.color,
            ) == (source_id, item_id, 1, 4)
        }));
    }
    acknowledge_highlights(&f, green.0, &green.1, cx);
    let refresh = query(&mut f);
    let all = [ids[0], ids[1], rows[2].id()];
    reply(
        &f,
        &refresh,
        rows[2..].to_vec(),
        4,
        all.map(|id| (id, true)).to_vec(),
        cx,
    );
    f.window
        .update(cx, |_, window, cx| {
            f.view.update(cx, |view, cx| {
                view.query_history(0, window, cx);
            });
        })
        .unwrap();
    cx.run_until_parked();
    let previous = query(&mut f);
    assert_eq!(previous.offset, 0);
    reply(
        &f,
        &previous,
        rows[..2].to_vec(),
        4,
        all.map(|id| (id, true)).to_vec(),
        cx,
    );
    cx.read(|cx| {
        for row in &f.view.read(cx).history.read(cx).delegate().rows {
            let HistoryRow::ExtProxy(row) = row else {
                unreachable!()
            };
            assert_eq!(
                row.highlight,
                Some(ExtHttpHighlight {
                    revision: 2,
                    color: 4
                }),
                "stale page summaries must not overwrite an acknowledged cached revision"
            );
        }
    });
}
