use super::*;

#[gpui_kit::test]
fn destination_cancel_preserves_inspection_until_conversion_and_failure_retries_inspect(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![], 64);
    let (inspection_job, _) = inspect(&mut fixture, cx);
    let inspection_id = inspected(&fixture, inspection_job, cx);
    click(&fixture, "ext-convert", cx);
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    cx.read(|cx| {
        let import = &fixture.view.read(cx).ext_import;
        assert!(matches!(&import.result, Some(ExtJobOutcome::Inspected(info)) if info.id == inspection_id));
        assert!(import.job.is_none());
    });
    click(&fixture, "ext-convert", cx);
    let destination = std::env::current_dir()
        .unwrap_or_default()
        .join(".local/mio-ui-ext-created.mio");
    cx.simulate_new_path_selection(|_| Some(destination.clone()));
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::ConvertExternal {
            job_id,
            inspection_id: converted,
            destination: actual,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("one confirmed conversion command");
    };
    assert_eq!(*converted, inspection_id);
    assert_eq!(actual, &destination);
    cx.read(|cx| assert!(fixture.view.read(cx).ext_import.result.is_none()));
    fixture
        .events
        .try_send(Event::ExtJob {
            project: fixture.project,
            job_id: *job_id,
            outcome: ExtJobOutcome::Failed {
                message: "目标已存在；源未改变".into(),
                created_path: None,
            },
        })
        .unwrap();
    cx.run_until_parked();
    click(&fixture, "ext-inspect-retry", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::InspectExternal {
            job_id: retried, ..
        },
    ] = commands.as_slice()
    else {
        panic!("conversion failures must retry from a fresh inspection");
    };
    assert_ne!(*retried, inspection_job);
    assert_ne!(*retried, *job_id);
    fixture
        .events
        .try_send(Event::ExtJob {
            project: fixture.project,
            job_id: *job_id,
            outcome: ExtJobOutcome::Failed {
                message: "late failure".into(),
                created_path: None,
            },
        })
        .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).ext_import.job.as_ref().unwrap().id,
            *retried
        )
    });
}
