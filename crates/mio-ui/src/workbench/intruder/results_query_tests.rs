use gpui_kit::test::TestWindowExt;
use gpui_kit::{AnyWindowHandle, ElementId, TestAppContext};
use mio_core::{
    FlowState, IntruderBaseline, IntruderPayloadPreview, IntruderResultFilter,
    IntruderResultOutcome, IntruderResultSort, IntruderResultSortColumn, IntruderResultSummary,
    IntruderResultViewPage, IntruderResultViewQuery, IntruderResultViewRow, IntruderRetention,
    IntruderRunState, TrafficSource,
};

use super::*;
use crate::workbench::tests::{Fixture, drain, fixture, record};

mod part_1;
mod part_2;

pub(super) fn emit(f: &Fixture, event: Event, cx: &mut TestAppContext) {
    f.events.try_send(event).unwrap();
    cx.run_until_parked();
}

pub(super) fn click(window: AnyWindowHandle, id: impl Into<ElementId>, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

pub(super) fn query(f: &mut Fixture) -> (IntruderResultViewQuery, CancellationToken) {
    let commands = drain(&mut f.commands);
    let [Command::QueryIntruderResultView { query, cancel }] = commands.as_slice() else {
        panic!("expected one bounded result view query, without a spec read or network operation")
    };
    assert_eq!(query.project, f.project);
    assert_eq!(query.limit, PAGE_SIZE);
    (query.clone(), cancel.clone())
}

pub(super) fn run(total: usize, baseline: Option<u64>) -> IntruderRunSummary {
    IntruderRunSummary {
        id: Uuid::new_v4(),
        tab_id: Uuid::new_v4(),
        revision: 2,
        spec_revision: 1,
        total: total as u64,
        dispatched: total as u64,
        completed: total as u64,
        state: IntruderRunState::Completed,
        started_unix_ms: 1,
        retention: IntruderRetention::Project,
        title: "result query fixture".into(),
        title_revision: 0,
        baseline: baseline.map(|ordinal| IntruderBaseline {
            ordinal,
            completed: true,
        }),
        resource_pool: None,
        resource_pool_revision: 0,
    }
}

pub(super) fn open(
    summary: &IntruderRunSummary,
    cx: &mut TestAppContext,
) -> (
    Fixture,
    AnyWindowHandle,
    IntruderResultViewQuery,
    CancellationToken,
) {
    let mut f = fixture(cx, vec![], 64);
    let initial = drain(&mut f.commands);
    assert!(
        initial.iter().all(|command| matches!(command,
            Command::QueryHttpHistory { query } if query.project == f.project
        )),
        "setup may only request the initial HTTP history page"
    );
    f.view.update(cx, |view, cx| {
        view.intruder.results.runs.rows.push(summary.clone());
        view.open_intruder_results_window(summary.id, cx);
    });
    cx.run_until_parked();
    let window = cx.read(|cx| f.view.read(cx).intruder.results.views[&summary.id].owner);
    assert_ne!(window, f.window);
    let (query, cancel) = query(&mut f);
    assert_eq!(
        (query.run_id, query.view_id, query.offset),
        (summary.id, summary.id, 0)
    );
    (f, window, query, cancel)
}

pub(super) fn flow(ordinal: usize, status: u16) -> FlowRecord {
    let mut flow = record(&format!("http://127.0.0.1:1/result/{ordinal}"), "fixture");
    flow.summary.source = TrafficSource::Intruder;
    flow.summary.state = FlowState::Complete;
    flow.summary.status = Some(status);
    flow
}

pub(super) fn row(
    ordinal: u64,
    flow: &FlowRecord,
    payload: Option<&[u8]>,
) -> IntruderResultViewRow {
    IntruderResultViewRow {
        ordinal,
        result: Some(IntruderResultSummary {
            ordinal,
            outcome: IntruderResultOutcome::Flow(flow.summary.clone()),
        }),
        response_length: flow.summary.status.map(|_| flow.summary.response_bytes),
        notes_revision: 0,
        notes_preview: String::new(),
        highlight: None,
        highlight_revision: 0,
        payloads: payload
            .into_iter()
            .map(|bytes| IntruderPayloadPreview {
                set_index: 0,
                payload_index: ordinal as usize,
                raw_bytes: bytes.len() as u64,
                prefix: bytes.to_vec(),
            })
            .collect(),
        grep_matches: Vec::new(),
        grep_extracts: Vec::new(),
        grep_payload_hits: Vec::new(),
    }
}

pub(super) fn page(
    summary: &IntruderRunSummary,
    query: &IntruderResultViewQuery,
    rows: Vec<IntruderResultViewRow>,
    total: usize,
) -> IntruderResultViewPage {
    let selected_rank = query.selected_ordinal.and_then(|ordinal| {
        rows.iter()
            .position(|row| row.ordinal == ordinal)
            .map(|index| query.offset + index)
    });
    IntruderResultViewPage {
        run: summary.clone(),
        offset: query.offset,
        next_offset: (query.offset + rows.len() < total).then_some(query.offset + rows.len()),
        total,
        rows,
        selected_rank,
        selected_row: None,
        persisted: true,
    }
}

pub(super) fn answer(
    f: &Fixture,
    query: IntruderResultViewQuery,
    page: IntruderResultViewPage,
    cx: &mut TestAppContext,
) {
    emit(
        f,
        Event::IntruderResultView {
            query,
            result: Ok(page),
        },
        cx,
    );
}

pub(super) fn select(
    f: &mut Fixture,
    window: AnyWindowHandle,
    rank: usize,
    flow: &FlowRecord,
    cx: &mut TestAppContext,
) {
    click(
        window,
        SharedString::from(format!("table-cell-{rank}:0")),
        cx,
    );
    let commands = drain(&mut f.commands);
    let [
        Command::ReadFlow {
            project,
            selection_revision,
            flow_id,
        },
    ] = commands.as_slice()
    else {
        panic!("a visible rank must read only its stable flow identity")
    };
    assert_eq!((*project, *flow_id), (f.project, flow.summary.id));
    emit(
        f,
        Event::FlowDetail {
            project: f.project,
            selection_revision: *selection_revision,
            record: Box::new(flow.clone()),
            original: Box::default(),
        },
        cx,
    );
    assert!(drain(&mut f.commands).is_empty());
}

pub(super) fn cells(window: AnyWindowHandle, expected: &[(&str, &str)], cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        for (id, label) in expected {
            assert_eq!(
                window.find(SharedString::from((*id).to_owned())).label(),
                Some(*label)
            );
        }
    })
    .unwrap();
}
