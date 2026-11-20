use gpui_kit::test::TestWindowExt;
use gpui_kit::{AnyWindowHandle, TestAppContext, WeakEntity, WindowBounds, WindowOptions, size};
use mio_core::{
    FlowState, IntruderResultOutcome, IntruderResultPage, IntruderResultSummary,
    IntruderResultViewPage, IntruderResultViewQuery, IntruderResultViewRow, IntruderRunPage,
    IntruderRunState, TrafficSource,
};

use super::*;
use crate::workbench::tests::{Fixture, drain, fixture, record, tab};

mod part_1;
mod part_2;
mod part_3;
mod part_4;
mod part_5;

pub(super) fn detail(
    fixture: &Fixture,
    selection_revision: u64,
    flow: &FlowRecord,
    cx: &mut TestAppContext,
) {
    emit(
        fixture,
        Event::FlowDetail {
            project: fixture.project,
            selection_revision,
            record: Box::new(flow.clone()),
            original: Box::default(),
        },
        cx,
    );
}

pub(super) fn named_revision_fixture(
    cx: &mut TestAppContext,
) -> (Fixture, IntruderRunSummary, u64) {
    let mut fixture = setup(cx, 64);
    let mut summary = run();
    summary.revision = 5;
    summary.state = IntruderRunState::Paused;
    summary.title = "已确认的任务名".into();
    summary.title_revision = 1;
    let (query, _) = open(&mut fixture, summary.id, cx);
    page(&fixture, &summary, &query, vec![], None, true, cx);
    update(&fixture, cx, |view, window, cx| {
        view.intruder.results.runs.rows = vec![summary.clone()];
        view.intruder.runs.insert(
            summary.id,
            IntruderRun {
                summary: summary.clone(),
                cancel: CancellationToken::new(),
            },
        );
        view.query_intruder_results(summary.id, None, window, cx);
    });
    let query = next_probe(&mut fixture, summary.id);
    (fixture, summary, query)
}

pub(super) fn assert_summary_caches(
    fixture: &Fixture,
    expected: &IntruderRunSummary,
    cx: &TestAppContext,
) {
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.intruder.results.views[&expected.id].summary.as_ref(),
            Some(expected)
        );
        assert_eq!(
            view.intruder.results.runs.rows.as_slice(),
            std::slice::from_ref(expected)
        );
        assert_eq!(&view.intruder.runs[&expected.id].summary, expected);
    });
}
pub(in crate::workbench::intruder) use part_1::*;
