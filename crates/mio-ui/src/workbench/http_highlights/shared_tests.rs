use super::*;
use crate::workbench::tests::{Fixture, drain, fixture, pump_native};
use gpui_kit::TestAppContext;
use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};

fn fixture_highlight(cx: &mut TestAppContext) -> (Fixture, Uuid, Uuid) {
    let mut f = fixture(cx, vec![], 64);
    let initial = drain(&mut f.commands);
    let [
        Command::QueryHttpHistory {
            query: mio_core::HttpHistoryQuery { query_revision, .. },
        },
    ] = initial.as_slice()
    else {
        panic!("initial history query")
    };
    let source_id = Uuid::new_v4();
    let item_id = Uuid::new_v4();
    f.events
        .try_send(Event::HistoryPage {
            project: f.project,
            query_revision: *query_revision,
            offset: 0,
            total: 1,
            rows: vec![HistoryRow::ExtProxy(Box::new(mio_core::ExtHistoryRow {
                item: mio_core::ExtItemSummary {
                    id: item_id,
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
            }))],
        })
        .unwrap();
    cx.run_until_parked();
    (f, source_id, item_id)
}

fn choose_color(f: &Fixture, color: usize, cx: &mut TestAppContext) {
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
            window.click(("http-highlight-color", color), cx);
        })
        .unwrap();
    cx.run_until_parked();
}

fn take_write(f: &mut Fixture) -> (Uuid, Vec<ExtHttpHighlightChange>) {
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
            ..
        },
    ] = commands.as_slice()
    else {
        panic!(
            "expected one automatic Highlight write, got {} commands",
            commands.len()
        )
    };
    assert_eq!(*project, f.project);
    (*operation_id, changes.clone())
}

#[gpui_kit::test]
fn cancelling_quit_resumes_newer_highlight_after_older_save_receipt(cx: &mut TestAppContext) {
    let (mut f, source_id, item_id) = fixture_highlight(cx);
    choose_color(&f, 1, cx);
    let (operation_a, changes_a) = take_write(&mut f);
    assert_eq!(
        changes_a,
        [ExtHttpHighlightChange {
            source_id,
            item_id,
            expected_highlight_revision: 0,
            color: 1,
        }]
    );
    choose_color(&f, 2, cx);
    assert!(
        drain(&mut f.commands).is_empty(),
        "A still owns the in-flight batch"
    );
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert_eq!(
            view.selected_history,
            Some(HistoryEntryId::External { source_id, item_id })
        );
        let draft = &view.http_highlights.drafts[&(source_id, item_id)];
        assert_eq!(
            draft.color, 2,
            "the second real menu selection must update B before ACK A"
        );
        assert_eq!(
            draft.stored,
            ExtHttpHighlight {
                revision: 0,
                color: 0
            }
        );
        assert_eq!(
            view.http_highlights.pending.as_ref().unwrap().operation,
            operation_a
        );
        let HistoryRow::ExtProxy(row) = &view.history.read(cx).delegate().rows[0] else {
            unreachable!()
        };
        assert_eq!(
            row.highlight,
            Some(ExtHttpHighlight {
                revision: 0,
                color: 2
            })
        );
    });
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.press(crate::kbd!("q"), cx);
        })
        .unwrap();
    pump_native(cx);
    assert!(cx.has_pending_prompt());
    cx.read(|cx| assert!(f.view.read(cx).transition == Some(TransitionPhase::Confirming)));
    assert!(drain(&mut f.commands).is_empty());
    f.events
        .try_send(Event::ExtHttpHighlightsChanged {
            project: f.project,
            operation_id: operation_a,
            result: Ok(vec![(
                source_id,
                item_id,
                ExtHttpHighlight {
                    revision: 1,
                    color: 1,
                },
            )]),
        })
        .unwrap();
    cx.run_until_parked();
    let while_confirming = drain(&mut f.commands);
    assert!(
        while_confirming
            .iter()
            .all(|command| matches!(command, Command::QueryHttpHistory { .. })),
        "B must wait while the quit sheet owns the decision; got {} commands",
        while_confirming.len()
    );
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.transition == Some(TransitionPhase::Confirming));
        assert!(!view.http_highlights_pending());
        let draft = &view.http_highlights.drafts[&(source_id, item_id)];
        assert_eq!(draft.color, 2);
        assert_eq!(
            draft.stored,
            ExtHttpHighlight {
                revision: 1,
                color: 1
            }
        );
    });
    cx.simulate_prompt_answer("取消");
    pump_native(cx);
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(view.transition.is_none() && view.quit_attempt.is_none());
        assert!(!view.shutdown_requested && !view.quitting);
    });
    let (operation_b, changes_b) = take_write(&mut f);
    assert_ne!(operation_b, operation_a);
    assert_eq!(
        changes_b,
        [ExtHttpHighlightChange {
            source_id,
            item_id,
            expected_highlight_revision: 1,
            color: 2,
        }]
    );
    f.events
        .try_send(Event::ExtHttpHighlightsChanged {
            project: f.project,
            operation_id: operation_b,
            result: Ok(vec![(
                source_id,
                item_id,
                ExtHttpHighlight {
                    revision: 2,
                    color: 2,
                },
            )]),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = f.view.read(cx);
        assert!(!view.http_highlights_pending() && !view.http_highlights_unsaved());
        let HistoryRow::ExtProxy(row) = &view.history.read(cx).delegate().rows[0] else {
            unreachable!()
        };
        assert_eq!(
            row.highlight,
            Some(ExtHttpHighlight {
                revision: 2,
                color: 2
            })
        );
    });
}

#[gpui_kit::test]
fn live_proxy_row_supports_context_menu_highlight_and_notes_sidebar(cx: &mut TestAppContext) {
    let mut f = fixture(cx, vec![], 64);
    let initial = drain(&mut f.commands);
    let [
        Command::QueryHttpHistory {
            query: mio_core::HttpHistoryQuery { query_revision, .. },
        },
    ] = initial.as_slice()
    else {
        panic!("initial history query")
    };
    let live = crate::workbench::tests::record("http://127.0.0.1:9001/live", "ok");
    let flow_id = live.summary.id;
    f.events
        .try_send(Event::HistoryPage {
            project: f.project,
            query_revision: *query_revision,
            offset: 0,
            total: 1,
            rows: vec![HistoryRow::Live(live.summary.clone())],
        })
        .unwrap();
    cx.run_until_parked();
    choose_color(&f, 4, cx);
    let commands = drain(&mut f.commands);
    let hl = commands
        .iter()
        .find_map(|cmd| match cmd {
            Command::SetExtHttpHighlights {
                operation_id,
                changes,
                ..
            } => Some((*operation_id, changes.clone())),
            _ => None,
        })
        .expect("live highlight command");
    assert_eq!(
        hl.1,
        [ExtHttpHighlightChange {
            source_id: Uuid::nil(),
            item_id: flow_id,
            expected_highlight_revision: 0,
            color: 4,
        }]
    );
    f.events
        .try_send(Event::ExtHttpHighlightsChanged {
            project: f.project,
            operation_id: hl.0,
            result: Ok(vec![(
                Uuid::nil(),
                flow_id,
                ExtHttpHighlight {
                    revision: 1,
                    color: 4,
                },
            )]),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let delegate = f.view.read(cx).history.read(cx).delegate();
        assert_eq!(
            delegate.live_highlights.borrow().get(&flow_id),
            Some(&ExtHttpHighlight {
                revision: 1,
                color: 4,
            })
        );
    });

    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.right_click("table-cell-0:0", cx);
        })
        .unwrap();
    cx.run_until_parked();
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            let pos = window.find_by_label("添加备注").bounds().center();
            window.pointer_down(pos, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(pos, gpui_kit::MouseButton::Left, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let read_rev = drain(&mut f.commands)
        .into_iter()
        .find_map(|cmd| match cmd {
            Command::ReadExtHttpNotes {
                selection_revision,
                source_id,
                item_id,
                ..
            } if source_id.is_nil() && item_id == flow_id => Some(selection_revision),
            _ => None,
        })
        .expect("live note read command");
    f.events
        .try_send(Event::ExtHttpNotes {
            project: f.project,
            selection_revision: read_rev,
            source_id: Uuid::nil(),
            item_id: flow_id,
            result: Ok(mio_core::ExtHttpNotes {
                revision: 0,
                text: "实时备注".into(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            f.view.read(cx).http_notes.editor.read(cx).value().as_ref(),
            "实时备注"
        );
    });
}
