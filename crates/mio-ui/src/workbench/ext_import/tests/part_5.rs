use super::*;

#[gpui_kit::test]
fn inspection_queue_failure_leaves_a_real_retry_and_no_phantom_job(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 1);
    project_menu(&fixture, "导入 外部归档项目…", cx);
    cx.simulate_path_prompt_response(|_| Some(vec![PathBuf::from("/tmp/mio-ui-ext-source.archive")]));
    cx.run_until_parked();
    cx.read(|cx| {
        let import = &fixture.view.read(cx).ext_import;
        assert!(import.job.is_none());
        assert!(import.error.as_ref().unwrap().contains("队列已满"));
        assert!(!import.blocks_transition());
    });
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryHttpHistory { .. }]
    ));
    click(&fixture, "ext-inspect-retry", cx);
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::InspectExternal { .. }]
    ));
}

#[gpui_kit::test]
fn disconnected_backend_cancels_the_owned_job_and_keeps_an_unconfirmed_target_hint(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    let (job, _) = inspect(&mut fixture, cx);
    inspected(&fixture, job, cx);
    let (_, cancel, destination) = convert(&mut fixture, cx);
    let (replacement, _) = mpsc::channel(1);
    drop(std::mem::replace(&mut fixture.events, replacement));
    cx.run_until_parked();
    assert!(
        cancel.is_cancelled(),
        "service loss must cancel the UI-owned token"
    );
    cx.read(|cx| {
        let import = &fixture.view.read(cx).ext_import;
        assert!(
            !import.blocks_transition(),
            "a dead service must not trap the window forever"
        );
        assert!(
            matches!(&import.result, Some(ExtJobOutcome::Failed { message, created_path: None })
            if message.contains("未确认") && message.contains(&destination.display().to_string()))
        );
    });
}

#[gpui_kit::test]
fn report_queries_reject_old_errors_and_project_generations_and_show_unknown_counts(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    project_menu(&fixture, "查看 外部导入记录", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryExtReport {
            query_revision: first,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("report query")
    };
    fixture
        .events
        .try_send(Event::ExtReport {
            project: fixture.project,
            query_revision: *first,
            result: Err("read failure".into()),
        })
        .unwrap();
    cx.run_until_parked();
    click(&fixture, "ext-report-refresh", cx);
    let next_commands = drain(&mut fixture.commands);
    let [
        Command::QueryExtReport {
            query_revision: second,
            ..
        },
    ] = next_commands.as_slice()
    else {
        panic!("report retry")
    };
    assert!(*second > *first);
    fixture
        .events
        .try_send(Event::ExtReport {
            project: fixture.project,
            query_revision: *first,
            result: Err("obsolete".into()),
        })
        .unwrap();
    let mut source = ExtSourceInfo {
        id: Uuid::new_v4(),
        report: report(),
    };
    source.report.collections[0].logical_items = None;
    source.report.collections[0].state = "unavailable".into();
    fixture
        .events
        .try_send(Event::ExtReport {
            project: fixture.project,
            query_revision: *second,
            result: Ok(Some(source.clone())),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "report alone must not read message bodies"
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .try_find_by_label("root.f1.f0 · unavailable · 逻辑位置 未知 · 已索引 1 · 消息 2")
                .is_some()
        );
        assert_eq!(
            fixture.view.read(cx).ext_import.report.as_ref(),
            Some(&source)
        );
        assert!(fixture.view.read(cx).ext_import.report_error.is_none());
    })
    .unwrap();
    let old_project = fixture.project;
    fixture.project.generation += 1;
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: fixture.project,
            path: None,
            workspace: Workspace::default(),
        })
        .unwrap();
    fixture
        .events
        .try_send(Event::ExtReport {
            project: old_project,
            query_revision: 0,
            result: Ok(Some(source)),
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let import = &fixture.view.read(cx).ext_import;
        assert!(import.report.is_none());
        assert!(!import.visible);
    });
}
