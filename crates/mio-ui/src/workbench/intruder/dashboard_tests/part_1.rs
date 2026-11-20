use super::*;

#[test]
fn retention_status_never_calls_session_memory_pending_project_recovery() {
    for persisted in [None, Some(false), Some(true)] {
        let session = retention_note(IntruderRetention::Session, persisted).unwrap();
        assert!(session.contains("会话内存") && session.contains("不会写入项目"));
        assert!(!session.contains("待写盘") && !session.contains("重试写盘"));
    }
    let project = retention_note(IntruderRetention::Project, Some(false)).unwrap();
    assert!(project.contains("待写盘") && project.contains("不会重发请求"));
    assert_eq!(retention_note(IntruderRetention::Project, Some(true)), None);
    assert_eq!(retention_note(IntruderRetention::Project, None), None);
}

#[gpui_kit::test]
fn dashboard_cards_page_both_retention_classes_and_reject_late_pages(cx: &mut TestAppContext) {
    let (mut fixture, project_run, _) = setup(cx);
    click(fixture.window, "tool-dashboard", cx);
    let first = run_query(&mut fixture, None);
    runs(&fixture, first, vec![], None, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("dashboard-empty").visible());
        assert!(window.try_find("intruder-start").is_none());
        assert!(window.try_find("dashboard-next").is_none());
    })
    .unwrap();
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let refreshed = run_query(&mut fixture, None);
    let mut session_run = project_run.clone();
    session_run.id = Uuid::new_v4();
    session_run.retention = IntruderRetention::Session;
    runs(
        &fixture,
        refreshed,
        vec![project_run.clone(), session_run.clone()],
        Some(session_run.id),
        cx,
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find(SharedString::from(format!(
                    "dashboard-intruder-{}",
                    project_run.id
                )))
                .visible()
        );
        assert!(
            window
                .find(SharedString::from(format!(
                    "dashboard-intruder-{}",
                    session_run.id
                )))
                .visible()
        );
        assert!(fixture.view.read(cx).intruder.results.views.is_empty());
        assert!(window.try_find("dashboard-first").is_none());
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.scroll(
            "dashboard-tasks",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-4000.))),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    let next = run_query(&mut fixture, Some(session_run.id));
    assert!(next > refreshed);
    runs(&fixture, refreshed, vec![project_run.clone()], None, cx);
    cx.read(|cx| {
        let page = &fixture.view.read(cx).intruder.results.runs;
        assert!(page.rows.is_empty());
        assert_eq!(page.pending, Some(next));
    });
    runs(&fixture, next, vec![], None, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.scroll(
            "dashboard-tasks",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(4000.))),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    let first_again = run_query(&mut fixture, None);
    runs(&fixture, first_again, vec![session_run], None, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.intruder.results.runs.rows.len(), 1);
        assert!(view.intruder.results.views.is_empty());
    });
    click(fixture.window, "tool-proxy", cx);
    click(fixture.window, "tool-dashboard", cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "revisit must reuse the loaded RunPage"
    );
}

#[gpui_kit::test]
fn dashboard_errors_require_retry_and_disconnect_never_looks_empty(cx: &mut TestAppContext) {
    let (mut fixture, _, _) = setup(cx);
    click(fixture.window, "tool-dashboard", cx);
    let revision = run_query(&mut fixture, None);
    emit(
        &fixture,
        Event::IntruderRuns {
            project: fixture.project,
            query_revision: revision,
            result: Err("fixture list unavailable".into()),
        },
        cx,
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("dashboard-error").visible());
        assert!(window.try_find("dashboard-empty").is_none());
        assert!(window.find("dashboard-retry").visible());
    })
    .unwrap();
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    click(fixture.window, "dashboard-retry", cx);
    let retry = run_query(&mut fixture, None);
    assert!(retry > revision);
    let (replacement, _receiver) = mpsc::channel(1);
    drop(std::mem::replace(&mut fixture.events, replacement));
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("dashboard-error").visible());
        assert_eq!(window.find("dashboard-retry").disabled(), Some(true));
        assert!(
            fixture
                .view
                .read(cx)
                .intruder
                .results
                .runs
                .pending
                .is_none()
        );
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn embedded_and_native_same_run_have_independent_pages_selections_and_frozen_specs(
    cx: &mut TestAppContext,
) {
    let (mut fixture, mut run, spec) = setup(cx);
    run.retention = IntruderRetention::Session;
    show_tasks(&mut fixture, vec![run.clone()], cx);
    let (native, native_query) = native(&mut fixture, &run, cx);
    let mut binary = flow("binary");
    binary.request.body = vec![0, 255, 7];
    let text = flow("separate editor");
    let flows = [binary, text];
    page(&fixture, &run, &native_query, &flows, false, cx);
    let (embedded, embedded_query) = choose(&mut fixture, &run, cx);
    assert_ne!(native_query.query_revision, embedded_query.query_revision);
    cx.read(|cx| {
        let views = &fixture.view.read(cx).intruder.results.views;
        assert_eq!(views.len(), 2);
        assert_eq!(views[&embedded].run_id, run.id);
        assert_eq!(views[&embedded].owner, fixture.window);
        assert_eq!(views[&run.id].owner, native);
        assert!(views[&embedded].query.page.is_none());
        assert_ne!(
            views[&embedded].request.raw.entity_id(),
            views[&run.id].request.raw.entity_id()
        );
    });
    let mut wrong = run.clone();
    wrong.id = Uuid::new_v4();
    let mut wrong_query = embedded_query.clone();
    wrong_query.run_id = wrong.id;
    page(&fixture, &wrong, &wrong_query, &flows, false, cx);
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&embedded]
                .query
                .pending
                .as_ref()
                .map(|pending| &pending.query),
            Some(&embedded_query)
        )
    });
    page(&fixture, &run, &embedded_query, &flows, false, cx);
    click(native, ("intruder-result-row", 0_usize), cx);
    let native_read = read(&mut fixture, flows[0].summary.id);
    click(fixture.window, ("intruder-result-row", 1_usize), cx);
    let embedded_read = read(&mut fixture, flows[1].summary.id);
    assert_ne!(native_read, embedded_read);
    detail(&fixture, embedded_read, &flows[1], cx);
    detail(&fixture, native_read, &flows[0], cx);
    cx.read(|cx| {
        let views = &fixture.view.read(cx).intruder.results.views;
        assert_eq!(views[&run.id].detail.as_deref(), Some(&flows[0]));
        assert_eq!(views[&embedded].detail.as_deref(), Some(&flows[1]));
        assert!(views[&run.id].request.view == MessageView::Hex);
        assert!(views[&embedded].request.view == MessageView::Raw);
    });
    // The native selection is deferred; a main-window selection must not cancel it.
    update(&fixture, cx, |view, window, cx| {
        view.select_intruder_result(run.id, 1, window, cx);
        view.select_intruder_result(embedded, 0, window, cx);
    });
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert_eq!(commands.len(), 2);
    let reads: HashMap<_, _> = commands
        .into_iter()
        .map(|command| {
            let Command::ReadFlow {
                project,
                selection_revision,
                flow_id,
            } = command
            else {
                panic!("independent selections must only load their messages");
            };
            assert_eq!(project, fixture.project);
            (flow_id, selection_revision)
        })
        .collect();
    detail(&fixture, native_read, &flows[0], cx);
    detail(&fixture, embedded_read, &flows[1], cx);
    cx.read(|cx| {
        let views = &fixture.view.read(cx).intruder.results.views;
        assert!(views[&run.id].detail.is_none() && views[&embedded].detail.is_none());
    });
    detail(&fixture, reads[&flows[0].summary.id], &flows[0], cx);
    detail(&fixture, reads[&flows[1].summary.id], &flows[1], cx);
    click(fixture.window, "intruder-results-positions", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadIntruderSpec {
            project,
            run_id,
            selection_revision,
        },
    ] = commands.as_slice()
    else {
        panic!("embedded Positions must read only its frozen spec");
    };
    assert_eq!((*project, *run_id), (fixture.project, run.id));
    emit(
        &fixture,
        Event::IntruderSpec {
            project: fixture.project,
            run_id: run.id,
            selection_revision: *selection_revision,
            result: Ok(Box::new(spec.clone())),
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let views = &view.intruder.results.views;
        assert_eq!(views[&embedded].spec.as_deref(), Some(&spec));
        assert!(views[&run.id].spec.is_none());
        assert_eq!(views[&run.id].detail.as_deref(), Some(&flows[1]));
        assert_eq!(views[&embedded].detail.as_deref(), Some(&flows[0]));
        let target = ResultTarget {
            workbench: fixture.view.downgrade(),
            project: fixture.project,
            run_id: run.id,
            view_id: embedded,
            owner: fixture.window,
            editor_id: views[&embedded].request.raw.entity_id(),
        };
        assert!(target.owns(view));
        assert!(
            !ResultTarget {
                run_id: Uuid::new_v4(),
                ..target
            }
            .owns(view)
        );
    });
    click(fixture.window, "intruder-results-results", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let note = retention_note(IntruderRetention::Session, Some(false)).unwrap();
        assert!(
            gpui_kit::base::test_support::texts(window)
                .iter()
                .any(|text| { text.visible() && !text.is_alternative() && text.text() == note })
        );
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn switching_cards_drops_old_reads_and_keeps_only_one_embedded_view(cx: &mut TestAppContext) {
    let (mut fixture, first, spec) = setup(cx);
    let mut second = first.clone();
    second.id = Uuid::new_v4();
    show_tasks(&mut fixture, vec![first.clone(), second.clone()], cx);
    let (old_view, query) = choose(&mut fixture, &first, cx);
    let flow = flow("old selected task");
    page(
        &fixture,
        &first,
        &query,
        std::slice::from_ref(&flow),
        true,
        cx,
    );
    click(fixture.window, ("intruder-result-row", 0_usize), cx);
    let old_body = read(&mut fixture, flow.summary.id);
    click(fixture.window, "intruder-results-positions", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadIntruderSpec {
            selection_revision: old_spec,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("Positions read expected");
    };
    update(&fixture, cx, |view, window, cx| {
        let (filter, sort) = view.intruder.results.views[&old_view].query.requested();
        assert!(
            view.query_intruder_result_view(old_view, 0, filter, sort, window, cx)
                .is_some()
        );
    });
    let old_page = result_query(&mut fixture, first.id);
    let (second_view, second_query) = choose(&mut fixture, &second, cx);
    detail(&fixture, old_body, &flow, cx);
    emit(
        &fixture,
        Event::IntruderSpec {
            project: fixture.project,
            run_id: first.id,
            selection_revision: *old_spec,
            result: Ok(Box::new(spec)),
        },
        cx,
    );
    page(
        &fixture,
        &first,
        &old_page,
        std::slice::from_ref(&flow),
        true,
        cx,
    );
    cx.read(|cx| {
        let views = &fixture.view.read(cx).intruder.results.views;
        assert_eq!(views.len(), 1);
        assert!(!views.contains_key(&old_view));
        let current = &views[&second_view];
        assert_eq!(current.run_id, second.id);
        assert_eq!(
            current.query.pending.as_ref().map(|pending| &pending.query),
            Some(&second_query)
        );
        assert!(current.query.page.is_none() && current.detail.is_none() && current.spec.is_none());
    });
    page(&fixture, &second, &second_query, &[], true, cx);
    let (reopened, fresh) = choose(&mut fixture, &first, cx);
    assert_ne!(old_view, reopened);
    detail(&fixture, old_body, &flow, cx);
    page(&fixture, &first, &old_page, &[flow], true, cx);
    cx.read(|cx| {
        let views = &fixture.view.read(cx).intruder.results.views;
        assert_eq!(views.len(), 1);
        assert_eq!(
            views[&reopened]
                .query
                .pending
                .as_ref()
                .map(|pending| &pending.query),
            Some(&fresh)
        );
        assert!(views[&reopened].detail.is_none() && views[&reopened].query.page.is_none());
    });
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
    assert!(drain(&mut fixture.commands).is_empty());
}
