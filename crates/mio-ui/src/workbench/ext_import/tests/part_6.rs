use super::*;

#[gpui_kit::test]
fn release_failure_is_visible_and_does_not_claim_cleanup(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let (job, _) = inspect(&mut fixture, cx);
    inspected(&fixture, job, cx);
    click(&fixture, "ext-dismiss", cx);
    let commands = drain(&mut fixture.commands);
    let [Command::ReleaseExtInspection { job_id, .. }] = commands.as_slice() else {
        panic!("closing preview must release its snapshot")
    };
    fixture
        .events
        .try_send(Event::ExtJob {
            project: fixture.project,
            job_id: *job_id,
            outcome: ExtJobOutcome::Failed {
                message: "临时副本清理失败".into(),
                created_path: None,
            },
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let import = &fixture.view.read(cx).ext_import;
        assert!(import.visible);
        assert!(matches!(&import.result, Some(ExtJobOutcome::Failed { message, .. }) if message.contains("清理失败")));
    });
}

#[gpui_kit::test]
fn ext_picker_source_disconnect_keeps_busy_until_callback(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    project_menu(&fixture, "导入 外部归档项目…", cx);
    assert!(cx.did_prompt_for_paths());
    let picker = cx.read(|cx| fixture.view.read(cx).ext_import.picker.unwrap());
    let (replacement, _) = mpsc::channel(1);
    drop(std::mem::replace(&mut fixture.events, replacement));
    cx.run_until_parked();
    let busy_before_callback = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.backend_disconnected);
        (
            view.ext_import.picker,
            view.ext_import.blocks_transition(),
        )
    });
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|_| None);
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_paths());
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "closing a disconnected source picker must not enqueue work"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(fixture.project));
        let import = &view.ext_import;
        assert!(import.picker.is_none());
        assert!(!import.blocks_transition());
        assert!(import.source.is_none());
        assert!(import.created_path().is_none());
    });
    assert_eq!(
        busy_before_callback,
        (Some(picker), true),
        "backend disconnect must retain the source panel's busy token until its callback"
    );
}

#[gpui_kit::test]
fn ext_picker_destination_disconnect_keeps_busy_until_callback(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let (inspection_job, _) = inspect(&mut fixture, cx);
    inspected(&fixture, inspection_job, cx);
    click(&fixture, "ext-convert", cx);
    assert!(cx.did_prompt_for_new_path());
    let picker = cx.read(|cx| fixture.view.read(cx).ext_import.picker.unwrap());
    let (replacement, _) = mpsc::channel(1);
    drop(std::mem::replace(&mut fixture.events, replacement));
    cx.run_until_parked();
    let busy_before_callback = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.backend_disconnected);
        (
            view.ext_import.picker,
            view.ext_import.blocks_transition(),
        )
    });
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_new_path());
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "closing a disconnected destination picker must not enqueue work"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(fixture.project));
        let import = &view.ext_import;
        assert!(import.picker.is_none());
        assert!(!import.blocks_transition());
        assert!(import.created_path().is_none());
    });
    assert_eq!(
        busy_before_callback,
        (Some(picker), true),
        "backend disconnect must retain the destination panel's busy token until its callback"
    );
}
