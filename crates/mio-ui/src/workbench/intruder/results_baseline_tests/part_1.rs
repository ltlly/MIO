use super::*;

pub(in crate::workbench::intruder) fn discard_pending_baseline(
    accepted: bool,
    cx: &mut TestAppContext,
) {
    let (mut fixture, mut started, rows) = setup(cx);
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let (revision, _) = query(&mut fixture, started.summary.id);
    started.summary.revision += 1;
    started.summary.retention = mio_core::IntruderRetention::Session;
    emit(
        &fixture,
        result_event(
            &fixture,
            &started.summary,
            revision,
            rows.clone(),
            None,
            false,
            cx,
        ),
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    let (operation, admission) = request(&mut fixture, &started, cx);
    cx.update_window(started.window, |_, window, cx| {
        window.press(crate::kbd!("w"), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert!(has_close_dialog(cx));
    answer_close_dialog("丢弃", cx);
    let commands = drain(&mut fixture.commands);
    assert!(
        admission.is_cancelled(),
        "Discard must revoke pending baseline admission even while the cached run is Completed"
    );
    assert!(
        matches!(commands.as_slice(), [Command::StopIntruder { project, run_id }]
        if *project == fixture.project && *run_id == started.summary.id),
        "Discard must first stop a possibly accepted baseline, never delete using the cached terminal revision"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            view.intruder_baseline_pending(started.summary.id),
            "retain operation identity until the baseline receipt resolves"
        );
        assert!(
            view.pending_workspace.is_none(),
            "no discard workspace may be submitted yet"
        );
        assert!(view.intruder.session_runs.contains(&started.summary.id));
        assert_eq!(
            cached_results(&view.intruder.results.views[&started.summary.id]),
            rows
        );
        assert_eq!(
            view.intruder.results.views[&started.summary.id]
                .summary
                .as_ref(),
            Some(&started.summary)
        );
    });
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let (old_revision, _) = query(&mut fixture, started.summary.id);
    emit(
        &fixture,
        Event::IntruderResults {
            project: fixture.project,
            run_id: started.summary.id,
            query_revision: old_revision,
            result: Ok(IntruderResultPage {
                run: started.summary.clone(),
                rows: rows.iter().take(1).cloned().collect(),
                next_after: (rows.len() > 1).then_some(rows[0].ordinal),
                persisted: false,
            }),
        },
        cx,
    );
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "an old Completed status cannot resend Stop or authorize Discard before the baseline ACK"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder.baseline_requests[&started.summary.id].stop_requested);
        assert!(view.pending_workspace.is_none());
        assert!(view.intruder.session_runs.contains(&started.summary.id));
    });
    // The callback must still be owned by the first admitted operation, not a replay.
    emit(
        &fixture,
        Event::IntruderBaselineRequested {
            project: fixture.project,
            run_id: started.summary.id,
            operation_id: Uuid::new_v4(),
            result: Err("unrelated operation".into()),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        assert!(
            fixture
                .view
                .read(cx)
                .intruder_baseline_pending(started.summary.id),
            "wrong ACK cannot retire {operation}"
        )
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
                Err("准入已取消；未发送".into())
            },
        },
        cx,
    );
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
        panic!("after the baseline receipt, Discard must read a new status before deleting")
    };
    assert_eq!((*project, *run_id), (fixture.project, started.summary.id));
    cx.read(|cx| {
        assert!(
            fixture
                .view
                .read(cx)
                .intruder_baseline_pending(started.summary.id)
        );
        assert!(fixture.view.read(cx).pending_workspace.is_none());
    });
    emit(
        &fixture,
        Event::IntruderResults {
            project: fixture.project,
            run_id: started.summary.id,
            query_revision: old_revision,
            result: Ok(IntruderResultPage {
                run: started.summary.clone(),
                rows: rows.iter().take(1).cloned().collect(),
                next_after: (rows.len() > 1).then_some(rows[0].ordinal),
                persisted: false,
            }),
        },
        cx,
    );
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "a late pre-ACK page cannot satisfy the new terminal barrier"
    );
    let terminal = if accepted {
        assert!(
            accepted_receipt.cancel.is_cancelled(),
            "the explicit Discard must stop the accepted active token"
        );
        let mut stopped = accepted_receipt.run.clone();
        stopped.revision += 1;
        stopped.state = IntruderRunState::Stopped;
        stopped
    } else {
        started.summary.clone()
    };
    emit(
        &fixture,
        Event::IntruderResults {
            project: fixture.project,
            run_id: started.summary.id,
            query_revision: *query_revision,
            result: Ok(IntruderResultPage {
                run: terminal.clone(),
                rows: rows.iter().take(1).cloned().collect(),
                next_after: (rows.len() > 1).then_some(rows[0].ordinal),
                persisted: false,
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
        panic!("only the post-baseline terminal status may authorize the one Discard CAS")
    };
    assert_eq!(
        (*project, *run_id, *expected_revision),
        (fixture.project, started.summary.id, terminal.revision)
    );
    let mut saved_workspace = *workspace.clone();
    for tab in saved_workspace
        .intruder
        .tabs
        .iter_mut()
        .chain(&mut saved_workspace.intruder.closed)
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
                workspace: saved_workspace,
                deleted_flows: 0,
                retained_flows: 0,
            }),
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.intruder_baseline_pending(started.summary.id));
        assert!(!view.intruder.session_runs.contains(&started.summary.id));
        assert!(view.intruder_result_close.is_none());
        assert!(view.pending_workspace.is_none());
    });
    assert!(started.window.update(cx, |_, _, _| ()).is_err());
    assert!(
        drain(&mut fixture.commands)
            .iter()
            .all(|command| matches!(command, Command::QueryHttpHistory { .. })),
        "discard completion never replays baseline or payload requests"
    );
}
