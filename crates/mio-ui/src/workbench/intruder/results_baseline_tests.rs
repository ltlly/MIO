use gpui_kit::test::TestEventExt;
use mio_core::{IntruderBaseline, IntruderBaselineReceipt, IntruderRunPage};

use super::*;

#[path = "results_baseline_tests/part_1.rs"]
mod part_1;
#[path = "results_baseline_tests/part_2.rs"]
mod part_2;
#[path = "results_baseline_tests/part_3.rs"]
mod part_3;
#[path = "results_baseline_tests/part_4.rs"]
mod part_4;

pub(super) fn cached_results(
    view: &crate::workbench::intruder::results::ResultView,
) -> Vec<IntruderResultSummary> {
    let mut rows: Vec<_> = view
        .query
        .page
        .as_ref()
        .expect("display page accepted")
        .rows
        .iter()
        .filter_map(|row| row.result.clone())
        .collect();
    rows.sort_by_key(|row| row.ordinal);
    rows
}

pub(super) fn setup(cx: &mut TestAppContext) -> (Fixture, Started, Vec<IntruderResultSummary>) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    let flows = [flow(0), flow(1)];
    bootstrap(&mut fixture, &mut started, &flows, cx);
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let (revision, _) = query(&mut fixture, started.summary.id);
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Completed;
    let rows = vec![row(0, &flows[0]), row(1, &flows[1])];
    page(&fixture, &started, revision, rows.clone(), None, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    click(started.window, "intruder-results-settings", cx);
    (fixture, started, rows)
}

pub(super) fn request(
    fixture: &mut Fixture,
    started: &Started,
    cx: &mut TestAppContext,
) -> (Uuid, CancellationToken) {
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("intruder-result-baseline").checked(),
            Some(false)
        );
        assert_ne!(
            window.find("intruder-result-baseline").disabled(),
            Some(true)
        );
        window.click("intruder-result-baseline", cx);
        window.click("intruder-result-baseline", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::RequestIntruderBaseline {
            project,
            run_id,
            operation_id,
            expected_revision,
            cancel,
        },
    ] = commands.as_slice()
    else {
        panic!("one explicit baseline admission, no Start or payload replay")
    };
    assert_eq!(
        (*project, *run_id, *expected_revision),
        (
            fixture.project,
            started.summary.id,
            started.summary.revision
        )
    );
    (*operation_id, cancel.clone())
}

pub(super) fn receipt(started: &Started) -> IntruderBaselineReceipt {
    let mut run = started.summary.clone();
    run.revision += 1;
    run.total += 1;
    run.state = IntruderRunState::Running;
    run.baseline = Some(IntruderBaseline {
        ordinal: started.summary.dispatched,
        completed: false,
    });
    IntruderBaselineReceipt {
        run,
        cancel: CancellationToken::new(),
    }
}

pub(super) fn baseline_blocked_by_close(saving: bool, cx: &mut TestAppContext) {
    let (mut fixture, started) =
        result_settings_fixture(true, mio_core::IntruderRetention::Session, cx);
    if saving {
        click(started.window, "intruder-result-save-to-project", cx);
        assert!(
            matches!(drain(&mut fixture.commands).as_slice(), [Command::SetIntruderRetention { run_id, .. }] if *run_id == started.summary.id)
        );
    } else {
        cx.update_window(started.window, |_, window, cx| {
            window.press(crate::kbd!("w"), cx)
        })
        .unwrap();
        cx.run_until_parked();
        assert!(has_close_dialog(cx));
        assert!(drain(&mut fixture.commands).is_empty());
    }
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("intruder-result-baseline").disabled(),
            Some(true)
        );
        window.click("intruder-result-baseline", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "the old result window must not submit a baseline during a retention or close transaction"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close_pending(started.summary.id));
        assert!(!view.intruder_baseline_pending(started.summary.id));
    });
}
pub(in crate::workbench::intruder) use part_1::*;
