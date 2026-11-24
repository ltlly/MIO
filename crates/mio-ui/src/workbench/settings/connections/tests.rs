use gpui_kit::component::IndexPath;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{ScrollDelta, TestAppContext, point};
use mio_core::{PlatformAuthType, UpstreamAuthType, UpstreamProxyProtocol};

use super::*;
use crate::workbench::tests::{
    drain, fixture,
    settings::{acknowledge, open_settings, saved},
    tab,
};

#[gpui_kit::test]
fn network_connections_upstream_rules_socks_timeouts_and_hostname_resolution_work(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("Connections 测试")], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-network-connections", cx);
        window.render_frame(cx);
        assert!(window.find("settings-upstream-heading").visible());
        assert!(window.find("settings-socks-heading").visible());
        assert!(window.find("settings-socks-configure").visible());
        assert!(window.find("settings-socks-bypass-input").visible());
        window.scroll(
            "settings-connections-page",
            ScrollDelta::Pixels(point(px(0.), px(-600.))),
            cx,
        );
        window.render_frame(cx);
        assert!(window.find("settings-timeouts-heading").visible());
        assert!(window.find("settings-hostname-heading").visible());
        window.scroll(
            "settings-connections-page",
            ScrollDelta::Pixels(point(px(0.), px(600.))),
            cx,
        );
        window.render_frame(cx);
        window.click("settings-upstream-add", cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.connections
                .rule_protocol
                .update(cx, |s, cx| s.set_selected_index(Some(IndexPath::new(1)), window, cx));
            this.connections
                .rule_auth
                .update(cx, |s, cx| s.set_selected_index(Some(IndexPath::new(1)), window, cx));
            this.connections
                .rule_destination
                .update(cx, |s, cx| s.set_value("*.internal.example", window, cx));
            this.connections
                .rule_host
                .update(cx, |s, cx| s.set_value("proxy1.example", window, cx));
            this.connections
                .rule_port
                .update(cx, |s, cx| s.set_value("8443", window, cx));
            this.connections
                .rule_username
                .update(cx, |s, cx| s.set_value("bob", window, cx));
            this.connections
                .rule_password
                .update(cx, |s, cx| s.set_value("pw1", window, cx));
            this.connections
                .rule_description
                .update(cx, |s, cx| s.set_value("corp proxy", window, cx));
            this.save_upstream_rule(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let ws1 = saved(&mut fixture.commands);
    assert_eq!(ws1.upstream_proxy.rules.len(), 1);
    assert_eq!(
        ws1.upstream_proxy.rules[0].destination_host,
        "*.internal.example"
    );
    assert_eq!(
        ws1.upstream_proxy.rules[0].protocol,
        UpstreamProxyProtocol::Https
    );
    assert_eq!(
        ws1.upstream_proxy.rules[0].auth_type,
        UpstreamAuthType::Basic
    );
    assert_eq!(ws1.upstream_proxy.rules[0].username, "bob");
    assert_eq!(ws1.upstream_proxy.rules[0].description, "corp proxy");
    acknowledge(&fixture, ws1, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-upstream-add", cx);
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.connections
                .rule_destination
                .update(cx, |s, cx| s.set_value("*", window, cx));
            this.connections
                .rule_host
                .update(cx, |s, cx| s.set_value("proxy2.example", window, cx));
            this.connections
                .rule_port
                .update(cx, |s, cx| s.set_value("8080", window, cx));
            this.save_upstream_rule(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let ws2 = saved(&mut fixture.commands);
    assert_eq!(ws2.upstream_proxy.rules.len(), 2);
    acknowledge(&fixture, ws2, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-upstream-up", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws3 = saved(&mut fixture.commands);
    assert_eq!(ws3.upstream_proxy.rules[0].host, "proxy2.example");
    assert_eq!(ws3.upstream_proxy.rules[1].host, "proxy1.example");
    acknowledge(&fixture, ws3, cx);

    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.connections.socks_enabled = true;
            this.connections.socks_dns_remote = false;
            this.upstream_host
                .update(cx, |s, cx| s.set_value("127.0.0.1", window, cx));
            this.upstream_port
                .update(cx, |s, cx| s.set_value("1080", window, cx));
            this.save_socks(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let ws4 = saved(&mut fixture.commands);
    assert!(ws4.upstream_proxy.socks.use_proxy);
    assert!(!ws4.upstream_proxy.socks.dns_over_socks);
    assert_eq!(ws4.upstream_proxy.socks.host, "127.0.0.1");
    assert_eq!(ws4.upstream_proxy.socks.port, 1080);
    acknowledge(&fixture, ws4, cx);

    // Verify SOCKS bypass list add & clear persist to workspace.upstream_proxy
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-socks-bypass-input", cx);
        window.input("localhost, *.internal", cx);
        window.click("settings-socks-bypass-add", cx);
        window.render_frame(cx);
        assert!(window.find(("settings-socks-bypass-row", 0usize)).visible());
        assert!(window.find(("settings-socks-bypass-row", 1usize)).visible());
    })
    .unwrap();
    cx.run_until_parked();
    let ws_bypass = saved(&mut fixture.commands);
    assert_eq!(
        ws_bypass.upstream_proxy.socks.bypass_hosts,
        vec!["localhost".to_string(), "*.internal".to_string()]
    );
    acknowledge(&fixture, ws_bypass, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-socks-bypass-clear", cx);
        window.render_frame(cx);
        assert!(
            window
                .try_find(("settings-socks-bypass-row", 0usize))
                .is_none()
        );
    })
    .unwrap();
    cx.run_until_parked();
    let ws_bypass_clear = saved(&mut fixture.commands);
    assert!(ws_bypass_clear.upstream_proxy.socks.bypass_hosts.is_empty());
    acknowledge(&fixture, ws_bypass_clear, cx);

    // Verify project-only override toggles persist
    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-upstream-override", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws_up_ovr = saved(&mut fixture.commands);
    assert!(ws_up_ovr.upstream_proxy.override_project_only);
    acknowledge(&fixture, ws_up_ovr, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-socks-override", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws_socks_ovr = saved(&mut fixture.commands);
    assert!(ws_socks_ovr.upstream_proxy.socks.override_project_only);
    acknowledge(&fixture, ws_socks_ovr, cx);

    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.connections
                .normal_timeout_input
                .update(cx, |s, cx| s.set_value("60", window, cx));
            this.connections
                .open_ended_timeout_input
                .update(cx, |s, cx| s.set_value("120", window, cx));
            this.save_network_timeouts(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let ws5 = saved(&mut fixture.commands);
    assert_eq!(ws5.repeater_settings.request_timeout_secs, 60);
    assert_eq!(ws5.upstream_proxy.open_ended_response_timeout_secs, 120);
    acknowledge(&fixture, ws5, cx);

    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.open_hostname_editor(true, window, cx);
            this.connections
                .dns_hostname_input
                .update(cx, |s, cx| s.set_value("api.corp.local", window, cx));
            this.connections
                .dns_ip_input
                .update(cx, |s, cx| s.set_value("10.0.0.42", window, cx));
            this.save_hostname_rule(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let ws6 = saved(&mut fixture.commands);
    assert_eq!(ws6.upstream_proxy.hostname_resolutions.len(), 1);
    assert_eq!(
        ws6.upstream_proxy.hostname_resolutions[0].hostname,
        "api.corp.local"
    );
    assert_eq!(
        ws6.upstream_proxy.hostname_resolutions[0].ip_address,
        "10.0.0.42"
    );
    acknowledge(&fixture, ws6, cx);
}

#[gpui_kit::test]
fn network_connections_ntlm_upstream_and_platform_auth_use_first_class_fields(
    cx: &mut TestAppContext,
) {
    let mut fixture = fixture(cx, vec![tab("Connections NTLM")], 64);
    drain(&mut fixture.commands);
    let settings = open_settings(&fixture, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-network-connections", cx);
        window.render_frame(cx);
        window.click("settings-upstream-add", cx);
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.connections
                .rule_auth
                .update(cx, |s, cx| s.set_selected_index(Some(IndexPath::new(4)), window, cx));
            this.connections
                .rule_destination
                .update(cx, |s, cx| s.set_value("*.lab.example.invalid", window, cx));
            this.connections
                .rule_host
                .update(cx, |s, cx| s.set_value("127.0.0.1", window, cx));
            this.connections
                .rule_port
                .update(cx, |s, cx| s.set_value("3128", window, cx));
            this.connections
                .rule_username
                .update(cx, |s, cx| s.set_value("alice", window, cx));
            this.connections
                .rule_password
                .update(cx, |s, cx| s.set_value("pw", window, cx));
            this.connections
                .rule_domain
                .update(cx, |s, cx| s.set_value("LAB", window, cx));
            this.connections
                .rule_workstation
                .update(cx, |s, cx| s.set_value("WKSTN1", window, cx));
            this.connections
                .rule_description
                .update(cx, |s, cx| s.set_value("ntlm proxy", window, cx));
            this.save_upstream_rule(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let ws1 = saved(&mut fixture.commands);
    assert_eq!(ws1.upstream_proxy.rules.len(), 1);
    let r0 = &ws1.upstream_proxy.rules[0];
    assert_eq!(r0.auth_type, UpstreamAuthType::NtlmV2);
    assert_eq!(r0.domain, "LAB");
    assert_eq!(r0.workstation, "WKSTN1");
    assert_eq!(r0.description, "ntlm proxy");
    acknowledge(&fixture, ws1, cx);

    cx.update_window(settings, |_, window, cx| {
        let entity = settings_entity(window, cx).unwrap();
        entity.update(cx, |this, cx| {
            this.open_platform_auth_editor(true, window, cx);
            this.connections
                .platform_auth_type
                .update(cx, |s, cx| s.set_selected_index(Some(IndexPath::new(3)), window, cx));
            this.connections
                .platform_destination
                .update(cx, |s, cx| s.set_value("app.example.invalid", window, cx));
            this.connections
                .platform_username
                .update(cx, |s, cx| s.set_value("bob", window, cx));
            this.connections
                .platform_password
                .update(cx, |s, cx| s.set_value("pw2", window, cx));
            this.connections
                .platform_domain
                .update(cx, |s, cx| s.set_value("CORP", window, cx));
            this.connections
                .platform_hostname
                .update(cx, |s, cx| s.set_value("HOST1", window, cx));
            this.save_platform_auth_rule(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    let ws2 = saved(&mut fixture.commands);
    assert_eq!(ws2.upstream_proxy.rules.len(), 1);
    assert_eq!(ws2.upstream_proxy.platform_auth.rules.len(), 1);
    assert_eq!(
        ws2.upstream_proxy.platform_auth.rules[0].auth_type,
        PlatformAuthType::NtlmV2
    );
    assert_eq!(ws2.upstream_proxy.platform_auth.rules[0].domain, "CORP");
    assert_eq!(ws2.upstream_proxy.platform_auth.rules[0].hostname, "HOST1");
    acknowledge(&fixture, ws2, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "settings-connections-page",
            ScrollDelta::Pixels(point(px(0.), px(-1200.))),
            cx,
        );
        window.render_frame(cx);
        window.click("settings-platform-auth-enabled", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws3 = saved(&mut fixture.commands);
    assert!(ws3.upstream_proxy.platform_auth.enabled);
    assert_eq!(ws3.upstream_proxy.rules.len(), 1);
    acknowledge(&fixture, ws3, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-platform-auth-prompt", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws4 = saved(&mut fixture.commands);
    assert!(ws4.upstream_proxy.platform_auth.prompt_on_failure);
    acknowledge(&fixture, ws4, cx);

    cx.update_window(settings, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-platform-auth-override", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let ws5 = saved(&mut fixture.commands);
    assert!(ws5.upstream_proxy.platform_auth.override_project_only);
    assert_eq!(ws5.upstream_proxy.rules.len(), 1);
    acknowledge(&fixture, ws5, cx);
}
