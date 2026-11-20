use super::*;

pub(in crate::workbench::intruder) fn update<R>(
    fixture: &Fixture,
    cx: &mut TestAppContext,
    action: impl FnOnce(&mut Workbench, &mut Window, &mut Context<Workbench>) -> R,
) -> R {
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| action(view, window, cx))
    })
    .unwrap()
}

pub(in crate::workbench::intruder) fn setup(cx: &mut TestAppContext, capacity: usize) -> Fixture {
    let mut fixture = fixture(cx, vec![tab("existing draft")], capacity);
    drain(&mut fixture.commands);
    fixture
}

pub(in crate::workbench::intruder) fn run() -> IntruderRunSummary {
    IntruderRunSummary {
        resource_pool: None,
        resource_pool_revision: 0,
        baseline: None,
        id: Uuid::new_v4(),
        tab_id: Uuid::new_v4(),
        title: String::new(),
        title_revision: 0,
        revision: 1,
        spec_revision: 1,
        total: 80,
        dispatched: 3,
        completed: 1,
        state: IntruderRunState::Running,
        started_unix_ms: 0,
        retention: mio_core::IntruderRetention::Project,
    }
}

pub(in crate::workbench::intruder) fn flow() -> FlowRecord {
    let mut flow = record("http://127.0.0.1:1/intruder", "fixture");
    flow.summary.source = TrafficSource::Intruder;
    flow.summary.state = FlowState::Pending;
    flow
}

pub(in crate::workbench::intruder) fn row(
    ordinal: u64,
    flow: &FlowRecord,
) -> IntruderResultSummary {
    IntruderResultSummary {
        ordinal,
        outcome: IntruderResultOutcome::Flow(flow.summary.clone()),
    }
}

pub(in crate::workbench::intruder) fn open(
    fixture: &mut Fixture,
    run_id: Uuid,
    cx: &mut TestAppContext,
) -> (IntruderResultViewQuery, u64) {
    update(fixture, cx, |view, window, cx| {
        view.select_intruder_run(run_id, window, cx);
    });
    opening_queries(fixture, run_id)
}

pub(in crate::workbench::intruder) fn opening_queries(
    fixture: &mut Fixture,
    run_id: Uuid,
) -> (IntruderResultViewQuery, u64) {
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryIntruderResultView { query, cancel },
        Command::ReadIntruderSpec {
            project: spec_project,
            run_id: spec_id,
            selection_revision,
        },
    ] = commands.as_slice()
    else {
        panic!("opening a results view reads only one bounded page and its frozen spec");
    };
    assert_eq!(
        (query.project, *spec_project),
        (fixture.project, fixture.project)
    );
    assert_eq!(
        (query.run_id, query.view_id, *spec_id),
        (run_id, run_id, run_id)
    );
    assert_eq!((query.offset, query.limit), (0, PAGE_SIZE));
    assert!(query.selected_ordinal.is_none());
    assert_eq!(query.filter, Default::default());
    assert_eq!(query.sort, Default::default());
    assert!(!cancel.is_cancelled());
    (query.clone(), *selection_revision)
}

pub(in crate::workbench::intruder) fn emit(
    fixture: &Fixture,
    event: Event,
    cx: &mut TestAppContext,
) {
    fixture.events.try_send(event).unwrap();
    cx.run_until_parked();
}

pub(in crate::workbench::intruder) fn emit_from_main(
    fixture: &Fixture,
    event: Event,
    cx: &mut TestAppContext,
) {
    update(fixture, cx, |view, window, cx| {
        view.on_event(event, window, cx);
        view.refresh_intruder_results(window, cx);
    });
    cx.run_until_parked();
}

pub(in crate::workbench::intruder) fn page(
    fixture: &Fixture,
    run: &IntruderRunSummary,
    query: &IntruderResultViewQuery,
    rows: Vec<IntruderResultSummary>,
    total: Option<usize>,
    persisted: bool,
    cx: &mut TestAppContext,
) {
    emit(
        fixture,
        Event::IntruderResultView {
            query: query.clone(),
            result: Ok(view_page(run, query, rows, total, persisted)),
        },
        cx,
    );
}

pub(in crate::workbench::intruder) fn view_page(
    run: &IntruderRunSummary,
    query: &IntruderResultViewQuery,
    rows: Vec<IntruderResultSummary>,
    total: Option<usize>,
    persisted: bool,
) -> IntruderResultViewPage {
    let total = total.unwrap_or(query.offset + rows.len());
    let selected_rank = query.selected_ordinal.and_then(|ordinal| {
        rows.iter()
            .position(|row| row.ordinal == ordinal)
            .map(|index| query.offset + index)
    });
    IntruderResultViewPage {
        run: run.clone(),
        offset: query.offset,
        next_offset: (query.offset + rows.len() < total).then_some(query.offset + rows.len()),
        total,
        rows: rows
            .into_iter()
            .map(|row| {
                let response_length = match &row.outcome {
                    IntruderResultOutcome::Flow(flow) if flow.status.is_some() => {
                        Some(flow.response_bytes)
                    }
                    _ => None,
                };
                IntruderResultViewRow {
                    ordinal: row.ordinal,
                    result: Some(row),
                    payloads: vec![],
                    response_length,
                    notes_revision: 0,
                    notes_preview: String::new(),
                    highlight: None,
                    highlight_revision: 0,
                    grep_matches: Vec::new(),
                    grep_extracts: Vec::new(),
                    grep_payload_hits: Vec::new(),
                }
            })
            .collect(),
        selected_rank,
        selected_row: None,
        persisted,
    }
}

pub(in crate::workbench::intruder) fn displayed_rows(
    view: &results::ResultView,
) -> Vec<IntruderResultSummary> {
    view.query.page.as_ref().map_or_else(Vec::new, |page| {
        page.rows
            .iter()
            .map(|row| row.result.clone().expect("fixture result row"))
            .collect()
    })
}

pub(in crate::workbench::intruder) fn next_query(
    fixture: &mut Fixture,
    run_id: Uuid,
    offset: usize,
) -> IntruderResultViewQuery {
    let commands = drain(&mut fixture.commands);
    let [Command::QueryIntruderResultView { query, cancel }] = commands.as_slice() else {
        panic!("one result view page query expected")
    };
    assert_eq!(query.project, fixture.project);
    assert_eq!(
        (query.run_id, query.view_id, query.offset),
        (run_id, run_id, offset)
    );
    assert_eq!(query.limit, PAGE_SIZE);
    assert!(!cancel.is_cancelled());
    query.clone()
}

pub(in crate::workbench::intruder) fn request_page(
    view: &mut Workbench,
    run_id: Uuid,
    offset: usize,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let (filter, sort) = view.intruder.results.views[&run_id].query.requested();
    assert!(
        view.query_intruder_result_view(run_id, offset, filter, sort, window, cx)
            .is_some()
    );
}

pub(in crate::workbench::intruder) fn probe_page(
    fixture: &Fixture,
    run: &IntruderRunSummary,
    query_revision: u64,
    rows: Vec<IntruderResultSummary>,
    cx: &mut TestAppContext,
) {
    emit(
        fixture,
        Event::IntruderResults {
            project: fixture.project,
            run_id: run.id,
            query_revision,
            result: Ok(IntruderResultPage {
                run: run.clone(),
                rows,
                next_after: None,
                persisted: true,
            }),
        },
        cx,
    );
}

pub(in crate::workbench::intruder) fn next_probe(fixture: &mut Fixture, run_id: Uuid) -> u64 {
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryIntruderResults {
            project,
            run_id: id,
            query_revision,
            after: None,
            limit: 1,
        },
    ] = commands.as_slice()
    else {
        panic!("one bounded status probe expected")
    };
    assert_eq!(*project, fixture.project);
    assert_eq!(*id, run_id);
    *query_revision
}

pub(in crate::workbench::intruder) fn select(
    fixture: &mut Fixture,
    run_id: Uuid,
    ordinal: u64,
    flow_id: FlowId,
    cx: &mut TestAppContext,
) -> u64 {
    update(fixture, cx, |view, window, cx| {
        view.select_intruder_result(run_id, ordinal, window, cx);
    });
    detail_query(fixture, flow_id)
}

pub(in crate::workbench::intruder) fn detail_query(fixture: &mut Fixture, flow_id: FlowId) -> u64 {
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadFlow {
            project,
            selection_revision,
            flow_id: id,
        },
    ] = commands.as_slice()
    else {
        panic!("only the selected message should be read")
    };
    assert_eq!((*project, *id), (fixture.project, flow_id));
    *selection_revision
}

// A test-only event surface, not the unverified production results-window layout.
pub(in crate::workbench::intruder) struct ResultWindow {
    pub(in crate::workbench::intruder) workbench: WeakEntity<Workbench>,
    pub(in crate::workbench::intruder) run_id: Uuid,
    pub(in crate::workbench::intruder) _subscription: Subscription,
}

impl Render for ResultWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div()
            .id("result-window")
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("result-window-close")
                    .test_support()
                    .h(px(30.))
                    .child("close test window")
                    .on_click(|_, window, _| window.remove_window()),
            );
        let Some(workbench) = self.workbench.upgrade() else {
            return root;
        };
        let Some(view) = workbench.read(cx).intruder.results.views.get(&self.run_id) else {
            return root;
        };
        root = root.track_focus(&view.focus);
        let rows: Vec<_> = view.query.page.as_ref().map_or_else(Vec::new, |page| {
            page.rows.iter().map(|row| row.ordinal).collect()
        });
        let request = view.request.state().clone();
        let response = view.response.state().clone();
        for ordinal in rows {
            let weak = self.workbench.clone();
            let run_id = self.run_id;
            root = root.child(
                div()
                    .id(("result-row", ordinal))
                    .test_support()
                    .h(px(30.))
                    .child(ordinal.to_string())
                    .on_click(move |_, window, cx| {
                        let _ = weak.update(cx, |view, cx| {
                            view.select_intruder_result(run_id, ordinal, window, cx);
                        });
                    }),
            );
        }
        root.child(
            div()
                .flex_1()
                .min_h_0()
                .child(Editor::new(&request).readonly(true).size_full()),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .child(Editor::new(&response).readonly(true).size_full()),
        )
    }
}

pub(in crate::workbench::intruder) fn result_window(
    fixture: &mut Fixture,
    run_id: Uuid,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, IntruderResultViewQuery, u64) {
    let workbench = fixture.view.clone();
    let (window, _) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(gpui_kit::Bounds {
                    origin: Default::default(),
                    size: size(px(700.), px(500.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                workbench.update(cx, |view, cx| view.select_intruder_run(run_id, window, cx));
                cx.new(|cx| ResultWindow {
                    workbench: workbench.downgrade(),
                    run_id,
                    _subscription: cx.observe_in(&workbench, window, |_, _, _, cx| cx.notify()),
                })
            },
        )
        .unwrap()
    });
    let (query, spec) = opening_queries(fixture, run_id);
    (window, query, spec)
}

pub(in crate::workbench::intruder) fn click_result(
    fixture: &mut Fixture,
    window: AnyWindowHandle,
    ordinal: u64,
    flow_id: FlowId,
    cx: &mut TestAppContext,
) -> u64 {
    cx.update_window(window, |_, window, cx| {
        window.click(("result-row", ordinal), cx)
    })
    .unwrap();
    detail_query(fixture, flow_id)
}

pub(in crate::workbench::intruder) fn focus_result(
    fixture: &Fixture,
    window: AnyWindowHandle,
    run_id: Uuid,
    cx: &mut TestAppContext,
) -> FocusHandle {
    let input = cx.read(|cx| {
        fixture.view.read(cx).intruder.results.views[&run_id]
            .request
            .state()
            .clone()
    });
    cx.update_window(window, |_, window, cx| {
        window.click(("input", input.entity_id()), cx);
        let focus = input.focus_handle(cx);
        assert!(focus.is_focused(window));
        focus
    })
    .unwrap()
}

pub(in crate::workbench::intruder) fn focus_main_request(
    fixture: &mut Fixture,
    cx: &mut TestAppContext,
) -> FocusHandle {
    crate::workbench::tests::settings::activate_empty_repeater(fixture, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        let view = fixture.view.read(cx);
        let input = view.tab_editors[&view.workspace.active.unwrap()]
            .request
            .state()
            .clone();
        window.click(("input", input.entity_id()), cx);
        let focus = input.focus_handle(cx);
        assert!(focus.is_focused(window));
        focus
    })
    .unwrap()
}

pub(in crate::workbench::intruder) fn assert_focused(
    window: AnyWindowHandle,
    focus: &FocusHandle,
    cx: &mut TestAppContext,
) {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(focus.is_focused(window));
    })
    .unwrap();
}
