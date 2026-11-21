use super::*;

#[gpui_kit::test]
fn proxy_interception_rules_stale_project_or_revision_cannot_apply_dialog_draft(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    proxy_rule_click(settings, "proxy-request-rule-add", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_condition(dialog, "^old-project$", cx);
    fixture.project.generation += 1;
    fixture
        .events
        .try_send(Event::ProjectOpened {
            project: fixture.project,
            path: None,
            workspace: Workspace::default(),
        })
        .unwrap();
    cx.run_until_parked();
    drain(&mut fixture.commands);
    proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        assert_eq!(
            fixture.view.read(cx).workspace.proxy_interception,
            mio_core::ProxyInterceptionSettings::default()
        )
    });
    proxy_rule_click(dialog, "proxy-rule-editor-cancel", cx);
    proxy_rule_click(settings, "proxy-request-rule-add", cx);
    let dialog = proxy_rule_dialog(settings, &fixture, cx);
    proxy_rule_condition(dialog, "^old-revision$", cx);
    cx.update(|cx| {
        fixture.view.update(cx, |view, cx| {
            view.workspace.revision += 1;
            cx.notify();
        })
    });
    proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
    assert!(drain(&mut fixture.commands).is_empty());
    cx.update_window(dialog, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("proxy-rule-editor-error").visible());
        assert_eq!(
            window.find("proxy-rule-editor-condition").value(),
            Some("^old-revision$")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn proxy_interception_rules_protocol_selects_https_without_condition_and_waits_for_exact_ack(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    for response in [false, true] {
        let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
        proxy_rule_click(
            settings,
            if response {
                "proxy-response-rule-add"
            } else {
                "proxy-request-rule-add"
            },
            cx,
        );
        let dialog = proxy_rule_dialog(settings, &fixture, cx);
        proxy_rule_condition(dialog, "^preserved-disabled-condition$", cx);
        proxy_rule_open_dropdown(dialog, "proxy-rule-editor-type", cx);
        cx.simulate_keystrokes(dialog, "down down enter");
        cx.run_until_parked();
        cx.update_window(dialog, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("proxy-rule-editor-type").value(),
                Some("Protocol")
            );
            assert_eq!(
                window.find("proxy-rule-editor-relationship").value(),
                Some("Is HTTP")
            );
            assert_eq!(
                window.find("proxy-rule-editor-condition").value(),
                Some("^preserved-disabled-condition$")
            );
        })
        .unwrap();
        assert_proxy_rule_condition_disabled(dialog, cx);
        proxy_rule_open_dropdown(dialog, "proxy-rule-editor-relationship", cx);
        cx.update_window(dialog, |_, window, cx| {
            window.render_frame(cx);
            for label in ["Is HTTP", "Is HTTPS"] {
                assert!(!window.find_all_by_label(label).is_empty());
            }
            for label in ["Matches", "Does not match", "Contains parameters"] {
                assert!(window.find_all_by_label(label).is_empty());
            }
        })
        .unwrap();
        cx.simulate_keystrokes(dialog, "down enter");
        cx.run_until_parked();
        cx.update_window(dialog, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("proxy-rule-editor-relationship").value(),
                Some("Is HTTPS")
            );
        })
        .unwrap();
        assert_proxy_rule_condition_disabled(dialog, cx);
        proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
        let candidate = saved(&mut fixture.commands);
        let rules = if response {
            assert_eq!(
                candidate.proxy_interception.request,
                original.proxy_interception.request
            );
            &candidate.proxy_interception.response.rules
        } else {
            assert_eq!(
                candidate.proxy_interception.response,
                original.proxy_interception.response
            );
            &candidate.proxy_interception.request.rules
        };
        let rule = rules.last().unwrap();
        assert_eq!(rule.match_type, mio_core::ProxyInterceptType::Protocol);
        assert_eq!(rule.relationship, mio_core::ProxyInterceptRelation::IsHttps);
        assert!(rule.condition.is_empty());
        assert_eq!(candidate.revision, original.revision + 1);
        assert!(cx.windows().contains(&dialog));
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(view.pending_workspace, Some(candidate.revision));
            assert_eq!(
                view.pending_proxy_interception,
                Some((
                    candidate.revision,
                    candidate.proxy_interception.clone(),
                    candidate.proxy_messages,
                    candidate.proxy_websocket_interception
                ))
            );
        });
        acknowledge(&fixture, candidate.clone(), cx);
        assert!(!cx.windows().contains(&dialog));
        cx.read(|cx| {
            let view = fixture.view.read(cx);
            assert_eq!(*view.workspace, candidate);
            assert!(view.pending_workspace.is_none());
            assert!(view.pending_proxy_interception.is_none());
        });
    }
}

#[gpui_kit::test]
fn proxy_interception_rules_listener_port_selects_last_type_and_saves_latest_regex_input(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    for response in [false, true] {
        let original = cx.read(|cx| fixture.view.read(cx).workspace.clone());
        proxy_rule_click(
            settings,
            if response {
                "proxy-response-rule-add"
            } else {
                "proxy-request-rule-add"
            },
            cx,
        );
        let dialog = proxy_rule_dialog(settings, &fixture, cx);
        proxy_rule_open_dropdown(dialog, "proxy-rule-editor-type", cx);
        cx.simulate_keystrokes(
            dialog,
            if response {
                "down down down down down down down down down down down down down down down down down enter"
            } else {
                "down down down down down down down down down down down down down enter"
            },
        );
        cx.run_until_parked();
        let before_revision = cx
            .update_window(dialog, |_, window, cx| {
                window.render_frame(cx);
                assert_eq!(
                    window.find("proxy-rule-editor-type").value(),
                    Some("Listener port")
                );
                let editor = window
                    .root::<gpui_kit::base::Root>()
                    .unwrap()
                    .unwrap()
                    .read(cx)
                    .view()
                    .clone()
                    .downcast::<crate::workbench::settings::InterceptionEditor>()
                    .unwrap();
                assert!(
                    !editor
                        .read(cx)
                        .condition
                        .read(cx)
                        .presentation()
                        .is_disabled()
                );
                editor.read(cx).condition.read(cx).document_revision()
            })
            .unwrap();
        proxy_rule_condition(dialog, "^8080$", cx);
        cx.update_window(dialog, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("proxy-rule-editor-condition").value(),
                Some("^8080$")
            );
            let editor = window
                .root::<gpui_kit::base::Root>()
                .unwrap()
                .unwrap()
                .read(cx)
                .view()
                .clone()
                .downcast::<crate::workbench::settings::InterceptionEditor>()
                .unwrap();
            assert!(
                editor.read(cx).condition.read(cx).document_revision() > before_revision,
                "actual condition input must advance its document revision before confirming"
            );
        })
        .unwrap();
        proxy_rule_open_dropdown(dialog, "proxy-rule-editor-relationship", cx);
        cx.update_window(dialog, |_, window, cx| {
            window.render_frame(cx);
            for label in ["Matches", "Does not match"] {
                assert!(!window.find_all_by_label(label).is_empty());
            }
            assert!(window.find_all_by_label("Is HTTP").is_empty());
            assert!(window.find_all_by_label("Is HTTPS").is_empty());
        })
        .unwrap();
        cx.simulate_keystrokes(dialog, "down enter");
        cx.run_until_parked();
        proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
        let candidate = saved(&mut fixture.commands);
        let rules = if response {
            assert_eq!(
                candidate.proxy_interception.request,
                original.proxy_interception.request
            );
            &candidate.proxy_interception.response.rules
        } else {
            assert_eq!(
                candidate.proxy_interception.response,
                original.proxy_interception.response
            );
            &candidate.proxy_interception.request.rules
        };
        let rule = rules.last().unwrap();
        assert_eq!(rule.match_type, mio_core::ProxyInterceptType::ListenerPort);
        assert_eq!(
            rule.relationship,
            mio_core::ProxyInterceptRelation::DoesNotMatch
        );
        assert_eq!(rule.condition, "^8080$");
        assert_eq!(candidate.revision, original.revision + 1);
        assert!(cx.windows().contains(&dialog));
        acknowledge(&fixture, candidate.clone(), cx);
        assert!(!cx.windows().contains(&dialog));
        cx.read(|cx| assert_eq!(*fixture.view.read(cx).workspace, candidate));
    }
}
