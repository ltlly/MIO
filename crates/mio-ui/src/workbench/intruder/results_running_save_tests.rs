use mio_core::{IntruderRetention, IntruderRetentionReceipt};

use super::*;

#[path = "results_running_save_tests/part_1.rs"]
mod part_1;

pub(super) fn setup(cx: &mut TestAppContext) -> (Fixture, Started, Vec<IntruderResultSummary>) {
    let mut config = attack();
    config.spec.save_to_project = false;
    let mut fixture = intruder_fixture(cx, vec![config], 64);
    cx.update(|cx| {
        fixture.view.update(cx, |view, _| {
            view.project_path = Some(PathBuf::from("/tmp/mio-running-explicit-save.mio"));
        })
    });
    let mut started = start(&mut fixture, cx);
    let first = flow(0);
    bootstrap(&mut fixture, &mut started, std::slice::from_ref(&first), cx);
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let (revision, _) = query(&mut fixture, started.summary.id);
    started.summary.revision += 1;
    started.summary.dispatched = 2;
    let rows = vec![row(0, &first)];
    emit_page(
        &fixture,
        &started.summary,
        revision,
        rows.clone(),
        false,
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    click(started.window, "intruder-results-settings", cx);
    (fixture, started, rows)
}

pub(super) fn emit_page(
    fixture: &Fixture,
    summary: &IntruderRunSummary,
    query_revision: u64,
    rows: Vec<IntruderResultSummary>,
    persisted: bool,
    cx: &mut TestAppContext,
) {
    let mut event = result_event(fixture, summary, query_revision, rows, None, persisted, cx);
    if let Event::IntruderResults {
        result: Ok(page), ..
    } = &mut event
    {
        assert!(page.rows.len() <= 1, "status probes return at most one row");
        page.next_after = page
            .rows
            .last()
            .filter(|row| row.ordinal + 1 < summary.completed)
            .map(|row| row.ordinal);
    }
    emit(fixture, event, cx);
}

pub(super) fn refresh_display(
    fixture: &mut Fixture,
    summary: &IntruderRunSummary,
    rows: Vec<IntruderResultSummary>,
    persisted: bool,
    cx: &mut TestAppContext,
) {
    let (revision, after) = query(fixture, summary.id);
    assert_eq!(after, None);
    let query = view_query(fixture, summary.id, revision, cx);
    let page = build_view_page(&query, summary, rows, None, persisted);
    emit(
        fixture,
        Event::IntruderResultView {
            query,
            result: Ok(page),
        },
        cx,
    );
}

pub(super) fn result_query_revision(
    command: &Command,
    fixture: &Fixture,
    run_id: Uuid,
) -> Option<u64> {
    match command {
        Command::QueryIntruderResultView { query, cancel } => {
            assert_eq!(
                (query.project, query.run_id, query.view_id),
                (fixture.project, run_id, run_id)
            );
            assert_eq!((query.offset, query.limit), (0, PAGE_SIZE));
            assert_eq!(query.filter, mio_core::IntruderResultFilter::default());
            assert_eq!(query.sort, mio_core::IntruderResultSort::default());
            assert!(!cancel.is_cancelled());
            Some(query.query_revision)
        }
        Command::QueryIntruderResults {
            project,
            run_id: queried_run,
            query_revision,
            after,
            limit,
        } => {
            assert_eq!((*project, *queried_run), (fixture.project, run_id));
            assert_eq!((*after, *limit), (None, 1));
            Some(*query_revision)
        }
        _ => None,
    }
}

pub(super) fn save(
    fixture: &mut Fixture,
    started: &Started,
    cx: &mut TestAppContext,
) -> (Uuid, IntruderRetentionReceipt) {
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        let checkbox = window.find("intruder-result-save-to-project");
        assert_eq!(checkbox.checked(), Some(false));
        assert_ne!(
            checkbox.disabled(),
            Some(true),
            "explicit Save is available while the Session attack is running"
        );
        window.click("intruder-result-save-to-project", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetIntruderRetention {
            project,
            run_id,
            operation_id,
            expected_revision,
            retention: IntruderRetention::Project,
            workspace,
        },
    ] = commands.as_slice()
    else {
        panic!("running Save must submit one retention CAS and no Stop, Start, or Send")
    };
    assert_eq!(
        (*project, *run_id, *expected_revision),
        (
            fixture.project,
            started.summary.id,
            started.summary.revision
        )
    );
    assert!(!started.cancel.is_cancelled());
    let mut run = started.summary.clone();
    run.revision += 1;
    run.retention = IntruderRetention::Project;
    (
        *operation_id,
        IntruderRetentionReceipt {
            run,
            retention: IntruderRetention::Project,
            workspace: *workspace.clone(),
            session_logical_bytes: 0,
        },
    )
}

pub(super) fn retained(
    fixture: &Fixture,
    operation: Uuid,
    receipt: IntruderRetentionReceipt,
    cx: &mut TestAppContext,
) {
    emit(
        fixture,
        Event::IntruderRetentionChanged {
            project: fixture.project,
            run_id: receipt.run.id,
            operation_id: operation,
            result: Ok(receipt),
        },
        cx,
    );
}
