use super::*;

fn discard_detached_baseline(accepted: bool, cx: &mut TestAppContext) {
    let (mut fixture, started, rows) = setup(cx);
    // Supply the already-saved project identity without opening a real file.
    cx.update(|cx| {
        fixture.view.update(cx, |view, _| {
            view.project_path = Some(PathBuf::from("/tmp/mio-baseline-detach-discard.mio"));
        });
    });
    let before = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let (operation, admission) = request(&mut fixture, &started, cx);
    cx.update_window(started.window, |_, window, cx| {
        window.press(crate::kbd!("w"), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    assert!(!has_close_dialog(cx) && !cx.has_pending_prompt());
    assert!(
        !admission.is_cancelled(),
        "safe detach must retain the admitted operation"
    );
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, before);
        assert!(view.intruder.results.views.is_empty());
        assert!(!view.intruder.runs.contains_key(&started.summary.id));
        assert!(view.intruder_baseline_pending(started.summary.id));
    });
    click(fixture.window, "tool-dashboard", cx);
    let commands = drain(&mut fixture.commands);
    let [Command::QueryIntruderRuns { query_revision, .. }] = commands.as_slice() else {
        panic!("Dashboard must read the detached saved task")
    };
    emit(
        &fixture,
        Event::IntruderRuns {
            project: fixture.project,
            query_revision: *query_revision,
            result: Ok(IntruderRunPage {
                rows: vec![started.summary.clone()],
                next_after: None,
            }),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(
            SharedString::from(format!("dashboard-intruder-options-{}", started.summary.id)),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        let delete = window.find_by_label("删除").bounds().center();
        window.pointer_down(delete, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(delete, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.simulate_prompt_answer("是");
    cx.run_until_parked();
    assert!(admission.is_cancelled());
    assert!(
        matches!(drain(&mut fixture.commands).as_slice(),
            [Command::StopIntruder { project, run_id }] if *project == fixture.project && *run_id == started.summary.id
        ),
        "Delete must send exactly one Stop for the detached baseline, and must not discard before its ACK"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder.baseline_requests[&started.summary.id].stop_requested);
        assert!(view.intruder_result_close_pending(started.summary.id));
        assert!(view.pending_workspace.is_none());
        assert_eq!(view.workspace, before);
        assert_eq!(
            view.intruder.results.runs.rows,
            vec![started.summary.clone()]
        );
    });
    let accepted_receipt = receipt(&started);
    emit(
        &fixture,
        Event::IntruderBaselineRequested {
            project: fixture.project,
            run_id: started.summary.id,
            operation_id: operation,
            result: if accepted {
                Ok(accepted_receipt.clone())
            } else {
                Err("准入取消已确认；未发送".into())
            },
        },
        cx,
    );
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryIntruderResults {
            project,
            run_id,
            query_revision: terminal_revision,
            limit: 1,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("the baseline ACK must schedule one new terminal probe without a result window")
    };
    assert_eq!((*project, *run_id), (fixture.project, started.summary.id));
    let terminal = if accepted {
        assert!(
            accepted_receipt.cancel.is_cancelled(),
            "explicit Dashboard Delete must cancel the newly acknowledged active token"
        );
        let mut terminal = accepted_receipt.run.clone();
        terminal.revision += 1;
        terminal.state = IntruderRunState::Stopped;
        terminal
    } else {
        started.summary.clone()
    };
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let refresh = drain(&mut fixture.commands);
    let [Command::QueryIntruderRuns { query_revision, .. }] = refresh.as_slice() else {
        panic!("a Dashboard refresh must preserve the in-flight terminal probe")
    };
    emit(
        &fixture,
        Event::IntruderRuns {
            project: fixture.project,
            query_revision: *query_revision,
            result: Ok(IntruderRunPage {
                rows: vec![started.summary.clone()],
                next_after: None,
            }),
        },
        cx,
    );
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "the task-list Completed row is not the post-ACK terminal probe"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder.baseline_requests[&started.summary.id].stop_requested);
        assert!(view.intruder_result_close_pending(started.summary.id));
        assert!(view.pending_workspace.is_none());
        assert_eq!(view.workspace, before);
    });
    emit(
        &fixture,
        Event::IntruderResults {
            project: fixture.project,
            run_id: started.summary.id,
            query_revision: *terminal_revision,
            result: Ok(IntruderResultPage {
                run: terminal.clone(),
                rows: rows.iter().take(1).cloned().collect(),
                next_after: (rows.len() > 1).then_some(rows[0].ordinal),
                persisted: true,
            }),
        },
        cx,
    );
    let commands = drain(&mut fixture.commands);
    let [
        Command::DiscardIntruder {
            project,
            run_id,
            operation_id: discard_operation,
            expected_revision,
            workspace,
        },
    ] = commands.as_slice()
    else {
        panic!(
            "refresh must not erase the no-window probe: its correlated terminal reply must authorize exactly one Discard"
        )
    };
    assert_eq!(
        (*project, *run_id, *expected_revision),
        (fixture.project, started.summary.id, terminal.revision)
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.intruder_baseline_pending(started.summary.id));
        assert!(view.intruder_result_close_pending(started.summary.id));
        assert_eq!(view.intruder.results.runs.rows, vec![terminal.clone()]);
    });
    let mut discarded_workspace = *workspace.clone();
    for tab in discarded_workspace
        .intruder
        .tabs
        .iter_mut()
        .chain(&mut discarded_workspace.intruder.closed)
    {
        if tab.last_run == Some(started.summary.id) {
            tab.last_run = None;
        }
    }
    emit(
        &fixture,
        Event::IntruderDiscarded {
            project: fixture.project,
            run_id: started.summary.id,
            operation_id: *discard_operation,
            result: Ok(mio_core::IntruderDiscardReceipt {
                revision: terminal.revision + 1,
                workspace: discarded_workspace,
                deleted_flows: 0,
                retained_flows: 0,
            }),
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert!(view.pending_workspace.is_none());
        assert!(!view.intruder_baseline_pending(started.summary.id));
        assert!(
            view.intruder
                .results
                .runs
                .rows
                .iter()
                .all(|run| run.id != started.summary.id)
        );
        assert!(
            view.workspace
                .intruder
                .tabs
                .iter()
                .all(|tab| tab.last_run != Some(started.summary.id))
        );
    });
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
    assert!(
        drain(&mut fixture.commands).iter().all(|command| matches!(
            command,
            Command::QueryIntruderRuns { .. } | Command::QueryHttpHistory { .. }
        )),
        "confirmed deletion never replays any request"
    );
}

#[gpui_kit::test]
fn detached_completed_baseline_discard_keeps_terminal_probe_until_correlated_reply(
    cx: &mut TestAppContext,
) {
    discard_detached_baseline(false, cx);
}

#[gpui_kit::test]
fn detached_completed_baseline_discard_stops_accepted_run_before_correlated_delete(
    cx: &mut TestAppContext,
) {
    discard_detached_baseline(true, cx);
}
