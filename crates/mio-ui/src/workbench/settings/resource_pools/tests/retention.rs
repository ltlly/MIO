use super::*;

#[gpui_kit::test]
fn management_pool_editor_preserves_ime_and_failed_commit_input(cx: &mut TestAppContext) {
    let (mut fixture, settings, before) = setup(cx, false);
    click(settings, "settings-pool-new", cx);
    let dialog = dialog(cx);
    let content = editor(dialog, cx);
    cx.update_window(dialog, |_, window, cx| {
        let input = content.read(cx).form.name.clone();
        input.update(cx, |input, cx| {
            input.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
        });
        window.click("settings-pool-ok", cx);
        assert!(input.update(cx, |input, cx| {
            input.marked_text_range(window, cx).is_some()
        }));
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(dialog, |_, window, cx| window.input("输入保留", cx))
        .unwrap();
    cx.run_until_parked();
    click(dialog, "settings-pool-ok", cx);
    let commands = drain(&mut fixture.commands);
    let [Command::SetIntruderResourcePools { operation_id, .. }] = commands.as_slice() else {
        panic!("committed IME may submit one pool")
    };
    emit(
        &fixture,
        Event::IntruderResourcePoolsUpdated {
            project: fixture.project,
            operation_id: *operation_id,
            result: Err("fixture disk full".into()),
        },
        cx,
    );
    cx.read(|cx| {
        assert_eq!(
            content.read(cx).form.name.read(cx).value().as_ref(),
            "输入保留"
        );
        assert!(!content.read(cx).pending);
        assert!(
            content
                .read(cx)
                .error
                .as_ref()
                .unwrap()
                .contains("fixture disk full")
        );
        assert_eq!(
            fixture.view.read(cx).intruder.resource_pools.catalog,
            Some(before)
        );
    });
    assert!(dialog.update(cx, |_, _, _| ()).is_ok());
}

#[gpui_kit::test]
fn task_usage_changes_coalesce_while_query_is_pending_and_do_not_poll_unopened_tasks(
    cx: &mut TestAppContext,
) {
    let mut untouched = fixture(cx, vec![tab("未打开 Tasks")], 64);
    drain(&mut untouched.commands);
    untouched
        .view
        .update(cx, |view, cx| view.settings_pool_usage_changed(cx));
    assert!(drain(&mut untouched.commands).is_empty());

    let (mut fixture, _, catalog) = setup(cx, false);
    fixture
        .view
        .update(cx, |view, cx| view.settings_pool_usage_changed(cx));
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryIntruderPoolUsage {
            project,
            query_revision: first,
        },
    ] = commands.as_slice()
    else {
        panic!("a relevant change after Tasks has been read queries usage only once")
    };
    assert_eq!(*project, fixture.project);
    for _ in 0..3 {
        fixture
            .view
            .update(cx, |view, cx| view.settings_pool_usage_changed(cx));
    }
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "in-flight invalidations coalesce without duplicate queries"
    );
    let old = IntruderPoolUsage {
        catalog_revision: catalog.revision,
        task_counts: catalog.pools.iter().map(|pool| (pool.id, 0)).collect(),
        requires_confirmation: BTreeSet::new(),
    };
    emit(
        &fixture,
        Event::IntruderPoolUsage {
            project: fixture.project,
            query_revision: *first,
            result: Ok(old.clone()),
        },
        cx,
    );
    let commands = drain(&mut fixture.commands);
    let [
        Command::QueryIntruderPoolUsage {
            query_revision: next,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("the old response must schedule exactly one follow-up for intervening changes")
    };
    assert!(*next > *first);
    let mut latest = old.clone();
    latest.task_counts.insert(catalog.pools[1].id, 2);
    latest.requires_confirmation.insert(catalog.pools[1].id);
    emit(
        &fixture,
        Event::IntruderPoolUsage {
            project: fixture.project,
            query_revision: *first,
            result: Ok(old),
        },
        cx,
    );
    assert!(drain(&mut fixture.commands).is_empty());
    emit(
        &fixture,
        Event::IntruderPoolUsage {
            project: fixture.project,
            query_revision: *next,
            result: Ok(latest.clone()),
        },
        cx,
    );
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "a final fresh response must not poll again"
    );
    cx.read(|cx| {
        let management = &fixture.view.read(cx).intruder.resource_pools.management;
        assert_eq!(management.usage, Some(latest));
        assert!(management.query.is_none() && !management.usage_dirty);
    });
}

fn accept_pool_editor_save(
    fixture: &mut Fixture,
    before: &IntruderResourcePools,
    cx: &mut TestAppContext,
) -> IntruderResourcePools {
    let commands = drain(&mut fixture.commands);
    let [
        Command::SetIntruderResourcePools {
            project,
            operation_id,
            expected_revision,
            pools,
            ..
        },
    ] = commands.as_slice()
    else {
        panic!("OK must submit one complete catalog with retained input")
    };
    assert_eq!(
        (*project, *expected_revision),
        (fixture.project, before.revision)
    );
    let after = IntruderResourcePools {
        revision: before.revision + 1,
        pools: pools.clone(),
        default_pool_id: before.default_pool_id,
    };
    after.validate().unwrap();
    emit(
        fixture,
        Event::IntruderResourcePoolsUpdated {
            project: fixture.project,
            operation_id: *operation_id,
            result: Ok(after.clone()),
        },
        cx,
    );
    answer_reads(fixture, &after, false, cx);
    after
}

#[gpui_kit::test]
fn management_disabled_numeric_inputs_round_trip_and_reenable_without_losing_raw_text(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings, before) = setup(cx, false);
    let default_id = before.default_pool_id.unwrap();
    click(
        settings,
        SharedString::from(format!("settings-pool-row-{default_id}")),
        cx,
    );
    click(settings, "settings-pool-edit", cx);
    let first = dialog(cx);
    let content = editor(first, cx);
    cx.read(|cx| {
        let form = &content.read(cx).form;
        assert_eq!(
            form.delay.read(cx).value().as_ref(),
            "500",
            "legacy delay fallback belongs to the opened form only"
        );
        assert_eq!(
            fixture.view.read(cx).intruder.resource_pools.catalog,
            Some(before.clone())
        );
    });
    for (index, value) in [(1, "002"), (2, "00100"), (3, "00050")] {
        if index == 2 {
            click(first, "intruder-pool-delay-enabled", cx);
        }
        if index == 3 {
            click(first, "intruder-pool-increment", cx);
        }
        let input = cx.read(|cx| content.read(cx).form.inputs()[index].clone());
        click(first, ("input", input.entity_id()), cx);
        cx.update_window(first, |_, window, cx| {
            window.press(crate::kbd!("a"), cx);
            window.input(value, cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
    click(first, "intruder-pool-cap-enabled", cx);
    click(first, "intruder-pool-delay-enabled", cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "editing and disabling values remain local until OK"
    );
    // Match native window ownership: retaining the editor entity delays its on_release cleanup.
    drop(content);
    click(first, "settings-pool-ok", cx);
    let disabled = accept_pool_editor_save(&mut fixture, &before, cx);
    let pool = disabled.find(default_id).unwrap();
    assert_eq!(pool.max_concurrent_requests, None);
    assert_eq!(pool.delay, None);
    let retained = mio_core::IntruderPoolInputs {
        cap: "002".into(),
        delay: "00100".into(),
        increment: "00050".into(),
        delay_mode: mio_core::IntruderPoolDelayMode::Increment,
    };
    assert_eq!(pool.retained_inputs, Some(retained.clone()));
    assert!(first.update(cx, |_, _, _| ()).is_err());
    click(settings, "settings-pool-edit", cx);
    let second = dialog(cx);
    let content = editor(second, cx);
    cx.read(|cx| {
        let form = &content.read(cx).form;
        assert!(!form.cap_enabled && !form.delay_enabled);
        assert_eq!(form.cap.read(cx).value().as_ref(), retained.cap);
        assert_eq!(form.delay.read(cx).value().as_ref(), retained.delay);
        assert_eq!(form.increment.read(cx).value().as_ref(), retained.increment);
        assert_eq!(form.delay_kind, retained.delay_mode);
    });
    drop(content);
    click(second, "settings-pool-cancel", cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "Cancel does not save retained inputs again"
    );
    click(settings, "settings-pool-edit", cx);
    let third = dialog(cx);
    click(third, "intruder-pool-cap-enabled", cx);
    click(third, "intruder-pool-delay-enabled", cx);
    click(third, "settings-pool-ok", cx);
    let enabled = accept_pool_editor_save(&mut fixture, &disabled, cx);
    let pool = enabled.find(default_id).unwrap();
    assert_eq!(pool.max_concurrent_requests, Some(2));
    assert_eq!(
        pool.delay,
        Some(mio_core::IntruderPoolDelay::Increment {
            initial_ms: 100,
            increment_ms: 50
        })
    );
    assert_eq!(pool.retained_inputs, Some(retained));
}

#[gpui_kit::test]
fn management_explicitly_cleared_disabled_delay_stays_empty_when_reopened(cx: &mut TestAppContext) {
    let (mut fixture, settings, before) = setup(cx, false);
    let default_id = before.default_pool_id.unwrap();
    click(
        settings,
        SharedString::from(format!("settings-pool-row-{default_id}")),
        cx,
    );
    click(settings, "settings-pool-edit", cx);
    let first = dialog(cx);
    let content = editor(first, cx);
    let delay = cx.read(|cx| content.read(cx).form.delay.clone());
    drop(content);
    assert_eq!(cx.read(|cx| delay.read(cx).value().to_string()), "500");
    click(first, "intruder-pool-delay-enabled", cx);
    click(first, ("input", delay.entity_id()), cx);
    cx.update_window(first, |_, window, cx| {
        window.press(crate::kbd!("a"), cx);
        window.press("backspace", cx);
    })
    .unwrap();
    cx.run_until_parked();
    click(first, "intruder-pool-delay-enabled", cx);
    assert!(cx.read(|cx| delay.read(cx).value().is_empty()));
    click(first, "settings-pool-ok", cx);
    let cleared = accept_pool_editor_save(&mut fixture, &before, cx);
    let pool = cleared.find(default_id).unwrap();
    assert_eq!(pool.delay, None);
    assert_eq!(pool.retained_inputs.as_ref().unwrap().delay, "");
    click(settings, "settings-pool-edit", cx);
    let second = dialog(cx);
    let content = editor(second, cx);
    cx.read(|cx| {
        assert!(!content.read(cx).form.delay_enabled);
        assert!(content.read(cx).form.delay.read(cx).value().is_empty());
    });
    click(second, "settings-pool-cancel", cx);
    assert!(drain(&mut fixture.commands).is_empty());
    assert_eq!(
        cx.read(|cx| fixture
            .view
            .read(cx)
            .intruder
            .resource_pools
            .catalog
            .clone()),
        Some(cleared)
    );
}
