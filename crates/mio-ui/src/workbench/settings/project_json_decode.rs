use std::collections::BTreeMap;

use super::project_json::ParsedProjectSettings;
use super::project_json_parse::{
    JVal, check_known_keys, opt_str, req_bool, req_num, req_str, req_uuid,
};
use mio_core::*;

pub(super) fn decode_project_settings(
    root: &BTreeMap<String, JVal>,
) -> Result<ParsedProjectSettings, String> {
    let repeater_settings = match root.get("repeater_settings") {
        None => RepeaterSettings::default(),
        Some(JVal::Obj(o)) => decode_repeater(o)?,
        _ => return Err("`repeater_settings` 必须是对象。".into()),
    };
    let network_http = match root.get("network_http") {
        None => NetworkHttpSettings::default(),
        Some(JVal::Obj(o)) => decode_network_http(o)?,
        _ => return Err("`network_http` 必须是对象。".into()),
    };
    let upstream_proxy = match root.get("upstream_proxy") {
        None => UpstreamProxySettings::default(),
        Some(JVal::Obj(o)) => decode_upstream(o)?,
        _ => return Err("`upstream_proxy` 必须是对象。".into()),
    };
    let upstream_tls = match root.get("upstream_tls") {
        None => UpstreamTlsSettings::default(),
        Some(JVal::Obj(o)) => decode_upstream_tls(o)?,
        _ => return Err("`upstream_tls` 必须是对象。".into()),
    };
    let proxy_listeners = match root.get("proxy_listeners") {
        None => Vec::new(),
        Some(JVal::Arr(arr)) => arr
            .iter()
            .map(decode_proxy_listener)
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("`proxy_listeners` 必须是数组。".into()),
    };
    let client_certificates = match root.get("client_certificates") {
        None => Vec::new(),
        Some(JVal::Arr(arr)) => arr
            .iter()
            .map(decode_client_certificate)
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("`client_certificates` 必须是数组。".into()),
    };
    let scope = match root.get("scope") {
        None => TargetScope::default(),
        Some(JVal::Obj(o)) => decode_scope(o)?,
        _ => return Err("`scope` 必须是对象。".into()),
    };
    let match_and_replace_rules = match root.get("match_and_replace_rules") {
        None => Vec::new(),
        Some(JVal::Arr(arr)) => arr
            .iter()
            .map(decode_mr_rule)
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("`match_and_replace_rules` 必须是数组。".into()),
    };
    let proxy_interception = match root.get("proxy_interception") {
        None => ProxyInterceptionSettings::default(),
        Some(JVal::Obj(o)) => decode_proxy_interception(o)?,
        _ => return Err("`proxy_interception` 必须是对象。".into()),
    };
    let proxy_messages = match root.get("proxy_messages") {
        None => ProxyMessageSettings::default(),
        Some(JVal::Obj(o)) => decode_proxy_messages(o)?,
        _ => return Err("`proxy_messages` 必须是对象。".into()),
    };
    let proxy_websocket_interception = match root.get("proxy_websocket_interception") {
        None => ProxyWebSocketInterceptionSettings::default(),
        Some(JVal::Obj(o)) => decode_ws_interception(o)?,
        _ => return Err("`proxy_websocket_interception` 必须是对象。".into()),
    };
    let sessions = match root.get("sessions") {
        None => SessionSettings::default(),
        Some(JVal::Obj(o)) => super::project_json_sessions::decode_sessions(o)?,
        _ => return Err("`sessions` 必须是对象。".into()),
    };
    let tasks_auto_start = req_bool(root, "tasks_auto_start", true)?;
    let scheduled_tasks = match root.get("scheduled_tasks") {
        None => Vec::new(),
        Some(JVal::Arr(arr)) => arr
            .iter()
            .map(decode_scheduled_task)
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("`scheduled_tasks` 必须是数组。".into()),
    };
    Ok(ParsedProjectSettings {
        repeater_settings,
        network_http,
        upstream_proxy,
        upstream_tls,
        proxy_listeners,
        client_certificates,
        scope,
        match_and_replace_rules,
        proxy_interception,
        proxy_messages,
        proxy_websocket_interception,
        sessions,
        tasks_auto_start,
        scheduled_tasks,
    })
}

fn decode_repeater(o: &BTreeMap<String, JVal>) -> Result<RepeaterSettings, String> {
    check_known_keys(
        o,
        &[
            "unpack_compressed_responses",
            "update_content_length",
            "normalize_http1_line_endings",
            "follow_redirects",
            "request_timeout_secs",
            "layout",
            "strip_connection_headers_over_http2",
            "enforce_http1_connection_close",
            "auto_follow_redirections_max_hops",
            "http1_connection_reuse",
            "http2_connection_reuse",
            "allow_http2_alpn_override",
            "process_cookies_in_redirects",
            "cross_domain_protocol_redirects",
            "streaming_response_timeout_secs",
        ],
        "repeater_settings",
    )?;
    let def = RepeaterSettings::default();
    let follow_redirects = match opt_str(o, "follow_redirects", "never")?.as_str() {
        "never" => RepeaterRedirectMode::Never,
        "on_site_only" => RepeaterRedirectMode::OnSiteOnly,
        "in_scope_only" => RepeaterRedirectMode::InScopeOnly,
        "always" => RepeaterRedirectMode::Always,
        other => return Err(format!("未知 Repeater 重定向模式：`{other}`。")),
    };
    let layout = match opt_str(o, "layout", "side_by_side")?.as_str() {
        "side_by_side" => RepeaterLayoutMode::SideBySide,
        "top_bottom" => RepeaterLayoutMode::TopBottom,
        "combined" => RepeaterLayoutMode::Combined,
        other => return Err(format!("未知 Repeater 布局模式：`{other}`。")),
    };
    let timeout = req_num(o, "request_timeout_secs", def.request_timeout_secs as i128)?;
    let hops = req_num(
        o,
        "auto_follow_redirections_max_hops",
        i128::from(def.auto_follow_redirections_max_hops),
    )?;
    let streaming_timeout = req_num(
        o,
        "streaming_response_timeout_secs",
        def.streaming_response_timeout_secs as i128,
    )?;
    let request_timeout_secs = u64::try_from(timeout)
        .map_err(|_| "Repeater 请求超时必须在 1 到 3600 秒之间".to_owned())?;
    let auto_follow_redirections_max_hops =
        u8::try_from(hops).map_err(|_| "Repeater 最大重定向跳数必须在 1 到 20 之间".to_owned())?;
    let streaming_response_timeout_secs = u64::try_from(streaming_timeout)
        .map_err(|_| "Repeater 流式响应超时必须在 0 到 86400 秒之间".to_owned())?;
    Ok(RepeaterSettings {
        unpack_compressed_responses: req_bool(
            o,
            "unpack_compressed_responses",
            def.unpack_compressed_responses,
        )?,
        update_content_length: req_bool(o, "update_content_length", def.update_content_length)?,
        normalize_http1_line_endings: req_bool(
            o,
            "normalize_http1_line_endings",
            def.normalize_http1_line_endings,
        )?,
        follow_redirects,
        request_timeout_secs,
        layout,
        strip_connection_headers_over_http2: req_bool(
            o,
            "strip_connection_headers_over_http2",
            def.strip_connection_headers_over_http2,
        )?,
        enforce_http1_connection_close: req_bool(
            o,
            "enforce_http1_connection_close",
            def.enforce_http1_connection_close,
        )?,
        auto_follow_redirections_max_hops,
        http1_connection_reuse: req_bool(
            o,
            "http1_connection_reuse",
            def.http1_connection_reuse,
        )?,
        http2_connection_reuse: req_bool(
            o,
            "http2_connection_reuse",
            def.http2_connection_reuse,
        )?,
        allow_http2_alpn_override: req_bool(
            o,
            "allow_http2_alpn_override",
            def.allow_http2_alpn_override,
        )?,
        process_cookies_in_redirects: req_bool(
            o,
            "process_cookies_in_redirects",
            def.process_cookies_in_redirects,
        )?,
        cross_domain_protocol_redirects: req_bool(
            o,
            "cross_domain_protocol_redirects",
            def.cross_domain_protocol_redirects,
        )?,
        streaming_response_timeout_secs,
    })
}

fn decode_upstream(o: &BTreeMap<String, JVal>) -> Result<UpstreamProxySettings, String> {
    check_known_keys(
        o,
        &[
            "protocol",
            "host",
            "port",
            "username",
            "password",
            "rules",
            "socks",
            "open_ended_response_timeout_secs",
            "hostname_resolutions",
            "override_project_only",
        ],
        "upstream_proxy",
    )?;
    let protocol = match opt_str(o, "protocol", "direct")?.as_str() {
        "direct" => UpstreamProxyProtocol::Direct,
        "http" => UpstreamProxyProtocol::Http,
        "https" => UpstreamProxyProtocol::Https,
        "socks5" => UpstreamProxyProtocol::Socks5,
        other => return Err(format!("未知上游代理协议：`{other}`。")),
    };
    let port_num = req_num(o, "port", 0)?;
    let port =
        u16::try_from(port_num).map_err(|_| "上游代理端口必须在 0 到 65535 之间。".to_owned())?;
    let rules = match o.get("rules") {
        None => Vec::new(),
        Some(JVal::Arr(arr)) => arr
            .iter()
            .map(|item| {
                let JVal::Obj(ro) = item else {
                    return Err("上游代理规则必须是对象。".into());
                };
                check_known_keys(
                    ro,
                    &[
                        "id",
                        "enabled",
                        "destination_host",
                        "protocol",
                        "host",
                        "port",
                        "auth_type",
                        "username",
                        "password",
                        "description",
                    ],
                    "上游代理规则",
                )?;
                let rule_proto = match opt_str(ro, "protocol", "http")?.as_str() {
                    "http" => UpstreamProxyProtocol::Http,
                    "https" => UpstreamProxyProtocol::Https,
                    other => return Err(format!("不支持的上游代理规则协议：`{other}`。")),
                };
                let auth_type = match opt_str(ro, "auth_type", "none")?.as_str() {
                    "none" => UpstreamAuthType::None,
                    "basic" => UpstreamAuthType::Basic,
                    other => return Err(format!("未知上游代理认证类型：`{other}`。")),
                };
                let r_port = u16::try_from(req_num(ro, "port", 8080)?)
                    .map_err(|_| "上游代理规则端口必须在 1 到 65535 之间。".to_owned())?;
                Ok(UpstreamProxyRule {
                    id: req_uuid(ro, "id")?,
                    enabled: req_bool(ro, "enabled", true)?,
                    destination_host: opt_str(ro, "destination_host", "*")?,
                    protocol: rule_proto,
                    host: opt_str(ro, "host", "")?,
                    port: r_port,
                    auth_type,
                    username: opt_str(ro, "username", "")?,
                    password: opt_str(ro, "password", "")?,
                    description: opt_str(ro, "description", "")?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?,
        _ => return Err("`upstream_proxy.rules` 必须是数组。".into()),
    };
    let socks = match o.get("socks") {
        None => SocksProxySettings::default(),
        Some(JVal::Obj(so)) => {
            check_known_keys(
                so,
                &[
                    "use_proxy",
                    "host",
                    "port",
                    "username",
                    "password",
                    "dns_over_socks",
                    "bypass_hosts",
                    "override_project_only",
                ],
                "upstream_proxy.socks",
            )?;
            let s_port = u16::try_from(req_num(so, "port", 0)?)
                .map_err(|_| "SOCKS 代理端口必须在 0 到 65535 之间。".to_owned())?;
            let bypass_hosts = match so.get("bypass_hosts") {
                None => Vec::new(),
                Some(JVal::Arr(arr)) => arr
                    .iter()
                    .map(|v| match v {
                        JVal::Str(s) => Ok(s.clone()),
                        _ => Err("`upstream_proxy.socks.bypass_hosts` 元素必须是字符串。".into()),
                    })
                    .collect::<Result<Vec<_>, String>>()?,
                _ => return Err("`upstream_proxy.socks.bypass_hosts` 必须是数组。".into()),
            };
            SocksProxySettings {
                use_proxy: req_bool(so, "use_proxy", false)?,
                host: opt_str(so, "host", "")?,
                port: s_port,
                username: opt_str(so, "username", "")?,
                password: opt_str(so, "password", "")?,
                dns_over_socks: req_bool(so, "dns_over_socks", true)?,
                bypass_hosts,
                override_project_only: req_bool(so, "override_project_only", false)?,
            }
        }
        _ => return Err("`upstream_proxy.socks` 必须是对象。".into()),
    };
    let open_ended_num = req_num(o, "open_ended_response_timeout_secs", 0)?;
    let open_ended_response_timeout_secs = u64::try_from(open_ended_num)
        .map_err(|_| "流式响应超时必须在 0 到 86400 秒之间。".to_owned())?;
    let hostname_resolutions = match o.get("hostname_resolutions") {
        None => Vec::new(),
        Some(JVal::Arr(arr)) => arr
            .iter()
            .map(|item| {
                let JVal::Obj(ho) = item else {
                    return Err("自定义主机名解析规则必须是对象。".into());
                };
                check_known_keys(
                    ho,
                    &["id", "enabled", "hostname", "ip_address"],
                    "自定义主机名解析规则",
                )?;
                Ok(HostnameResolutionRule {
                    id: req_uuid(ho, "id")?,
                    enabled: req_bool(ho, "enabled", true)?,
                    hostname: req_str(ho, "hostname")?,
                    ip_address: req_str(ho, "ip_address")?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?,
        _ => return Err("`upstream_proxy.hostname_resolutions` 必须是数组。".into()),
    };
    Ok(UpstreamProxySettings {
        protocol,
        host: opt_str(o, "host", "")?,
        port,
        username: opt_str(o, "username", "")?,
        password: opt_str(o, "password", "")?,
        rules,
        socks,
        open_ended_response_timeout_secs,
        hostname_resolutions,
        override_project_only: req_bool(o, "override_project_only", false)?,
    })
}

fn decode_scope_rules_array(val: &JVal, ctx: &str) -> Result<Vec<ScopeRule>, String> {
    let JVal::Arr(arr) = val else {
        return Err(format!("{ctx} 必须是数组。"));
    };
    arr.iter()
        .map(|item| {
            let JVal::Obj(ro) = item else {
                return Err("Scope 规则必须是对象。".into());
            };
            check_known_keys(
                ro,
                &["id", "enabled", "prefix", "include_subdomains"],
                "Scope 规则",
            )?;
            Ok(ScopeRule {
                id: req_uuid(ro, "id")?,
                enabled: req_bool(ro, "enabled", true)?,
                prefix: req_str(ro, "prefix")?,
                include_subdomains: req_bool(ro, "include_subdomains", false)?,
            })
        })
        .collect()
}

fn decode_scope(o: &BTreeMap<String, JVal>) -> Result<TargetScope, String> {
    check_known_keys(
        o,
        &[
            "include",
            "exclude",
            "advanced_mode",
            "advanced_include",
            "advanced_exclude",
            "drop_out_of_scope",
            "out_of_scope_use_custom",
            "out_of_scope_custom_rules",
        ],
        "scope",
    )?;
    let decode_simple = |key: &str| -> Result<Vec<ScopeRule>, String> {
        let Some(val) = o.get(key) else {
            return Ok(Vec::new());
        };
        decode_scope_rules_array(val, &format!("scope.{key}"))
    };
    let decode_adv = |key: &str| -> Result<Vec<AdvancedScopeRule>, String> {
        let Some(val) = o.get(key) else {
            return Ok(Vec::new());
        };
        let JVal::Arr(arr) = val else {
            return Err(format!("scope.{key} 必须是数组。"));
        };
        arr.iter()
            .map(|item| {
                let JVal::Obj(ro) = item else {
                    return Err("高级 Scope 规则必须是对象。".into());
                };
                check_known_keys(
                    ro,
                    &["id", "enabled", "protocol", "host", "port", "file"],
                    "高级 Scope 规则",
                )?;
                let protocol = match opt_str(ro, "protocol", "any")?.as_str() {
                    "any" => ScopeProtocol::Any,
                    "http" => ScopeProtocol::Http,
                    "https" => ScopeProtocol::Https,
                    other => return Err(format!("未知 Scope 协议：`{other}`。")),
                };
                Ok(AdvancedScopeRule {
                    id: req_uuid(ro, "id")?,
                    enabled: req_bool(ro, "enabled", true)?,
                    protocol,
                    host: opt_str(ro, "host", "")?,
                    port: opt_str(ro, "port", "")?,
                    file: opt_str(ro, "file", "")?,
                })
            })
            .collect()
    };
    Ok(TargetScope {
        include: decode_simple("include")?,
        exclude: decode_simple("exclude")?,
        advanced_mode: req_bool(o, "advanced_mode", false)?,
        advanced_include: decode_adv("advanced_include")?,
        advanced_exclude: decode_adv("advanced_exclude")?,
        drop_out_of_scope: req_bool(o, "drop_out_of_scope", false)?,
        out_of_scope_use_custom: req_bool(o, "out_of_scope_use_custom", false)?,
        out_of_scope_custom_rules: decode_simple("out_of_scope_custom_rules")?,
    })
}

fn decode_network_http(o: &BTreeMap<String, JVal>) -> Result<NetworkHttpSettings, String> {
    check_known_keys(
        o,
        &[
            "default_to_http2",
            "treat_event_stream_as_streaming",
            "store_streaming_responses",
            "strip_chunked_metadata_in_streaming",
            "understand_100_continue",
            "remove_100_continue_headers",
            "redirect_3xx",
            "redirect_refresh_header",
            "redirect_meta_refresh",
            "redirect_javascript",
            "redirect_any_location",
            "streaming_advanced_scope",
            "streaming_scope_rules",
        ],
        "network_http",
    )?;
    let def = NetworkHttpSettings::default();
    let streaming_scope_rules = match o.get("streaming_scope_rules") {
        None => Vec::new(),
        Some(val) => decode_scope_rules_array(val, "network_http.streaming_scope_rules")?,
    };
    Ok(NetworkHttpSettings {
        default_to_http2: req_bool(o, "default_to_http2", def.default_to_http2)?,
        treat_event_stream_as_streaming: req_bool(
            o,
            "treat_event_stream_as_streaming",
            def.treat_event_stream_as_streaming,
        )?,
        store_streaming_responses: req_bool(
            o,
            "store_streaming_responses",
            def.store_streaming_responses,
        )?,
        strip_chunked_metadata_in_streaming: req_bool(
            o,
            "strip_chunked_metadata_in_streaming",
            def.strip_chunked_metadata_in_streaming,
        )?,
        understand_100_continue: req_bool(
            o,
            "understand_100_continue",
            def.understand_100_continue,
        )?,
        remove_100_continue_headers: req_bool(
            o,
            "remove_100_continue_headers",
            def.remove_100_continue_headers,
        )?,
        redirect_3xx: req_bool(o, "redirect_3xx", def.redirect_3xx)?,
        redirect_refresh_header: req_bool(
            o,
            "redirect_refresh_header",
            def.redirect_refresh_header,
        )?,
        redirect_meta_refresh: req_bool(o, "redirect_meta_refresh", def.redirect_meta_refresh)?,
        redirect_javascript: req_bool(o, "redirect_javascript", def.redirect_javascript)?,
        redirect_any_location: req_bool(o, "redirect_any_location", def.redirect_any_location)?,
        streaming_advanced_scope: req_bool(
            o,
            "streaming_advanced_scope",
            def.streaming_advanced_scope,
        )?,
        streaming_scope_rules,
    })
}

fn decode_proxy_listener(item: &JVal) -> Result<ProxyListenerConfig, String> {
    let JVal::Obj(o) = item else {
        return Err("Proxy 监听器配置必须是对象。".into());
    };
    check_known_keys(
        o,
        &[
            "id",
            "enabled",
            "bind_address",
            "invisible_proxying",
            "redirect_to_host",
            "redirect_to_port",
            "force_use_of_tls",
            "certificate_mode",
            "specific_cert_hostname",
            "enable_http2",
            "tls_protocols_label",
            "custom_cert_pem",
            "custom_key_pem",
        ],
        "proxy_listener",
    )?;
    let redirect_to_port = match o.get("redirect_to_port") {
        None | Some(JVal::Null) => None,
        Some(JVal::Num(n)) => {
            let p = u16::try_from(*n)
                .ok()
                .filter(|p| *p > 0)
                .ok_or_else(|| "Proxy 监听器重定向端口必须在 1 到 65535 之间。".to_owned())?;
            Some(p)
        }
        _ => return Err("`redirect_to_port` 必须是整数或 null。".into()),
    };
    let certificate_mode = match opt_str(o, "certificate_mode", "per_host_ca_signed")?.as_str() {
        "per_host_ca_signed" => ProxyListenerCertMode::PerHostCaSigned,
        "self_signed" => ProxyListenerCertMode::SelfSigned,
        "specific_hostname" => ProxyListenerCertMode::SpecificHostname,
        "custom_certificate" => ProxyListenerCertMode::CustomCertificate,
        other => return Err(format!("未知 Proxy 监听器证书模式：`{other}`。")),
    };
    let opt_nullable_str = |key: &str| -> Result<Option<String>, String> {
        match o.get(key) {
            None | Some(JVal::Null) => Ok(None),
            Some(JVal::Str(s)) => Ok(Some(s.clone())),
            _ => Err(format!("字段 `{key}` 必须是字符串或 null。")),
        }
    };
    Ok(ProxyListenerConfig {
        id: req_uuid(o, "id")?,
        enabled: req_bool(o, "enabled", true)?,
        bind_address: req_str(o, "bind_address")?,
        invisible_proxying: req_bool(o, "invisible_proxying", false)?,
        redirect_to_host: opt_str(o, "redirect_to_host", "")?,
        redirect_to_port,
        force_use_of_tls: req_bool(o, "force_use_of_tls", false)?,
        certificate_mode,
        specific_cert_hostname: opt_str(o, "specific_cert_hostname", "")?,
        enable_http2: req_bool(o, "enable_http2", true)?,
        tls_protocols_label: opt_str(o, "tls_protocols_label", "")?,
        custom_cert_pem: opt_nullable_str("custom_cert_pem")?,
        custom_key_pem: opt_nullable_str("custom_key_pem")?,
    })
}

fn decode_client_certificate(item: &JVal) -> Result<ClientCertificateRule, String> {
    let JVal::Obj(o) = item else {
        return Err("客户端 TLS 证书规则必须是对象。".into());
    };
    check_known_keys(
        o,
        &[
            "enabled",
            "host_pattern",
            "format",
            "certificate_b64",
            "private_key_pem_b64",
            "password",
        ],
        "client_certificate",
    )?;
    let format = match opt_str(o, "format", "pem")?.as_str() {
        "pem" => ClientCertificateFormat::Pem,
        "pkcs12" => ClientCertificateFormat::Pkcs12,
        other => return Err(format!("未知客户端证书格式：`{other}`。")),
    };
    let certificate_bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        req_str(o, "certificate_b64")?,
    )
    .map_err(|_| "客户端证书 Base64 解码失败。".to_owned())?;
    let private_key_pem = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        opt_str(o, "private_key_pem_b64", "")?,
    )
    .map_err(|_| "客户端证书私钥 Base64 解码失败。".to_owned())?;
    Ok(ClientCertificateRule {
        enabled: req_bool(o, "enabled", true)?,
        host_pattern: req_str(o, "host_pattern")?,
        format,
        certificate_bytes,
        private_key_pem,
        password: opt_str(o, "password", "")?,
    })
}

fn decode_upstream_tls(o: &BTreeMap<String, JVal>) -> Result<UpstreamTlsSettings, String> {
    check_known_keys(
        o,
        &[
            "custom_ca_ders_b64",
            "include_system_roots",
            "insecure_skip_verify",
            "protocol_mode",
            "allow_unsafe_renegotiation",
            "disable_session_resume",
            "client_certs_override_project_only",
        ],
        "upstream_tls",
    )?;
    let def = UpstreamTlsSettings::default();
    let custom_ca_ders = match o.get("custom_ca_ders_b64") {
        None => Vec::new(),
        Some(JVal::Arr(arr)) => arr
            .iter()
            .map(|v| match v {
                JVal::Str(s) => base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    s,
                )
                .map_err(|_| "上游 TLS 自定义根证书 Base64 解码失败。".to_owned()),
                _ => Err("`upstream_tls.custom_ca_ders_b64` 元素必须是字符串。".into()),
            })
            .collect::<Result<Vec<_>, String>>()?,
        _ => return Err("`upstream_tls.custom_ca_ders_b64` 必须是数组。".into()),
    };
    let protocol_mode = match opt_str(o, "protocol_mode", "all_supported")?.as_str() {
        "all_supported" => TlsProtocolMode::AllSupported,
        "default" => TlsProtocolMode::Default,
        "custom" => TlsProtocolMode::Custom,
        other => return Err(format!("未知上游 TLS 协议模式：`{other}`。")),
    };
    Ok(UpstreamTlsSettings {
        custom_ca_ders,
        include_system_roots: req_bool(o, "include_system_roots", def.include_system_roots)?,
        insecure_skip_verify: req_bool(o, "insecure_skip_verify", def.insecure_skip_verify)?,
        protocol_mode,
        allow_unsafe_renegotiation: req_bool(
            o,
            "allow_unsafe_renegotiation",
            def.allow_unsafe_renegotiation,
        )?,
        disable_session_resume: req_bool(
            o,
            "disable_session_resume",
            def.disable_session_resume,
        )?,
        client_certs_override_project_only: req_bool(
            o,
            "client_certs_override_project_only",
            def.client_certs_override_project_only,
        )?,
    })
}

fn decode_scheduled_task(item: &JVal) -> Result<ScheduledTask, String> {
    let JVal::Obj(o) = item else {
        return Err("定时任务配置必须是对象。".into());
    };
    check_known_keys(
        o,
        &["id", "enabled", "action", "trigger_spec", "description"],
        "scheduled_task",
    )?;
    let action = match opt_str(o, "action", "pause_tasks")?.as_str() {
        "pause_tasks" => ScheduledTaskAction::PauseTasks,
        "resume_tasks" => ScheduledTaskAction::ResumeTasks,
        "run_scan" => ScheduledTaskAction::RunScan,
        other => return Err(format!("未知定时任务动作：`{other}`。")),
    };
    Ok(ScheduledTask {
        id: req_uuid(o, "id")?,
        enabled: req_bool(o, "enabled", true)?,
        action,
        trigger_spec: opt_str(o, "trigger_spec", "")?,
        description: opt_str(o, "description", "")?,
    })
}

fn decode_mr_rule(item: &JVal) -> Result<MatchAndReplaceRule, String> {
    let JVal::Obj(o) = item else {
        return Err("Match & Replace 规则必须是对象。".into());
    };
    check_known_keys(
        o,
        &[
            "id",
            "enabled",
            "item_type",
            "match_pattern",
            "replace_with",
            "regex",
            "comment",
            "script_name",
            "filter_script",
        ],
        "match_and_replace_rule",
    )?;
    let item_type = match req_str(o, "item_type")?.as_str() {
        "request_header" => MatchReplaceTarget::RequestHeader,
        "request_body" => MatchReplaceTarget::RequestBody,
        "response_header" => MatchReplaceTarget::ResponseHeader,
        "response_body" => MatchReplaceTarget::ResponseBody,
        "request_param_name" => MatchReplaceTarget::RequestParamName,
        "request_param_value" => MatchReplaceTarget::RequestParamValue,
        "request_first_line" => MatchReplaceTarget::RequestFirstLine,
        "request_script" => MatchReplaceTarget::RequestScript,
        "response_script" => MatchReplaceTarget::ResponseScript,
        other => return Err(format!("未知 Match & Replace 类型：`{other}`。")),
    };
    Ok(MatchAndReplaceRule {
        id: req_uuid(o, "id")?,
        enabled: req_bool(o, "enabled", true)?,
        item_type,
        match_pattern: opt_str(o, "match_pattern", "")?,
        replace_with: opt_str(o, "replace_with", "")?,
        regex: req_bool(o, "regex", false)?,
        comment: opt_str(o, "comment", "")?,
        script_name: opt_str(o, "script_name", "")?,
        filter_script: opt_str(o, "filter_script", "")?,
    })
}

fn decode_proxy_interception(
    o: &BTreeMap<String, JVal>,
) -> Result<ProxyInterceptionSettings, String> {
    check_known_keys(o, &["request", "response"], "proxy_interception")?;
    let def = ProxyInterceptionSettings::default();
    let request = match o.get("request") {
        None => def.request,
        Some(JVal::Obj(ro)) => decode_intercept_set(ro)?,
        _ => return Err("`proxy_interception.request` 必须是对象。".into()),
    };
    let response = match o.get("response") {
        None => def.response,
        Some(JVal::Obj(ro)) => decode_intercept_set(ro)?,
        _ => return Err("`proxy_interception.response` 必须是对象。".into()),
    };
    Ok(ProxyInterceptionSettings { request, response })
}

fn decode_intercept_set(o: &BTreeMap<String, JVal>) -> Result<ProxyInterceptRuleSet, String> {
    check_known_keys(o, &["enabled", "rules"], "proxy_intercept_set")?;
    let enabled = req_bool(o, "enabled", false)?;
    let rules = match o.get("rules") {
        None => Vec::new(),
        Some(JVal::Arr(arr)) => arr
            .iter()
            .map(|item| {
                let JVal::Obj(ro) = item else {
                    return Err("Proxy 拦截规则必须是对象。".into());
                };
                check_known_keys(
                    ro,
                    &[
                        "id",
                        "enabled",
                        "operator",
                        "match_type",
                        "relationship",
                        "condition",
                    ],
                    "Proxy 拦截规则",
                )?;
                let operator = match req_str(ro, "operator")?.as_str() {
                    "And" | "and" => ProxyInterceptOperator::And,
                    "Or" | "or" => ProxyInterceptOperator::Or,
                    other => return Err(format!("未知拦截规则逻辑运算符：`{other}`。")),
                };
                let match_type = parse_intercept_type(&req_str(ro, "match_type")?)?;
                let relationship = parse_intercept_rel(&req_str(ro, "relationship")?)?;
                Ok(ProxyInterceptRule {
                    id: req_uuid(ro, "id")?,
                    enabled: req_bool(ro, "enabled", true)?,
                    operator,
                    match_type,
                    relationship,
                    condition: opt_str(ro, "condition", "")?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("`rules` 必须是数组。".into()),
    };
    Ok(ProxyInterceptRuleSet { enabled, rules })
}

fn parse_intercept_type(s: &str) -> Result<ProxyInterceptType, String> {
    use ProxyInterceptType::*;
    Ok(match s {
        "DomainName" => DomainName,
        "IpAddress" => IpAddress,
        "Protocol" => Protocol,
        "HttpMethod" => HttpMethod,
        "FileExtension" => FileExtension,
        "Url" => Url,
        "Header" => Header,
        "Request" => Request,
        "CookieName" => CookieName,
        "CookieValue" => CookieValue,
        "Body" => Body,
        "ParamName" => ParamName,
        "ParamValue" => ParamValue,
        "ContentTypeHeader" => ContentTypeHeader,
        "MimeType" => MimeType,
        "PageTitle" => PageTitle,
        "StatusCode" => StatusCode,
        "ListenerPort" => ListenerPort,
        other => return Err(format!("未知拦截匹配类型：`{other}`。")),
    })
}

fn parse_intercept_rel(s: &str) -> Result<ProxyInterceptRelation, String> {
    use ProxyInterceptRelation::*;
    Ok(match s {
        "Matches" => Matches,
        "DoesNotMatch" => DoesNotMatch,
        "RequestMatches" => RequestMatches,
        "RequestDoesNotMatch" => RequestDoesNotMatch,
        "ResponseMatches" => ResponseMatches,
        "ResponseDoesNotMatch" => ResponseDoesNotMatch,
        "IsHttp" => IsHttp,
        "IsHttps" => IsHttps,
        "ContainsParameters" => ContainsParameters,
        "DoesNotContainParameters" => DoesNotContainParameters,
        "WasModified" => WasModified,
        "WasNotModified" => WasNotModified,
        "WasIntercepted" => WasIntercepted,
        "WasNotIntercepted" => WasNotIntercepted,
        "WasAnnotated" => WasAnnotated,
        "WasNotAnnotated" => WasNotAnnotated,
        "InScope" => InScope,
        "NotInScope" => NotInScope,
        other => return Err(format!("未知拦截关系条件：`{other}`。")),
    })
}

fn decode_proxy_messages(o: &BTreeMap<String, JVal>) -> Result<ProxyMessageSettings, String> {
    check_known_keys(
        o,
        &[
            "unpack_compressed_requests",
            "unpack_compressed_responses",
            "match_replace_in_scope_only",
            "ws_match_replace_in_scope_only",
            "auto_update_request_content_length",
            "auto_fix_request_newlines",
            "auto_update_response_content_length",
            "ssl_pass_through_auto_add_on_failure",
            "response_modification",
            "misc",
        ],
        "proxy_messages",
    )?;
    let def = ProxyMessageSettings::default();
    let response_modification = match o.get("response_modification") {
        None => ProxyResponseModificationSettings::default(),
        Some(JVal::Obj(rm)) => {
            check_known_keys(
                rm,
                &[
                    "unhide_hidden_form_fields",
                    "enable_disabled_form_fields",
                    "highlight_unhidden_fields",
                    "remove_input_field_length_limits",
                    "remove_javascript_form_validation",
                    "remove_all_javascript",
                    "remove_object_tags",
                    "convert_https_links_to_http",
                    "remove_secure_flag_from_cookies",
                ],
                "response_modification",
            )?;
            ProxyResponseModificationSettings {
                unhide_hidden_form_fields: req_bool(rm, "unhide_hidden_form_fields", false)?,
                enable_disabled_form_fields: req_bool(rm, "enable_disabled_form_fields", false)?,
                highlight_unhidden_fields: req_bool(rm, "highlight_unhidden_fields", false)?,
                remove_input_field_length_limits: req_bool(
                    rm,
                    "remove_input_field_length_limits",
                    false,
                )?,
                remove_javascript_form_validation: req_bool(
                    rm,
                    "remove_javascript_form_validation",
                    false,
                )?,
                remove_all_javascript: req_bool(rm, "remove_all_javascript", false)?,
                remove_object_tags: req_bool(rm, "remove_object_tags", false)?,
                convert_https_links_to_http: req_bool(rm, "convert_https_links_to_http", false)?,
                remove_secure_flag_from_cookies: req_bool(
                    rm,
                    "remove_secure_flag_from_cookies",
                    false,
                )?,
            }
        }
        _ => return Err("`response_modification` 必须是对象。".into()),
    };
    let misc = match o.get("misc") {
        None => ProxyMiscSettings::default(),
        Some(JVal::Obj(mo)) => {
            check_known_keys(
                mo,
                &[
                    "use_http10_in_requests",
                    "use_http10_in_responses",
                    "set_response_connection_close",
                    "set_request_connection_header",
                    "strip_proxy_headers",
                    "remove_unsupported_accept_encodings",
                    "strip_sec_websocket_extensions",
                    "suppress_browser_error_messages",
                    "disable_history_logging",
                    "disable_out_of_scope_history_logging",
                    "use_http1_keep_alive",
                    "disable_web_interface",
                ],
                "proxy_messages.misc",
            )?;
            let mdef = ProxyMiscSettings::default();
            ProxyMiscSettings {
                use_http10_in_requests: req_bool(
                    mo,
                    "use_http10_in_requests",
                    mdef.use_http10_in_requests,
                )?,
                use_http10_in_responses: req_bool(
                    mo,
                    "use_http10_in_responses",
                    mdef.use_http10_in_responses,
                )?,
                set_response_connection_close: req_bool(
                    mo,
                    "set_response_connection_close",
                    mdef.set_response_connection_close,
                )?,
                set_request_connection_header: req_bool(
                    mo,
                    "set_request_connection_header",
                    mdef.set_request_connection_header,
                )?,
                strip_proxy_headers: req_bool(
                    mo,
                    "strip_proxy_headers",
                    mdef.strip_proxy_headers,
                )?,
                remove_unsupported_accept_encodings: req_bool(
                    mo,
                    "remove_unsupported_accept_encodings",
                    mdef.remove_unsupported_accept_encodings,
                )?,
                strip_sec_websocket_extensions: req_bool(
                    mo,
                    "strip_sec_websocket_extensions",
                    mdef.strip_sec_websocket_extensions,
                )?,
                suppress_browser_error_messages: req_bool(
                    mo,
                    "suppress_browser_error_messages",
                    mdef.suppress_browser_error_messages,
                )?,
                disable_history_logging: req_bool(
                    mo,
                    "disable_history_logging",
                    mdef.disable_history_logging,
                )?,
                disable_out_of_scope_history_logging: req_bool(
                    mo,
                    "disable_out_of_scope_history_logging",
                    mdef.disable_out_of_scope_history_logging,
                )?,
                use_http1_keep_alive: req_bool(
                    mo,
                    "use_http1_keep_alive",
                    mdef.use_http1_keep_alive,
                )?,
                disable_web_interface: req_bool(
                    mo,
                    "disable_web_interface",
                    mdef.disable_web_interface,
                )?,
            }
        }
        _ => return Err("`proxy_messages.misc` 必须是对象。".into()),
    };
    Ok(ProxyMessageSettings {
        unpack_compressed_requests: req_bool(
            o,
            "unpack_compressed_requests",
            def.unpack_compressed_requests,
        )?,
        unpack_compressed_responses: req_bool(
            o,
            "unpack_compressed_responses",
            def.unpack_compressed_responses,
        )?,
        match_replace_in_scope_only: req_bool(
            o,
            "match_replace_in_scope_only",
            def.match_replace_in_scope_only,
        )?,
        ws_match_replace_in_scope_only: req_bool(
            o,
            "ws_match_replace_in_scope_only",
            def.ws_match_replace_in_scope_only,
        )?,
        auto_update_request_content_length: req_bool(
            o,
            "auto_update_request_content_length",
            def.auto_update_request_content_length,
        )?,
        auto_fix_request_newlines: req_bool(
            o,
            "auto_fix_request_newlines",
            def.auto_fix_request_newlines,
        )?,
        auto_update_response_content_length: req_bool(
            o,
            "auto_update_response_content_length",
            def.auto_update_response_content_length,
        )?,
        ssl_pass_through_auto_add_on_failure: req_bool(
            o,
            "ssl_pass_through_auto_add_on_failure",
            def.ssl_pass_through_auto_add_on_failure,
        )?,
        response_modification,
        misc,
    })
}

fn decode_ws_interception(
    o: &BTreeMap<String, JVal>,
) -> Result<ProxyWebSocketInterceptionSettings, String> {
    check_known_keys(
        o,
        &[
            "client_to_server_messages",
            "server_to_client_messages",
            "intercept_in_scope_only",
        ],
        "proxy_websocket_interception",
    )?;
    let def = ProxyWebSocketInterceptionSettings::default();
    Ok(ProxyWebSocketInterceptionSettings {
        client_to_server_messages: req_bool(
            o,
            "client_to_server_messages",
            def.client_to_server_messages,
        )?,
        server_to_client_messages: req_bool(
            o,
            "server_to_client_messages",
            def.server_to_client_messages,
        )?,
        intercept_in_scope_only: req_bool(
            o,
            "intercept_in_scope_only",
            def.intercept_in_scope_only,
        )?,
    })
}
