use super::*;

pub(in crate::workbench) fn report() -> ExtImportReport {
    ExtImportReport {
        repeater_active_tab: None,
        schema_floor: 228,
        schema_revision: 228,
        source_bytes: 100_000,
        indexed_items: 2,
        indexed_messages: 3,
        collections: vec![
            ExtCollectionReport {
                path: "root.f1.f0".into(),
                logical_items: Some(1),
                indexed_items: 1,
                message_count: 2,
                state: "present".into(),
            },
            ExtCollectionReport {
                path: "root.f2.f1".into(),
                logical_items: Some(1),
                indexed_items: 1,
                message_count: 1,
                state: "present".into(),
            },
        ],
        gaps: vec!["未验证设置仅保存在原始档案，不作为已恢复设置".into()],
        complete_archive: true,
    }
}

pub(in crate::workbench) fn click(fixture: &Fixture, id: &'static str, cx: &mut TestAppContext) {
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
}

pub(in crate::workbench) fn inspected(
    fixture: &Fixture,
    job_id: Uuid,
    cx: &mut TestAppContext,
) -> Uuid {
    let id = Uuid::new_v4();
    fixture
        .events
        .try_send(Event::ExtJob {
            project: fixture.project,
            job_id,
            outcome: ExtJobOutcome::Inspected(ExtInspection {
                id,
                source_name: "synthetic.archive".into(),
                report: report(),
            }),
        })
        .unwrap();
    cx.run_until_parked();
    id
}

pub(in crate::workbench) fn inspect(
    fixture: &mut Fixture,
    cx: &mut TestAppContext,
) -> (Uuid, CancellationToken) {
    drain(&mut fixture.commands);
    project_menu(fixture, "导入 外部归档项目…", cx);
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files);
        assert!(!options.directories && !options.multiple);
        Some(vec![PathBuf::from("/tmp/mio-ui-ext-synthetic.archive")])
    });
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::InspectExternal {
            project,
            job_id,
            path,
            cancel,
        },
    ] = commands.as_slice()
    else {
        panic!("native picker must start one read-only inspection");
    };
    assert_eq!(*project, fixture.project);
    assert_eq!(path, &PathBuf::from("/tmp/mio-ui-ext-synthetic.archive"));
    (*job_id, cancel.clone())
}

pub(in crate::workbench) fn convert(
    fixture: &mut Fixture,
    cx: &mut TestAppContext,
) -> (Uuid, CancellationToken, PathBuf) {
    click(fixture, "ext-convert", cx);
    assert!(cx.did_prompt_for_new_path());
    let destination = std::env::current_dir()
        .unwrap_or_default()
        .join(".local/mio-ui-ext-created.mio");
    cx.simulate_new_path_selection(|_| Some(destination.clone()));
    cx.run_until_parked();
    let commands = drain(&mut fixture.commands);
    let [
        Command::ConvertExternal {
            project,
            job_id,
            destination: actual,
            cancel,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("confirmed destination must start one conversion");
    };
    assert_eq!(*project, fixture.project);
    assert_eq!(actual, &destination);
    (*job_id, cancel.clone(), destination)
}

#[gpui_kit::test]
fn source_picker_cancel_and_late_selection_do_not_replace_the_current_project(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("保留的工作区")], 64);
    drain(&mut fixture.commands);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    project_menu(&fixture, "导入 外部归档项目…", cx);
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|_| None);
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, original);
        assert!(!view.ext_import.blocks_transition());
    });

    project_menu(&fixture, "导入 外部归档项目…", cx);
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
    drain(&mut fixture.commands);
    cx.simulate_path_prompt_response(|_| Some(vec![PathBuf::from("/tmp/late-source.archive")]));
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(next));
        assert!(view.ext_import.source.is_none());
    });
}
