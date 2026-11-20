use super::*;

#[gpui_kit::test]
fn finished_task_adds_one_baseline_without_replaying_rows_or_changing_frozen_spec(
    cx: &mut TestAppContext,
) {
    let (mut fixture, mut started, mut rows) = setup(cx);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let (operation, admission) = request(&mut fixture, &started, cx);
    let accepted = receipt(&started);
    emit(
        &fixture,
        Event::IntruderBaselineRequested {
            project: fixture.project,
            run_id: started.summary.id,
            operation_id: Uuid::new_v4(),
            result: Ok(accepted.clone()),
        },
        cx,
    );
    assert!(!admission.is_cancelled());
    cx.read(|cx| {
        assert!(
            fixture
                .view
                .read(cx)
                .intruder_baseline_pending(started.summary.id)
        )
    });
    emit(
        &fixture,
        Event::IntruderBaselineRequested {
            project: fixture.project,
            run_id: started.summary.id,
            operation_id: operation,
            result: Ok(accepted.clone()),
        },
        cx,
    );
    assert!(admission.is_cancelled());
    assert!(!accepted.cancel.is_cancelled());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, original);
        assert_eq!(
            cached_results(&view.intruder.results.views[&started.summary.id]),
            rows
        );
        assert_eq!(
            view.intruder.runs[&started.summary.id].summary,
            accepted.run
        );
        assert!(!view.intruder_baseline_pending(started.summary.id));
    });
    let (revision, _) = query(&mut fixture, started.summary.id);
    let refresh_query = view_query(&fixture, started.summary.id, revision, cx);
    started.summary = accepted.run;
    started.summary.revision += 1;
    started.summary.dispatched += 1;
    let mut baseline_flow = flow(2);
    baseline_flow.summary.state = mio_core::FlowState::Pending;
    rows.push(row(2, &baseline_flow));
    emit(
        &fixture,
        Event::IntruderResultView {
            query: refresh_query.clone(),
            result: Ok(build_view_page(
                &refresh_query,
                &started.summary,
                rows.clone(),
                None,
                true,
            )),
        },
        cx,
    );
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("intruder-result-baseline").checked(),
            Some(true)
        );
        assert_eq!(
            window.find("intruder-result-baseline").disabled(),
            Some(true)
        );
        assert!(window.find_by_label("运行中  2/2 · 等待基准请求").visible());
        let baseline = window.find(("intruder-result-row", 2_usize)).bounds();
        let first = window.find(("intruder-result-row", 0_usize)).bounds();
        let second = window.find(("intruder-result-row", 1_usize)).bounds();
        assert!(
            baseline.origin.y < first.origin.y && first.origin.y < second.origin.y,
            "Request order moves baseline visually first without changing logical row IDs"
        );
        window.click("intruder-result-baseline", cx);
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    let read = select(&mut fixture, &started, 2, &baseline_flow, cx);
    detail(&fixture, read, &baseline_flow, cx);
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .detail
                .as_deref(),
            Some(&baseline_flow)
        )
    });
    assert!(!accepted.cancel.is_cancelled());
    let mut first_payload = flow(0);
    let IntruderResultOutcome::Flow(first_summary) = &rows[0].outcome else {
        panic!("payload row")
    };
    first_payload.summary = first_summary.clone();
    let read = select(&mut fixture, &started, 0, &first_payload, cx);
    detail(&fixture, read, &first_payload, cx);
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id].selected,
            Some(0),
            "clicking the second displayed row selects original payload ordinal zero"
        )
    });
}

#[gpui_kit::test]
fn baseline_cas_error_preserves_task_and_timeout_cancels_only_admission(cx: &mut TestAppContext) {
    let (mut fixture, started, rows) = setup(cx);
    let (operation, _) = request(&mut fixture, &started, cx);
    emit(
        &fixture,
        Event::IntruderBaselineRequested {
            project: fixture.project,
            run_id: started.summary.id,
            operation_id: operation,
            result: Err("revision 已改变；未发送".into()),
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.intruder_baseline_pending(started.summary.id));
        assert_eq!(
            cached_results(&view.intruder.results.views[&started.summary.id]),
            rows
        );
        assert!(
            view.intruder.results.views[&started.summary.id]
                .baseline_error
                .as_ref()
                .unwrap()
                .contains("revision")
        );
    });
    let (query_revision, after) = query(&mut fixture, started.summary.id);
    assert_eq!(after, None);
    cx.read(|cx| {
        let result = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert_eq!(result.pending, Some(query_revision));
        assert!(result.query.pending.is_none());
    });
    page(
        &fixture,
        &started,
        query_revision,
        vec![rows[0].clone()],
        Some(rows[0].ordinal),
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    let (operation, admission) = request(&mut fixture, &started, cx);
    let accepted = receipt(&started);
    cx.dispatcher
        .scheduler()
        .clock()
        .advance(std::time::Duration::from_secs(31));
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.on_intruder_baseline_requested(
                fixture.project,
                started.summary.id,
                operation,
                Ok(accepted.clone()),
                window,
                cx,
            );
            assert!(!view.intruder_baseline_pending(started.summary.id));
            assert_eq!(
                cached_results(&view.intruder.results.views[&started.summary.id]),
                rows
            );
        })
    })
    .unwrap();
    cx.run_until_parked();
    assert!(admission.is_cancelled());
    assert!(
        !accepted.cancel.is_cancelled(),
        "late admission cancellation must not stop the accepted baseline"
    );
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryIntruderResults {
            project,
            run_id,
            query_revision: status_revision,
            after: None,
            limit: 1,
        },
        Command::QueryIntruderResultView { query, cancel },
    ] = commands.as_slice()
    else {
        panic!(
            "timeout needs one status probe and the accepted late ACK needs one display refresh, without replay"
        )
    };
    assert_eq!((*project, *run_id), (fixture.project, started.summary.id));
    assert_eq!(
        (
            query.project,
            query.run_id,
            query.view_id,
            query.offset,
            query.limit
        ),
        (
            fixture.project,
            started.summary.id,
            started.summary.id,
            0,
            PAGE_SIZE
        )
    );
    assert!(query.query_revision > *status_revision);
    assert_eq!(query.filter, mio_core::IntruderResultFilter::default());
    assert_eq!(query.sort, mio_core::IntruderResultSort::default());
    assert!(!cancel.is_cancelled());
}

#[gpui_kit::test]
fn completed_session_discard_cancels_pending_baseline_before_any_terminal_delete(
    cx: &mut TestAppContext,
) {
    discard_pending_baseline(false, cx);
}

#[gpui_kit::test]
fn completed_session_discard_waits_for_accepted_baseline_to_stop_before_delete(
    cx: &mut TestAppContext,
) {
    discard_pending_baseline(true, cx);
}

#[gpui_kit::test]
fn close_confirmation_blocks_new_baseline_checkbox_event(cx: &mut TestAppContext) {
    baseline_blocked_by_close(false, cx);
}

#[gpui_kit::test]
fn retention_save_blocks_new_baseline_checkbox_event(cx: &mut TestAppContext) {
    baseline_blocked_by_close(true, cx);
}
