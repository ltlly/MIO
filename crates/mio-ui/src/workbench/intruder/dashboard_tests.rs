use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{AnyWindowHandle, TestAppContext};
use mio_core::{
    IntruderResultOutcome, IntruderResultPage, IntruderResultSummary, IntruderResultViewPage,
    IntruderResultViewQuery, IntruderResultViewRow, IntruderRunPage, IntruderRunState,
    TrafficSource,
};

use super::*;
use crate::workbench::intruder::results_view::{ResultTarget, retention_note};
use crate::workbench::intruder::results_view_tests::close_tests::{
    answer_close_dialog, has_close_dialog,
};
use crate::workbench::intruder::tests::{attack, intruder_fixture};
use crate::workbench::tests::{Fixture, drain, record};

#[path = "dashboard_tests/part_1.rs"]
mod part_1;
#[path = "dashboard_tests/part_2.rs"]
mod part_2;
#[path = "dashboard_tests/part_3.rs"]
mod part_3;

pub(super) fn setup(cx: &mut TestAppContext) -> (Fixture, IntruderRunSummary, IntruderSpec) {
    let tab = attack();
    let run = IntruderRunSummary {
        resource_pool: None,
        resource_pool_revision: 0,
        baseline: None,
        id: Uuid::new_v4(),
        tab_id: tab.id,
        title: String::new(),
        title_revision: 0,
        revision: 1,
        spec_revision: tab.spec.revision,
        total: 2,
        dispatched: 2,
        completed: 2,
        state: IntruderRunState::Completed,
        started_unix_ms: 1,
        retention: IntruderRetention::Project,
    };
    let spec = tab.spec.clone();
    (intruder_fixture(cx, vec![tab], 64), run, spec)
}

pub(super) fn update<R>(
    fixture: &Fixture,
    cx: &mut TestAppContext,
    action: impl FnOnce(&mut Workbench, &mut Window, &mut Context<Workbench>) -> R,
) -> R {
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| action(view, window, cx))
    })
    .unwrap()
}

pub(super) fn emit(fixture: &Fixture, event: Event, cx: &mut TestAppContext) {
    fixture.events.try_send(event).unwrap();
    cx.run_until_parked();
}

pub(super) fn click(
    window: AnyWindowHandle,
    id: impl Into<gpui_kit::ElementId>,
    cx: &mut TestAppContext,
) {
    cx.update_window(window, |_, window, cx| window.click(id, cx))
        .unwrap();
    cx.run_until_parked();
}

pub(super) fn run_query(fixture: &mut Fixture, after: Option<Uuid>) -> u64 {
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryIntruderRuns {
            project,
            query_revision,
            after: cursor,
            limit,
        },
    ] = commands.as_slice()
    else {
        panic!("Dashboard must request exactly one summary page, without loading bodies");
    };
    assert_eq!(
        (*project, *cursor, *limit),
        (fixture.project, after, PAGE_SIZE)
    );
    *query_revision
}

pub(super) fn runs(
    fixture: &Fixture,
    revision: u64,
    rows: Vec<IntruderRunSummary>,
    next_after: Option<Uuid>,
    cx: &mut TestAppContext,
) {
    emit(
        fixture,
        Event::IntruderRuns {
            project: fixture.project,
            query_revision: revision,
            result: Ok(IntruderRunPage { rows, next_after }),
        },
        cx,
    );
}

pub(super) fn show_tasks(
    fixture: &mut Fixture,
    rows: Vec<IntruderRunSummary>,
    cx: &mut TestAppContext,
) {
    click(fixture.window, "tool-dashboard", cx);
    let revision = run_query(fixture, None);
    runs(fixture, revision, rows, None, cx);
    assert!(drain(&mut fixture.commands).is_empty());
}

#[track_caller]
pub(super) fn result_query(fixture: &mut Fixture, run_id: Uuid) -> IntruderResultViewQuery {
    let commands = drain(&mut fixture.commands);
    let [Command::QueryIntruderResultView { query, cancel }] = commands.as_slice() else {
        panic!("opening a task must read one bounded result page and no message or frozen spec");
    };
    assert_eq!(
        (query.project, query.run_id, query.offset, query.limit),
        (fixture.project, run_id, 0, PAGE_SIZE)
    );
    assert!(!cancel.is_cancelled());
    query.clone()
}

pub(super) fn choose(
    fixture: &mut Fixture,
    run: &IntruderRunSummary,
    cx: &mut TestAppContext,
) -> (Uuid, IntruderResultViewQuery) {
    click(
        fixture.window,
        SharedString::from(format!("dashboard-intruder-{}", run.id)),
        cx,
    );
    let view_id = cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .intruder
            .dashboard
            .selection
            .as_ref()
            .unwrap()
            .view_id
    });
    assert_ne!(view_id, run.id);
    let query = result_query(fixture, run.id);
    assert_eq!(query.view_id, view_id);
    (view_id, query)
}

pub(super) fn native(
    fixture: &mut Fixture,
    run: &IntruderRunSummary,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, IntruderResultViewQuery) {
    update(fixture, cx, |view, _, cx| {
        view.open_intruder_results_window(run.id, cx)
    });
    cx.run_until_parked();
    let owner = cx.read(|cx| fixture.view.read(cx).intruder.results.views[&run.id].owner);
    assert_ne!(owner, fixture.window);
    let query = result_query(fixture, run.id);
    assert_eq!(query.view_id, run.id);
    (owner, query)
}

pub(super) fn flow(body: &str) -> FlowRecord {
    let mut flow = record("http://127.0.0.1:1/dashboard", body);
    flow.summary.source = TrafficSource::Intruder;
    flow
}

pub(super) fn page(
    fixture: &Fixture,
    run: &IntruderRunSummary,
    query: &IntruderResultViewQuery,
    flows: &[FlowRecord],
    persisted: bool,
    cx: &mut TestAppContext,
) {
    emit(
        fixture,
        Event::IntruderResultView {
            query: query.clone(),
            result: Ok(IntruderResultViewPage {
                run: run.clone(),
                offset: query.offset,
                total: flows.len(),
                rows: flows
                    .iter()
                    .enumerate()
                    .map(|(ordinal, flow)| IntruderResultViewRow {
                        ordinal: ordinal as u64,
                        result: Some(IntruderResultSummary {
                            ordinal: ordinal as u64,
                            outcome: IntruderResultOutcome::Flow(flow.summary.clone()),
                        }),
                        response_length: flow.summary.status.map(|_| flow.summary.response_bytes),
                        notes_revision: 0,
                        notes_preview: String::new(),
                        highlight: None,
                        highlight_revision: 0,
                        payloads: Vec::new(),
                        grep_matches: Vec::new(),
                        grep_extracts: Vec::new(),
                        grep_payload_hits: Vec::new(),
                    })
                    .collect(),
                next_offset: None,
                selected_rank: query
                    .selected_ordinal
                    .and_then(|ordinal| usize::try_from(ordinal).ok())
                    .filter(|ordinal| *ordinal < flows.len()),
                selected_row: None,
                persisted,
            }),
        },
        cx,
    );
}

pub(super) fn read(fixture: &mut Fixture, flow_id: FlowId) -> u64 {
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadFlow {
            project,
            selection_revision,
            flow_id: id,
        },
    ] = commands.as_slice()
    else {
        panic!("only the selected message may be read");
    };
    assert_eq!((*project, *id), (fixture.project, flow_id));
    *selection_revision
}

pub(super) fn detail(fixture: &Fixture, revision: u64, flow: &FlowRecord, cx: &mut TestAppContext) {
    emit(
        fixture,
        Event::FlowDetail {
            project: fixture.project,
            selection_revision: revision,
            record: Box::new(flow.clone()),
            original: Box::default(),
        },
        cx,
    );
}
