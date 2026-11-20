use super::*;

#[gpui_kit::test]
fn legacy_pool_sidebar_queries_without_binding_and_rows_select_explicitly(cx: &mut TestAppContext) {
    let (mut fixture, id, catalog) = setup(cx);
    let original = cx.read(|cx| {
        fixture.view.read(cx).workspace.intruder.tabs[0]
            .spec
            .clone()
    });
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("intruder-resource-pools-panel").visible());
        let payload = window.find("intruder-sidebar-payloads").bounds();
        let pool = window.find("intruder-sidebar-resource-pool").bounds();
        let settings = window.find("intruder-sidebar-settings").bounds();
        assert!(payload.origin.y < pool.origin.y && pool.origin.y < settings.origin.y);
        assert_eq!(window.find("intruder-pool-existing").checked(), Some(false));
        assert_eq!(window.find("intruder-pool-create").checked(), Some(false));
        assert!(!fixture.view.read(cx).intruder.editors[&id].payloads_open);
        assert!(!fixture.view.read(cx).intruder.editors[&id].settings_open);
        let selector = SharedString::from(format!(
            "intruder-pool-select-{}",
            catalog.default_pool_id.unwrap()
        ));
        assert_eq!(window.find(selector.clone()).checked(), Some(false));
        window.click(selector, cx);
    })
    .unwrap();
    cx.run_until_parked();
    saves(&mut fixture, cx);
    cx.read(|cx| {
        let spec = &fixture.view.read(cx).workspace.intruder.tabs[0].spec;
        assert_eq!(
            spec.resource_pool,
            Some(IntruderPoolSelection::Existing(
                catalog.default_pool_id.unwrap()
            ))
        );
        assert_eq!(spec.draft, original.draft);
        assert_eq!(spec.positions, original.positions);
    });
    click(&fixture, "intruder-sidebar-settings", cx);
    cx.read(|cx| assert!(!fixture.view.read(cx).intruder.editors[&id].resource_pool_open));
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn create_pool_form_saves_all_modes_and_preserves_values_across_existing_and_disabled_flags(
    cx: &mut TestAppContext,
) {
    let (mut fixture, id, catalog) = setup(cx);
    click(&fixture, "intruder-pool-create", cx);
    saves(&mut fixture, cx);
    text(&fixture, id, 0, "共享池🙂", cx);
    saves(&mut fixture, cx);
    click(&fixture, "intruder-pool-cap-enabled", cx);
    text(&fixture, id, 1, "2", cx);
    saves(&mut fixture, cx);
    click(&fixture, "intruder-pool-delay-enabled", cx);
    text(&fixture, id, 2, "100", cx);
    saves(&mut fixture, cx);
    assert_eq!(
        created(&fixture, id, cx).delay,
        Some(IntruderPoolDelay::Fixed { milliseconds: 100 })
    );
    click(&fixture, "intruder-pool-random", cx);
    saves(&mut fixture, cx);
    assert_eq!(
        created(&fixture, id, cx).delay,
        Some(IntruderPoolDelay::Random { milliseconds: 100 })
    );
    click(&fixture, "intruder-pool-increment", cx);
    text(&fixture, id, 3, "50", cx);
    saves(&mut fixture, cx);
    click(&fixture, "intruder-pool-auto", cx);
    click(&fixture, "intruder-pool-503", cx);
    click(&fixture, "intruder-pool-other-enabled", cx);
    text(&fixture, id, 4, " 504,505 ", cx);
    saves(&mut fixture, cx);
    let expected = created(&fixture, id, cx);
    assert_eq!(expected.name, "共享池🙂");
    assert_eq!(expected.max_concurrent_requests, Some(2));
    assert_eq!(
        expected.delay,
        Some(IntruderPoolDelay::Increment {
            initial_ms: 100,
            increment_ms: 50
        })
    );
    assert_eq!(
        expected.automatic_throttling.response_codes().unwrap(),
        [429, 503, 504, 505]
    );
    assert_eq!(expected.automatic_throttling.other_codes, " 504,505 ");
    for key in [
        "intruder-pool-cap-enabled",
        "intruder-pool-delay-enabled",
        "intruder-pool-auto",
    ] {
        click(&fixture, key, cx);
    }
    saves(&mut fixture, cx);
    let off = created(&fixture, id, cx);
    assert_eq!(off.max_concurrent_requests, None);
    assert_eq!(off.delay, None);
    assert!(!off.automatic_throttling.enabled);
    assert_eq!(
        off.automatic_throttling.other_codes,
        expected.automatic_throttling.other_codes
    );
    for key in [
        "intruder-pool-cap-enabled",
        "intruder-pool-delay-enabled",
        "intruder-pool-auto",
    ] {
        click(&fixture, key, cx);
    }
    saves(&mut fixture, cx);
    assert_eq!(created(&fixture, id, cx), expected);
    click(
        &fixture,
        SharedString::from(format!(
            "intruder-pool-select-{}",
            catalog.default_pool_id.unwrap()
        )),
        cx,
    );
    saves(&mut fixture, cx);
    cx.read(|cx| {
        let form = &fixture.view.read(cx).intruder.editors[&id].pool_form;
        assert_eq!(form.name.read(cx).value().as_ref(), "共享池🙂");
        assert_eq!(form.cap.read(cx).value().as_ref(), "2");
        assert_eq!(form.other.read(cx).value().as_ref(), " 504,505 ");
    });
    click(&fixture, "intruder-pool-create", cx);
    saves(&mut fixture, cx);
    assert_eq!(created(&fixture, id, cx), expected);
    assert_eq!(
        cx.read(|cx| fixture
            .view
            .read(cx)
            .intruder
            .resource_pools
            .catalog
            .clone()),
        Some(catalog)
    );
}

#[gpui_kit::test]
fn unfinished_create_cap_can_switch_existing_and_return_without_losing_input(
    cx: &mut TestAppContext,
) {
    let (mut fixture, id, catalog) = setup(cx);
    let default_id = catalog.default_pool_id.unwrap();
    click(
        &fixture,
        SharedString::from(format!("intruder-pool-select-{default_id}")),
        cx,
    );
    saves(&mut fixture, cx);
    click(&fixture, "intruder-pool-create", cx);
    text(&fixture, id, 0, "待填写并发", cx);
    saves(&mut fixture, cx);
    let planned = created(&fixture, id, cx);
    click(&fixture, "intruder-pool-cap-enabled", cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "an enabled but empty cap stays in the editor, not a fabricated saved value"
    );
    cx.read(|cx| {
        let form = &fixture.view.read(cx).intruder.editors[&id].pool_form;
        assert!(form.cap_enabled);
        assert!(form.cap.read(cx).value().is_empty());
        assert_eq!(form.id, planned.id);
    });
    click(&fixture, "intruder-pool-existing", cx);
    let commands = drain(&mut fixture.commands);
    let [Command::SaveWorkspace { project, workspace }] = commands.as_slice() else {
        panic!(
            "switching to the remembered Existing pool must save that selection despite unfinished local Create input"
        )
    };
    assert_eq!(*project, fixture.project);
    assert_eq!(
        workspace.intruder.tabs[0].spec.resource_pool,
        Some(IntruderPoolSelection::Existing(default_id))
    );
    emit(
        &fixture,
        Event::WorkspaceChanged {
            project: *project,
            workspace: workspace.clone(),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    let existing = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("intruder-pool-existing").checked(), Some(true));
        assert_eq!(window.find("intruder-pool-create").checked(), Some(false));
        let form = &fixture.view.read(cx).intruder.editors[&id].pool_form;
        assert_eq!(form.name.read(cx).value().as_ref(), "待填写并发");
        assert!(form.cap_enabled && form.cap.read(cx).value().is_empty());
        assert_eq!(form.id, planned.id);
    })
    .unwrap();
    click(&fixture, "intruder-pool-create", cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "returning to unfinished Create must not persist invented numbers"
    );
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("intruder-pool-create").checked(), Some(true));
        assert_eq!(window.find("intruder-pool-existing").checked(), Some(false));
        assert_eq!(
            window.find("intruder-pool-cap-enabled").checked(),
            Some(true)
        );
        let view = fixture.view.read(cx);
        assert_eq!(
            view.workspace, existing,
            "the last valid Existing snapshot remains until Create input is complete"
        );
        assert_eq!(view.intruder.editors[&id].pool_form.id, planned.id);
        assert!(
            view.intruder.editors[&id]
                .pool_form
                .cap
                .read(cx)
                .value()
                .is_empty()
        );
    })
    .unwrap();
    click(&fixture, "intruder-start", cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.press(crate::kbd!("s"), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "Start and Save must validate the active local Create form instead of using the older Existing snapshot"
    );
    assert!(!cx.did_prompt_for_new_path());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.intruder.starts.is_empty());
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("并发"))
        );
    });
    text(&fixture, id, 1, "2", cx);
    saves(&mut fixture, cx);
    let complete = created(&fixture, id, cx);
    assert_eq!(complete.id, planned.id);
    assert_eq!(complete.name, planned.name);
    assert_eq!(complete.max_concurrent_requests, Some(2));
    complete.validate().unwrap();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.pending_workspace.is_none() && !view.workspace_dirty);
        assert_eq!(view.intruder.resource_pools.catalog, Some(catalog));
    });
}

#[gpui_kit::test]
fn pool_preedit_blocks_start_and_selection_without_harvesting_any_input(cx: &mut TestAppContext) {
    let (mut fixture, id, catalog) = setup(cx);
    click(&fixture, "intruder-pool-create", cx);
    saves(&mut fixture, cx);
    let before = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    cx.update_window(fixture.window, |_, window, cx| {
        let input = fixture.view.read(cx).intruder.editors[&id]
            .pool_form
            .name
            .clone();
        window.click(("input", input.entity_id()), cx);
        input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
        });
        window.click("intruder-start", cx);
        window.click(
            SharedString::from(format!(
                "intruder-pool-select-{}",
                catalog.default_pool_id.unwrap()
            )),
            cx,
        );
        assert!(input.update(cx, |input, cx| {
            input.marked_text_range(window, cx).is_some()
        }));
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, before);
        assert!(view.intruder.starts.is_empty());
    });
}

#[gpui_kit::test]
fn pool_query_failure_timeout_and_old_callbacks_preserve_confirmed_catalog(
    cx: &mut TestAppContext,
) {
    let (mut fixture, _, catalog) = setup(cx);
    let before = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    click(&fixture, "intruder-sidebar-resource-pool", cx);
    click(&fixture, "intruder-sidebar-resource-pool", cx);
    let failed = query(&mut fixture);
    emit(
        &fixture,
        Event::IntruderResourcePools {
            project: fixture.project,
            query_revision: failed,
            result: Err("fixture read failure".into()),
        },
        cx,
    );
    cx.read(|cx| {
        assert!(
            fixture
                .view
                .read(cx)
                .intruder
                .resource_pools
                .error
                .as_ref()
                .unwrap()
                .contains("fixture read failure")
        )
    });
    click(&fixture, "intruder-sidebar-resource-pool", cx);
    click(&fixture, "intruder-sidebar-resource-pool", cx);
    let expired = query(&mut fixture);
    cx.dispatcher
        .scheduler()
        .clock()
        .advance(Duration::from_secs(31));
    cx.run_until_parked();
    reply(&fixture, expired, catalog.clone(), cx);
    cx.read(|cx| {
        let state = &fixture.view.read(cx).intruder.resource_pools;
        assert_eq!(state.catalog, Some(catalog.clone()));
        assert!(state.pending.is_none());
        assert!(state.error.as_ref().unwrap().contains("超时"));
    });
    click(&fixture, "intruder-sidebar-resource-pool", cx);
    click(&fixture, "intruder-sidebar-resource-pool", cx);
    let current = query(&mut fixture);
    assert!(current > expired);
    reply(&fixture, expired, IntruderResourcePools::default(), cx);
    reply(&fixture, current, catalog.clone(), cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, before);
        assert_eq!(view.intruder.resource_pools.catalog, Some(catalog));
        assert!(view.intruder.resource_pools.error.is_none());
    });
    assert!(drain(&mut fixture.commands).is_empty());
}
