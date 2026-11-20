use super::*;

#[gpui_kit::test]
fn created_project_survives_failed_current_save_then_opens_through_the_existing_barrier(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("旧项目草稿")], 64);
    let old_path = PathBuf::from("/tmp/mio-ui-ext-existing.mio");
    fixture
        .view
        .update(cx, |view, _| view.project_path = Some(old_path.clone()));
    let (job, _) = inspect(&mut fixture, cx);
    inspected(&fixture, job, cx);
    let (job_id, _, created) = convert(&mut fixture, cx);
    fixture
        .events
        .try_send(Event::ExtJob {
            project: fixture.project,
            job_id,
            outcome: ExtJobOutcome::Imported {
                path: created.clone(),
                report: report(),
                total_bytes: 120_000,
            },
        })
        .unwrap();
    cx.run_until_parked();
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::Quiesce { .. }]
    ));
    fixture
        .events
        .try_send(Event::Quiesced {
            project: fixture.project,
        })
        .unwrap();
    cx.run_until_parked();
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::SaveProject { path: None, .. }]
    ));
    fixture
        .events
        .try_send(Event::Error {
            project: Some(fixture.project),
            operation: "save_project",
            message: "旧项目磁盘不可写".into(),
        })
        .unwrap();
    cx.run_until_parked();
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::Resume { .. }]
    ));
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project_path, Some(old_path.clone()));
        assert_eq!(view.ext_import.created_path(), Some(&created));
        assert!(view.transition.is_none());
    });
    click(&fixture, "ext-open-created", cx);
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::Quiesce { .. }]
    ));
    fixture
        .events
        .try_send(Event::Quiesced {
            project: fixture.project,
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);
    fixture
        .events
        .try_send(Event::ProjectSaved {
            websocket_notes: vec![],
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: fixture.project,
            path: old_path,
            total_bytes: 4096,
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        matches!(drain(&mut fixture.commands).as_slice(), [Command::OpenProject { path }] if path == &created)
    );
    let mut next = fixture.project;
    next.generation += 1;
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: next,
            path: Some(created),
            workspace: Workspace::default(),
        })
        .unwrap();
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    assert!(commands.iter().any(
        |command| matches!(command, Command::QueryExtReport { project, .. } if *project == next)
    ));
    assert!(!commands.iter().any(|command| matches!(
        command,
        Command::StartProxy { .. } | Command::Send { .. } | Command::Plugin { .. }
    )));
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(next));
        assert!(view.ext_import.visible);
        assert!(view.ext_import.report_loading);
    });
}

#[gpui_kit::test]
fn published_failure_retains_target_even_after_a_cancel_request(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let (job, _) = inspect(&mut fixture, cx);
    inspected(&fixture, job, cx);
    let (job_id, cancel, created) = convert(&mut fixture, cx);
    click(&fixture, "ext-cancel", cx);
    assert!(cancel.is_cancelled());
    fixture
        .events
        .try_send(Event::ExtJob {
            project: fixture.project,
            job_id,
            outcome: ExtJobOutcome::Failed {
                message: "目录同步失败".into(),
                created_path: Some(created.clone()),
            },
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let import = &fixture.view.read(cx).ext_import;
        assert_eq!(import.created_path(), Some(&created));
        assert!(matches!(&import.result, Some(ExtJobOutcome::Failed { message, .. }) if message == "目录同步失败"));
    });
    click(&fixture, "ext-open-created", cx);
    assert!(
        matches!(drain(&mut fixture.commands).as_slice(), [Command::OpenProject { path }] if path == &created)
    );
}

#[gpui_kit::test]
fn committed_import_after_cancel_request_is_reported_as_created_not_cancelled(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    let (job, _) = inspect(&mut fixture, cx);
    inspected(&fixture, job, cx);
    let (job_id, cancel, created) = convert(&mut fixture, cx);
    click(&fixture, "ext-cancel", cx);
    assert!(cancel.is_cancelled());
    fixture
        .events
        .try_send(Event::ExtJob {
            project: fixture.project,
            job_id,
            outcome: ExtJobOutcome::Imported {
                path: created.clone(),
                report: report(),
                total_bytes: 120_000,
            },
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        matches!(drain(&mut fixture.commands).as_slice(), [Command::OpenProject { path }] if path == &created)
    );
    cx.read(|cx| {
        let import = &fixture.view.read(cx).ext_import;
        assert!(matches!(
            import.result,
            Some(ExtJobOutcome::Imported { .. })
        ));
        assert_eq!(import.created_path(), Some(&created));
    });
}
