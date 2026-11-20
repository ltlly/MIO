use std::time::Duration;

use mio_core::{
    FinishedAttackClose, RunningAttackClose, UserDisplaySettings, UserMessageEditorSettings,
    UserMiscSettings, UserPreferences, Uuid,
};

use super::*;

impl UserPreferencesUi {
    pub(in crate::workbench) fn sync_from(&mut self, latest: &Self) -> bool {
        let confirmed = |preferences: &Option<UserPreferences>| {
            preferences
                .as_ref()
                .map(|preferences| (preferences.version, preferences.revision))
        };
        let certificates_changed = confirmed(&self.confirmed) != confirmed(&latest.confirmed)
            && self
                .confirmed
                .as_ref()
                .map(|preferences| &preferences.custom_ca_ders)
                != latest
                    .confirmed
                    .as_ref()
                    .map(|preferences| &preferences.custom_ca_ders);
        if confirmed(&self.confirmed) != confirmed(&latest.confirmed) {
            self.confirmed.clone_from(&latest.confirmed);
        }
        if self.pending.as_ref().map(|pending| pending.operation)
            != latest.pending.as_ref().map(|pending| pending.operation)
        {
            self.pending.clone_from(&latest.pending);
        }
        self.running = latest.running;
        self.finished = latest.finished;
        self.display.clone_from(&latest.display);
        self.message_editor.clone_from(&latest.message_editor);
        self.misc.clone_from(&latest.misc);
        self.load_error.clone_from(&latest.load_error);
        self.error.clone_from(&latest.error);
        self.remember_error.clone_from(&latest.remember_error);
        self.status.clone_from(&latest.status);
        self.query.clone_from(&latest.query);
        self.retain_selection = latest.retain_selection;
        self.retain_workbench = latest.retain_workbench;
        certificates_changed
    }

    fn overdue(&self) -> bool {
        self.query
            .as_ref()
            .is_some_and(|(_, deadline, executor)| executor.now() >= *deadline)
            || self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.deadline.1.now() >= pending.deadline.0)
    }

    pub(in crate::workbench) fn busy(&self) -> bool {
        self.query.is_some() || self.pending.is_some()
    }

    pub(in crate::workbench) fn can_save(&self) -> bool {
        self.confirmed.is_some() && self.load_error.is_none() && !self.busy()
    }
}

impl Workbench {
    pub(in crate::workbench) fn intruder_close_preferences(
        &self,
    ) -> (RunningAttackClose, FinishedAttackClose) {
        if !self.backend_available()
            || self.user_preferences.load_error.is_some()
            || self.user_preferences.overdue()
        {
            return (RunningAttackClose::Ask, FinishedAttackClose::Ask);
        }
        self.user_preferences.confirmed.as_ref().map_or(
            (RunningAttackClose::Ask, FinishedAttackClose::Ask),
            |confirmed| (confirmed.running, confirmed.finished),
        )
    }

    pub(in crate::workbench) fn query_user_preferences(&mut self, cx: &mut Context<Self>) {
        if self.user_preferences.query.is_some() || !self.backend_available() {
            return;
        }
        let operation = Uuid::new_v4();
        if self.enqueue(
            Command::QueryUserPreferences {
                operation_id: operation,
            },
            cx,
        ) {
            let executor = cx.background_executor().clone();
            self.user_preferences.query = Some((
                operation,
                executor.now() + Duration::from_secs(30),
                executor,
            ));
            self.user_preferences.status = "正在读取用户设置…".into();
            self.user_preferences_deadline(operation, false, cx);
        } else {
            self.user_preferences.error = Some("用户设置读取命令未入队，请重试。".into());
        }
        cx.notify();
    }

    pub(in crate::workbench) fn set_user_closing_preferences(
        &mut self,
        running: RunningAttackClose,
        finished: FinishedAttackClose,
        cx: &mut Context<Self>,
    ) {
        self.expire_user_preferences(cx);
        if !self.backend_available() || !self.user_preferences.can_save() {
            return;
        }
        self.user_preferences.running = running;
        self.user_preferences.finished = finished;
        self.user_preferences.retain_selection = true;
        let _ = self.submit_user_closing_preferences(running, finished, SaveOrigin::Settings, cx);
    }

    pub(in crate::workbench) fn set_user_workbench_preferences(
        &mut self,
        display: UserDisplaySettings,
        message_editor: UserMessageEditorSettings,
        misc: UserMiscSettings,
        cx: &mut Context<Self>,
    ) -> Result<Uuid, String> {
        self.expire_user_preferences(cx);
        self.user_preferences_admission()?;
        display.validate().map_err(|error| error.to_string())?;
        message_editor
            .validate()
            .map_err(|error| error.to_string())?;
        misc.validate().map_err(|error| error.to_string())?;
        let confirmed = self.user_preferences.confirmed.as_ref().unwrap();
        let expected_revision = confirmed.revision;
        let running = confirmed.running;
        let finished = confirmed.finished;
        let certificates = confirmed.custom_ca_ders.clone();
        let operation = Uuid::new_v4();
        if !self.enqueue(
            Command::SetUserWorkbenchPreferences {
                operation_id: operation,
                expected_revision,
                display: display.clone(),
                message_editor: message_editor.clone(),
                misc: misc.clone(),
            },
            cx,
        ) {
            let message = "用户设置保存命令未入队；所选选项仍保留，请重试。".to_owned();
            self.user_preferences.error = Some(message.clone());
            self.user_preferences.status = "用户设置未保存。".into();
            cx.notify();
            return Err(message);
        }
        self.user_preferences.display = display.clone();
        self.user_preferences.message_editor = message_editor.clone();
        self.user_preferences.misc = misc.clone();
        self.user_preferences.retain_workbench = true;
        self.user_preferences.pending = Some(PendingSave {
            operation,
            expected_revision,
            running,
            finished,
            display,
            message_editor,
            misc,
            certificates,
            origin: SaveOrigin::Workbench,
            deadline: (
                cx.background_executor().now() + Duration::from_secs(30),
                cx.background_executor().clone(),
            ),
        });
        self.user_preferences.error = None;
        self.user_preferences.status = "正在保存用户设置；确认前保持当前界面生效值。".into();
        self.user_preferences_deadline(operation, true, cx);
        cx.notify();
        Ok(operation)
    }

    pub(in crate::workbench) fn set_user_tls_certificates(
        &mut self,
        certificates: Vec<Vec<u8>>,
        expected_revision: u64,
        cx: &mut Context<Self>,
    ) -> Result<Uuid, String> {
        self.expire_user_preferences(cx);
        if !self.backend_available() || !self.user_preferences.can_save() {
            return Err("用户设置尚未载入、后台不可用或正在保存；证书列表未修改。".into());
        }
        let confirmed = self.user_preferences.confirmed.as_ref().unwrap();
        if confirmed.revision != expected_revision {
            return Err("用户设置在选择证书期间已改变；证书列表未修改，请核对后重试。".into());
        }
        mio_core::UpstreamTlsSettings {
            custom_ca_ders: certificates.clone(),
            include_system_roots: true,
            insecure_skip_verify: false,
            ..Default::default()
        }
        .validate()
        .map_err(|error| error.to_string())?;
        if certificates
            .iter()
            .enumerate()
            .any(|(index, certificate)| certificates[..index].contains(certificate))
        {
            return Err("自定义 CA 证书重复；原列表未修改。".into());
        }
        let operation = Uuid::new_v4();
        let running = confirmed.running;
        let finished = confirmed.finished;
        let display = confirmed.display.clone();
        let message_editor = confirmed.message_editor.clone();
        let misc = confirmed.misc.clone();
        if !self.enqueue(
            Command::SetUserTlsCertificates {
                operation_id: operation,
                expected_revision,
                certificates: certificates.clone(),
            },
            cx,
        ) {
            let message = "自定义 CA 证书保存命令未入队；原列表仍保留，请重试。".to_owned();
            self.user_preferences.error = Some(message.clone());
            cx.notify();
            return Err(message);
        }
        self.user_preferences.pending = Some(PendingSave {
            operation,
            expected_revision,
            running,
            finished,
            display,
            message_editor,
            misc,
            certificates,
            origin: SaveOrigin::Tls,
            deadline: (
                cx.background_executor().now() + Duration::from_secs(30),
                cx.background_executor().clone(),
            ),
        });
        self.user_preferences.error = None;
        self.user_preferences.status = "正在保存用户 TLS 证书；确认前保留原列表。".into();
        self.user_preferences_deadline(operation, true, cx);
        cx.notify();
        Ok(operation)
    }

    pub(in crate::workbench) fn remember_intruder_closing_preference(
        &mut self,
        choice: RememberCloseChoice,
        cx: &mut Context<Self>,
    ) -> Result<Uuid, String> {
        self.expire_user_preferences(cx);
        self.user_preferences_admission()?;
        let confirmed = self.user_preferences.confirmed.as_ref().unwrap();
        let (running, finished) = match choice {
            RememberCloseChoice::Running(value) => (value, confirmed.finished),
            RememberCloseChoice::Finished(value) => (confirmed.running, value),
        };
        let operation = self.submit_user_closing_preferences(
            running,
            finished,
            SaveOrigin::Remember(choice),
            cx,
        )?;
        match choice {
            RememberCloseChoice::Running(value) => self.user_preferences.running = value,
            RememberCloseChoice::Finished(value) => self.user_preferences.finished = value,
        }
        self.user_preferences.retain_selection = true;
        cx.notify();
        Ok(operation)
    }

    fn user_preferences_admission(&self) -> Result<(), String> {
        if !self.backend_available() {
            return Err("后台不可用；下次关闭的选择未提交，本次操作仍需按原选择处理。".into());
        }
        if self.user_preferences.busy() {
            return Err("用户设置操作尚未结束；下次关闭的选择未提交，请稍后重试。".into());
        }
        if self.user_preferences.load_error.is_some() || self.user_preferences.confirmed.is_none() {
            return Err(
                "用户设置尚未正确载入；下次关闭的选择未提交，请在设置中重新读取核对。".into(),
            );
        }
        Ok(())
    }

    fn submit_user_closing_preferences(
        &mut self,
        running: RunningAttackClose,
        finished: FinishedAttackClose,
        origin: SaveOrigin,
        cx: &mut Context<Self>,
    ) -> Result<Uuid, String> {
        self.user_preferences_admission()?;
        let confirmed = self.user_preferences.confirmed.as_ref().unwrap();
        let expected_revision = confirmed.revision;
        let display = confirmed.display.clone();
        let message_editor = confirmed.message_editor.clone();
        let misc = confirmed.misc.clone();
        let certificates = confirmed.custom_ca_ders.clone();
        let operation = Uuid::new_v4();
        if self.enqueue(
            Command::SetUserPreferences {
                operation_id: operation,
                expected_revision,
                running,
                finished,
            },
            cx,
        ) {
            self.user_preferences.remember_error = None;
            self.user_preferences.pending = Some(PendingSave {
                operation,
                expected_revision,
                running,
                finished,
                display,
                message_editor,
                misc,
                certificates,
                origin,
                deadline: (
                    cx.background_executor().now() + Duration::from_secs(30),
                    cx.background_executor().clone(),
                ),
            });
            self.user_preferences.error = None;
            self.user_preferences.status = "正在保存用户设置；关闭行为仍使用已确认的选项。".into();
            self.user_preferences_deadline(operation, true, cx);
            cx.notify();
            Ok(operation)
        } else {
            self.user_preferences.error =
                Some("用户设置保存命令未入队；所选选项仍保留，请重试。".into());
            self.user_preferences.status = "用户设置未保存。".into();
            let message = self
                .error
                .clone()
                .unwrap_or_else(|| "用户设置保存命令未入队。".into());
            cx.notify();
            Err(message)
        }
    }

    pub(in crate::workbench) fn remember_preference_failed(
        &mut self,
        detail: &str,
        cx: &mut Context<Self>,
    ) {
        let message =
            format!("下次关闭的选择未确认；本次操作按原选择处理，可在设置中核对。{detail}");
        self.user_preferences.remember_error = Some(message.clone());
        self.fail(message, cx);
    }

    pub(in crate::workbench) fn retry_user_preferences(&mut self, cx: &mut Context<Self>) {
        self.expire_user_preferences(cx);
        if self.user_preferences.busy() {
            return;
        }
        if self.user_preferences.can_save() {
            if self.user_preferences.retain_workbench && !self.user_preferences.retain_selection {
                let _ = self.set_user_workbench_preferences(
                    self.user_preferences.display.clone(),
                    self.user_preferences.message_editor.clone(),
                    self.user_preferences.misc.clone(),
                    cx,
                );
            } else {
                self.set_user_closing_preferences(
                    self.user_preferences.running,
                    self.user_preferences.finished,
                    cx,
                );
            }
        } else {
            self.query_user_preferences(cx);
        }
    }

    fn user_preferences_deadline(&self, operation: Uuid, save: bool, cx: &mut Context<Self>) {
        let timer = cx.background_executor().timer(Duration::from_secs(30));
        let owner = self.owner_window;
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = owner.update(cx, |_, _, cx| {
                let _ = this.update(cx, |view, cx| {
                    let pending = if save {
                        view.user_preferences
                            .pending
                            .as_ref()
                            .map(|pending| pending.operation)
                    } else {
                        view.user_preferences
                            .query
                            .as_ref()
                            .map(|(operation, _, _)| *operation)
                    };
                    if pending != Some(operation) {
                        return;
                    }
                    view.expire_user_preferences(cx);
                });
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn expire_user_preferences(&mut self, cx: &mut Context<Self>) {
        if self
            .user_preferences
            .query
            .as_ref()
            .is_some_and(|(_, deadline, executor)| executor.now() >= *deadline)
        {
            self.user_preferences.query = None;
            self.user_preferences.load_error =
                Some("用户设置读取已超时；暂使用询问策略，请重试。".into());
            self.user_preferences.status = "用户设置未确认。".into();
            cx.notify();
        }
        if self
            .user_preferences
            .pending
            .as_ref()
            .is_some_and(|pending| pending.deadline.1.now() >= pending.deadline.0)
        {
            let pending = self.user_preferences.pending.take().unwrap();
            self.user_preferences.query = None;
            self.user_preferences.error =
                Some("用户设置保存确认已超时；写入结果未知，正在重新读取，未自动重试保存。".into());
            self.user_preferences.load_error = Some("保存结果尚未确认，暂使用询问策略。".into());
            self.query_user_preferences(cx);
            if matches!(pending.origin, SaveOrigin::Remember(_)) {
                self.remember_preference_failed("保存确认已超时，已重新读取；未自动重试保存。", cx);
            }
            cx.notify();
        }
    }
}
