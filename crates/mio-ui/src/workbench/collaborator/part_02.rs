use super::*;

impl Workbench {
    pub(in crate::workbench) fn record_collaborator_interaction(
        &mut self,
        interaction: OastInteraction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = interaction.id;
        if let Some(existing) = self
            .collaborator_pane
            .status
            .interactions
            .iter_mut()
            .find(|item| item.id == id)
        {
            *existing = interaction.clone();
        } else {
            self.collaborator_pane
                .status
                .interactions
                .insert(0, interaction.clone());
            if self.collaborator_pane.status.interactions.len() > MAX_COLLABORATOR_INTERACTIONS {
                self.collaborator_pane
                    .status
                    .interactions
                    .truncate(MAX_COLLABORATOR_INTERACTIONS);
            }
        }
        if self.collaborator_pane.selected_interaction == Some(id) {
            self.load_collaborator_interaction_editor(&interaction, window, cx);
        }
    }

    pub(in crate::workbench) fn load_collaborator_interaction_editor(
        &mut self,
        interaction: &OastInteraction,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.collaborator_pane
            .raw_editor
            .load(&interaction.raw_request, window, cx);
        if interaction.protocol == OastProtocol::Dns {
            let _ = self
                .collaborator_pane
                .raw_editor
                .switch(MessageView::Hex, window, cx);
        }
    }

    pub(in crate::workbench) fn fail_collaborator(
        &mut self,
        message: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let message = message.into();
        self.collaborator_pane.error = Some(message.clone());
        self.fail(message, cx);
    }

    pub(in crate::workbench) fn snapshot_collaborator_config(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<SelfHostedOastConfig, String> {
        self.collaborator_pane
            .validate_composition(window, cx)
            .map_err(str::to_owned)?;
        let pane = &self.collaborator_pane;
        if !pane.self_hosted_enabled {
            return Ok(SelfHostedOastConfig::default());
        }
        let polling_url = editing::input_snapshot(&pane.polling_url_input, window, cx)
            .map_err(str::to_owned)?
            .trim()
            .to_owned();
        let server_domain = editing::input_snapshot(&pane.server_domain_input, window, cx)
            .map_err(str::to_owned)?
            .trim()
            .to_owned();
        let auth_header = editing::input_snapshot(&pane.auth_header_input, window, cx)
            .map_err(str::to_owned)?
            .trim()
            .to_owned();
        let poll_interval_text = editing::input_snapshot(&pane.poll_interval_input, window, cx)
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
        };
        config
            .validate()
            .map_err(|err| format!("自托管 OAST 配置无效：{err}"))?;
        Ok(config)
    }

    pub(in crate::workbench) fn start_collaborator_server(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.collaborator_can_operate() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let config = match self.snapshot_collaborator_config(window, cx) {
            Ok(config) => config,
            Err(err) => {
                self.fail_collaborator(err, cx);
                return;
            }
        };
        if self.enqueue(
            Command::Collaborator(CollaboratorCommand::Start { project, config }),
            cx,
        ) {
            self.collaborator_pane.onboarding_dismissed = true;
            self.collaborator_pane.error = None;
            self.collaborator_pane.notice = Some("正在启动本地 Loopback OAST 监听器…".into());
        }
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_collaborator_server(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.collaborator_can_operate() {
            return;
        }
        if let Err(err) = self.collaborator_pane.validate_composition(window, cx) {
            self.fail_collaborator(err, cx);
            return;
        }
        if self.collaborator_pane.status.running {
            let Some(project) = self.project else {
                return;
            };
            if self.enqueue(
                Command::Collaborator(CollaboratorCommand::Stop { project }),
                cx,
            ) {
                self.collaborator_pane.error = None;
                self.collaborator_pane.notice = Some("正在停止本地 Loopback OAST 监听器…".into());
            }
            cx.notify();
        } else {
            self.start_collaborator_server(window, cx);
        }
    }

    pub(in crate::workbench) fn generate_collaborator_payloads(
        &mut self,
        mode: CopyMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.collaborator_can_operate() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        if let Err(err) = self.collaborator_pane.validate_composition(window, cx) {
            self.fail_collaborator(err, cx);
            return;
        }
        let count_text =
            match editing::input_snapshot(&self.collaborator_pane.count_input, window, cx) {
                Ok(text) => text,
                Err(err) => {
                    self.fail_collaborator(err, cx);
                    return;
                }
            };
        let count: usize = match count_text.trim().parse() {
            Ok(value) if (1..=MAX_COLLABORATOR_BATCH_PAYLOADS).contains(&value) => value,
            _ => {
                self.fail_collaborator(
                    format!("生成数量须为 1–{MAX_COLLABORATOR_BATCH_PAYLOADS} 的整数。"),
                    cx,
                );
                return;
            }
        };
        let label = match editing::input_snapshot(&self.collaborator_pane.label_input, window, cx) {
            Ok(text) => text.trim().to_owned(),
            Err(err) => {
                self.fail_collaborator(err, cx);
                return;
            }
        };
        if label.len() > MAX_LABEL_BYTES {
            self.fail_collaborator(
                format!("备注标签长度不能超过 {MAX_LABEL_BYTES} 字节；输入已保留。"),
                cx,
            );
            return;
        }
        if self.enqueue(
            Command::Collaborator(CollaboratorCommand::GeneratePayloads {
                project,
                count,
                label,
            }),
            cx,
        ) {
            self.collaborator_pane.onboarding_dismissed = true;
            self.collaborator_pane.pending_copy = Some(mode);
            self.collaborator_pane.error = None;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn poll_collaborator_now(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.collaborator_can_operate() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        if let Err(err) = self.collaborator_pane.validate_composition(window, cx) {
            self.fail_collaborator(err, cx);
            return;
        }
        if self.enqueue(
            Command::Collaborator(CollaboratorCommand::PollNow { project }),
            cx,
        ) {
            self.collaborator_pane.error = None;
            self.collaborator_pane.notice = Some("已触发立即轮询 OAST 交互事件。".into());
        }
        cx.notify();
    }

    pub(in crate::workbench) fn clear_collaborator_interactions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.collaborator_can_operate() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        if let Err(err) = self.collaborator_pane.validate_composition(window, cx) {
            self.fail_collaborator(err, cx);
            return;
        }
        if self.enqueue(
            Command::Collaborator(CollaboratorCommand::ClearInteractions { project }),
            cx,
        ) {
            self.collaborator_pane.status.interactions.clear();
            self.collaborator_pane.selected_interaction = None;
            self.collaborator_pane.raw_editor.load(b"", window, cx);
            self.collaborator_pane.error = None;
            self.collaborator_pane.notice = Some("已清空已捕获的 OAST 交互记录。".into());
        }
        cx.notify();
    }

    pub(in crate::workbench) fn set_collaborator_protocol_filter(
        &mut self,
        filter: Option<OastProtocol>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.collaborator_pane.validate_composition(window, cx) {
            self.fail_collaborator(err, cx);
            return;
        }
        self.collaborator_pane.protocol_filter = filter;
        cx.notify();
    }

    pub(in crate::workbench) fn select_collaborator_interaction(
        &mut self,
        interaction_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.collaborator_pane.validate_composition(window, cx) {
            self.fail_collaborator(err, cx);
            return;
        }
        let Some(interaction) = self
            .collaborator_pane
            .status
            .interactions
            .iter()
            .find(|item| item.id == interaction_id)
            .cloned()
        else {
            return;
        };
        self.collaborator_pane.selected_interaction = Some(interaction_id);
        self.load_collaborator_interaction_editor(&interaction, window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn switch_collaborator_raw_pretty(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.collaborator_pane.raw_editor.switch_pretty(window, cx) {
            self.fail_collaborator(err, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn switch_collaborator_raw_view(
        &mut self,
        view: MessageView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.collaborator_pane.raw_editor.switch(view, window, cx) {
            self.fail_collaborator(err, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn handle_collaborator_search_action(
        &mut self,
        action: SearchBarAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self
            .collaborator_pane
            .raw_editor
            .handle_search_bar_action(action, window, cx)
        {
            self.fail_collaborator(err, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_collaborator_config_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.collaborator_pane.validate_composition(window, cx) {
            self.fail_collaborator(err, cx);
            return;
        }
        self.collaborator_pane.onboarding_dismissed = true;
        self.collaborator_pane.config_open = !self.collaborator_pane.config_open;
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_collaborator_self_hosted(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.collaborator_pane.validate_composition(window, cx) {
            self.fail_collaborator(err, cx);
            return;
        }
        self.collaborator_pane.self_hosted_enabled = !self.collaborator_pane.self_hosted_enabled;
        cx.notify();
    }

    pub(in crate::workbench::collaborator) fn collaborator_user_misc(
        &self,
    ) -> &mio_core::UserMiscSettings {
        if self.user_preferences.busy() {
            &self.user_preferences.misc
        } else if let Some(confirmed) = &self.user_preferences.confirmed {
            if self.user_preferences.misc != mio_core::UserMiscSettings::default()
                && confirmed.misc == mio_core::UserMiscSettings::default()
            {
                &self.user_preferences.misc
            } else {
                &confirmed.misc
            }
        } else {
            &self.user_preferences.misc
        }
    }

    pub(in crate::workbench) fn show_collaborator_onboarding(&self) -> bool {
        !self.collaborator_pane.onboarding_dismissed
            && !self.collaborator_user_misc().collaborator_onboarding_dismissed
            && !self.collaborator_pane.status.running
            && self.collaborator_pane.status.issued_tokens.is_empty()
            && self.collaborator_pane.status.interactions.is_empty()
            && !self.collaborator_pane.config_open
    }

    pub(in crate::workbench) fn dismiss_collaborator_onboarding(&mut self, cx: &mut Context<Self>) {
        self.collaborator_pane.onboarding_dismissed = true;
        let mut misc = self.collaborator_user_misc().clone();
        let changed = !misc.collaborator_onboarding_dismissed;
        misc.collaborator_onboarding_dismissed = true;
        self.user_preferences.misc = misc.clone();
        if changed && self.user_preferences.can_save() {
            let display = self
                .user_preferences
                .confirmed
                .as_ref()
                .map(|p| p.display.clone())
                .unwrap_or_else(|| self.user_preferences.display.clone());
            let message_editor = self
                .user_preferences
                .confirmed
                .as_ref()
                .map(|p| p.message_editor.clone())
                .unwrap_or_else(|| self.user_preferences.message_editor.clone());
            let _ = self.set_user_workbench_preferences(display, message_editor, misc, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_collaborator_payloads_drawer(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.collaborator_pane.payloads_drawer_open =
            !self.collaborator_pane.payloads_drawer_open;
        cx.notify();
    }

    pub(in crate::workbench) fn render_collaborator(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.activate_collaborator(cx);
        if self.show_collaborator_onboarding() {
            return div()
                .id("collaborator-root")
                .test_support()
                .size_full()
                .flex()
                .flex_col()
                .bg(theme::background())
                .text_color(theme::foreground())
                .child(self.render_collaborator_onboarding(window, cx))
                .into_any_element();
        }
        let query = self
            .collaborator_pane
            .search_input
            .read(cx)
            .value()
            .to_lowercase();
        let query = query.trim().to_owned();
        let interactions: Vec<(usize, &OastInteraction)> = self
            .collaborator_pane
            .status
            .interactions
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                self.collaborator_pane
                    .protocol_filter
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
                    || self
                        .collaborator_pane
                        .token_label(item.token.as_deref().unwrap_or_default())
                        .is_some_and(|label| label.to_lowercase().contains(&query))
                    || match item.protocol {
                        OastProtocol::Http => "http".contains(&query),
                        OastProtocol::Dns => "dns".contains(&query),
                    }
            })
            .collect();
        let has_payload_strip = self.collaborator_pane.latest_payload_preview.is_some()
            || !self.collaborator_pane.status.issued_tokens.is_empty()
            || self.collaborator_pane.notice.is_some()
            || self.collaborator_pane.payloads_drawer_open;
        let show_inspector_dock = self.collaborator_pane.selected_interaction.is_some();
        let interaction_rows = self.render_collaborator_interaction_rows(interactions, cx);
        let raw_viewer = self.render_collaborator_raw_viewer(window, cx);
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
            self.render_inspector_dock(split_body.into_any_element(), cx)
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
            .child(self.render_collaborator_toolbar(window, cx))
            .when(self.collaborator_pane.config_open, |section| {
                section.child(self.render_collaborator_config_panel(cx))
            })
            .when(has_payload_strip, |section| {
                section.child(self.render_collaborator_payload_panel(cx))
            })
            .child(div().flex_1().min_h_0().child(main_area))
            .into_any_element()
    }
}
