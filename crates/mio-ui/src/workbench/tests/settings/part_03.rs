use super::*;

#[gpui_kit::test]
fn proxy_interception_rules_coalesced_old_save_ack_waits_for_candidate_ack(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    proxy_rule_click(settings, "proxy-response-rules-enabled", cx);
    let old = saved(&mut fixture.commands);
    proxy_rule_click(settings, "proxy-request-rule-add", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_condition(dialog, "^coalesced-new-rule$", cx);
    proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "new config must coalesce while the old CAS is pending"
    );
    acknowledge(&fixture, old, cx);
    let candidate = saved(&mut fixture.commands);
    assert_eq!(
        candidate
            .proxy_interception
            .request
            .rules
            .last()
            .unwrap()
            .condition,
        "^coalesced-new-rule$"
    );
    assert!(cx.windows().contains(&dialog));
    acknowledge(&fixture, candidate, cx);
    assert!(
        !cx.windows().contains(&dialog),
        "actual candidate acknowledgment must complete the retained editor"
    );
}

#[gpui_kit::test]
fn proxy_interception_rules_pending_snapshot_survives_coalesced_editor_base(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    proxy_rule_click(settings, "proxy-response-rules-enabled", cx);
    let prior = saved(&mut fixture.commands);
    proxy_rule_click(settings, "proxy-request-rules-enabled", cx);
    assert!(drain(&mut fixture.commands).is_empty());
    let base = cx.read(|cx| fixture.view.read(cx).workspace.proxy_interception.clone());
    assert_ne!(base, prior.proxy_interception);
    assert!(!base.request.enabled && base.response.enabled);
    proxy_rule_click(settings, "proxy-request-rule-add", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_condition(dialog, "^coalesced-after-pending-snapshot$", cx);
    proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
    assert!(drain(&mut fixture.commands).is_empty());
    acknowledge(&fixture, prior.clone(), cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveWorkspace {
            project,
            workspace: candidate,
        },
    ] = commands.as_slice()
    else {
        panic!(
            "a valid prior ACK must release the candidate using the actually enqueued snapshot, not the later editor base"
        )
    };
    assert_eq!(*project, fixture.project);
    assert_eq!(candidate.revision, prior.revision + 1);
    assert!(!candidate.proxy_interception.request.enabled);
    assert_eq!(candidate.proxy_interception.response, base.response);
    assert_eq!(
        candidate.proxy_interception.request.rules.len(),
        base.request.rules.len() + 1
    );
    assert_eq!(
        candidate
            .proxy_interception
            .request
            .rules
            .last()
            .unwrap()
            .condition,
        "^coalesced-after-pending-snapshot$"
    );
    assert!(cx.windows().contains(&dialog));
    acknowledge(&fixture, candidate.clone(), cx);
    assert!(!cx.windows().contains(&dialog));
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(&*view.workspace, candidate);
        assert!(view.error.is_none());
        assert!(!view.workspace_dirty && view.pending_workspace.is_none());
    });
}

#[gpui_kit::test]
fn proxy_interception_rules_checkbox_mouse_press_keeps_original_project(cx: &mut TestAppContext) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    let position = cx
        .update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            let checkbox = window.find("proxy-response-rules-enabled");
            assert_eq!(checkbox.checked(), Some(false));
            let position = checkbox.bounds().center();
            window.pointer_down(position, MouseButton::Left, cx);
            position
        })
        .unwrap();
    let next = ProjectKey {
        id: Uuid::new_v4(),
        generation: fixture.project.generation + 1,
    };
    let workspace = Workspace::default();
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: next,
            path: None,
            workspace: workspace.clone(),
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.pointer_up(position, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.project, Some(next));
        assert_eq!(
            view.workspace.proxy_interception, workspace.proxy_interception,
            "a checkbox press in the previous project must not change the new project's identical rules"
        );
        assert!(!view.workspace_dirty && view.pending_workspace.is_none());
    });
    assert!(
        !drain(&mut fixture.commands)
            .iter()
            .any(|command| matches!(command, Command::SaveWorkspace { .. }))
    );
    fixture.project = next;
    proxy_rule_click(settings, "proxy-response-rules-enabled", cx);
    let commands = drain(&mut fixture.commands);
    let [
        Command::SaveWorkspace {
            project,
            workspace: candidate,
        },
    ] = commands.as_slice()
    else {
        panic!("a new checkbox press must still save the current project")
    };
    assert_eq!(*project, next);
    assert!(candidate.proxy_interception.response.enabled);
    acknowledge(&fixture, candidate.clone(), cx);
    checkbox_checked(settings, "proxy-response-rules-enabled", true, cx);
}

#[gpui_kit::test]
fn proxy_interception_rules_wrong_ack_payload_never_closes_pending_editor(cx: &mut TestAppContext) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    proxy_rule_click(settings, "proxy-request-rule-add", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_condition(dialog, "^candidate-not-confirmed$", cx);
    proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
    let sent = saved(&mut fixture.commands);
    let mut wrong = sent.clone();
    wrong
        .proxy_interception
        .request
        .rules
        .last_mut()
        .unwrap()
        .condition = "^different-payload$".into();
    acknowledge(&fixture, wrong, cx);
    assert!(
        cx.windows().contains(&dialog),
        "same revision with different actual rules is not candidate confirmation"
    );
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("proxy-rule-editor-condition").value(),
            Some("^candidate-not-confirmed$")
        );
        assert!(window.find("proxy-rule-editor-error").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn proxy_interception_rules_wrong_prior_ack_does_not_advance_coalesced_candidate(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    proxy_rule_click(settings, "proxy-response-rules-enabled", cx);
    let prior = saved(&mut fixture.commands);
    proxy_rule_click(settings, "proxy-request-rule-add", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_condition(dialog, "^coalesced-after-wrong-prior$", cx);
    proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
    assert!(drain(&mut fixture.commands).is_empty());
    let mut wrong = prior.clone();
    wrong.proxy_interception.request.rules[0].condition = "^not-the-captured-prior$".into();
    acknowledge(&fixture, wrong, cx);
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "wrong prior payload must not flush the coalesced candidate"
    );
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert_eq!(view.pending_workspace, Some(prior.revision));
        assert_eq!(view.workspace.revision, prior.revision - 1);
    });
    assert!(cx.windows().contains(&dialog));
    acknowledge(&fixture, prior, cx);
    let candidate = saved(&mut fixture.commands);
    acknowledge(&fixture, candidate, cx);
    assert!(!cx.windows().contains(&dialog));
}

#[gpui_kit::test]
fn proxy_interception_rules_unchanged_confirmed_edit_closes_without_extra_save(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let id = original.proxy_interception.request.rules[0].id;
    proxy_rule_click(
        settings,
        SharedString::from(format!("proxy-request-rule-row-{id}")),
        cx,
    );
    proxy_rule_click(settings, "proxy-request-rule-edit", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
    assert!(drain(&mut fixture.commands).is_empty());
    assert!(
        !cx.windows().contains(&dialog),
        "an unchanged confirmed edit needs no nonexistent future acknowledgment"
    );
    cx.read(|cx| assert_eq!(fixture.view.read(cx).workspace, original));
}

#[gpui_kit::test]
fn proxy_interception_rules_toggle_reorder_remove_preserve_stable_identity(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    let id = original.proxy_interception.request.rules[2].id;
    proxy_rule_click(
        settings,
        SharedString::from(format!("proxy-request-rule-enabled-{id}")),
        cx,
    );
    let enabled = saved(&mut fixture.commands);
    assert!(enabled.proxy_interception.request.rules[2].enabled);
    acknowledge(&fixture, enabled, cx);
    proxy_rule_click(
        settings,
        SharedString::from(format!("proxy-request-rule-row-{id}")),
        cx,
    );
    proxy_rule_click(settings, "proxy-request-rule-up", cx);
    let up = saved(&mut fixture.commands);
    assert_eq!(up.proxy_interception.request.rules[1].id, id);
    acknowledge(&fixture, up, cx);
    proxy_rule_click(settings, "proxy-request-rule-down", cx);
    let down = saved(&mut fixture.commands);
    assert_eq!(down.proxy_interception.request.rules[2].id, id);
    acknowledge(&fixture, down, cx);
    proxy_rule_click(settings, "proxy-request-rule-remove", cx);
    let removed = saved(&mut fixture.commands);
    assert_eq!(removed.proxy_interception.request.rules.len(), 3);
    assert!(
        removed
            .proxy_interception
            .request
            .rules
            .iter()
            .all(|rule| rule.id != id)
    );
    acknowledge(&fixture, removed, cx);
    proxy_rule_click(settings, "proxy-response-rules-enabled", cx);
    let responses = saved(&mut fixture.commands);
    assert!(responses.proxy_interception.response.enabled);
    acknowledge(&fixture, responses, cx);
}

#[gpui_kit::test]
fn proxy_interception_rules_defaults_cancel_and_apply_only_the_selected_group(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    proxy_rule_click(settings, "proxy-request-rules-enabled", cx);
    let requests = saved(&mut fixture.commands);
    assert!(!requests.proxy_interception.request.enabled);
    acknowledge(&fixture, requests, cx);
    proxy_rule_click(settings, "proxy-response-rules-enabled", cx);
    let modified = saved(&mut fixture.commands);
    acknowledge(&fixture, modified.clone(), cx);
    proxy_rule_click(settings, "proxy-request-rules-defaults", cx);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("否");
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| assert_eq!(*fixture.view.read(cx).workspace, modified));
    proxy_rule_click(settings, "proxy-request-rules-defaults", cx);
    cx.simulate_prompt_answer("是");
    cx.run_until_parked();
    let reset = saved(&mut fixture.commands);
    assert_eq!(
        reset.proxy_interception.request,
        mio_core::ProxyInterceptionSettings::default().request
    );
    assert_eq!(
        reset.proxy_interception.response,
        modified.proxy_interception.response
    );
    acknowledge(&fixture, reset, cx);
}

#[gpui_kit::test]
fn proxy_interception_rules_invalid_regex_keeps_dialog_input_and_sends_nothing(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    proxy_rule_click(settings, "proxy-request-rule-add", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_condition(dialog, "[", cx);
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-rule-editor-ok", cx);
        assert!(
            fixture.commands.try_recv().is_err(),
            "event handler must return before background regex validation or save"
        );
        window.render_frame(cx);
        assert!(
            window.try_find("proxy-rule-editor-error").is_none(),
            "regex validation must not run synchronously in the UI event"
        );
    })
    .unwrap();
    cx.run_until_parked();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| assert_eq!(fixture.view.read(cx).workspace, original));
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("proxy-rule-editor-condition").value(),
            Some("[")
        );
        assert!(window.find("proxy-rule-editor-error").visible());
    })
    .unwrap();
    proxy_rule_condition(dialog, "^valid$", cx);
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        let ok = window.find("proxy-rule-editor-ok");
        assert!(
            ok.bounds().bottom() <= window.viewport_size().height,
            "validation errors must not push OK outside the interactive viewport: OK={:?}, viewport={:?}",
            ok.bounds(), window.viewport_size(),
        );
        assert_eq!(
            window.find("proxy-rule-editor-condition").value(),
            Some("^valid$")
        );
    })
    .unwrap();
    proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
    let fixed = saved(&mut fixture.commands);
    assert_eq!(
        fixed
            .proxy_interception
            .request
            .rules
            .last()
            .unwrap()
            .condition,
        "^valid$"
    );
    acknowledge(&fixture, fixed, cx);
}

#[gpui_kit::test]
fn proxy_interception_rules_cancel_during_background_validation_never_enqueues_save(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    proxy_rule_click(settings, "proxy-request-rule-add", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_condition(dialog, "^cancel-before-validation$", cx);
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-rule-editor-ok", cx);
        assert!(fixture.commands.try_recv().is_err());
        window.render_frame(cx);
        window.click("proxy-rule-editor-cancel", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!cx.windows().contains(&dialog));
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| assert_eq!(fixture.view.read(cx).workspace, original));
}
