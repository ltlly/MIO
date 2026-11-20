use super::*;

#[gpui_kit::test]
fn cancellation_bypasses_full_queue_and_late_inspection_waits_for_release_ack(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 1);
    let (job_id, cancel) = inspect(&mut fixture, cx);
    project_menu(&fixture, "查看 外部导入记录", cx);
    click(&fixture, "ext-cancel", cx);
    assert!(
        cancel.is_cancelled(),
        "cancellation cannot depend on the full command queue"
    );
    cx.read(|cx| {
        let import = &fixture.view.read(cx).ext_import;
        assert!(import.job.as_ref().unwrap().cancelling);
        assert!(
            import.result.is_none(),
            "requesting cancel is not its terminal acknowledgement"
        );
    });
    let inspection_id = inspected(&fixture, job_id, cx);
    cx.read(|cx| {
        let import = &fixture.view.read(cx).ext_import;
        assert!(import.pending_release.is_some());
        assert!(matches!(import.result, Some(ExtJobOutcome::Inspected(_))));
        assert!(import.error.as_ref().unwrap().contains("队列已满"));
    });
    assert!(matches!(
        drain(&mut fixture.commands).as_slice(),
        [Command::QueryExtReport { .. }]
    ));
    click(&fixture, "ext-release-retry", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::ReleaseExtInspection {
            job_id: release_id,
            inspection_id: released,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("late inspection must be released by a separate correlated job");
    };
    assert_ne!(*release_id, job_id);
    assert_eq!(*released, inspection_id);
    project_menu(&fixture, "新建项目", cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "project switch waits for actual cleanup"
    );
    fixture
        .events
        .try_send(Event::ExtJob {
            project: fixture.project,
            job_id: *release_id,
            outcome: ExtJobOutcome::Released {
                inspection_id: Uuid::new_v4(),
            },
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| assert!(fixture.view.read(cx).ext_import.job.is_some()));
    fixture
        .events
        .try_send(Event::ExtJob {
            project: fixture.project,
            job_id: *release_id,
            outcome: ExtJobOutcome::Released { inspection_id },
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let import = &fixture.view.read(cx).ext_import;
        assert!(import.job.is_none());
        assert!(!import.blocks_transition());
        assert!(matches!(import.result, Some(ExtJobOutcome::Cancelled)));
        assert!(
            import.error.is_none(),
            "matching terminal clears the obsolete mismatch"
        );
    });
}
