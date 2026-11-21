use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;

use super::*;
use crate::workbench::tests::settings::{acknowledge, saved};
use crate::workbench::tests::{drain, fixture, tab};

#[gpui_kit::test]
fn match_and_replace_two_sections_collapsible_editors_and_ws_rules_work(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, vec![tab("MatchReplace")], 64);
    drain(&mut f.commands);

    cx.update_window(f.window, |_, window, cx| {
        window.click("tool-proxy", cx);
        window.click("proxy-match-replace", cx);
        window.render_frame(cx);

        assert!(window.find("mr-http-heading").visible());
        assert!(window.find("mr-ws-heading").visible());
        assert!(window.find("proxy-http-mr-in-scope-only").visible());
        assert!(window.find("proxy-ws-mr-in-scope-only").visible());
        assert!(window.find("mr-side-add").visible());
        assert!(window.find("ws-mr-add").visible());
        assert!(window.try_find("mr-target-select").is_none());
        assert!(window.try_find("ws-mr-direction-select").is_none());
        assert_eq!(window.find("mr-edit").disabled(), Some(true));
        assert_eq!(window.find("mr-remove").disabled(), Some(true));
        assert_eq!(window.find("ws-mr-edit").disabled(), Some(true));
        assert_eq!(window.find("ws-mr-remove").disabled(), Some(true));

        window.click("proxy-http-mr-in-scope-only", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws = saved(&mut f.commands);
    assert!(ws.proxy_messages.match_replace_in_scope_only);
    acknowledge(&f, ws, cx);

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("proxy-ws-mr-in-scope-only", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws = saved(&mut f.commands);
    assert!(ws.proxy_messages.ws_match_replace_in_scope_only);
    acknowledge(&f, ws, cx);

    // Open HTTP editor via Add, submit inline, verify dialog + inline editor close cleanly
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("mr-side-add", cx);
        window.render_frame(cx);
        assert!(window.find("mr-target-select").visible());
        window.click("mr-match-input", cx);
        window.input("User-Agent: old", cx);
        window.click("mr-replace-input", cx);
        window.input("User-Agent: mio", cx);
        window.click("mr-comment-input", cx);
        window.input("swap ua", cx);
        window.click("mr-add-button", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws = saved(&mut f.commands);
    assert_eq!(ws.match_and_replace_rules.len(), 1);
    assert_eq!(ws.match_and_replace_rules[0].match_pattern, "User-Agent: old");
    assert_eq!(ws.match_and_replace_rules[0].replace_with, "User-Agent: mio");
    assert_eq!(ws.match_and_replace_rules[0].comment, "swap ua");
    acknowledge(&f, ws, cx);
    assert_eq!(cx.windows().len(), 1);

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("mr-target-select").is_none());
        assert!(window.find(("mr-row", 0usize)).visible());
        window.click(("mr-toggle", 0usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws = saved(&mut f.commands);
    assert!(!ws.match_and_replace_rules[0].enabled);
    acknowledge(&f, ws, cx);

    // Add two WebSocket match-and-replace rules, reorder, toggle, and remove
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("ws-mr-add", cx);
        window.render_frame(cx);
        assert!(window.find("ws-mr-direction-select").visible());
        window.click("ws-mr-match-input", cx);
        window.input("token=([0-9]+)", cx);
        window.click("ws-mr-replace-input", cx);
        window.input("token=masked", cx);
        window.click("ws-mr-comment-input", cx);
        window.input("mask token", cx);
        window.click("ws-mr-regex-toggle", cx);
        window.click("ws-mr-submit", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws = saved(&mut f.commands);
    assert_eq!(ws.match_and_replace_rules.len(), 1);
    let ws_rules = &ws.ws_match_and_replace_rules;
    assert_eq!(ws_rules.len(), 1);
    assert!(ws_rules[0].is_regex);
    assert_eq!(ws_rules[0].comment, "mask token");
    acknowledge(&f, ws, cx);

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("ws-mr-direction-select").is_none());
        window.click("ws-mr-add", cx);
        window.render_frame(cx);
        window.click("ws-mr-match-input", cx);
        window.input("ping", cx);
        window.click("ws-mr-replace-input", cx);
        window.input("pong", cx);
        window.click("ws-mr-submit", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws = saved(&mut f.commands);
    assert_eq!(ws.ws_match_and_replace_rules.len(), 2);
    acknowledge(&f, ws, cx);

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(("ws-mr-row", 1usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("ws-mr-up").disabled(), None);
        window.click("ws-mr-up", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws = saved(&mut f.commands);
    assert_eq!(ws.ws_match_and_replace_rules[0].match_pattern, "ping");
    acknowledge(&f, ws, cx);

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(("ws-mr-toggle", 0usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws = saved(&mut f.commands);
    assert!(!ws.ws_match_and_replace_rules[0].enabled);
    acknowledge(&f, ws, cx);

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(("ws-mr-row", 0usize), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("ws-mr-remove", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws = saved(&mut f.commands);
    let ws_rules = &ws.ws_match_and_replace_rules;
    assert_eq!(ws_rules.len(), 1);
    assert_eq!(ws_rules[0].match_pattern, "token=([0-9]+)");
    acknowledge(&f, ws, cx);
}
