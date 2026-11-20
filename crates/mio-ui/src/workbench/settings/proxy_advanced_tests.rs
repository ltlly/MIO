use gpui_kit::test::TestWindowExt;
use gpui_kit::{TestAppContext, point};
use mio_core::{
    ProxyMessageSettings, SslPassThroughRule, WsMatchAndReplaceRule,
    extract_ssl_pass_through_rules, extract_ws_match_and_replace_rules,
    replace_ssl_pass_through_rules, replace_ws_match_and_replace_rules,
};

use super::*;
use crate::workbench::tests::{
    Fixture, drain, fixture,
    settings::{acknowledge, open_settings, saved},
};

fn proxy_rule_settings(cx: &mut TestAppContext) -> (Fixture, AnyWindowHandle) {
    let mut fixture = fixture(cx, Vec::new(), 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);
    proxy_rule_click(settings, "settings-proxy", cx);
    (fixture, settings)
}

fn proxy_rule_click(
    window: AnyWindowHandle,
    id: impl Into<gpui_kit::ElementId>,
    cx: &mut TestAppContext,
) {
    let id = id.into();
    ensure_visible(window, id.clone(), cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn visible_scroll_anchor(window: &gpui_kit::Window) -> &'static str {
    for anchor in [
        "proxy-listeners-heading",
        "proxy-request-rules-heading",
        "proxy-response-rules-heading",
        "proxy-ws-rules-heading",
        "proxy-miscellaneous-heading",
        "proxy-http-match-replace-heading",
        "proxy-ssl-passthrough-heading",
        "proxy-ws-match-replace-heading",
        "proxy-response-modification-heading",
    ] {
        if window.try_find(anchor).is_some_and(|n| n.visible()) {
            return anchor;
        }
    }
    "proxy-listeners-heading"
}

fn ensure_visible(
    settings: AnyWindowHandle,
    id: impl Into<gpui_kit::ElementId>,
    cx: &mut TestAppContext,
) {
    let id = id.into();
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        for step in [
            -350., -350., -350., -350., -350., -350., -350., -350., 500., 500., 500., 500., 500.,
        ] {
            if window
                .try_find(id.clone())
                .is_some_and(|el| el.visible())
            {
                return;
            }
            let anchor = visible_scroll_anchor(window);
            window.scroll(
                anchor,
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(step))),
                cx,
            );
            window.render_frame(cx);
        }
    })
    .unwrap();
    cx.run_until_parked();
}

fn scroll_to_proxy_advanced(settings: AnyWindowHandle, cx: &mut TestAppContext) {
    ensure_visible(settings, "proxy-ssl-passthrough-heading", cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("proxy-ssl-passthrough-heading").visible());
    })
    .unwrap();
    ensure_visible(settings, "proxy-ws-match-replace-heading", cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("proxy-ws-match-replace-heading").visible());
    })
    .unwrap();
}

fn scroll_to_heading(
    settings: AnyWindowHandle,
    heading_id: &'static str,
    _delta_y: f32,
    cx: &mut TestAppContext,
) {
    ensure_visible(settings, heading_id, cx);
}

fn select_dropdown_down(
    settings: AnyWindowHandle,
    id: &'static str,
    steps: usize,
    cx: &mut TestAppContext,
) {
    proxy_rule_click(settings, id, cx);
    let open = cx
        .update_window(settings, |_, window, cx| {
            window.render_frame(cx);
            window.find(id).expanded() == Some(true)
        })
        .unwrap();
    if !open {
        cx.simulate_keystrokes(settings, "enter");
        cx.run_until_parked();
    }
    cx.simulate_keystrokes(settings, &format!("{}enter", "down ".repeat(steps)));
    cx.run_until_parked();
}

fn input_value(settings: AnyWindowHandle, id: &'static str, text: &str, cx: &mut TestAppContext) {
    ensure_visible(settings, id, cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
        window.press(crate::kbd!("a"), cx);
        window.input(text, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn proxy_advanced_rules_add_toggle_edit_remove_and_save_persist(cx: &mut TestAppContext) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    scroll_to_heading(settings, "proxy-http-match-replace-heading", -900., cx);

    proxy_rule_click(settings, "proxy-http-mr-add", cx);
    select_dropdown_down(settings, "proxy-http-mr-target-select", 2, cx);
    input_value(settings, "proxy-http-mr-match-input", "Server: Apache", cx);
    input_value(settings, "proxy-http-mr-replace-input", "Server: MIO", cx);
    input_value(settings, "proxy-http-mr-comment-input", "mask server", cx);
    proxy_rule_click(settings, "proxy-http-mr-submit", cx);
    let saved_http = saved(&mut fixture.commands);
    assert_eq!(saved_http.match_and_replace_rules.len(), 1);
    assert_eq!(
        saved_http.match_and_replace_rules[0].item_type,
        mio_core::MatchReplaceTarget::ResponseHeader
    );
    assert_eq!(
        saved_http.match_and_replace_rules[0].replace_with,
        "Server: MIO"
    );
    acknowledge(&fixture, saved_http, cx);

    scroll_to_proxy_advanced(settings, cx);

    proxy_rule_click(settings, "proxy-ssl-pt-add", cx);
    input_value(settings, "proxy-ssl-pt-host-input", "*.pin.example", cx);
    input_value(settings, "proxy-ssl-pt-port-input", "8443", cx);
    proxy_rule_click(settings, "proxy-ssl-pt-submit", cx);
    let first = saved(&mut fixture.commands);
    let ssl_rules = extract_ssl_pass_through_rules(&first.match_and_replace_rules);
    assert_eq!(ssl_rules.len(), 1);
    let ssl_rule = ssl_rules[0].clone();
    assert_eq!(ssl_rule.host_pattern, "*.pin.example");
    assert_eq!(ssl_rule.port, Some(8443));
    assert!(ssl_rule.enabled && !ssl_rule.is_regex);
    acknowledge(&fixture, first.clone(), cx);

    proxy_rule_click(settings, "proxy-ws-mr-add", cx);
    select_dropdown_down(settings, "proxy-ws-mr-direction-select", 2, cx);
    input_value(settings, "proxy-ws-mr-match-input", "otp=([0-9]{6})", cx);
    input_value(settings, "proxy-ws-mr-replace-input", "otp=[hidden:$1]", cx);
    input_value(settings, "proxy-ws-mr-comment-input", "mask otp", cx);
    proxy_rule_click(settings, "proxy-ws-mr-regex", cx);
    proxy_rule_click(settings, "proxy-ws-mr-submit", cx);
    let second = saved(&mut fixture.commands);
    let ws_rules = extract_ws_match_and_replace_rules(&second.match_and_replace_rules);
    assert_eq!(ws_rules.len(), 1);
    let ws_rule = ws_rules[0].clone();
    assert_eq!(
        ws_rule.direction,
        mio_core::WsReplaceDirection::ServerToClient
    );
    assert!(ws_rule.is_regex);
    assert_eq!(ws_rule.comment, "mask otp");
    acknowledge(&fixture, second.clone(), cx);

    proxy_rule_click(settings, "proxy-ws-mr-add", cx);
    input_value(settings, "proxy-ws-mr-match-input", "ping", cx);
    input_value(settings, "proxy-ws-mr-replace-input", "pong", cx);
    proxy_rule_click(settings, "proxy-ws-mr-submit", cx);
    let second_b = saved(&mut fixture.commands);
    assert_eq!(
        extract_ws_match_and_replace_rules(&second_b.match_and_replace_rules).len(),
        2
    );
    acknowledge(&fixture, second_b, cx);

    proxy_rule_click(settings, ("proxy-ws-mr-row", 1usize), cx);
    proxy_rule_click(settings, "proxy-ws-mr-up", cx);
    let moved_up = saved(&mut fixture.commands);
    assert_eq!(
        extract_ws_match_and_replace_rules(&moved_up.match_and_replace_rules)[0].match_pattern,
        "ping"
    );
    acknowledge(&fixture, moved_up, cx);

    proxy_rule_click(settings, "proxy-resp-mod-unhide-hidden", cx);
    let third = saved(&mut fixture.commands);
    assert_eq!(
        third.proxy_messages.response_modification,
        ProxyResponseModificationSettings {
            unhide_hidden_form_fields: true,
            ..Default::default()
        }
    );
    acknowledge(&fixture, third.clone(), cx);

    proxy_rule_click(settings, ("proxy-ssl-pt-toggle", 0usize), cx);
    let disabled = saved(&mut fixture.commands);
    assert!(!extract_ssl_pass_through_rules(&disabled.match_and_replace_rules)[0].enabled);
    acknowledge(&fixture, disabled.clone(), cx);

    proxy_rule_click(settings, ("proxy-ssl-pt-row", 0usize), cx);
    proxy_rule_click(settings, "proxy-ssl-pt-edit", cx);
    input_value(settings, "proxy-ssl-pt-host-input", "pinned.local", cx);
    proxy_rule_click(settings, "proxy-ssl-pt-submit", cx);
    let edited = saved(&mut fixture.commands);
    let edited_ssl = extract_ssl_pass_through_rules(&edited.match_and_replace_rules);
    assert_eq!(edited_ssl.len(), 1);
    assert_eq!(edited_ssl[0].id, ssl_rule.id);
    assert_eq!(edited_ssl[0].host_pattern, "pinned.local");
    acknowledge(&fixture, edited.clone(), cx);

    proxy_rule_click(settings, ("proxy-ws-mr-row", 0usize), cx);
    proxy_rule_click(settings, "proxy-ws-mr-remove", cx);
    let mid_removed = saved(&mut fixture.commands);
    acknowledge(&fixture, mid_removed, cx);
    proxy_rule_click(settings, ("proxy-ws-mr-row", 0usize), cx);
    proxy_rule_click(settings, "proxy-ws-mr-remove", cx);
    let removed = saved(&mut fixture.commands);
    assert!(extract_ws_match_and_replace_rules(&removed.match_and_replace_rules).is_empty());
    acknowledge(&fixture, removed.clone(), cx);

    cx.simulate_keystrokes(settings, crate::kbd!("w"));
    cx.run_until_parked();
    let reopened = open_settings(&fixture, cx);
    proxy_rule_click(reopened, "settings-proxy", cx);
    scroll_to_proxy_advanced(reopened, cx);
    let latest = cx.read(|cx| fixture.view.read(cx).workspace.clone());
    assert_eq!(latest.proxy_messages, removed.proxy_messages);
    assert_eq!(
        extract_ssl_pass_through_rules(&latest.match_and_replace_rules)[0].host_pattern,
        "pinned.local"
    );
    assert!(
        latest
            .proxy_messages
            .response_modification
            .unhide_hidden_form_fields
    );
}

#[gpui_kit::test]
fn proxy_advanced_invalid_regex_preserves_inputs_and_sends_no_save(cx: &mut TestAppContext) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    scroll_to_proxy_advanced(settings, cx);
    proxy_rule_click(settings, "proxy-ws-mr-add", cx);
    input_value(settings, "proxy-ws-mr-match-input", "(", cx);
    proxy_rule_click(settings, "proxy-ws-mr-regex", cx);
    proxy_rule_click(settings, "proxy-ws-mr-submit", cx);
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        let error = window.find("proxy-ws-mr-error");
        assert!(error.visible());
        assert!(
            error
                .label()
                .unwrap()
                .contains("正则表达式无效或超过 256 KiB")
        );
        assert_eq!(window.find("proxy-ws-mr-match-input").value(), Some("("));
    })
    .unwrap();
    assert!(drain(&mut fixture.commands).is_empty());
    cx.read(|cx| {
        let view = fixture.view.read(cx);
        assert!(
            extract_ws_match_and_replace_rules(&view.workspace.match_and_replace_rules).is_empty()
        );
    });
}

#[test]
fn proxy_advanced_default_fields_preserve_legacy_json_and_security_boundaries() {
    let parsed = ProxyMessageSettings {
        unpack_compressed_requests: true,
        unpack_compressed_responses: false,
        ..Default::default()
    };
    assert!(parsed.response_modification.is_default());

    let mut rules = Vec::new();
    replace_ssl_pass_through_rules(
        &mut rules,
        &[SslPassThroughRule {
            id: mio_core::Uuid::new_v4(),
            enabled: true,
            host_pattern: "127.0.0.1".into(),
            port: Some(8443),
            is_regex: false,
        }],
    );
    replace_ws_match_and_replace_rules(
        &mut rules,
        &[WsMatchAndReplaceRule {
            id: mio_core::Uuid::new_v4(),
            enabled: true,
            direction: mio_core::WsReplaceDirection::Both,
            match_pattern: "a".into(),
            replace_with: "b".into(),
            is_regex: false,
            comment: "note".into(),
        }],
    );
    assert_eq!(extract_ssl_pass_through_rules(&rules).len(), 1);
    assert_eq!(extract_ws_match_and_replace_rules(&rules).len(), 1);
}

#[gpui_kit::test]
fn proxy_advanced_script_mode_test_panel_param_rules_and_nine_response_modifications_work(
    cx: &mut TestAppContext,
) {
    let (mut fixture, settings) = proxy_rule_settings(cx);
    scroll_to_heading(settings, "proxy-http-match-replace-heading", -900., cx);
    proxy_rule_click(settings, "proxy-http-mr-in-scope-only", cx);
    let in_scope = saved(&mut fixture.commands);
    assert!(in_scope.proxy_messages.match_replace_in_scope_only);
    acknowledge(&fixture, in_scope, cx);

    proxy_rule_click(settings, "proxy-http-mr-add", cx);
    let dialog = *cx
        .windows()
        .iter()
        .find(|&&w| w != fixture.window && w != settings)
        .expect("HTTP match/replace dialog must open");
    proxy_rule_click(dialog, "proxy-http-mr-mode-script", cx);
    proxy_rule_click(dialog, "proxy-http-mr-load-template", cx);
    input_value(dialog, "proxy-http-mr-script-name-input", "Sign request header", cx);
    input_value(dialog, "proxy-http-mr-comment-input", "mio_script sign", cx);
    proxy_rule_click(dialog, "proxy-http-mr-test-btn", cx);
    cx.update_window(dialog, |root, window, cx| {
        window.render_frame(cx);
        let root = root.downcast::<gpui_kit::base::Root>().unwrap();
        let editor = root
            .read(cx)
            .view()
            .clone()
            .downcast::<crate::workbench::tools_extra::HttpMatchReplaceEditor>()
            .unwrap();
        let modified = editor.read(cx).test_modified.read(cx).value();
        assert!(modified.contains("X-Forwarded-For: 127.0.0.1"), "modified: {modified}");
    })
    .unwrap();
    proxy_rule_click(dialog, "proxy-http-mr-submit", cx);
    let saved_script = saved(&mut fixture.commands);
    assert_eq!(saved_script.match_and_replace_rules.len(), 1);
    assert_eq!(
        saved_script.match_and_replace_rules[0].item_type,
        mio_core::MatchReplaceTarget::RequestScript
    );
    assert_eq!(
        saved_script.match_and_replace_rules[0].script_name,
        "Sign request header"
    );
    acknowledge(&fixture, saved_script, cx);

    for (steps, expected_target) in [
        (4, mio_core::MatchReplaceTarget::RequestParamName),
        (5, mio_core::MatchReplaceTarget::RequestParamValue),
        (6, mio_core::MatchReplaceTarget::RequestFirstLine),
    ] {
        proxy_rule_click(settings, "proxy-http-mr-add", cx);
        let dlg = *cx
            .windows()
            .iter()
            .find(|&&w| w != fixture.window && w != settings)
            .unwrap();
        select_dropdown_down(dlg, "proxy-http-mr-type-select", steps, cx);
        input_value(dlg, "proxy-http-mr-match-input", "token", cx);
        input_value(dlg, "proxy-http-mr-replace-input", "masked", cx);
        proxy_rule_click(dlg, "proxy-http-mr-test-btn", cx);
        proxy_rule_click(dlg, "proxy-http-mr-submit", cx);
        let saved_rule = saved(&mut fixture.commands);
        assert_eq!(
            saved_rule.match_and_replace_rules.last().unwrap().item_type,
            expected_target
        );
        acknowledge(&fixture, saved_rule, cx);
    }

    ensure_visible(settings, "proxy-response-modification-heading", cx);
    for id in [
        "proxy-resp-mod-unhide-hidden",
        "proxy-resp-mod-highlight-unhidden",
        "proxy-resp-mod-enable-disabled",
        "proxy-resp-mod-remove-length-limits",
        "proxy-resp-mod-remove-js-validation",
        "proxy-resp-mod-remove-all-js",
        "proxy-resp-mod-remove-object-tags",
        "proxy-resp-mod-convert-https-links",
        "proxy-resp-mod-remove-cookie-secure",
    ] {
        proxy_rule_click(settings, id, cx);
        let ws = saved(&mut fixture.commands);
        acknowledge(&fixture, ws, cx);
    }
    cx.read(|cx| {
        let rm = fixture.view.read(cx).workspace.proxy_messages.response_modification;
        assert!(
            rm.unhide_hidden_form_fields
                && rm.highlight_unhidden_fields
                && rm.enable_disabled_form_fields
                && rm.remove_input_field_length_limits
                && rm.remove_javascript_form_validation
                && rm.remove_all_javascript
                && rm.remove_object_tags
                && rm.convert_https_links_to_http
                && rm.remove_secure_flag_from_cookies
        );
    });
}
