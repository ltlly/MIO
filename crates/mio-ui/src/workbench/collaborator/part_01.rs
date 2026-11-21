use super::*;

pub(in crate::workbench) const LOCAL_LOOPBACK_NOTICE: &str =
    "默认使用本地回环 (Loopback) HTTP/DNS 监听，不发起公网连接；仅在启用自托管端点时轮询指定服务器";

pub(in crate::workbench) const SELF_HOSTED_SECURITY_NOTE: &str =
    "安全提示：启用自托管端点后将向该 URL 发起轮询请求；默认阻止公共第三方 Collaborator 云域名。";

pub(in crate::workbench) const MAX_INPUT_BYTES: usize = 2048;

pub(in crate::workbench) const MAX_LABEL_BYTES: usize = 256;

const COLLABORATOR_RADAR_LIGHT_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="126" height="83" viewBox="0 0 252 166">
  <defs>
    <linearGradient id="sweep" gradientUnits="userSpaceOnUse" x1="8" y1="0" x2="114" y2="0">
      <stop offset="0%" stop-color="#786cf5" stop-opacity="0.92"/>
      <stop offset="48%" stop-color="#9b9de9" stop-opacity="0.72"/>
      <stop offset="100%" stop-color="#bfd0e0" stop-opacity="0.08"/>
    </linearGradient>
  </defs>
  <path d="M 4 72 L 4 91 A 122.5 70.5 0 0 0 249 91 L 249 72 A 122.5 70.5 0 0 1 4 72 Z" fill="#cacacd"/>
  <g transform="translate(126.5 72) scale(1 0.576)">
    <circle cx="0" cy="0" r="118" fill="#bfd0e0"/>
    <circle cx="0" cy="0" r="85" fill="none" stroke="#759abc" stroke-width="6.5"/>
    <circle cx="0" cy="0" r="50.5" fill="none" stroke="#517faa" stroke-width="6.5"/>
    <path d="M 0 0 L 101.6 -51.8 A 114 114 0 0 1 101.6 51.8 Z" fill="url(#sweep)"/>
    <line x1="-83.4" y1="-83.4" x2="83.4" y2="83.4" stroke="#2f6499" stroke-width="3.6"/>
    <line x1="-83.4" y1="83.4" x2="83.4" y2="-83.4" stroke="#2f6499" stroke-width="3.6"/>
    <circle cx="0" cy="0" r="118" fill="none" stroke="#2f6499" stroke-width="10"/>
    <circle cx="0" cy="0" r="10" fill="#2f6499"/>
    <circle cx="-112.5" cy="41.7" r="14" fill="#f67f50"/>
    <circle cx="-7.0" cy="-85.1" r="14" fill="#f67f50"/>
    <circle cx="17.5" cy="45.1" r="14" fill="#f67f50"/>
  </g>
</svg>"##;

const COLLABORATOR_RADAR_DARK_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="126" height="83" viewBox="0 0 252 166">
  <defs>
    <linearGradient id="sweep" gradientUnits="userSpaceOnUse" x1="8" y1="0" x2="114" y2="0">
      <stop offset="0%" stop-color="#8b80ff" stop-opacity="0.90"/>
      <stop offset="48%" stop-color="#7c82d8" stop-opacity="0.62"/>
      <stop offset="100%" stop-color="#2c3e52" stop-opacity="0.08"/>
    </linearGradient>
  </defs>
  <path d="M 4 72 L 4 91 A 122.5 70.5 0 0 0 249 91 L 249 72 A 122.5 70.5 0 0 1 4 72 Z" fill="#32363d"/>
  <g transform="translate(126.5 72) scale(1 0.576)">
    <circle cx="0" cy="0" r="118" fill="#2c3e52"/>
    <circle cx="0" cy="0" r="85" fill="none" stroke="#5c84ad" stroke-width="6.5"/>
    <circle cx="0" cy="0" r="50.5" fill="none" stroke="#4d79a6" stroke-width="6.5"/>
    <path d="M 0 0 L 101.6 -51.8 A 114 114 0 0 1 101.6 51.8 Z" fill="url(#sweep)"/>
    <line x1="-83.4" y1="-83.4" x2="83.4" y2="83.4" stroke="#6c9ed0" stroke-width="3.6"/>
    <line x1="-83.4" y1="83.4" x2="83.4" y2="-83.4" stroke="#6c9ed0" stroke-width="3.6"/>
    <circle cx="0" cy="0" r="118" fill="none" stroke="#5386ba" stroke-width="10"/>
    <circle cx="0" cy="0" r="10" fill="#6c9ed0"/>
    <circle cx="-112.5" cy="41.7" r="14" fill="#f67f50"/>
    <circle cx="-7.0" cy="-85.1" r="14" fill="#f67f50"/>
    <circle cx="17.5" cy="45.1" r="14" fill="#f67f50"/>
  </g>
</svg>"##;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum CopyMode {
    Http,
    Dns,
}

pub(in crate::workbench) struct CollaboratorPane {
    pub(in crate::workbench) status: CollaboratorStatus,
    pub(in crate::workbench) loaded: bool,
    pub(in crate::workbench) onboarding_dismissed: bool,
    pub(in crate::workbench) payloads_drawer_open: bool,
    pub(in crate::workbench) radar_light: Option<Arc<RenderImage>>,
    pub(in crate::workbench) radar_dark: Option<Arc<RenderImage>>,
    pub(in crate::workbench) count_input: Entity<InputState>,
    pub(in crate::workbench) label_input: Entity<InputState>,
    pub(in crate::workbench) protocol_filter: Option<OastProtocol>,
    pub(in crate::workbench) search_input: Entity<InputState>,
    pub(in crate::workbench) selected_interaction: Option<Uuid>,
    pub(in crate::workbench) raw_editor: MessageEditor,
    pub(in crate::workbench) latest_payload_preview: Option<String>,
    pub(in crate::workbench) notice: Option<String>,
    pub(in crate::workbench) error: Option<String>,
    pub(in crate::workbench) pending_copy: Option<CopyMode>,
    pub(in crate::workbench) config_open: bool,
    pub(in crate::workbench) self_hosted_enabled: bool,
    pub(in crate::workbench) polling_url_input: Entity<InputState>,
    pub(in crate::workbench) server_domain_input: Entity<InputState>,
    pub(in crate::workbench) auth_header_input: Entity<InputState>,
    pub(in crate::workbench) poll_interval_input: Entity<InputState>,
    pub(in crate::workbench) _subscriptions: Vec<Subscription>,
}

impl CollaboratorPane {
    pub(in crate::workbench) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let count_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value("1")
                .placeholder("1–32")
        });
        let label_input = cx.new(|cx| InputState::new(window, cx).placeholder("备注标签"));
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("搜索交互…"));
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
        let raw_editor = MessageEditor::new(window, cx);

        let mut subscriptions = Vec::new();
        for editor in [&raw_editor.raw, &raw_editor.hex, &raw_editor.pretty] {
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
            status: CollaboratorStatus::default(),
            loaded: false,
            onboarding_dismissed: false,
            payloads_drawer_open: false,
            radar_light: None,
            radar_dark: None,
            count_input,
            label_input,
            protocol_filter: None,
            search_input,
            selected_interaction: None,
            raw_editor,
            latest_payload_preview: None,
            notice: None,
            error: None,
            pending_copy: None,
            config_open: false,
            self_hosted_enabled: false,
            polling_url_input,
            server_domain_input,
            auth_header_input,
            poll_interval_input,
            _subscriptions: subscriptions,
        }
    }

    pub(in crate::workbench) fn radar_image(
        &mut self,
        dark: bool,
        cx: &App,
    ) -> Option<Arc<RenderImage>> {
        let slot = if dark {
            &mut self.radar_dark
        } else {
            &mut self.radar_light
        };
        if slot.is_none() {
            let svg = if dark {
                COLLABORATOR_RADAR_DARK_SVG
            } else {
                COLLABORATOR_RADAR_LIGHT_SVG
            };
            *slot = cx
                .svg_renderer()
                .render_single_frame(svg.as_bytes(), 1.0)
                .ok();
        }
        slot.clone()
    }

    pub(in crate::workbench) fn inputs(&self) -> [&Entity<InputState>; 8] {
        [
            &self.count_input,
            &self.label_input,
            &self.search_input,
            &self.polling_url_input,
            &self.server_domain_input,
            &self.auth_header_input,
            &self.poll_interval_input,
            &self.raw_editor.search,
        ]
    }

    pub(in crate::workbench) fn validate_composition(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), &'static str> {
        for input in self.inputs() {
            editing::input_snapshot(input, window, cx)?;
        }
        for editor in [
            &self.raw_editor.raw,
            &self.raw_editor.hex,
            &self.raw_editor.pretty,
        ] {
            editing::snapshot(editor, window, cx)?;
        }
        Ok(())
    }

    pub(in crate::workbench) fn has_non_editor_focus(&self, window: &Window, cx: &App) -> bool {
        [
            &self.count_input,
            &self.label_input,
            &self.search_input,
            &self.polling_url_input,
            &self.server_domain_input,
            &self.auth_header_input,
            &self.poll_interval_input,
        ]
        .into_iter()
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

    pub(in crate::workbench) fn sync_config_inputs(
        &mut self,
        config: &SelfHostedOastConfig,
        window: &mut Window,
        cx: &mut App,
    ) {
        let any_composing = [
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
        for (input, val) in [
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

impl Workbench {
    pub(in crate::workbench) fn collaborator_can_operate(&self) -> bool {
        self.project.is_some() && self.backend_available() && self.transition.is_none()
    }

    pub(in crate::workbench) fn activate_collaborator(&mut self, cx: &mut Context<Self>) {
        if !self.collaborator_can_operate() || self.collaborator_pane.loaded {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        if self.enqueue(
            Command::Collaborator(CollaboratorCommand::QueryStatus { project }),
            cx,
        ) {
            self.collaborator_pane.loaded = true;
        }
    }

    pub(in crate::workbench) fn reset_collaborator_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = &mut self.collaborator_pane;
        pane.status = CollaboratorStatus::default();
        pane.loaded = false;
        pane.onboarding_dismissed = false;
        pane.payloads_drawer_open = false;
        pane.protocol_filter = None;
        pane.selected_interaction = None;
        pane.latest_payload_preview = None;
        pane.notice = None;
        pane.error = None;
        pane.pending_copy = None;
        pane.config_open = false;
        pane.self_hosted_enabled = false;
        pane.raw_editor.load(b"", window, cx);
        for (input, value) in [
            (&pane.count_input, "1"),
            (&pane.label_input, ""),
            (&pane.search_input, ""),
            (&pane.polling_url_input, ""),
            (&pane.server_domain_input, ""),
            (&pane.auth_header_input, ""),
            (&pane.poll_interval_input, "5000"),
            (&pane.raw_editor.search, ""),
        ] {
            input.update(cx, |state, cx| state.set_value(value, window, cx));
        }
        cx.notify();
    }

    pub(in crate::workbench) fn collaborator_backend_disconnected(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let pane = &mut self.collaborator_pane;
        let had_activity = pane.status.running || pane.pending_copy.take().is_some();
        pane.status.running = false;
        pane.status.http_addr = None;
        pane.status.dns_addr = None;
        if had_activity {
            pane.error = Some("后台服务已断开；OAST 监听器状态与在途请求结果未确认。".into());
        }
        cx.notify();
    }

    pub(in crate::workbench) fn active_collaborator_message_editor(
        &mut self,
        window: &Window,
        cx: &App,
    ) -> Option<&mut MessageEditor> {
        if self.tool != Tool::Collaborator
            || self.collaborator_pane.selected_interaction.is_none()
            || self.collaborator_pane.has_non_editor_focus(window, cx)
        {
            return None;
        }
        Some(&mut self.collaborator_pane.raw_editor)
    }

    pub(in crate::workbench) fn on_collaborator_event(
        &mut self,
        event: CollaboratorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project != Some(event.project()) {
            return;
        }
        match event {
            CollaboratorEvent::Status { status, .. } => {
                let initial = !self.collaborator_pane.config_open
                    && !self.collaborator_pane.self_hosted_enabled;
                if initial || status.config.enabled {
                    self.collaborator_pane
                        .sync_config_inputs(&status.config, window, cx);
                }
                if status.running
                    || !status.issued_tokens.is_empty()
                    || !status.interactions.is_empty()
                {
                    self.collaborator_pane.onboarding_dismissed = true;
                }
                self.collaborator_pane.loaded = true;
                self.collaborator_pane.status = *status;
                if let Some(selected_id) = self.collaborator_pane.selected_interaction
                    && !self
                        .collaborator_pane
                        .status
                        .interactions
                        .iter()
                        .any(|item| item.id == selected_id)
                {
                    self.collaborator_pane.selected_interaction = None;
                    self.collaborator_pane.raw_editor.load(b"", window, cx);
                }
            }
            CollaboratorEvent::PayloadsGenerated { payloads, .. } => {
                self.apply_generated_collaborator_payloads(payloads, cx);
            }
            CollaboratorEvent::InteractionObserved { interaction, .. } => {
                self.collaborator_pane.onboarding_dismissed = true;
                self.record_collaborator_interaction(*interaction, window, cx);
            }
            CollaboratorEvent::Error { message, .. } => {
                self.collaborator_pane.pending_copy = None;
                self.collaborator_pane.error = Some(message.clone());
                self.fail(message, cx);
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn apply_generated_collaborator_payloads(
        &mut self,
        payloads: Vec<OastPayloadToken>,
        cx: &mut Context<Self>,
    ) {
        let mode = self
            .collaborator_pane
            .pending_copy
            .take()
            .unwrap_or(CopyMode::Http);
        for payload in &payloads {
            if !self
                .collaborator_pane
                .status
                .issued_tokens
                .iter()
                .any(|existing| existing.token == payload.token)
            {
                self.collaborator_pane
                    .status
                    .issued_tokens
                    .push(payload.clone());
            }
        }
        if payloads.is_empty() {
            return;
        }
        self.collaborator_pane.onboarding_dismissed = true;
        let (mode_label, lines): (&str, Vec<&str>) = match mode {
            CopyMode::Http => (
                "HTTP 回调 URL",
                payloads
                    .iter()
                    .map(|item| item.http_callback_url.as_str())
                    .collect(),
            ),
            CopyMode::Dns => (
                "DNS 域名",
                payloads
                    .iter()
                    .map(|item| item.dns_qname.as_str())
                    .collect(),
            ),
        };
        let joined = lines.join("\n");
        cx.write_to_clipboard(ClipboardItem::new_string(joined.clone()));
        self.collaborator_pane.latest_payload_preview = Some(joined);
        self.collaborator_pane.notice = Some(format!(
            "已生成并复制 {} 条 {mode_label} 到剪贴板。",
            payloads.len()
        ));
        self.collaborator_pane.error = None;
    }
}
