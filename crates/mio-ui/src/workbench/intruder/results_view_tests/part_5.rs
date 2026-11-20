use super::*;

#[gpui_kit::test]
fn a_newer_page_before_control_ack_keeps_revision_and_terminal_state(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    let pause = pause_command(&mut fixture, &started, true, cx);
    started.summary.revision += 2;
    started.summary.state = IntruderRunState::Paused;
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let (revision, _) = query(&mut fixture, started.summary.id);
    page(&fixture, &started, revision, vec![], None, cx);
    cx.read(|cx| {
        assert!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .control_pending
                .is_some()
        );
    });
    let mut older = started.summary.clone();
    older.revision -= 1;
    emit(
        &fixture,
        Event::IntruderRunControl {
            project: fixture.project,
            run_id: started.summary.id,
            operation_id: pause,
            result: Ok(older),
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.intruder.runs[&started.summary.id].summary,
            started.summary
        );
        assert_eq!(
            view.intruder.results.views[&started.summary.id]
                .summary
                .as_ref(),
            Some(&started.summary)
        );
    });
    let (revision, _) = query(&mut fixture, started.summary.id);
    page(&fixture, &started, revision, vec![], None, cx);
    let resume = pause_command(&mut fixture, &started, false, cx);
    let mut late = started.summary.clone();
    late.revision += 1;
    late.state = IntruderRunState::Running;
    started.summary.revision += 2;
    started.summary.state = IntruderRunState::Completed;
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let (revision, _) = query(&mut fixture, started.summary.id);
    page(&fixture, &started, revision, vec![], None, cx);
    let (display, after) = query(&mut fixture, started.summary.id);
    assert_eq!(after, None);
    assert!(display > revision);
    let display_query = view_query(&fixture, started.summary.id, display, cx);
    emit(
        &fixture,
        Event::IntruderRunControl {
            project: fixture.project,
            run_id: started.summary.id,
            operation_id: resume,
            result: Ok(late),
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.intruder.runs.contains_key(&started.summary.id));
        assert_eq!(
            view.intruder.results.views[&started.summary.id]
                .summary
                .as_ref(),
            Some(&started.summary)
        );
        assert!(
            view.intruder.results.views[&started.summary.id]
                .control_pending
                .is_none()
        );
    });
    assert!(drain(&mut fixture.commands).is_empty());
    assert_eq!(
        view_query(&fixture, started.summary.id, display, cx),
        display_query,
        "a stale control ACK cannot replace the completed display query"
    );
    cx.read(|cx| {
        assert!(
            !fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .query
                .pending
                .as_ref()
                .unwrap()
                .cancel
                .is_cancelled()
        );
    });
    page(&fixture, &started, display, vec![], None, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let result = &view.intruder.results.views[&started.summary.id];
        assert!(!view.intruder.runs.contains_key(&started.summary.id));
        assert_eq!(result.summary.as_ref(), Some(&started.summary));
        assert_eq!(result.query.page.as_ref().unwrap().run, started.summary);
        assert!(result.query.pending.is_none());
        assert!(result.control_pending.is_none());
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn a_queued_page_before_control_ack_keeps_the_results_window_refreshing(cx: &mut TestAppContext) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    let flow = flow(0);
    bootstrap(&mut fixture, &mut started, std::slice::from_ref(&flow), cx);
    let selection = select(&mut fixture, &started, 0, &flow, cx);
    detail(&fixture, selection, &flow, cx);
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let (pending, after) = query(&mut fixture, started.summary.id);
    assert_eq!(after, None);
    let pending_query = view_query(&fixture, started.summary.id, pending, cx);
    let pause = pause_command(&mut fixture, &started, true, cx);
    cx.read(|cx| {
        let result = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert_ne!(result.owner, fixture.window);
        assert_eq!(
            result
                .query
                .pending
                .as_ref()
                .map(|pending| pending.query.query_revision),
            Some(pending)
        );
        assert!(result.control_pending.is_some());
    });
    let previous = started.summary.clone();
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Paused;
    let capacity = fixture.events.capacity();
    // Queue both replies before the main pump can defer the results-window update.
    for event in [
        Event::IntruderResultView {
            query: pending_query.clone(),
            result: Ok(build_view_page(
                &pending_query,
                &previous,
                vec![row(0, &flow)],
                None,
                true,
            )),
        },
        control_event(&fixture, &started, pause),
    ] {
        fixture.events.try_send(event).unwrap();
    }
    assert_eq!(fixture.events.capacity(), capacity - 2);
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.intruder.runs[&started.summary.id].summary,
            started.summary
        );
        let result = &view.intruder.results.views[&started.summary.id];
        assert_eq!(result.summary.as_ref(), Some(&started.summary));
        assert!(result.control_pending.is_none());
        assert!(result.control_error.is_none());
        assert!(
            result.error.is_none(),
            "a page queued before its control ACK must not block refresh: {:?}",
            result.error
        );
        assert_eq!(result.selected, Some(0));
        assert_eq!(result.detail.as_deref(), Some(&flow));
    });
    let (refresh, after) = query(&mut fixture, started.summary.id);
    assert!(refresh > pending);
    assert_eq!(after, None);
    page(&fixture, &started, refresh, vec![row(0, &flow)], None, cx);
    assert!(!started.cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
    let resume = pause_command(&mut fixture, &started, false, cx);
    assert_ne!(resume, pause);
}

#[gpui_kit::test]
fn backend_disconnect_leaves_control_uncertain_and_disables_its_production_icon(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![attack()], 64);
    let mut started = start(&mut fixture, cx);
    bootstrap(&mut fixture, &mut started, &[], cx);
    pause_command(&mut fixture, &started, true, cx);
    let events = fixture.events.downgrade();
    let (replacement, _receiver) = mpsc::channel(1);
    drop(std::mem::replace(&mut fixture.events, replacement));
    cx.run_until_parked();
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("intruder-result-pause").disabled(), Some(true));
        let view = fixture.view.read(cx);
        assert!(view.backend_disconnected);
        assert!(view.intruder.runs.contains_key(&started.summary.id));
        let result = &view.intruder.results.views[&started.summary.id];
        assert!(result.control_pending.is_none());
        assert!(result.control_error.as_ref().unwrap().contains("未确认"));
        assert_eq!(
            result.summary.as_ref().unwrap().state,
            IntruderRunState::Running
        );
    })
    .unwrap();
    assert!(started.cancel.is_cancelled());
    assert!(events.upgrade().is_none());
    click(started.window, "intruder-result-pause", cx);
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn project_switch_closes_production_windows_and_rejects_old_spec_body_page_and_control(
    cx: &mut TestAppContext,
) {
    let attack = attack();
    let mut fixture = intruder_fixture(cx, vec![attack], 64);
    let mut started = start(&mut fixture, cx);
    let flow = flow(0);
    bootstrap(&mut fixture, &mut started, std::slice::from_ref(&flow), cx);
    let old_body = select(&mut fixture, &started, 0, &flow, cx);
    click(started.window, "intruder-results-positions", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadIntruderSpec {
            selection_revision, ..
        },
    ] = commands.as_slice()
    else {
        panic!("frozen spec read expected")
    };
    let old_spec = *selection_revision;
    let old_control = pause_command(&mut fixture, &started, true, cx);
    let old_project = fixture.project;
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    fixture.project.generation += 1;
    emit(
        &fixture,
        Event::ProjectOpened {
            project: fixture.project,
            path: None,
            workspace,
        },
        cx,
    );
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    assert!(started.cancel.is_cancelled());
    drain(&mut fixture.commands);
    let current = start(&mut fixture, cx);
    for event in [
        Event::IntruderSpec {
            project: old_project,
            run_id: started.summary.id,
            selection_revision: old_spec,
            result: Ok(Box::new(started.spec.clone())),
        },
        Event::FlowDetail {
            project: old_project,
            selection_revision: old_body,
            record: Box::new(flow.clone()),
            original: Box::default(),
        },
        Event::IntruderResults {
            project: old_project,
            run_id: started.summary.id,
            query_revision: started.probe,
            result: Ok(IntruderResultPage {
                run: started.summary.clone(),
                rows: vec![row(0, &flow)],
                next_after: None,
                persisted: true,
            }),
        },
    ] {
        emit(&fixture, event, cx);
    }
    emit(
        &fixture,
        Event::IntruderRunControl {
            project: old_project,
            run_id: started.summary.id,
            operation_id: old_control,
            result: Ok(started.summary.clone()),
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.intruder.results.views.len(), 1);
        let result = &view.intruder.results.views[&current.summary.id];
        assert_eq!(result.owner, current.window);
        assert!(result.detail.is_none() && result.spec.is_none() && result.rows.is_empty());
        assert_eq!(result.pending, Some(current.probe));
        assert_eq!(
            view.workspace.intruder.tabs[0].last_run,
            Some(current.summary.id)
        );
    });
    assert!(!current.cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
}
