use super::*;

#[gpui_kit::test]
fn proxy_interception_rules_param_name_and_value_normalize_from_body_and_save_exact_ack(
    cx: &mut TestAppContext,
) {
    use mio_core::ProxyInterceptRelation::{DoesNotMatch, Matches};
    use mio_core::ProxyInterceptType::{ParamName, ParamValue};
    let (mut fixture, settings) = proxy_rule_settings(cx);
    for response in [false, true] {
        let stage = if response { "response" } else { "request" };
        for (match_type, label, relation, relation_label) in [
            (ParamName, "参数名称", Matches, "匹配"),
            (ParamValue, "参数值", DoesNotMatch, "不匹配"),
        ] {
            let original = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
            let id = if response {
                original.proxy_interception.response.rules[0].id
            } else {
                original.proxy_interception.request.rules[0].id
            };
            proxy_rule_click(
                settings,
                SharedString::from(format!("proxy-{stage}-rule-row-{id}")),
                cx,
            );
            proxy_rule_click(
                settings,
                if response {
                    "proxy-response-rule-edit"
                } else {
                    "proxy-request-rule-edit"
                },
                cx,
            );
            let dialog = proxy_rule_dialog(settings, &fixture, cx);
            if response && match_type == ParamName {
                proxy_rule_choose_type(dialog, "正文", true, cx);
                proxy_rule_open_dropdown(dialog, "proxy-rule-editor-relationship", cx);
                cx.simulate_keystrokes(dialog, "down down enter");
                cx.run_until_parked();
                cx.update_window(dialog, |_, window, cx| {
                    window.render_frame(cx);
                    assert_eq!(
                        window.find("proxy-rule-editor-relationship").value(),
                        Some("响应中的正文匹配")
                    );
                })
                .unwrap();
            }
            proxy_rule_choose_type(dialog, label, response, cx);
            proxy_rule_open_dropdown(dialog, "proxy-rule-editor-relationship", cx);
            let expected_initial = if !response && match_type == ParamName {
                "不匹配"
            } else {
                "匹配"
            };
            let current = cx
                .update_window(dialog, |_, window, cx| {
                    window.render_frame(cx);
                    assert_eq!(
                        window.find("proxy-rule-editor-relationship").value(),
                        Some(expected_initial),
                        "switching to {label} must preserve compatible relationships or normalize incompatible Body relationships to Matches"
                    );
                    for expected in ["匹配", "不匹配"] {
                        assert!(!window.find_all_by_label(expected).is_empty());
                    }
                    for unexpected in [
                        "请求中匹配",
                        "请求中不匹配",
                        "响应中匹配",
                        "响应中不匹配",
                        "请求中的正文匹配",
                        "请求中的正文不匹配",
                        "响应中的正文匹配",
                        "响应中的正文不匹配",
                    ] {
                        assert!(window.find_all_by_label(unexpected).is_empty());
                    }
                    usize::from(
                        window.find("proxy-rule-editor-relationship").value()
                            == Some("不匹配"),
                    )
                })
                .unwrap();
            let destination = usize::from(relation == DoesNotMatch);
            let direction = if destination < current {
                "up "
            } else {
                "down "
            };
            cx.simulate_keystrokes(
                dialog,
                &format!("{}enter", direction.repeat(destination.abs_diff(current))),
            );
            cx.run_until_parked();
            let before_revision = cx
                .update_window(dialog, |_, window, cx| {
                    window.render_frame(cx);
                    assert_eq!(
                        window.find("proxy-rule-editor-relationship").value(),
                        Some(relation_label)
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
            let condition = format!("^MIO-{stage}-{label}-{destination}$");
            proxy_rule_condition(dialog, &condition, cx);
            cx.update_window(dialog, |_, window, cx| {
                window.render_frame(cx);
                assert_eq!(
                    window.find("proxy-rule-editor-condition").value(),
                    Some(condition.as_str())
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
                    "input must advance the document revision before saving {stage} {label}"
                );
            })
            .unwrap();
            proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
            let candidate = saved(&mut fixture.commands);
            let mut expected = original.clone();
            expected.revision += 1;
            expected.tab_sidebar_width = cx.read(|cx| fixture.view.read(cx).tab_width as u16);
            let rule = if response {
                &mut expected.proxy_interception.response.rules[0]
            } else {
                &mut expected.proxy_interception.request.rules[0]
            };
            rule.match_type = match_type;
            rule.relationship = relation;
            rule.condition = condition;
            assert_eq!(candidate, expected);
            assert!(cx.windows().contains(&dialog));
            acknowledge(&fixture, candidate.clone(), cx);
            assert!(!cx.windows().contains(&dialog));
            cx.read(|cx| {
                let view = fixture.view.read(cx);
                assert_eq!(*view.workspace, candidate);
                assert!(view.pending_workspace.is_none());
                assert!(view.pending_proxy_interception.is_none());
            });
            cx.update_window(settings, |_, window, cx| {
                window.render_frame(cx);
                let row = window.within(SharedString::from(format!("proxy-{stage}-rule-row-{id}")));
                assert!(row.find_by_label(label).visible());
                assert!(row.find_by_label(relation_label).visible());
            })
            .unwrap();
        }
    }
}

#[gpui_kit::test]
fn proxy_interception_rules_cookie_and_body_relation_choices_save_actual_input_and_exact_ack(
    cx: &mut TestAppContext,
) {
    use mio_core::ProxyInterceptRelation::{
        DoesNotMatch, Matches, RequestDoesNotMatch, RequestMatches, ResponseDoesNotMatch,
        ResponseMatches,
    };
    use mio_core::ProxyInterceptType::{Body, CookieName, CookieValue};
    let (mut fixture, settings) = proxy_rule_settings(cx);
    for response in [false, true] {
        for (match_type, label) in [
            (CookieName, "Cookie 名称"),
            (CookieValue, "Cookie 值"),
            (Body, "正文"),
        ] {
            let relations = if !response {
                vec![(Matches, "匹配"), (DoesNotMatch, "不匹配")]
            } else if match_type == Body {
                vec![
                    (RequestMatches, "请求中的正文匹配"),
                    (RequestDoesNotMatch, "请求中的正文不匹配"),
                    (ResponseMatches, "响应中的正文匹配"),
                    (ResponseDoesNotMatch, "响应中的正文不匹配"),
                ]
            } else {
                vec![
                    (RequestMatches, "请求中匹配"),
                    (RequestDoesNotMatch, "请求中不匹配"),
                    (ResponseMatches, "响应中匹配"),
                    (ResponseDoesNotMatch, "响应中不匹配"),
                ]
            };
            for (destination, (relationship, relationship_label)) in relations.iter().enumerate() {
                let original = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
                let id = if response {
                    original.proxy_interception.response.rules[0].id
                } else {
                    original.proxy_interception.request.rules[0].id
                };
                let stage = if response { "response" } else { "request" };
                proxy_rule_click(
                    settings,
                    SharedString::from(format!("proxy-{stage}-rule-row-{id}")),
                    cx,
                );
                proxy_rule_click(
                    settings,
                    if response {
                        "proxy-response-rule-edit"
                    } else {
                        "proxy-request-rule-edit"
                    },
                    cx,
                );
                let dialog = proxy_rule_dialog(settings, &fixture, cx);
                proxy_rule_choose_type(dialog, label, response, cx);
                proxy_rule_open_dropdown(dialog, "proxy-rule-editor-relationship", cx);
                let current = cx
                    .update_window(dialog, |_, window, cx| {
                        window.render_frame(cx);
                        for (_, expected) in &relations {
                            assert!(!window.find_all_by_label(expected).is_empty());
                        }
                        for unexpected in if !response {
                            [
                                "请求中匹配",
                                "响应中匹配",
                                "请求中的正文匹配",
                                "响应中的正文匹配",
                            ]
                        } else if match_type == Body {
                            [
                                "匹配",
                                "不匹配",
                                "请求中匹配",
                                "响应中匹配",
                            ]
                        } else {
                            [
                                "匹配",
                                "不匹配",
                                "请求中的正文匹配",
                                "响应中的正文匹配",
                            ]
                        } {
                            assert!(window.find_all_by_label(unexpected).is_empty());
                        }
                        let select = window.find("proxy-rule-editor-relationship");
                        let selected = select.value().unwrap();
                        relations
                            .iter()
                            .position(|(_, label)| *label == selected)
                            .unwrap()
                    })
                    .unwrap();
                let direction = if destination < current {
                    "up "
                } else {
                    "down "
                };
                cx.simulate_keystrokes(
                    dialog,
                    &format!("{}enter", direction.repeat(destination.abs_diff(current))),
                );
                cx.run_until_parked();
                let before_revision = cx
                    .update_window(dialog, |_, window, cx| {
                        window.render_frame(cx);
                        assert_eq!(
                            window.find("proxy-rule-editor-relationship").value(),
                            Some(*relationship_label)
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
                let condition = format!("^MIO-{stage}-{label}-{destination}$");
                proxy_rule_condition(dialog, &condition, cx);
                cx.update_window(dialog, |_, window, cx| {
                    window.render_frame(cx);
                    assert_eq!(window.find("proxy-rule-editor-condition").value(), Some(condition.as_str()));
                    let editor = window
                        .root::<gpui_kit::base::Root>().unwrap().unwrap().read(cx).view().clone()
                        .downcast::<crate::workbench::settings::InterceptionEditor>().unwrap();
                    assert!(
                        editor.read(cx).condition.read(cx).document_revision() > before_revision,
                        "input must change the actual document revision before saving {stage} {label} {relationship_label}"
                    );
                })
                .unwrap();
                proxy_rule_click(dialog, "proxy-rule-editor-ok", cx);
                let candidate = saved(&mut fixture.commands);
                let mut expected = original.clone();
                expected.revision += 1;
                expected.tab_sidebar_width = cx.read(|cx| fixture.view.read(cx).tab_width as u16);
                let rule = if response {
                    &mut expected.proxy_interception.response.rules[0]
                } else {
                    &mut expected.proxy_interception.request.rules[0]
                };
                rule.match_type = match_type;
                rule.relationship = *relationship;
                rule.condition = condition;
                assert_eq!(candidate, expected);
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
                cx.update_window(settings, |_, window, cx| {
                    window.render_frame(cx);
                    let row =
                        window.within(SharedString::from(format!("proxy-{stage}-rule-row-{id}")));
                    let cell = row.find_by_label(relationship_label);
                    assert!(cell.visible());
                    assert_eq!(cell.label(), Some(*relationship_label));
                    assert_eq!(cell.role(), Some(gpui_kit::accesskit::Role::Cell));
                })
                .unwrap();
            }
        }
    }
}
