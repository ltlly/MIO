use mio_core::IntruderPoolDeleteReceipt;

use super::*;

#[gpui_kit::test]
fn tasks_gear_reuses_settings_window_and_new_cancel_has_no_mutation(cx: &mut TestAppContext) {
    let (mut fixture, settings, catalog) = setup(cx, false);
    click(settings, "settings-intruder", cx);
    cx.update_window(settings, |_, window, cx| {
        window.press("tab", cx);
        assert_eq!(window.find("settings-repeater").focused(), Some(true));
        window.press("tab", cx);
        assert_eq!(window.find("settings-project-tasks").focused(), Some(true));
        window.press("enter", cx);
        window.render_frame(cx);
        assert_eq!(window.find("settings-project-tasks").selected(), Some(true));
        assert!(window.find("settings-resource-pools").visible());
    })
    .unwrap();
    cx.run_until_parked();
    answer_reads(&mut fixture, &catalog, false, cx);
    click(settings, "settings-intruder", cx);
    click(fixture.window, "dashboard-task-settings", cx);
    assert_eq!(
        cx.read(|cx| fixture.view.read(cx).settings_window),
        Some(settings)
    );
    answer_reads(&mut fixture, &catalog, false, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("settings-project-tasks").selected(), Some(true));
        assert!(window.find("settings-resource-pools").visible());
        assert_eq!(window.find("settings-pool-edit").disabled(), Some(true));
        assert_eq!(window.find("settings-pool-delete").disabled(), Some(true));
    })
    .unwrap();
    click(settings, "settings-pool-new", cx);
    let dialog = dialog(cx);
    let content = editor(dialog, cx);
    cx.read(|cx| {
        let form = &content.read(cx).form;
        assert!(form.name.read(cx).value().is_empty());
        assert!(form.cap_enabled && !form.delay_enabled && form.automatic && form.status_429);
        assert!(!form.status_503 && !form.other_enabled);
        assert_eq!(form.cap.read(cx).value().as_ref(), "10");
        assert_eq!(form.delay.read(cx).value().as_ref(), "500");
    });
    name(dialog, "不提交", cx);
    click(dialog, "settings-pool-cancel", cx);
    assert!(dialog.update(cx, |_, _, _| ()).is_err());
    assert!(drain(&mut fixture.commands).is_empty());
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
fn task_pool_edit_default_keeps_identity_and_waits_for_exact_ok_receipt(cx: &mut TestAppContext) {
    let (mut fixture, settings, before) = setup(cx, false);
    click(
        settings,
        SharedString::from(format!(
            "settings-pool-row-{}",
            before.default_pool_id.unwrap()
        )),
        cx,
    );
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("settings-pool-delete").disabled(), Some(true));
    })
    .unwrap();
    click(settings, "settings-pool-edit", cx);
    let dialog = dialog(cx);
    name(dialog, "已编辑默认池", cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "editing a management dialog does not save before OK"
    );
    click(dialog, "settings-pool-ok", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetIntruderResourcePools {
            project,
            operation_id,
            expected_revision,
            pools,
            cancel,
        },
    ] = commands.as_slice()
    else {
        panic!("OK submits exactly one catalog CAS")
    };
    assert_eq!(
        (*project, *expected_revision),
        (fixture.project, before.revision)
    );
    assert_eq!(pools[0].id, before.default_pool_id.unwrap());
    assert_eq!(pools[0].name, "已编辑默认池");
    assert!(!cancel.is_cancelled());
    assert_eq!(
        cx.read(|cx| fixture
            .view
            .read(cx)
            .intruder
            .resource_pools
            .catalog
            .clone()),
        Some(before.clone())
    );
    let after = IntruderResourcePools {
        revision: before.revision + 1,
        pools: pools.clone(),
        default_pool_id: before.default_pool_id,
    };
    emit(
        &fixture,
        Event::IntruderResourcePoolsUpdated {
            project: fixture.project,
            operation_id: Uuid::new_v4(),
            result: Ok(after.clone()),
        },
        cx,
    );
    assert!(dialog.update(cx, |_, _, _| ()).is_ok());
    assert!(drain(&mut fixture.commands).is_empty());
    emit(
        &fixture,
        Event::IntruderResourcePoolsUpdated {
            project: fixture.project,
            operation_id: *operation_id,
            result: Ok(after.clone()),
        },
        cx,
    );
    assert!(dialog.update(cx, |_, _, _| ()).is_err());
    answer_reads(&mut fixture, &after, false, cx);
    assert_eq!(
        cx.read(|cx| fixture
            .view
            .read(cx)
            .intruder
            .resource_pools
            .catalog
            .clone()),
        Some(after)
    );
}

#[gpui_kit::test]
fn closing_pending_pool_editor_cancels_admission_but_owner_accepts_late_commit(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings, before) = setup(cx, false);
    click(settings, "settings-pool-new", cx);
    let dialog = dialog(cx);
    name(dialog, "迟到已提交", cx);
    click(dialog, "settings-pool-ok", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetIntruderResourcePools {
            operation_id,
            pools,
            cancel,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("new pool commit")
    };
    let after = IntruderResourcePools {
        revision: before.revision + 1,
        pools: pools.clone(),
        default_pool_id: before.default_pool_id,
    };
    click(dialog, "settings-pool-cancel", cx);
    cx.update_window(settings, |_, window, cx| window.press("escape", cx))
        .unwrap();
    cx.run_until_parked();
    assert!(cancel.is_cancelled());
    cx.read(|cx| {
        assert!(
            fixture
                .view
                .read(cx)
                .intruder
                .resource_pools
                .management
                .mutation_pending()
        )
    });
    emit(
        &fixture,
        Event::IntruderResourcePoolsUpdated {
            project: fixture.project,
            operation_id: *operation_id,
            result: Ok(after.clone()),
        },
        cx,
    );
    answer_reads(&mut fixture, &after, false, cx);
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).intruder.resource_pools.catalog,
            Some(after)
        );
        assert!(
            !fixture
                .view
                .read(cx)
                .intruder
                .resource_pools
                .management
                .mutation_pending()
        );
    });
}

fn delete_pool_fixture(used: bool, cx: &mut TestAppContext) {
    let (mut fixture, settings, before) = setup(cx, used);
    let removed = before.pools[1].id;
    click(
        settings,
        SharedString::from(format!("settings-pool-row-{removed}")),
        cx,
    );
    click(settings, "settings-pool-delete", cx);
    if used {
        assert!(cx.has_pending_prompt());
        assert!(drain(&mut fixture.commands).is_empty());
        cx.simulate_prompt_answer("否");
        cx.run_until_parked();
        assert!(drain(&mut fixture.commands).is_empty());
        click(settings, "settings-pool-delete", cx);
        cx.simulate_prompt_answer("是");
        cx.run_until_parked();
    } else {
        assert!(!cx.has_pending_prompt());
    }
    let commands = drain(&mut fixture.commands);
    let [
        Command::DeleteIntruderResourcePool {
            project,
            operation_id,
            pool_id,
            expected_catalog_revision,
            expected_workspace_revision,
            allow_reassign,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("Delete must send only the correlated catalog/workspace CAS")
    };
    assert_eq!(
        (
            *project,
            *pool_id,
            *expected_catalog_revision,
            *allow_reassign
        ),
        (fixture.project, removed, before.revision, used)
    );
    let workspace = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
    assert_eq!(*expected_workspace_revision, workspace.revision);
    let mut after = before.clone();
    after.revision += 1;
    after.pools.remove(1);
    assert_eq!(
        cx.read(|cx| fixture
            .view
            .read(cx)
            .intruder
            .resource_pools
            .catalog
            .clone()),
        Some(before)
    );
    emit(
        &fixture,
        Event::IntruderResourcePoolDeleted {
            project: fixture.project,
            operation_id: *operation_id,
            pool_id: removed,
            result: Ok(IntruderPoolDeleteReceipt {
                catalog: after.clone(),
                workspace,
                reassigned_runs: u64::from(used),
                reassigned_tabs: 0,
            }),
        },
        cx,
    );
    answer_reads(&mut fixture, &after, false, cx);
    assert_eq!(
        cx.read(|cx| fixture
            .view
            .read(cx)
            .intruder
            .resource_pools
            .catalog
            .clone()),
        Some(after)
    );
}

#[gpui_kit::test]
fn unused_pool_delete_is_immediate_and_waits_for_receipt(cx: &mut TestAppContext) {
    delete_pool_fixture(false, cx);
}

#[gpui_kit::test]
fn used_pool_delete_requires_explicit_yes_and_no_is_inert(cx: &mut TestAppContext) {
    delete_pool_fixture(true, cx);
}
