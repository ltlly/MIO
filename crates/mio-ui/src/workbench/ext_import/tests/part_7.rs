use super::*;

#[gpui_kit::test]
fn ext_picker_source_project_opened_keeps_busy_until_callback(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    drain(&mut fixture.commands);
    project_menu(&fixture, "导入 外部归档项目…", cx);
    assert!(cx.did_prompt_for_paths());
    let picker = cx.read(|cx| fixture.view.read(cx).ext_import.picker.unwrap());
    let mut next = fixture.project;
    next.generation += 1;
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: next,
            path: None,
            workspace: Workspace::default(),
        })
        .unwrap();
    cx.run_until_parked();
    let busy_before_callback = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(next));
        (
            view.ext_import.picker,
            view.ext_import.blocks_transition(),
        )
    });
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|_| {
        Some(vec![PathBuf::from("/tmp/mio-ui-ext-late-source.archive")])
    });
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_paths());
    assert!(
        matches!(drain(&mut fixture.commands).as_slice(),
            [Command::QueryHttpHistory { query: mio_core::HttpHistoryQuery { project, .. } }] if *project == next),
        "a stale picker must not inspect, convert, publish, run plugins or send traffic"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(next));
        let import = &view.ext_import;
        assert!(import.picker.is_none());
        assert!(!import.blocks_transition());
        assert!(import.source.is_none());
        assert!(import.created_path().is_none());
    });
    assert_eq!(
        busy_before_callback,
        (Some(picker), true),
        "ProjectOpened must retain the old source panel's busy token until its callback"
    );
}

#[gpui_kit::test]
fn ext_picker_destination_project_opened_keeps_busy_until_callback(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![], 64);
    let (inspection_job, _) = inspect(&mut fixture, cx);
    inspected(&fixture, inspection_job, cx);
    click(&fixture, "ext-convert", cx);
    assert!(cx.did_prompt_for_new_path());
    let picker = cx.read(|cx| fixture.view.read(cx).ext_import.picker.unwrap());
    let mut next = fixture.project;
    next.generation += 1;
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: next,
            path: None,
            workspace: Workspace::default(),
        })
        .unwrap();
    cx.run_until_parked();
    let busy_before_callback = cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(next));
        (
            view.ext_import.picker,
            view.ext_import.blocks_transition(),
        )
    });
    assert!(cx.did_prompt_for_new_path());
    let destination = std::env::temp_dir().join(format!("mio-ui-ext-picker-{picker}.mio"));
    cx.simulate_new_path_selection(|_| Some(destination.clone()));
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_new_path());
    assert!(
        matches!(drain(&mut fixture.commands).as_slice(),
            [Command::QueryHttpHistory { query: mio_core::HttpHistoryQuery { project, .. } }] if *project == next),
        "a stale picker must not inspect, convert, publish, run plugins or send traffic"
    );
    assert!(
        !destination.exists(),
        "a stale picker must not publish its target"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(next));
        let import = &view.ext_import;
        assert!(import.picker.is_none());
        assert!(!import.blocks_transition());
        assert!(import.source.is_none());
        assert!(import.created_path().is_none());
    });
    assert_eq!(
        busy_before_callback,
        (Some(picker), true),
        "ProjectOpened must retain the old destination panel's busy token until its callback"
    );
}

pub(in crate::workbench) fn open_ext_picker(
    fixture: &mut Fixture,
    destination: bool,
    cx: &mut TestAppContext,
) -> Uuid {
    if destination {
        let (job, _) = inspect(fixture, cx);
        inspected(fixture, job, cx);
        click(fixture, "ext-convert", cx);
        assert!(cx.did_prompt_for_new_path());
    } else {
        drain(&mut fixture.commands);
        project_menu(fixture, "导入 外部归档项目…", cx);
        assert!(cx.did_prompt_for_paths());
    }
    cx.read(|cx| fixture.view.read(cx).ext_import.picker.unwrap())
}

pub(in crate::workbench) fn answer_ext_picker(
    destination: bool,
    answer: Option<PathBuf>,
    cx: &mut TestAppContext,
) {
    if destination {
        cx.simulate_new_path_selection(|_| answer);
    } else {
        cx.simulate_path_prompt_response(|_| answer.map(|path| vec![path]));
    }
    cx.run_until_parked();
}

pub(in crate::workbench) fn close_ext_picker_receiver(destination: bool, cx: &mut TestAppContext) {
    struct PickerDisconnected;

    // The platform pops the sender before invoking the selection closure.
    let disconnected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if destination {
            cx.simulate_new_path_selection(|_| {
                std::panic::resume_unwind(Box::new(PickerDisconnected))
            });
        } else {
            cx.simulate_path_prompt_response(|_| {
                std::panic::resume_unwind(Box::new(PickerDisconnected))
            });
        }
    }))
    .expect_err("drop the picker sender to exercise the receiver-error callback");
    assert!(disconnected.is::<PickerDisconnected>());
    cx.run_until_parked();
}
