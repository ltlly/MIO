use super::*;

#[gpui_kit::test]
fn dashboard_task_delete_no_is_inert_and_yes_deletes_only_unselected_task_without_a_view(
    cx: &mut TestAppContext,
) {
    let (mut fixture, first, second) = dashboard_tasks(cx);
    let before = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let views = cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .intruder
            .results
            .views
            .keys()
            .copied()
            .collect::<HashSet<_>>()
    });
    dashboard_task_menu(&fixture, first.id, "删除", cx);
    assert!(cx.has_pending_prompt());
    assert!(drain(&mut fixture.commands).is_empty());
    cx.simulate_prompt_answer("否");
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, before);
        assert!(view.intruder_result_close.is_none());
        assert_eq!(
            view.intruder
                .results
                .views
                .keys()
                .copied()
                .collect::<HashSet<_>>(),
            views
        );
    });
    dashboard_task_menu(&fixture, first.id, "删除", cx);
    cx.simulate_prompt_answer("是");
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::DiscardIntruder {
            project,
            run_id,
            operation_id,
            expected_revision,
            workspace,
        },
    ] = commands.as_slice()
    else {
        panic!("terminal task deletion needs only one discard, never selection or Stop");
    };
    assert_eq!(
        (*project, *run_id, *expected_revision),
        (fixture.project, first.id, first.revision)
    );
    let discard = Discard {
        project: *project,
        run_id: *run_id,
        operation_id: *operation_id,
        expected_revision: *expected_revision,
        workspace: workspace.clone(),
    };
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.intruder
                .results
                .views
                .keys()
                .copied()
                .collect::<HashSet<_>>(),
            views
        );
        assert!(
            view.intruder
                .results
                .views
                .values()
                .all(|result| result.run_id == second.id)
        );
    });
    emit(&fixture, reply(&discard, Ok(receipt(&discard))), cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert!(!view.intruder.session_runs.contains(&first.id));
        assert!(view.intruder.session_runs.contains(&second.id));
        assert!(
            view.intruder
                .results
                .runs
                .rows
                .iter()
                .all(|run| run.id != first.id)
        );
        assert_eq!(
            view.intruder
                .results
                .views
                .keys()
                .copied()
                .collect::<HashSet<_>>(),
            views
        );
        assert_eq!(view.workspace.tabs, before.tabs);
    });
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
}

#[gpui_kit::test]
fn dashboard_task_save_is_explicit_keeps_other_selection_and_waits_for_project_file_ack(
    cx: &mut TestAppContext,
) {
    let (mut fixture, first, second) = dashboard_tasks(cx);
    let path = PathBuf::from("/tmp/mio-dashboard-save-fixture.mio");
    cx.update(|cx| {
        fixture
            .view
            .update(cx, |view, _| view.project_path = Some(path.clone()))
    });
    let views = cx.read(|cx| {
        fixture
            .view
            .read(cx)
            .intruder
            .results
            .views
            .keys()
            .copied()
            .collect::<HashSet<_>>()
    });
    dashboard_task_menu(&fixture, first.id, "保存到项目文件", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetIntruderRetention {
            project,
            run_id,
            operation_id,
            expected_revision,
            retention,
            workspace,
        },
    ] = commands.as_slice()
    else {
        panic!("saving an unselected Session must explicitly promote that task only");
    };
    assert_eq!(
        (*project, *run_id, *expected_revision, *retention),
        (
            fixture.project,
            first.id,
            first.revision,
            mio_core::IntruderRetention::Project
        )
    );
    let promoted = IntruderRunSummary {
        revision: first.revision + 1,
        retention: *retention,
        ..first.clone()
    };
    emit(
        &fixture,
        Event::IntruderRetentionChanged {
            project: *project,
            run_id: *run_id,
            operation_id: *operation_id,
            result: Ok(mio_core::IntruderRetentionReceipt {
                run: promoted,
                retention: *retention,
                workspace: *workspace.clone(),
                session_logical_bytes: 0,
            }),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).iter().any(|command| matches!(command, Command::SaveProject { project, path: None, .. } if *project == fixture.project)));
    cx.read(|cx| assert!(fixture.view.read(cx).intruder_result_close.is_some()));
    emit(
        &fixture,
        Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path,
            total_bytes: 128,
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert!(!view.intruder.session_runs.contains(&first.id));
        assert!(view.intruder.session_runs.contains(&second.id));
        assert_eq!(
            view.intruder
                .results
                .views
                .keys()
                .copied()
                .collect::<HashSet<_>>(),
            views
        );
        assert!(
            view.intruder
                .results
                .views
                .values()
                .all(|result| result.run_id == second.id)
        );
    });
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
}

#[gpui_kit::test]
fn dashboard_task_retention_accepts_newer_title_from_a_late_rename_ack(cx: &mut TestAppContext) {
    let (mut fixture, first, _) = dashboard_tasks(cx);
    let path = PathBuf::from("/tmp/mio-dashboard-name-retention.mio");
    cx.update(|cx| {
        fixture
            .view
            .update(cx, |view, _| view.project_path = Some(path.clone()))
    });
    let windows = cx.read(|cx| cx.windows());
    dashboard_task_menu(&fixture, first.id, "重命名任务", cx);
    let dialog = cx
        .read(|cx| {
            cx.windows()
                .into_iter()
                .find(|window| !windows.contains(window))
        })
        .unwrap();
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        window.press(crate::kbd!("a"), cx);
        window.input("迟到的新任务名", cx);
        window.click("intruder-task-name-confirm", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::RenameIntruder {
            project,
            run_id,
            operation_id,
            expected_title_revision,
            title,
        },
    ] = commands.as_slice()
    else {
        panic!("actual rename confirmation must submit exactly once");
    };
    assert_eq!(
        (*project, *run_id, *expected_title_revision, title.as_str()),
        (
            fixture.project,
            first.id,
            first.title_revision,
            "迟到的新任务名"
        )
    );
    let rename_operation = *operation_id;
    click(dialog, "intruder-task-name-cancel", cx);
    cx.dispatcher
        .scheduler()
        .clock()
        .advance(std::time::Duration::from_secs(31));
    cx.run_until_parked();
    assert!(dialog.update(cx, |_, _, _| ()).is_err());
    dashboard_task_menu(&fixture, first.id, "保存到项目文件", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetIntruderRetention {
            project,
            run_id,
            operation_id,
            expected_revision,
            retention,
            workspace,
        },
    ] = commands.as_slice()
    else {
        panic!("expired rename must not hide the explicit task-save action");
    };
    assert_eq!((*expected_revision, *run_id), (first.revision, first.id));
    let mut renamed = first.clone();
    renamed.title = "迟到的新任务名".into();
    renamed.title_revision += 1;
    emit(
        &fixture,
        Event::IntruderRenamed {
            project: fixture.project,
            run_id: first.id,
            operation_id: rename_operation,
            result: Ok(renamed.clone()),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    renamed.revision += 1;
    renamed.retention = *retention;
    emit(
        &fixture,
        Event::IntruderRetentionChanged {
            project: *project,
            run_id: *run_id,
            operation_id: *operation_id,
            result: Ok(mio_core::IntruderRetentionReceipt {
                run: renamed.clone(),
                retention: *retention,
                workspace: *workspace.clone(),
                session_logical_bytes: 0,
            }),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).iter().any(|command| matches!(command, Command::SaveProject { project, .. } if *project == fixture.project)), "valid lifecycle/workspace ACK must advance despite an independently newer title");
    emit(
        &fixture,
        Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path,
            total_bytes: 128,
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert!(view.pending_workspace.is_none());
        assert!(!view.intruder.session_runs.contains(&first.id));
        assert_eq!(
            view.intruder
                .results
                .runs
                .rows
                .iter()
                .find(|run| run.id == first.id)
                .unwrap(),
            &renamed
        );
    });
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
}

#[gpui_kit::test]
fn dashboard_task_retention_rejects_conflicting_or_older_title_without_losing_receipt(
    cx: &mut TestAppContext,
) {
    let (mut fixture, first, _) = dashboard_tasks(cx);
    cx.update(|cx| {
        fixture.view.update(cx, |view, _| {
            view.project_path = Some(PathBuf::from("/tmp/mio-dashboard-invalid-name.mio"))
        })
    });
    dashboard_task_menu(&fixture, first.id, "保存到项目文件", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetIntruderRetention {
            project,
            run_id,
            operation_id,
            retention,
            workspace,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("expected explicit retention command");
    };
    let correct = IntruderRunSummary {
        revision: first.revision + 1,
        retention: *retention,
        ..first.clone()
    };
    for (title, title_revision) in [
        ("same revision conflict", first.title_revision),
        ("older name", first.title_revision - 1),
    ] {
        let malformed = IntruderRunSummary {
            title: title.into(),
            title_revision,
            ..correct.clone()
        };
        emit(
            &fixture,
            Event::IntruderRetentionChanged {
                project: *project,
                run_id: *run_id,
                operation_id: *operation_id,
                result: Ok(mio_core::IntruderRetentionReceipt {
                    run: malformed,
                    retention: *retention,
                    workspace: *workspace.clone(),
                    session_logical_bytes: 0,
                }),
            },
            cx,
        );
        assert!(drain(&mut fixture.commands).is_empty());
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(view.pending_workspace, Some(workspace.revision));
            assert!(view.intruder_close_owns_workspace());
            assert!(view.intruder.session_runs.contains(&first.id));
            assert_eq!(
                view.intruder
                    .results
                    .runs
                    .rows
                    .iter()
                    .find(|run| run.id == first.id)
                    .unwrap(),
                &first
            );
        });
    }
    emit(
        &fixture,
        Event::IntruderRetentionChanged {
            project: *project,
            run_id: *run_id,
            operation_id: *operation_id,
            result: Ok(mio_core::IntruderRetentionReceipt {
                run: correct.clone(),
                retention: *retention,
                workspace: *workspace.clone(),
                session_logical_bytes: 0,
            }),
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder_result_close.is_none());
        assert!(view.pending_workspace.is_none());
        assert_eq!(
            view.intruder
                .results
                .runs
                .rows
                .iter()
                .find(|run| run.id == first.id)
                .unwrap(),
            &correct
        );
    });
    assert!(drain(&mut fixture.commands).iter().all(|command| matches!(
        command,
        Command::QueryHttpHistory { .. } | Command::QueryIntruderRuns { .. }
    )));
    assert!(fixture.window.update(cx, |_, _, _| ()).is_ok());
}
