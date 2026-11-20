use super::*;

pub(in crate::workbench::intruder) struct Started {
    pub(in crate::workbench::intruder) window: AnyWindowHandle,
    pub(in crate::workbench::intruder) summary: IntruderRunSummary,
    pub(in crate::workbench::intruder) spec: IntruderSpec,
    pub(in crate::workbench::intruder) cancel: CancellationToken,
    pub(in crate::workbench::intruder) probe: u64,
}

pub(in crate::workbench::intruder) fn emit(
    fixture: &Fixture,
    event: Event,
    cx: &mut TestAppContext,
) {
    fixture.events.try_send(event).unwrap();
    cx.run_until_parked();
}

pub(in crate::workbench::intruder) fn click(
    window: AnyWindowHandle,
    id: &'static str,
    cx: &mut TestAppContext,
) {
    cx.update_window(window, |_, window, cx| window.click(id, cx))
        .unwrap();
    cx.run_until_parked();
}

pub(in crate::workbench::intruder) fn start(
    fixture: &mut Fixture,
    cx: &mut TestAppContext,
) -> Started {
    click(fixture.window, "tool-intruder", cx);
    click(fixture.window, "intruder-start", cx);
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { project, workspace }] = commands.as_slice() else {
        panic!("Start click must first save its latest input");
    };
    assert_eq!(*project, fixture.project);
    emit(
        fixture,
        Event::WorkspaceChanged {
            project: fixture.project,
            workspace: workspace.clone(),
        },
        cx,
    );
    let commands = drain(&mut fixture.commands);
    let [
        Command::StartIntruder {
            project,
            run_id,
            tab_id,
            spec,
            cancel,
        },
    ] = commands.as_slice()
    else {
        panic!("only the acknowledged snapshot can start");
    };
    assert_eq!(*project, fixture.project);
    let summary = IntruderRunSummary {
        resource_pool: None,
        resource_pool_revision: 0,
        baseline: spec.baseline_request.then_some(mio_core::IntruderBaseline {
            ordinal: 0,
            completed: false,
        }),
        id: *run_id,
        tab_id: *tab_id,
        title: String::new(),
        title_revision: 0,
        revision: 1,
        spec_revision: spec.revision,
        total: spec.validate().unwrap(),
        dispatched: 0,
        completed: 0,
        state: IntruderRunState::Running,
        started_unix_ms: 1,
        retention: if spec.save_to_project {
            mio_core::IntruderRetention::Project
        } else {
            mio_core::IntruderRetention::Session
        },
    };
    let spec = *spec.clone();
    let cancel = cancel.clone();
    emit(
        fixture,
        Event::IntruderStarted {
            project: fixture.project,
            run_id: summary.id,
            tab_id: summary.tab_id,
            spec_revision: summary.spec_revision,
            result: Ok(summary.clone()),
        },
        cx,
    );
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveWorkspace { workspace, .. },
        Command::QueryIntruderResults {
            project,
            run_id,
            query_revision,
            after: None,
            limit: 1,
        },
    ] = commands.as_slice()
    else {
        panic!("a new production window must reuse the in-flight admission probe");
    };
    assert_eq!((*project, *run_id), (fixture.project, summary.id));
    assert_eq!(
        workspace
            .intruder
            .tabs
            .iter()
            .find(|tab| tab.id == summary.tab_id)
            .unwrap()
            .last_run,
        spec.save_to_project.then_some(summary.id)
    );
    emit(
        fixture,
        Event::WorkspaceChanged {
            project: fixture.project,
            workspace: workspace.clone(),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    let window = cx.read(|cx| fixture.view.read(cx).intruder.results.views[&summary.id].owner);
    assert_ne!(window, fixture.window);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("intruder-results-window").visible());
        assert!(window.find("intruder-results-table").visible());
        for (id, selected) in [
            ("intruder-results-results", true),
            ("intruder-results-positions", false),
            ("intruder-result-request", true),
            ("intruder-result-response", false),
        ] {
            assert_eq!(window.find(id).role(), Some(gpui_kit::accesskit::Role::Tab));
            assert_eq!(window.find(id).selected(), Some(selected));
        }
        assert!(window.try_find("result-window-close").is_none());
    })
    .unwrap();
    Started {
        window,
        summary,
        spec,
        cancel,
        probe: *query_revision,
    }
}

pub(in crate::workbench::intruder) fn query(
    fixture: &mut Fixture,
    run_id: Uuid,
) -> (u64, Option<u64>) {
    let commands = drain(&mut fixture.commands);
    match commands.as_slice() {
        [Command::QueryIntruderResultView { query, cancel }] => {
            assert_eq!(
                (query.project, query.run_id, query.view_id),
                (fixture.project, run_id, run_id)
            );
            assert_eq!(query.limit, PAGE_SIZE);
            assert_eq!(query.filter, mio_core::IntruderResultFilter::default());
            assert_eq!(query.sort, mio_core::IntruderResultSort::default());
            assert!(!cancel.is_cancelled());
            (
                query.query_revision,
                query.offset.checked_sub(1).map(|offset| offset as u64),
            )
        }
        [
            Command::QueryIntruderResults {
                project,
                run_id: id,
                query_revision,
                after,
                limit,
            },
        ] => {
            assert_eq!((*project, *id), (fixture.project, run_id));
            assert_eq!(
                *limit, 1,
                "legacy queries are bounded status/close probes, never display pages"
            );
            (*query_revision, *after)
        }
        _ => panic!(
            "one display query or one bounded status probe, without body reads or resends, was expected"
        ),
    }
}

pub(in crate::workbench::intruder) fn view_query(
    fixture: &Fixture,
    run_id: Uuid,
    revision: u64,
    cx: &TestAppContext,
) -> IntruderResultViewQuery {
    cx.read(|cx| {
        let view = &fixture.view.read(cx).intruder.results.views[&run_id];
        let query = &view
            .query
            .pending
            .as_ref()
            .expect("a real display query is pending")
            .query;
        assert_eq!(
            (
                query.project,
                query.run_id,
                query.view_id,
                query.query_revision
            ),
            (fixture.project, run_id, run_id, revision)
        );
        assert_eq!(query.limit, PAGE_SIZE);
        query.clone()
    })
}

pub(in crate::workbench::intruder) fn build_view_page(
    query: &IntruderResultViewQuery,
    summary: &IntruderRunSummary,
    rows: Vec<IntruderResultSummary>,
    next_after: Option<u64>,
    persisted: bool,
) -> IntruderResultViewPage {
    assert_eq!(query.run_id, summary.id);
    assert!(rows.len() <= query.limit);
    let mut rows: Vec<_> = rows
        .into_iter()
        .map(|result| {
            let response_length = match &result.outcome {
                IntruderResultOutcome::Flow(flow) if flow.status.is_some() => {
                    Some(flow.response_bytes)
                }
                _ => None,
            };
            IntruderResultViewRow {
                ordinal: result.ordinal,
                result: Some(result),
                payloads: Vec::new(),
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
        .collect();
    if query.offset == 0
        && summary
            .baseline
            .is_some_and(|baseline| !baseline.completed && baseline.ordinal == summary.dispatched)
        && !rows.iter().any(|row| {
            summary
                .baseline
                .is_some_and(|baseline| baseline.ordinal == row.ordinal)
        })
    {
        rows.insert(
            0,
            IntruderResultViewRow {
                ordinal: summary.baseline.unwrap().ordinal,
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
        );
    }
    if query.sort == mio_core::IntruderResultSort::default() {
        rows.sort_by_key(|row| {
            (
                summary
                    .baseline
                    .is_none_or(|baseline| baseline.ordinal != row.ordinal),
                row.ordinal,
            )
        });
    }
    assert!(rows.len() <= query.limit);
    let total = if next_after.is_some() {
        (summary.dispatched as usize
            + usize::from(summary.baseline.is_some_and(|baseline| {
                !baseline.completed && baseline.ordinal == summary.dispatched
            })))
        .max(query.offset + rows.len() + 1)
    } else {
        query.offset + rows.len()
    };
    let selected_rank = query.selected_ordinal.and_then(|ordinal| {
        rows.iter()
            .position(|row| row.ordinal == ordinal)
            .map(|rank| query.offset + rank)
    });
    IntruderResultViewPage {
        run: summary.clone(),
        offset: query.offset,
        total,
        next_offset: (query.offset + rows.len() < total).then_some(query.offset + rows.len()),
        rows,
        selected_rank,
        selected_row: None,
        persisted,
    }
}

pub(in crate::workbench::intruder) fn result_event(
    fixture: &Fixture,
    summary: &IntruderRunSummary,
    revision: u64,
    rows: Vec<IntruderResultSummary>,
    next_after: Option<u64>,
    persisted: bool,
    cx: &TestAppContext,
) -> Event {
    let pending = cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .intruder
            .results
            .views
            .values()
            .filter(|view| view.run_id == summary.id)
            .find_map(|view| {
                view.query
                    .pending
                    .as_ref()
                    .filter(|pending| pending.query.query_revision == revision)
                    .map(|pending| pending.query.clone())
            })
    });
    if let Some(query) = pending {
        assert_eq!(query.project, fixture.project);
        let page = build_view_page(&query, summary, rows, next_after, persisted);
        Event::IntruderResultView {
            query,
            result: Ok(page),
        }
    } else {
        Event::IntruderResults {
            project: fixture.project,
            run_id: summary.id,
            query_revision: revision,
            result: Ok(IntruderResultPage {
                run: summary.clone(),
                rows,
                next_after,
                persisted,
            }),
        }
    }
}

pub(in crate::workbench::intruder) fn displayed_rows(
    view: &results::ResultView,
) -> Vec<IntruderResultSummary> {
    view.query
        .page
        .as_ref()
        .expect("a result-view page was accepted")
        .rows
        .iter()
        .filter_map(|row| row.result.clone())
        .collect()
}

pub(in crate::workbench::intruder) fn flow(index: usize) -> FlowRecord {
    let mut flow = record(
        &format!("http://127.0.0.1:1/result/{index}"),
        &format!("body-{index}"),
    );
    flow.summary.source = TrafficSource::Intruder;
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

pub(in crate::workbench::intruder) fn page(
    fixture: &Fixture,
    started: &Started,
    query_revision: u64,
    rows: Vec<IntruderResultSummary>,
    next_after: Option<u64>,
    cx: &mut TestAppContext,
) {
    let event = result_event(
        fixture,
        &started.summary,
        query_revision,
        rows,
        next_after,
        true,
        cx,
    );
    emit(fixture, event, cx);
}
