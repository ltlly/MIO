use std::path::Path;

use gpui_kit::base::CheckboxState;
use gpui_kit::component::{
    IndexPath,
    select::{SelectEvent, SelectState},
    table::{Column, TableDelegate},
};
use mio_core::{UpstreamProxyProtocol, Uuid};

use super::*;

const MAX_BYPASS_FILE_BYTES: u64 = 128 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConnectionsTableKind {
    Upstream,
    Hostname,
    PlatformAuth,
}

pub(in crate::workbench::settings) struct ConnectionsUi {
    pub(super) rules_table: Entity<TableState<ConnectionsTable>>,
    pub(super) dns_table: Entity<TableState<ConnectionsTable>>,
    pub(super) platform_table: Entity<TableState<ConnectionsTable>>,
    pub(super) selected_rule: Option<Uuid>,
    pub(super) selected_dns: Option<Uuid>,
    pub(super) selected_platform_auth: Option<Uuid>,
    pub(in crate::workbench::settings) editing_rule: Option<Uuid>,
    pub(in crate::workbench::settings) editing_dns: Option<Uuid>,
    pub(in crate::workbench::settings) editing_platform_auth: Option<Uuid>,
    pub(in crate::workbench::settings) rule_editor_visible: bool,
    pub(in crate::workbench::settings) dns_editor_visible: bool,
    pub(in crate::workbench::settings) platform_editor_visible: bool,
    pub(in crate::workbench::settings) socks_editor_visible: bool,
    pub(super) rule_protocol: Entity<SelectState<Vec<&'static str>>>,
    pub(super) rule_auth: Entity<SelectState<Vec<&'static str>>>,
    pub(super) platform_auth_type: Entity<SelectState<Vec<&'static str>>>,
    pub(in crate::workbench::settings) rule_destination: Entity<InputState>,
    pub(in crate::workbench::settings) rule_host: Entity<InputState>,
    pub(in crate::workbench::settings) rule_port: Entity<InputState>,
    pub(in crate::workbench::settings) rule_username: Entity<InputState>,
    pub(in crate::workbench::settings) rule_password: Entity<InputState>,
    pub(in crate::workbench::settings) rule_domain: Entity<InputState>,
    pub(in crate::workbench::settings) rule_workstation: Entity<InputState>,
    pub(in crate::workbench::settings) rule_description: Entity<InputState>,
    pub(in crate::workbench::settings) platform_destination: Entity<InputState>,
    pub(in crate::workbench::settings) platform_username: Entity<InputState>,
    pub(in crate::workbench::settings) platform_password: Entity<InputState>,
    pub(in crate::workbench::settings) platform_domain: Entity<InputState>,
    pub(in crate::workbench::settings) platform_hostname: Entity<InputState>,
    pub(super) socks_enabled: bool,
    pub(super) socks_dns_remote: bool,
    pub(super) socks_bypass_hosts: Vec<String>,
    pub(super) selected_socks_bypass: Option<usize>,
    pub(in crate::workbench::settings) socks_bypass_input: Entity<InputState>,
    pub(in crate::workbench::settings) normal_timeout_input: Entity<InputState>,
    pub(in crate::workbench::settings) open_ended_timeout_input: Entity<InputState>,
    pub(in crate::workbench::settings) dns_hostname_input: Entity<InputState>,
    pub(in crate::workbench::settings) dns_ip_input: Entity<InputState>,
    pub(in crate::workbench::settings) error: Option<String>,
}

impl ConnectionsUi {
    pub(in crate::workbench::settings) fn new(
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
            cx.subscribe_in(&table, window, move |this, _, event, window, cx| match event {
                TableEvent::SelectRow(row) => {
                    match kind {
                        ConnectionsTableKind::Upstream => {
                            this.connections.selected_rule = this
                                .state
                                .upstream_proxy
                                .rules
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
                                .platform_auth
                                .rules
                                .get(*row)
                                .map(|rule| rule.id);
                        }
                    }
                    cx.notify();
                }
                TableEvent::DoubleClickedRow(row) => {
                    match kind {
                        ConnectionsTableKind::Upstream => {
                            this.connections.selected_rule = this
                                .state
                                .upstream_proxy
                                .rules
                                .get(*row)
                                .map(|rule| rule.id);
                            if this.connections.selected_rule.is_some() && this.state.enabled {
                                this.open_upstream_editor(false, window, cx);
                            }
                        }
                        ConnectionsTableKind::Hostname => {
                            this.connections.selected_dns = this
                                .state
                                .upstream_proxy
                                .hostname_resolutions
                                .get(*row)
                                .map(|rule| rule.id);
                            if this.connections.selected_dns.is_some() && this.state.enabled {
                                this.open_hostname_editor(false, window, cx);
                            }
                        }
                        ConnectionsTableKind::PlatformAuth => {
                            this.connections.selected_platform_auth = this
                                .state
                                .upstream_proxy
                                .platform_auth
                                .rules
                                .get(*row)
                                .map(|rule| rule.id);
                            if this.connections.selected_platform_auth.is_some() && this.state.enabled
                            {
                                this.open_platform_auth_editor(false, window, cx);
                            }
                        }
                    }
                    cx.notify();
                }
                _ => {}
            })
            .detach();
            table
        });
        let rule_protocol = cx.new(|scx| {
            SelectState::new(
                vec!["HTTP", "HTTPS", "Direct"],
                Some(IndexPath::new(0)),
                window,
                scx,
            )
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
        let socks_bypass_input =
            cx.new(|icx| InputState::new(window, icx).placeholder("输入新条目"));
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
        let socks = &state.upstream_proxy.socks;
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
            rule_password: cx.new(|icx| InputState::new(window, icx).masked(true)),
            rule_domain: cx.new(|icx| InputState::new(window, icx).placeholder("可选 NTLM 域")),
            rule_workstation: cx
                .new(|icx| InputState::new(window, icx).placeholder("可选 NTLM 工作站")),
            rule_description: cx.new(|icx| InputState::new(window, icx).placeholder("可选描述")),
            platform_destination: cx
                .new(|icx| InputState::new(window, icx).default_value("*")),
            platform_username: cx.new(|icx| InputState::new(window, icx)),
            platform_password: cx.new(|icx| InputState::new(window, icx).masked(true)),
            platform_domain: cx.new(|icx| InputState::new(window, icx).placeholder("可选域")),
            platform_hostname: cx
                .new(|icx| InputState::new(window, icx).placeholder("可选主机名")),
            socks_enabled: socks.use_proxy,
            socks_dns_remote: socks.dns_over_socks,
            socks_bypass_hosts: socks.bypass_hosts.clone(),
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

pub(in crate::workbench::settings) struct ConnectionsTable {
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
                let rule = settings.state.upstream_proxy.platform_auth.rules.get(row)?;
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
                let rule = settings.state.upstream_proxy.rules.get(row)?;
                let is_direct = rule.protocol == UpstreamProxyProtocol::Direct;
                Some(match column {
                    0 => if rule.enabled { "Yes" } else { "No" }.into(),
                    1 => rule.destination_host.clone(),
                    2 => {
                        if is_direct {
                            "(直连 / Direct)".into()
                        } else {
                            rule.host.clone()
                        }
                    }
                    3 => {
                        if is_direct && rule.port == 0 {
                            String::new()
                        } else {
                            rule.port.to_string()
                        }
                    }
                    4 => {
                        if is_direct {
                            "Direct".into()
                        } else {
                            rule.auth_type.label().into()
                        }
                    }
                    5 => rule.username.clone(),
                    _ => {
                        if !rule.description.is_empty() {
                            rule.description.clone()
                        } else if rule.protocol == UpstreamProxyProtocol::Https {
                            "HTTPS".into()
                        } else if is_direct {
                            "Direct".into()
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
                    settings.state.upstream_proxy.rules.len()
                }
                ConnectionsTableKind::Hostname => {
                    settings.state.upstream_proxy.hostname_resolutions.len()
                }
                ConnectionsTableKind::PlatformAuth => {
                    settings.state.upstream_proxy.platform_auth.rules.len()
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
                    let Some(rule) = settings.state.upstream_proxy.platform_auth.rules.get(row)
                    else {
                        return div().into_any_element();
                    };
                    (rule.id, rule.enabled, settings.state.enabled)
                }
                ConnectionsTableKind::Upstream => {
                    let Some(rule) = settings.state.upstream_proxy.rules.get(row) else {
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
                            if let Some(entry) = next
                                .hostname_resolutions
                                .iter_mut()
                                .find(|entry| entry.id == id)
                            {
                                entry.enabled = value == CheckboxState::Checked;
                                this.commit_connections(next, cx);
                            }
                        }
                        ConnectionsTableKind::PlatformAuth => {
                            let mut next = this.state.upstream_proxy.clone();
                            if let Some(entry) = next
                                .platform_auth
                                .rules
                                .iter_mut()
                                .find(|entry| entry.id == id)
                            {
                                entry.enabled = value == CheckboxState::Checked;
                                this.commit_connections(next, cx);
                            }
                        }
                        ConnectionsTableKind::Upstream => {
                            this.change_upstream_rules(
                                |rules| {
                                    if let Some(entry) = rules.iter_mut().find(|entry| entry.id == id)
                                    {
                                        entry.enabled = value == CheckboxState::Checked;
                                    }
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

pub(super) fn read_bypass_list_file(path: &Path) -> Result<Vec<String>, String> {
    let raw = mio_core::ui_files::read_regular_file_bounded(
        path,
        MAX_BYPASS_FILE_BYTES,
        true,
        "绕过列表文件",
    )?;
    let text = String::from_utf8(raw)
        .map_err(|_| "绕过列表文件必须为 UTF-8 编码文本。".to_owned())?;
    Ok(text
        .split([',', ';', '\r', '\n'])
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect())
}
