use gpui_kit::size;
use mio_core::{
    ProjectStartOptions, StartupDefaultConfig, UserPreferences, UserPreferencesState, Uuid,
};

use super::*;

impl Workbench {
    pub(in crate::workbench) fn init_startup_wizard(
        &mut self,
        initial_project: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = cx.weak_entity();
        let recent_table = cx.new(|cx| {
            TableState::new(
                RecentProjectsDelegate::default().with_owner(owner),
                window,
                cx,
            )
            .sortable(true)
            .col_selectable(false)
            .select_on_modifier_click(false)
        });
        let new_name_input = cx.new(|cx| InputState::new(window, cx));
        let new_file_input = cx.new(|cx| InputState::new(window, cx));
        let open_file_input = cx.new(|cx| InputState::new(window, cx));
        let config_file_input = cx.new(|cx| InputState::new(window, cx));

        let name_sub = cx.subscribe_in(&new_name_input, window, |this, _, event, window, cx| {
            match event {
                InputEvent::Focus => {
                    if this.startup_wizard.as_ref().is_some_and(|w| {
                        w.step == WizardStep::Step1
                            && !w.cancel_confirm_open
                            && w.mode != ProjectMode::NewOnDisk
                    }) {
                        this.select_wizard_project_mode(ProjectMode::NewOnDisk, window, cx);
                    }
                }
                InputEvent::Change => {
                    if this.startup_wizard.as_ref().is_some_and(|w| {
                        !w.syncing_inputs
                            && w.step == WizardStep::Step1
                            && !w.cancel_confirm_open
                            && w.mode != ProjectMode::NewOnDisk
                    }) {
                        this.select_wizard_project_mode(ProjectMode::NewOnDisk, window, cx);
                    }
                    this.on_wizard_new_name_changed(window, cx);
                }
                _ => {}
            }
        });
        let new_file_sub =
            cx.subscribe_in(&new_file_input, window, |this, _, event, window, cx| {
                match event {
                    InputEvent::Focus => {
                        if this.startup_wizard.as_ref().is_some_and(|w| {
                            w.step == WizardStep::Step1
                                && !w.cancel_confirm_open
                                && w.mode != ProjectMode::NewOnDisk
                        }) {
                            this.select_wizard_project_mode(ProjectMode::NewOnDisk, window, cx);
                        }
                    }
                    InputEvent::Change => {
                        if this.startup_wizard.as_ref().is_some_and(|w| {
                            !w.syncing_inputs
                                && w.step == WizardStep::Step1
                                && !w.cancel_confirm_open
                                && w.mode != ProjectMode::NewOnDisk
                        }) {
                            this.select_wizard_project_mode(ProjectMode::NewOnDisk, window, cx);
                        }
                        this.on_wizard_new_file_changed(window, cx);
                    }
                    _ => {}
                }
            });
        let open_file_sub =
            cx.subscribe_in(&open_file_input, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::Focus | InputEvent::Change)
                    && this.startup_wizard.as_ref().is_some_and(|w| {
                        !w.syncing_inputs
                            && w.step == WizardStep::Step1
                            && !w.cancel_confirm_open
                            && w.mode != ProjectMode::OpenExisting
                    })
                {
                    this.select_wizard_project_mode(ProjectMode::OpenExisting, window, cx);
                }
            });
        let config_file_sub =
            cx.subscribe_in(&config_file_input, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::Focus | InputEvent::Change)
                    && let Some(wizard) = &mut this.startup_wizard
                    && wizard.step == WizardStep::Step2
                    && !wizard.cancel_confirm_open
                    && wizard.config_choice != ConfigChoice::ConfigFile
                {
                    wizard.config_choice = ConfigChoice::ConfigFile;
                    wizard.config_choice_customized = true;
                    wizard.last_start_failed = false;
                    wizard.error = None;
                    cx.notify();
                }
            });
        let recent_sub =
            cx.subscribe_in(
                &recent_table,
                window,
                |this, _, event, window, cx| match event {
                    TableEvent::SelectRow(row) | TableEvent::DoubleClickedRow(row) => {
                        this.on_wizard_select_recent_row(*row, window, cx);
                    }
                    _ => {}
                },
            );

        let has_cli = initial_project.is_some();
        self.startup_wizard = Some(Box::new(StartupWizard {
            active: !has_cli,
            step: WizardStep::Step1,
            mode: ProjectMode::Temporary,
            new_name_input,
            new_file_input,
            open_file_input,
            config_file_input,
            recent_table,
            config_list_scroll: ScrollHandle::new(),
            trust_project_file: true,
            pause_automated_tasks: true,
            config_choice: ConfigChoice::Defaults,
            config_choice_customized: false,
            default_in_future: false,
            disable_extensions: false,
            disable_extensions_customized: false,
            cancel_confirm_open: false,
            new_file_custom_dir: None,
            new_file_customized: false,
            syncing_inputs: false,
            picker: None,
            pending_preference_save: None,
            pending_recent_save: None,
            awaiting_start: false,
            initial_project_ready: false,
            cli_initial_path: initial_project,
            cli_retry_path: None,
            last_start_failed: false,
            error: None,
            _subscriptions: vec![
                name_sub,
                new_file_sub,
                open_file_sub,
                config_file_sub,
                recent_sub,
            ],
        }));
        if !has_cli {
            self.status = "请选择要创建或打开的项目。".into();
            window.set_window_title("MIO");
            let frame = window.bounds().size;
            let viewport = window.viewport_size();
            let scale = theme::ui_scale();
            let target_w = if cfg!(test) || scale <= 1.0 {
                px(902.)
            } else {
                theme::scaled_px(902.)
            };
            let target_h = if cfg!(test) {
                px(610.)
            } else if scale <= 1.0 {
                px(610. + crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
            } else {
                theme::scaled_px(610. + crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
            };
            let (target_w, target_h) = if !cfg!(test)
                && scale > 1.0
                && let Some(display) = window.display(cx)
            {
                let visible = display.visible_bounds().size;
                (target_w.min(visible.width), target_h.min(visible.height))
            } else {
                (target_w, target_h)
            };
            let w = target_w - (frame.width - viewport.width);
            let h = target_h - (frame.height - viewport.height);
            if w > px(0.) && h > px(0.) {
                window.resize(size(w, h));
            }
        }
    }

    pub(in crate::workbench) fn startup_wizard_active(&self) -> bool {
        self.startup_wizard
            .as_ref()
            .is_some_and(|wizard| wizard.active)
    }

    pub(in crate::workbench) fn startup_wizard_picker_pending(&self) -> bool {
        self.startup_wizard
            .as_ref()
            .is_some_and(|wizard| wizard.picker.is_some() || wizard.cancel_confirm_open)
    }

    pub(in crate::workbench) fn startup_wizard_disconnected(&mut self, cx: &mut Context<Self>) {
        let fallback_error = self.error.clone();
        if let Some(wizard) = &mut self.startup_wizard {
            wizard.pending_preference_save = None;
            wizard.pending_recent_save = None;
            wizard.last_start_failed = false;
            if wizard.awaiting_start {
                wizard.awaiting_start = false;
                wizard.step = WizardStep::Step2;
                wizard.error = Some("后台服务已断开，项目启动未完成；当前输入已保留。".into());
            } else if wizard.error.is_none() {
                wizard.error = fallback_error;
            }
            cx.notify();
        }
    }

    pub(in crate::workbench) fn receive_startup_wizard_preferences(
        &mut self,
        _operation_id: Uuid,
        state: &UserPreferencesState,
        cx: &mut Context<Self>,
    ) -> bool {
        if state.load_error.is_none()
            && state.preferences.version == 1
            && self
                .user_preferences
                .confirmed
                .as_ref()
                .is_some_and(|current| state.preferences.revision > current.revision)
        {
            self.user_preferences.confirmed = Some(state.preferences.clone());
            cx.notify();
        }
        false
    }

    pub(in crate::workbench) fn receive_startup_wizard_preferences_saved(
        &mut self,
        operation_id: Uuid,
        result: &Result<UserPreferences, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(wizard) = &mut self.startup_wizard else {
            return false;
        };
        if let Some((pending_op, expected_rev, expected_cfg, expected_disable)) =
            wizard.pending_preference_save.clone()
            && pending_op == operation_id
        {
            wizard.pending_preference_save = None;
            match result {
                Ok(saved)
                    if saved.version == 1
                        && expected_rev.checked_add(1) == Some(saved.revision)
                        && saved.startup.default_config.as_ref() == Some(&expected_cfg)
                        && saved.startup.disable_extensions == expected_disable =>
                {
                    self.user_preferences.confirmed = Some(saved.clone());
                    self.user_preferences.load_error = None;
                }
                Ok(_) => {
                    let message = "默认启动配置回执不匹配；结果未确认。".to_owned();
                    wizard.error = Some(message.clone());
                    self.fail(message, cx);
                    self.query_user_preferences(cx);
                }
                Err(err) => {
                    let message = format!("无法保存默认启动设置：{err}");
                    wizard.error = Some(message.clone());
                    self.fail(message, cx);
                    self.query_user_preferences(cx);
                }
            }
            cx.notify();
            return true;
        }
        let Some(pending) = wizard
            .pending_recent_save
            .as_ref()
            .filter(|pending| pending.operation_id == operation_id)
            .map(|pending| {
                (
                    pending.expected_revision,
                    pending.label,
                    pending.removed_path.clone(),
                    pending.previous_paths.clone(),
                )
            })
        else {
            return false;
        };
        let (expected_rev, action_label, removed_path, previous_paths) = pending;
        wizard.pending_recent_save = None;
        let open_input = wizard.open_file_input.clone();
        match result {
            Ok(saved)
                if saved.version == 1 && expected_rev.checked_add(1) == Some(saved.revision) =>
            {
                self.user_preferences.confirmed = Some(saved.clone());
                self.user_preferences.load_error = None;
                let current_open = open_input.read(cx).value().trim().to_owned();
                if !current_open.is_empty() {
                    let current_path = std::path::Path::new(&current_open);
                    let still_present = saved
                        .recent_projects
                        .iter()
                        .any(|entry| entry.path == current_path);
                    let was_removed = match removed_path.as_deref() {
                        Some(path) => path == current_path && !still_present,
                        None => {
                            previous_paths.iter().any(|p| p == current_path) && !still_present
                        }
                    };
                    if was_removed {
                        let composing = open_input
                            .update(cx, |s, cx| s.marked_text_range(window, cx).is_some());
                        if !composing {
                            if let Some(wizard) = &mut self.startup_wizard {
                                wizard.syncing_inputs = true;
                            }
                            open_input.update(cx, |s, cx| s.set_value("", window, cx));
                            if let Some(wizard) = &mut self.startup_wizard {
                                wizard.syncing_inputs = false;
                            }
                        }
                    }
                }
                self.sync_startup_wizard_preferences(cx);
                self.status = format!("已{action_label}。").into();
            }
            Ok(_) => {
                let message = format!("{action_label}回执不匹配；列表尚未确认。");
                if let Some(wizard) = &mut self.startup_wizard {
                    wizard.error = Some(message.clone());
                }
                self.fail(message, cx);
                self.query_user_preferences(cx);
            }
            Err(err) => {
                let message = format!("无法{action_label}：{err}");
                if let Some(wizard) = &mut self.startup_wizard {
                    wizard.error = Some(message.clone());
                }
                self.fail(message, cx);
                self.query_user_preferences(cx);
            }
        }
        cx.notify();
        true
    }

    pub(in crate::workbench) fn sync_startup_wizard_preferences(&mut self, cx: &mut Context<Self>) {
        let Some(confirmed) = self.user_preferences.confirmed.clone() else {
            return;
        };
        let Some(wizard) = &mut self.startup_wizard else {
            return;
        };
        let table = wizard.recent_table.clone();
        let mode = wizard.mode;
        if !wizard.disable_extensions_customized {
            wizard.disable_extensions = confirmed.startup.disable_extensions;
        }
        if !wizard.config_choice_customized {
            match confirmed.startup.default_config.as_ref() {
                None | Some(StartupDefaultConfig::Project) => {
                    wizard.config_choice = if mode == ProjectMode::OpenExisting {
                        ConfigChoice::Project
                    } else {
                        ConfigChoice::Defaults
                    };
                }
                Some(StartupDefaultConfig::Defaults) => {
                    wizard.config_choice = ConfigChoice::Defaults;
                }
                Some(StartupDefaultConfig::ConfigFile { path }) => {
                    match checked_utf8_path(path, "启动配置文件") {
                        Ok(text) => {
                            wizard.config_choice = ConfigChoice::ConfigFile;
                            let config_input = wizard.config_file_input.clone();
                            if let Some(window) = cx.active_window() {
                                let _ = window.update(cx, |_, window, cx| {
                                    config_input
                                        .update(cx, |state, cx| state.set_value(text, window, cx));
                                });
                            }
                        }
                        Err(err) => {
                            wizard.error = Some(err);
                        }
                    }
                }
            }
        }
        let recent = confirmed.recent_projects;
        table.update(cx, |state, cx| {
            let prev_selected = state
                .selected_row()
                .and_then(|row| state.delegate().rows.get(row).map(|entry| entry.path.clone()))
                .or_else(|| state.delegate().selected_path.clone());
            state.delegate_mut().set_rows(recent);
            state.refresh(cx);
            if let Some(prev_path) = prev_selected {
                if let Some(next_row) = state
                    .delegate()
                    .rows
                    .iter()
                    .position(|entry| entry.path == prev_path)
                {
                    state.delegate_mut().selected_path = Some(prev_path);
                    state.set_selected_row_without_scroll(next_row, cx);
                } else {
                    state.delegate_mut().selected_path = None;
                    state.clear_selection(cx);
                }
            } else {
                state.delegate_mut().selected_path = None;
            }
        });
        cx.notify();
    }

    pub(in crate::workbench) fn startup_wizard_project_opened(
        &mut self,
        path: Option<&PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(wizard) = &mut self.startup_wizard else {
            return false;
        };
        if let Some(cli_path) = wizard.cli_initial_path.take() {
            wizard.initial_project_ready = true;
            wizard.cli_retry_path = Some(cli_path.clone());
            let is_archive = mio_core::is_external_archive_file(&cli_path);
            let options = ProjectStartOptions {
                config_mode: StartupDefaultConfig::Project,
                pause_automated_tasks: !is_archive,
                ..ProjectStartOptions::default()
            };
            let queued = self.enqueue(
                Command::StartProject {
                    path: Some(cli_path),
                    create_new: false,
                    options,
                },
                cx,
            );
            if let Some(wizard) = &mut self.startup_wizard {
                if queued {
                    wizard.step = WizardStep::Starting;
                    wizard.awaiting_start = true;
                    wizard.last_start_failed = false;
                    wizard.error = None;
                } else {
                    wizard.active = true;
                    wizard.step = WizardStep::Step1;
                    wizard.awaiting_start = false;
                    wizard.last_start_failed = true;
                    wizard.error = self
                        .error
                        .clone()
                        .or_else(|| Some("无法启动命令行指定的项目文件。".into()));
                }
            }
            cx.notify();
            return false;
        }
        if wizard.active
            && !wizard.awaiting_start
            && !wizard.initial_project_ready
            && path.is_none()
        {
            wizard.initial_project_ready = true;
            return false;
        }
        if wizard.awaiting_start {
            wizard.awaiting_start = false;
            wizard.active = false;
            wizard.last_start_failed = false;
            wizard.cli_retry_path = None;
            wizard.error = None;
            let frame = window.bounds().size;
            let viewport = window.viewport_size();
            if viewport.width <= px(920.) && viewport.height <= px(680.) {
                let target_h = if cfg!(test) {
                    px(820.)
                } else {
                    px(822. + crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
                };
                let fitted = crate::fit_window_bounds(size(px(1280.), target_h), cx).size;
                window.resize(size(
                    (fitted.width - (frame.width - viewport.width)).max(px(640.)),
                    (fitted.height - (frame.height - viewport.height)).max(px(480.)),
                ));
            }
            return true;
        }
        false
    }

    pub(in crate::workbench) fn startup_initial_project_opening(
        &self,
        path: Option<&PathBuf>,
    ) -> bool {
        self.project.is_none()
            && path.is_none()
            && self.startup_wizard.as_ref().is_some_and(|wizard| {
                wizard.active
                    && !wizard.awaiting_start
                    && !wizard.initial_project_ready
                    && wizard.cli_initial_path.is_none()
            })
    }

    pub(in crate::workbench) fn finish_startup_wizard_after_open(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    pub(in crate::workbench) fn startup_wizard_start_failed(
        &mut self,
        message: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(wizard) = &mut self.startup_wizard
            && (wizard.active || wizard.awaiting_start)
        {
            wizard.awaiting_start = false;
            wizard.active = true;
            if wizard.step == WizardStep::Starting {
                wizard.step = WizardStep::Step1;
            }
            wizard.last_start_failed = true;
            wizard.error = Some(message.to_owned());
            cx.notify();
        }
    }

    pub(in crate::workbench) fn request_startup_wizard_quit(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cx
            .global::<crate::native_terminate::NativeTerminate>()
            .rejected()
        {
            self.reject_quit(cx);
            return;
        }
        self.quit_attempt.get_or_insert_with(Uuid::new_v4);
        if let Some(wizard) = &mut self.startup_wizard {
            wizard.cancel_confirm_open = true;
            cx.notify();
        }
    }
}
