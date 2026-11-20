use gpui_kit::test::TestWindowExt;

use super::*;

#[gpui_kit::test]
fn baseline_new_tab_default_and_group_restore_require_explicit_confirmation(
    cx: &mut TestAppContext,
) {
    let mut fixture = intruder_fixture(cx, vec![], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.click("intruder-new", cx);
    })
    .unwrap();
    cx.run_until_parked();
    acknowledge_default_pool(&mut fixture, cx);
    let initial = saved(&mut fixture);
    assert!(initial.intruder.tabs[0].spec.baseline_request);
    acknowledge(&fixture, initial.clone(), cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("intruder-sidebar-settings", cx);
        assert_eq!(
            window.find("intruder-config-baseline").checked(),
            Some(true)
        );
        window.click("intruder-config-baseline", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let disabled = saved(&mut fixture);
    assert!(!disabled.intruder.tabs[0].spec.baseline_request);
    assert_eq!(
        disabled.intruder.tabs[0].spec.draft,
        initial.intruder.tabs[0].spec.draft
    );
    assert_eq!(
        disabled.intruder.tabs[0].spec.save_to_project,
        initial.intruder.tabs[0].spec.save_to_project
    );
    acknowledge(&fixture, disabled.clone(), cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("intruder-attack-results-defaults", cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("否");
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| assert_eq!(fixture.view.read(cx).workspace, disabled));
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("intruder-attack-results-defaults", cx)
    })
    .unwrap();
    cx.run_until_parked();
    cx.simulate_prompt_answer("是");
    cx.run_until_parked();
    let restored = saved(&mut fixture);
    let mut expected = disabled.intruder.tabs[0].spec.clone();
    expected.baseline_request = true;
    expected.revision += 1;
    assert_eq!(restored.intruder.tabs[0].spec, expected);
}

#[gpui_kit::test]
fn baseline_change_revokes_a_pending_start_without_harvesting_ime(cx: &mut TestAppContext) {
    let config = attack();
    let id = config.id;
    let mut fixture = intruder_fixture(cx, vec![config.clone()], 64);
    start(&fixture, cx);
    let pending = saved(&mut fixture);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.click("intruder-sidebar-settings", cx);
        window.click("intruder-config-baseline", cx);
        let view = fixture.view.read(cx);
        assert!(view.intruder.starts.is_empty());
        assert!(view.workspace.intruder.tabs[0].spec.baseline_request);
        assert_eq!(
            view.workspace.intruder.tabs[0].spec.draft,
            config.spec.draft
        );
    })
    .unwrap();
    acknowledge(&fixture, pending, cx);
    let saved_config = saved(&mut fixture);
    acknowledge(&fixture, saved_config.clone(), cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        let input = fixture.view.read(cx).intruder.editors[&id].target.clone();
        window.click(("input", input.entity_id()), cx);
        input.update(cx, |state, cx| {
            state.replace_and_mark_text_in_range(None, "zhong", Some(5..5), window, cx)
        });
        window.click("intruder-config-baseline", cx);
        assert_eq!(fixture.view.read(cx).workspace, saved_config);
        assert!(input.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }));
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
}

#[gpui_kit::test]
fn baseline_enabled_empty_payload_start_is_rejected_before_save_or_admission(
    cx: &mut TestAppContext,
) {
    let config = attack();
    let id = config.id;
    let mut fixture = intruder_fixture(cx, vec![config.clone()], 64);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("tool-intruder", cx);
        window.click("intruder-payload-clear", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let empty = saved(&mut fixture);
    assert_eq!(
        empty.intruder.tabs[0].spec.payload_sets,
        vec![Vec::<Vec<u8>>::new()]
    );
    acknowledge(&fixture, empty, cx);
    cx.update_window(fixture.window, |_, window, cx| {
        window.click("intruder-sidebar-settings", cx);
        window.click("intruder-config-baseline", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let enabled = saved(&mut fixture);
    assert!(enabled.intruder.tabs[0].spec.baseline_request);
    assert_eq!(enabled.intruder.tabs[0].spec.validate().unwrap(), 1);
    assert_eq!(enabled.intruder.tabs[0].spec.draft, config.spec.draft);
    assert_eq!(
        enabled.intruder.tabs[0].spec.positions,
        config.spec.positions
    );
    acknowledge(&fixture, enabled.clone(), cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(fixture.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("intruder-config-baseline").checked(),
            Some(true)
        );
        window.click("intruder-start", cx);
        window.click("intruder-start", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "an enabled baseline is not a payload; empty Start must be rejected by the UI before any SaveWorkspace or StartIntruder"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.workspace, enabled);
        assert!(!view.intruder.starts.contains_key(&id));
        assert!(
            view.error
                .as_ref()
                .is_some_and(|error| error.contains("payload 列表为空"))
        );
        assert!(view.pending_workspace.is_none());
    });
}
