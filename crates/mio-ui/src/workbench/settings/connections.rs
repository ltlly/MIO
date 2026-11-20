use std::{fs::OpenOptions, io::Read, path::Path};

use gpui_kit::accesskit::Role;
use gpui_kit::base::{CheckboxState, Switch as BaseSwitch};
use gpui_kit::component::{
    IndexPath,
    select::{Select, SelectEvent, SelectState},
    table::{Column, TableDelegate},
};
use mio_core::{
    HostnameResolutionRule, HttpAuthScheme, PlatformAuthRule, PlatformAuthType,
    SocksProxySettings, UpstreamAuthType, UpstreamProxyProtocol, UpstreamProxyRule,
    UpstreamProxySettings, Uuid,
};

use super::*;

const MAX_BYPASS_FILE_BYTES: u64 = 128 * 1024;
const MAX_BYPASS_ENTRIES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConnectionsTableKind {
    Upstream,
    Hostname,
    PlatformAuth,
}

pub(super) struct ConnectionsUi {
    rules_table: Entity<TableState<ConnectionsTable>>,
    dns_table: Entity<TableState<ConnectionsTable>>,
    platform_table: Entity<TableState<ConnectionsTable>>,
    selected_rule: Option<Uuid>,
    selected_dns: Option<Uuid>,
    selected_platform_auth: Option<Uuid>,
    editing_rule: Option<Uuid>,
    editing_dns: Option<Uuid>,
    editing_platform_auth: Option<Uuid>,
    rule_editor_visible: bool,
    dns_editor_visible: bool,
    platform_editor_visible: bool,
    socks_editor_visible: bool,
    rule_protocol: Entity<SelectState<Vec<&'static str>>>,
    rule_auth: Entity<SelectState<Vec<&'static str>>>,
    platform_auth_type: Entity<SelectState<Vec<&'static str>>>,
    pub(super) rule_destination: Entity<InputState>,
    pub(super) rule_host: Entity<InputState>,
    pub(super) rule_port: Entity<InputState>,
    pub(super) rule_username: Entity<InputState>,
    pub(super) rule_password: Entity<InputState>,
    pub(super) rule_domain: Entity<InputState>,
    pub(super) rule_workstation: Entity<InputState>,
    pub(super) rule_description: Entity<InputState>,
    pub(super) platform_destination: Entity<InputState>,
    pub(super) platform_username: Entity<InputState>,
    pub(super) platform_password: Entity<InputState>,
    pub(super) platform_domain: Entity<InputState>,
    pub(super) platform_hostname: Entity<InputState>,
    socks_enabled: bool,
    socks_dns_remote: bool,
    socks_bypass_hosts: Vec<String>,
    selected_socks_bypass: Option<usize>,
    pub(super) socks_bypass_input: Entity<InputState>,
    pub(super) normal_timeout_input: Entity<InputState>,
    pub(super) open_ended_timeout_input: Entity<InputState>,
    pub(super) dns_hostname_input: Entity<InputState>,
    pub(super) dns_ip_input: Entity<InputState>,
    error: Option<String>,
}

impl ConnectionsUi {
    pub(super) fn new(
        state: &SettingsState,
        window: &mut Window,
        cx: &mut Context<Settings>,
    ) -> Self {
        let tables: [Entity<TableState<ConnectionsTable>>; 3] = std::array::from_fn(|index| {
            let owner = cx.weak_entity();
            let kind = match index {
                0 => ConnectionsTableKind::Upstream,
                1 => ConnectionsTableKind::Hostname,
                _ => ConnectionsTableKind::PlatformAuth,
            };
            let table = cx.new(|tcx| {
                TableState::new(ConnectionsTable { owner, kind }, window, tcx)
                    .sortable(false)
                    .col_selectable(false)
            });
            cx.subscribe_in(&table, window, move |this, _, event, _, cx| {
                if let TableEvent::SelectRow(row) = event {
                    match kind {
                        ConnectionsTableKind::Upstream => {
                            this.connections.selected_rule = this
                                .state
                                .upstream_proxy
                                .effective_rules()
                                .get(*row)
                                .map(|rule| rule.id);
                        }
                        ConnectionsTableKind::Hostname => {
                            this.connections.selected_dns = this
                                .state
                                .upstream_proxy
                                .hostname_resolutions
                                .get(*row)
                                .map(|rule| rule.id);
                        }
                        ConnectionsTableKind::PlatformAuth => {
                            this.connections.selected_platform_auth = this
                                .state
                                .upstream_proxy
                                .platform_auth_rules()
                                .get(*row)
                                .map(|rule| rule.id);
                        }
                    }
                    cx.notify();
                }
            })
            .detach();
            table
        });
        let rule_protocol = cx.new(|scx| {
            SelectState::new(vec!["HTTP", "HTTPS"], Some(IndexPath::new(0)), window, scx)
        });
        let rule_auth = cx.new(|scx| {
            SelectState::new(
                vec!["None", "Basic", "Digest", "NTLM v1", "NTLM v2"],
                Some(IndexPath::new(0)),
                window,
                scx,
            )
        });
        let platform_auth_type = cx.new(|scx| {
            SelectState::new(
                vec!["Basic", "Digest", "NTLM v1", "NTLM v2"],
                Some(IndexPath::new(0)),
                window,
                scx,
            )
        });
        cx.subscribe_in(
            &rule_protocol,
            window,
            |_, _, _: &SelectEvent<Vec<&'static str>>, _, cx| cx.notify(),
        )
        .detach();
        cx.subscribe_in(
            &rule_auth,
            window,
            |_, _, _: &SelectEvent<Vec<&'static str>>, _, cx| cx.notify(),
        )
        .detach();
        cx.subscribe_in(
            &platform_auth_type,
            window,
            |_, _, _: &SelectEvent<Vec<&'static str>>, _, cx| cx.notify(),
        )
        .detach();
        let socks_bypass_input = cx.new(|icx| InputState::new(window, icx));
        cx.subscribe_in(&socks_bypass_input, window, |this, _, event, window, cx| {
            match event {
                InputEvent::PressEnter { .. } => this.add_socks_bypass_host(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            }
        })
        .detach();
        let normal_timeout_input = cx.new(|icx| {
            InputState::new(window, icx).default_value(state.request_timeout_secs.to_string())
        });
        let open_ended_timeout_input = cx.new(|icx| {
            InputState::new(window, icx).default_value(
                state
                    .upstream_proxy
                    .open_ended_response_timeout_secs
                    .to_string(),
            )
        });
        for timeout_input in [&normal_timeout_input, &open_ended_timeout_input] {
            cx.subscribe_in(timeout_input, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    this.save_network_timeouts(window, cx);
                }
            })
            .detach();
        }
        let socks = state.upstream_proxy.effective_socks();
        Self {
            rules_table: tables[0].clone(),
            dns_table: tables[1].clone(),
            platform_table: tables[2].clone(),
            selected_rule: None,
            selected_dns: None,
            selected_platform_auth: None,
            editing_rule: None,
            editing_dns: None,
            editing_platform_auth: None,
            rule_editor_visible: false,
            dns_editor_visible: false,
            platform_editor_visible: false,
            socks_editor_visible: false,
            rule_protocol,
            rule_auth,
            platform_auth_type,
            rule_destination: cx.new(|icx| InputState::new(window, icx).default_value("*")),
            rule_host: cx.new(|icx| InputState::new(window, icx).placeholder("proxy.example.com")),
            rule_port: cx.new(|icx| InputState::new(window, icx).default_value("8080")),
            rule_username: cx.new(|icx| InputState::new(window, icx)),
            rule_password: cx.new(|icx| InputState::new(window, icx)),
            rule_domain: cx.new(|icx| InputState::new(window, icx).placeholder("可选 NTLM 域")),
            rule_workstation: cx
                .new(|icx| InputState::new(window, icx).placeholder("可选 NTLM 工作站")),
            rule_description: cx.new(|icx| InputState::new(window, icx).placeholder("可选描述")),
            platform_destination: cx
                .new(|icx| InputState::new(window, icx).default_value("*")),
            platform_username: cx.new(|icx| InputState::new(window, icx)),
            platform_password: cx.new(|icx| InputState::new(window, icx)),
            platform_domain: cx.new(|icx| InputState::new(window, icx).placeholder("可选域")),
            platform_hostname: cx
                .new(|icx| InputState::new(window, icx).placeholder("可选主机名")),
            socks_enabled: socks.use_proxy,
            socks_dns_remote: socks.dns_over_socks,
            socks_bypass_hosts: socks.bypass_hosts,
            selected_socks_bypass: None,
            socks_bypass_input,
            normal_timeout_input,
            open_ended_timeout_input,
            dns_hostname_input: cx
                .new(|icx| InputState::new(window, icx).placeholder("api.example.com")),
            dns_ip_input: cx.new(|icx| InputState::new(window, icx).placeholder("127.0.0.1")),
            error: None,
        }
    }
}

pub(super) struct ConnectionsTable {
    owner: WeakEntity<Settings>,
    kind: ConnectionsTableKind,
}

const UPSTREAM_COLUMNS: [(&str, f32); 7] = [
    ("Enabled", 72.),
    ("Destination host", 160.),
    ("Proxy host", 160.),
    ("Proxy port", 90.),
    ("Auth type", 78.),
    ("Username", 82.),
    ("Description", 180.),
];
const HOSTNAME_COLUMNS: [(&str, f32); 3] = [
    ("Enabled", 76.),
    ("Hostname", 373.),
    ("IP address", 373.),
];
const PLATFORM_AUTH_COLUMNS: [(&str, f32); 6] = [
    ("Enabled", 72.),
    ("Destination host", 180.),
    ("Auth type", 100.),
    ("Username", 150.),
    ("Domain", 150.),
    ("Hostname", 170.),
];

impl ConnectionsTable {
    fn col_meta(&self, index: usize) -> (&'static str, f32, usize) {
        match self.kind {
            ConnectionsTableKind::Upstream => {
                let (label, width) = UPSTREAM_COLUMNS[index];
                (label, width, UPSTREAM_COLUMNS.len())
            }
            ConnectionsTableKind::Hostname => {
                let (label, width) = HOSTNAME_COLUMNS[index];
                (label, width, HOSTNAME_COLUMNS.len())
            }
            ConnectionsTableKind::PlatformAuth => {
                let (label, width) = PLATFORM_AUTH_COLUMNS[index];
                (label, width, PLATFORM_AUTH_COLUMNS.len())
            }
        }
    }

    fn cell(&self, row: usize, column: usize, cx: &App) -> Option<String> {
        let owner = self.owner.upgrade()?;
        let settings = owner.read(cx);
        match self.kind {
            ConnectionsTableKind::Hostname => {
                let entry = settings.state.upstream_proxy.hostname_resolutions.get(row)?;
                Some(match column {
                    0 => if entry.enabled { "Yes" } else { "No" }.into(),
                    1 => entry.hostname.clone(),
                    _ => entry.ip_address.clone(),
                })
            }
            ConnectionsTableKind::PlatformAuth => {
                let rules = settings.state.upstream_proxy.platform_auth_rules();
                let rule = rules.get(row)?;
                Some(match column {
                    0 => if rule.enabled { "Yes" } else { "No" }.into(),
                    1 => rule.destination_host.clone(),
                    2 => rule.auth_type.label().into(),
                    3 => rule.username.clone(),
                    4 => rule.domain.clone(),
                    _ => rule.hostname.clone(),
                })
            }
            ConnectionsTableKind::Upstream => {
                let rules = settings.state.upstream_proxy.effective_rules();
                let rule = rules.get(row)?;
                let user_desc = rule.user_description();
                Some(match column {
                    0 => if rule.enabled { "Yes" } else { "No" }.into(),
                    1 => rule.destination_host.clone(),
                    2 => rule.host.clone(),
                    3 => rule.port.to_string(),
                    4 => rule.auth_scheme().label().into(),
                    5 => rule.username.clone(),
                    _ => {
                        if !user_desc.is_empty() {
                            user_desc
                        } else if rule.protocol == UpstreamProxyProtocol::Https {
                            "HTTPS".into()
                        } else {
                            String::new()
                        }
                    }
                })
            }
        }
    }
}

impl TableDelegate for ConnectionsTable {
    fn columns_count(&self, _: &App) -> usize {
        match self.kind {
            ConnectionsTableKind::Upstream => UPSTREAM_COLUMNS.len(),
            ConnectionsTableKind::Hostname => HOSTNAME_COLUMNS.len(),
            ConnectionsTableKind::PlatformAuth => PLATFORM_AUTH_COLUMNS.len(),
        }
    }

    fn column(&self, index: usize, _: &App) -> Column {
        let (label, width, count) = self.col_meta(index);
        Column::new(index.to_string(), label)
            .width((width * theme::ui_scale()).round())
            .p_0()
            .movable(false)
            .resizable(index + 1 < count)
    }

    fn rows_count(&self, cx: &App) -> usize {
        self.owner.upgrade().map_or(0, |owner| {
            let settings = owner.read(cx);
            match self.kind {
                ConnectionsTableKind::Upstream => {
                    settings.state.upstream_proxy.effective_rules().len()
                }
                ConnectionsTableKind::Hostname => {
                    settings.state.upstream_proxy.hostname_resolutions.len()
                }
                ConnectionsTableKind::PlatformAuth => {
                    settings.state.upstream_proxy.platform_auth_rules().len()
                }
            }
        })
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let (label, _, _) = self.col_meta(col_ix);
        div()
            .px(px(6.))
            .text_color(theme::foreground())
            .text_size(theme::scaled_px(12.5))
            .truncate()
            .child(label)
    }

    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        div()
            .id((
                match self.kind {
                    ConnectionsTableKind::Upstream => "settings-upstream-rule-row",
                    ConnectionsTableKind::Hostname => "settings-hostname-row",
                    ConnectionsTableKind::PlatformAuth => "settings-platform-auth-row",
                },
                row,
            ))
            .border_b_0()
    }

    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        if column > 0 {
            return listeners_tls::mio_table_td_text(self.cell(row, column, cx).unwrap_or_default())
                .into_any_element();
        }
        let Some(owner) = self.owner.upgrade() else {
            return div().into_any_element();
        };
        let (id, checked, enabled) = {
            let settings = owner.read(cx);
            match self.kind {
                ConnectionsTableKind::Hostname => {
                    let Some(rule) = settings.state.upstream_proxy.hostname_resolutions.get(row)
                    else {
                        return div().into_any_element();
                    };
                    (rule.id, rule.enabled, settings.state.enabled)
                }
                ConnectionsTableKind::PlatformAuth => {
                    let rules = settings.state.upstream_proxy.platform_auth_rules();
                    let Some(rule) = rules.get(row) else {
                        return div().into_any_element();
                    };
                    (rule.id, rule.enabled, settings.state.enabled)
                }
                ConnectionsTableKind::Upstream => {
                    let rules = settings.state.upstream_proxy.effective_rules();
                    let Some(rule) = rules.get(row) else {
                        return div().into_any_element();
                    };
                    (rule.id, rule.enabled, settings.state.enabled)
                }
            }
        };
        let kind = self.kind;
        listeners_tls::mio_table_td_checkbox(
            listeners_tls::indexed_checkbox(
                (
                    match kind {
                        ConnectionsTableKind::Upstream => "settings-upstream-rule-toggle",
                        ConnectionsTableKind::Hostname => "settings-hostname-toggle",
                        ConnectionsTableKind::PlatformAuth => "settings-platform-auth-toggle",
                    },
                    row,
                ),
                "",
                checked,
                enabled,
            )
            .accessibility_label(match kind {
                ConnectionsTableKind::Upstream => "启用上游代理规则",
                ConnectionsTableKind::Hostname => "启用主机名解析规则",
                ConnectionsTableKind::PlatformAuth => "启用平台认证规则",
            })
            .on_change(move |value, _, _, cx| {
                owner.update(cx, |this, cx| {
                    match kind {
                        ConnectionsTableKind::Hostname => {
                            let mut next = this.state.upstream_proxy.clone();
                            let entry = next
                                .hostname_resolutions
                                .iter_mut()
                                .find(|entry| entry.id == id)
                                .expect("选中的主机名解析规则存在");
                            entry.enabled = value == CheckboxState::Checked;
                            this.commit_connections(next, cx);
                        }
                        ConnectionsTableKind::PlatformAuth => {
                            let mut next = this.state.upstream_proxy.clone();
                            let mut rules = next.platform_auth_rules();
                            if let Some(entry) = rules.iter_mut().find(|entry| entry.id == id) {
                                entry.enabled = value == CheckboxState::Checked;
                            }
                            next.set_platform_auth_rules(rules);
                            this.commit_connections(next, cx);
                        }
                        ConnectionsTableKind::Upstream => {
                            this.change_upstream_rules(
                                |rules| {
                                    let entry = rules
                                        .iter_mut()
                                        .find(|entry| entry.id == id)
                                        .expect("选中的上游代理规则存在");
                                    entry.enabled = value == CheckboxState::Checked;
                                },
                                cx,
                            );
                        }
                    }
                })
            }),
        )
        .into_any_element()
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        listeners_tls::mio_table_empty()
    }

    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div().w_0().h_full()
    }

    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        self.cell(row, column, cx).map(Into::into)
    }
}

fn read_bypass_list_file(path: &Path) -> Result<Vec<String>, String> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NONBLOCK);
    }
    let file = opts
        .open(path)
        .map_err(|error| format!("无法读取绕过列表文件：{error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("无法读取绕过列表文件信息：{error}"))?;
    if !metadata.is_file() || metadata.len() > MAX_BYPASS_FILE_BYTES {
        return Err("绕过列表文件必须是大小不超过 128 KiB 的普通文本文件。".into());
    }
    let mut raw = Vec::new();
    file.take(MAX_BYPASS_FILE_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(|error| format!("无法完整读取绕过列表文件：{error}"))?;
    let text = String::from_utf8(raw)
        .map_err(|_| "绕过列表文件必须为 UTF-8 编码文本。".to_owned())?;
    Ok(text
        .split([',', ';', '\r', '\n'])
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect())
}

impl Workbench {
    #[allow(dead_code)]
    pub(in crate::workbench) fn set_proxy_listener(
        &mut self,
        project: Option<ProjectKey>,
        address: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return false;
        }
        let listen: SocketAddr = match address.trim().parse() {
            Ok(address) => address,
            Err(_) => {
                self.fail("监听地址须包含 IP 和端口，例如 127.0.0.1:8080。", cx);
                return false;
            }
        };
        if !listen.ip().is_loopback() {
            self.fail("Proxy 监听地址必须使用 loopback 地址。", cx);
            return false;
        }
        let mut next = self.workspace.clone();
        if let Some(item) = next.proxy_listeners.iter_mut().find(|item| item.enabled) {
            item.bind_address = listen.to_string();
        }
        next.proxy_listener = (listen.to_string() != "127.0.0.1:8080").then(|| listen.to_string());
        listeners_tls::sync_primary_listener_field(&mut next);
        if let Err(error) = next.validate_structure() {
            self.fail(error.to_string(), cx);
            return false;
        }
        if next != self.workspace {
            self.workspace = next;
            self.save_workspace(cx);
        }
        self.listener_input
            .update(cx, |input, cx| input.set_value(listen.to_string(), window, cx));
        cx.notify();
        true
    }

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
        settings.sync_user_level_cache();
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
        let socks = settings.effective_socks();
        self.upstream_protocol = settings.protocol;
        for (input, value) in [
            (&self.upstream_host, socks.host),
            (&self.upstream_port, socks.port.to_string()),
            (&self.upstream_username, socks.username),
            (&self.upstream_password, socks.password),
        ] {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        cx.notify();
    }

    fn commit_connections(&mut self, next: UpstreamProxySettings, cx: &mut Context<Self>) -> bool {
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

    fn change_upstream_rules(
        &mut self,
        change: impl FnOnce(&mut Vec<UpstreamProxyRule>),
        cx: &mut Context<Self>,
    ) {
        let mut next = self.state.upstream_proxy.clone();
        let mut real_rules = next.effective_rules();
        next.socks = next.effective_socks();
        change(&mut real_rules);
        next.retain_with_platform_auth(real_rules);
        next.sync_legacy_summary();
        self.commit_connections(next, cx);
    }

    pub(super) fn sync_connections_ui(
        &mut self,
        project_changed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let socks = self.state.upstream_proxy.effective_socks();
        self.connections.socks_enabled = socks.use_proxy;
        self.connections.socks_dns_remote = socks.dns_over_socks;
        self.connections.socks_bypass_hosts = socks.bypass_hosts;
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
                .effective_rules()
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
                .platform_auth_rules()
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

    fn open_upstream_editor(&mut self, new: bool, window: &mut Window, cx: &mut Context<Self>) {
        let rule = if new {
            UpstreamProxyRule::default()
        } else {
            let Some(rule) = self
                .state
                .upstream_proxy
                .effective_rules()
                .into_iter()
                .find(|rule| Some(rule.id) == self.connections.selected_rule)
            else {
                return;
            };
            rule
        };
        let scheme = rule.auth_scheme();
        let domain = rule.domain();
        let workstation = rule.workstation();
        let description = rule.user_description();
        self.connections.editing_rule = (!new).then_some(rule.id);
        self.connections.rule_protocol.update(cx, |select, cx| {
            select.set_selected_index(
                Some(IndexPath::new(usize::from(
                    rule.protocol == UpstreamProxyProtocol::Https,
                ))),
                window,
                cx,
            )
        });
        let auth_idx = match scheme {
            HttpAuthScheme::None => 0,
            HttpAuthScheme::Basic => 1,
            HttpAuthScheme::Digest => 2,
            HttpAuthScheme::NtlmV1 => 3,
            HttpAuthScheme::NtlmV2 => 4,
        };
        self.connections.rule_auth.update(cx, |select, cx| {
            select.set_selected_index(Some(IndexPath::new(auth_idx)), window, cx)
        });
        for (input, value) in [
            (&self.connections.rule_destination, rule.destination_host),
            (&self.connections.rule_host, rule.host),
            (&self.connections.rule_port, rule.port.to_string()),
            (&self.connections.rule_username, rule.username),
            (&self.connections.rule_password, rule.password),
            (&self.connections.rule_domain, domain),
            (&self.connections.rule_workstation, workstation),
            (&self.connections.rule_description, description),
        ] {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        self.connections.rule_editor_visible = true;
        cx.notify();
    }

    fn save_upstream_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        let Ok(port) = port.trim().parse::<u16>() else {
            self.connections.error = Some("代理端口必须在 1 到 65535 之间。".into());
            cx.notify();
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
            _ => unreachable!(),
        };
        let scheme = match self
            .connections
            .rule_auth
            .read(cx)
            .selected_value()
            .copied()
            .expect("认证类型已选择")
        {
            "None" => HttpAuthScheme::None,
            "Basic" => HttpAuthScheme::Basic,
            "Digest" => HttpAuthScheme::Digest,
            "NTLM v1" => HttpAuthScheme::NtlmV1,
            "NTLM v2" => HttpAuthScheme::NtlmV2,
            _ => unreachable!(),
        };
        let mut rule = UpstreamProxyRule {
            id: self.connections.editing_rule.unwrap_or_else(Uuid::new_v4),
            enabled: true,
            destination_host: destination.trim().into(),
            protocol,
            host: host.trim().into(),
            port,
            auth_type: if scheme == HttpAuthScheme::None {
                UpstreamAuthType::None
            } else {
                UpstreamAuthType::Basic
            },
            username: username.to_string(),
            password: password.to_string(),
            description: description.trim().to_owned(),
        };
        rule.set_extended_auth(
            scheme,
            domain.trim(),
            workstation.trim(),
            description.trim(),
        );
        if let Err(error) = rule.validate() {
            self.connections.error = Some(error.to_string());
            cx.notify();
            return;
        }
        let mut next = self.state.upstream_proxy.clone();
        let mut real_rules = next.effective_rules();
        next.socks = next.effective_socks();
        let saved_id = rule.id;
        if let Some(existing) = real_rules.iter_mut().find(|entry| entry.id == rule.id) {
            let enabled = existing.enabled;
            *existing = rule;
            existing.enabled = enabled;
        } else {
            real_rules.push(rule);
        }
        next.retain_with_platform_auth(real_rules);
        next.sync_legacy_summary();
        self.connections.selected_rule = Some(saved_id);
        if self.commit_connections(next, cx) {
            self.connections.rule_editor_visible = false;
            self.connections.editing_rule = None;
        }
    }

    fn open_platform_auth_editor(&mut self, new: bool, window: &mut Window, cx: &mut Context<Self>) {
        let rule = if new {
            PlatformAuthRule::default()
        } else {
            let Some(rule) = self
                .state
                .upstream_proxy
                .platform_auth_rules()
                .into_iter()
                .find(|rule| Some(rule.id) == self.connections.selected_platform_auth)
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

    fn save_platform_auth_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        let mut rules = next.platform_auth_rules();
        let saved_id = rule.id;
        if let Some(existing) = rules.iter_mut().find(|entry| entry.id == rule.id) {
            let enabled = existing.enabled;
            *existing = rule;
            existing.enabled = enabled;
        } else {
            rules.push(rule);
        }
        next.set_platform_auth_rules(rules);
        self.connections.selected_platform_auth = Some(saved_id);
        if self.commit_connections(next, cx) {
            self.connections.platform_editor_visible = false;
            self.connections.editing_platform_auth = None;
        }
    }

    fn save_socks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        let prev_override = next.effective_socks().override_project_only;
        let real_rules = next.effective_rules();
        next.retain_with_platform_auth(real_rules);
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
        next.sync_legacy_summary();
        self.commit_connections(next, cx);
    }

    fn commit_socks_bypass_hosts(&mut self, cx: &mut Context<Self>) {
        let mut next = self.state.upstream_proxy.clone();
        let real_rules = next.effective_rules();
        next.retain_with_platform_auth(real_rules);
        next.socks = next.effective_socks();
        next.socks.bypass_hosts = self.connections.socks_bypass_hosts.clone();
        next.sync_legacy_summary();
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

    fn add_socks_bypass_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn paste_socks_bypass_hosts(&mut self, cx: &mut Context<Self>) {
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

    fn load_socks_bypass_hosts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.enabled {
            return;
        }
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
            if let Ok(Ok(Some(selected))) = paths.await
                && let Some(path) = selected.into_iter().next()
            {
                let parsed = cx
                    .background_executor()
                    .spawn(async move { read_bypass_list_file(&path) })
                    .await;
                let _ = this.update_in(cx, |this, _, cx| {
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

    fn save_network_timeouts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn open_hostname_editor(&mut self, new: bool, window: &mut Window, cx: &mut Context<Self>) {
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

    fn save_hostname_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(in crate::workbench) fn render_connections(
        &self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let enabled = self.state.enabled;
        let rules = self.state.upstream_proxy.effective_rules();
        let selected = self
            .connections
            .selected_rule
            .and_then(|id| rules.iter().position(|rule| rule.id == id));

        let mut upstream_actions = div()
            .w(theme::scaled_px(88.).max(px(84.)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(6.));
        for (id, label, action, available) in [
            ("settings-upstream-add", "添加", 0u8, true),
            ("settings-upstream-edit", "编辑", 1, selected.is_some()),
            ("settings-upstream-remove", "移除", 2, selected.is_some()),
            (
                "settings-upstream-up",
                "上移",
                3,
                selected.is_some_and(|index| index > 0),
            ),
            (
                "settings-upstream-down",
                "下移",
                4,
                selected.is_some_and(|index| index + 1 < rules.len()),
            ),
        ] {
            upstream_actions = upstream_actions.child(
                listeners_tls::mio_action_button(id, label, !enabled || !available)
                    .w_full()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if action < 2 {
                            this.open_upstream_editor(action == 0, window, cx);
                            return;
                        }
                        let selected = this.connections.selected_rule;
                        this.change_upstream_rules(
                            |rules| {
                                let Some(index) =
                                    rules.iter().position(|rule| Some(rule.id) == selected)
                                else {
                                    return;
                                };
                                match action {
                                    2 => {
                                        rules.remove(index);
                                    }
                                    3 if index > 0 => rules.swap(index, index - 1),
                                    4 if index + 1 < rules.len() => rules.swap(index, index + 1),
                                    _ => {}
                                }
                            },
                            cx,
                        );
                    })),
            );
        }

        let upstream_override = self.state.upstream_proxy.override_project_only;
        let mut body = div()
            .px(px(22.))
            .py(px(14.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(connection_dual_section_header(
                "settings-upstream-heading",
                "上游代理服务器",
                "使用这些设置控制将出站请求发送到上游代理服务器，或直接发送到目标 Web 服务器。将使用与每个目标主机匹配的第一条规则。要将所有流量发送到单个代理服务器，请创建目标主机为 * 的规则。",
                upstream_override,
                true,
            ))
            .child(sleek_override_card(
                "settings-upstream-override",
                upstream_override,
                enabled,
                cx.listener(|this, checked: &bool, _, cx| {
                    let mut next = this.state.upstream_proxy.clone();
                    next.override_project_only = *checked;
                    this.commit_connections(next, cx);
                }),
            ))
            .child(div().pl(px(30.)).child(connection_table(
                &self.connections.rules_table,
                upstream_actions,
                144.,
                "Upstream proxy servers",
            )));

        if self.connections.rule_editor_visible {
            let selected_auth = self
                .connections
                .rule_auth
                .read(cx)
                .selected_value()
                .copied()
                .unwrap_or("None");
            let auth = selected_auth != "None";
            let is_ntlm = matches!(selected_auth, "NTLM v1" | "NTLM v2");
            let mut editor = div()
                .ml(px(30.))
                .p(px(10.))
                .rounded(px(4.))
                .border_1()
                .border_color(theme::border())
                .bg(theme::background())
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(connection_field(
                    "代理协议",
                    Select::new(&self.connections.rule_protocol)
                        .id("settings-upstream-protocol")
                        .small()
                        .w(px(280.))
                        .disabled(!enabled),
                ))
                .child(connection_field(
                    "认证类型",
                    Select::new(&self.connections.rule_auth)
                        .id("settings-upstream-auth-type")
                        .small()
                        .w(px(280.))
                        .disabled(!enabled),
                ));
            for (id, label, input, field_enabled) in [
                (
                    "settings-upstream-destination",
                    "Destination host",
                    &self.connections.rule_destination,
                    true,
                ),
                (
                    "settings-upstream-rule-host",
                    "Proxy host",
                    &self.connections.rule_host,
                    true,
                ),
                (
                    "settings-upstream-rule-port",
                    "Proxy port",
                    &self.connections.rule_port,
                    true,
                ),
                (
                    "settings-upstream-rule-username",
                    "Username",
                    &self.connections.rule_username,
                    auth,
                ),
                (
                    "settings-upstream-rule-password",
                    "Password",
                    &self.connections.rule_password,
                    auth,
                ),
                (
                    "settings-upstream-rule-domain",
                    "Domain",
                    &self.connections.rule_domain,
                    auth || is_ntlm,
                ),
                (
                    "settings-upstream-rule-workstation",
                    "Workstation",
                    &self.connections.rule_workstation,
                    auth || is_ntlm,
                ),
                (
                    "settings-upstream-rule-description",
                    "Description",
                    &self.connections.rule_description,
                    true,
                ),
            ] {
                editor = editor.child(connection_field(
                    label,
                    Input::new(input)
                        .id(id)
                        .aria_label(label)
                        .small()
                        .disabled(!enabled || !field_enabled),
                ));
            }
            body = body.child(
                editor.child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            listeners_tls::mio_action_button(
                                "settings-upstream-rule-save",
                                "保存",
                                !enabled,
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_upstream_rule(window, cx)
                            })),
                        )
                        .child(
                            listeners_tls::mio_action_button(
                                "settings-upstream-rule-cancel",
                                "取消",
                                false,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.connections.rule_editor_visible = false;
                                this.connections.editing_rule = None;
                                cx.notify();
                            })),
                        ),
                ),
            );
        }

        // Section 2: SOCKS proxy
        let socks = self.state.upstream_proxy.effective_socks();
        let socks_summary = if !socks.host.trim().is_empty() && socks.port > 0 {
            format!("已配置代理 {}:{}。", socks.host, socks.port)
        } else {
            "未配置代理。".into()
        };

        let mut bypass_actions = div()
            .w(theme::scaled_px(88.).max(px(84.)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(6.));
        let has_bypass_sel = self
            .connections
            .selected_socks_bypass
            .is_some_and(|idx| idx < self.connections.socks_bypass_hosts.len());
        let has_bypass_items = !self.connections.socks_bypass_hosts.is_empty();
        bypass_actions = bypass_actions
            .child(
                listeners_tls::mio_action_button("settings-socks-bypass-paste", "粘贴", !enabled)
                    .w_full()
                    .on_click(cx.listener(|this, _, _, cx| this.paste_socks_bypass_hosts(cx))),
            )
            .child(
                listeners_tls::mio_action_button(
                    "settings-socks-bypass-load",
                    "加载...",
                    !enabled,
                )
                .w_full()
                .on_click(cx.listener(|this, _, window, cx| {
                    this.load_socks_bypass_hosts(window, cx)
                })),
            )
            .child(
                listeners_tls::mio_action_button(
                    "settings-socks-bypass-remove",
                    "移除",
                    !enabled || !has_bypass_sel,
                )
                .w_full()
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(idx) = this.connections.selected_socks_bypass
                        && idx < this.connections.socks_bypass_hosts.len()
                    {
                        this.connections.socks_bypass_hosts.remove(idx);
                        this.connections.selected_socks_bypass =
                            this.connections.socks_bypass_hosts.len().checked_sub(1);
                        this.commit_socks_bypass_hosts(cx);
                    }
                })),
            )
            .child(
                listeners_tls::mio_action_button(
                    "settings-socks-bypass-clear",
                    "清空",
                    !enabled || !has_bypass_items,
                )
                .w_full()
                .on_click(cx.listener(|this, _, _, cx| {
                    if !this.connections.socks_bypass_hosts.is_empty() {
                        this.connections.socks_bypass_hosts.clear();
                        this.connections.selected_socks_bypass = None;
                        this.commit_socks_bypass_hosts(cx);
                    }
                })),
            );

        let mut bypass_list = div()
            .id("settings-socks-bypass-list")
            .flex_1()
            .min_w_0()
            .h_full()
            .border_1()
            .border_color(listeners_tls::mio_table_border())
            .bg(theme::surface())
            .overflow_y_scroll()
            .flex()
            .flex_col();
        for (idx, host) in self.connections.socks_bypass_hosts.iter().enumerate() {
            let is_sel = self.connections.selected_socks_bypass == Some(idx);
            let zebra = idx % 2 == 1;
            bypass_list = bypass_list.child(
                div()
                    .id(("settings-socks-bypass-row", idx))
                    .test_support()
                    .aria_label(host.clone())
                    .h(theme::scaled_px(22.).max(px(20.)))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .when(is_sel, |this| this.bg(theme::selected()))
                    .when(!is_sel && zebra, |this| {
                        this.bg(rgb(if theme::is_dark() { 0x25272b } else { 0xfafafa }))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.connections.selected_socks_bypass = Some(idx);
                        cx.notify();
                    }))
                    .child(div().min_w_0().flex_1().truncate().child(host.clone())),
            );
        }

        let mut socks_details = div()
            .pl(px(32.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .id("settings-socks-summary")
                    .test_support()
                    .aria_label(socks_summary.clone())
                    .text_color(theme::foreground())
                    .child(socks_summary),
            )
            .child(
                div().flex().child(
                    listeners_tls::mio_action_button("settings-socks-configure", "配置", !enabled)
                        .w(theme::scaled_px(96.).max(px(92.)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.connections.socks_editor_visible =
                                !this.connections.socks_editor_visible;
                            cx.notify();
                        })),
                ),
            );

        if self.connections.socks_editor_visible {
            let dns_owner = cx.entity();
            socks_details = socks_details.child(
                div()
                    .mt(px(2.))
                    .p(px(10.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::background())
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(14.))
                            .child(connection_compact_field(
                                "SOCKS 代理主机",
                                120.,
                                210.,
                                Input::new(&self.upstream_host)
                                    .id("settings-upstream-host")
                                    .aria_label("SOCKS proxy host")
                                    .small()
                                    .disabled(!enabled),
                            ))
                            .child(connection_compact_field(
                                "SOCKS 代理端口",
                                120.,
                                110.,
                                Input::new(&self.upstream_port)
                                    .id("settings-upstream-port")
                                    .aria_label("SOCKS proxy port")
                                    .small()
                                    .disabled(!enabled),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(14.))
                            .child(connection_compact_field(
                                "用户名",
                                120.,
                                210.,
                                Input::new(&self.upstream_username)
                                    .id("settings-upstream-username")
                                    .aria_label("Username")
                                    .small()
                                    .disabled(!enabled),
                            ))
                            .child(connection_compact_field(
                                "密码",
                                120.,
                                210.,
                                Input::new(&self.upstream_password)
                                    .id("settings-upstream-password")
                                    .aria_label("Password")
                                    .small()
                                    .disabled(!enabled),
                            )),
                    )
                    .child(
                        listeners_tls::indexed_checkbox(
                            "settings-socks-dns",
                            "通过 SOCKS 代理执行 DNS 解析",
                            self.connections.socks_dns_remote,
                            enabled,
                        )
                        .on_change(move |value, _, _, cx| {
                            dns_owner.update(cx, |this, cx| {
                                this.connections.socks_dns_remote = value == CheckboxState::Checked;
                                cx.notify();
                            })
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(
                                listeners_tls::mio_action_button(
                                    "settings-upstream-apply",
                                    "保存",
                                    !enabled,
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_socks(window, cx)
                                })),
                            )
                            .child(
                                listeners_tls::mio_action_button(
                                    "settings-upstream-reset",
                                    "重置",
                                    !enabled,
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let mut next = this.state.upstream_proxy.clone();
                                    next.socks = SocksProxySettings::default();
                                    next.protocol = UpstreamProxyProtocol::Direct;
                                    next.sync_legacy_summary();
                                    this.commit_connections(next.clone(), cx);
                                    this.restore_upstream_draft(&next, window, cx);
                                    this.connections.socks_editor_visible = false;
                                    cx.notify();
                                })),
                            )
                            .child(
                                listeners_tls::mio_action_button(
                                    "settings-socks-close",
                                    "收起",
                                    false,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.connections.socks_editor_visible = false;
                                    cx.notify();
                                })),
                            ),
                    ),
            );
        }

        socks_details = socks_details
            .child(
                div()
                    .mt(px(4.))
                    .text_color(theme::foreground())
                    .child("如需指定绕过 SOCKS 代理的主机，请在此处添加。"),
            )
            .child(
                div()
                    .flex()
                    .gap(px(9.))
                    .h(px(126.))
                    .flex_shrink_0()
                    .child(bypass_actions)
                    .child(bypass_list),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .child(
                        listeners_tls::mio_action_button(
                            "settings-socks-bypass-add",
                            "添加",
                            !enabled,
                        )
                        .w(theme::scaled_px(88.).max(px(84.)))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.add_socks_bypass_host(window, cx)
                        })),
                    )
                    .child({
                        let bypass_empty = self
                            .connections
                            .socks_bypass_input
                            .read(cx)
                            .value()
                            .is_empty();
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .child(
                                Input::new(&self.connections.socks_bypass_input)
                                    .id("settings-socks-bypass-input")
                                    .aria_label("输入新条目")
                                    .small()
                                    .disabled(!enabled),
                            )
                            .when(bypass_empty, |this| {
                                this.child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .px(px(9.))
                                        .flex()
                                        .items_center()
                                        .italic()
                                        .text_size(theme::ui_font_size())
                                        .text_color(rgb(if theme::is_dark() {
                                            0xc4c7ce
                                        } else {
                                            0x333333
                                        }))
                                        .child("输入新条目"),
                                )
                            })
                    }),
            );

        let socks_section = div()
            .pl(px(30.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(sleek_mio_switch(
                "settings-socks-enabled",
                "使用 SOCKS 代理",
                self.connections.socks_enabled,
                enabled,
                cx.listener(|this, checked: &bool, window, cx| {
                    this.connections.socks_enabled = *checked;
                    let configured = this.state.upstream_proxy.effective_socks();
                    if *checked && configured.host.trim().is_empty() {
                        this.connections.socks_editor_visible = true;
                        cx.notify();
                    } else if !configured.host.trim().is_empty() && configured.port > 0 {
                        this.save_socks(window, cx);
                    } else {
                        cx.notify();
                    }
                }),
            ))
            .child(socks_details);

        let socks_override = socks.override_project_only;
        body = body
            .child(connection_dual_section_header(
                "settings-socks-heading",
                "SOCKS 代理",
                "使用这些设置配置 MIO 对所有出站通信使用 SOCKS 代理。此设置在 TCP 层生效，所有出站请求都会经由此代理发送。如果同时配置了上游 HTTP 代理规则，发往上游代理的请求也会经由此处配置的 SOCKS 代理发送。",
                socks_override,
                false,
            ))
            .child(sleek_override_card(
                "settings-socks-override",
                socks_override,
                enabled,
                cx.listener(|this, checked: &bool, _, cx| {
                    let mut next = this.state.upstream_proxy.clone();
                    let real_rules = next.effective_rules();
                    next.retain_with_platform_auth(real_rules);
                    next.socks = next.effective_socks();
                    next.socks.override_project_only = *checked;
                    next.sync_legacy_summary();
                    this.commit_connections(next, cx);
                }),
            ))
            .child(socks_section)
            .child(connection_project_section_header(
                "settings-timeouts-heading",
                "超时",
                "使用这些设置指定各项网络任务的超时时间（单位：秒）。若设为 0 或留空则视为无超时限制，失焦或按回车后自动应用。",
            ))
            .child(
                div()
                    .pl(px(30.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(connection_compact_field(
                        "普通超时（秒）:",
                        200.,
                        92.,
                        Input::new(&self.connections.normal_timeout_input)
                            .id("settings-normal-timeout-input")
                            .aria_label("普通超时（秒）")
                            .small()
                            .disabled(!enabled),
                    ))
                    .child(connection_compact_field(
                        "开放式响应超时（秒）:",
                        200.,
                        92.,
                        Input::new(&self.connections.open_ended_timeout_input)
                            .id("settings-open-ended-timeout-input")
                            .aria_label("开放式响应超时（秒）")
                            .small()
                            .disabled(!enabled),
                    )),
            );

        // Section 4: Hostname resolution
        let mut hostname_actions = div()
            .w(theme::scaled_px(88.).max(px(84.)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(6.));
        for (id, label, action) in [
            ("settings-hostname-add", "添加", 0u8),
            ("settings-hostname-edit", "编辑", 1),
            ("settings-hostname-remove", "移除", 2),
        ] {
            hostname_actions = hostname_actions.child(
                listeners_tls::mio_action_button(
                    id,
                    label,
                    !enabled || (action > 0 && self.connections.selected_dns.is_none()),
                )
                .w_full()
                .on_click(cx.listener(move |this, _, window, cx| {
                    if action < 2 {
                        this.open_hostname_editor(action == 0, window, cx);
                    } else {
                        let mut next = this.state.upstream_proxy.clone();
                        next.hostname_resolutions
                            .retain(|entry| Some(entry.id) != this.connections.selected_dns);
                        this.commit_connections(next, cx);
                    }
                })),
            );
        }

        body = body
            .child(connection_project_section_header(
                "settings-hostname-heading",
                "主机名解析",
                "使用这些设置覆盖指定主机名的 DNS 解析结果并路由至目标 IP 地址。",
            ))
            .child(div().pl(px(30.)).child(connection_table(
                &self.connections.dns_table,
                hostname_actions,
                124.,
                "Hostname resolution",
            )));

        if self.connections.dns_editor_visible {
            body = body.child(
                div()
                    .ml(px(30.))
                    .p(px(10.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::background())
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(connection_field(
                        "Hostname",
                        Input::new(&self.connections.dns_hostname_input)
                            .id("settings-hostname-input")
                            .small()
                            .disabled(!enabled),
                    ))
                    .child(connection_field(
                        "IP address",
                        Input::new(&self.connections.dns_ip_input)
                            .id("settings-hostname-ip-input")
                            .small()
                            .disabled(!enabled),
                    ))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(
                                listeners_tls::mio_action_button(
                                    "settings-hostname-save",
                                    "保存",
                                    !enabled,
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_hostname_rule(window, cx)
                                })),
                            )
                            .child(
                                listeners_tls::mio_action_button(
                                    "settings-hostname-cancel",
                                    "取消",
                                    false,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.connections.dns_editor_visible = false;
                                    this.connections.editing_dns = None;
                                    cx.notify();
                                })),
                            ),
                    ),
            );
        }

        // Section 5: Platform authentication
        let plat_rules = self.state.upstream_proxy.platform_auth_rules();
        let plat_selected = self
            .connections
            .selected_platform_auth
            .and_then(|id| plat_rules.iter().position(|r| r.id == id));
        let mut plat_actions = div()
            .w(theme::scaled_px(88.).max(px(84.)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(6.));
        for (id, label, action, available) in [
            ("settings-platform-auth-add", "添加", 0u8, true),
            ("settings-platform-auth-edit", "编辑", 1, plat_selected.is_some()),
            (
                "settings-platform-auth-remove",
                "移除",
                2,
                plat_selected.is_some(),
            ),
            (
                "settings-platform-auth-up",
                "上移",
                3,
                plat_selected.is_some_and(|idx| idx > 0),
            ),
            (
                "settings-platform-auth-down",
                "下移",
                4,
                plat_selected.is_some_and(|idx| idx + 1 < plat_rules.len()),
            ),
        ] {
            plat_actions = plat_actions.child(
                listeners_tls::mio_action_button(id, label, !enabled || !available)
                    .w_full()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if action < 2 {
                            this.open_platform_auth_editor(action == 0, window, cx);
                            return;
                        }
                        let selected = this.connections.selected_platform_auth;
                        let mut next = this.state.upstream_proxy.clone();
                        let mut rules = next.platform_auth_rules();
                        let Some(index) = rules.iter().position(|r| Some(r.id) == selected) else {
                            return;
                        };
                        match action {
                            2 => {
                                rules.remove(index);
                            }
                            3 if index > 0 => rules.swap(index, index - 1),
                            4 if index + 1 < rules.len() => rules.swap(index, index + 1),
                            _ => {}
                        }
                        next.set_platform_auth_rules(rules);
                        this.commit_connections(next, cx);
                    })),
            );
        }

        let plat_override = self
            .state
            .upstream_proxy
            .platform_auth_override_project_only();
        let plat_enabled = self.state.upstream_proxy.platform_auth_enabled();
        let plat_prompt = self.state.upstream_proxy.platform_auth_prompt_on_failure();
        body = body
            .child(connection_dual_section_header(
                "settings-platform-auth-heading",
                "平台认证",
                "使用这些设置配置 MIO 自动对目标 Web 服务器执行 HTTP 平台认证（支持 Basic、Digest、NTLM v1 与 NTLM v2）。将使用与每个目标主机匹配的第一条启用规则。",
                plat_override,
                false,
            ))
            .child(sleek_override_card(
                "settings-platform-auth-override",
                plat_override,
                enabled,
                cx.listener(|this, checked: &bool, _, cx| {
                    let mut next = this.state.upstream_proxy.clone();
                    next.set_platform_auth_override_project_only(*checked);
                    this.commit_connections(next, cx);
                }),
            ))
            .child(
                div()
                    .pl(px(30.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(sleek_mio_switch(
                        "settings-platform-auth-enabled",
                        "执行平台认证 (Do platform authentication)",
                        plat_enabled,
                        enabled,
                        cx.listener(|this, checked: &bool, _, cx| {
                            let mut next = this.state.upstream_proxy.clone();
                            next.set_platform_auth_enabled(*checked);
                            this.commit_connections(next, cx);
                        }),
                    ))
                    .child(sleek_mio_switch(
                        "settings-platform-auth-prompt",
                        "平台认证失败时提示输入凭据 (Prompt for credentials on platform authentication failure)",
                        plat_prompt,
                        enabled,
                        cx.listener(|this, checked: &bool, _, cx| {
                            let mut next = this.state.upstream_proxy.clone();
                            next.set_platform_auth_prompt_on_failure(*checked);
                            this.commit_connections(next, cx);
                        }),
                    ))
                    .child(connection_table(
                        &self.connections.platform_table,
                        plat_actions,
                        144.,
                        "Platform authentication",
                    )),
            );

        if self.connections.platform_editor_visible {
            let mut plat_editor = div()
                .ml(px(30.))
                .p(px(10.))
                .rounded(px(4.))
                .border_1()
                .border_color(theme::border())
                .bg(theme::background())
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(connection_field(
                    "Destination host",
                    Input::new(&self.connections.platform_destination)
                        .id("settings-platform-auth-destination")
                        .aria_label("Destination host")
                        .small()
                        .disabled(!enabled),
                ))
                .child(connection_field(
                    "Auth type",
                    Select::new(&self.connections.platform_auth_type)
                        .id("settings-platform-auth-type")
                        .small()
                        .w(px(280.))
                        .disabled(!enabled),
                ));
            for (id, label, input) in [
                (
                    "settings-platform-auth-username",
                    "Username",
                    &self.connections.platform_username,
                ),
                (
                    "settings-platform-auth-password",
                    "Password",
                    &self.connections.platform_password,
                ),
                (
                    "settings-platform-auth-domain",
                    "Domain",
                    &self.connections.platform_domain,
                ),
                (
                    "settings-platform-auth-hostname",
                    "Hostname",
                    &self.connections.platform_hostname,
                ),
            ] {
                plat_editor = plat_editor.child(connection_field(
                    label,
                    Input::new(input)
                        .id(id)
                        .aria_label(label)
                        .small()
                        .disabled(!enabled),
                ));
            }
            body = body.child(
                plat_editor.child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            listeners_tls::mio_action_button(
                                "settings-platform-auth-save",
                                "保存",
                                !enabled,
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_platform_auth_rule(window, cx)
                            })),
                        )
                        .child(
                            listeners_tls::mio_action_button(
                                "settings-platform-auth-cancel",
                                "取消",
                                false,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.connections.platform_editor_visible = false;
                                this.connections.editing_platform_auth = None;
                                cx.notify();
                            })),
                        ),
                ),
            );
        }

        let content = body.when_some(
            self.connections
                .error
                .clone()
                .or_else(|| self.state.error.clone()),
            |this, error| {
                this.child(
                    div()
                        .id("settings-upstream-error")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .ml(px(30.))
                        .p(px(8.))
                        .rounded(px(4.))
                        .bg(theme::danger_bg())
                        .text_color(theme::danger_fg())
                        .child(error),
                )
            },
        );
        settings_scrollable_pane("settings-connections-page", &self.content_scroll, content)
            .into_any_element()
    }
}

fn sleek_mio_switch(
    id: &'static str,
    label: &'static str,
    checked: bool,
    enabled: bool,
    on_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> BaseSwitch {
    let dark = theme::is_dark();
    let track_bg: gpui_kit::Hsla = if checked {
        settings_blue()
    } else {
        rgb(if dark { 0x4e535b } else { 0xb0afaf }).into()
    };
    let thumb_bg = if checked {
        rgb(0xffffff)
    } else if dark {
        rgb(0xd4d4d4)
    } else {
        rgb(0x2b2b2b)
    };
    BaseSwitch::new(id)
        .checked(checked)
        .disabled(!enabled)
        .accessibility_label(label)
        .flex()
        .items_center()
        .gap(px(8.))
        .cursor_pointer()
        .styles(|styles| styles.disabled(|style| style.opacity(0.5).cursor_not_allowed()))
        .on_change(move |next, _, window, cx| on_change(&next, window, cx))
        .child(
            div()
                .w(px(24.))
                .h(px(13.))
                .flex_shrink_0()
                .rounded_full()
                .p(px(2.))
                .flex()
                .items_center()
                .when(checked, |this| this.justify_end())
                .when(!checked, |this| this.justify_start())
                .bg(track_bg)
                .child(div().size(px(9.)).rounded_full().bg(thumb_bg)),
        )
        .child(
            div()
                .text_size(theme::ui_font_size())
                .text_color(theme::foreground())
                .child(label),
        )
}

fn sleek_override_card(
    id: &'static str,
    checked: bool,
    enabled: bool,
    on_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> gpui_kit::Div {
    div()
        .ml(px(30.))
        .px(px(12.))
        .py(px(9.))
        .rounded(px(3.))
        .border_1()
        .border_color(listeners_tls::mio_table_border())
        .bg(theme::surface())
        .child(sleek_mio_switch(
            id,
            "仅为当前项目覆盖选项",
            checked,
            enabled,
            on_change,
        ))
}

fn connection_dual_section_header(
    id: &'static str,
    title: &'static str,
    desc: &'static str,
    override_project: bool,
    first: bool,
) -> gpui_kit::Div {
    div()
        .when(!first, |this| {
            this.mt(px(6.))
                .pt(px(12.))
                .border_t_1()
                .border_color(theme::border())
        })
        .flex()
        .flex_col()
        .gap(px(5.))
        .child(listeners_tls::mio_section_heading(
            id,
            title,
            listeners_tls::mio_dual_scope_pills(override_project),
        ))
        .child(listeners_tls::mio_desc_row(desc))
}

fn connection_project_section_header(
    id: &'static str,
    title: &'static str,
    desc: &'static str,
) -> gpui_kit::Div {
    div()
        .mt(px(6.))
        .pt(px(12.))
        .border_t_1()
        .border_color(theme::border())
        .flex()
        .flex_col()
        .gap(px(5.))
        .child(listeners_tls::mio_section_heading(
            id,
            title,
            listeners_tls::mio_project_pill(),
        ))
        .child(listeners_tls::mio_desc_row(desc))
}

fn connection_compact_field(
    label: &'static str,
    label_w: f32,
    input_w: f32,
    input: impl IntoElement,
) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(div().w(px(label_w)).flex_shrink_0().child(label))
        .child(div().w(px(input_w)).min_w_0().child(input))
}

fn connection_field(label: &'static str, input: impl IntoElement) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(div().w(px(180.)).flex_shrink_0().child(label))
        .child(div().w(px(300.)).min_w_0().child(input))
}

fn connection_table(
    table: &Entity<TableState<ConnectionsTable>>,
    actions: gpui_kit::Div,
    height: f32,
    label: &'static str,
) -> gpui_kit::Div {
    div()
        .flex()
        .gap(px(9.))
        .h(px(height))
        .flex_shrink_0()
        .child(actions)
        .child(listeners_tls::mio_table_frame(table, label))
}

#[cfg(test)]
mod tests {
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{ScrollDelta, TestAppContext, point};
    use mio_core::{UpstreamAuthType, UpstreamProxyProtocol};

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
}
