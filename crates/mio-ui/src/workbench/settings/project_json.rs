use super::*;

pub(super) fn export_project_settings_json(ws: &Workspace) -> Result<String, String> {
    mio_core::export_project_settings_json(ws)
}

pub(super) fn apply_project_settings_subset(ws: &mut Workspace, raw: &str) -> Result<(), String> {
    let patch = mio_core::parse_project_settings_patch(raw)?;
    patch.apply_to(ws)
}

impl Settings {
    fn validate_all_settings_ime(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), &'static str> {
        for input in [
            &self.search_input,
            &self.repeater_timeout_input,
            &self.repeater_redirect_max_hops_input,
            &self.repeater_request_timeout_input,
            &self.upstream_host,
            &self.upstream_port,
            &self.upstream_username,
            &self.upstream_password,
            &self.connections.rule_destination,
            &self.connections.rule_host,
            &self.connections.rule_port,
            &self.connections.rule_username,
            &self.connections.rule_password,
            &self.connections.rule_domain,
            &self.connections.rule_workstation,
            &self.connections.rule_description,
            &self.connections.platform_destination,
            &self.connections.platform_username,
            &self.connections.platform_password,
            &self.connections.platform_domain,
            &self.connections.platform_hostname,
            &self.connections.socks_bypass_input,
            &self.connections.normal_timeout_input,
            &self.connections.open_ended_timeout_input,
            &self.connections.dns_hostname_input,
            &self.connections.dns_ip_input,
            &self.listeners_tls.client_cert_host,
            &self.listeners_tls.client_cert_path,
            &self.listeners_tls.client_cert_key_path,
            &self.listeners_tls.client_cert_password,
            &self.http_streaming_scope_input,
            &self.scheduled_task_time,
            &self.scheduled_task_repeat,
            &self.scheduled_task_name,
            &self.scope_include_input,
            &self.scope_exclude_input,
            &self.scope_oos_custom_input,
            &self.proxy_advanced.ssl_host,
            &self.proxy_advanced.ssl_port,
            &self.proxy_advanced.ws_match,
            &self.proxy_advanced.ws_replace,
            &self.proxy_advanced.ws_comment,
            &self.sessions.rule_name,
            &self.sessions.rule_prefixes,
            &self.sessions.rule_req_param,
            &self.sessions.cookie_domain,
            &self.sessions.cookie_name,
            &self.sessions.cookie_value,
            &self.sessions.macro_name,
            &self.sessions.step_url,
        ] {
            editing::input_snapshot(input, window, cx)?;
        }
        Ok(())
    }

    pub(super) fn export_project_settings_to_clipboard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.enabled {
            return;
        }
        if let Err(err) = self.validate_all_settings_ime(window, cx) {
            self.project_json_feedback = Some((true, err.to_owned()));
            self.settings_error(err, cx);
            cx.notify();
            return;
        }
        let project = self.state.project;
        let res = self.owner.update(cx, |view, cx| {
            if project.is_none()
                || view.project != project
                || !view.backend_available()
                || view.transition.is_some()
            {
                return Err("当前项目尚未就绪或正在切换；未导出项目设置。".to_owned());
            }
            let json = export_project_settings_json(&view.workspace)?;
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(json));
            view.error = None;
            view.status = "已复制项目设置 JSON 到剪贴板。".into();
            cx.notify();
            Ok(())
        });
        match res {
            Ok(Ok(())) => {
                self.project_json_feedback = Some((false, "已复制项目设置 JSON 到剪贴板。".into()));
            }
            Ok(Err(err)) => {
                self.project_json_feedback = Some((true, err.clone()));
                self.settings_error(err, cx);
            }
            Err(_) => {
                self.project_json_feedback = Some((true, "主工作台已关闭；未导出设置。".into()));
            }
        }
        cx.notify();
    }

    pub(super) fn import_project_settings_from_clipboard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.enabled {
            return;
        }
        if let Err(err) = self.validate_all_settings_ime(window, cx) {
            self.project_json_feedback = Some((true, err.to_owned()));
            self.settings_error(err, cx);
            cx.notify();
            return;
        }
        let Some(raw) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            let msg = "剪贴板中没有可导入的项目设置 JSON 文本。";
            self.project_json_feedback = Some((true, msg.into()));
            self.settings_error(msg, cx);
            cx.notify();
            return;
        };
        let project = self.state.project;
        let res = self.owner.update(cx, |view, cx| {
            if project.is_none()
                || view.project != project
                || !view.backend_available()
                || view.transition.is_some()
                || view.choosing_save_as
            {
                return Err("当前项目尚未就绪或正在切换；未导入项目设置。".to_owned());
            }
            let mut candidate = view.workspace.clone();
            apply_project_settings_subset(&mut candidate, &raw)?;
            let unchanged = candidate.repeater_settings == view.workspace.repeater_settings
                && candidate.network_http == view.workspace.network_http
                && candidate.upstream_proxy == view.workspace.upstream_proxy
                && candidate.upstream_tls == view.workspace.upstream_tls
                && candidate.proxy_listeners == view.workspace.proxy_listeners
                && candidate.client_certificates == view.workspace.client_certificates
                && candidate.scope == view.workspace.scope
                && candidate.match_and_replace_rules == view.workspace.match_and_replace_rules
                && candidate.ssl_pass_through_rules == view.workspace.ssl_pass_through_rules
                && candidate.ws_match_and_replace_rules
                    == view.workspace.ws_match_and_replace_rules
                && candidate.proxy_interception == view.workspace.proxy_interception
                && candidate.proxy_messages == view.workspace.proxy_messages
                && candidate.proxy_websocket_interception
                    == view.workspace.proxy_websocket_interception
                && candidate.sessions == view.workspace.sessions
                && candidate.tasks_auto_start == view.workspace.tasks_auto_start
                && candidate.scheduled_tasks == view.workspace.scheduled_tasks;
            if !unchanged {
                view.workspace = candidate;
                let primary = view.workspace.proxy_listener_or_default().to_owned();
                view.listener_input
                    .update(cx, |input, cx| input.set_value(primary, window, cx));
                view.target_pane.advanced_mode_input = view.workspace.scope.advanced_mode;
                view.target_pane.editing_simple_scope = [None, None];
                view.target_pane.editing_advanced_scope = [None, None];
                view.target_pane.settings_editing_scope = [None, None];
                if !view.save_workspace(cx) {
                    return Err(view
                        .error
                        .clone()
                        .unwrap_or_else(|| "保存项目设置失败。".into()));
                }
            }
            view.error = None;
            view.status = "已从剪贴板导入项目设置 JSON。".into();
            cx.notify();
            Ok(())
        });
        match res {
            Ok(Ok(())) => {
                if let Some(dialog) = self.listeners_tls.dialog.take() {
                    let _ = dialog.update(cx, |_, window, _| window.remove_window());
                }
                if let Some(dialog) = self.interception_dialog.take() {
                    let _ = dialog.update(cx, |_, window, _| window.remove_window());
                }
                if let Some(dialog) = self.proxy_advanced.http_dialog.take() {
                    let _ = dialog.update(cx, |_, window, _| window.remove_window());
                }
                self.reset_inline_editors(cx);
                self.project_json_feedback = Some((false, "已从剪贴板导入项目设置 JSON。".into()));
            }
            Ok(Err(err)) => {
                self.project_json_feedback = Some((true, err.clone()));
                self.settings_error(err, cx);
            }
            Err(_) => {
                self.project_json_feedback = Some((true, "主工作台已关闭；未导入设置。".into()));
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mio_core::{
        PlatformAuthRule, PlatformAuthSettings, PlatformAuthType, ProxyListenerCertMode,
        ProxyListenerConfig, ScheduledTask, ScheduledTaskAction, ScopeRule, SocksProxySettings,
        SslPassThroughRule, TlsProtocolMode, UpstreamAuthType, UpstreamProxyProtocol,
        UpstreamProxyRule, Uuid, WsMatchAndReplaceRule, WsReplaceDirection,
    };

    #[test]
    fn project_json_roundtrip_preserves_all_phase2_fields_and_legacy_compat() {
        let mut ws = Workspace::default();
        ws.repeater_settings.http1_connection_reuse = true;
        ws.repeater_settings.http2_connection_reuse = false;
        ws.repeater_settings.allow_http2_alpn_override = true;
        ws.repeater_settings.process_cookies_in_redirects = true;
        ws.repeater_settings.cross_domain_protocol_redirects = true;
        ws.repeater_settings.streaming_response_timeout_secs = 123;

        ws.network_http.redirect_3xx = false;
        ws.network_http.redirect_javascript = true;
        ws.network_http.redirect_any_location = true;
        ws.network_http.streaming_advanced_scope = true;
        ws.network_http.streaming_scope_rules.push(ScopeRule {
            id: Uuid::new_v4(),
            enabled: true,
            prefix: "https://stream.example.com/events".into(),
            include_subdomains: true,
        });

        ws.upstream_proxy.override_project_only = true;
        ws.upstream_proxy.rules.push(UpstreamProxyRule {
            id: Uuid::new_v4(),
            enabled: true,
            destination_host: "*.corp.local".into(),
            protocol: UpstreamProxyProtocol::Https,
            host: "proxy.corp.local".into(),
            port: 8443,
            auth_type: UpstreamAuthType::Basic,
            username: "u1".into(),
            password: "p1".into(),
            domain: String::new(),
            workstation: String::new(),
            description: "corporate proxy".into(),
        });
        ws.upstream_proxy.platform_auth = PlatformAuthSettings {
            enabled: true,
            prompt_on_failure: true,
            override_project_only: true,
            rules: vec![PlatformAuthRule {
                id: Uuid::new_v4(),
                enabled: true,
                destination_host: "*.corp.local".into(),
                auth_type: PlatformAuthType::NtlmV2,
                username: "u2".into(),
                password: "p2".into(),
                domain: "CORP".into(),
                hostname: "WKSTN1".into(),
            }],
        };
        ws.upstream_proxy.socks = SocksProxySettings {
            use_proxy: false,
            host: "127.0.0.1".into(),
            port: 1080,
            username: String::new(),
            password: String::new(),
            dns_over_socks: false,
            bypass_hosts: vec!["localhost".into(), "*.local".into()],
            override_project_only: true,
        };

        ws.upstream_tls.protocol_mode = TlsProtocolMode::Custom;
        ws.upstream_tls.custom_protocols = vec!["TLSv1.3".into(), "TLSv1.2".into()];
        ws.upstream_tls.custom_cipher_suites = vec!["TLS13_CHACHA20_POLY1305_SHA256".into()];
        ws.upstream_tls.disable_session_resume = true;
        ws.upstream_tls.client_certs_override_project_only = true;

        const TEST_LISTENER_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\n\
MIIBbTCCARSgAwIBAgIUVIpxIWbRtuDLpYl1477vzV1ugh0wCgYIKoZIzj0EAwIw\n\
HTEbMBkGA1UEAwwSY2xpZW50LmV4YW1wbGUuY29tMB4XDTI2MTExMjA1MzA1NloX\n\
DTM2MTEwOTA1MzA1NlowHTEbMBkGA1UEAwwSY2xpZW50LmV4YW1wbGUuY29tMFkw\n\
EwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEIbSR+tNi17qXQFl59/hH+44RmlzWWDXc\n\
PEoCCPz1C/LzkFknUsbyvWR0EFyYoN31/z+spQ+LUJ8jj+u+DnVYUaMyMDAwHQYD\n\
VR0OBBYEFPiXZXvod4JsIMGiUYlRQAUcuszHMA8GA1UdEwEB/wQFMAMBAf8wCgYI\n\
KoZIzj0EAwIDRwAwRAIgaOz+x4MzyOceAJOUPHMeueynmih/2RJdlZ8u/MVFd5EC\n\
IF63cYtN914wygUdSOfm2bLCW6p5IoVVbzuVti6y/Qml\n\
-----END CERTIFICATE-----\n";
        const TEST_LISTENER_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQg4XlmnemZ2BNPQoa8\n\
WzmlhPk1AkNyYLQDHfm6fDB64SChRANCAAQhtJH602LXupdAWXn3+Ef7jhGaXNZY\n\
Ndw8SgII/PUL8vOQWSdSxvK9ZHQQXJig3fX/P6ylD4tQnyOP674OdVhR\n\
-----END PRIVATE KEY-----\n";
        ws.proxy_listeners.push(ProxyListenerConfig {
            id: Uuid::new_v4(),
            enabled: true,
            bind_address: "127.0.0.1:8082".into(),
            invisible_proxying: true,
            redirect_to_host: "backend.local".into(),
            redirect_to_port: Some(8443),
            force_use_of_tls: true,
            certificate_mode: ProxyListenerCertMode::CustomCertificate,
            specific_cert_hostname: String::new(),
            enable_http2: false,
            tls_protocols_label: "TLSv1.3 only".into(),
            custom_cert_pem: Some(TEST_LISTENER_CERT_PEM.into()),
            custom_key_pem: Some(TEST_LISTENER_KEY_PEM.into()),
        });

        ws.scope.include.push(ScopeRule {
            id: Uuid::new_v4(),
            enabled: true,
            prefix: "https://example.com".into(),
            include_subdomains: true,
        });
        ws.scope.drop_out_of_scope = true;
        ws.scope.out_of_scope_use_custom = true;
        ws.scope.out_of_scope_custom_rules.push(ScopeRule {
            id: Uuid::new_v4(),
            enabled: true,
            prefix: "https://allow.example.com".into(),
            include_subdomains: false,
        });

        ws.sessions.cookie_jar.monitor_repeater = true;
        ws.sessions.cookie_jar.monitor_mio_ai = true;
        ws.ssl_pass_through_rules.push(SslPassThroughRule {
            id: Uuid::new_v4(),
            enabled: true,
            host_pattern: "*.pin.example.invalid".into(),
            port: Some(8443),
            is_regex: false,
        });
        ws.ws_match_and_replace_rules.push(WsMatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            direction: WsReplaceDirection::Both,
            match_pattern: "foo".into(),
            replace_with: "bar".into(),
            is_regex: false,
            comment: "ws".into(),
        });
        ws.tasks_auto_start = false;
        ws.scheduled_tasks.push(ScheduledTask {
            id: Uuid::new_v4(),
            enabled: true,
            action: ScheduledTaskAction::RunScan,
            trigger_spec: "09:00 | 每日".into(),
            description: "morning scan".into(),
        });

        let exported = export_project_settings_json(&ws).expect("export must succeed");
        let mut restored = Workspace::default();
        apply_project_settings_subset(&mut restored, &exported).expect("import must succeed");

        assert_eq!(restored.repeater_settings, ws.repeater_settings);
        assert_eq!(restored.network_http, ws.network_http);
        assert_eq!(restored.upstream_proxy, ws.upstream_proxy);
        assert_eq!(restored.upstream_tls, ws.upstream_tls);
        assert_eq!(restored.proxy_listeners, ws.proxy_listeners);
        assert_eq!(restored.scope, ws.scope);
        assert_eq!(restored.ssl_pass_through_rules, ws.ssl_pass_through_rules);
        assert_eq!(
            restored.ws_match_and_replace_rules,
            ws.ws_match_and_replace_rules
        );
        assert_eq!(restored.sessions, ws.sessions);
        assert_eq!(restored.tasks_auto_start, ws.tasks_auto_start);
        assert_eq!(restored.scheduled_tasks, ws.scheduled_tasks);

        // Legacy v1 JSON without any Phase 2 keys must still import cleanly with defaults.
        let legacy = format!(
            r#"{{
              "format": "mio.project_settings.v1",
              "repeater_settings": {{"request_timeout_secs": 45}},
              "scope": {{"include": [{{"id": "{}", "enabled": true, "prefix": "https://legacy.test"}}]}}
            }}"#,
            Uuid::new_v4()
        );
        let mut from_legacy = Workspace::default();
        apply_project_settings_subset(&mut from_legacy, &legacy)
            .expect("legacy v1 JSON must remain backward compatible");
        assert_eq!(from_legacy.repeater_settings.request_timeout_secs, 45);
        assert_eq!(
            from_legacy.repeater_settings.streaming_response_timeout_secs,
            600
        );
        assert!(from_legacy.repeater_settings.http2_connection_reuse);
        assert!(from_legacy.network_http.is_default());
        assert!(from_legacy.upstream_tls.is_default());
        assert!(from_legacy.proxy_listeners.is_empty());
        assert!(from_legacy.tasks_auto_start);
        assert!(from_legacy.scheduled_tasks.is_empty());
        assert!(from_legacy.ssl_pass_through_rules.is_empty());
        assert!(from_legacy.ws_match_and_replace_rules.is_empty());
        assert!(from_legacy.upstream_proxy.platform_auth.is_default());
        assert!(!from_legacy.scope.include[0].include_subdomains);
        assert!(!from_legacy.scope.drop_out_of_scope);

        // Partial import into an existing workspace must preserve omitted sections.
        let mut partial_target = restored.clone();
        let partial_scope = format!(
            r#"{{
              "format": "mio.project_settings.v1",
              "scope": {{"include": [{{"id": "{}", "enabled": true, "prefix": "https://only-scope.example.invalid"}}]}}
            }}"#,
            Uuid::new_v4()
        );
        apply_project_settings_subset(&mut partial_target, &partial_scope)
            .expect("partial merge must succeed");
        assert_eq!(partial_target.scope.include.len(), 1);
        assert_eq!(
            partial_target.scope.include[0].prefix,
            "https://only-scope.example.invalid"
        );
        assert_eq!(partial_target.proxy_listeners, restored.proxy_listeners);
        assert_eq!(partial_target.upstream_proxy, restored.upstream_proxy);
    }
}
