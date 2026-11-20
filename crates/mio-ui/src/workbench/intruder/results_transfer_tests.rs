use gpui_kit::test::{TestEventExt, TestWindowExt};
use gpui_kit::{MouseButton, TestAppContext};
use mio_core::{
    FlowState, IntruderBaseline, IntruderResultOutcome, IntruderResultSummary,
    IntruderResultViewRow, IntruderRunPage, TrafficSource,
};

use super::*;
use crate::workbench::intruder::results_query_tests as query_tests;
use crate::workbench::tests::{Fixture, drain, fixture, record, tab};

fn detached(
    cx: &mut TestAppContext,
) -> (
    Fixture,
    AnyWindowHandle,
    IntruderRunSummary,
    [FlowRecord; 2],
) {
    let mut main = tab("source-main");
    main.draft.raw = b"GET /main?q=original HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n".to_vec();
    let mut f = fixture(cx, vec![main], 64);
    drain(&mut f.commands);
    query_tests::click(f.window, "tool-repeater", cx);
    assert!(drain(&mut f.commands).iter().all(|command| matches!(
        command,
        Command::QueryRepeaterHistory { project, .. } if *project == f.project
    )));
    let summary = query_tests::run(2, None);
    f.view.update(cx, |view, cx| {
        view.intruder.results.runs.rows.push(summary.clone());
        view.open_intruder_results_window(summary.id, cx);
    });
    cx.run_until_parked();
    let window = cx.read(|cx| f.view.read(cx).intruder.results.views[&summary.id].owner);
    let (query, _) = query_tests::query(&mut f);
    let flows = ["A", "B"].map(|payload| {
        let mut flow = record(
            &format!("http://127.0.0.1:9002/expanded?q={payload}"),
            "result response must not transfer",
        );
        flow.summary.source = TrafficSource::Intruder;
        flow.summary.state = FlowState::Complete;
        flow
    });
    query_tests::answer(
        &f,
        query.clone(),
        query_tests::page(
            &summary,
            &query,
            flows
                .iter()
                .enumerate()
                .map(|(ordinal, flow)| query_tests::row(ordinal as u64, flow, None))
                .collect(),
            flows.len(),
        ),
        cx,
    );
    assert!(drain(&mut f.commands).is_empty());
    (f, window, summary, flows)
}

fn pending_detail(f: &mut Fixture, flow: &FlowRecord) {
    let commands = drain(&mut f.commands);
    let [
        Command::ReadFlow {
            project, flow_id, ..
        },
    ] = commands.as_slice()
    else {
        panic!("selecting the result reads its detail once, without a transfer or resend")
    };
    assert_eq!((*project, *flow_id), (f.project, flow.summary.id));
}

fn transferred(f: &mut Fixture, flow: &FlowRecord) {
    let commands = drain(&mut f.commands);
    let [
        Command::ReadHistoryTransfer {
            project,
            entries,
            workspace_revision,
            cancel,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("one captured result must only read its transfer proposal")
    };
    assert_eq!(*project, f.project);
    assert_eq!(*workspace_revision, 0);
    assert!(!cancel.is_cancelled());
    assert_eq!(
        entries,
        &[HistoryTransferEntry::Live {
            flow_id: flow.summary.id,
            revision: flow.summary.revision,
            original: false,
        }]
    );
}

#[gpui_kit::test]
fn result_row_menu_keeps_unselected_right_click_and_captured_identity(cx: &mut TestAppContext) {
    let (mut f, window, summary, flows) = detached(cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.right_click("table-cell-1:0", cx);
    })
    .unwrap();
    cx.run_until_parked();
    pending_detail(&mut f, &flows[1]);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let menu = window.within("popup-menu");
        let title = menu.find_by_label("Result #2");
        let intruder = menu.find_by_label("发送到 Intruder");
        let repeater = menu.find_by_label("发送到 Repeater");
        assert!(title.visible() && intruder.visible() && repeater.visible());
        assert!(title.bounds().bottom() <= intruder.bounds().origin.y);
        assert!(intruder.bounds().bottom() <= repeater.bounds().origin.y);
        window.pointer_down(title.bounds().center(), MouseButton::Left, cx);
        window.pointer_up(title.bounds().center(), MouseButton::Left, cx);
        window.render_frame(cx);
        assert!(
            window
                .within("popup-menu")
                .find_by_label("Result #2")
                .visible()
        );
        f.view.update(cx, |view, cx| {
            let result = view.intruder.results.views.get_mut(&summary.id).unwrap();
            assert_eq!(result.selected, Some(1));
            assert!(result.detail.is_none());
            result.query.page.as_mut().unwrap().rows.swap(0, 1);
            cx.notify();
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut f.commands).is_empty());
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let item = window.within("popup-menu").find_by_label("发送到 Repeater");
        window.pointer_down(item.bounds().center(), MouseButton::Left, cx);
        window.pointer_up(item.bounds().center(), MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    transferred(&mut f, &flows[1]);
}

#[gpui_kit::test]
fn workspace_rejected_and_closing_or_discarding_result_view_cancel_in_flight_transfer(
    cx: &mut TestAppContext,
) {
    let (mut f, window, summary, flows) = detached(cx);
    query_tests::click(window, "table-cell-1:0", cx);
    pending_detail(&mut f, &flows[1]);
    f.view.update(cx, |view, _| {
        view.workspace.tabs[0]
            .draft
            .raw
            .extend_from_slice(b"X-Dirty: 1\r\n");
        view.workspace_dirty = true;
    });
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.press(crate::kbd!("r"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    assert!(matches!(
        commands.as_slice(),
        [Command::SaveWorkspace { .. }]
    ));
    let stored = cx.read(|cx| f.view.read(cx).workspace.clone());
    query_tests::emit(
        &f,
        Event::WorkspaceRejected {
            project: f.project,
            revision: stored.revision + 1,
            stored,
            message: "工作区冲突".into(),
        },
        cx,
    );
    assert!(
        cx.read(|cx| f.view.read(cx).history_transfer.is_none()),
        "WorkspaceRejected must clear pending transfer waiting on workspace flush"
    );

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.press(crate::kbd!("r"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut f.commands);
    let [Command::SaveWorkspace { workspace, .. }] = commands.as_slice() else {
        panic!("retrying after WorkspaceRejected must flush dirty workspace first")
    };
    query_tests::emit(
        &f,
        Event::WorkspaceChanged {
            project: f.project,
            workspace: workspace.clone(),
        },
        cx,
    );
    let commands = drain(&mut f.commands);
    let [Command::ReadHistoryTransfer { cancel, .. }] = commands.as_slice() else {
        panic!("workspace ack must dispatch the queued transfer once")
    };
    let cancel = cancel.clone();
    assert!(!cancel.is_cancelled());
    cx.update_window(window, |_, window, cx| {
        f.view.update(cx, |view, cx| {
            view.close_intruder_results(summary.id, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        cancel.is_cancelled(),
        "closing the source result view must immediately cancel the in-flight ReadHistoryTransfer"
    );
}

#[gpui_kit::test]
fn detached_result_shortcuts_use_selected_summary_instead_of_main_draft(cx: &mut TestAppContext) {
    for key in [crate::kbd!("i"), crate::kbd!("r")] {
        let (mut f, window, summary, flows) = detached(cx);
        query_tests::click(window, "table-cell-1:0", cx);
        pending_detail(&mut f, &flows[1]);
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let result = &f.view.read(cx).intruder.results.views[&summary.id];
            assert!(result.focus.contains_focused(window, cx));
            assert_eq!(result.selected, Some(1));
            assert!(result.detail.is_none());
            assert!(matches!(f.view.read(cx).tool, Tool::Repeater));
            assert!(
                f.view.read(cx).workspace.tabs[0]
                    .draft
                    .raw
                    .starts_with(b"GET /main?q=original")
            );
            window.press(key, cx);
        })
        .unwrap();
        cx.run_until_parked();
        transferred(&mut f, &flows[1]);
    }
}

#[gpui_kit::test]
fn embedded_result_shortcuts_stop_propagation_and_transfer_selected_row(cx: &mut TestAppContext) {
    for key in [crate::kbd!("i"), crate::kbd!("r")] {
        let mut main = tab("source-main");
        main.draft.raw = b"GET /main?q=original HTTP/1.1\r\nHost: 127.0.0.1:9001\r\n\r\n".to_vec();
        let mut f = fixture(cx, vec![main], 64);
        drain(&mut f.commands);
        let summary = query_tests::run(2, None);
        query_tests::click(f.window, "tool-dashboard", cx);
        let commands = drain(&mut f.commands);
        let [
            Command::QueryIntruderRuns {
                project,
                query_revision,
                after: None,
                ..
            },
        ] = commands.as_slice()
        else {
            panic!("opening Dashboard must query Intruder runs once")
        };
        assert_eq!(*project, f.project);
        query_tests::emit(
            &f,
            Event::IntruderRuns {
                project: f.project,
                query_revision: *query_revision,
                result: Ok(IntruderRunPage {
                    rows: vec![summary.clone()],
                    next_after: None,
                }),
            },
            cx,
        );
        query_tests::click(
            f.window,
            SharedString::from(format!("dashboard-intruder-{}", summary.id)),
            cx,
        );
        let (query, _) = query_tests::query(&mut f);
        assert_ne!(query.view_id, summary.id);
        let flows = ["A", "B"].map(|payload| {
            let mut flow = record(
                &format!("http://127.0.0.1:9002/embedded?q={payload}"),
                "embedded response",
            );
            flow.summary.source = TrafficSource::Intruder;
            flow.summary.state = FlowState::Complete;
            flow
        });
        query_tests::answer(
            &f,
            query.clone(),
            query_tests::page(
                &summary,
                &query,
                flows
                    .iter()
                    .enumerate()
                    .map(|(ordinal, flow)| query_tests::row(ordinal as u64, flow, None))
                    .collect(),
                flows.len(),
            ),
            cx,
        );
        query_tests::click(f.window, "table-cell-1:0", cx);
        pending_detail(&mut f, &flows[1]);
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            let result = &f.view.read(cx).intruder.results.views[&query.view_id];
            assert!(result.focus.contains_focused(window, cx));
            assert_eq!(result.selected, Some(1));
            window.press(key, cx);
        })
        .unwrap();
        cx.run_until_parked();
        transferred(&mut f, &flows[1]);
    }
}

#[gpui_kit::test]
fn rejected_and_pending_baseline_rows_disable_transfer_menu_and_shortcuts(cx: &mut TestAppContext) {
    let mut summary = query_tests::run(2, None);
    summary.state = IntruderRunState::Running;
    summary.dispatched = 1;
    summary.completed = 1;
    summary.baseline = Some(IntruderBaseline {
        ordinal: 1,
        completed: false,
    });
    let (mut f, window, query, _) = query_tests::open(&summary, cx);
    let rows = vec![
        IntruderResultViewRow {
            ordinal: 1,
            result: None,
            payloads: Vec::new(),
            response_length: None,
            notes_revision: 0,
            notes_preview: String::new(),
            highlight: None,
            highlight_revision: 0,
            grep_matches: Vec::new(),
            grep_extracts: Vec::new(),
            grep_payload_hits: Vec::new(),
        },
        IntruderResultViewRow {
            ordinal: 0,
            result: Some(IntruderResultSummary {
                ordinal: 0,
                outcome: IntruderResultOutcome::Rejected("目标连接被拒绝".into()),
            }),
            payloads: Vec::new(),
            response_length: None,
            notes_revision: 0,
            notes_preview: String::new(),
            highlight: None,
            highlight_revision: 0,
            grep_matches: Vec::new(),
            grep_extracts: Vec::new(),
            grep_payload_hits: Vec::new(),
        },
    ];
    query_tests::answer(
        &f,
        query.clone(),
        query_tests::page(&summary, &query, rows, 2),
        cx,
    );
    assert!(drain(&mut f.commands).is_empty());
    for (rank, title_label) in [(0usize, "Result #0"), (1usize, "Result #1")] {
        let cell = SharedString::from(format!("table-cell-{rank}:0"));
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.right_click(cell, cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(drain(&mut f.commands).is_empty());
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            for label in [title_label, "发送到 Intruder", "发送到 Repeater"] {
                let item = window.within("popup-menu").find_by_label(label);
                window.pointer_down(item.bounds().center(), MouseButton::Left, cx);
                window.pointer_up(item.bounds().center(), MouseButton::Left, cx);
                window.render_frame(cx);
                assert!(
                    window
                        .within("popup-menu")
                        .find_by_label(title_label)
                        .visible(),
                    "{label} on rank {rank} must stay disabled"
                );
            }
            window.press("escape", cx);
            window.press(crate::kbd!("r"), cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(drain(&mut f.commands).is_empty());
        cx.read(|cx| assert!(f.view.read(cx).error.is_some()));
    }
}

#[gpui_kit::test]
fn readonly_result_editor_copies_selection_and_dispatches_captured_action(cx: &mut TestAppContext) {
    let (mut f, window, summary, flows) = detached(cx);
    query_tests::click(window, "table-cell-1:0", cx);
    let commands = drain(&mut f.commands);
    let [
        Command::ReadFlow {
            selection_revision,
            flow_id,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("selecting rank 1 must read its flow detail once")
    };
    assert_eq!(*flow_id, flows[1].summary.id);
    query_tests::emit(
        &f,
        Event::FlowDetail {
            project: f.project,
            selection_revision: *selection_revision,
            record: Box::new(flows[1].clone()),
            original: Box::default(),
        },
        cx,
    );
    assert!(drain(&mut f.commands).is_empty());
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let (editor_id, state) = {
            let result = &f.view.read(cx).intruder.results.views[&summary.id];
            (
                result.request.raw.entity_id(),
                result.request.state().clone(),
            )
        };
        window.click(("input", editor_id), cx);
        state.update(cx, |state, cx| {
            state.focus(window, cx);
            state.set_selected_range(0..4, cx);
        });
        window.render_frame(cx);
        assert_eq!(&state.read(cx).value()[..4], "POST");
        assert_eq!(state.read(cx).selected_range(), 0..4);
        assert!(
            state.read(cx).focus_handle(cx).is_focused(window),
            "focused={:?}",
            window.focused(cx)
        );
        window.dispatch_action(Box::new(Copy), cx);
        window.dispatch_action(
            Box::new(TransferIntruderResult {
                project: f.project,
                view_id: summary.id,
                run_id: summary.id,
                owner: window.window_handle(),
                editor_id,
                entry: Ok(HistoryTransferEntry::Live {
                    flow_id: flows[1].summary.id,
                    revision: flows[1].summary.revision,
                    original: false,
                }),
                intruder: false,
            }),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        cx.read(|cx| cx.read_from_clipboard().and_then(|item| item.text())),
        Some("POST".into())
    );
    transferred(&mut f, &flows[1]);
}
