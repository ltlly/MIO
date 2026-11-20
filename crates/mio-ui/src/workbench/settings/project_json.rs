use super::*;
use mio_core::{
    ClientCertificateFormat, ClientCertificateRule, MatchAndReplaceRule, NetworkHttpSettings,
    ProxyInterceptionSettings, ProxyListenerCertMode, ProxyListenerConfig, ProxyMessageSettings,
    ProxyWebSocketInterceptionSettings, RepeaterSettings, ScheduledTask, ScheduledTaskAction,
    SessionSettings, TargetScope, TlsProtocolMode, UpstreamProxySettings, UpstreamTlsSettings,
    extract_ssl_pass_through_rules, extract_ws_match_and_replace_rules, validate_ssl_and_ws_rules,
};

const MAX_PROJECT_SETTINGS_JSON_BYTES: usize = 512 * 1024;
const FORMAT_TAG: &str = "mio.project_settings.v1";

pub(super) struct ProjectSettingsSubset {
    pub(super) repeater_settings: RepeaterSettings,
    pub(super) network_http: NetworkHttpSettings,
    pub(super) upstream_proxy: UpstreamProxySettings,
    pub(super) upstream_tls: UpstreamTlsSettings,
    pub(super) proxy_listeners: Vec<ProxyListenerConfig>,
    pub(super) client_certificates: Vec<ClientCertificateRule>,
    pub(super) scope: TargetScope,
    pub(super) match_and_replace_rules: Vec<MatchAndReplaceRule>,
    pub(super) proxy_interception: ProxyInterceptionSettings,
    pub(super) proxy_messages: ProxyMessageSettings,
    pub(super) proxy_websocket_interception: ProxyWebSocketInterceptionSettings,
    pub(super) sessions: SessionSettings,
    pub(super) tasks_auto_start: bool,
    pub(super) scheduled_tasks: Vec<ScheduledTask>,
}

fn esc_json_str(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('"');
    for ch in raw.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub(super) fn export_project_settings_json(ws: &Workspace) -> Result<String, String> {
    let scope_json = mio_core::export_target_scope_json(&ws.scope)?;
    if !(1..=3600).contains(&ws.repeater_settings.request_timeout_secs) {
        return Err("Repeater 请求超时必须在 1 到 3600 秒之间".into());
    }
    if ws.repeater_settings.streaming_response_timeout_secs > 86_400 {
        return Err("Repeater 流式响应超时必须在 0 到 86400 秒之间".into());
    }
    ws.upstream_proxy
        .validate()
        .map_err(|err| err.to_string())?;
    ws.upstream_tls.validate().map_err(|err| err.to_string())?;
    ws.proxy_interception
        .validate()
        .map_err(|err| err.to_string())?;
    ws.sessions.validate().map_err(|err| err.to_string())?;

    let probe = Workspace {
        repeater_settings: ws.repeater_settings.clone(),
        network_http: ws.network_http.clone(),
        upstream_proxy: ws.upstream_proxy.clone(),
        upstream_tls: ws.upstream_tls.clone(),
        proxy_listeners: ws.proxy_listeners.clone(),
        proxy_listener: ws.proxy_listener.clone(),
        client_certificates: ws.client_certificates.clone(),
        scope: ws.scope.clone(),
        match_and_replace_rules: ws.match_and_replace_rules.clone(),
        proxy_interception: ws.proxy_interception.clone(),
        proxy_messages: ws.proxy_messages,
        proxy_websocket_interception: ws.proxy_websocket_interception,
        sessions: ws.sessions.clone(),
        tasks_auto_start: ws.tasks_auto_start,
        scheduled_tasks: ws.scheduled_tasks.clone(),
        ..Default::default()
    };
    probe.validate().map_err(|err| err.to_string())?;

    Ok(render_subset_json(&ProjectSettingsSubset {
        repeater_settings: ws.repeater_settings.clone(),
        network_http: ws.network_http.clone(),
        upstream_proxy: ws.upstream_proxy.clone(),
        upstream_tls: ws.upstream_tls.clone(),
        proxy_listeners: ws.proxy_listeners.clone(),
        client_certificates: ws.client_certificates.clone(),
        scope: mio_core::parse_target_scope_json(&scope_json)?,
        match_and_replace_rules: ws.match_and_replace_rules.clone(),
        proxy_interception: ws.proxy_interception.clone(),
        proxy_messages: ws.proxy_messages,
        proxy_websocket_interception: ws.proxy_websocket_interception,
        sessions: ws.sessions.clone(),
        tasks_auto_start: ws.tasks_auto_start,
        scheduled_tasks: ws.scheduled_tasks.clone(),
    }))
}

fn render_repeater_json(r: &RepeaterSettings) -> String {
    let redir = match r.follow_redirects {
        mio_core::RepeaterRedirectMode::Never => "never",
        mio_core::RepeaterRedirectMode::OnSiteOnly => "on_site_only",
        mio_core::RepeaterRedirectMode::InScopeOnly => "in_scope_only",
        mio_core::RepeaterRedirectMode::Always => "always",
    };
    let layout = match r.layout {
        mio_core::RepeaterLayoutMode::SideBySide => "side_by_side",
        mio_core::RepeaterLayoutMode::TopBottom => "top_bottom",
        mio_core::RepeaterLayoutMode::Combined => "combined",
    };
    format!(
        "{{\"unpack_compressed_responses\":{},\"update_content_length\":{},\"normalize_http1_line_endings\":{},\"follow_redirects\":{},\"request_timeout_secs\":{},\"layout\":{},\"strip_connection_headers_over_http2\":{},\"enforce_http1_connection_close\":{},\"auto_follow_redirections_max_hops\":{},\"http1_connection_reuse\":{},\"http2_connection_reuse\":{},\"allow_http2_alpn_override\":{},\"process_cookies_in_redirects\":{},\"cross_domain_protocol_redirects\":{},\"streaming_response_timeout_secs\":{}}}",
        r.unpack_compressed_responses,
        r.update_content_length,
        r.normalize_http1_line_endings,
        esc_json_str(redir),
        r.request_timeout_secs,
        esc_json_str(layout),
        r.strip_connection_headers_over_http2,
        r.enforce_http1_connection_close,
        r.auto_follow_redirections_max_hops,
        r.http1_connection_reuse,
        r.http2_connection_reuse,
        r.allow_http2_alpn_override,
        r.process_cookies_in_redirects,
        r.cross_domain_protocol_redirects,
        r.streaming_response_timeout_secs,
    )
}

fn render_network_http_json(nh: &NetworkHttpSettings) -> String {
    let rules: Vec<String> = nh
        .streaming_scope_rules
        .iter()
        .map(|r| {
            format!(
                "{{\"id\":{},\"enabled\":{},\"prefix\":{},\"include_subdomains\":{}}}",
                esc_json_str(&r.id.to_string()),
                r.enabled,
                esc_json_str(&r.prefix),
                r.include_subdomains,
            )
        })
        .collect();
    format!(
        "{{\"default_to_http2\":{},\"treat_event_stream_as_streaming\":{},\"store_streaming_responses\":{},\"strip_chunked_metadata_in_streaming\":{},\"understand_100_continue\":{},\"remove_100_continue_headers\":{},\"redirect_3xx\":{},\"redirect_refresh_header\":{},\"redirect_meta_refresh\":{},\"redirect_javascript\":{},\"redirect_any_location\":{},\"streaming_advanced_scope\":{},\"streaming_scope_rules\":[{}]}}",
        nh.default_to_http2,
        nh.treat_event_stream_as_streaming,
        nh.store_streaming_responses,
        nh.strip_chunked_metadata_in_streaming,
        nh.understand_100_continue,
        nh.remove_100_continue_headers,
        nh.redirect_3xx,
        nh.redirect_refresh_header,
        nh.redirect_meta_refresh,
        nh.redirect_javascript,
        nh.redirect_any_location,
        nh.streaming_advanced_scope,
        rules.join(","),
    )
}

fn render_upstream_proxy_json(up: &UpstreamProxySettings) -> String {
    let up_proto = match up.protocol {
        mio_core::UpstreamProxyProtocol::Direct => "direct",
        mio_core::UpstreamProxyProtocol::Http => "http",
        mio_core::UpstreamProxyProtocol::Https => "https",
        mio_core::UpstreamProxyProtocol::Socks5 => "socks5",
    };
    let up_rules: Vec<String> = up
        .rules
        .iter()
        .map(|r| {
            let proto = match r.protocol {
                mio_core::UpstreamProxyProtocol::Http => "http",
                mio_core::UpstreamProxyProtocol::Https => "https",
                mio_core::UpstreamProxyProtocol::Direct => "direct",
                mio_core::UpstreamProxyProtocol::Socks5 => "socks5",
            };
            let auth = match r.auth_type {
                mio_core::UpstreamAuthType::None => "none",
                mio_core::UpstreamAuthType::Basic => "basic",
            };
            format!(
                "{{\"id\":{},\"enabled\":{},\"destination_host\":{},\"protocol\":{},\"host\":{},\"port\":{},\"auth_type\":{},\"username\":{},\"password\":{},\"description\":{}}}",
                esc_json_str(&r.id.to_string()),
                r.enabled,
                esc_json_str(&r.destination_host),
                esc_json_str(proto),
                esc_json_str(&r.host),
                r.port,
                esc_json_str(auth),
                esc_json_str(&r.username),
                esc_json_str(&r.password),
                esc_json_str(&r.description),
            )
        })
        .collect();
    let host_res: Vec<String> = up
        .hostname_resolutions
        .iter()
        .map(|h| {
            format!(
                "{{\"id\":{},\"enabled\":{},\"hostname\":{},\"ip_address\":{}}}",
                esc_json_str(&h.id.to_string()),
                h.enabled,
                esc_json_str(&h.hostname),
                esc_json_str(&h.ip_address),
            )
        })
        .collect();
    let socks = &up.socks;
    let bypass_items: Vec<String> = socks
        .bypass_hosts
        .iter()
        .map(|h| esc_json_str(h))
        .collect();
    format!(
        "{{\"protocol\":{},\"host\":{},\"port\":{},\"username\":{},\"password\":{},\"rules\":[{}],\"socks\":{{\"use_proxy\":{},\"host\":{},\"port\":{},\"username\":{},\"password\":{},\"dns_over_socks\":{},\"bypass_hosts\":[{}],\"override_project_only\":{}}},\"open_ended_response_timeout_secs\":{},\"hostname_resolutions\":[{}],\"override_project_only\":{}}}",
        esc_json_str(up_proto),
        esc_json_str(&up.host),
        up.port,
        esc_json_str(&up.username),
        esc_json_str(&up.password),
        up_rules.join(","),
        socks.use_proxy,
        esc_json_str(&socks.host),
        socks.port,
        esc_json_str(&socks.username),
        esc_json_str(&socks.password),
        socks.dns_over_socks,
        bypass_items.join(","),
        socks.override_project_only,
        up.open_ended_response_timeout_secs,
        host_res.join(","),
        up.override_project_only,
    )
}

fn render_upstream_tls_json(tls: &UpstreamTlsSettings) -> String {
    let ders: Vec<String> = tls
        .custom_ca_ders
        .iter()
        .map(|der| {
            let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, der);
            esc_json_str(&b64)
        })
        .collect();
    let mode = match tls.protocol_mode {
        TlsProtocolMode::AllSupported => "all_supported",
        TlsProtocolMode::Default => "default",
        TlsProtocolMode::Custom => "custom",
    };
    format!(
        "{{\"custom_ca_ders_b64\":[{}],\"include_system_roots\":{},\"insecure_skip_verify\":{},\"protocol_mode\":{},\"allow_unsafe_renegotiation\":{},\"disable_session_resume\":{},\"client_certs_override_project_only\":{}}}",
        ders.join(","),
        tls.include_system_roots,
        tls.insecure_skip_verify,
        esc_json_str(mode),
        tls.allow_unsafe_renegotiation,
        tls.disable_session_resume,
        tls.client_certs_override_project_only,
    )
}

fn render_proxy_listeners_json(listeners: &[ProxyListenerConfig]) -> String {
    let items: Vec<String> = listeners
        .iter()
        .map(|l| {
            let port_str = l
                .redirect_to_port
                .map_or_else(|| "null".into(), |p| p.to_string());
            let cert_mode = match l.certificate_mode {
                ProxyListenerCertMode::PerHostCaSigned => "per_host_ca_signed",
                ProxyListenerCertMode::SelfSigned => "self_signed",
                ProxyListenerCertMode::SpecificHostname => "specific_hostname",
                ProxyListenerCertMode::CustomCertificate => "custom_certificate",
            };
            let cert_pem = l
                .custom_cert_pem
                .as_deref()
                .map_or_else(|| "null".into(), esc_json_str);
            let key_pem = l
                .custom_key_pem
                .as_deref()
                .map_or_else(|| "null".into(), esc_json_str);
            format!(
                "{{\"id\":{},\"enabled\":{},\"bind_address\":{},\"invisible_proxying\":{},\"redirect_to_host\":{},\"redirect_to_port\":{},\"force_use_of_tls\":{},\"certificate_mode\":{},\"specific_cert_hostname\":{},\"enable_http2\":{},\"tls_protocols_label\":{},\"custom_cert_pem\":{},\"custom_key_pem\":{}}}",
                esc_json_str(&l.id.to_string()),
                l.enabled,
                esc_json_str(&l.bind_address),
                l.invisible_proxying,
                esc_json_str(&l.redirect_to_host),
                port_str,
                l.force_use_of_tls,
                esc_json_str(cert_mode),
                esc_json_str(&l.specific_cert_hostname),
                l.enable_http2,
                esc_json_str(&l.tls_protocols_label),
                cert_pem,
                key_pem,
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

fn render_client_certificates_json(certs: &[ClientCertificateRule]) -> String {
    let items: Vec<String> = certs
        .iter()
        .map(|c| {
            let fmt = match c.format {
                ClientCertificateFormat::Pem => "pem",
                ClientCertificateFormat::Pkcs12 => "pkcs12",
            };
            let cert_b64 = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                &c.certificate_bytes,
            );
            let key_b64 = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                &c.private_key_pem,
            );
            format!(
                "{{\"enabled\":{},\"host_pattern\":{},\"format\":{},\"certificate_b64\":{},\"private_key_pem_b64\":{},\"password\":{}}}",
                c.enabled,
                esc_json_str(&c.host_pattern),
                esc_json_str(fmt),
                esc_json_str(&cert_b64),
                esc_json_str(&key_b64),
                esc_json_str(&c.password),
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

fn render_scheduled_tasks_json(tasks: &[ScheduledTask]) -> String {
    let items: Vec<String> = tasks
        .iter()
        .map(|t| {
            let action = match t.action {
                ScheduledTaskAction::PauseTasks => "pause_tasks",
                ScheduledTaskAction::ResumeTasks => "resume_tasks",
                ScheduledTaskAction::RunScan => "run_scan",
            };
            format!(
                "{{\"id\":{},\"enabled\":{},\"action\":{},\"trigger_spec\":{},\"description\":{}}}",
                esc_json_str(&t.id.to_string()),
                t.enabled,
                esc_json_str(action),
                esc_json_str(&t.trigger_spec),
                esc_json_str(&t.description),
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

fn render_subset_json(s: &ProjectSettingsSubset) -> String {
    let scope_json = mio_core::export_target_scope_json(&s.scope).unwrap_or_else(|_| "{}".into());
    let repeater_json = render_repeater_json(&s.repeater_settings);
    let network_http_json = render_network_http_json(&s.network_http);
    let upstream_proxy_json = render_upstream_proxy_json(&s.upstream_proxy);
    let upstream_tls_json = render_upstream_tls_json(&s.upstream_tls);
    let proxy_listeners_json = render_proxy_listeners_json(&s.proxy_listeners);
    let client_certs_json = render_client_certificates_json(&s.client_certificates);
    let scheduled_tasks_json = render_scheduled_tasks_json(&s.scheduled_tasks);
    let mr_items: Vec<String> = s
        .match_and_replace_rules
        .iter()
        .map(|r| {
            let item_type = match r.item_type {
                mio_core::MatchReplaceTarget::RequestHeader => "request_header",
                mio_core::MatchReplaceTarget::RequestBody => "request_body",
                mio_core::MatchReplaceTarget::ResponseHeader => "response_header",
                mio_core::MatchReplaceTarget::ResponseBody => "response_body",
                mio_core::MatchReplaceTarget::RequestParamName => "request_param_name",
                mio_core::MatchReplaceTarget::RequestParamValue => "request_param_value",
                mio_core::MatchReplaceTarget::RequestFirstLine => "request_first_line",
                mio_core::MatchReplaceTarget::RequestScript => "request_script",
                mio_core::MatchReplaceTarget::ResponseScript => "response_script",
            };
            format!(
                "{{\"id\":{},\"enabled\":{},\"item_type\":{},\"match_pattern\":{},\"replace_with\":{},\"regex\":{},\"comment\":{},\"script_name\":{},\"filter_script\":{}}}",
                esc_json_str(&r.id.to_string()),
                r.enabled,
                esc_json_str(item_type),
                esc_json_str(&r.match_pattern),
                esc_json_str(&r.replace_with),
                r.regex,
                esc_json_str(&r.comment),
                esc_json_str(&r.script_name),
                esc_json_str(&r.filter_script),
            )
        })
        .collect();
    let intercept_set_json = |set: &mio_core::ProxyInterceptRuleSet| -> String {
        let rules: Vec<String> = set
            .rules
            .iter()
            .map(|r| {
                let op = match r.operator {
                    mio_core::ProxyInterceptOperator::And => "And",
                    mio_core::ProxyInterceptOperator::Or => "Or",
                };
                format!(
                    "{{\"id\":{},\"enabled\":{},\"operator\":{},\"match_type\":\"{:?}\",\"relationship\":\"{:?}\",\"condition\":{}}}",
                    esc_json_str(&r.id.to_string()),
                    r.enabled,
                    esc_json_str(op),
                    r.match_type,
                    r.relationship,
                    esc_json_str(&r.condition)
                )
            })
            .collect();
        format!(
            "{{\"enabled\":{},\"rules\":[{}]}}",
            set.enabled,
            rules.join(",")
        )
    };
    let rm = s.proxy_messages.response_modification;
    let misc = s.proxy_messages.misc;
    let sessions_json = render_sessions_json(&s.sessions);
    format!(
        "{{\n  \"format\": {},\n  \"repeater_settings\": {},\n  \"network_http\": {},\n  \"upstream_proxy\": {},\n  \"upstream_tls\": {},\n  \"proxy_listeners\": {},\n  \"client_certificates\": {},\n  \"scope\": {},\n  \"match_and_replace_rules\": [{}],\n  \"proxy_interception\": {{\"request\":{},\"response\":{}}},\n  \"proxy_messages\": {{\"unpack_compressed_requests\":{},\"unpack_compressed_responses\":{},\"match_replace_in_scope_only\":{},\"ws_match_replace_in_scope_only\":{},\"auto_update_request_content_length\":{},\"auto_fix_request_newlines\":{},\"auto_update_response_content_length\":{},\"ssl_pass_through_auto_add_on_failure\":{},\"response_modification\":{{\"unhide_hidden_form_fields\":{},\"enable_disabled_form_fields\":{},\"highlight_unhidden_fields\":{},\"remove_input_field_length_limits\":{},\"remove_javascript_form_validation\":{},\"remove_all_javascript\":{},\"remove_object_tags\":{},\"convert_https_links_to_http\":{},\"remove_secure_flag_from_cookies\":{}}},\"misc\":{{\"use_http10_in_requests\":{},\"use_http10_in_responses\":{},\"set_response_connection_close\":{},\"set_request_connection_header\":{},\"strip_proxy_headers\":{},\"remove_unsupported_accept_encodings\":{},\"strip_sec_websocket_extensions\":{},\"suppress_browser_error_messages\":{},\"disable_history_logging\":{},\"disable_out_of_scope_history_logging\":{},\"use_http1_keep_alive\":{},\"disable_web_interface\":{}}}}},\n  \"proxy_websocket_interception\": {{\"client_to_server_messages\":{},\"server_to_client_messages\":{},\"intercept_in_scope_only\":{}}},\n  \"sessions\": {},\n  \"tasks_auto_start\": {},\n  \"scheduled_tasks\": {}\n}}",
        esc_json_str(FORMAT_TAG),
        repeater_json,
        network_http_json,
        upstream_proxy_json,
        upstream_tls_json,
        proxy_listeners_json,
        client_certs_json,
        scope_json,
        mr_items.join(","),
        intercept_set_json(&s.proxy_interception.request),
        intercept_set_json(&s.proxy_interception.response),
        s.proxy_messages.unpack_compressed_requests,
        s.proxy_messages.unpack_compressed_responses,
        s.proxy_messages.match_replace_in_scope_only,
        s.proxy_messages.ws_match_replace_in_scope_only,
        s.proxy_messages.auto_update_request_content_length,
        s.proxy_messages.auto_fix_request_newlines,
        s.proxy_messages.auto_update_response_content_length,
        s.proxy_messages.ssl_pass_through_auto_add_on_failure,
        rm.unhide_hidden_form_fields,
        rm.enable_disabled_form_fields,
        rm.highlight_unhidden_fields,
        rm.remove_input_field_length_limits,
        rm.remove_javascript_form_validation,
        rm.remove_all_javascript,
        rm.remove_object_tags,
        rm.convert_https_links_to_http,
        rm.remove_secure_flag_from_cookies,
        misc.use_http10_in_requests,
        misc.use_http10_in_responses,
        misc.set_response_connection_close,
        misc.set_request_connection_header,
        misc.strip_proxy_headers,
        misc.remove_unsupported_accept_encodings,
        misc.strip_sec_websocket_extensions,
        misc.suppress_browser_error_messages,
        misc.disable_history_logging,
        misc.disable_out_of_scope_history_logging,
        misc.use_http1_keep_alive,
        misc.disable_web_interface,
        s.proxy_websocket_interception.client_to_server_messages,
        s.proxy_websocket_interception.server_to_client_messages,
        s.proxy_websocket_interception.intercept_in_scope_only,
        sessions_json,
        s.tasks_auto_start,
        scheduled_tasks_json,
    )
}

fn render_sessions_json(sessions: &SessionSettings) -> String {
    let entries: Vec<String> = sessions
        .cookie_jar
        .entries
        .iter()
        .map(|e| {
            let exp = e
                .expires_unix_ms
                .map_or_else(|| "null".into(), |ms| ms.to_string());
            format!(
                "{{\"id\":{},\"domain\":{},\"host_only\":{},\"path\":{},\"name\":{},\"value\":{},\"secure\":{},\"http_only\":{},\"expires_unix_ms\":{}}}",
                esc_json_str(&e.id.to_string()),
                esc_json_str(&e.domain),
                e.host_only,
                esc_json_str(&e.path),
                esc_json_str(&e.name),
                esc_json_str(&e.value),
                e.secure,
                e.http_only,
                exp
            )
        })
        .collect();
    let macros: Vec<String> = sessions
        .macros
        .iter()
        .map(|m| {
            let steps: Vec<String> = m
                .steps
                .iter()
                .map(|st| {
                    let hdrs: Vec<String> = st
                        .headers
                        .iter()
                        .map(|h| {
                            let b64 = base64::Engine::encode(
                                &base64::engine::general_purpose::STANDARD,
                                &h.value,
                            );
                            format!(
                                "{{\"name\":{},\"value_b64\":{}}}",
                                esc_json_str(&h.name),
                                esc_json_str(&b64)
                            )
                        })
                        .collect();
                    let body_b64 = base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        &st.body,
                    );
                    let status = st
                        .expected_status
                        .map_or_else(|| "null".into(), |c| c.to_string());
                    let exts: Vec<String> = st
                        .extractions
                        .iter()
                        .map(|ex| {
                            let (sk, sa, sb) = match &ex.source {
                                mio_core::MacroExtractSource::Delimiter { start, end } => {
                                    ("delimiter", start.as_str(), end.as_str())
                                }
                                mio_core::MacroExtractSource::Regex { pattern } => {
                                    ("regex", pattern.as_str(), "")
                                }
                                mio_core::MacroExtractSource::ResponseHeader { header_name } => {
                                    ("response_header", header_name.as_str(), "")
                                }
                            };
                            let (tk, tn) = match &ex.target {
                                mio_core::MacroApplyTarget::UrlOrBodyParam => ("param", ""),
                                mio_core::MacroApplyTarget::Header { header_name } => {
                                    ("header", header_name.as_str())
                                }
                                mio_core::MacroApplyTarget::Cookie { cookie_name } => {
                                    ("cookie", cookie_name.as_str())
                                }
                            };
                            format!(
                                "{{\"name\":{},\"source_kind\":{},\"arg1\":{},\"arg2\":{},\"target_kind\":{},\"target_name\":{}}}",
                                esc_json_str(&ex.name),
                                esc_json_str(sk),
                                esc_json_str(sa),
                                esc_json_str(sb),
                                esc_json_str(tk),
                                esc_json_str(tn)
                            )
                        })
                        .collect();
                    format!(
                        "{{\"id\":{},\"method\":{},\"url\":{},\"headers\":[{}],\"body_b64\":{},\"use_cookie_jar\":{},\"expected_status\":{},\"extractions\":[{}]}}",
                        esc_json_str(&st.id.to_string()),
                        esc_json_str(&st.method),
                        esc_json_str(&st.url),
                        hdrs.join(","),
                        esc_json_str(&body_b64),
                        st.use_cookie_jar,
                        status,
                        exts.join(",")
                    )
                })
                .collect();
            let (pol, retries) = match m.failure_policy {
                mio_core::MacroFailurePolicy::Abort => ("abort", 0u8),
                mio_core::MacroFailurePolicy::Retry { max_retries } => ("retry", max_retries),
            };
            format!(
                "{{\"id\":{},\"name\":{},\"policy\":{},\"max_retries\":{},\"timeout_ms\":{},\"steps\":[{}]}}",
                esc_json_str(&m.id.to_string()),
                esc_json_str(&m.name),
                esc_json_str(pol),
                retries,
                m.timeout_ms,
                steps.join(",")
            )
        })
        .collect();
    let rules: Vec<String> = sessions
        .rules
        .iter()
        .map(|r| {
            let (umode, uprefixes) = match &r.condition.url_scope {
                mio_core::SessionUrlScope::AllUrls => ("all_urls", Vec::new()),
                mio_core::SessionUrlScope::SuiteScope => ("suite_scope", Vec::new()),
                mio_core::SessionUrlScope::CustomPrefixes(list) => {
                    ("custom_prefixes", list.iter().map(|p| esc_json_str(p)).collect())
                }
            };
            let (pmode, pnames) = match &r.condition.param_condition {
                None => ("none", Vec::new()),
                Some(mio_core::SessionParamCondition::RequirePresent(ns)) => {
                    ("require_present", ns.iter().map(|n| esc_json_str(n)).collect())
                }
                Some(mio_core::SessionParamCondition::RequireAbsent(ns)) => {
                    ("require_absent", ns.iter().map(|n| esc_json_str(n)).collect())
                }
            };
            let hcont = r
                .condition
                .header_contains
                .as_deref()
                .map_or_else(|| "null".into(), esc_json_str);
            let acts: Vec<String> = r
                .actions
                .iter()
                .map(|a| match a {
                    mio_core::SessionRuleAction::UseCookieJar {
                        add_new_cookies,
                        update_existing_cookies,
                    } => format!(
                        "{{\"type\":\"use_cookie_jar\",\"add_new_cookies\":{add_new_cookies},\"update_existing_cookies\":{update_existing_cookies}}}"
                    ),
                    mio_core::SessionRuleAction::SetHeader {
                        name,
                        value,
                        only_if_present,
                    } => format!(
                        "{{\"type\":\"set_header\",\"name\":{},\"value\":{},\"only_if_present\":{only_if_present}}}",
                        esc_json_str(name),
                        esc_json_str(value)
                    ),
                    mio_core::SessionRuleAction::SetCookie {
                        name,
                        value,
                        only_if_present,
                    } => format!(
                        "{{\"type\":\"set_cookie\",\"name\":{},\"value\":{},\"only_if_present\":{only_if_present}}}",
                        esc_json_str(name),
                        esc_json_str(value)
                    ),
                    mio_core::SessionRuleAction::RewriteUrlParam {
                        name,
                        value,
                        remove,
                        only_if_present,
                    } => format!(
                        "{{\"type\":\"rewrite_url_param\",\"name\":{},\"value\":{},\"remove\":{remove},\"only_if_present\":{only_if_present}}}",
                        esc_json_str(name),
                        esc_json_str(value)
                    ),
                    mio_core::SessionRuleAction::ReplaceRequestText {
                        match_pattern,
                        replace_with,
                        regex,
                    } => format!(
                        "{{\"type\":\"replace_request_text\",\"match_pattern\":{},\"replace_with\":{},\"regex\":{regex}}}",
                        esc_json_str(match_pattern),
                        esc_json_str(replace_with)
                    ),
                    mio_core::SessionRuleAction::RunMacro {
                        macro_id,
                        update_cookies_from_jar,
                        apply_extractions,
                    } => format!(
                        "{{\"type\":\"run_macro\",\"macro_id\":{},\"update_cookies_from_jar\":{update_cookies_from_jar},\"apply_extractions\":{apply_extractions}}}",
                        esc_json_str(&macro_id.to_string())
                    ),
                })
                .collect();
            format!(
                "{{\"id\":{},\"name\":{},\"enabled\":{},\"tools\":{{\"proxy\":{},\"repeater\":{},\"intruder\":{},\"scanner\":{},\"sequencer\":{},\"extensions\":{}}},\"url_mode\":{},\"url_prefixes\":[{}],\"param_mode\":{},\"param_names\":[{}],\"header_contains\":{},\"actions\":[{}]}}",
                esc_json_str(&r.id.to_string()),
                esc_json_str(&r.name),
                r.enabled,
                r.tools.proxy,
                r.tools.repeater,
                r.tools.intruder,
                r.tools.scanner,
                r.tools.sequencer,
                r.tools.extensions,
                esc_json_str(umode),
                uprefixes.join(","),
                esc_json_str(pmode),
                pnames.join(","),
                hcont,
                acts.join(",")
            )
        })
        .collect();
    format!(
        "{{\"cookie_jar\":{{\"monitor_proxy\":{},\"monitor_repeater\":{},\"monitor_intruder\":{},\"monitor_scanner\":{},\"monitor_sequencer\":{},\"monitor_extensions\":{},\"monitor_mio_ai\":{},\"entries\":[{}]}},\"rules\":[{}],\"macros\":[{}]}}",
        sessions.cookie_jar.monitor_proxy,
        sessions.cookie_jar.monitor_repeater,
        sessions.cookie_jar.monitor_intruder,
        sessions.cookie_jar.monitor_scanner,
        sessions.cookie_jar.monitor_sequencer,
        sessions.cookie_jar.monitor_extensions,
        sessions.cookie_jar.monitor_mio_ai,
        entries.join(","),
        rules.join(","),
        macros.join(",")
    )
}

pub(super) fn apply_project_settings_subset(ws: &mut Workspace, raw: &str) -> Result<(), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("剪贴板中没有可导入的项目设置 JSON 文本。".into());
    }
    if trimmed.len() > MAX_PROJECT_SETTINGS_JSON_BYTES {
        return Err("项目设置 JSON 超过 512 KiB 上限；未导入。".into());
    }
    let subset = super::project_json_parse::parse_project_settings_json(trimmed)?;
    if !(1..=3600).contains(&subset.repeater_settings.request_timeout_secs) {
        return Err("Repeater 请求超时必须在 1 到 3600 秒之间".into());
    }
    if subset.repeater_settings.streaming_response_timeout_secs > 86_400 {
        return Err("Repeater 流式响应超时必须在 0 到 86400 秒之间".into());
    }
    if !(1..=20).contains(&subset.repeater_settings.auto_follow_redirections_max_hops) {
        return Err("Repeater 最大重定向跳数必须在 1 到 20 之间".into());
    }
    subset
        .upstream_proxy
        .validate()
        .map_err(|err| err.to_string())?;
    subset
        .upstream_tls
        .validate()
        .map_err(|err| err.to_string())?;
    subset.scope.validate().map_err(|err| err.to_string())?;
    subset
        .proxy_interception
        .validate()
        .map_err(|err| err.to_string())?;
    subset.sessions.validate().map_err(|err| err.to_string())?;
    for cert in &subset.client_certificates {
        if cert.format == ClientCertificateFormat::Pem {
            cert.parse_identity().map_err(|err| err.to_string())?;
        }
    }
    let ssl = extract_ssl_pass_through_rules(&subset.match_and_replace_rules);
    let ws_mr = extract_ws_match_and_replace_rules(&subset.match_and_replace_rules);
    validate_ssl_and_ws_rules(&ssl, &ws_mr).map_err(|err| err.to_string())?;

    let mut candidate = ws.clone();
    candidate.repeater_settings = subset.repeater_settings;
    candidate.network_http = subset.network_http;
    candidate.upstream_proxy = subset.upstream_proxy;
    candidate.upstream_tls = subset.upstream_tls;
    candidate.proxy_listeners = subset.proxy_listeners;
    if candidate.proxy_listeners.is_empty() {
        candidate.proxy_listener = None;
    } else {
        listeners_tls::sync_primary_listener_field(&mut candidate);
    }
    candidate.client_certificates = subset.client_certificates;
    candidate.scope = subset.scope;
    candidate.match_and_replace_rules = subset.match_and_replace_rules;
    candidate.proxy_interception = subset.proxy_interception;
    candidate.proxy_messages = subset.proxy_messages;
    candidate.proxy_websocket_interception = subset.proxy_websocket_interception;
    candidate.sessions = subset.sessions;
    candidate.tasks_auto_start = subset.tasks_auto_start;
    candidate.scheduled_tasks = subset.scheduled_tasks;
    candidate
        .validate_structure()
        .map_err(|err| err.to_string())?;
    *ws = candidate;
    Ok(())
}

pub(super) use ProjectSettingsSubset as ParsedProjectSettings;
pub(super) const PROJECT_SETTINGS_FORMAT: &str = FORMAT_TAG;

#[cfg(test)]
mod tests {
    use super::*;
    use mio_core::{
        ScopeRule, SocksProxySettings, UpstreamAuthType, UpstreamProxyProtocol, UpstreamProxyRule,
        Uuid,
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
            description: "corporate proxy".into(),
        });
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
        ws.upstream_tls.allow_unsafe_renegotiation = true;
        ws.upstream_tls.disable_session_resume = true;
        ws.upstream_tls.client_certs_override_project_only = true;

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
            custom_cert_pem: Some("-----BEGIN CERTIFICATE-----\nTEST\n-----END CERTIFICATE-----".into()),
            custom_key_pem: Some("-----BEGIN PRIVATE KEY-----\nTEST\n-----END PRIVATE KEY-----".into()),
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

        ws.sessions.cookie_jar.monitor_mio_ai = true;
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
        assert!(!from_legacy.scope.include[0].include_subdomains);
        assert!(!from_legacy.scope.drop_out_of_scope);
    }
}
