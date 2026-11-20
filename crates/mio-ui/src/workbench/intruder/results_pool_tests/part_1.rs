use super::*;

#[gpui_kit::test]
fn result_pool_radio_waits_for_matching_ack_and_preserves_frozen_spec(cx: &mut TestAppContext) {
    let (mut fixture, started, catalog) = setup(cx, 64);
    click(started.window, "intruder-results-positions", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReadIntruderSpec {
            project,
            run_id,
            selection_revision,
        },
    ] = commands.as_slice()
    else {
        panic!("Positions must read the frozen run")
    };
    assert_eq!((*project, *run_id), (fixture.project, started.summary.id));
    emit(
        &fixture,
        Event::IntruderSpec {
            project: fixture.project,
            run_id: started.summary.id,
            selection_revision: *selection_revision,
            result: Ok(Box::new(started.spec.clone())),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    click(started.window, "intruder-results-results", cx);
    let workspace = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let pool = catalog.pools[0].id;
    let (operation, cancel) = request(&mut fixture, &started, pool, cx);
    let changed = accepted(&started, pool);
    for (project, operation_id) in [
        (
            ProjectKey {
                generation: fixture.project.generation + 1,
                ..fixture.project
            },
            operation,
        ),
        (fixture.project, Uuid::new_v4()),
    ] {
        emit(
            &fixture,
            Event::IntruderResourcePoolChanged {
                project,
                run_id: started.summary.id,
                operation_id,
                result: Ok(changed.clone()),
            },
            cx,
        );
        assert!(!cancel.is_cancelled());
        cx.read(|cx| {
            let owner = fixture.view.read(cx);
            assert!(owner.intruder_pool_change_pending(started.summary.id));
            assert_eq!(
                owner.intruder.results.views[&started.summary.id]
                    .summary
                    .as_ref(),
                Some(&started.summary)
            );
        });
        assert!(drain(&mut fixture.commands).is_empty());
    }
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(pool_radio(pool)).checked(), Some(false));
        assert_eq!(window.find(pool_radio(pool)).disabled(), Some(true));
    })
    .unwrap();
    reply(&fixture, &started, operation, Ok(changed.clone()), cx);
    assert!(cancel.is_cancelled());
    assert!(!started.cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let owner = fixture.view.read(cx);
        let view = &owner.intruder.results.views[&started.summary.id];
        assert_eq!(owner.workspace, workspace);
        assert_eq!(view.spec.as_deref(), Some(&started.spec));
        assert_eq!(view.summary.as_ref(), Some(&changed));
        assert_eq!(owner.intruder.runs[&started.summary.id].summary, changed);
        assert!(!owner.intruder_pool_change_pending(started.summary.id));
    });
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(pool_radio(pool)).checked(), Some(true));
    })
    .unwrap();
    select_pool(&started, pool, cx);
    reply(&fixture, &started, operation, Ok(changed), cx);
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn pool_ack_and_running_pages_merge_independent_revisions_without_rollback(
    cx: &mut TestAppContext,
) {
    let (mut fixture, mut started, catalog) = setup(cx, 64);
    let pool = catalog.pools[0].id;
    let (operation, _) = request(&mut fixture, &started, pool, cx);
    let changed = accepted(&started, pool);
    let rows = cx.read(|cx| {
        displayed_rows(&fixture.view.read(cx).intruder.results.views[&started.summary.id])
    });
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let (revision, after) = query(&mut fixture, started.summary.id);
    assert_eq!(after, None);
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Paused;
    page(&fixture, &started, revision, rows.clone(), None, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    reply(&fixture, &started, operation, Ok(changed), cx);
    let mut merged = started.summary.clone();
    merged.resource_pool = Some(pool);
    merged.resource_pool_revision = 1;
    cx.read(|cx| {
        let owner = fixture.view.read(cx);
        assert_eq!(
            owner.intruder.results.views[&started.summary.id]
                .summary
                .as_ref(),
            Some(&merged)
        );
        assert_eq!(owner.intruder.runs[&started.summary.id].summary, merged);
    });
    assert!(drain(&mut fixture.commands).is_empty());
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let (revision, _) = query(&mut fixture, started.summary.id);
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Running;
    page(&fixture, &started, revision, rows.clone(), None, cx);
    merged.revision = started.summary.revision;
    merged.state = IntruderRunState::Running;
    cx.read(|cx| {
        let owner = fixture.view.read(cx);
        let view = &owner.intruder.results.views[&started.summary.id];
        assert_eq!(view.summary.as_ref(), Some(&merged));
        assert_eq!(displayed_rows(view), rows);
        assert_eq!(owner.intruder.runs[&started.summary.id].summary, merged);
    });
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(!started.cancel.is_cancelled());
}

#[gpui_kit::test]
fn terminal_pool_malformed_ack_recovers_actual_binding_from_status_reply(cx: &mut TestAppContext) {
    let (mut fixture, mut started, catalog) = setup(cx, 64);
    let mut rows = cx.read(|cx| {
        displayed_rows(&fixture.view.read(cx).intruder.results.views[&started.summary.id])
    });
    rows.push(row(1, &flow(1)));
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let (revision, after) = query(&mut fixture, started.summary.id);
    assert_eq!(after, None);
    started.summary.revision += 1;
    started.summary.dispatched = started.summary.total;
    started.summary.completed = started.summary.total;
    started.summary.state = IntruderRunState::Completed;
    page(&fixture, &started, revision, rows.clone(), None, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    let workspace = cx.read(|cx| {
        let owner = fixture.view.read(cx);
        assert!(!owner.intruder.runs.contains_key(&started.summary.id));
        owner.workspace.clone()
    });

    let pool = catalog.pools[0].id;
    let (operation, cancel) = request(&mut fixture, &started, pool, cx);
    let actual = accepted(&started, pool);
    let mut malformed = actual.clone();
    malformed.spec_revision += 1;
    reply(&fixture, &started, operation, Ok(malformed), cx);
    assert!(cancel.is_cancelled());
    cx.read(|cx| {
        let view = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert_eq!(view.summary.as_ref(), Some(&started.summary));
        assert!(view.pool_error.as_ref().unwrap().contains("不一致"));
    });
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryIntruderResults {
            project,
            run_id,
            query_revision,
            after: None,
            limit: 1,
        },
    ] = commands.as_slice()
    else {
        panic!("a malformed matching ACK must query actual status without another binding or send")
    };
    assert_eq!((*project, *run_id), (fixture.project, started.summary.id));
    emit(
        &fixture,
        Event::IntruderResults {
            project: fixture.project,
            run_id: started.summary.id,
            query_revision: *query_revision,
            result: Ok(IntruderResultPage {
                run: actual.clone(),
                rows: vec![rows[0].clone()],
                next_after: Some(0),
                persisted: true,
            }),
        },
        cx,
    );
    cx.read(|cx| {
        let owner = fixture.view.read(cx);
        let view = &owner.intruder.results.views[&started.summary.id];
        assert_eq!(
            view.summary.as_ref(),
            Some(&actual),
            "the event-batch refresh must retain a terminal run's pending reconciliation probe"
        );
        assert_eq!(displayed_rows(view), rows);
        assert_eq!(owner.workspace, workspace);
        assert!(!owner.intruder.runs.contains_key(&started.summary.id));
    });
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(pool_radio(pool)).checked(), Some(true));
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn pool_revision_conflict_queries_actual_binding_before_explicit_retry(cx: &mut TestAppContext) {
    let (mut fixture, mut started, catalog) = setup(cx, 64);
    let pool = catalog.pools[0].id;
    let other = catalog.pools[1].id;
    let (workspace, rows) = cx.read(|cx| {
        let owner = fixture.view.read(cx);
        (
            owner.workspace.clone(),
            displayed_rows(&owner.intruder.results.views[&started.summary.id]),
        )
    });
    let (operation, cancel) = request(&mut fixture, &started, pool, cx);
    reply(
        &fixture,
        &started,
        operation,
        Err("资源池绑定 revision 已改变".into()),
        cx,
    );
    assert!(cancel.is_cancelled());
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder.results.views[&started.summary.id]
                .summary
                .as_ref(),
            Some(&started.summary)
        );
    });
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryIntruderResults {
            project,
            run_id,
            query_revision,
            after: None,
            limit: 1,
        },
    ] = commands.as_slice()
    else {
        panic!(
            "a binding revision conflict must query actual status, without another Set, Save or Start"
        )
    };
    assert_eq!((*project, *run_id), (fixture.project, started.summary.id));
    let mut actual = accepted(&started, pool);
    actual.resource_pool_revision = 3;
    emit(
        &fixture,
        Event::IntruderResults {
            project: fixture.project,
            run_id: started.summary.id,
            query_revision: *query_revision,
            result: Ok(IntruderResultPage {
                run: actual.clone(),
                rows: rows.clone(),
                next_after: None,
                persisted: true,
            }),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let owner = fixture.view.read(cx);
        let view = &owner.intruder.results.views[&started.summary.id];
        assert_eq!(view.summary.as_ref(), Some(&actual));
        assert_eq!(owner.intruder.runs[&started.summary.id].summary, actual);
        assert_eq!(displayed_rows(view), rows);
        assert_eq!(owner.workspace, workspace);
    });
    started.summary = actual;
    let (retry, retry_cancel) = request(&mut fixture, &started, other, cx);
    assert_ne!(retry, operation);
    let confirmed = accepted(&started, other);
    reply(&fixture, &started, retry, Ok(confirmed.clone()), cx);
    assert!(retry_cancel.is_cancelled());
    assert!(!started.cancel.is_cancelled());
    cx.read(|cx| {
        let owner = fixture.view.read(cx);
        let view = &owner.intruder.results.views[&started.summary.id];
        assert_eq!(view.summary.as_ref(), Some(&confirmed));
        assert_eq!(displayed_rows(view), rows);
        assert_eq!(owner.workspace, workspace);
        assert!(view.pool_error.is_none());
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn same_lifecycle_pool_ack_cannot_complete_an_active_run(cx: &mut TestAppContext) {
    let (mut fixture, started, catalog) = setup(cx, 64);
    let pool = catalog.pools[0].id;
    let rows = cx.read(|cx| {
        displayed_rows(&fixture.view.read(cx).intruder.results.views[&started.summary.id])
    });
    let (operation, cancel) = request(&mut fixture, &started, pool, cx);
    let actual = accepted(&started, pool);
    let mut malformed = actual.clone();
    malformed.state = IntruderRunState::Completed;
    reply(&fixture, &started, operation, Ok(malformed), cx);
    assert!(cancel.is_cancelled());
    assert!(!started.cancel.is_cancelled());
    cx.read(|cx| {
        let owner = fixture.view.read(cx);
        assert!(
            owner.intruder.runs.contains_key(&started.summary.id),
            "a binding ACK cannot remove the active run by changing state at the same lifecycle revision"
        );
        let active = &owner.intruder.runs[&started.summary.id];
        assert_eq!(active.summary, started.summary);
        assert!(!active.cancel.is_cancelled());
        let view = &owner.intruder.results.views[&started.summary.id];
        assert_eq!(view.summary.as_ref(), Some(&started.summary));
        assert_eq!(displayed_rows(view), rows);
        assert!(view.pool_error.as_ref().unwrap().contains("不一致"));
    });
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryIntruderResults {
            project,
            run_id,
            query_revision,
            after: None,
            limit: 1,
        },
    ] = commands.as_slice()
    else {
        panic!("a contradictory lifecycle ACK must query status without resending or stopping")
    };
    assert_eq!((*project, *run_id), (fixture.project, started.summary.id));
    emit(
        &fixture,
        Event::IntruderResults {
            project: fixture.project,
            run_id: started.summary.id,
            query_revision: *query_revision,
            result: Ok(IntruderResultPage {
                run: actual.clone(),
                rows: rows.clone(),
                next_after: None,
                persisted: true,
            }),
        },
        cx,
    );
    cx.read(|cx| {
        let owner = fixture.view.read(cx);
        let active = &owner.intruder.runs[&started.summary.id];
        assert_eq!(active.summary, actual);
        assert!(!active.cancel.is_cancelled());
        let view = &owner.intruder.results.views[&started.summary.id];
        assert_eq!(view.summary.as_ref(), Some(&actual));
        assert_eq!(displayed_rows(view), rows);
    });
    assert!(!started.cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
}
