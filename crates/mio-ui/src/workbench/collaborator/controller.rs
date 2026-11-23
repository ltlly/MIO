use super::*;

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
        pane.selected_interaction = None;
        pane.latest_payload_preview = None;
        pane.notice = None;
        pane.error = None;
        pane.pending_copy = None;
        pane.raw_editor.load(b"", window, cx);
        pane.raw_editor
            .search
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.collaborator_view
            .update(cx, |view, cx| view.reset(window, cx));
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
        self.collaborator_view.update(cx, |_, cx| cx.notify());
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
                let view_state = self.collaborator_view.read(cx);
                let initial = !view_state.config_open && !view_state.self_hosted_enabled;
                if initial || status.config.enabled {
                    let config = status.config.clone();
                    self.collaborator_view
                        .update(cx, |view, cx| view.sync_config_inputs(&config, window, cx));
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
        self.collaborator_view.update(cx, |_, cx| cx.notify());
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

    pub(super) fn start_collaborator_server_with(
        &mut self,
        config: SelfHostedOastConfig,
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
            Command::Collaborator(CollaboratorCommand::Start { project, config }),
            cx,
        ) {
            self.collaborator_pane.onboarding_dismissed = true;
            self.collaborator_pane.error = None;
            self.collaborator_pane.notice = Some("正在启动 Collaborator OAST 监听器…".into());
        }
        cx.notify();
    }

    pub(super) fn stop_collaborator_server(
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
    }

    pub(super) fn generate_collaborator_payloads_with(
        &mut self,
        mode: CopyMode,
        count_text: &str,
        label: String,
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
        self.collaborator_view.update(cx, |_, cx| cx.notify());
    }

    pub(super) fn collaborator_user_misc(&self) -> &mio_core::UserMiscSettings {
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

    pub(super) fn show_collaborator_onboarding(&self, config_open: bool) -> bool {
        !self.collaborator_pane.onboarding_dismissed
            && !self.collaborator_user_misc().collaborator_onboarding_dismissed
            && !self.collaborator_pane.status.running
            && self.collaborator_pane.status.issued_tokens.is_empty()
            && self.collaborator_pane.status.interactions.is_empty()
            && !config_open
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

    pub(in crate::workbench) fn copy_collaborator_interactions_json(
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
        match mio_core::render_collaborator_interactions_json(
            &self.collaborator_pane.status.interactions,
        ) {
            Ok(json) => {
                let count = self.collaborator_pane.status.interactions.len();
                cx.write_to_clipboard(ClipboardItem::new_string(json));
                self.collaborator_pane.error = None;
                self.collaborator_pane.notice =
                    Some(format!("已复制 {count} 条交互记录 (JSON) 到剪贴板。"));
                cx.notify();
            }
            Err(err) => self.fail_collaborator(err.to_string(), cx),
        }
    }

    pub(in crate::workbench) fn export_collaborator_interactions_file(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.collaborator_can_operate() || self.file_picker_pending() {
            return;
        }
        if let Err(err) = self.collaborator_pane.validate_composition(window, cx) {
            self.fail_collaborator(err, cx);
            return;
        }
        let count = self.collaborator_pane.status.interactions.len();
        let json = match mio_core::render_collaborator_interactions_json(
            &self.collaborator_pane.status.interactions,
        ) {
            Ok(json) => json,
            Err(err) => {
                self.fail_collaborator(err.to_string(), cx);
                return;
            }
        };
        let Some(project) = self.project else { return };
        let directory = self.default_directory();
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &directory,
            Some("collaborator-interactions.json"),
        );
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = this.update_in(cx, |view, _, cx| {
                if !view.finish_file_picker(picker, cx) || view.project != Some(project) {
                    return;
                }
                match result {
                    Ok(Ok(Some(path))) => {
                        match mio_core::ui_files::write_export_file(&path, json.as_bytes()) {
                            Ok(()) => {
                                view.collaborator_pane.error = None;
                                let msg =
                                    format!("已导出 {count} 条交互记录到 {}。", path.display());
                                view.collaborator_pane.notice = Some(msg.clone());
                                view.status = msg;
                            }
                            Err(err) => {
                                view.fail_collaborator(format!("导出交互记录失败：{err}"), cx);
                            }
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(err)) => {
                        view.fail_collaborator(format!("无法选择导出路径：{err}"), cx);
                    }
                    Err(_) => view.fail_collaborator("文件选择器已关闭。", cx),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
