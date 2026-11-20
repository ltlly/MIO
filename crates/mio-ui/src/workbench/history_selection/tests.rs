use super::*;
use crate::workbench::tests::{Fixture, drain, fixture};
use gpui_kit::TestAppContext;
use gpui_kit::test::{TestEventExt, TestWindowExt};
use mio_core::Uuid;

fn setup(cx: &mut TestAppContext) -> (Fixture, Vec<HistoryRow>) {
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
    let rows: Vec<_> = (0..4)
        .map(|index| {
            HistoryRow::ExtProxy(Box::new(mio_core::ExtHistoryRow {
                item: mio_core::ExtItemSummary {
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
                highlight: Some(mio_core::ExtHttpHighlight {
                    revision: 0,
                    color: 1,
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
            total: rows.len(),
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
    (f, rows)
}

fn click(
    f: &Fixture,
    row: usize,
    modifiers: Modifiers,
    button: MouseButton,
    cx: &mut TestAppContext,
) {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let p = window.find(format!("table-cell-{row}:0")).bounds().center();
            window.change_modifiers(modifiers, cx);
            window.pointer_down(p, button, cx);
            window.pointer_up(p, button, cx);
            window.change_modifiers(Modifiers::default(), cx);
        })
        .unwrap();
    cx.run_until_parked();
}

fn selected(f: &Fixture, cx: &TestAppContext) -> HashSet<HistoryEntryId> {
    cx.read(|cx| {
        f.view
            .read(cx)
            .history
            .read(cx)
            .delegate()
            .selected_ids
            .clone()
    })
}

#[gpui_kit::test]
fn cmd_toggle_range_keyboard_and_context_selection_keep_stable_targets(cx: &mut TestAppContext) {
    let (f, rows) = setup(cx);
    let cmd = crate::secondary_modifiers(true);
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    click(&f, 0, Modifiers::default(), MouseButton::Left, cx);
    click(&f, 2, cmd, MouseButton::Left, cx);
    assert_eq!(
        selected(&f, cx),
        HashSet::from([rows[0].id(), rows[2].id()])
    );
    click(&f, 0, Modifiers::default(), MouseButton::Right, cx);
    assert_eq!(selected(&f, cx).len(), 2);
    f.window
        .update(cx, |_, w, cx| w.press("escape", cx))
        .unwrap();
    click(&f, 2, cmd, MouseButton::Left, cx);
    assert_eq!(selected(&f, cx), HashSet::from([rows[0].id()]));
    click(&f, 3, shift, MouseButton::Left, cx);
    assert_eq!(
        selected(&f, cx),
        HashSet::from([rows[2].id(), rows[3].id()])
    );
    click(&f, 1, Modifiers::default(), MouseButton::Right, cx);
    assert_eq!(selected(&f, cx), HashSet::from([rows[1].id()]));
    f.window
        .update(cx, |_, w, cx| {
            w.press("escape", cx);
            w.press(crate::kbd!("a"), cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(selected(&f, cx).len(), 4);
    click(&f, 0, Modifiers::default(), MouseButton::Left, cx);
    f.window
        .update(cx, |_, w, cx| w.press("shift-down", cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        selected(&f, cx),
        HashSet::from([rows[0].id(), rows[1].id()])
    );
}

#[gpui_kit::test]
fn metadata_refresh_reorders_without_changing_selected_ids_or_anchor(cx: &mut TestAppContext) {
    let (mut f, mut rows) = setup(cx);
    click(&f, 0, Modifiers::default(), MouseButton::Left, cx);
    click(
        &f,
        2,
        crate::secondary_modifiers(true),
        MouseButton::Left,
        cx,
    );
    let expected = selected(&f, cx);
    let anchor = rows[2].id();
    f.window
        .update(cx, |_, w, cx| {
            w.render_frame(cx);
            w.click("history-filter", cx);
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let revision = commands
        .iter()
        .find_map(|c| {
            if let Command::QueryHttpHistory {
                query: mio_core::HttpHistoryQuery { query_revision, .. },
            } = c
            {
                Some(*query_revision)
            } else {
                None
            }
        })
        .unwrap();
    rows.reverse();
    for row in &mut rows {
        if let HistoryRow::ExtProxy(r) = row {
            r.highlight = Some(mio_core::ExtHttpHighlight {
                revision: 1,
                color: 2,
            });
            r.time_unix_ms = Some(1794126317491 + r.item.index as i64);
            r.time_error = None;
        }
    }
    f.events
        .try_send(Event::HistoryPage {
            project: f.project,
            query_revision: revision,
            offset: 0,
            total: 4,
            rows,
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(selected(&f, cx), expected);
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).history.read(cx).delegate().selection_anchor,
            Some(anchor)
        )
    });
    assert!(drain(&mut f.commands).iter().all(|c| !matches!(
        c,
        Command::ReadExtMessage { .. } | Command::ReadExtHttpNotes { .. }
    )));
}

#[gpui_kit::test]
fn cmd_selection_over_the_summary_limit_keeps_the_existing_selection(cx: &mut TestAppContext) {
    let (f, rows) = setup(cx);
    click(&f, 0, Modifiers::default(), MouseButton::Left, cx);
    f.view.update(cx, |view, cx| {
        view.history.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            for index in 1..MAX_SELECTED_HISTORY {
                let mut row = rows[0].clone();
                let HistoryRow::ExtProxy(archive) = &mut row else {
                    unreachable!()
                };
                archive.item.id = Uuid::new_v4();
                archive.item.index = index as u64 + 100;
                let id = row.id();
                delegate.selected_ids.insert(id);
                delegate.selected_order.push(id);
                delegate.selected_ranks.insert(id, index + 100);
                delegate.selected_rows.insert(id, row);
            }
            table.refresh(cx);
        });
    });
    let before = selected(&f, cx);
    assert_eq!(before.len(), MAX_SELECTED_HISTORY);
    click(
        &f,
        1,
        crate::secondary_modifiers(true),
        MouseButton::Left,
        cx,
    );
    cx.read(|cx| {
        let view = f.view.read(cx);
        let delegate = view.history.read(cx).delegate();
        assert_eq!(delegate.selected_ids, before);
        assert_eq!(delegate.selected_rows.len(), MAX_SELECTED_HISTORY);
        assert_eq!(view.selected_history, Some(rows[0].id()));
        assert_eq!(delegate.selection_anchor, Some(rows[0].id()));
        assert_eq!(view.history.read(cx).selected_row(), Some(0));
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("500"))
        );
    });
}
