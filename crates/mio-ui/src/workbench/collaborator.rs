use std::sync::Arc;

use gpui_kit::accesskit::Role;
use gpui_kit::component::{
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
    resizable::{resizable_panel, v_resizable},
};
use gpui_kit::{
    AnyElement, ClipboardItem, Context, Entity, FontWeight, MouseButton, Render, RenderImage,
    Subscription, WeakEntity, Window, img,
};
use mio_core::{
    CollaboratorCommand, CollaboratorEvent, CollaboratorStatus, Command,
    MAX_COLLABORATOR_BATCH_PAYLOADS, MAX_COLLABORATOR_INTERACTIONS, OastInteraction,
    OastPayloadToken, OastProtocol, SelfHostedOastConfig, Uuid,
};

use super::*;

mod controller;
mod render_interactions;
mod render_onboarding;
mod render_toolbar;
mod render_viewer;
#[cfg(test)]
mod tests;
mod transfers;

pub(in crate::workbench) const LOCAL_LOOPBACK_NOTICE: &str =
    "默认使用本地回环 (Loopback) HTTP/DNS 监听，不发起公网连接；仅在启用自托管端点时轮询指定服务器";

pub(in crate::workbench) const SELF_HOSTED_SECURITY_NOTE: &str =
    "安全提示：启用自托管端点后将向该 URL 发起轮询请求；默认阻止公共第三方 Collaborator 云域名。";

pub(in crate::workbench) const MAX_INPUT_BYTES: usize = 2048;

pub(in crate::workbench) const MAX_LABEL_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum CopyMode {
    Http,
    Dns,
}

pub(in crate::workbench) struct CollaboratorPane {
    pub(in crate::workbench) status: CollaboratorStatus,
    pub(in crate::workbench) loaded: bool,
    pub(in crate::workbench) onboarding_dismissed: bool,
    pub(in crate::workbench) selected_interaction: Option<Uuid>,
    pub(in crate::workbench) raw_editor: MessageEditor,
    pub(in crate::workbench) latest_payload_preview: Option<String>,
    pub(in crate::workbench) notice: Option<String>,
    pub(in crate::workbench) error: Option<String>,
    pub(in crate::workbench) pending_copy: Option<CopyMode>,
    pub(super) view_inputs: Vec<Entity<InputState>>,
}

impl CollaboratorPane {
    pub(in crate::workbench) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let raw_editor = MessageEditor::new(window, cx);
        Self {
            status: CollaboratorStatus::default(),
            loaded: false,
            onboarding_dismissed: false,
            selected_interaction: None,
            raw_editor,
            latest_payload_preview: None,
            notice: None,
            error: None,
            pending_copy: None,
            view_inputs: Vec::new(),
        }
    }

    pub(in crate::workbench) fn attach_view_inputs(&mut self, inputs: Vec<Entity<InputState>>) {
        self.view_inputs = inputs;
    }

    pub(in crate::workbench) fn validate_composition(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), &'static str> {
        for input in &self.view_inputs {
            editing::input_snapshot(input, window, cx)?;
        }
        editing::input_snapshot(&self.raw_editor.search, window, cx)?;
        for editor in [&self.raw_editor.raw, &self.raw_editor.hex] {
            editing::snapshot(editor, window, cx)?;
        }
        Ok(())
    }

    pub(in crate::workbench) fn has_non_editor_focus(&self, window: &Window, cx: &App) -> bool {
        self.view_inputs
            .iter()
            .any(|input| input.focus_handle(cx).is_focused(window))
    }

    pub(in crate::workbench) fn token_label(&self, token: &str) -> Option<&str> {
        self.status
            .issued_tokens
            .iter()
            .find(|item| item.token == token)
            .map(|item| item.label.as_str())
            .filter(|label| !label.is_empty())
    }
}

pub(in crate::workbench) struct CollaboratorView {
    pub(super) workbench: WeakEntity<Workbench>,
    pub(super) payloads_drawer_open: bool,
    pub(super) radar_light: Option<Arc<RenderImage>>,
    pub(super) radar_dark: Option<Arc<RenderImage>>,
    pub(super) count_input: Entity<InputState>,
    pub(super) label_input: Entity<InputState>,
    pub(super) protocol_filter: Option<OastProtocol>,
    pub(super) search_input: Entity<InputState>,
    pub(super) config_open: bool,
    pub(super) self_hosted_enabled: bool,
    pub(super) allow_public_cloud_domains: bool,
    pub(super) bind_address_input: Entity<InputState>,
    pub(super) http_port_input: Entity<InputState>,
    pub(super) dns_port_input: Entity<InputState>,
    pub(super) dns_answer_ip_input: Entity<InputState>,
    pub(super) polling_url_input: Entity<InputState>,
    pub(super) server_domain_input: Entity<InputState>,
    pub(super) auth_header_input: Entity<InputState>,
    pub(super) poll_interval_input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl CollaboratorView {
    pub(in crate::workbench) fn new(
        workbench: WeakEntity<Workbench>,
        raw_editor: &MessageEditor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let count_placeholder = format!("1–{MAX_COLLABORATOR_BATCH_PAYLOADS}");
        let count_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value("1")
                .placeholder(count_placeholder)
        });
        let label_input = cx.new(|cx| InputState::new(window, cx).placeholder("备注标签"));
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索交互…"));
        let bind_address_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("127.0.0.1 / 0.0.0.0"));
        let http_port_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("自动 (0)"));
        let dns_port_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("自动 (0)"));
        let dns_answer_ip_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("127.0.0.1 / 公网 IP"));
        let polling_url_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://oast.internal/poll"));
        let server_domain_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("oast.internal（可选）"));
        let auth_header_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Bearer <token>（可选）"));
        let poll_interval_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value("5000")
                .placeholder("≥ 250 ms")
        });

        let mut subscriptions = Vec::new();
        for editor in [&raw_editor.raw, &raw_editor.hex] {
            let mut composing = false;
            subscriptions.push(cx.observe_in(editor, window, move |_, input, window, cx| {
                let marked = input.update(cx, |state, cx| {
                    state.marked_text_range(window, cx).is_some()
                });
                if marked != composing {
                    composing = marked;
                    cx.notify();
                }
            }));
            subscriptions.push(cx.subscribe(editor, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }
        for input in [
            &count_input,
            &label_input,
            &search_input,
            &bind_address_input,
            &http_port_input,
            &dns_port_input,
            &dns_answer_ip_input,
            &polling_url_input,
            &server_domain_input,
            &auth_header_input,
            &poll_interval_input,
            &raw_editor.search,
        ] {
            let mut composing = false;
            subscriptions.push(cx.observe_in(input, window, move |_, input, window, cx| {
                let marked = input.update(cx, |state, cx| {
                    state.marked_text_range(window, cx).is_some()
                });
                if marked != composing {
                    composing = marked;
                    cx.notify();
                }
            }));
            subscriptions.push(cx.subscribe(input, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }

        Self {
            workbench,
            payloads_drawer_open: false,
            radar_light: None,
            radar_dark: None,
            count_input,
            label_input,
            protocol_filter: None,
            search_input,
            config_open: false,
            self_hosted_enabled: false,
            allow_public_cloud_domains: false,
            bind_address_input,
            http_port_input,
            dns_port_input,
            dns_answer_ip_input,
            polling_url_input,
            server_domain_input,
            auth_header_input,
            poll_interval_input,
            _subscriptions: subscriptions,
        }
    }

    pub(in crate::workbench) fn non_editor_inputs(&self) -> Vec<Entity<InputState>> {
        vec![
            self.count_input.clone(),
            self.label_input.clone(),
            self.search_input.clone(),
            self.bind_address_input.clone(),
            self.http_port_input.clone(),
            self.dns_port_input.clone(),
            self.dns_answer_ip_input.clone(),
            self.polling_url_input.clone(),
            self.server_domain_input.clone(),
            self.auth_header_input.clone(),
            self.poll_interval_input.clone(),
        ]
    }

    pub(super) fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.payloads_drawer_open = false;
        self.protocol_filter = None;
        self.config_open = false;
        self.self_hosted_enabled = false;
        self.allow_public_cloud_domains = false;
        for (input, value) in [
            (&self.count_input, "1"),
            (&self.label_input, ""),
            (&self.search_input, ""),
            (&self.bind_address_input, ""),
            (&self.http_port_input, ""),
            (&self.dns_port_input, ""),
            (&self.dns_answer_ip_input, ""),
            (&self.polling_url_input, ""),
            (&self.server_domain_input, ""),
            (&self.auth_header_input, ""),
            (&self.poll_interval_input, "5000"),
        ] {
            input.update(cx, |state, cx| state.set_value(value, window, cx));
        }
        cx.notify();
    }

    pub(super) fn sync_config_inputs(
        &mut self,
        config: &SelfHostedOastConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let any_composing = [
            &self.bind_address_input,
            &self.http_port_input,
            &self.dns_port_input,
            &self.dns_answer_ip_input,
            &self.polling_url_input,
            &self.server_domain_input,
            &self.auth_header_input,
            &self.poll_interval_input,
        ]
        .into_iter()
        .any(|input| {
            input.update(cx, |state, cx| {
                state.marked_text_range(window, cx).is_some()
            })
        });
        if any_composing {
            return;
        }
        self.self_hosted_enabled = config.enabled;
        self.allow_public_cloud_domains = config.allow_public_cloud_domains;
        for (input, val) in [
            (
                &self.bind_address_input,
                config.bind_address.clone().unwrap_or_default(),
            ),
            (
                &self.http_port_input,
                config.http_port.map_or_else(String::new, |p| p.to_string()),
            ),
            (
                &self.dns_port_input,
                config.dns_port.map_or_else(String::new, |p| p.to_string()),
            ),
            (
                &self.dns_answer_ip_input,
                config.dns_answer_ip.clone().unwrap_or_default(),
            ),
            (
                &self.polling_url_input,
                config.polling_url.clone().unwrap_or_default(),
            ),
            (
                &self.server_domain_input,
                config.server_domain.clone().unwrap_or_default(),
            ),
            (
                &self.auth_header_input,
                config.authorization_header.clone().unwrap_or_default(),
            ),
            (
                &self.poll_interval_input,
                config.poll_interval_ms.max(250).to_string(),
            ),
        ] {
            input.update(cx, |state, cx| state.set_value(val, window, cx));
        }
        cx.notify();
    }

    pub(super) fn snapshot_config(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<SelfHostedOastConfig, String> {
        let bind_address = editing::input_snapshot(&self.bind_address_input, window, cx)
            .map_err(str::to_owned)?
            .trim()
            .to_owned();
        let http_port_text = editing::input_snapshot(&self.http_port_input, window, cx)
            .map_err(str::to_owned)?
            .trim()
            .to_owned();
        let dns_port_text = editing::input_snapshot(&self.dns_port_input, window, cx)
            .map_err(str::to_owned)?
            .trim()
            .to_owned();
        let dns_answer_ip = editing::input_snapshot(&self.dns_answer_ip_input, window, cx)
            .map_err(str::to_owned)?
            .trim()
            .to_owned();
        let parse_opt_port = |label: &str, raw: &str| -> Result<Option<u16>, String> {
            if raw.is_empty() {
                return Ok(None);
            }
            let port = raw
                .parse::<u16>()
                .map_err(|_| format!("{label}须为 0–65535 整数；输入已保留。"))?;
            Ok((port > 0).then_some(port))
        };
        let http_port = parse_opt_port("HTTP 端口", &http_port_text)?;
        let dns_port = parse_opt_port("DNS 端口", &dns_port_text)?;

        if !self.self_hosted_enabled {
            let config = SelfHostedOastConfig {
                bind_address: (!bind_address.is_empty()).then_some(bind_address),
                http_port,
                dns_port,
                dns_answer_ip: (!dns_answer_ip.is_empty()).then_some(dns_answer_ip),
                ..SelfHostedOastConfig::default()
            };
            config
                .validate()
                .map_err(|err| format!("OAST 监听配置无效：{err}"))?;
            return Ok(config);
        }
        let polling_url = editing::input_snapshot(&self.polling_url_input, window, cx)
            .map_err(str::to_owned)?
            .trim()
            .to_owned();
        let server_domain = editing::input_snapshot(&self.server_domain_input, window, cx)
            .map_err(str::to_owned)?
            .trim()
            .to_owned();
        let auth_header = editing::input_snapshot(&self.auth_header_input, window, cx)
            .map_err(str::to_owned)?
            .trim()
            .to_owned();
        let poll_interval_text = editing::input_snapshot(&self.poll_interval_input, window, cx)
            .map_err(str::to_owned)?
            .trim()
            .to_owned();
        for (name, value) in [
            ("轮询 URL", &polling_url),
            ("服务域名", &server_domain),
            ("认证请求头", &auth_header),
        ] {
            if value.len() > MAX_INPUT_BYTES {
                return Err(format!(
                    "{name}长度不能超过 {MAX_INPUT_BYTES} 字节；输入已保留。"
                ));
            }
        }
        let poll_interval_ms: u64 = if poll_interval_text.is_empty() {
            5000
        } else {
            poll_interval_text
                .parse::<u64>()
                .ok()
                .filter(|&ms| ms >= 250)
                .ok_or_else(|| "轮询间隔至少为 250 ms（整数）；输入已保留。".to_owned())?
        };
        let config = SelfHostedOastConfig {
            enabled: true,
            polling_url: (!polling_url.is_empty()).then_some(polling_url),
            server_domain: (!server_domain.is_empty()).then_some(server_domain),
            authorization_header: (!auth_header.is_empty()).then_some(auth_header),
            poll_interval_ms,
            bind_address: (!bind_address.is_empty()).then_some(bind_address),
            http_port,
            dns_port,
            dns_answer_ip: (!dns_answer_ip.is_empty()).then_some(dns_answer_ip),
            allow_public_cloud_domains: self.allow_public_cloud_domains,
        };
        config
            .validate()
            .map_err(|err| format!("自托管 OAST 配置无效：{err}"))?;
        Ok(config)
    }

    pub(super) fn set_protocol_filter(
        &mut self,
        filter: Option<OastProtocol>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let valid = self
            .workbench
            .update(cx, |wb, cx| {
                if let Err(err) = wb.collaborator_pane.validate_composition(window, cx) {
                    wb.fail_collaborator(err, cx);
                    return false;
                }
                true
            })
            .unwrap_or(false);
        if !valid {
            return;
        }
        self.protocol_filter = filter;
        cx.notify();
    }

    pub(super) fn toggle_config_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let valid = self
            .workbench
            .update(cx, |wb, cx| {
                if let Err(err) = wb.collaborator_pane.validate_composition(window, cx) {
                    wb.fail_collaborator(err, cx);
                    return false;
                }
                wb.collaborator_pane.onboarding_dismissed = true;
                true
            })
            .unwrap_or(false);
        if !valid {
            return;
        }
        self.config_open = !self.config_open;
        cx.notify();
    }

    pub(super) fn toggle_self_hosted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let valid = self
            .workbench
            .update(cx, |wb, cx| {
                if let Err(err) = wb.collaborator_pane.validate_composition(window, cx) {
                    wb.fail_collaborator(err, cx);
                    return false;
                }
                true
            })
            .unwrap_or(false);
        if !valid {
            return;
        }
        self.self_hosted_enabled = !self.self_hosted_enabled;
        cx.notify();
    }

    pub(super) fn toggle_payloads_drawer(&mut self, cx: &mut Context<Self>) {
        self.payloads_drawer_open = !self.payloads_drawer_open;
        cx.notify();
    }

    pub(super) fn start_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let config = match self.snapshot_config(window, cx) {
            Ok(config) => config,
            Err(err) => {
                let _ = self
                    .workbench
                    .update(cx, |wb, cx| wb.fail_collaborator(err, cx));
                cx.notify();
                return;
            }
        };
        let _ = self
            .workbench
            .update(cx, |wb, cx| wb.start_collaborator_server_with(config, window, cx));
        cx.notify();
    }

    pub(super) fn toggle_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let running = self
            .workbench
            .upgrade()
            .is_some_and(|wb| wb.read(cx).collaborator_pane.status.running);
        if running {
            let _ = self
                .workbench
                .update(cx, |wb, cx| wb.stop_collaborator_server(window, cx));
            cx.notify();
        } else {
            self.start_server(window, cx);
        }
    }

    pub(super) fn generate_payloads(
        &mut self,
        mode: CopyMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count_text = match editing::input_snapshot(&self.count_input, window, cx) {
            Ok(text) => text,
            Err(err) => {
                let _ = self
                    .workbench
                    .update(cx, |wb, cx| wb.fail_collaborator(err, cx));
                cx.notify();
                return;
            }
        };
        let label = match editing::input_snapshot(&self.label_input, window, cx) {
            Ok(text) => text.trim().to_owned(),
            Err(err) => {
                let _ = self
                    .workbench
                    .update(cx, |wb, cx| wb.fail_collaborator(err, cx));
                cx.notify();
                return;
            }
        };
        let _ = self.workbench.update(cx, |wb, cx| {
            wb.generate_collaborator_payloads_with(mode, &count_text, label, window, cx);
        });
        cx.notify();
    }
}

impl Render for CollaboratorView {
    #[inline(never)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(wb_entity) = self.workbench.upgrade() else {
            return div().id("collaborator-root").size_full().into_any_element();
        };
        if !wb_entity.read(cx).collaborator_pane.loaded {
            wb_entity.update(cx, |wb, cx| wb.activate_collaborator(cx));
        }
        let show_onboarding = {
            let wb = wb_entity.read(cx);
            wb.show_collaborator_onboarding(self.config_open)
        };
        if show_onboarding {
            return div()
                .id("collaborator-root")
                .test_support()
                .size_full()
                .flex()
                .flex_col()
                .bg(theme::background())
                .text_color(theme::foreground())
                .child(self.render_onboarding(window, cx))
                .into_any_element();
        }
        let query = self.search_input.read(cx).value().to_lowercase();
        let query = query.trim().to_owned();
        let (filtered_rows, total_interactions, has_payload_strip, show_inspector_dock) = {
            let wb = wb_entity.read(cx);
            let pane = &wb.collaborator_pane;
            let filtered: Vec<(usize, Uuid)> = pane
                .status
                .interactions
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    self.protocol_filter
                        .is_none_or(|protocol| item.protocol == protocol)
                })
                .filter(|(_, item)| {
                    query.is_empty()
                        || item.summary.to_lowercase().contains(&query)
                        || item.peer_addr.to_lowercase().contains(&query)
                        || item
                            .token
                            .as_ref()
                            .is_some_and(|token| token.to_lowercase().contains(&query))
                        || pane
                            .token_label(item.token.as_deref().unwrap_or_default())
                            .is_some_and(|label| label.to_lowercase().contains(&query))
                        || match item.protocol {
                            OastProtocol::Http => "http".contains(&query),
                            OastProtocol::Dns => "dns".contains(&query),
                        }
                })
                .map(|(idx, item)| (idx, item.id))
                .collect();
            let has_payload_strip = pane.latest_payload_preview.is_some()
                || !pane.status.issued_tokens.is_empty()
                || pane.notice.is_some()
                || self.payloads_drawer_open;
            let show_inspector_dock = pane.selected_interaction.is_some();
            (
                filtered,
                pane.status.interactions.len(),
                has_payload_strip,
                show_inspector_dock,
            )
        };

        let interaction_rows =
            self.render_interaction_rows(&wb_entity, filtered_rows, total_interactions, cx);
        let raw_viewer = self.render_raw_viewer(&wb_entity, window, cx);
        let split_body = v_resizable("collaborator-vsplit")
            .child(
                resizable_panel()
                    .size(px(280.))
                    .size_range(px(130.)..px(900.))
                    .child(interaction_rows),
            )
            .child(
                resizable_panel()
                    .size_range(px(160.)..px(10000.))
                    .child(raw_viewer),
            );
        let main_area = if show_inspector_dock {
            wb_entity.update(cx, |wb, cx| {
                wb.render_inspector_dock(split_body.into_any_element(), cx)
            })
        } else {
            split_body.into_any_element()
        };

        div()
            .id("collaborator-root")
            .test_support()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .text_color(theme::foreground())
            .child(self.render_toolbar(&wb_entity, window, cx))
            .when(self.config_open, |section| {
                section.child(self.render_config_panel(cx))
            })
            .when(has_payload_strip, |section| {
                section.child(self.render_payload_panel(&wb_entity, cx))
            })
            .child(div().flex_1().min_h_0().child(main_area))
            .into_any_element()
    }
}

pub(in crate::workbench) fn format_local_hms(unix_ms: i64) -> String {
    let seconds = unix_ms.div_euclid(1000);
    let Some(local) = crate::local_time::from_unix_seconds(seconds) else {
        return "时间无法显示".into();
    };
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        local.hour,
        local.min,
        local.sec,
        unix_ms.rem_euclid(1000),
    )
}

pub(in crate::workbench) fn protocol_badge_style(
    protocol: OastProtocol,
) -> (&'static str, u32, u32) {
    let dark = theme::is_dark();
    match protocol {
        OastProtocol::Http if dark => ("HTTP", 0x20324a, 0x8ab4f8),
        OastProtocol::Http => ("HTTP", 0xe8f0fe, 0x1967d2),
        OastProtocol::Dns if dark => ("DNS", 0x1e3528, 0x81c995),
        OastProtocol::Dns => ("DNS", 0xe6f4ea, 0x137333),
    }
}

pub(in crate::workbench) fn server_badge_text(status: &CollaboratorStatus) -> (String, u32, u32) {
    let dark = theme::is_dark();
    let (running_bg, running_fg) = if dark {
        (0x1e3528, 0x81c995)
    } else {
        (0xe6f4ea, 0x137333)
    };
    let (idle_bg, idle_fg) = if dark {
        (0x33363a, 0x9da2a8)
    } else {
        (0xeeeeee, 0x555555)
    };
    match (status.running, status.http_addr, status.dns_addr) {
        (true, Some(http), Some(dns)) => (format!("HTTP {http} · DNS {dns}"), running_bg, running_fg),
        (true, http, dns) => (
            format!(
                "运行中{}{}",
                http.map_or_else(String::new, |addr| format!(" · HTTP {addr}")),
                dns.map_or_else(String::new, |addr| format!(" · DNS {addr}")),
            ),
            running_bg,
            running_fg,
        ),
        (false, _, _) => ("未启动".into(), idle_bg, idle_fg),
    }
}
