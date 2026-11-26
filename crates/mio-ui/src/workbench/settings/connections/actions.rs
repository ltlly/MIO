use gpui_kit::component::IndexPath;
use mio_core::{
    HostnameResolutionRule, PlatformAuthRule, PlatformAuthType, SocksProxySettings,
    UpstreamAuthType, UpstreamProxyProtocol, UpstreamProxyRule, UpstreamProxySettings, Uuid,
};

use super::*;

const MAX_BYPASS_ENTRIES: usize = 128;

impl Workbench {
    pub(in crate::workbench) fn set_upstream_proxy(
        &mut self,
        project: Option<ProjectKey>,
        settings: UpstreamProxySettings,
        cx: &mut Context<Self>,
    ) {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return;
        }
        if let Err(error) = settings.validate() {
            self.fail(error.to_string(), cx);
            return;
        }
        if self.workspace.upstream_proxy == settings {
            return;
        }
        self.workspace.upstream_proxy = settings;
        self.save_workspace(cx);
        cx.notify();
    }
}

impl Settings {
    pub(in crate::workbench) fn restore_upstream_draft(
        &mut self,
        settings: &UpstreamProxySettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let socks = &settings.socks;
        for (input, value) in [
            (&self.upstream_host, socks.host.clone()),
            (&self.upstream_port, socks.port.to_string()),
            (&self.upstream_username, socks.username.clone()),
            (&self.upstream_password, socks.password.clone()),
        ] {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        cx.notify();
    }

    pub(super) fn commit_connections(
        &mut self,
        next: UpstreamProxySettings,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.state.enabled {
            return false;
        }
        if let Err(error) = next.validate() {
            self.connections.error = Some(error.to_string());
            cx.notify();
            return false;
        }
        let project = self.state.project;
        let outcome = self
            .owner
            .update(cx, |view, cx| view.set_upstream_proxy(project, next, cx));
        self.connections.error = outcome
            .err()
            .map(|_| "主工作台已关闭；连接设置未保存。".into());
        cx.notify();
        self.connections.error.is_none()
    }

    pub(super) fn change_upstream_rules(
        &mut self,
        change: impl FnOnce(&mut Vec<UpstreamProxyRule>),
        cx: &mut Context<Self>,
    ) {
        let mut next = self.state.upstream_proxy.clone();
        change(&mut next.rules);
        self.commit_connections(next, cx);
    }

    pub(in crate::workbench::settings) fn sync_connections_ui(
        &mut self,
        project_changed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let socks = &self.state.upstream_proxy.socks;
        self.connections.socks_enabled = socks.use_proxy;
        self.connections.socks_dns_remote = socks.dns_over_socks;
        self.connections.socks_bypass_hosts = socks.bypass_hosts.clone();
        if self
            .connections
            .selected_socks_bypass
            .is_some_and(|idx| idx >= self.connections.socks_bypass_hosts.len())
        {
            self.connections.selected_socks_bypass =
                self.connections.socks_bypass_hosts.len().checked_sub(1);
        }
        self.connections.open_ended_timeout_input.update(cx, |input, cx| {
            input.set_value(
                self.state
                    .upstream_proxy
                    .open_ended_response_timeout_secs
                    .to_string(),
                window,
                cx,
            )
        });
        if project_changed {
            self.connections.selected_rule = None;
            self.connections.selected_dns = None;
            self.connections.selected_platform_auth = None;
            self.connections.editing_rule = None;
            self.connections.editing_dns = None;
            self.connections.editing_platform_auth = None;
            self.connections.rule_editor_visible = false;
            self.connections.dns_editor_visible = false;
            self.connections.platform_editor_visible = false;
            self.connections.socks_editor_visible = false;
            self.connections.selected_socks_bypass = None;
            self.connections.error = None;
        }
        let rule_row = self.connections.selected_rule.and_then(|id| {
            self.state
                .upstream_proxy
                .rules
                .iter()
                .position(|rule| rule.id == id)
        });
        let dns_row = self.connections.selected_dns.and_then(|id| {
            self.state
                .upstream_proxy
                .hostname_resolutions
                .iter()
                .position(|rule| rule.id == id)
        });
        let plat_row = self.connections.selected_platform_auth.and_then(|id| {
            self.state
                .upstream_proxy
                .platform_auth
                .rules
                .iter()
                .position(|rule| rule.id == id)
        });
        for (table, selected) in [
            (&self.connections.rules_table, rule_row),
            (&self.connections.dns_table, dns_row),
            (&self.connections.platform_table, plat_row),
        ] {
            table.update(cx, |table, cx| {
                if let Some(row) = selected {
                    table.set_selected_row(row, cx);
                } else {
                    table.clear_selection(cx);
                }
                table.refresh(cx);
            });
        }
    }

    pub(super) fn open_upstream_editor(
        &mut self,
        new: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rule = if new {
            UpstreamProxyRule::default()
        } else {
            let Some(rule) = self
                .state
                .upstream_proxy
                .rules
                .iter()
                .find(|rule| Some(rule.id) == self.connections.selected_rule)
                .cloned()
            else {
                return;
            };
            rule
        };
        self.connections.editing_rule = (!new).then_some(rule.id);
        let proto_idx = match rule.protocol {
            UpstreamProxyProtocol::Http => 0,
            UpstreamProxyProtocol::Https => 1,
            UpstreamProxyProtocol::Direct | UpstreamProxyProtocol::Socks5 => 2,
        };
        self.connections.rule_protocol.update(cx, |select, cx| {
            select.set_selected_index(Some(IndexPath::new(proto_idx)), window, cx)
        });
        let auth_idx = match rule.auth_type {
            UpstreamAuthType::None => 0,
            UpstreamAuthType::Basic => 1,
            UpstreamAuthType::Digest => 2,
            UpstreamAuthType::NtlmV1 => 3,
            UpstreamAuthType::NtlmV2 => 4,
        };
        self.connections.rule_auth.update(cx, |select, cx| {
            select.set_selected_index(Some(IndexPath::new(auth_idx)), window, cx)
        });
        let port_text = if rule.protocol == UpstreamProxyProtocol::Direct && rule.port == 0 {
            String::new()
        } else {
            rule.port.to_string()
        };
        for (input, value) in [
            (&self.connections.rule_destination, rule.destination_host),
            (&self.connections.rule_host, rule.host),
            (&self.connections.rule_port, port_text),
            (&self.connections.rule_username, rule.username),
            (&self.connections.rule_password, rule.password),
            (&self.connections.rule_domain, rule.domain),
            (&self.connections.rule_workstation, rule.workstation),
            (&self.connections.rule_description, rule.description),
        ] {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        self.connections.rule_editor_visible = true;
        cx.notify();
    }

    pub(super) fn save_upstream_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let values = [
            &self.connections.rule_destination,
            &self.connections.rule_host,
            &self.connections.rule_port,
            &self.connections.rule_username,
            &self.connections.rule_password,
            &self.connections.rule_domain,
            &self.connections.rule_workstation,
            &self.connections.rule_description,
        ]
        .map(|input| editing::input_snapshot(input, window, cx));
        let [
            Ok(destination),
            Ok(host),
            Ok(port),
            Ok(username),
            Ok(password),
            Ok(domain),
            Ok(workstation),
            Ok(description),
        ] = values
        else {
            self.settings_error(editing::COMPOSITION_PENDING, cx);
            return;
        };
        let protocol = match self
            .connections
            .rule_protocol
            .read(cx)
            .selected_value()
            .copied()
            .expect("代理协议已选择")
        {
            "HTTP" => UpstreamProxyProtocol::Http,
            "HTTPS" => UpstreamProxyProtocol::Https,
            "直连" | "Direct" => UpstreamProxyProtocol::Direct,
            _ => unreachable!(),
        };
        let is_direct = protocol == UpstreamProxyProtocol::Direct;
        let port = if is_direct && port.trim().is_empty() {
            0
        } else {
            let Ok(port) = port.trim().parse::<u16>() else {
                self.connections.error = Some("代理端口必须在 1 到 65535 之间。".into());
                cx.notify();
                return;
            };
            port
        };
        let auth_type = if is_direct {
            UpstreamAuthType::None
        } else {
            match self
                .connections
                .rule_auth
                .read(cx)
                .selected_value()
                .copied()
                .expect("认证类型已选择")
            {
                "无" | "None" => UpstreamAuthType::None,
                "Basic" => UpstreamAuthType::Basic,
                "Digest" => UpstreamAuthType::Digest,
                "NTLM v1" => UpstreamAuthType::NtlmV1,
                "NTLM v2" => UpstreamAuthType::NtlmV2,
                _ => unreachable!(),
            }
        };
        let rule = UpstreamProxyRule {
            id: self.connections.editing_rule.unwrap_or_else(Uuid::new_v4),
            enabled: true,
            destination_host: destination.trim().into(),
            protocol,
            host: if is_direct {
                String::new()
            } else {
                host.trim().into()
            },
            port,
            auth_type,
            username: if is_direct {
                String::new()
            } else {
                username.to_string()
            },
            password: if is_direct {
                String::new()
            } else {
                password.to_string()
            },
            domain: if is_direct {
                String::new()
            } else {
                domain.trim().to_owned()
            },
            workstation: if is_direct {
                String::new()
            } else {
                workstation.trim().to_owned()
            },
            description: description.trim().to_owned(),
        };
        if let Err(error) = rule.validate() {
            self.connections.error = Some(error.to_string());
            cx.notify();
            return;
        }
        let mut next = self.state.upstream_proxy.clone();
        let saved_id = rule.id;
        if let Some(existing) = next.rules.iter_mut().find(|entry| entry.id == rule.id) {
            let enabled = existing.enabled;
            *existing = rule;
            existing.enabled = enabled;
        } else {
            next.rules.push(rule);
        }
        self.connections.selected_rule = Some(saved_id);
        if self.commit_connections(next, cx) {
            self.connections.rule_editor_visible = false;
            self.connections.editing_rule = None;
        }
    }

    pub(super) fn open_platform_auth_editor(
        &mut self,
        new: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rule = if new {
            PlatformAuthRule::default()
        } else {
            let Some(rule) = self
                .state
                .upstream_proxy
                .platform_auth
                .rules
                .iter()
                .find(|rule| Some(rule.id) == self.connections.selected_platform_auth)
                .cloned()
            else {
                return;
            };
            rule
        };
        self.connections.editing_platform_auth = (!new).then_some(rule.id);
        let auth_idx = match rule.auth_type {
            PlatformAuthType::Basic => 0,
            PlatformAuthType::Digest => 1,
            PlatformAuthType::NtlmV1 => 2,
            PlatformAuthType::NtlmV2 => 3,
        };
        self.connections
            .platform_auth_type
            .update(cx, |select, cx| {
                select.set_selected_index(Some(IndexPath::new(auth_idx)), window, cx)
            });
        for (input, value) in [
            (&self.connections.platform_destination, rule.destination_host),
            (&self.connections.platform_username, rule.username),
            (&self.connections.platform_password, rule.password),
            (&self.connections.platform_domain, rule.domain),
            (&self.connections.platform_hostname, rule.hostname),
        ] {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        self.connections.platform_editor_visible = true;
        cx.notify();
    }

    pub(super) fn save_platform_auth_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let values = [
            &self.connections.platform_destination,
            &self.connections.platform_username,
            &self.connections.platform_password,
            &self.connections.platform_domain,
            &self.connections.platform_hostname,
        ]
        .map(|input| editing::input_snapshot(input, window, cx));
        let [
            Ok(destination),
            Ok(username),
            Ok(password),
            Ok(domain),
            Ok(hostname),
        ] = values
        else {
            self.settings_error(editing::COMPOSITION_PENDING, cx);
            return;
        };
        let auth_type = match self
            .connections
            .platform_auth_type
            .read(cx)
            .selected_value()
            .copied()
            .expect("平台认证类型已选择")
        {
            "Basic" => PlatformAuthType::Basic,
            "Digest" => PlatformAuthType::Digest,
            "NTLM v1" => PlatformAuthType::NtlmV1,
            "NTLM v2" => PlatformAuthType::NtlmV2,
            _ => unreachable!(),
        };
        let rule = PlatformAuthRule {
            id: self
                .connections
                .editing_platform_auth
                .unwrap_or_else(Uuid::new_v4),
            enabled: true,
            destination_host: destination.trim().into(),
            auth_type,
            username: username.to_string(),
            password: password.to_string(),
            domain: domain.trim().to_owned(),
            hostname: hostname.trim().to_owned(),
        };
        if let Err(error) = rule.validate() {
            self.connections.error = Some(error.to_string());
            cx.notify();
            return;
        }
        let mut next = self.state.upstream_proxy.clone();
        let saved_id = rule.id;
        if let Some(existing) = next
            .platform_auth
            .rules
            .iter_mut()
            .find(|entry| entry.id == rule.id)
        {
            let enabled = existing.enabled;
            *existing = rule;
            existing.enabled = enabled;
        } else {
            next.platform_auth.rules.push(rule);
        }
        self.connections.selected_platform_auth = Some(saved_id);
        if self.commit_connections(next, cx) {
            self.connections.platform_editor_visible = false;
            self.connections.editing_platform_auth = None;
        }
    }

    pub(super) fn save_socks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let values = [
            &self.upstream_host,
            &self.upstream_port,
            &self.upstream_username,
            &self.upstream_password,
        ]
        .map(|input| editing::input_snapshot(input, window, cx));
        let [Ok(host), Ok(port), Ok(username), Ok(password)] = values else {
            self.settings_error(editing::COMPOSITION_PENDING, cx);
            return;
        };
        let Ok(port) = port.trim().parse::<u16>() else {
            self.connections.error = Some("SOCKS 代理端口必须在 0 到 65535 之间。".into());
            cx.notify();
            return;
        };
        let mut next = self.state.upstream_proxy.clone();
        let prev_override = next.socks.override_project_only;
        next.socks = SocksProxySettings {
            use_proxy: self.connections.socks_enabled,
            host: host.trim().into(),
            port,
            username: username.to_string(),
            password: password.to_string(),
            dns_over_socks: self.connections.socks_dns_remote,
            bypass_hosts: self.connections.socks_bypass_hosts.clone(),
            override_project_only: prev_override,
        };
        self.commit_connections(next, cx);
    }

    pub(super) fn commit_socks_bypass_hosts(&mut self, cx: &mut Context<Self>) {
        let mut next = self.state.upstream_proxy.clone();
        next.socks.bypass_hosts = self.connections.socks_bypass_hosts.clone();
        self.commit_connections(next, cx);
    }

    fn append_socks_bypass_entries(&mut self, entries: impl IntoIterator<Item = String>) -> bool {
        let mut added = false;
        for raw in entries {
            for part in raw.split([',', ';', '\r', '\n']) {
                let host = part.trim();
                if host.is_empty()
                    || host.len() > 253
                    || host.chars().any(char::is_control)
                    || self.connections.socks_bypass_hosts.len() >= MAX_BYPASS_ENTRIES
                    || self
                        .connections
                        .socks_bypass_hosts
                        .iter()
                        .any(|item| item.eq_ignore_ascii_case(host))
                {
                    continue;
                }
                self.connections.socks_bypass_hosts.push(host.to_owned());
                added = true;
            }
        }
        added
    }

    pub(super) fn add_socks_bypass_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(raw) = editing::input_snapshot(&self.connections.socks_bypass_input, window, cx)
        else {
            self.settings_error(editing::COMPOSITION_PENDING, cx);
            return;
        };
        let trimmed = raw.trim().to_owned();
        if trimmed.is_empty() {
            return;
        }
        let added = self.append_socks_bypass_entries([trimmed]);
        self.connections
            .socks_bypass_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.connections.error = None;
        if added {
            self.commit_socks_bypass_hosts(cx);
        } else {
            cx.notify();
        }
    }

    pub(super) fn paste_socks_bypass_hosts(&mut self, cx: &mut Context<Self>) {
        let Some(raw) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let added = self.append_socks_bypass_entries([raw]);
        self.connections.error = None;
        if added {
            self.commit_socks_bypass_hosts(cx);
        } else {
            cx.notify();
        }
    }

    pub(super) fn load_socks_bypass_hosts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.enabled {
            return;
        }
        let Some(picker) = self.begin_workbench_file_picker(cx) else {
            return;
        };
        let project = self.state.project;
        let owner = self.owner.clone();
        let paths = crate::native_terminate::prompt_for_paths(
            window,
            cx,
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("加载 SOCKS 代理绕过主机列表".into()),
            },
        );
        cx.spawn_in(window, async move |this, cx| {
            let result = paths.await;
            let admitted = this
                .update_in(cx, |this, _, cx| {
                    this.finish_workbench_file_picker(picker, cx)
                })
                .unwrap_or_else(|_| {
                    owner
                        .update(cx, |view, cx| view.finish_file_picker(picker, cx))
                        .unwrap_or(false)
                });
            if !admitted {
                return;
            }
            if let Ok(Ok(Some(selected))) = result
                && let Some(path) = selected.into_iter().next()
            {
                let parsed = cx
                    .background_executor()
                    .spawn(async move { read_bypass_list_file(&path) })
                    .await;
                let _ = this.update_in(cx, |this, _, cx| {
                    if this.state.project != project || !this.state.enabled {
                        return;
                    }
                    match parsed {
                        Ok(entries) => {
                            let added = this.append_socks_bypass_entries(entries);
                            this.connections.error = None;
                            if added {
                                this.commit_socks_bypass_hosts(cx);
                                return;
                            }
                        }
                        Err(error) => {
                            this.connections.error = Some(error);
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn save_network_timeouts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Ok(normal), Ok(open)) = (
            editing::input_snapshot(&self.connections.normal_timeout_input, window, cx),
            editing::input_snapshot(&self.connections.open_ended_timeout_input, window, cx),
        ) else {
            self.settings_error(editing::COMPOSITION_PENDING, cx);
            return;
        };
        let normal_trimmed = normal.trim();
        let open_trimmed = open.trim();
        let Ok(normal) = normal_trimmed.parse::<u64>() else {
            self.connections.error = Some("网络超时必须是有效的整数秒数。".into());
            cx.notify();
            return;
        };
        let open = if open_trimmed.is_empty() {
            0
        } else {
            match open_trimmed.parse::<u64>() {
                Ok(value) => value,
                Err(_) => {
                    self.connections.error = Some("网络超时必须是有效的整数秒数。".into());
                    cx.notify();
                    return;
                }
            }
        };
        if !(1..=3600).contains(&normal) || open > 86400 {
            self.connections.error =
                Some("普通超时范围为 1–3600 秒，流式响应超时范围为 0–86400 秒。".into());
            cx.notify();
            return;
        }
        if self.state.request_timeout_secs == normal
            && self.state.upstream_proxy.open_ended_response_timeout_secs == open
        {
            self.connections.error = None;
            cx.notify();
            return;
        }
        let project = self.state.project;
        let _ = self.owner.update(cx, |view, cx| {
            if project.is_none()
                || view.project != project
                || !view.backend_available()
                || view.transition.is_some()
            {
                return;
            }
            view.workspace.repeater_settings.request_timeout_secs = normal;
            view.workspace.upstream_proxy.open_ended_response_timeout_secs = open;
            view.save_workspace(cx);
            cx.notify();
        });
        self.connections.error = None;
        cx.notify();
    }

    pub(super) fn open_hostname_editor(
        &mut self,
        new: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rule = if new {
            HostnameResolutionRule::default()
        } else {
            let Some(rule) = self
                .state
                .upstream_proxy
                .hostname_resolutions
                .iter()
                .find(|rule| Some(rule.id) == self.connections.selected_dns)
                .cloned()
            else {
                return;
            };
            rule
        };
        self.connections.editing_dns = (!new).then_some(rule.id);
        self.connections
            .dns_hostname_input
            .update(cx, |input, cx| input.set_value(rule.hostname, window, cx));
        self.connections
            .dns_ip_input
            .update(cx, |input, cx| input.set_value(rule.ip_address, window, cx));
        self.connections.dns_editor_visible = true;
        cx.notify();
    }

    pub(super) fn save_hostname_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Ok(host), Ok(ip)) = (
            editing::input_snapshot(&self.connections.dns_hostname_input, window, cx),
            editing::input_snapshot(&self.connections.dns_ip_input, window, cx),
        ) else {
            self.settings_error(editing::COMPOSITION_PENDING, cx);
            return;
        };
        let mut rule = HostnameResolutionRule {
            id: self.connections.editing_dns.unwrap_or_else(Uuid::new_v4),
            enabled: true,
            hostname: host.trim().into(),
            ip_address: ip.trim().into(),
        };
        let (host, ip) = match rule.validate() {
            Ok(value) => value,
            Err(error) => {
                self.connections.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        rule.hostname = host;
        rule.ip_address = ip.to_string();
        let mut next = self.state.upstream_proxy.clone();
        let saved_id = rule.id;
        if let Some(existing) = next
            .hostname_resolutions
            .iter_mut()
            .find(|entry| entry.id == rule.id)
        {
            let enabled = existing.enabled;
            *existing = rule;
            existing.enabled = enabled;
        } else {
            next.hostname_resolutions.push(rule);
        }
        self.connections.selected_dns = Some(saved_id);
        if self.commit_connections(next, cx) {
            self.connections.dns_editor_visible = false;
            self.connections.editing_dns = None;
        }
    }
}
