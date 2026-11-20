use super::*;

#[gpui_kit::test]
fn create_start_keeps_frozen_spec_then_selects_created_pool_and_refreshes_catalog(
    cx: &mut TestAppContext,
) {
    let (mut fixture, id, mut catalog) = setup(cx);
    click(&fixture, "intruder-pool-create", cx);
    saves(&mut fixture, cx);
    text(&fixture, id, 0, "Start 创建池", cx);
    saves(&mut fixture, cx);
    let pool = created(&fixture, id, cx);
    click(&fixture, "intruder-start", cx);
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { project, workspace }] = commands.as_slice() else {
        panic!("Start must first save the full Create specification")
    };
    emit(
        &fixture,
        Event::WorkspaceChanged {
            project: *project,
            workspace: workspace.clone(),
        },
        cx,
    );
    let commands = drain(&mut fixture.commands);
    let [
        Command::StartIntruder {
            run_id,
            tab_id,
            spec,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("one Start after its save ACK")
    };
    let frozen = *spec.clone();
    assert_eq!(
        frozen.resource_pool,
        Some(IntruderPoolSelection::Create(pool.clone()))
    );
    let run = IntruderRunSummary {
        id: *run_id,
        tab_id: *tab_id,
        revision: 1,
        spec_revision: frozen.revision,
        total: frozen.validate().unwrap(),
        dispatched: 0,
        completed: 0,
        state: mio_core::IntruderRunState::Running,
        started_unix_ms: 1,
        retention: mio_core::IntruderRetention::Project,
        title: String::new(),
        title_revision: 0,
        baseline: None,
        resource_pool: Some(pool.id),
        resource_pool_revision: 1,
    };
    emit(
        &fixture,
        Event::IntruderStarted {
            project: fixture.project,
            run_id: run.id,
            tab_id: id,
            spec_revision: frozen.revision,
            result: Ok(run.clone()),
        },
        cx,
    );
    let commands = drain(&mut fixture.commands);
    let mut pool_query = None;
    let mut spec_read = None;
    for command in commands {
        match command {
            Command::SaveWorkspace { project, workspace } => {
                emit(&fixture, Event::WorkspaceChanged { project, workspace }, cx)
            }
            Command::QueryIntruderResourcePools { query_revision, .. } => {
                assert!(pool_query.replace(query_revision).is_none());
            }
            Command::ReadIntruderSpec {
                selection_revision, ..
            } => spec_read = Some(selection_revision),
            Command::QueryIntruderResults { .. } => {}
            _ => panic!("the ACK cannot create or start the pool twice"),
        }
    }
    catalog.revision += 1;
    catalog.pools.push(pool.clone());
    reply(
        &fixture,
        pool_query.expect("a successful Create refreshes the pool catalog"),
        catalog,
        cx,
    );
    if spec_read.is_none() {
        let owner = cx.read(|cx| fixture.view.read(cx).intruder.results.views[&run.id].owner);
        cx.update_window(owner, |_, window, cx| {
            window.click("intruder-results-positions", cx)
        })
        .unwrap();
        cx.run_until_parked();
        let commands = drain(&mut fixture.commands);
        let [
            Command::ReadIntruderSpec {
                selection_revision, ..
            },
        ] = commands.as_slice()
        else {
            panic!("Positions reads the frozen Start specification")
        };
        spec_read = Some(*selection_revision);
    }
    emit(
        &fixture,
        Event::IntruderSpec {
            project: fixture.project,
            run_id: run.id,
            selection_revision: spec_read.unwrap(),
            result: Ok(Box::new(frozen.clone())),
        },
        cx,
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(
            view.workspace.intruder.tabs[0].spec.resource_pool,
            Some(IntruderPoolSelection::Existing(pool.id))
        );
        assert_eq!(
            view.workspace.intruder.tabs[0].spec.revision,
            frozen.revision + 1
        );
        assert_eq!(
            view.intruder.runs[&run.id].summary.spec_revision,
            frozen.revision
        );
        assert_eq!(
            view.intruder.results.views[&run.id].spec.as_deref(),
            Some(&frozen)
        );
        assert_eq!(
            view.intruder
                .resource_pools
                .catalog
                .as_ref()
                .unwrap()
                .find(pool.id),
            Some(&pool)
        );
        assert_eq!(
            view.intruder.editors[&id]
                .pool_form
                .name
                .read(cx)
                .value()
                .as_ref(),
            pool.name
        );
    });
    assert!(
        drain(&mut fixture.commands).iter().all(|command| matches!(
            command,
            Command::QueryIntruderResults { .. } | Command::ReadIntruderSpec { .. }
        )),
        "no replay after catalog/spec confirmation"
    );
}

#[gpui_kit::test]
fn create_start_ack_keeps_saved_request_edit_and_reuses_created_pool(cx: &mut TestAppContext) {
    create_start_ack_with_new_input(StartEdit::Request, cx);
}

#[gpui_kit::test]
fn create_start_ack_keeps_later_pool_name_on_a_new_create_id(cx: &mut TestAppContext) {
    create_start_ack_with_new_input(StartEdit::PoolName, cx);
}

#[gpui_kit::test]
fn create_start_ack_keeps_invalid_cap_editable_until_corrected(cx: &mut TestAppContext) {
    create_start_ack_with_new_input(StartEdit::InvalidCap, cx);
}

#[gpui_kit::test]
fn create_start_ack_preserves_name_composition_until_explicit_commit(cx: &mut TestAppContext) {
    create_start_ack_with_new_input(StartEdit::NameComposition, cx);
}

#[gpui_kit::test]
fn pool_blue_controls_keep_keyboard_focus_selection_and_checkbox_changes(cx: &mut TestAppContext) {
    let (mut fixture, id, _) = setup(cx);
    click(&fixture, "intruder-pool-existing", cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("intruder-pool-existing").focused(), Some(true));
        window.press("tab", cx);
        assert_eq!(window.find("intruder-pool-create").focused(), Some(true));
        assert_eq!(
            window.find("intruder-pool-create").role(),
            Some(gpui_kit::accesskit::Role::RadioButton)
        );
        window.press("space", cx);
    })
    .unwrap();
    cx.run_until_parked();
    saves(&mut fixture, cx);
    let selected = created(&fixture, id, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("intruder-pool-create").checked(), Some(true));
        assert_eq!(window.find("intruder-pool-create").selected(), Some(true));
        assert_eq!(window.find("intruder-pool-existing").checked(), Some(false));
        window.press("space", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "Space on the selected radio must not repeat its operation"
    );
    assert_eq!(created(&fixture, id, cx), selected);
    cx.update_window(fixture.window, |_, window, cx| {
        window.press("tab", cx);
        window.press("tab", cx);
        assert_eq!(
            window.find("intruder-pool-cap-enabled").focused(),
            Some(true)
        );
        window.press("space", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.press("tab", cx);
        window.input("2", cx);
    })
    .unwrap();
    cx.run_until_parked();
    saves(&mut fixture, cx);
    assert_eq!(created(&fixture, id, cx).max_concurrent_requests, Some(2));
}

#[gpui_kit::test]
fn untouched_legacy_pool_snapshot_keeps_absent_retained_inputs(cx: &mut TestAppContext) {
    let mut config = attack();
    let mut pool = default_pool_catalog().pools[0].clone();
    pool.delay = Some(IntruderPoolDelay::Random { milliseconds: 120 });
    assert!(pool.retained_inputs.is_none());
    config.spec.resource_pool = Some(IntruderPoolSelection::Create(pool));
    let id = config.id;
    let expected = config.spec.clone();
    let mut fixture = intruder_fixture(cx, vec![config], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        fixture.view.update(cx, |view, cx| {
            assert_eq!(
                view.snapshot_intruder_spec(id, window, cx).unwrap(),
                expected
            );
            assert!(!view.capture_drafts(window, cx).unwrap());
            assert_eq!(view.workspace.intruder.tabs[0].spec, expected);
        });
    })
    .unwrap();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "reading legacy input must not write migration data or start a request"
    );
}
