use super::*;

#[gpui_kit::test]
fn pool_queue_error_timeout_and_late_ack_preserve_actual_binding(cx: &mut TestAppContext) {
    let (mut fixture, started, catalog) = setup(cx, 4);
    let pool = catalog.pools[0].id;
    cx.read(|cx| {
        for query_revision in 0..4 {
            fixture
                .view
                .read(cx)
                .commands
                .try_send(Command::QueryIntruderResourcePools {
                    project: fixture.project,
                    query_revision,
                })
                .unwrap();
        }
    });
    select_pool(&started, pool, cx);
    let commands = drain(&mut fixture.commands);
    assert_eq!(commands.len(), 4);
    assert!(
        commands
            .iter()
            .all(|command| matches!(command, Command::QueryIntruderResourcePools { .. }))
    );
    cx.read(|cx| {
        let owner = fixture.view.read(cx);
        assert!(!owner.intruder_pool_change_pending(started.summary.id));
        let view = &owner.intruder.results.views[&started.summary.id];
        assert_eq!(view.summary.as_ref(), Some(&started.summary));
        assert!(view.pool_error.as_ref().unwrap().contains("未入队"));
    });
    let (operation, cancelled) = request(&mut fixture, &started, pool, cx);
    reply(
        &fixture,
        &started,
        operation,
        Err("binding commit failed".into()),
        cx,
    );
    assert!(cancelled.is_cancelled());
    cx.read(|cx| {
        let owner = fixture.view.read(cx);
        assert!(!owner.intruder_pool_change_pending(started.summary.id));
        let view = &owner.intruder.results.views[&started.summary.id];
        assert_eq!(view.summary.as_ref(), Some(&started.summary));
        assert!(
            view.pool_error
                .as_ref()
                .unwrap()
                .contains("binding commit failed")
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
        panic!("a failed binding must only query actual status before an explicit retry")
    };
    assert_eq!((*project, *run_id), (fixture.project, started.summary.id));
    let rows = cx.read(|cx| {
        displayed_rows(&fixture.view.read(cx).intruder.results.views[&started.summary.id])
    });
    page(&fixture, &started, *query_revision, rows.clone(), None, cx);
    cx.read(|cx| {
        let view = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert_eq!(view.summary.as_ref(), Some(&started.summary));
        assert_eq!(displayed_rows(view), rows);
    });
    assert!(!started.cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
    let (operation, cancel) = request(&mut fixture, &started, pool, cx);
    cx.background_executor
        .advance_clock(std::time::Duration::from_secs(31));
    cx.run_until_parked();
    assert!(cancel.is_cancelled());
    assert!(!started.cancel.is_cancelled());
    cx.read(|cx| {
        let owner = fixture.view.read(cx);
        assert!(owner.intruder_pool_change_pending(started.summary.id));
        let view = &owner.intruder.results.views[&started.summary.id];
        assert_eq!(view.summary.as_ref(), Some(&started.summary));
        assert!(view.pool_error.as_ref().unwrap().contains("超时"));
    });
    select_pool(&started, pool, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    let changed = accepted(&started, pool);
    reply(&fixture, &started, operation, Ok(changed.clone()), cx);
    cx.read(|cx| {
        let owner = fixture.view.read(cx);
        assert!(!owner.intruder_pool_change_pending(started.summary.id));
        let view = &owner.intruder.results.views[&started.summary.id];
        assert_eq!(view.summary.as_ref(), Some(&changed));
        assert!(view.pool_error.is_none());
        assert!(owner.status.contains("迟到"));
    });
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn project_change_disconnect_and_owner_drop_cancel_pending_pool_admission(cx: &mut TestAppContext) {
    for mode in 0..3 {
        let (mut fixture, started, catalog) = setup(cx, 64);
        let (operation, cancel) = request(&mut fixture, &started, catalog.pools[0].id, cx);
        let changed = accepted(&started, catalog.pools[0].id);
        if mode == 2 {
            let weak = fixture.view.downgrade();
            cx.update_window(fixture.window, |_, window, _| window.remove_window())
                .unwrap();
            drop(fixture);
            cx.run_until_parked();
            assert!(weak.upgrade().is_none());
        } else if mode == 1 {
            let (replacement, _receiver) = mpsc::channel(1);
            drop(std::mem::replace(&mut fixture.events, replacement));
            cx.run_until_parked();
            cx.read(|cx| {
                let owner = fixture.view.read(cx);
                assert!(owner.backend_disconnected);
                assert!(!owner.intruder_pool_change_pending(started.summary.id));
                let view = &owner.intruder.results.views[&started.summary.id];
                assert_eq!(view.summary.as_ref(), Some(&started.summary));
                assert!(view.pool_error.as_ref().unwrap().contains("断开"));
            });
            assert!(drain(&mut fixture.commands).is_empty());
        } else {
            let next = ProjectKey {
                generation: fixture.project.generation + 1,
                ..fixture.project
            };
            let workspace = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
            emit(
                &fixture,
                Event::ProjectOpened {
                    project: next,
                    path: None,
                    workspace,
                },
                cx,
            );
            assert!(
                drain(&mut fixture.commands)
                    .iter()
                    .all(|command| matches!(command, Command::QueryHttpHistory { .. }))
            );
            reply(&fixture, &started, operation, Ok(changed), cx);
            cx.read(|cx| {
                let owner = fixture.view.read(cx);
                assert_eq!(owner.project, Some(next));
                assert!(owner.intruder.pool_changes.is_empty());
                assert!(owner.intruder.results.views.is_empty());
            });
            assert!(drain(&mut fixture.commands).is_empty());
        }
        assert!(cancel.is_cancelled());
    }
}

#[gpui_kit::test]
fn pool_binding_baseline_and_close_admissions_exclude_conflicting_receipts(
    cx: &mut TestAppContext,
) {
    let (mut fixture, mut started, catalog) = setup(cx, 64);
    let pool = catalog.pools[0].id;
    let other = catalog.pools[1].id;
    let (operation, _) = request(&mut fixture, &started, pool, cx);
    click(started.window, "intruder-results-settings", cx);
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("intruder-result-baseline").disabled(),
            Some(true)
        );
        window.click("intruder-result-baseline", cx);
        window.press(crate::kbd!("w"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!has_close_dialog(cx));
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(!started.cancel.is_cancelled());
    cx.read(|cx| {
        let owner = fixture.view.read(cx);
        assert!(!owner.intruder_baseline_pending(started.summary.id));
        assert!(!owner.intruder_result_close_pending(started.summary.id));
    });
    let changed = accepted(&started, pool);
    reply(&fixture, &started, operation, Ok(changed.clone()), cx);
    started.summary = changed;
    assert!(drain(&mut fixture.commands).is_empty());
    click(started.window, "intruder-result-baseline", cx);
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
        panic!("baseline is admitted only after the binding ACK")
    };
    assert_eq!(
        (*project, *run_id, *expected_revision),
        (
            fixture.project,
            started.summary.id,
            started.summary.revision
        )
    );
    let baseline_operation = *operation_id;
    let baseline_cancel = cancel.clone();
    open_pool(&mut fixture, &started, &catalog, cx);
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(pool_radio(other)).disabled(), Some(true));
    })
    .unwrap();
    select_pool(&started, other, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    emit(
        &fixture,
        Event::IntruderBaselineRequested {
            project: fixture.project,
            run_id: started.summary.id,
            operation_id: baseline_operation,
            result: Err("baseline admission cancelled".into()),
        },
        cx,
    );
    assert!(baseline_cancel.is_cancelled());
    let (revision, _) = query(&mut fixture, started.summary.id);
    let rows = cx.read(|cx| {
        displayed_rows(&fixture.view.read(cx).intruder.results.views[&started.summary.id])
    });
    page(&fixture, &started, revision, rows, None, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(started.window, |_, window, cx| {
        window.press(crate::kbd!("w"), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert!(has_close_dialog(cx));
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(pool_radio(other)).disabled(), Some(true));
    })
    .unwrap();
    select_pool(&started, other, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    answer_close_dialog("取消", cx);
    assert!(!started.cancel.is_cancelled());
    let (operation, _) = request(&mut fixture, &started, other, cx);
    reply(
        &fixture,
        &started,
        operation,
        Ok(accepted(&started, other)),
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
}
