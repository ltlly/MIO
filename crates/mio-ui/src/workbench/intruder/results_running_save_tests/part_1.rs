use super::*;

#[gpui_kit::test]
fn running_session_explicit_save_waits_for_receipt_and_preserves_later_progress(
    cx: &mut TestAppContext,
) {
    let (mut fixture, mut started, rows) = setup(cx);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.press(crate::kbd!("s"), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        matches!(
            drain(&mut fixture.commands).as_slice(),
            [Command::SaveProject { path: None, .. }]
        ),
        "ordinary project save does not promote the running Session"
    );
    emit(
        &fixture,
        Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path: PathBuf::from("/tmp/mio-running-explicit-save.mio"),
            total_bytes: 128,
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        assert!(
            fixture
                .view
                .read(cx)
                .intruder
                .session_runs
                .contains(&started.summary.id)
        )
    });
    let (operation, receipt) = save(&mut fixture, &started, cx);
    retained(&fixture, Uuid::new_v4(), receipt.clone(), cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder.session_runs.contains(&started.summary.id));
        assert_eq!(
            displayed_rows(&view.intruder.results.views[&started.summary.id]),
            rows
        );
    });
    assert!(drain(&mut fixture.commands).is_empty());
    retained(&fixture, operation, receipt.clone(), cx);
    let commands = drain(&mut fixture.commands);
    assert_eq!(
        commands
            .iter()
            .filter(|command| matches!(command, Command::SaveProject { path: None, .. }))
            .count(),
        1
    );
    assert!(commands.iter().all(|command| {
        matches!(
            command,
            Command::SaveProject {
                path: None,
                workspace: None,
                ..
            } | Command::QueryHttpHistory { .. }
        ) || result_query_revision(command, &fixture, started.summary.id).is_some()
    }));
    let refresh = commands
        .iter()
        .find_map(|command| result_query_revision(command, &fixture, started.summary.id))
        .expect("promotion refreshes its result page");
    started.summary = receipt.run;
    started.summary.revision += 1;
    started.summary.completed = 2;
    let mut current_rows = rows;
    current_rows.push(row(1, &flow(1)));
    emit_page(
        &fixture,
        &started.summary,
        refresh,
        current_rows[..1].to_vec(),
        true,
        cx,
    );
    emit(
        &fixture,
        Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path: PathBuf::from("/tmp/mio-running-explicit-save.mio"),
            total_bytes: 256,
        },
        cx,
    );
    refresh_display(
        &mut fixture,
        &started.summary,
        current_rows.clone(),
        true,
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert!(!view.intruder.session_runs.contains(&started.summary.id));
        assert_eq!(
            view.intruder.runs[&started.summary.id].summary,
            started.summary
        );
        let result = &view.intruder.results.views[&started.summary.id];
        assert_eq!(displayed_rows(result), current_rows);
        assert_eq!(result.summary.as_ref(), Some(&started.summary));
        assert_eq!(
            view.workspace.intruder.tabs[0].spec,
            original.intruder.tabs[0].spec
        );
    });
    assert!(!started.cancel.is_cancelled());
    cx.update_window(started.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("intruder-result-save-to-project").checked(),
            Some(true)
        );
        assert_eq!(
            window.find("intruder-result-save-to-project").disabled(),
            Some(true)
        );
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn paused_session_save_cas_error_keeps_results_and_requires_explicit_retry(
    cx: &mut TestAppContext,
) {
    let (mut fixture, mut started, rows) = setup(cx);
    let pause = pause_command(&mut fixture, &started, true, cx);
    started.summary.revision += 1;
    started.summary.state = IntruderRunState::Paused;
    emit(&fixture, control_event(&fixture, &started, pause), cx);
    let (refresh, after) = query(&mut fixture, started.summary.id);
    assert_eq!(after, None);
    emit_page(&fixture, &started.summary, refresh, rows.clone(), false, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    let (operation, _) = save(&mut fixture, &started, cx);
    emit(
        &fixture,
        Event::IntruderRetentionChanged {
            project: fixture.project,
            run_id: started.summary.id,
            operation_id: operation,
            result: Err("Intruder run revision 已改变；数据保留".into()),
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert!(view.pending_workspace.is_none());
        assert!(view.intruder.session_runs.contains(&started.summary.id));
        assert_eq!(
            displayed_rows(&view.intruder.results.views[&started.summary.id]),
            rows
        );
        assert!(
            view.intruder.results.views[&started.summary.id]
                .close_error
                .as_ref()
                .unwrap()
                .contains("revision")
        );
    });
    assert!(!started.cancel.is_cancelled());
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "failed CAS is never retried automatically"
    );
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let (revision, _) = query(&mut fixture, started.summary.id);
    started.summary.revision += 1;
    emit_page(&fixture, &started.summary, revision, rows, false, cx);
    assert!(drain(&mut fixture.commands).is_empty());
    let (retry, _) = save(&mut fixture, &started, cx);
    assert_ne!(operation, retry);
    assert!(!started.cancel.is_cancelled());
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder.runs[&started.summary.id]
                .summary
                .state,
            IntruderRunState::Paused
        )
    });
}

#[gpui_kit::test]
fn running_keep_is_rejected_and_discard_still_waits_for_terminal(cx: &mut TestAppContext) {
    let (mut fixture, started, rows) = setup(cx);
    let target = cx.read(|cx| {
        let result = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        results_view::ResultTarget {
            workbench: fixture.view.downgrade(),
            project: fixture.project,
            view_id: started.summary.id,
            run_id: started.summary.id,
            owner: started.window,
            editor_id: result.request.raw.entity_id(),
        }
    });
    cx.update_window(started.window, |_, window, cx| {
        target.set_project_retention(false, window, cx)
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            view.intruder_result_close.is_none(),
            "Keep must not share the running Save exemption"
        );
        assert!(view.intruder.session_runs.contains(&started.summary.id));
        assert_eq!(
            displayed_rows(&view.intruder.results.views[&started.summary.id]),
            rows
        );
    });
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(!started.cancel.is_cancelled());
    cx.update_window(started.window, |_, window, cx| {
        window.press(crate::kbd!("w"), cx)
    })
    .unwrap();
    cx.run_until_parked();
    let dialog =
        close_tests::close_dialog(cx).expect("running close must still ask Continue or Discard");
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find_by_label("保留在内存").is_none());
        assert!(window.try_find_by_label("保存到项目文件").is_none());
    })
    .unwrap();
    answer_close_dialog("丢弃", cx);
    let commands = drain(&mut fixture.commands);
    assert!(
        matches!(commands.as_slice(), [Command::StopIntruder { project, run_id }]
        if *project == fixture.project && *run_id == started.summary.id),
        "Discard cancels the existing run before any terminal deletion or storage mutation"
    );
    assert!(started.cancel.is_cancelled());
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.pending_workspace.is_none());
        assert_eq!(
            view.intruder.results.views[&started.summary.id]
                .summary
                .as_ref(),
            Some(&started.summary)
        );
        assert_eq!(
            displayed_rows(&view.intruder.results.views[&started.summary.id]),
            rows
        );
    });
}

#[gpui_kit::test]
fn expired_running_save_receipt_merges_committed_retention_without_starting_more_work(
    cx: &mut TestAppContext,
) {
    let (mut fixture, started, rows) = setup(cx);
    let (operation, receipt) = save(&mut fixture, &started, cx);
    cx.dispatcher
        .scheduler()
        .clock()
        .advance(std::time::Duration::from_secs(31));
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            view.on_intruder_retention_changed(
                fixture.project,
                started.summary.id,
                operation,
                Ok(receipt),
                window,
                cx,
            );
            assert!(view.intruder_result_close.is_none());
            assert!(view.pending_workspace.is_none());
            assert!(!view.intruder.session_runs.contains(&started.summary.id));
            assert_eq!(
                displayed_rows(&view.intruder.results.views[&started.summary.id]),
                rows
            );
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());
    assert!(!started.cancel.is_cancelled());
    assert!(
        drain(&mut fixture.commands).iter().all(|command| {
            matches!(command, Command::QueryHttpHistory { .. })
                || result_query_revision(command, &fixture, started.summary.id).is_some()
        }),
        "late mutation receipt must not begin SaveProject, Stop, Keep, Discard, or retry"
    );
}

#[gpui_kit::test]
fn old_promotion_receipt_preserves_newer_unpersisted_project_progress(cx: &mut TestAppContext) {
    let (mut fixture, started, mut rows) = setup(cx);
    let previous_rows = rows.clone();
    let (operation, receipt) = save(&mut fixture, &started, cx);
    emit(
        &fixture,
        Event::IntruderChanged {
            project: fixture.project,
        },
        cx,
    );
    let (revision, _) = query(&mut fixture, started.summary.id);
    let mut newer = receipt.run.clone();
    newer.revision += 1;
    newer.completed = 2;
    rows.push(row(1, &flow(1)));
    emit_page(&fixture, &newer, revision, rows[..1].to_vec(), false, cx);
    cx.read(|cx| {
        let result = &fixture.view.read(cx).intruder.results.views[&started.summary.id];
        assert_eq!(result.summary.as_ref(), Some(&newer));
        assert_eq!(result.persisted, Some(false));
        assert_eq!(displayed_rows(result), previous_rows);
    });
    assert!(drain(&mut fixture.commands).is_empty());
    retained(&fixture, operation, receipt, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        let result = &view.intruder.results.views[&started.summary.id];
        assert_eq!(result.summary.as_ref(), Some(&newer));
        assert_eq!(displayed_rows(result), previous_rows);
        assert_eq!(
            result.persisted,
            Some(false),
            "an old promotion receipt cannot claim newer recovery rows are persisted"
        );
        assert_eq!(view.intruder.runs[&started.summary.id].summary, newer);
        assert!(view.intruder_result_close.is_some());
    });
    let commands = drain(&mut fixture.commands);
    assert!(
        commands.iter().all(|command| {
            matches!(command, Command::QueryHttpHistory { .. })
                || result_query_revision(command, &fixture, started.summary.id).is_some()
        }),
        "until a healthy page arrives, promotion cannot SaveProject, Stop, or replay work"
    );
    assert!(!started.cancel.is_cancelled());
    assert!(started.window.update(cx, |_, _, _| ()).is_ok());

    let refresh = if let Some(revision) = commands
        .iter()
        .find_map(|command| result_query_revision(command, &fixture, started.summary.id))
    {
        revision
    } else {
        emit(
            &fixture,
            Event::IntruderChanged {
                project: fixture.project,
            },
            cx,
        );
        query(&mut fixture, started.summary.id).0
    };
    newer.revision += 1;
    emit_page(&fixture, &newer, refresh, rows[..1].to_vec(), true, cx);
    let commands = drain(&mut fixture.commands);
    assert_eq!(
        commands
            .iter()
            .filter(|command| matches!(command, Command::SaveProject { path: None, .. }))
            .count(),
        1,
        "the healthy Project page allows the explicit save to continue"
    );
    assert!(
        commands
            .iter()
            .all(|command| matches!(command, Command::SaveProject { path: None, .. }))
    );
    emit(
        &fixture,
        Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path: PathBuf::from("/tmp/mio-running-explicit-save.mio"),
            total_bytes: 256,
        },
        cx,
    );
    refresh_display(&mut fixture, &newer, rows.clone(), true, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        let result = &view.intruder.results.views[&started.summary.id];
        assert_eq!(result.summary.as_ref(), Some(&newer));
        assert_eq!(displayed_rows(result), rows);
        assert_eq!(result.persisted, Some(true));
    });
    assert!(!started.cancel.is_cancelled());
    assert!(drain(&mut fixture.commands).is_empty());
}
