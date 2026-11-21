use super::*;

#[gpui_kit::test]
fn result_selection_reads_only_flows_and_correlates_success_and_failure(cx: &mut TestAppContext) {
    let mut fixture = setup(cx, 64);
    let run = run();
    let a = flow();
    let mut b = flow();
    b.request.body = vec![0, 255, 7];
    b.response =
        Some(parse_response(b"HTTP/1.1 200 OK\r\n\r\n\0\xff", HttpVersion::Http11).unwrap());
    let (query, _) = open(&mut fixture, run.id, cx);
    page(
        &fixture,
        &run,
        &query,
        vec![
            row(0, &a),
            IntruderResultSummary {
                ordinal: 1,
                outcome: IntruderResultOutcome::Rejected("invalid payload".into()),
            },
            row(2, &b),
        ],
        None,
        true,
        cx,
    );
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    update(&fixture, cx, |view, window, cx| {
        view.selection_revision = 120;
        view.select_intruder_result(run.id, 1, window, cx);
    });
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&run.id]
                .detail_error
                .as_deref(),
            Some("未发送：invalid payload")
        )
    });
    let a_read = select(&mut fixture, run.id, 0, a.summary.id, cx);
    let b_read = select(&mut fixture, run.id, 2, b.summary.id, cx);
    assert_eq!((a_read, b_read), (121, 122));
    detail(&fixture, a_read, &a, cx);
    for (revision, flow_id) in [(a_read, a.summary.id), (b_read, a.summary.id)] {
        emit(
            &fixture,
            Event::FlowDetailError {
                project: fixture.project,
                selection_revision: revision,
                flow_id,
                message: "late/wrong flow".into(),
            },
            cx,
        );
    }
    detail(&fixture, b_read, &a, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.has_intruder_result_read(b_read));
        assert!(view.intruder.results.views[&run.id].detail.is_none());
        assert!(view.intruder.results.views[&run.id].detail_error.is_none());
    });
    emit(
        &fixture,
        Event::FlowDetailError {
            project: fixture.project,
            selection_revision: b_read,
            flow_id: b.summary.id,
            message: "selected read failed".into(),
        },
        cx,
    );
    cx.read(|cx| {
        assert!(
            fixture.view.read(cx).intruder.results.views[&run.id]
                .detail_error
                .as_ref()
                .unwrap()
                .contains("selected read failed")
        )
    });
    let retry = select(&mut fixture, run.id, 2, b.summary.id, cx);
    assert!(retry > b_read);
    detail(&fixture, retry, &b, cx);
    update(&fixture, cx, |view, window, cx| {
        let result = &view.intruder.results.views[&run.id];
        assert_eq!(result.detail.as_deref(), Some(&b));
        assert!(result.request.view() == MessageView::Hex);
        assert!(result.response.view() == MessageView::Hex);
        assert_eq!(
            result.request.bytes(window, cx).unwrap(),
            request_to_raw(&b.request)
        );
        assert_eq!(
            result.response.bytes(window, cx).unwrap(),
            response_to_raw(b.response.as_ref().unwrap())
        );
        assert_eq!(view.workspace, workspace);
    });
}

#[gpui_kit::test]
fn a_new_flow_revision_invalidates_old_detail_and_returns_hidden_editor_focus(
    cx: &mut TestAppContext,
) {
    let mut fixture = setup(cx, 64);
    let mut run = run();
    let mut flow = flow();
    let (query, _) = open(&mut fixture, run.id, cx);
    page(&fixture, &run, &query, vec![row(0, &flow)], None, true, cx);
    let first_read = select(&mut fixture, run.id, 0, flow.summary.id, cx);
    detail(&fixture, first_read, &flow, cx);
    update(&fixture, cx, |view, window, cx| {
        view.intruder.results.views[&run.id]
            .request
            .state()
            .focus_handle(cx)
            .focus(window, cx);
    });
    let old = flow.clone();
    flow.summary.revision += 1;
    flow.summary.state = FlowState::Complete;
    flow.response =
        Some(parse_response(b"HTTP/1.1 200 OK\r\n\r\nlatest", HttpVersion::Http11).unwrap());
    run.revision += 1;
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let refresh = next_query(&mut fixture, run.id, 0);
    page(
        &fixture,
        &run,
        &refresh,
        vec![row(0, &flow)],
        None,
        true,
        cx,
    );
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadFlow {
            selection_revision: latest,
            flow_id,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("new revision must reread only the selected body")
    };
    assert!(*latest > first_read);
    assert_eq!(*flow_id, flow.summary.id);
    update(&fixture, cx, |view, window, _| {
        assert!(
            view.intruder.results.views[&run.id]
                .focus
                .is_focused(window)
        );
        assert!(view.intruder.results.views[&run.id].detail.is_none());
    });
    detail(&fixture, first_read, &old, cx);
    emit(
        &fixture,
        Event::FlowDetailError {
            project: fixture.project,
            selection_revision: first_read,
            flow_id: flow.summary.id,
            message: "late failure".into(),
        },
        cx,
    );
    detail(&fixture, *latest, &flow, cx);
    cx.read(|cx| {
        let result = &fixture.view.read(cx).intruder.results.views[&run.id];
        assert_eq!(result.detail.as_deref(), Some(&flow));
        assert!(result.detail_error.is_none());
    });
}

#[gpui_kit::test]
fn frozen_spec_and_unpersisted_results_do_not_replace_drafts_or_restart_requests(
    cx: &mut TestAppContext,
) {
    let mut fixture = setup(cx, 64);
    let mut run = run();
    run.state = IntruderRunState::Interrupted;
    let (query, spec_read) = open(&mut fixture, run.id, cx);
    let spec = super::tests::attack().spec;
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    page(&fixture, &run, &query, vec![], None, false, cx);
    emit(
        &fixture,
        Event::IntruderSpec {
            project: fixture.project,
            run_id: run.id,
            selection_revision: spec_read,
            result: Ok(Box::new(spec.clone())),
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let results = &view.intruder.results.views[&run.id];
        assert_eq!(results.persisted, Some(false));
        assert_eq!(results.spec.as_deref(), Some(&spec));
        assert_eq!(
            results.summary.as_ref().unwrap().state,
            IntruderRunState::Interrupted
        );
        assert!(view.intruder.runs.is_empty());
        assert_eq!(view.workspace, workspace);
        assert!(view.status.contains("不会重发"));
    });
    update(&fixture, cx, |view, _, cx| {
        view.stop_intruder_run(run.id, cx)
    });
    assert!(drain(&mut fixture.commands).is_empty());
    update(&fixture, cx, |view, window, cx| {
        view.close_intruder_results(run.id, window, cx)
    });
    emit(
        &fixture,
        Event::IntruderSpec {
            project: fixture.project,
            run_id: run.id,
            selection_revision: spec_read,
            result: Err("closed view".into()),
        },
        cx,
    );
    cx.read(|cx| assert!(fixture.view.read(cx).intruder.results.views.is_empty()));
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn discard_cancels_before_a_full_queue_and_only_confirmed_terminal_releases_ownership(
    cx: &mut TestAppContext,
) {
    let mut fixture = setup(cx, 2);
    let mut a = run();
    let b = run();
    let a_cancel = CancellationToken::new();
    let b_cancel = CancellationToken::new();
    update(&fixture, cx, |view, _, cx| {
        for (summary, cancel) in [(a.clone(), a_cancel.clone()), (b.clone(), b_cancel.clone())] {
            view.intruder
                .runs
                .insert(summary.id, IntruderRun { summary, cancel });
        }
        for _ in 0..2 {
            view.commands
                .try_send(Command::QueryIntruderRuns {
                    project: fixture.project,
                    query_revision: 900,
                    after: None,
                    limit: 1,
                })
                .unwrap();
        }
        view.stop_intruder_run(a.id, cx);
    });
    assert!(a_cancel.is_cancelled());
    assert!(!b_cancel.is_cancelled());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.intruder.runs[&a.id].summary.state,
            IntruderRunState::Running
        );
        assert!(view.status.contains("未确认"));
        assert!(view.error.as_ref().unwrap().contains("队列已满"));
    });
    assert_eq!(drain(&mut fixture.commands).len(), 2);
    update(&fixture, cx, |view, window, cx| {
        view.refresh_intruder_results(window, cx)
    });
    let commands = drain(&mut fixture.commands);
    assert_eq!(commands.len(), 2);
    let probes: HashMap<_, _> = commands
        .into_iter()
        .map(|command| {
            let Command::QueryIntruderResults {
                run_id,
                query_revision,
                after: None,
                limit: 1,
                ..
            } = command
            else {
                panic!("terminal confirmation must use bounded run probes")
            };
            (run_id, query_revision)
        })
        .collect();
    a.revision += 1;
    a.state = IntruderRunState::Stopped;
    probe_page(&fixture, &a, probes[&a.id], vec![], cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.intruder.runs.contains_key(&a.id));
        assert!(view.intruder.runs.contains_key(&b.id));
    });
    let mut wrong_b = b.clone();
    wrong_b.tab_id = Uuid::new_v4();
    wrong_b.revision += 1;
    wrong_b.state = IntruderRunState::Stopped;
    probe_page(&fixture, &wrong_b, probes[&b.id], vec![], cx);
    cx.read(|cx| assert!(fixture.view.read(cx).intruder.runs.contains_key(&b.id)));
    assert!(!b_cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn project_change_and_disconnect_invalidate_pending_results_without_claiming_stop(
    cx: &mut TestAppContext,
) {
    let mut fixture = setup(cx, 64);
    let run = run();
    let flow = flow();
    let cancel = CancellationToken::new();
    let (query, _) = open(&mut fixture, run.id, cx);
    page(&fixture, &run, &query, vec![row(0, &flow)], None, true, cx);
    let read = select(&mut fixture, run.id, 0, flow.summary.id, cx);
    update(&fixture, cx, |view, window, cx| {
        view.intruder.runs.insert(
            run.id,
            IntruderRun {
                summary: run.clone(),
                cancel: cancel.clone(),
            },
        );
        request_page(view, run.id, 0, window, cx);
        view.backend_disconnected = true;
        view.intruder_backend_disconnected(cx);
    });
    drain(&mut fixture.commands);
    assert!(cancel.is_cancelled());
    detail(&fixture, read, &flow, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let results = &view.intruder.results.views[&run.id];
        assert!(results.query.pending.is_none());
        assert!(results.spec_pending.is_none());
        assert!(results.query.error.as_ref().unwrap().contains("未确认"));
        assert!(results.detail_error.as_ref().unwrap().contains("未确认"));
        assert!(results.detail.is_none());
        assert_eq!(
            view.intruder.runs[&run.id].summary.state,
            IntruderRunState::Running
        );
    });
    let next = ProjectKey {
        id: Uuid::new_v4(),
        generation: fixture.project.generation + 1,
    };
    emit(
        &fixture,
        Event::ProjectOpened {
            project: next,
            path: None,
            workspace: Workspace::default(),
        },
        cx,
    );
    page(&fixture, &run, &query, vec![row(0, &flow)], None, true, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(next));
        assert!(view.intruder.results.views.is_empty());
        assert!(view.intruder.runs.is_empty());
    });
    assert!(!drain(&mut fixture.commands).iter().any(|command| matches!(
        command,
        Command::StartIntruder { .. } | Command::StopIntruder { .. }
    )));
}
