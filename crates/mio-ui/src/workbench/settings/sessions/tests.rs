use gpui_kit::TestAppContext;
use gpui_kit::point;
use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use mio_core::{MacroApplyTarget, MacroExtractSource, SessionUrlScope};

use super::*;
use crate::workbench::tests::{
    Fixture, drain, fixture,
    settings::{acknowledge, open_settings, saved},
    tab,
};

fn select_index(settings: AnyWindowHandle, id: &'static str, target: usize, cx: &mut TestAppContext) {
    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            match id {
                "settings-sessions-url-scope-select" => {
                    this.sessions.rule_url_mode = target as u8;
                }
                "settings-sessions-action-kind-select" => {
                    this.sessions.draft_action_kind = match target {
                        1 => DraftActionKind::SetHeader,
                        2 => DraftActionKind::RewriteUrlParam,
                        3 => DraftActionKind::RunMacro,
                        4 => DraftActionKind::SetCookie,
                        5 => DraftActionKind::ReplaceRequestText,
                        _ => DraftActionKind::UseCookieJar,
                    };
                }
                _ => unreachable!(),
            }
            cx.notify();
        });
    })
    .unwrap();
    cx.run_until_parked();
}

fn scroll_sessions_top(settings: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.sessions.scroll.set_offset(gpui_kit::Point::default());
            cx.notify();
        });
        window.render_frame(cx);
    })
    .unwrap();
}

fn scroll_sessions_bottom(settings: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.sessions.scroll.scroll_to_bottom();
            cx.notify();
        });
        window.render_frame(cx);
    })
    .unwrap();
}

fn open_sessions_page(fixture: &Fixture, cx: &mut TestAppContext) -> AnyWindowHandle {
    let settings = open_settings(fixture, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-sessions-nav", cx);
        window.render_frame(cx);
        assert_eq!(window.find("settings-sessions-nav").selected(), Some(true));
        assert!(window.find("settings-sessions-rules-heading").visible());
        assert!(window.find("settings-cookie-jar-heading").visible());
        assert!(window.find("settings-macros-heading").visible());
        assert!(window.try_find("settings-sessions-tab-rules").is_none());
        assert!(window.find("settings-sessions-open-tracer").visible());
        assert!(window.find("settings-cookie-jar-open").visible());
        assert!(window.try_find("settings-sessions-rule-name").is_none());
    })
    .unwrap();
    cx.run_until_parked();
    settings
}

#[gpui_kit::test]
fn sessions_settings_page_crud_reordering_and_reopen_roundtrip(cx: &mut TestAppContext) {
    let mut fixture = fixture(cx, vec![tab("Sessions CRUD 与顺序测试")], 64);
    drain(&mut fixture.commands);
    let settings = open_sessions_page(&fixture, cx);

    // 1. Toggle Repeater Set-Cookie monitor & add a scoped Secure+HttpOnly Cookie Jar entry.
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("settings-cookie-jar-heading").visible());
        window.click("settings-cookie-jar-monitor-repeater", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws1 = saved(&mut fixture.commands);
    assert!(ws1.sessions.cookie_jar.monitor_repeater);
    acknowledge(&fixture, ws1, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-cookie-jar-monitor-scanner", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws1_scanner = saved(&mut fixture.commands);
    assert!(ws1_scanner.sessions.cookie_jar.monitor_repeater);
    assert!(ws1_scanner.sessions.cookie_jar.monitor_scanner);
    acknowledge(&fixture, ws1_scanner, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-cookie-jar-open", cx);
        window.render_frame(cx);
    })
    .unwrap();
    scroll_sessions_bottom(settings, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-cookie-domain", cx);
        window.input("corp.example.com", cx);
        window.click("settings-cookie-subdomains", cx);
        window.click("settings-cookie-path", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("/api", cx);
        window.click("settings-cookie-name", cx);
        window.input("corp_sid", cx);
        window.click("settings-cookie-value", cx);
        window.input("v_9001", cx);
        window.click("settings-cookie-secure", cx);
        window.click("settings-cookie-expires", cx);
        window.input("1999999000000", cx);
        window.render_frame(cx);
        window.click("settings-cookie-jar-save", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws2 = saved(&mut fixture.commands);
    assert_eq!(ws2.sessions.cookie_jar.entries.len(), 1);
    let ck = &ws2.sessions.cookie_jar.entries[0];
    assert_eq!(ck.domain, "corp.example.com");
    assert!(!ck.host_only);
    assert_eq!(ck.path, "/api");
    assert_eq!(ck.name, "corp_sid");
    assert_eq!(ck.value, "v_9001");
    assert!(ck.secure && ck.http_only);
    assert_eq!(ck.expires_unix_ms, Some(1_999_999_000_000));
    acknowledge(&fixture, ws2, cx);

    // Close cookie jar panel and open macro editor.
    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.sessions.cookie_jar_open = false;
            cx.notify();
        });
        window.render_frame(cx);
    })
    .unwrap();
    scroll_sessions_bottom(settings, cx);

    // 2. Create a 2-step Macro with Retry policy and parameter extraction -> Header target.
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-macro-add", cx);
        window.render_frame(cx);
    })
    .unwrap();
    scroll_sessions_bottom(settings, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-macro-name", cx);
        window.input("刷新令牌宏", cx);
        window.click("settings-macro-policy-retry", cx);
        window.render_frame(cx);
        window.click("settings-macro-retries", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("2", cx);
        window.click("settings-macro-step-url", cx);
        window.input("https://corp.example.com/csrf", cx);
        window.click("settings-macro-ext-name", cx);
        window.input("csrf", cx);
        window.click("settings-macro-ext-arg1", cx);
        window.input("token=\"", cx);
        window.click("settings-macro-ext-arg2", cx);
        window.input("\"", cx);
        window.click("settings-macro-tgt-hdr", cx);
        window.render_frame(cx);
        window.click("settings-macro-ext-target", cx);
        window.input("X-CSRF-Token", cx);
        for _ in 0..4 {
            if window.find("settings-macro-step-add").visible() {
                break;
            }
            window.scroll(
                "settings-sessions-page",
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-250.))),
                cx,
            );
            window.render_frame(cx);
        }
        window.click("settings-macro-step-add", cx);
        window.render_frame(cx);
        for _ in 0..4 {
            if window.find("settings-macro-step-method").visible() {
                break;
            }
            window.scroll(
                "settings-sessions-page",
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(250.))),
                cx,
            );
            window.render_frame(cx);
        }
        window.click("settings-macro-step-method", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("POST", cx);
        window.click("settings-macro-step-url", cx);
        window.input("https://corp.example.com/session", cx);
        for _ in 0..4 {
            if window.find("settings-macro-step-add").visible() {
                break;
            }
            window.scroll(
                "settings-sessions-page",
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-250.))),
                cx,
            );
            window.render_frame(cx);
        }
        window.click("settings-macro-step-add", cx);
        window.render_frame(cx);
        for _ in 0..4 {
            if window.find("settings-macro-save").visible() {
                break;
            }
            window.scroll(
                "settings-sessions-page",
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-350.))),
                cx,
            );
            window.render_frame(cx);
        }
        window.click("settings-macro-save", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws3 = saved(&mut fixture.commands);
    assert_eq!(ws3.sessions.macros.len(), 1);
    let saved_macro = &ws3.sessions.macros[0];
    assert_eq!(saved_macro.name, "刷新令牌宏");
    assert_eq!(
        saved_macro.failure_policy,
        MacroFailurePolicy::Retry { max_retries: 2 }
    );
    assert_eq!(saved_macro.steps.len(), 2);
    assert_eq!(
        saved_macro.steps[0].extractions[0].source,
        MacroExtractSource::Delimiter {
            start: "token=\"".into(),
            end: "\"".into(),
        }
    );
    assert_eq!(
        saved_macro.steps[0].extractions[0].target,
        MacroApplyTarget::Header {
            header_name: "X-CSRF-Token".into()
        }
    );
    let macro_id = saved_macro.id;
    acknowledge(&fixture, ws3, cx);

    // 3. Add two Session handling rules, then reorder and toggle enabled state.
    scroll_sessions_top(settings, cx);
    cx.update_window(settings, |_, window, cx| {
        window.click("settings-sessions-rule-add", cx);
        window.render_frame(cx);
        window.click("settings-sessions-rule-name", cx);
        window.input("规则一：静态头", cx);
    })
    .unwrap();
    select_index(settings, "settings-sessions-action-kind-select", 1, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-sessions-act-hdr-name", cx);
        window.input("X-Env", cx);
        window.click("settings-sessions-act-hdr-val", cx);
        window.input("staging", cx);
        window.click("settings-sessions-action-add", cx);
        window.render_frame(cx);
        window.click("settings-sessions-rule-save", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws4 = saved(&mut fixture.commands);
    assert_eq!(ws4.sessions.rules.len(), 1);
    acknowledge(&fixture, ws4, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-sessions-rule-add", cx);
        window.render_frame(cx);
        window.click("settings-sessions-rule-name", cx);
        window.input("规则二：宏与参数重写", cx);
        window.click("settings-sessions-tool-proxy", cx);
        window.click("settings-sessions-tool-scanner", cx);
        window.click("settings-sessions-tool-sequencer", cx);
        window.click("settings-sessions-tool-extensions", cx);
    })
    .unwrap();
    select_index(settings, "settings-sessions-url-scope-select", 2, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        let entity = settings_entity(window, cx).unwrap();
        let prefixes = entity.read(cx).sessions.rule_prefixes.clone();
        prefixes.update(cx, |s, cx| {
            s.set_value("https://corp.example.com/api/", window, cx)
        });
    })
    .unwrap();
    select_index(settings, "settings-sessions-action-kind-select", 3, cx);
    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| this.add_draft_rule_action(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    select_index(settings, "settings-sessions-action-kind-select", 2, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-sessions-act-param-name", cx);
        window.input("tenant", cx);
        window.click("settings-sessions-act-param-val", cx);
        window.input("cn", cx);
        window.click("settings-sessions-action-add", cx);
    })
    .unwrap();
    cx.run_until_parked();
    select_index(settings, "settings-sessions-action-kind-select", 4, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-sessions-act-cookie-name", cx);
        window.input("rule_ck", cx);
        window.click("settings-sessions-act-cookie-val", cx);
        window.input("ck_99", cx);
        window.click("settings-sessions-action-add", cx);
    })
    .unwrap();
    cx.run_until_parked();
    select_index(settings, "settings-sessions-action-kind-select", 5, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-sessions-act-replace-match", cx);
        window.input("OLD_TOK", cx);
        window.click("settings-sessions-act-replace-with", cx);
        window.input("NEW_TOK", cx);
        window.click("settings-sessions-act-replace-regex", cx);
        window.click("settings-sessions-action-add", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-sessions-rule-save", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws5 = saved(&mut fixture.commands);
    assert_eq!(ws5.sessions.rules.len(), 2);
    assert!(ws5.sessions.rules[1].tools.proxy);
    assert!(ws5.sessions.rules[1].tools.scanner);
    assert!(ws5.sessions.rules[1].tools.sequencer);
    assert!(ws5.sessions.rules[1].tools.extensions);
    assert_eq!(
        ws5.sessions.rules[1].condition.url_scope,
        SessionUrlScope::CustomPrefixes(vec!["https://corp.example.com/api/".into()])
    );
    assert!(ws5.sessions.rules[1].actions.iter().any(|a| matches!(
        a,
        SessionRuleAction::RunMacro { macro_id: id, .. } if *id == macro_id
    )));
    assert!(ws5.sessions.rules[1].actions.iter().any(|a| matches!(
        a,
        SessionRuleAction::SetCookie { name, value, .. } if name == "rule_ck" && value == "ck_99"
    )));
    assert!(ws5.sessions.rules[1].actions.iter().any(|a| matches!(
        a,
        SessionRuleAction::ReplaceRequestText { match_pattern, replace_with, regex }
            if match_pattern == "OLD_TOK" && replace_with == "NEW_TOK" && *regex
    )));
    acknowledge(&fixture, ws5, cx);

    // Move rule 2 (currently selected after creation) up to index 0, then toggle it disabled via inline checkbox.
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-sessions-rule-up", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws6 = saved(&mut fixture.commands);
    assert_eq!(ws6.sessions.rules[0].name, "规则二：宏与参数重写");
    assert_eq!(ws6.sessions.rules[1].name, "规则一：静态头");
    acknowledge(&fixture, ws6, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click(("settings-sessions-rule-enable", 0usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws7 = saved(&mut fixture.commands);
    assert!(!ws7.sessions.rules[0].enabled);
    acknowledge(&fixture, ws7.clone(), cx);

    // Close and reopen Settings -> verify full state persists in UI.
    cx.simulate_keystrokes(settings, "escape");
    let reopened = open_sessions_page(&fixture, cx);
    cx.update_window(reopened, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        assert_eq!(entity.read(cx).state.sessions, ws7.sessions);
    })
    .unwrap();
}

#[gpui_kit::test]
fn sessions_settings_preserves_ime_composition_and_handles_workspace_cas_rejection(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("Sessions IME 与 CAS 拒绝保留")], 64);
    drain(&mut fixture.commands);
    let initial_revision = cx.read(|cx| fixture.view.read(cx).workspace.revision);
    let settings = open_sessions_page(&fixture, cx);

    // 1. Active IME composition in rule_name blocks commit and preserves marked text.
    cx.update_window(settings, |_, window, cx| {
        window.click("settings-sessions-rule-add", cx);
        window.render_frame(cx);
        let entity = settings_entity(window, cx).unwrap();
        let input = entity.read(cx).sessions.rule_name.clone();
        input.update(cx, |s, cx| {
            s.replace_and_mark_text_in_range(None, "huihua", Some(6..6), window, cx)
        });
        window.render_frame(cx);
        window.click("settings-sessions-rule-save", cx);
        window.render_frame(cx);
        assert!(
            input.update(cx, |s, cx| s.marked_text_range(window, cx).is_some()),
            "IME composition must remain intact"
        );
        let err = window.find("settings-sessions-error");
        assert!(err.visible());
        assert!(err.label().unwrap().contains("输入法仍在组词"));
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        drain(&mut fixture.commands).is_empty(),
        "uncommitted IME composition must never emit SaveWorkspace"
    );

    // 2. Commit IME candidate text -> SaveWorkspace emitted with revision + 1.
    cx.update_window(settings, |_, window, cx| {
        window.click("settings-sessions-rule-name", cx);
        window.input("中文会话规则", cx);
        window.render_frame(cx);
        window.click("settings-sessions-rule-save", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let attempted = saved(&mut fixture.commands);
    assert_eq!(attempted.revision, initial_revision + 1);
    assert_eq!(attempted.sessions.rules.len(), 1);
    assert_eq!(attempted.sessions.rules[0].name, "中文会话规则");

    // 3. Backend rejects with CAS conflict (stored has advanced to initial_revision + 5).
    let mut stored_on_disk = cx.read(|cx| (*fixture.view.read(cx).workspace).clone());
    stored_on_disk.revision = initial_revision + 5;
    stored_on_disk.sessions = mio_core::SessionSettings::default();
    fixture
        .events
        .try_send(Event::WorkspaceRejected {
            project: fixture.project,
            revision: attempted.revision,
            stored: stored_on_disk.clone(),
            message: "工作区已变更；保留当前草稿，请重新同步".into(),
        })
        .unwrap();
    cx.run_until_parked();

    // User's session rule edit is preserved as dirty, base revision rebased to stored (initial + 5), and error shown.
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        let err = window.find("settings-sessions-error");
        assert!(err.visible());
        assert!(err.label().unwrap().contains("工作区已变更"));
    })
    .unwrap();
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(view.workspace_dirty && view.pending_workspace.is_none());
        assert_eq!(view.workspace.revision, initial_revision + 5);
        assert_eq!(view.workspace.sessions.rules.len(), 1);
        assert_eq!(view.workspace.sessions.rules[0].name, "中文会话规则");
    });

    // 4. Subsequent toggle rebases onto stored.revision + 1 (initial_revision + 6) and succeeds.
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-cookie-jar-monitor-intruder", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let retried = saved(&mut fixture.commands);
    assert_eq!(retried.revision, initial_revision + 6);
    assert_eq!(retried.sessions.rules.len(), 1);
    assert!(retried.sessions.cookie_jar.monitor_intruder);
    acknowledge(&fixture, retried, cx);
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(!view.workspace_dirty && view.pending_workspace.is_none());
    });
}

#[gpui_kit::test]
fn settings_search_scope_filter_project_json_roundtrip_and_macro_history_recorder(
    cx: &mut TestAppContext,
) {
    let mut initial = tab("Macro 历史与草稿录制测试");
    initial.draft.target = "https://auth.example.com".into();
    initial.draft.raw = b"POST /oauth/token HTTP/1.1\r\nHost: auth.example.com\r\nContent-Type: application/x-www-form-urlencoded\r\nX-Client: mio\r\nContent-Length: 19\r\n\r\ngrant_type=password".to_vec();
    let mut fixture = fixture(cx, vec![initial], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);

    // 1. Verify User/Project scope filter pills and keyword search auto-selection.
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("settings-repeater").selected(), Some(true));
        window.click("settings-filter-user", cx);
        window.render_frame(cx);
        assert!(window.try_find("settings-repeater").is_none());
        assert_eq!(window.find("settings-intruder").selected(), Some(true));
        window.click("settings-filter-all", cx);
        window.render_frame(cx);
        window.click("settings-search-input", cx);
        window.input("SOCKS", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-network-connections").selected(),
            Some(true)
        );
        assert!(window.try_find("settings-repeater").is_none());
        window.click("settings-search-input", cx);
        window.press(crate::kbd!("a"), cx);
        window.input("macro", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("settings-sessions-nav").selected(), Some(true));
        window.click("settings-search-input", cx);
        window.press(crate::kbd!("a"), cx);
        window.press("backspace", cx);
    })
    .unwrap();
    cx.run_until_parked();

    // 2. Record Macro step 1 from active Repeater draft, and step 2 from selected Proxy History flow.
    scroll_sessions_bottom(settings, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-macro-add", cx);
        window.render_frame(cx);
    })
    .unwrap();
    scroll_sessions_bottom(settings, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("sessions-macro-import-active", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-macro-step-method").value(),
            Some("POST")
        );
        assert_eq!(
            window.find("settings-macro-step-url").value(),
            Some("https://auth.example.com/oauth/token")
        );
        assert_eq!(
            window.find("settings-macro-step-body").value(),
            Some("grant_type=password")
        );
        window.click("settings-macro-step-add", cx);
    })
    .unwrap();
    cx.run_until_parked();

    let mut hist_flow = crate::workbench::tests::record("https://auth.example.com/me", "ok");
    hist_flow.request.method = "GET".into();
    hist_flow.request.url = "https://auth.example.com/me".into();
    hist_flow.request.headers = vec![
        mio_core::Header {
            name: "Host".into(),
            value: b"auth.example.com".to_vec(),
        },
        mio_core::Header {
            name: "X-Trace".into(),
            value: b"t-1".to_vec(),
        },
    ];
    let flow_id = hist_flow.summary.id;
    cx.update(|cx| {
        fixture.view.update(cx, |view, _| {
            view.selected_history = Some(HistoryEntryId::Live(flow_id));
            view.detail = Some(Box::new(hist_flow));
        });
    });
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("sessions-macro-import-active", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("settings-macro-step-method").value(),
            Some("GET")
        );
        assert_eq!(
            window.find("settings-macro-step-url").value(),
            Some("https://auth.example.com/me")
        );
        assert_eq!(
            window.find("settings-macro-step-headers").value(),
            Some("X-Trace: t-1")
        );
        window.click("settings-macro-step-add", cx);
        window.click("settings-macro-name", cx);
        window.input("双步登录录制宏", cx);
        window.render_frame(cx);
        for _ in 0..4 {
            if window.find("settings-macro-save").visible() {
                break;
            }
            window.scroll(
                "settings-sessions-page",
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-350.))),
                cx,
            );
            window.render_frame(cx);
        }
        window.click("settings-macro-save", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws_macro = saved(&mut fixture.commands);
    assert_eq!(ws_macro.sessions.macros.len(), 1);
    assert_eq!(ws_macro.sessions.macros[0].steps.len(), 2);
    assert_eq!(
        ws_macro.sessions.macros[0].steps[0].url,
        "https://auth.example.com/oauth/token"
    );
    assert_eq!(
        ws_macro.sessions.macros[0].steps[1].url,
        "https://auth.example.com/me"
    );
    acknowledge(&fixture, ws_macro.clone(), cx);

    // 3. Export project settings JSON to clipboard, reset sessions, then import back and verify CAS save.
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-manage-global", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        let pos = window.find_by_label("复制项目设置 JSON").bounds().center();
        window.pointer_down(pos, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(pos, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    let exported = cx
        .read(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
        .expect("exported project settings JSON");
    assert!(exported.contains("\"format\": \"mio.project_settings.v1\""));
    assert!(exported.contains("双步登录录制宏"));

    cx.update(|cx| {
        fixture.view.update(cx, |view, cx| {
            view.workspace.sessions = mio_core::SessionSettings::default();
            view.workspace.repeater_settings.request_timeout_secs = 45;
            view.save_workspace(cx);
        });
    });
    let ws_cleared = saved(&mut fixture.commands);
    acknowledge(&fixture, ws_cleared, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-manage-global", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        let pos = window.find_by_label("导入项目设置 JSON").bounds().center();
        window.pointer_down(pos, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(pos, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws_imported = saved(&mut fixture.commands);
    assert_eq!(ws_imported.sessions, ws_macro.sessions);
    assert_eq!(ws_imported.repeater_settings.request_timeout_secs, 30);
    acknowledge(&fixture, ws_imported, cx);
}

#[gpui_kit::test]
fn sessions_default_cookie_jar_rule_and_interactive_tracer(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("Sessions 默认规则与实时追踪器测试")], 64);
    drain(&mut fixture.commands);
    let settings = open_sessions_page(&fixture, cx);

    // 1. Toggle MioAI Cookie Jar monitor and default Cookie Jar rule checkbox -> persisted in workspace.
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-cookie-jar-monitor-ai", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws_ai = saved(&mut fixture.commands);
    assert!(ws_ai.sessions.cookie_jar.monitor_mio_ai);
    acknowledge(&fixture, ws_ai, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-sessions-rule-enable-default", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws_rule = saved(&mut fixture.commands);
    assert_eq!(ws_rule.sessions.rules.len(), 1);
    assert!(!ws_rule.sessions.rules[0].enabled);
    assert!(matches!(
        ws_rule.sessions.rules[0].actions.as_slice(),
        [SessionRuleAction::UseCookieJar {
            add_new_cookies: true,
            update_existing_cookies: true,
        }]
    ));
    acknowledge(&fixture, ws_rule, cx);

    // 3. Open Sessions Tracer, receive live trace, inspect detail + before/after preview, pause/resume, clear.
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-sessions-open-tracer", cx);
        window.render_frame(cx);
        assert!(window.find("settings-sessions-tracer-panel").visible());
        assert!(window.find("settings-sessions-tracer-empty").visible());
    })
    .unwrap();

    let trace1 = mio_core::SessionTraceEntry {
        id: Uuid::new_v4(),
        timestamp_ms: 1_700_000_000_123,
        tool: mio_core::SessionTool::Repeater,
        url: "https://app.example.com/api/profile".into(),
        rule_name: "使用 External Cookie Jar 中的 Cookie".into(),
        action_summary: "Use cookies from MIO's cookie jar".into(),
        detail: "注入 1 条 Cookie: sid=abc123".into(),
        before_preview: "GET /api/profile HTTP/1.1\nHost: app.example.com".into(),
        after_preview: "GET /api/profile HTTP/1.1\nHost: app.example.com\nCookie: sid=abc123"
            .into(),
    };
    fixture
        .events
        .try_send(Event::SessionTraceRecorded {
            project: fixture.project,
            entry: trace1,
        })
        .unwrap();
    cx.run_until_parked();

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        let row0 = window.find(("settings-sessions-tracer-row", 0usize));
        assert!(row0.visible());
        assert!(row0.label().unwrap().contains("https://app.example.com/api/profile"));
        window.click(("settings-sessions-tracer-row", 0usize), cx);
        window.render_frame(cx);
        let detail = window.find("settings-sessions-tracer-detail");
        assert!(detail.label().unwrap().contains("注入 1 条 Cookie: sid=abc123"));
        let before = window.find("settings-sessions-tracer-before");
        assert!(before.label().unwrap().contains("GET /api/profile HTTP/1.1"));
        let after = window.find("settings-sessions-tracer-after");
        assert!(after.label().unwrap().contains("Cookie: sid=abc123"));

        // Pause tracing.
        window.click("settings-sessions-tracer-pause", cx);
        window.render_frame(cx);
        assert!(window.find("settings-sessions-tracer-paused-badge").visible());
    })
    .unwrap();
    cx.run_until_parked();

    // Trace sent while paused is ignored.
    let ignored_trace = mio_core::SessionTraceEntry {
        id: Uuid::new_v4(),
        timestamp_ms: 1_700_000_001_000,
        tool: mio_core::SessionTool::Proxy,
        url: "https://app.example.com/ignored".into(),
        rule_name: "Rule".into(),
        action_summary: "Ignored".into(),
        detail: "Paused".into(),
        before_preview: String::new(),
        after_preview: String::new(),
    };
    fixture
        .events
        .try_send(Event::SessionTraceRecorded {
            project: fixture.project,
            entry: ignored_trace,
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("settings-sessions-tracer-row", 1usize)).is_none());
        // Resume tracing.
        window.click("settings-sessions-tracer-pause", cx);
        window.render_frame(cx);
        assert!(window.try_find("settings-sessions-tracer-paused-badge").is_none());
    })
    .unwrap();
    cx.run_until_parked();

    let trace2 = mio_core::SessionTraceEntry {
        id: Uuid::new_v4(),
        timestamp_ms: 1_700_000_002_456,
        tool: mio_core::SessionTool::Intruder,
        url: "https://app.example.com/api/items".into(),
        rule_name: "Rule 2".into(),
        action_summary: "SetHeader(X-Auth: ok)".into(),
        detail: "附加认证头".into(),
        before_preview: "GET /api/items HTTP/1.1".into(),
        after_preview: "GET /api/items HTTP/1.1\nX-Auth: ok".into(),
    };
    fixture
        .events
        .try_send(Event::SessionTraceRecorded {
            project: fixture.project,
            entry: trace2,
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find(("settings-sessions-tracer-row", 1usize)).visible());
        // Clear traces.
        window.click("settings-sessions-tracer-clear", cx);
        window.render_frame(cx);
        assert!(window.try_find(("settings-sessions-tracer-row", 0usize)).is_none());
        assert!(window.find("settings-sessions-tracer-empty").visible());
    })
    .unwrap();
}
