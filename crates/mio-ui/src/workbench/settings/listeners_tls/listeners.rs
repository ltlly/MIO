use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
};

use gpui_kit::accesskit::Role;
use gpui_kit::assets::IconName;
use gpui_kit::base::CheckboxState;
use gpui_kit::component::IndexPath;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::component::table::{Column, TableDelegate};
use gpui_kit::{Bounds, PathPromptOptions, WindowOptions, size};
use mio_core::{MAX_CLIENT_CERT_BYTES, ProxyListenerCertMode, ProxyListenerConfig, Uuid};

use super::*;
use crate::CloseSettings;

fn read_bounded_pem_text(path: &Path, label: &str) -> Result<String, String> {
    let raw = mio_core::ui_files::read_regular_file_bounded(
        path,
        MAX_CLIENT_CERT_BYTES as u64,
        false,
        label,
    )?;
    let text = String::from_utf8(raw).map_err(|_| format!("{label}必须是有效 UTF-8 PEM 文本。"))?;
    if text.contains('\0') {
        return Err(format!("{label}包含非法 NUL 字节。"));
    }
    Ok(text)
}

fn parse_listener_address(address: &str) -> Result<SocketAddr, String> {
    let address = address.trim();
    let address = if let Ok(port) = address.parse::<u16>() {
        format!("127.0.0.1:{port}")
    } else {
        address.to_owned()
    };
    address.parse().map_err(|_| {
        "监听地址须为端口或 IP:端口（如 8080、127.0.0.1:8080、0.0.0.0:8080）；端口 0 自动选择空闲端口。".to_owned()
    })
}

fn next_available_listener_port(existing: &[ProxyListenerConfig]) -> u16 {
    let used: std::collections::HashSet<u16> = existing
        .iter()
        .filter_map(|item| item.bind_address.trim().parse::<SocketAddr>().ok())
        .map(|addr| addr.port())
        .collect();
    (8081..=u16::MAX)
        .find(|port| !used.contains(port))
        .unwrap_or(8081)
}

pub(crate) struct ProxyListenersTable {
    pub(super) owner: WeakEntity<Settings>,
}

const LISTENER_COLUMNS: [(&str, f32); 7] = [
    ("运行中", 68.),
    ("监听接口", 150.),
    ("隐形代理", 70.),
    ("重定向", 96.),
    ("证书", 116.),
    ("TLS 协议", 208.),
    ("支持 HTTP/2", 112.),
];

impl TableDelegate for ProxyListenersTable {
    fn columns_count(&self, _: &App) -> usize {
        LISTENER_COLUMNS.len()
    }

    fn column(&self, index: usize, _: &App) -> Column {
        let scale = theme::ui_scale();
        Column::new(index.to_string(), LISTENER_COLUMNS[index].0)
            .width((LISTENER_COLUMNS[index].1 * scale).round())
            .p_0()
            .movable(false)
    }

    fn rows_count(&self, cx: &App) -> usize {
        self.owner
            .upgrade()
            .map_or(0, |owner| owner.read(cx).state.proxy_listeners.len())
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        settings_table_th(LISTENER_COLUMNS[col_ix].0, LISTENER_COLUMNS[col_ix].1)
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }

    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }

    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let is_selected = self.owner.upgrade().is_some_and(|owner| {
            let settings = owner.read(cx);
            row < settings.state.proxy_listeners.len()
                && settings.listeners_tls.selected_index == Some(row)
        });
        let selected_border = rgb(if theme::is_dark() { 0x5c8ec4 } else { 0x4f7eb3 });
        div()
            .id(("settings-listener-row", row))
            .when(is_selected, |this| {
                this.border_t_1()
                    .border_b_1()
                    .border_color(selected_border)
            })
            .when(!is_selected, |this| this.border_b_0())
    }

    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(owner) = self.owner.upgrade() else {
            return div().into_any_element();
        };
        let (item, enabled, active_address, project, owner_wb) = {
            let settings = owner.read(cx);
            let state = &settings.state;
            let Some(item) = state.proxy_listeners.get(row).cloned() else {
                return div().into_any_element();
            };
            let enabled = settings.proxy_listeners_available();
            let first_enabled_idx = state.proxy_listeners.iter().position(|item| item.enabled);
            let active_addr = if !state.active_proxy_listeners.is_empty() {
                state
                    .active_proxy_listeners
                    .iter()
                    .find(|(bound_id, _)| {
                        *bound_id == item.id
                            || (first_enabled_idx == Some(row)
                                && *bound_id == mio_core::Uuid::from_u128(1))
                    })
                    .map(|(_, bound)| *bound)
            } else {
                state
                    .active_proxy_address
                    .and_then(|bound| {
                        let matches_exact = item
                            .bind_address
                            .parse::<SocketAddr>()
                            .is_ok_and(|configured| configured == bound);
                        let matches_primary_ephemeral = first_enabled_idx == Some(row)
                            && item
                                .bind_address
                                .parse::<SocketAddr>()
                                .is_ok_and(|configured| configured.port() == 0);
                        (item.enabled && (matches_exact || matches_primary_ephemeral))
                            .then_some(bound)
                    })
                    .or_else(|| (row == 0).then_some(state.active_proxy_address).flatten())
            };
            (
                item,
                enabled,
                active_addr,
                state.project,
                settings.owner.clone(),
            )
        };
        match column {
            0 => {
                let entity = self.owner.clone();
                settings_table_checkbox_cell(
                    indexed_checkbox(
                        ("settings-listener-toggle", row),
                        "",
                        active_address.is_some(),
                        enabled,
                    )
                    .p_0()
                    .gap_0()
                    .accessibility_label(format!("运行监听器 {}", item.bind_address))
                    .tab_stop(false)
                    .on_change(move |state, _, window, cx| {
                        let checked = state == CheckboxState::Checked;
                        let result = owner_wb.update(cx, |view, cx| {
                            view.toggle_proxy_listener(project, row, checked, window, cx)
                        });
                        if let Some(settings) = entity.upgrade() {
                            let wb_snap = owner_wb.upgrade();
                            settings.update(cx, |this, cx| {
                                if let Some(wb) = wb_snap {
                                    this.state.refresh(wb.read(cx));
                                }
                                this.listeners_tls.table.update(cx, |table, cx| {
                                    table.refresh(cx);
                                    cx.notify();
                                });
                                this.listeners_tls.listener_error = match result {
                                    Ok(Ok(())) => None,
                                    Ok(Err(error)) => Some(error),
                                    Err(_) => Some("主工作台已关闭；监听器未修改。".into()),
                                };
                                cx.notify();
                            });
                        }
                    }),
                )
                .into_any_element()
            }
            1 => {
                let text = match active_address {
                    Some(active)
                        if item.bind_address.trim().parse::<SocketAddr>().ok() != Some(active) =>
                    {
                        format!("{} (实际 {active})", item.bind_address)
                    }
                    _ => item.bind_address.clone(),
                };
                div().px(px(6.)).truncate().child(text).into_any_element()
            }
            6 => {
                let item_toggle = item.clone();
                let entity = self.owner.clone();
                settings_table_checkbox_cell(
                    indexed_checkbox(
                        ("settings-listener-http2", row),
                        "",
                        item.enable_http2,
                        enabled,
                    )
                    .p_0()
                    .gap_0()
                    .accessibility_label(format!("支持 HTTP/2 {}", item.bind_address))
                    .tab_stop(false)
                    .on_change(move |state, _, window, cx| {
                        let mut updated = item_toggle.clone();
                        updated.enable_http2 = state == CheckboxState::Checked;
                        let result = owner_wb.update(cx, |view, cx| {
                            view.edit_proxy_listener_config(project, row, updated, window, cx)
                        });
                        if let Some(settings) = entity.upgrade() {
                            let wb_snap = owner_wb.upgrade();
                            settings.update(cx, |this, cx| {
                                if let Some(wb) = wb_snap {
                                    this.state.refresh(wb.read(cx));
                                }
                                this.listeners_tls.table.update(cx, |table, cx| {
                                    table.refresh(cx);
                                    cx.notify();
                                });
                                this.listeners_tls.listener_error = match result {
                                    Ok(Ok(())) => None,
                                    Ok(Err(error)) => Some(error),
                                    Err(_) => Some("主工作台已关闭；监听器未修改。".into()),
                                };
                                cx.notify();
                            });
                        }
                    }),
                )
                .into_any_element()
            }
            _ => div()
                .px(px(6.))
                .truncate()
                .child(listener_column_text(&item, column))
                .into_any_element(),
        }
    }

    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        let owner = self.owner.upgrade()?;
        let state = &owner.read(cx).state;
        let item = state.proxy_listeners.get(row)?;
        Some(match column {
            0 => {
                let first_enabled_idx = state.proxy_listeners.iter().position(|entry| entry.enabled);
                let running = if !state.active_proxy_listeners.is_empty() {
                    state.active_proxy_listeners.iter().any(|(id, _)| {
                        *id == item.id
                            || (first_enabled_idx == Some(row)
                                && *id == mio_core::Uuid::from_u128(1))
                    })
                } else {
                    row == 0 && state.active_proxy_address.is_some()
                };
                if running { "是" } else { "否" }.into()
            }
            1 => item.bind_address.clone().into(),
            _ => listener_column_text(item, column).into(),
        })
    }
}

fn listener_column_text(item: &ProxyListenerConfig, column: usize) -> String {
    match column {
        2 => if item.invisible_proxying { "是" } else { "" }.into(),
        3 => {
            if item.redirect_to_host.is_empty() && item.redirect_to_port.is_none() {
                String::new()
            } else if let Some(port) = item.redirect_to_port {
                format!("{}:{port}", item.redirect_to_host)
            } else {
                item.redirect_to_host.clone()
            }
        }
        4 => match item.certificate_mode {
            ProxyListenerCertMode::PerHostCaSigned => "按主机签发",
            ProxyListenerCertMode::SelfSigned => "自签名",
            ProxyListenerCertMode::SpecificHostname => "指定主机名",
            ProxyListenerCertMode::CustomCertificate => "自定义",
        }
        .into(),
        5 => match item.tls_protocols_label.as_str() {
            "" => "默认".into(),
            "TLSv1.3 only" => "仅 TLSv1.3".into(),
            "TLSv1.2 only" => "仅 TLSv1.2".into(),
            other => other.to_owned(),
        },
        _ => if item.enable_http2 { "是" } else { "否" }.into(),
    }
}

impl Workbench {
    fn apply_proxy_listeners_change(
        &mut self,
        project: Option<ProjectKey>,
        next: Vec<ProxyListenerConfig>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return Err("当前项目尚未就绪或后台不可用；监听器未修改。".into());
        }
        let mut candidate = self.workspace.clone();
        candidate.proxy_listeners = next;
        candidate
            .validate_structure()
            .map_err(|error| error.to_string())?;
        let changed = self.workspace.proxy_listeners != candidate.proxy_listeners;
        self.workspace.proxy_listeners = candidate.proxy_listeners;
        let primary = self.workspace.proxy_listener_or_default().to_owned();
        self.listener_input
            .update(cx, |input, cx| input.set_value(primary, window, cx));
        if changed {
            self.save_workspace(cx);
        }
        self.error = None;
        cx.notify();
        Ok(())
    }

    pub(in crate::workbench) fn add_proxy_listener_config(
        &mut self,
        project: Option<ProjectKey>,
        config: ProxyListenerConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let listen = parse_listener_address(&config.bind_address)?;
        let mut next = self.workspace.effective_proxy_listeners();
        if config.enabled
            && listen.port() != 0
            && next.iter().any(|item| {
                item.enabled
                    && item
                        .bind_address
                        .parse::<SocketAddr>()
                        .is_ok_and(|existing| mio_core::proxy_listener_addrs_overlap(existing, listen))
            })
        {
            return Err(format!("监听地址 {listen} 与已启用监听器存在端口冲突。"));
        }
        next.push(config);
        self.apply_proxy_listeners_change(project, next, window, cx)
    }

    pub(in crate::workbench) fn edit_proxy_listener_config(
        &mut self,
        project: Option<ProjectKey>,
        index: usize,
        config: ProxyListenerConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let listen = parse_listener_address(&config.bind_address)?;
        let mut next = self.workspace.effective_proxy_listeners();
        let target_index = if next.get(index).is_some_and(|item| item.id == config.id) {
            index
        } else if let Some(pos) = next.iter().position(|item| item.id == config.id) {
            pos
        } else {
            return Err("待编辑的监听器已不存在。".into());
        };
        if config.enabled
            && listen.port() != 0
            && next.iter().enumerate().any(|(other_idx, item)| {
                other_idx != target_index
                    && item.enabled
                    && item
                        .bind_address
                        .parse::<SocketAddr>()
                        .is_ok_and(|existing| mio_core::proxy_listener_addrs_overlap(existing, listen))
            })
        {
            return Err(format!("监听地址 {listen} 与已启用监听器存在端口冲突。"));
        }
        next[target_index] = config;
        self.apply_proxy_listeners_change(project, next, window, cx)
    }

    pub(in crate::workbench) fn toggle_proxy_listener(
        &mut self,
        project: Option<ProjectKey>,
        index: usize,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if project.is_none()
            || self.project != project
            || !self.backend_available()
            || self.transition.is_some()
        {
            return Err("当前项目尚未就绪或后台不可用；监听器未修改。".into());
        }
        if self.proxy_busy {
            return Ok(());
        }
        let listeners = self.workspace.effective_proxy_listeners();
        let Some(item) = listeners.get(index).cloned() else {
            return Err("待操作的监听器已不存在。".into());
        };
        let project_key = project.unwrap();
        if (self.proxy_address.is_some() || !self.proxy_listeners_bound.is_empty()) && !enabled {
            let first_enabled_idx = listeners.iter().position(|entry| entry.enabled);
            let bound_id = self
                .proxy_listeners_bound
                .iter()
                .find(|(bound_id, _)| {
                    *bound_id == item.id
                        || (first_enabled_idx == Some(index) && *bound_id == Uuid::from_u128(1))
                })
                .map(|(bound_id, _)| *bound_id);
            let stop_single = self.proxy_listeners_bound.len() > 1
                || (index != 0
                    && !item
                        .bind_address
                        .parse::<SocketAddr>()
                        .is_ok_and(|address| self.proxy_address == Some(address)));
            if stop_single {
                if !self.workspace.proxy_listeners.is_empty()
                    && self.workspace.proxy_listeners.get(index).is_some_and(|l| l.enabled)
                {
                    let mut next = listeners;
                    next[index].enabled = false;
                    self.apply_proxy_listeners_change(Some(project_key), next, window, cx)?;
                } else {
                    self.stop_proxy_listener(bound_id.unwrap_or(item.id), cx);
                }
                return Ok(());
            }
            if let Some(cancel) = &self.proxy_cancel {
                cancel.cancel();
            }
            self.proxy_busy = true;
            self.status = "正在停止 Proxy；等待后台确认。".into();
            let _ = self.enqueue(Command::StopProxy { project: project_key }, cx);
            cx.notify();
            return Ok(());
        }
        if self.proxy_address.is_none() && self.proxy_listeners_bound.is_empty() && enabled {
            let listen = parse_listener_address(&item.bind_address)?;
            let needs_enable_adjustment = !self.workspace.proxy_listeners.is_empty()
                && (!item.enabled
                    || listeners
                        .iter()
                        .enumerate()
                        .any(|(other_idx, entry)| other_idx != index && entry.enabled));
            if needs_enable_adjustment {
                let mut next = listeners;
                for (entry_idx, entry) in next.iter_mut().enumerate() {
                    entry.enabled = entry_idx == index;
                }
                self.apply_proxy_listeners_change(Some(project_key), next, window, cx)?;
            }
            if self.enqueue(
                Command::StartProxy {
                    project: project_key,
                    listen,
                },
                cx,
            ) {
                self.proxy_busy = true;
            }
            cx.notify();
            return Ok(());
        }
        let mut next = listeners;
        next[index].enabled = enabled;
        self.apply_proxy_listeners_change(Some(project_key), next, window, cx)
    }

    pub(in crate::workbench) fn remove_proxy_listener(
        &mut self,
        project: Option<ProjectKey>,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let mut next = self.workspace.effective_proxy_listeners();
        if index >= next.len() {
            return Ok(());
        }
        next.remove(index);
        self.apply_proxy_listeners_change(project, next, window, cx)
    }

    pub(in crate::workbench) fn reset_proxy_listeners(
        &mut self,
        project: Option<ProjectKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.apply_proxy_listeners_change(project, Vec::new(), window, cx)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ListenerBindMode {
    Loopback,
    AllInterfaces,
    Specific,
}

#[derive(Clone)]
struct SpecificBindChoice {
    ip: String,
    label: String,
}

impl SelectItem for SpecificBindChoice {
    type Value = String;

    fn title(&self) -> SharedString {
        self.label.clone().into()
    }

    fn value(&self) -> &String {
        &self.ip
    }

    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .id(SharedString::from(format!(
                "proxy-listener-specific-choice-{}",
                self.ip
            )))
            .test_support()
            .role(Role::Label)
            .aria_label(self.label.clone())
            .child(self.label.clone())
    }
}

pub(super) struct ProxyListenerEditor {
    owner: WeakEntity<Workbench>,
    project: Option<ProjectKey>,
    index: Option<usize>,
    address: Entity<InputState>,
    bind_mode: ListenerBindMode,
    specific_ip: Entity<InputState>,
    specific_ip_select: Entity<SelectState<Vec<SpecificBindChoice>>>,
    enabled: bool,
    config: ProxyListenerConfig,
    tab: u8,
    redirect_host: Entity<InputState>,
    redirect_port: Entity<InputState>,
    cert_hostname: Entity<InputState>,
    custom_cert_pem: Entity<EditorState>,
    custom_key_pem: Entity<EditorState>,
    error: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
    active_file_picker: Option<mio_core::Uuid>,
}

impl ProxyListenerEditor {
    fn new(
        settings: WeakEntity<Settings>,
        owner: WeakEntity<Workbench>,
        project: Option<ProjectKey>,
        index: Option<usize>,
        item: ProxyListenerConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let dialog = window.window_handle();
        let release_owner = owner.clone();
        cx.on_release(move |editor, cx| {
            let active_picker = editor.active_file_picker.take();
            cx.defer(move |cx| {
                if let Some(picker) = active_picker {
                    let _ = release_owner.update(cx, |view, cx| {
                        view.finish_file_picker(picker, cx);
                    });
                }
                let _ = settings.update(cx, |settings, cx| {
                    if settings.listeners_tls.dialog == Some(dialog) {
                        settings.listeners_tls.dialog = None;
                    }
                    cx.notify();
                });
            });
        })
        .detach();
        if let Some(wb) = owner.upgrade() {
            cx.observe_in(&wb, window, |this, wb, window, cx| {
                if wb.read(cx).project != this.project {
                    window.remove_window();
                }
            })
            .detach();
        }
        window.set_window_title(if index.is_some() {
            "编辑 Proxy 监听器"
        } else {
            "添加 Proxy 监听器"
        });
        window.on_window_should_close(cx, |_, _| true);
        let focus = cx.focus_handle();
        focus.focus(window, cx);

        let parsed_addr = item.bind_address.trim().parse::<SocketAddr>().ok();
        let initial_port_or_addr = parsed_addr
            .map(|addr| addr.port().to_string())
            .unwrap_or_else(|| item.bind_address.clone());
        let bind_mode = match parsed_addr.map(|addr| addr.ip()) {
            Some(IpAddr::V4(v4)) if v4 == Ipv4Addr::LOCALHOST => ListenerBindMode::Loopback,
            Some(IpAddr::V4(v4)) if v4 == Ipv4Addr::UNSPECIFIED => ListenerBindMode::AllInterfaces,
            Some(_) => ListenerBindMode::Specific,
            None => ListenerBindMode::Loopback,
        };
        let initial_specific_ip = parsed_addr
            .map(|addr| addr.ip().to_string())
            .unwrap_or_else(|| "127.0.0.1".to_owned());

        let address =
            cx.new(|cx| InputState::new(window, cx).default_value(initial_port_or_addr));
        address.update(cx, |input, cx| {
            input.select_all(window, cx);
            input.focus(window, cx);
        });
        let specific_ip =
            cx.new(|cx| InputState::new(window, cx).default_value(initial_specific_ip.clone()));

        let choices: Vec<SpecificBindChoice> = mio_core::list_local_bind_addresses()
            .into_iter()
            .map(|entry| {
                let ip = entry.ip.to_string();
                let label = match entry.interface {
                    Some(iface) => format!("{ip} ({iface})"),
                    None => ip.clone(),
                };
                SpecificBindChoice { ip, label }
            })
            .collect();
        let selected_choice = choices
            .iter()
            .position(|c| c.ip == initial_specific_ip)
            .map(IndexPath::new);
        let specific_ip_select = cx.new(|cx| SelectState::new(choices, selected_choice, window, cx));
        let select_sub = cx.subscribe_in(&specific_ip_select, window, |this, _, event, window, cx| {
            if let SelectEvent::Confirm(Some(ip)) = event {
                let ip = ip.clone();
                this.bind_mode = ListenerBindMode::Specific;
                this.specific_ip
                    .update(cx, |input, cx| input.set_value(ip, window, cx));
                this.error = None;
                cx.notify();
            }
        });
        let addr_sub = cx.subscribe_in(&address, window, |_, _, event, _, cx| {
            if matches!(event, gpui_kit::component::input::InputEvent::Change) {
                cx.notify();
            }
        });
        let spec_sub = cx.subscribe_in(&specific_ip, window, |_, _, event, _, cx| {
            if matches!(event, gpui_kit::component::input::InputEvent::Change) {
                cx.notify();
            }
        });

        let custom_cert_pem = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("plaintext")
                .line_number(false)
                .soft_wrap(true)
                .auto_close(false)
                .autocorrect(false)
        });
        if let Some(pem) = item.custom_cert_pem.clone() {
            custom_cert_pem.update(cx, |ed, cx| ed.set_value(pem, window, cx));
        }
        let custom_key_pem = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("plaintext")
                .line_number(false)
                .soft_wrap(true)
                .auto_close(false)
                .autocorrect(false)
        });
        if let Some(pem) = item.custom_key_pem.clone() {
            custom_key_pem.update(cx, |ed, cx| ed.set_value(pem, window, cx));
        }
        Self {
            owner,
            project,
            index,
            address,
            bind_mode,
            specific_ip,
            specific_ip_select,
            enabled: item.enabled,
            redirect_host: cx.new(|cx| InputState::new(window, cx).default_value(item.redirect_to_host.clone())),
            redirect_port: cx.new(|cx| InputState::new(window, cx).default_value(item.redirect_to_port.map(|port| port.to_string()).unwrap_or_default())),
            cert_hostname: cx.new(|cx| InputState::new(window, cx).default_value(item.specific_cert_hostname.clone())),
            custom_cert_pem,
            custom_key_pem,
            config: item,
            tab: 0,
            error: None,
            focus,
            _subscriptions: vec![select_sub, addr_sub, spec_sub],
            active_file_picker: None,
        }
    }

    fn pick_pem_file(&mut self, is_key: bool, window: &mut Window, cx: &mut Context<Self>) {
        let picker = mio_core::Uuid::new_v4();
        let admitted = self
            .owner
            .update(cx, |view, cx| {
                if view.file_picker_pending()
                    || !view.backend_available()
                    || view.transition.is_some()
                {
                    return false;
                }
                view.file_picker = Some(picker);
                cx.notify();
                true
            })
            .unwrap_or(false);
        if !admitted {
            return;
        }
        self.active_file_picker = Some(picker);
        let label = if is_key {
            "监听器自定义私钥 PEM 文件"
        } else {
            "监听器自定义证书 PEM 文件"
        };
        let owner = self.owner.clone();
        let paths = crate::native_terminate::prompt_for_paths(
            window,
            cx,
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some(format!("选择{label}").into()),
            },
        );
        cx.spawn_in(window, async move |this, cx| {
            let result = paths.await;
            let released = this
                .update_in(cx, |this, _, cx| {
                    if this.active_file_picker == Some(picker) {
                        this.active_file_picker = None;
                    }
                    this.owner
                        .update(cx, |view, cx| view.finish_file_picker(picker, cx))
                        .unwrap_or(false)
                })
                .unwrap_or_else(|_| {
                    owner
                        .update(cx, |view, cx| view.finish_file_picker(picker, cx))
                        .unwrap_or(false)
                });
            if !released {
                return;
            }
            if let Ok(Ok(Some(selected))) = result
                && let Some(path) = selected.into_iter().next()
            {
                let loaded = cx
                    .background_executor()
                    .spawn(async move { read_bounded_pem_text(&path, label) })
                    .await;
                let _ = this.update_in(cx, |this, window, cx| {
                    if this.config.certificate_mode != ProxyListenerCertMode::CustomCertificate {
                        return;
                    }
                    match loaded {
                        Ok(text) => {
                            let target = if is_key {
                                &this.custom_key_pem
                            } else {
                                &this.custom_cert_pem
                            };
                            target.update(cx, |ed, cx| ed.set_value(text, window, cx));
                            this.error = None;
                        }
                        Err(err) => this.error = Some(err),
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let config = match self.read_config(window, cx) {
            Ok(config) => config,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let project = self.project;
        let index = self.index;
        let outcome = self.owner.update(cx, |view, cx| {
            if let Some(index) = index {
                view.edit_proxy_listener_config(project, index, config, window, cx)
            } else {
                view.add_proxy_listener_config(project, config, window, cx)
            }
        });
        match outcome {
            Ok(Ok(())) => {
                window.remove_window();
            }
            Ok(Err(error)) => {
                self.error = Some(error);
                cx.notify();
            }
            Err(_) => {
                self.error = Some("主工作台已关闭；监听器未保存。".into());
                cx.notify();
            }
        }
    }

    fn read_config(&self, window: &mut Window, cx: &mut Context<Self>) -> Result<ProxyListenerConfig, String> {
        let mut config = self.config.clone();
        let raw_addr = editing::input_snapshot(&self.address, window, cx)
            .map_err(str::to_owned)?
            .trim()
            .to_owned();
        let bind_socket = if let Ok(full_addr) = raw_addr.parse::<SocketAddr>() {
            full_addr
        } else {
            let port = raw_addr.parse::<u16>().map_err(|_| {
                "监听端口必须是 0 到 65535 之间的整数，或输入完整 IP:端口（如 0.0.0.0:8080）。".to_owned()
            })?;
            match self.bind_mode {
                ListenerBindMode::Loopback => SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
                ListenerBindMode::AllInterfaces => SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
                ListenerBindMode::Specific => {
                    let ip_raw = editing::input_snapshot(&self.specific_ip, window, cx)
                        .map_err(str::to_owned)?;
                    let clean_ip = ip_raw.trim().trim_matches(['[', ']']);
                    let ip: IpAddr = clean_ip.parse().map_err(|_| {
                        "指定绑定地址必须是有效的 IPv4 或 IPv6 地址（例如 127.0.0.1 或 ::1）。".to_owned()
                    })?;
                    SocketAddr::new(ip, port)
                }
            }
        };
        config.bind_address = bind_socket.to_string();
        config.enabled = self.enabled;
        config.redirect_to_host = editing::input_snapshot(&self.redirect_host, window, cx).map_err(str::to_owned)?.trim().to_owned();
        let port = editing::input_snapshot(&self.redirect_port, window, cx).map_err(str::to_owned)?;
        config.redirect_to_port = if port.trim().is_empty() {
            None
        } else {
            Some(port.trim().parse::<u16>().ok().filter(|port| *port > 0).ok_or_else(|| "重定向端口必须在 1 到 65535 之间。".to_owned())?)
        };
        config.specific_cert_hostname = editing::input_snapshot(&self.cert_hostname, window, cx).map_err(str::to_owned)?.trim().to_owned();
        let cert_pem_raw = editing::snapshot(&self.custom_cert_pem, window, cx).map_err(str::to_owned)?;
        let key_pem_raw = editing::snapshot(&self.custom_key_pem, window, cx).map_err(str::to_owned)?;
        let cert_pem = cert_pem_raw.trim().to_owned();
        let key_pem = key_pem_raw.trim().to_owned();
        if cert_pem.len() > MAX_CLIENT_CERT_BYTES || cert_pem.contains('\0') {
            return Err("自定义证书 PEM 超过大小上限或包含非法 NUL 字节。".into());
        }
        if key_pem.len() > MAX_CLIENT_CERT_BYTES || key_pem.contains('\0') {
            return Err("自定义私钥 PEM 超过大小上限或包含非法 NUL 字节。".into());
        }
        config.custom_cert_pem = (!cert_pem.is_empty()).then_some(cert_pem);
        config.custom_key_pem = (!key_pem.is_empty()).then_some(key_pem);
        if !config.redirect_to_host.is_empty() {
            config.redirect_to_host = mio_core::validate_redirect_host(&config.redirect_to_host)
                .map_err(|error| error.to_string())?;
        }
        if config.certificate_mode == ProxyListenerCertMode::SpecificHostname {
            mio_core::validate_listener_specific_hostname(&config.specific_cert_hostname)
                .map_err(|error| error.to_string())?;
        }
        if config.certificate_mode == ProxyListenerCertMode::CustomCertificate {
            let Some(cert) = config.custom_cert_pem.as_deref() else {
                return Err("使用自定义证书模式时，请提供证书 PEM 内容或选择 PEM 文件。".into());
            };
            mio_core::validate_listener_custom_pem(
                cert,
                config.custom_key_pem.as_deref().unwrap_or_default(),
            )
            .map_err(|error| error.to_string())?;
        }
        Ok(config)
    }

    fn effective_is_non_loopback(&self, cx: &App) -> bool {
        let raw = self.address.read(cx).value();
        if let Ok(full_addr) = raw.trim().parse::<SocketAddr>() {
            return !full_addr.ip().is_loopback();
        }
        match self.bind_mode {
            ListenerBindMode::Loopback => false,
            ListenerBindMode::AllInterfaces => true,
            ListenerBindMode::Specific => self
                .specific_ip
                .read(cx)
                .value()
                .trim()
                .trim_matches(['[', ']'])
                .parse::<IpAddr>()
                .is_ok_and(|ip| !ip.is_loopback()),
        }
    }

    fn choose_bind_mode(
        &mut self,
        mode: ListenerBindMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Ok(address) = editing::input_snapshot(&self.address, window, cx)
            && let Ok(full_addr) = address.trim().parse::<SocketAddr>()
        {
            self.address
                .update(cx, |input, cx| input.set_value(full_addr.port().to_string(), window, cx));
        }
        self.bind_mode = mode;
        self.error = None;
        cx.notify();
    }

    fn render_tab_content(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div().flex().flex_col().gap(px(10.));
        let e1 = cx.entity();
        let e2 = cx.entity();
        let e3 = cx.entity();
        let e4 = cx.entity();
        match self.tab {
            0 => {
                let non_loopback = self.effective_is_non_loopback(cx);
                let specific = self.bind_mode == ListenerBindMode::Specific;
                let r_loop = cx.entity();
                let r_all = cx.entity();
                let r_spec = cx.entity();
                body = body
                    .child(listener_input_row("绑定端口：", "proxy-listener-editor-address", &self.address))
                    .child(
                        div()
                            .text_size(theme::scaled_px(12.))
                            .text_color(theme::muted_foreground())
                            .child("输入监听端口（0 自动分配空闲端口），或直接输入完整 IP:端口。"),
                    )
                    .child("绑定地址：")
                    .child(
                        settings_radio(
                            "proxy-listener-bind-loopback",
                            "仅回环地址 (127.0.0.1)",
                            self.bind_mode == ListenerBindMode::Loopback,
                            true,
                            window,
                            cx,
                        )
                        .on_change(move |checked, _, window, cx| {
                            if checked {
                                r_loop.update(cx, |this, cx| {
                                    this.choose_bind_mode(ListenerBindMode::Loopback, window, cx)
                                });
                            }
                        }),
                    )
                    .child(
                        settings_radio(
                            "proxy-listener-bind-all",
                            "所有网络接口 (0.0.0.0)",
                            self.bind_mode == ListenerBindMode::AllInterfaces,
                            true,
                            window,
                            cx,
                        )
                        .on_change(move |checked, _, window, cx| {
                            if checked {
                                r_all.update(cx, |this, cx| {
                                    this.choose_bind_mode(ListenerBindMode::AllInterfaces, window, cx)
                                });
                            }
                        }),
                    )
                    .child(
                        settings_radio(
                            "proxy-listener-bind-specific",
                            "指定地址：",
                            specific,
                            true,
                            window,
                            cx,
                        )
                        .on_change(move |checked, _, window, cx| {
                            if checked {
                                r_spec.update(cx, |this, cx| {
                                    this.choose_bind_mode(ListenerBindMode::Specific, window, cx)
                                });
                            }
                        }),
                    )
                    .child(
                        div()
                            .pl(px(24.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .w(theme::scaled_px(240.).max(px(210.)))
                                    .flex_shrink_0()
                                    .child(
                                        Select::new(&self.specific_ip_select)
                                            .id("proxy-listener-specific-select")
                                            .small()
                                            .w_full()
                                            .disabled(!specific),
                                    ),
                            )
                            .child(
                                div().flex_1().min_w_0().child(
                                    Input::new(&self.specific_ip)
                                        .id("proxy-listener-specific-address")
                                        .aria_label("指定绑定 IP 地址")
                                        .small()
                                        .disabled(!specific),
                                ),
                            ),
                    )
                    .when(non_loopback, |this| {
                        this.child(
                            div()
                                .id("proxy-listener-non-loopback-warning")
                                .test_support()
                                .role(Role::Status)
                                .p(px(8.))
                                .rounded(px(4.))
                                .border_1()
                                .border_color(rgb(if theme::is_dark() { 0x6b5320 } else { 0xe5c07b }))
                                .bg(rgb(if theme::is_dark() { 0x3b2f13 } else { 0xfff8e1 }))
                                .text_color(rgb(if theme::is_dark() { 0xf5d68a } else { 0x6b4f00 }))
                                .text_size(theme::scaled_px(12.))
                                .child("注意：绑定非回环地址（如 0.0.0.0 或局域网 IP）后，同一网络内的其他设备可连接到此代理监听器。"),
                        )
                    })
                    .child(indexed_checkbox("proxy-listener-editor-enabled", "启用监听器", self.enabled, true)
                        .on_change(move |state, _, _, cx| {
                            e1.update(cx, |this, cx| {
                                this.enabled = state == CheckboxState::Checked;
                                cx.notify();
                            });
                        }));
            }
            1 => {
                body = body
                    .child(listener_input_row("重定向到主机：", "proxy-listener-redirect-host", &self.redirect_host))
                    .child(listener_input_row("重定向到端口：", "proxy-listener-redirect-port", &self.redirect_port))
                    .child(indexed_checkbox("proxy-listener-force-tls", "强制使用 TLS", self.config.force_use_of_tls, true)
                        .on_change(move |state, _, _, cx| {
                            e2.update(cx, |this, cx| {
                                this.config.force_use_of_tls = state == CheckboxState::Checked;
                                cx.notify();
                            });
                        }))
                    .child(indexed_checkbox("proxy-listener-invisible", "支持隐形代理（仅在需要时启用）", self.config.invisible_proxying, true)
                        .on_change(move |state, _, _, cx| {
                            e3.update(cx, |this, cx| {
                                this.config.invisible_proxying = state == CheckboxState::Checked;
                                cx.notify();
                            });
                        }));
            }
            2 => {
                for (id, label, mode) in [
                    ("proxy-listener-cert-per-host", "为每个主机生成由 CA 签发的证书", ProxyListenerCertMode::PerHostCaSigned),
                    ("proxy-listener-cert-self-signed", "使用自签名证书", ProxyListenerCertMode::SelfSigned),
                    ("proxy-listener-cert-fixed-host", "使用指定主机名生成由 CA 签发的证书", ProxyListenerCertMode::SpecificHostname),
                    ("proxy-listener-cert-custom", "使用自定义证书", ProxyListenerCertMode::CustomCertificate),
                ] {
                    let cert_ent = cx.entity();
                    body = body.child(settings_radio(id, label, self.config.certificate_mode == mode, true, window, cx)
                        .on_change(move |checked, _, _, cx| {
                            if checked {
                                cert_ent.update(cx, |this, cx| {
                                    this.config.certificate_mode = mode;
                                    cx.notify();
                                });
                            }
                        }));
                }
                let specific_host_enabled =
                    self.config.certificate_mode == ProxyListenerCertMode::SpecificHostname;
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(div().w(theme::scaled_px(140.)).flex_shrink_0().child("主机名："))
                        .child(
                            div().flex_1().min_w_0().child(
                                Input::new(&self.cert_hostname)
                                    .id("proxy-listener-cert-hostname")
                                    .aria_label("主机名")
                                    .small()
                                    .disabled(!specific_host_enabled),
                            ),
                        ),
                );
                if self.config.certificate_mode == ProxyListenerCertMode::CustomCertificate {
                    body = body
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap(px(8.))
                                        .child("证书 PEM（支持合并 PEM）：")
                                        .child(
                                            settings_action_button(
                                                "proxy-listener-custom-cert-browse",
                                                "选择文件...",
                                                false,
                                            )
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.pick_pem_file(false, window, cx)
                                            })),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("proxy-listener-custom-cert-pem")
                                        .test_support()
                                        .h(px(64.))
                                        .border_1()
                                        .border_color(theme::border())
                                        .rounded(px(3.))
                                        .overflow_hidden()
                                        .child(
                                            Editor::new(&self.custom_cert_pem)
                                                .size_full()
                                                .bordered(false)
                                                .aria_label("监听器自定义证书 PEM"),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap(px(8.))
                                        .child("私钥 PEM（合并 PEM 时可留空）：")
                                        .child(
                                            settings_action_button(
                                                "proxy-listener-custom-key-browse",
                                                "选择文件...",
                                                false,
                                            )
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.pick_pem_file(true, window, cx)
                                            })),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("proxy-listener-custom-key-pem")
                                        .test_support()
                                        .h(px(56.))
                                        .border_1()
                                        .border_color(theme::border())
                                        .rounded(px(3.))
                                        .overflow_hidden()
                                        .child(
                                            Editor::new(&self.custom_key_pem)
                                                .size_full()
                                                .bordered(false)
                                                .aria_label("监听器自定义私钥 PEM"),
                                        ),
                                ),
                        );
                }
            }
            _ => {
                body = body.child("TLS 协议：");
                for (id, label, value) in [
                    ("proxy-listener-tls-default", "默认 (TLSv1.2, TLSv1.3)", ""),
                    ("proxy-listener-tls13", "仅 TLSv1.3", "TLSv1.3 only"),
                    ("proxy-listener-tls12", "仅 TLSv1.2", "TLSv1.2 only"),
                ] {
                    let tls_ent = cx.entity();
                    body = body.child(settings_radio(id, label, self.config.tls_protocols_label == value, true, window, cx)
                        .on_change(move |checked, _, _, cx| {
                            if checked {
                                tls_ent.update(cx, |this, cx| {
                                    this.config.tls_protocols_label = value.into();
                                    cx.notify();
                                });
                            }
                        }));
                }
                body = body.child(indexed_checkbox("proxy-listener-http2", "支持 HTTP/2", self.config.enable_http2, true)
                    .on_change(move |state, _, _, cx| {
                        e4.update(cx, |this, cx| {
                            this.config.enable_http2 = state == CheckboxState::Checked;
                            cx.notify();
                        });
                    }));
            }
        }
        body.into_any_element()
    }
}

fn listener_input_row(label: &'static str, id: &'static str, input: &Entity<InputState>) -> gpui_kit::Div {
    div().flex().items_center().gap(px(10.))
        .child(div().w(theme::scaled_px(140.)).flex_shrink_0().child(label))
        .child(div().flex_1().min_w_0().child(Input::new(input).id(id).aria_label(label).small()))
}

impl Render for ProxyListenerEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title_bar = crate::chrome::render_title_bar(
            "mio-proxy-listener-editor",
            if self.index.is_some() {
                "编辑 Proxy 监听器"
            } else {
                "添加 Proxy 监听器"
            },
            crate::chrome::TitleBarControls::DIALOG,
            |window, _| window.remove_window(),
            window,
            cx,
        );
        let mut tabs = div().flex().gap(px(6.)).pb(px(8.)).border_b_1().border_color(theme::border());
        for (tab, id, label) in [
            (0, "proxy-listener-tab-binding", "绑定"),
            (1, "proxy-listener-tab-request-handling", "请求处理"),
            (2, "proxy-listener-tab-certificate", "证书"),
            (3, "proxy-listener-tab-tls", "TLS"),
        ] {
            tabs = tabs.child(button(id, label).selected(self.tab == tab)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = tab;
                    cx.notify();
                })));
        }
        let tab_content = self.render_tab_content(window, cx);
        let content = div()
            .id("proxy-listener-editor")
            .test_support()
            .key_context("MioSettings")
            .track_focus(&self.focus)
            .size_full()
            .px(px(14.))
            .py(px(12.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .text_size(theme::ui_font_size())
            .bg(theme::surface())
            .on_action(cx.listener(|_, _: &CloseSettings, window, _| window.remove_window()))
            .child(tabs)
            .child(tab_content)
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("proxy-listener-editor-error")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .text_color(theme::danger_fg())
                        .child(error),
                )
            })
            .child(div().flex_1().min_h_0())
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        button("proxy-listener-editor-ok", "确定")
                            .icon(IconName::Check)
                            .primary()
                            .min_w(theme::scaled_px(82.).max(px(82.)))
                            .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
                    )
                    .child(
                        button("proxy-listener-editor-cancel", "取消")
                            .icon(IconName::Close)
                            .min_w(theme::scaled_px(82.).max(px(82.)))
                            .on_click(|_, window, _| window.remove_window()),
                    ),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::background())
            .text_color(theme::foreground())
            .child(title_bar)
            .child(div().flex_1().min_h_0().child(content))
    }
}

impl Settings {
    pub(in crate::workbench::settings) fn proxy_listeners_available(&self) -> bool {
        self.state.enabled
            && !self.state.proxy_busy
            && !self.listeners_tls.dialog_opening
            && self.listeners_tls.dialog.is_none()
    }

    pub(in crate::workbench) fn open_proxy_listener_editor(
        &mut self,
        new: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.listeners_tls.dialog {
            if crate::chrome::activate_popup_window(dialog, cx) {
                return;
            }
            self.listeners_tls.dialog = None;
        }
        if !self.proxy_listeners_available() {
            return;
        }
        let index = if new {
            None
        } else {
            let Some(index) = self.listeners_tls.selected_index else {
                return;
            };
            Some(index)
        };
        let item = if let Some(index) = index {
            let Some(item) = self.state.proxy_listeners.get(index).cloned() else {
                return;
            };
            item
        } else {
            let port = next_available_listener_port(&self.state.proxy_listeners);
            ProxyListenerConfig {
                id: Uuid::new_v4(),
                enabled: true,
                bind_address: format!("127.0.0.1:{port}"),
                ..ProxyListenerConfig::default()
            }
        };
        let Some(display) = window.display(cx) else {
            self.listeners_tls.listener_error =
                Some("无法确定设置窗口所在显示器；未打开监听器编辑窗口。".into());
            cx.notify();
            return;
        };
        let visible = display.visible_bounds();
        let content_w = theme::scaled_px(640.).max(px(640.));
        let content_h = theme::scaled_px(520.).max(px(520.));
        let raw_h = if cfg!(test) {
            content_h
        } else {
            content_h
                + theme::scaled_px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
                    .max(px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT))
        };
        let dimensions = size(
            content_w.min(visible.size.width),
            raw_h.min(visible.size.height),
        );
        let mut bounds =
            Bounds::centered_at(window.window_bounds().get_bounds().center(), dimensions);
        bounds.origin.x = bounds.origin.x.clamp(
            visible.origin.x,
            (visible.right() - dimensions.width).max(visible.origin.x),
        );
        bounds.origin.y = bounds.origin.y.clamp(
            visible.origin.y,
            (visible.bottom() - dimensions.height).max(visible.origin.y),
        );
        let options = WindowOptions {
            display_id: Some(display.id()),
            ..crate::chrome::mio_window_options(bounds, false, false)
        };
        let settings = cx.weak_entity();
        let owner = self.owner.clone();
        let project = self.state.project;
        self.listeners_tls.dialog_opening = true;
        cx.defer(move |cx| {
            let Some(entity) = settings.upgrade() else {
                return;
            };
            if entity.read(cx).state.project != project || owner.upgrade().is_none() {
                entity.update(cx, |this, cx| {
                    this.listeners_tls.dialog_opening = false;
                    cx.notify();
                });
                return;
            }
            let opened = crate::chrome::open_popup_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    ProxyListenerEditor::new(
                        settings.clone(),
                        owner.clone(),
                        project,
                        index,
                        item,
                        window,
                        cx,
                    )
                })
            });
            entity.update(cx, |this, cx| {
                this.listeners_tls.dialog_opening = false;
                match opened {
                    Ok((window, _)) => this.listeners_tls.dialog = Some(window),
                    Err(error) => {
                        this.listeners_tls.listener_error =
                            Some(format!("无法打开监听器编辑窗口：{error}"))
                    }
                }
                cx.notify();
            });
        });
        cx.notify();
    }

    fn import_ca_from_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.proxy_listeners_available() {
            return;
        }
        let Some(cert_picker) = self.begin_workbench_file_picker(cx) else {
            return;
        };
        let project = self.state.project;
        let owner = self.owner.clone();
        let cert_paths = crate::native_terminate::prompt_for_paths(
            window,
            cx,
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("选择 CA 证书 PEM 文件（支持合并 PEM）".into()),
            },
        );
        cx.spawn_in(window, async move |this, cx| {
            let cert_pick = cert_paths.await;
            let cert_admitted = this
                .update_in(cx, |this, _, cx| {
                    this.finish_workbench_file_picker(cert_picker, cx)
                })
                .unwrap_or_else(|_| {
                    owner
                        .update(cx, |view, cx| view.finish_file_picker(cert_picker, cx))
                        .unwrap_or(false)
                });
            if !cert_admitted {
                return;
            }
            let cert_path = match cert_pick {
                Ok(Ok(Some(paths))) => {
                    let mut iter = paths.into_iter();
                    match (iter.next(), iter.next()) {
                        (Some(path), None) => path,
                        _ => {
                            let _ = this.update_in(cx, |this, _, cx| {
                                if this.state.project != project || !this.proxy_listeners_available() {
                                    return;
                                }
                                this.listeners_tls.listener_error =
                                    Some("请选择单个 CA 证书 PEM 文件。".into());
                                cx.notify();
                            });
                            return;
                        }
                    }
                }
                Ok(Ok(None)) => return,
                Ok(Err(err)) => {
                    let _ = this.update_in(cx, |this, _, cx| {
                        if this.state.project != project || !this.proxy_listeners_available() {
                            return;
                        }
                        this.listeners_tls.listener_error = Some(format!("无法选择 CA 证书文件：{err}"));
                        cx.notify();
                    });
                    return;
                }
                Err(_) => return,
            };
            let cert_res = cx
                .background_executor()
                .spawn(async move { read_bounded_pem_text(&cert_path, "CA 证书 PEM 文件") })
                .await;
            let cert_pem = match cert_res {
                Ok(text) => text,
                Err(err) => {
                    let _ = this.update_in(cx, |this, _, cx| {
                        if this.state.project != project || !this.proxy_listeners_available() {
                            return;
                        }
                        this.listeners_tls.listener_error = Some(err);
                        cx.notify();
                    });
                    return;
                }
            };
            if cert_pem.contains("PRIVATE KEY-----") {
                let _ = this.update_in(cx, |this, _, cx| {
                    if this.state.project != project || !this.proxy_listeners_available() {
                        return;
                    }
                    this.listeners_tls.listener_error = None;
                    let _ = this
                        .owner
                        .update(cx, |view, cx| view.import_ca(cert_pem, String::new(), cx));
                    cx.notify();
                });
                return;
            }
            let key_prompt = this
                .update_in(cx, |this, window, cx| {
                    if this.state.project != project || !this.proxy_listeners_available() {
                        return None;
                    }
                    let key_picker = this.begin_workbench_file_picker(cx)?;
                    let prompt = crate::native_terminate::prompt_for_paths(
                        window,
                        cx,
                        PathPromptOptions {
                            files: true,
                            directories: false,
                            multiple: false,
                            prompt: Some("选择 CA 私钥 PEM 文件".into()),
                        },
                    );
                    Some((key_picker, prompt))
                })
                .ok()
                .flatten();
            let Some((key_picker, key_paths)) = key_prompt else {
                return;
            };
            let key_pick = key_paths.await;
            let key_admitted = this
                .update_in(cx, |this, _, cx| {
                    this.finish_workbench_file_picker(key_picker, cx)
                })
                .unwrap_or_else(|_| {
                    owner
                        .update(cx, |view, cx| view.finish_file_picker(key_picker, cx))
                        .unwrap_or(false)
                });
            if !key_admitted {
                return;
            }
            let key_path = match key_pick {
                Ok(Ok(Some(paths))) => {
                    let mut iter = paths.into_iter();
                    match (iter.next(), iter.next()) {
                        (Some(path), None) => path,
                        _ => {
                            let _ = this.update_in(cx, |this, _, cx| {
                                if this.state.project != project || !this.proxy_listeners_available() {
                                    return;
                                }
                                this.listeners_tls.listener_error =
                                    Some("请选择单个 CA 私钥 PEM 文件。".into());
                                cx.notify();
                            });
                            return;
                        }
                    }
                }
                Ok(Ok(None)) => return,
                Ok(Err(err)) => {
                    let _ = this.update_in(cx, |this, _, cx| {
                        if this.state.project != project || !this.proxy_listeners_available() {
                            return;
                        }
                        this.listeners_tls.listener_error = Some(format!("无法选择 CA 私钥文件：{err}"));
                        cx.notify();
                    });
                    return;
                }
                Err(_) => return,
            };
            let key_res = cx
                .background_executor()
                .spawn(async move { read_bounded_pem_text(&key_path, "CA 私钥 PEM 文件") })
                .await;
            let _ = this.update_in(cx, |this, _, cx| {
                if this.state.project != project || !this.proxy_listeners_available() {
                    return;
                }
                match key_res {
                    Ok(key_pem) => {
                        this.listeners_tls.listener_error = None;
                        let _ = this
                            .owner
                            .update(cx, |view, cx| view.import_ca(cert_pem, key_pem, cx));
                    }
                    Err(err) => {
                        this.listeners_tls.listener_error = Some(err);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn confirm_use_builtin_ca_preset(
        &mut self,
        preset: mio_core::CaPreset,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.proxy_listeners_available() {
            return;
        }
        let project = self.state.project;
        let title = format!("切换内置 CA 预置：{}", preset.display_name);
        let detail = format!(
            "{}\n\n切换后会覆盖当前持久化的 Proxy CA 证书与私钥；如需恢复本地自签 MIO Root CA，可随时点击“重新生成 CA 证书”。",
            preset.description
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            Some(detail.as_str()),
            &["取消", "确认切换"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() == Some(1) {
                let _ = this.update_in(cx, |this, _, cx| {
                    if this.state.project != project || !this.proxy_listeners_available() {
                        return;
                    }
                    this.listeners_tls.listener_error = None;
                    let _ = this
                        .owner
                        .update(cx, |wb, cx| wb.use_builtin_ca_preset(preset, cx));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(in crate::workbench::settings) fn render_proxy_listeners_section(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Div {
        let enabled = self.proxy_listeners_available();
        let selected_exists = self
            .listeners_tls
            .selected_index
            .is_some_and(|index| index < self.state.proxy_listeners.len());
        let project = self.state.project;
        let owner_window = self.owner_window;
        let export_owner = self.owner.clone();

        let mut actions = div()
            .w(theme::scaled_px(84.).max(px(84.)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(5.));
        for (id, label, tip, available, action) in [
            (
                "proxy-listener-add",
                "添加",
                "添加新的 Proxy 监听器",
                enabled,
                0u8,
            ),
            (
                "proxy-listener-edit",
                "编辑",
                "修改所选监听器的绑定地址、端口与启用状态",
                enabled && selected_exists,
                1u8,
            ),
            (
                "proxy-listener-remove",
                "移除",
                "移除所选 Proxy 监听器",
                enabled && selected_exists,
                2u8,
            ),
        ] {
            actions = actions.child(
                settings_action_button(id, label, !available)
                    .w_full()
                    .tooltip(tip)
                    .tab_stop(false)
                    .on_click(cx.listener(move |this, _, window, cx| match action {
                        0 => this.open_proxy_listener_editor(true, window, cx),
                        1 => this.open_proxy_listener_editor(false, window, cx),
                        _ => {
                            if !this.proxy_listeners_available() {
                                return;
                            }
                            let Some(index) = this.listeners_tls.selected_index else {
                                return;
                            };
                            let res = this.owner.update(cx, |view, cx| {
                                view.remove_proxy_listener(project, index, window, cx)
                            });
                            this.listeners_tls.listener_error = match res {
                                Ok(Ok(())) => None,
                                Ok(Err(error)) => Some(error),
                                Err(_) => Some("主工作台已关闭；监听器未移除。".into()),
                            };
                            cx.notify();
                        }
                    })),
            );
        }

        let listener_gear = settings_gear_icon_button(
            "settings-proxy-listener-reset",
            "恢复默认单监听器 127.0.0.1:8080",
            !enabled,
        )
        .tab_stop(false)
        .on_click(cx.listener(|this, _, window, cx| {
            if !this.proxy_listeners_available() {
                return;
            }
            let project = this.state.project;
            let result = this
                .owner
                .update(cx, |view, cx| view.reset_proxy_listeners(project, window, cx));
            this.listeners_tls.listener_error = match result {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error),
                Err(_) => Some("主工作台已关闭；默认监听器未恢复。".into()),
            };
            this.listeners_tls.selected_index = (!this.state.proxy_listeners.is_empty()).then_some(0);
            cx.notify();
        }));

        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(px(9.))
            .pb(px(16.))
            .border_b_1()
            .border_color(theme::border())
            .child(settings_section_heading(
                "proxy-listeners-heading",
                "Proxy 监听器",
                project_scope_badge(),
            ))
            .child(settings_desc_row_with_action(
                listener_gear,
                "MIO Proxy 使用监听器接收来自浏览器的 HTTP/HTTPS 与 WebSocket 请求。请将浏览器代理配置为下方任一监听地址。",
            ))
            .child(
                div()
                    .pl(px(31.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .gap(px(9.))
                            .h(theme::scaled_px(136.).max(px(136.)))
                            .child(actions)
                            .child(settings_table_frame(
                                DataTable::new(&self.listeners_tls.table)
                                    .with_size(px(22.))
                                    .header_row_height(px(24.))
                                    .bordered(false)
                                    .stripe(false)
                                    .scrollbar_visible(true, false)
                                    .scroll_bounce(false)
                                    .accessibility_label("Proxy 监听器"),
                                false,
                            )),
                    )
                    .child(
                        "访问 HTTPS 站点需要浏览器信任 MIO Root CA 证书；监听器为每个目标主机动态签发证书。内置 CA 预置面向信任库已包含该 CA、且不做吊销校验的客户端（例如未升级的旧安卓设备）。",
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .child({
                                let settings_weak = cx.weak_entity();
                                settings_action_button(
                                    "settings-proxy-export-ca",
                                    "导入 / 导出 CA 证书",
                                    !enabled,
                                )
                                .tooltip(
                                    "导出或导入用户级持久化 MIO Root CA 证书（支持 DER / PEM / 合并 PEM）",
                                )
                                .tab_stop(false)
                                .dropdown_menu(move |menu, _, _| {
                                    let exp_owner = export_owner.clone();
                                    let imp_settings = settings_weak.clone();
                                    menu.item(
                                        PopupMenuItem::new("导出 CA 证书 (DER / PEM)...")
                                            .disabled(!enabled)
                                            .on_click(move |_, _, cx| {
                                                let owner = exp_owner.clone();
                                                cx.defer(move |cx| {
                                                    let _ = owner_window.update(cx, |_, window, cx| {
                                                        let _ = owner.update(cx, |wb, cx| {
                                                            wb.export_ca(window, cx);
                                                        });
                                                    });
                                                });
                                            }),
                                    )
                                    .item(
                                        PopupMenuItem::new("导入 CA 证书 (PEM)...")
                                            .disabled(!enabled)
                                            .on_click(move |_, window, cx| {
                                                let _ = imp_settings.update(cx, |this, cx| {
                                                    this.import_ca_from_picker(window, cx)
                                                });
                                            }),
                                    )
                                })
                            })
                            .child(
                                settings_action_button(
                                    "settings-proxy-regenerate-ca",
                                    "重新生成 CA 证书",
                                    !enabled,
                                )
                                .tooltip("重新生成本地 MIO Root CA 证书（需在浏览器中重新信任）")
                                .tab_stop(false)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if !this.proxy_listeners_available() {
                                        return;
                                    }
                                    let project = this.state.project;
                                    let answer = window.prompt(
                                        PromptLevel::Warning,
                                        "重新生成 CA 证书",
                                        Some("重新生成 MIO Root CA 证书后，需要在浏览器中重新导入并信任新的 CA 证书。"),
                                        &["取消", "确定"],
                                        cx,
                                    );
                                    cx.spawn_in(window, async move |this, cx| {
                                        if answer.await.ok() == Some(1) {
                                            let _ = this.update_in(cx, |this, _, cx| {
                                                if this.state.project != project
                                                    || !this.proxy_listeners_available()
                                                {
                                                    return;
                                                }
                                                this.listeners_tls.listener_error = None;
                                                let _ = this.owner.update(cx, |view, cx| {
                                                    view.regenerate_ca(cx)
                                                });
                                                cx.notify();
                                            });
                                        }
                                    })
                                    .detach();
                                })),
                            )
                            .child({
                                let preset_settings = cx.weak_entity();
                                let active_preset_id = self
                                    .state
                                    .last_ca_fingerprint
                                    .as_deref()
                                    .and_then(mio_core::ca_preset_by_fingerprint)
                                    .map(|preset| preset.id);
                                settings_action_button(
                                    "settings-proxy-builtin-ca",
                                    "内置 CA 预置",
                                    !enabled,
                                )
                                .tooltip(
                                    "选择内置签发 CA：适用于信任库已包含该 CA、且不做吊销校验的客户端",
                                )
                                .tab_stop(false)
                                .dropdown_menu(move |mut menu, _, _| {
                                    for preset in mio_core::BUILTIN_CA_PRESETS {
                                        let preset = *preset;
                                        let settings = preset_settings.clone();
                                        menu = menu.item(
                                            PopupMenuItem::new(preset.display_name)
                                                .checked(active_preset_id == Some(preset.id))
                                                .disabled(!enabled)
                                                .on_click(move |_, window, cx| {
                                                    let _ = settings.update(cx, |this, cx| {
                                                        this.confirm_use_builtin_ca_preset(
                                                            preset, window, cx,
                                                        )
                                                    });
                                                }),
                                        );
                                    }
                                    menu
                                })
                            }),
                    )
                    .when_some(self.state.last_ca_fingerprint.clone(), |this, fp| {
                        let text = match mio_core::ca_preset_by_fingerprint(&fp) {
                            Some(preset) => {
                                format!("CA SHA-256 指纹：{fp}（内置预置：{}）", preset.display_name)
                            }
                            None => format!("CA SHA-256 指纹：{fp}"),
                        };
                        let aria = match mio_core::ca_preset_by_fingerprint(&fp) {
                            Some(preset) => format!(
                                "当前 CA 指纹 (SHA-256)：{fp}，内置预置：{}",
                                preset.display_name
                            ),
                            None => format!("当前 CA 指纹 (SHA-256)：{fp}"),
                        };
                        this.child(
                            div()
                                .id("settings-proxy-ca-fingerprint")
                                .test_support()
                                .role(Role::Status)
                                .aria_label(aria)
                                .text_size(theme::scaled_px(12.))
                                .text_color(theme::muted_foreground())
                                .font_family(theme::mono_font_family())
                                .child(text),
                        )
                    }),
            )
            .when_some(self.listeners_tls.listener_error.clone(), |this, error| {
                this.child(
                    div()
                        .id("settings-listener-error")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .ml(px(31.))
                        .p(px(8.))
                        .rounded(px(4.))
                        .bg(theme::danger_bg())
                        .text_color(theme::danger_fg())
                        .child(error),
                )
            })
    }
}
