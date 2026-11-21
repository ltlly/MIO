use super::*;
use std::{net::IpAddr, sync::Arc, time::Duration};

fn is_valid_dns_hostname(hostname: &str) -> bool {
    let clean = hostname.trim().trim_end_matches('.');
    if clean.is_empty() || clean.len() > 253 || clean.parse::<IpAddr>().is_ok() {
        return false;
    }
    clean.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

impl UpstreamProxyRule {
    pub fn validate_destination_host(pattern: &str) -> Result<String> {
        let trimmed = pattern.trim();
        ensure!(!trimmed.is_empty(), "上游代理规则目标主机不能为空");
        if trimmed == "*" {
            return Ok("*".into());
        }
        mio_proxy::ClientCertificateRule::validate_host_pattern(trimmed)
            .map_err(|err| anyhow::anyhow!("上游代理规则目标主机无效：{err}"))
    }

    pub fn matches_destination(&self, host: &str) -> bool {
        if !self.enabled {
            return false;
        }
        let pat = self.destination_host.trim();
        if pat == "*" {
            return true;
        }
        mio_proxy::matches_host(pat, host)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(!self.id.is_nil(), "上游代理规则 ID 无效");
        Self::validate_destination_host(&self.destination_host)?;
        ensure!(
            matches!(
                self.protocol,
                UpstreamProxyProtocol::Http | UpstreamProxyProtocol::Https
            ),
            "上游代理规则列表仅支持 HTTP 或 HTTPS 代理协议"
        );
        ensure!(
            self.host.len() <= 253
                && self.username.len() <= 4096
                && self.password.len() <= 4096
                && self.domain.len() <= 253
                && self.workstation.len() <= 253
                && self.description.len() <= 1024
                && !self
                    .host
                    .bytes()
                    .chain(self.username.bytes())
                    .chain(self.password.bytes())
                    .chain(self.domain.bytes())
                    .chain(self.workstation.bytes())
                    .chain(self.description.bytes())
                    .any(|b| matches!(b, b'\r' | b'\n' | 0)),
            "上游代理规则包含控制字符或超过长度上限"
        );
        let _ = self.to_route_with_tls(mio_proxy::UpstreamTls::System)?;
        Ok(())
    }

    pub fn to_route_with_tls(
        &self,
        tls: mio_proxy::UpstreamTls,
    ) -> Result<mio_proxy::UpstreamRoute> {
        let host = self.host.trim();
        ensure!(
            !host.is_empty() && self.port > 0,
            "上游代理规则必须提供有效的主机名和端口"
        );
        ensure!(
            !host.contains("://") && !host.contains(['/', '?', '#', '@', ' ']),
            "上游代理主机必须是纯主机名或 IP，不能包含 URL 或凭据"
        );
        validate_proxy_host_syntax(host)?;
        let credentials = match self.auth_type {
            UpstreamAuthType::None => None,
            UpstreamAuthType::Basic => {
                ensure!(!self.username.is_empty(), "HTTP/HTTPS 代理 Basic 认证必须填写用户名");
                ensure!(
                    !self.username.contains(':'),
                    "HTTP/HTTPS 代理认证用户名不能包含冒号"
                );
                Some(mio_proxy::ProxyCredentials {
                    username: self.username.clone(),
                    password: self.password.clone(),
                })
            }
            UpstreamAuthType::Digest | UpstreamAuthType::NtlmV1 | UpstreamAuthType::NtlmV2 => {
                ensure!(
                    !self.username.is_empty(),
                    "HTTP/HTTPS 代理 {} 认证必须填写用户名",
                    self.auth_type.label()
                );
                None
            }
        };
        let endpoint = mio_proxy::ProxyEndpoint {
            host: host.to_owned(),
            port: self.port,
            credentials,
        };
        let route = match self.protocol {
            UpstreamProxyProtocol::Http => mio_proxy::UpstreamRoute::Http(endpoint),
            UpstreamProxyProtocol::Https => mio_proxy::UpstreamRoute::Https { endpoint, tls },
            UpstreamProxyProtocol::Direct | UpstreamProxyProtocol::Socks5 => {
                anyhow::bail!("上游代理规则协议不支持：{:?}", self.protocol)
            }
        };
        Ok(route)
    }
}

fn validate_proxy_host_syntax(host: &str) -> Result<()> {
    let clean = host.trim().trim_matches(['[', ']']);
    ensure!(
        clean.parse::<IpAddr>().is_ok() || is_valid_dns_hostname(clean),
        "代理服务器主机名或 IP 地址格式无效：{host}"
    );
    Ok(())
}

impl SocksProxySettings {
    pub fn should_bypass_host(&self, host: &str) -> bool {
        let clean = host.trim().trim_matches(['[', ']']);
        if clean.is_empty() {
            return false;
        }
        self.bypass_hosts.iter().any(|pattern| {
            let pat = pattern.trim();
            !pat.is_empty() && (pat == "*" || mio_proxy::matches_host(pat, clean))
        })
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.host.len() <= 253
                && self.username.len() <= 255
                && self.password.len() <= 255
                && !self
                    .host
                    .bytes()
                    .chain(self.username.bytes())
                    .chain(self.password.bytes())
                    .any(|b| matches!(b, b'\r' | b'\n' | 0)),
            "SOCKS5 代理配置包含控制字符或超过长度上限"
        );
        ensure!(
            self.bypass_hosts.len() <= 128,
            "SOCKS5 代理绕过主机数量超过 128 条上限"
        );
        for host in &self.bypass_hosts {
            let trimmed = host.trim();
            ensure!(
                !trimmed.is_empty()
                    && host.len() <= 253
                    && !host.chars().any(char::is_control),
                "SOCKS5 代理绕过主机不能为空、包含控制字符或超过 253 字节"
            );
        }
        if self.use_proxy {
            let _ = self.to_route()?;
        } else if !self.host.trim().is_empty() {
            validate_proxy_host_syntax(&self.host)?;
        }
        Ok(())
    }

    pub fn to_route(&self) -> Result<mio_proxy::UpstreamRoute> {
        if !self.use_proxy {
            return Ok(mio_proxy::UpstreamRoute::Direct);
        }
        let host = self.host.trim();
        ensure!(
            !host.is_empty() && self.port > 0,
            "启用 SOCKS5 代理时必须提供有效的主机名和端口"
        );
        validate_proxy_host_syntax(host)?;
        let credentials = if self.username.is_empty() && self.password.is_empty() {
            None
        } else {
            ensure!(
                !self.username.is_empty()
                    && !self.password.is_empty()
                    && self.username.len() <= 255
                    && self.password.len() <= 255,
                "SOCKS5 认证用户名与密码不能为空且长度不能超过 255 字节"
            );
            Some(mio_proxy::ProxyCredentials {
                username: self.username.clone(),
                password: self.password.clone(),
            })
        };
        let endpoint = mio_proxy::ProxyEndpoint {
            host: host.to_owned(),
            port: self.port,
            credentials,
        };
        Ok(if !self.bypass_hosts.is_empty() {
            mio_proxy::UpstreamRoute::socks5_with_bypass(
                endpoint,
                !self.dns_over_socks,
                self.bypass_hosts.clone(),
            )
        } else if self.dns_over_socks {
            mio_proxy::UpstreamRoute::Socks5(endpoint)
        } else {
            mio_proxy::UpstreamRoute::Socks5LocalDns(endpoint)
        })
    }
}

impl HostnameResolutionRule {
    pub fn validate(&self) -> Result<(String, IpAddr)> {
        ensure!(!self.id.is_nil(), "主机名解析覆盖规则 ID 无效");
        let clean_host = self.hostname.trim().trim_end_matches('.').to_ascii_lowercase();
        ensure!(
            is_valid_dns_hostname(&clean_host),
            "主机名解析覆盖的 Hostname 必须是合法域名（不能是 IP 或通配符）：{}",
            self.hostname
        );
        let ip: IpAddr = self
            .ip_address
            .trim()
            .trim_matches(['[', ']'])
            .parse()
            .with_context(|| format!("主机名解析覆盖的 IP 地址无效：{}", self.ip_address))?;
        Ok((clean_host, ip))
    }
}

impl UpstreamProxySettings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.open_ended_response_timeout_secs <= 86_400,
            "流式响应超时必须在 0 到 86400 秒之间"
        );
        ensure!(
            self.rules.len() <= 64,
            "上游代理服务器规则数量超过 64 条上限"
        );
        let mut rule_ids = HashSet::new();
        for rule in &self.rules {
            ensure!(rule_ids.insert(rule.id), "上游代理规则 ID 重复");
            rule.validate()?;
        }
        self.socks.validate()?;
        ensure!(
            self.hostname_resolutions.len() <= 128,
            "自定义主机名解析规则数量超过 128 条上限"
        );
        let mut host_ids = HashSet::new();
        let mut enabled_hosts = HashSet::new();
        for entry in &self.hostname_resolutions {
            ensure!(host_ids.insert(entry.id), "自定义主机名解析规则 ID 重复");
            let (norm_host, _) = entry.validate()?;
            if entry.enabled {
                ensure!(
                    enabled_hosts.insert(norm_host.clone()),
                    "启用的自定义主机名解析存在重复域名：{norm_host}"
                );
            }
        }
        ensure!(
            self.platform_auth.rules.len() <= 64,
            "平台认证规则数量超过 64 条上限"
        );
        let mut platform_auth_ids = HashSet::new();
        for rule in &self.platform_auth.rules {
            ensure!(platform_auth_ids.insert(rule.id), "平台认证规则 ID 重复");
            rule.validate()?;
        }
        let _ = self.to_route()?;
        Ok(())
    }

    pub fn compiled_hostname_overrides(&self) -> Result<Arc<[(String, IpAddr)]>> {
        let mut out = Vec::new();
        for entry in &self.hostname_resolutions {
            let (host, ip) = entry.validate()?;
            if entry.enabled {
                out.push((host, ip));
            }
        }
        let _ = mio_proxy::build_dns_resolver(&out)?;
        Ok(Arc::from(out))
    }

    pub fn effective_streaming_timeout(&self) -> Option<Duration> {
        (self.open_ended_response_timeout_secs > 0)
            .then(|| Duration::from_secs(self.open_ended_response_timeout_secs.min(86_400)))
    }

    pub fn route_for_host_with_tls(
        &self,
        destination_host: Option<&str>,
        tls: mio_proxy::UpstreamTls,
    ) -> Result<mio_proxy::UpstreamRoute> {
        let socks = &self.socks;
        if socks.use_proxy {
            if let Some(host) = destination_host
                && socks.should_bypass_host(host)
            {
                return Ok(mio_proxy::UpstreamRoute::Direct);
            }
            return socks.to_route();
        }
        let rules = &self.rules;
        if !rules.is_empty() {
            if let Some(host) = destination_host {
                let clean = host.trim().trim_matches(['[', ']']);
                if let Some(matched) = rules.iter().find(|rule| rule.matches_destination(clean)) {
                    return matched.to_route_with_tls(tls);
                }
            } else if let Some(wildcard) = rules
                .iter()
                .find(|rule| rule.enabled && rule.destination_host.trim() == "*")
            {
                return wildcard.to_route_with_tls(tls);
            }
        }
        Ok(mio_proxy::UpstreamRoute::Direct)
    }

    pub fn route_for_url_with_tls(
        &self,
        url: &str,
        tls: mio_proxy::UpstreamTls,
    ) -> Result<mio_proxy::UpstreamRoute> {
        let parsed_host = url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned));
        self.route_for_host_with_tls(parsed_host.as_deref(), tls)
    }

    pub fn build_route_resolver(
        &self,
        tls: mio_proxy::UpstreamTls,
    ) -> Result<mio_proxy::UpstreamRouteResolver> {
        if self.socks.use_proxy {
            let socks = self.socks.clone();
            let socks_route = socks.to_route()?;
            if socks.bypass_hosts.is_empty() {
                return Ok(Arc::new(move |_| socks_route.clone()));
            }
            return Ok(Arc::new(move |host: &str| {
                if socks.should_bypass_host(host) {
                    mio_proxy::UpstreamRoute::Direct
                } else {
                    socks_route.clone()
                }
            }));
        }
        let mut compiled_rules = Vec::new();
        for rule in &self.rules {
            if rule.enabled {
                let route = rule.to_route_with_tls(tls.clone())?;
                compiled_rules.push((rule.clone(), route));
            }
        }
        if !compiled_rules.is_empty() {
            return Ok(Arc::new(move |host: &str| {
                let clean = host.trim().trim_matches(['[', ']']);
                for (rule, route) in &compiled_rules {
                    if rule.matches_destination(clean) {
                        return route.clone();
                    }
                }
                mio_proxy::UpstreamRoute::Direct
            }));
        }
        Ok(Arc::new(|_| mio_proxy::UpstreamRoute::Direct))
    }

    pub fn to_route(&self) -> Result<mio_proxy::UpstreamRoute> {
        self.to_route_with_tls(mio_proxy::UpstreamTls::System)
    }

    pub fn to_route_with_tls(
        &self,
        tls: mio_proxy::UpstreamTls,
    ) -> Result<mio_proxy::UpstreamRoute> {
        self.route_for_host_with_tls(None, tls)
    }
}

impl Workspace {
    pub fn validate(&self) -> Result<()> {
        self.validate_fields(true)
    }

    pub fn validate_structure(&self) -> Result<()> {
        self.validate_fields(false)
    }

    fn validate_fields(&self, validate_certs: bool) -> Result<()> {
        if let Some(name) = &self.project_name {
            ensure!(
                !name.trim().is_empty() && name.len() <= 256 && !name.chars().any(char::is_control),
                "项目名称不能为空、包含控制字符或超过 256 字节"
            );
        }
        // Temporary validation retained for `store/` tests until `proxy_listener` is dropped.
        if let Some(listener) = &self.proxy_listener {
            ensure!(
                !listener.is_empty() && listener.len() <= 128,
                "Proxy 监听地址不能为空或超过 128 字节"
            );
            let addr: std::net::SocketAddr = listener
                .parse()
                .context("Proxy 监听地址格式无效，须为 IP:端口")?;
            ensure!(
                addr.ip().is_loopback(),
                "Proxy 监听地址仅允许 loopback 地址（如 127.0.0.1:8080）"
            );
        }
        ensure!(
            self.proxy_listeners.len() <= 32,
            "Proxy 监听器数量超过 32 个上限"
        );
        let mut listener_ids = HashSet::new();
        let mut enabled_listen_addrs = HashSet::new();
        for listener in &self.proxy_listeners {
            ensure!(
                !listener.id.is_nil() && listener_ids.insert(listener.id),
                "Proxy 监听器 ID 无效或重复"
            );
            ensure!(
                !listener.bind_address.is_empty() && listener.bind_address.len() <= 128,
                "Proxy 监听地址不能为空或超过 128 字节"
            );
            let addr: std::net::SocketAddr = listener
                .bind_address
                .parse()
                .context("Proxy 监听地址格式无效，须为 IP:端口")?;
            ensure!(
                addr.ip().is_loopback(),
                "Proxy 监听地址仅允许 loopback 地址（如 127.0.0.1:8080）"
            );
            if listener.enabled && addr.port() != 0 {
                ensure!(
                    enabled_listen_addrs.insert(addr),
                    "启用的 Proxy 监听地址重复：{addr}"
                );
            }
            if let Some(cert_pem) = &listener.custom_cert_pem {
                ensure!(
                    cert_pem.len() <= mio_proxy::MAX_CLIENT_CERT_BYTES && !cert_pem.contains('\0'),
                    "Proxy 监听器自定义证书 PEM 超过大小上限或包含 NUL"
                );
            }
            if let Some(key_pem) = &listener.custom_key_pem {
                ensure!(
                    key_pem.len() <= mio_proxy::MAX_CLIENT_CERT_BYTES && !key_pem.contains('\0'),
                    "Proxy 监听器自定义私钥 PEM 超过大小上限或包含 NUL"
                );
            }
        }
        ensure!(
            self.client_certificates.len() <= 64,
            "客户端证书规则数量超过 64 条上限"
        );
        for rule in &self.client_certificates {
            mio_proxy::ClientCertificateRule::validate_host_pattern(&rule.host_pattern)?;
            ensure!(
                !rule.certificate_bytes.is_empty()
                    && rule.certificate_bytes.len() <= mio_proxy::MAX_CLIENT_CERT_BYTES,
                "客户端证书数据须为非空且不超过 {} 字节",
                mio_proxy::MAX_CLIENT_CERT_BYTES
            );
            ensure!(
                rule.private_key_pem.len() <= mio_proxy::MAX_CLIENT_CERT_BYTES,
                "客户端 PEM 私钥数据不能超过 {} 字节",
                mio_proxy::MAX_CLIENT_CERT_BYTES
            );
            ensure!(
                rule.password.len() <= mio_proxy::MAX_CLIENT_CERT_PASSWORD_BYTES
                    && !rule.password.contains(['\r', '\n', '\0']),
                "客户端证书口令超过 {} 字节或包含非法换行/NUL",
                mio_proxy::MAX_CLIENT_CERT_PASSWORD_BYTES
            );
            if validate_certs && rule.format == mio_proxy::ClientCertificateFormat::Pem {
                mio_proxy::ParsedClientIdentity::from_pem(
                    &rule.certificate_bytes,
                    &rule.private_key_pem,
                )?;
            }
        }
        ensure!(
            (1..=3600).contains(&self.repeater_settings.request_timeout_secs),
            "Repeater 请求超时必须在 1 到 3600 秒之间"
        );
        ensure!(
            self.repeater_settings.streaming_response_timeout_secs <= 86_400,
            "Repeater 流式响应超时必须在 0 到 86400 秒之间"
        );
        ensure!(
            self.network_http.streaming_scope_rules.len() <= 256,
            "HTTP 流式响应范围规则数量超过 256 条上限"
        );
        let mut streaming_rule_ids = HashSet::new();
        for rule in &self.network_http.streaming_scope_rules {
            ensure!(
                !rule.id.is_nil() && streaming_rule_ids.insert(rule.id),
                "HTTP 流式响应范围规则 ID 无效或重复"
            );
            let trimmed = rule.prefix.trim();
            ensure!(
                !trimmed.is_empty()
                    && rule.prefix.len() <= 2048
                    && !rule.prefix.chars().any(char::is_control),
                "HTTP 流式响应范围规则前缀不能为空、包含控制字符或超过 2048 字节"
            );
        }
        self.upstream_proxy.validate()?;
        self.upstream_tls.validate()?;
        self.scope.validate()?;
        self.sessions.validate()?;
        if validate_certs {
            self.proxy_interception.validate()?;
        } else {
            crate::http::validate_proxy_interception_shape(&self.proxy_interception)?;
        }
        ensure!(
            self.match_and_replace_rules.len() <= 256,
            "Match and replace 规则数量超过 256 条上限"
        );
        let mut rule_ids = HashSet::new();
        for rule in &self.match_and_replace_rules {
            ensure!(
                !rule.id.is_nil() && rule_ids.insert(rule.id),
                "Match and replace 规则 ID 无效或重复"
            );
            ensure!(
                rule.match_pattern.len() <= 4096
                    && rule.replace_with.len() <= 4096
                    && rule.comment.len() <= 1024
                    && rule.script_name.len() <= 256
                    && rule.filter_script.len() <= 64 * 1024
                    && !rule.replace_with.contains(['\r', '\n', '\0']),
                "Match and replace 规则文本过长或包含非法换行/NUL"
            );
            if rule.item_type.is_script() {
                crate::http::validate_filter_script(&rule.filter_script)?;
            }
            if rule.regex && !rule.match_pattern.is_empty() {
                regex::RegexBuilder::new(&rule.match_pattern)
                    .size_limit(1024 * 1024)
                    .build()
                    .map_err(|e| anyhow::anyhow!("Match and replace 正则表达式无效：{e}"))?;
            }
        }
        crate::http::validate_ssl_and_ws_rules(
            &self.ssl_pass_through_rules,
            &self.ws_match_and_replace_rules,
        )?;
        ensure!(self.tab_count() <= 512, "工作标签超过 512 个限额");
        ensure!(
            self.groups.len() + self.intruder.groups.len() <= 512,
            "工作组过多"
        );
        let groups = validate_groups(&self.groups, self.tab_sidebar_width)?;
        let intruder_groups =
            validate_groups(&self.intruder.groups, self.intruder.tab_sidebar_width)?;
        ensure!(groups.is_disjoint(&intruder_groups), "工作组 ID 重复");
        let mut tabs = HashSet::new();
        for tab in self.tabs.iter().chain(&self.closed) {
            ensure!(tabs.insert(tab.id), "工作标签 ID 重复");
            ensure!(
                tab.title.len() <= 4096 && tab.draft.target.len() <= 64 * 1024,
                "工作标签文本过长"
            );
            ensure!(
                tab.group.is_none_or(|id| groups.contains(&id)),
                "工作标签的组不存在"
            );
            ensure!(
                tab.draft.raw.len() <= MAX_BODY + 256 * 1024,
                "草稿超过可编辑大小限额"
            );
            if let Some(response) = &tab.archive_response {
                ensure!(
                    tab.last_flow.is_none()
                        && !response.source_id.is_nil()
                        && !response.item_id.is_nil()
                        && !response.message_id.is_nil()
                        && response.draft_revision > 0
                        && response.draft_revision <= tab.draft.revision,
                    "归档响应引用的来源、草稿 revision 或响应归属无效"
                );
            }
            if let Some(history) = &tab.history {
                if let Some(source) = &history.source {
                    ensure!(
                        !source.source_id.is_nil() && !source.parent_item_id.is_nil(),
                        "Repeater 历史来源 ID 无效"
                    );
                }
                if let Some(cursor) = &history.cursor {
                    ensure!(
                        cursor.draft_revision > 0 && cursor.draft_revision <= tab.draft.revision,
                        "Repeater 历史草稿 revision 无效"
                    );
                    match cursor.entry {
                        crate::RepeaterHistoryEntryId::Archive(_) => ensure!(
                            history.source.is_some() && tab.last_flow.is_none(),
                            "归档历史选择缺少来源或混入 live flow"
                        ),
                        crate::RepeaterHistoryEntryId::Live(flow) => ensure!(
                            !flow.is_nil()
                                && tab.last_flow == Some(flow)
                                && tab.archive_response.is_none(),
                            "live 历史选择与响应引用不一致"
                        ),
                    }
                }
            }
        }
        let mut payload_count = 0usize;
        for tab in self.intruder.tabs.iter().chain(&self.intruder.closed) {
            ensure!(tabs.insert(tab.id), "工作标签 ID 重复");
            ensure!(tab.title.len() <= 4096, "工作标签文本过长");
            ensure!(
                tab.group.is_none_or(|id| intruder_groups.contains(&id)),
                "Intruder 工作标签的组不存在"
            );
            tab.spec.validate_draft()?;
            for set in &tab.spec.payload_sets {
                payload_count = payload_count
                    .checked_add(set.len())
                    .ok_or_else(|| anyhow::anyhow!("工作区 payload 数量溢出"))?;
                ensure!(
                    payload_count <= MAX_PAYLOADS,
                    "工作区 payload 总数量超过 {MAX_PAYLOADS} 限额"
                );
            }
        }
        ensure!(
            self.draft_bytes()? <= 64 * 1024 * 1024,
            "工作区草稿总量超过 64 MiB 限额"
        );
        ensure!(
            self.active
                .is_none_or(|id| self.tabs.iter().any(|tab| tab.id == id)),
            "当前工作标签不存在"
        );
        ensure!(
            self.intruder
                .active
                .is_none_or(|id| self.intruder.tabs.iter().any(|tab| tab.id == id)),
            "当前 Intruder 工作标签不存在"
        );
        ensure!(
            self.websocket_tabs.len() <= crate::MAX_WEBSOCKET_REPEATER_TABS,
            "WebSocket Repeater 标签数量超过 {} 上限",
            crate::MAX_WEBSOCKET_REPEATER_TABS
        );
        for ws_tab in &self.websocket_tabs {
            ensure!(
                !ws_tab.id.is_nil() && tabs.insert(ws_tab.id),
                "WebSocket Repeater 标签 ID 无效或重复"
            );
            ensure!(
                ws_tab.group.is_none_or(|id| groups.contains(&id)),
                "WebSocket Repeater 工作标签的组不存在"
            );
            ws_tab.validate()?;
        }
        ensure!(
            self.active_websocket_tab.is_none_or(|id| {
                self.websocket_tabs
                    .iter()
                    .any(|tab| !tab.closed && tab.id == id)
            }),
            "当前 WebSocket Repeater 工作标签不存在"
        );
        ensure!(
            self.intruder.grep_configs.len() <= 512,
            "Intruder Grep 配置数量超过 512 条上限"
        );
        for (tab_id, config) in &self.intruder.grep_configs {
            ensure!(!tab_id.is_nil(), "Intruder Grep 配置标签 ID 无效");
            ensure!(
                self.intruder
                    .tabs
                    .iter()
                    .chain(&self.intruder.closed)
                    .any(|tab| tab.id == *tab_id),
                "Intruder Grep 配置关联的工作标签不存在"
            );
            ensure!(!config.is_empty(), "Intruder Grep 配置不能为空");
            ensure!(
                config.match_expressions.len() <= 64
                    && config
                        .match_expressions
                        .iter()
                        .all(|expr| !expr.is_empty() && expr.len() <= 1024),
                "Intruder Grep Match 规则数量或长度超出范围"
            );
            ensure!(
                config.extract_rules.len() <= 32
                    && config.extract_rules.iter().all(|rule| {
                        !rule.start_delimiter.is_empty()
                            && rule.start_delimiter.len() <= 1024
                            && !rule.end_delimiter.is_empty()
                            && rule.end_delimiter.len() <= 1024
                    }),
                "Intruder Grep Extract 规则数量或分隔符长度超出范围"
            );
        }
        crate::validate_organiser_items(&self.organiser_items)?;
        ensure!(
            self.tab_notes.len() <= 1024,
            "工作区标签备注数量超过 1024 条上限"
        );
        for (tab_id, note) in &self.tab_notes {
            ensure!(
                !tab_id.is_nil() && note.len() <= 64 * 1024,
                "工作标签备注 ID 无效或文本超过 64 KiB 上限"
            );
        }
        ensure!(
            self.scheduled_tasks.len() <= 128,
            "计划任务数量超过 128 条上限"
        );
        let mut scheduled_ids = HashSet::new();
        for task in &self.scheduled_tasks {
            ensure!(
                !task.id.is_nil() && scheduled_ids.insert(task.id),
                "计划任务 ID 无效或重复"
            );
            ensure!(
                task.trigger_spec.len() <= 512
                    && task.description.len() <= 1024
                    && !task.trigger_spec.contains('\0')
                    && !task.description.contains('\0'),
                "计划任务字段超过长度上限或包含 NUL"
            );
        }
        ensure!(
            self.organiser_collections.len() <= crate::MAX_ORGANISER_COLLECTIONS,
            "Organiser 集合数量超过 {} 条上限",
            crate::MAX_ORGANISER_COLLECTIONS
        );
        let mut collection_ids = HashSet::new();
        for collection in &self.organiser_collections {
            ensure!(
                !collection.id.is_nil() && collection_ids.insert(collection.id),
                "Organiser 集合 ID 无效或重复"
            );
            ensure!(
                !collection.name.trim().is_empty()
                    && collection.name.len() <= 256
                    && !collection.name.chars().any(char::is_control),
                "Organiser 集合名称不能为空、包含控制字符或超过 256 字节"
            );
            ensure!(
                collection.item_ids.len() <= crate::MAX_ORGANISER_ITEMS
                    && collection.item_ids.iter().all(|id| !id.is_nil()),
                "Organiser 集合条目列表超出上限或包含无效 ID"
            );
        }
        ensure!(
            self.custom_scan_checks.len() <= 256,
            "自定义扫描检查数量超过 256 条上限"
        );
        let mut check_ids = HashSet::new();
        for check in &self.custom_scan_checks {
            ensure!(
                !check.id.is_nil() && check_ids.insert(check.id),
                "自定义扫描检查 ID 无效或重复"
            );
            ensure!(
                check.name.len() <= 256
                    && check.description.len() <= 2048
                    && check.script.len() <= 128 * 1024
                    && !check.name.contains('\0')
                    && !check.description.contains('\0')
                    && !check.script.contains('\0'),
                "自定义扫描检查字段超过长度上限或包含 NUL"
            );
        }
        Ok(())
    }

    pub(crate) fn tab_count(&self) -> usize {
        self.tabs.len()
            + self.closed.len()
            + self.intruder.tabs.len()
            + self.intruder.closed.len()
            + self.websocket_tabs.len()
    }

    pub(crate) fn draft_bytes(&self) -> Result<usize> {
        let repeater = self
            .tabs
            .iter()
            .chain(&self.closed)
            .map(|tab| tab.draft.raw.len());
        let intruder = self
            .intruder
            .tabs
            .iter()
            .chain(&self.intruder.closed)
            .flat_map(|tab| {
                std::iter::once(tab.spec.draft.raw.len())
                    .chain(tab.spec.payload_sets.iter().flatten().map(Vec::len))
            });
        let websocket = self.websocket_tabs.iter().flat_map(|tab| {
            std::iter::once(tab.draft_payload.len())
                .chain(tab.history.iter().map(|item| item.payload.len()))
        });
        let organiser = self.organiser_items.iter().map(|item| {
            item.request_raw.len()
                + item.response_raw.as_ref().map_or(0, Vec::len)
                + item.notes.len()
        });
        let notes = self.tab_notes.values().map(String::len);
        repeater
            .chain(intruder)
            .chain(websocket)
            .chain(organiser)
            .chain(notes)
            .try_fold(0usize, |bytes, length| {
                bytes
                    .checked_add(length)
                    .ok_or_else(|| anyhow::anyhow!("工作区大小溢出"))
            })
    }

    #[cfg(test)]
    pub(crate) fn open_request(&mut self, request: Option<&HttpRequest>) -> Result<crate::TabId> {
        let tab = WorkTab::from_request(request, (self.tabs.len() + 1).to_string());
        let id = tab.id;
        self.tabs.push(tab);
        self.active = Some(id);
        self.revision += 1;
        self.validate()?;
        Ok(id)
    }
}

fn validate_groups(groups: &[TabGroup], sidebar_width: u16) -> Result<HashSet<Uuid>> {
    ensure!(
        sidebar_width == 0 || (120..=640).contains(&sidebar_width),
        "工作标签栏宽度超出范围"
    );
    let ids: HashSet<_> = groups.iter().map(|group| group.id).collect();
    ensure!(ids.len() == groups.len(), "工作组 ID 重复");
    ensure!(
        groups.iter().all(|group| group.title.len() <= 4096),
        "工作组标题过长"
    );
    ensure!(
        groups
            .iter()
            .all(|group| group.color.is_none_or(|color| color <= 14)),
        "工作组颜色不在已确认色板中"
    );
    Ok(ids)
}

impl WorkTab {
    pub(crate) fn from_request(request: Option<&HttpRequest>, title: String) -> Self {
        let draft = request
            .map(|request| Draft {
                revision: 1,
                target: request.url.clone(),
                raw: request_to_raw(request),
                version: request.version,
            })
            .unwrap_or_else(|| Draft {
                revision: 1,
                target: String::new(),
                raw: b"GET / HTTP/1.1\r\n\r\n".to_vec(),
                version: HttpVersion::Http11,
            });
        Self {
            id: Uuid::new_v4(),
            title,
            group: None,
            draft,
            last_flow: None,
            archive_response: None,
            history: None,
        }
    }
}
