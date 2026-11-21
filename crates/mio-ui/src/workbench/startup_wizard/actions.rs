use mio_core::{ProjectStartOptions, StartupDefaultConfig, Uuid};

use super::*;

impl Workbench {
    pub(in crate::workbench) fn wizard_next(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available() || self.file_picker_pending() {
            return;
        }
        match self.validate_wizard_step1(window, cx) {
            Ok(_) => {
                if let Some(wizard) = &mut self.startup_wizard {
                    if wizard.mode != ProjectMode::OpenExisting
                        && wizard.config_choice == ConfigChoice::Project
                    {
                        wizard.config_choice = ConfigChoice::Defaults;
                    }
                    wizard.step = WizardStep::Step2;
                    wizard.last_start_failed = false;
                    wizard.cli_retry_path = None;
                    wizard.error = None;
                }
                cx.notify();
            }
            Err(err) => {
                if let Some(wizard) = &mut self.startup_wizard {
                    wizard.error = Some(err);
                }
                cx.notify();
            }
        }
    }

    pub(in crate::workbench) fn wizard_back(&mut self, cx: &mut Context<Self>) {
        if let Some(wizard) = &mut self.startup_wizard
            && wizard.step == WizardStep::Step2
            && !wizard.cancel_confirm_open
        {
            wizard.step = WizardStep::Step1;
            wizard.error = None;
            cx.notify();
        }
    }

    pub(in crate::workbench) fn wizard_start(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available() || self.file_picker_pending() {
            return;
        }
        if self
            .startup_wizard
            .as_ref()
            .is_some_and(|wizard| wizard.pending_preference_save.is_some())
        {
            if let Some(wizard) = &mut self.startup_wizard {
                wizard.error = Some("默认启动配置仍在保存中；请等待回执后再启动项目。".into());
            }
            cx.notify();
            return;
        }
        let (name, path) = match self.validate_wizard_step1(window, cx) {
            Ok(pair) => pair,
            Err(err) => {
                if let Some(wizard) = &mut self.startup_wizard {
                    wizard.error = Some(err);
                }
                cx.notify();
                return;
            }
        };
        let Some(wizard) = &self.startup_wizard else {
            return;
        };
        let mode = wizard.mode;
        let config_choice = wizard.config_choice;
        let config_input = wizard.config_file_input.clone();
        let default_in_future = wizard.default_in_future;
        let disable_extensions = wizard.disable_extensions;
        let trust_project_file = wizard.trust_project_file;
        let pause_automated_tasks = wizard.pause_automated_tasks;

        let config_mode = match config_choice {
            ConfigChoice::Defaults => StartupDefaultConfig::Defaults,
            ConfigChoice::Project => {
                if mode != ProjectMode::OpenExisting {
                    if let Some(wizard) = &mut self.startup_wizard {
                        wizard.error = Some("仅在打开现有项目时可使用随项目保存的设置。".into());
                    }
                    cx.notify();
                    return;
                }
                StartupDefaultConfig::Project
            }
            ConfigChoice::ConfigFile => {
                let raw = match editing::input_snapshot(&config_input, window, cx) {
                    Ok(val) => val,
                    Err(err) => {
                        if let Some(wizard) = &mut self.startup_wizard {
                            wizard.error = Some(err.to_owned());
                        }
                        cx.notify();
                        return;
                    }
                };
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    if let Some(wizard) = &mut self.startup_wizard {
                        wizard.error = Some("请选择或输入要加载的配置文件路径（File）。".into());
                    }
                    cx.notify();
                    return;
                }
                let cfg_path = PathBuf::from(trimmed);
                let resolved = if cfg_path.is_absolute() {
                    cfg_path
                } else {
                    self.wizard_base_directory().join(cfg_path)
                };
                StartupDefaultConfig::ConfigFile { path: resolved }
            }
        };

        if default_in_future {
            let Some(confirmed) = self.user_preferences.confirmed.as_ref() else {
                if let Some(wizard) = &mut self.startup_wizard {
                    wizard.error = Some(
                        "用户偏好尚未加载完成，无法保存“今后默认使用以上配置”，请稍后重试。".into(),
                    );
                }
                cx.notify();
                return;
            };
            let expected_revision = confirmed.revision;
            let operation_id = Uuid::new_v4();
            if !self.enqueue(
                Command::SetUserStartupPreferences {
                    operation_id,
                    expected_revision,
                    default_config: config_mode.clone(),
                    disable_extensions,
                },
                cx,
            ) {
                if let Some(wizard) = &mut self.startup_wizard {
                    wizard.error = self
                        .error
                        .clone()
                        .or_else(|| Some("默认启动设置命令未入队。".into()));
                }
                cx.notify();
                return;
            }
            if let Some(wizard) = &mut self.startup_wizard {
                wizard.pending_preference_save = Some((
                    operation_id,
                    expected_revision,
                    config_mode.clone(),
                    disable_extensions,
                ));
            }
        }

        let create_new = matches!(mode, ProjectMode::Temporary | ProjectMode::NewOnDisk);

        let options = ProjectStartOptions {
            name,
            config_mode,
            pause_automated_tasks: if mode == ProjectMode::OpenExisting {
                pause_automated_tasks
            } else {
                true
            },
            trust_project_file: if mode == ProjectMode::OpenExisting {
                trust_project_file
            } else {
                true
            },
            disable_extensions,
        };

        if self.enqueue(
            Command::StartProject {
                path,
                create_new,
                options,
            },
            cx,
        ) {
            if let Some(wizard) = &mut self.startup_wizard {
                wizard.step = WizardStep::Starting;
                wizard.awaiting_start = true;
                wizard.last_start_failed = false;
                wizard.cli_retry_path = None;
                wizard.error = None;
            }
            self.status = "正在启动项目，请稍候…".into();
            cx.notify();
        } else if let Some(wizard) = &mut self.startup_wizard {
            wizard.last_start_failed = true;
            wizard.error = self.error.clone();
            cx.notify();
        }
    }

    pub(in crate::workbench) fn wizard_retry(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available() || self.file_picker_pending() {
            return;
        }
        let cli_retry = self
            .startup_wizard
            .as_ref()
            .and_then(|wizard| wizard.cli_retry_path.clone());
        if let Some(cli_path) = cli_retry {
            let is_archive = mio_core::is_external_archive_file(&cli_path);
            let options = ProjectStartOptions {
                config_mode: StartupDefaultConfig::Project,
                pause_automated_tasks: !is_archive,
                ..ProjectStartOptions::default()
            };
            if self.enqueue(
                Command::StartProject {
                    path: Some(cli_path),
                    create_new: false,
                    options,
                },
                cx,
            ) {
                if let Some(wizard) = &mut self.startup_wizard {
                    wizard.step = WizardStep::Starting;
                    wizard.awaiting_start = true;
                    wizard.last_start_failed = false;
                    wizard.error = None;
                }
                self.status = "正在启动项目，请稍候…".into();
                cx.notify();
            } else if let Some(wizard) = &mut self.startup_wizard {
                wizard.last_start_failed = true;
                wizard.error = self
                    .error
                    .clone()
                    .or_else(|| Some("无法重试启动命令行指定的项目文件。".into()));
                cx.notify();
            }
            return;
        }
        self.wizard_start(window, cx);
    }

    pub(in crate::workbench) fn dismiss_wizard_error(&mut self, cx: &mut Context<Self>) {
        if let Some(wizard) = &mut self.startup_wizard {
            wizard.error = None;
            wizard.last_start_failed = false;
            cx.notify();
        }
    }

    pub(in crate::workbench) fn open_wizard_cancel_confirm(&mut self, cx: &mut Context<Self>) {
        self.quit_attempt.get_or_insert_with(Uuid::new_v4);
        if let Some(wizard) = &mut self.startup_wizard {
            wizard.cancel_confirm_open = true;
            cx.notify();
        }
        cx.global::<crate::native_terminate::NativeTerminate>()
            .request();
    }

    pub(in crate::workbench) fn answer_wizard_cancel(
        &mut self,
        confirm_exit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(wizard) = &mut self.startup_wizard {
            wizard.cancel_confirm_open = false;
        }
        if confirm_exit {
            self.quit_attempt.get_or_insert_with(Uuid::new_v4);
            if self.runtime_result.is_some() {
                let _ = self.finish_joined_exit(cx);
                cx.notify();
                return;
            }
            self.begin_shutdown(window, cx);
        } else {
            self.reject_quit(cx);
            cx.notify();
        }
    }

    pub(in crate::workbench) fn on_wizard_new_file_changed(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(wizard) = &mut self.startup_wizard else {
            return;
        };
        if wizard.syncing_inputs {
            return;
        }
        let file_input = wizard.new_file_input.clone();
        let name_input = wizard.new_name_input.clone();
        let custom_dir = wizard.new_file_custom_dir.clone();
        let (Ok(file_text), Ok(name_text)) = (
            editing::input_snapshot(&file_input, window, cx),
            editing::input_snapshot(&name_input, window, cx),
        ) else {
            return;
        };
        let expected = derived_mio_file(&name_text, custom_dir.as_deref());
        if let Some(wizard) = &mut self.startup_wizard
            && file_text.as_ref() != expected
        {
            wizard.new_file_customized = true;
        }
    }

    pub(in crate::workbench) fn on_wizard_select_recent_row(
        &mut self,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(wizard) = &self.startup_wizard else {
            return;
        };
        if wizard.step != WizardStep::Step1 || wizard.cancel_confirm_open {
            return;
        }
        if wizard.mode != ProjectMode::OpenExisting {
            self.select_wizard_project_mode(ProjectMode::OpenExisting, window, cx);
        }
        let Some(wizard) = &mut self.startup_wizard else {
            return;
        };
        let open_input = wizard.open_file_input.clone();
        let table = wizard.recent_table.clone();
        let Some(entry) = table.read(cx).delegate().rows.get(row).cloned() else {
            return;
        };
        table.update(cx, |state, _| {
            state.delegate_mut().selected_path = Some(entry.path.clone());
        });
        if open_input.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }) {
            wizard.error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        }
        let path_str = match checked_utf8_path(&entry.path, "项目文件") {
            Ok(text) => text,
            Err(err) => {
                wizard.error = Some(err);
                cx.notify();
                return;
            }
        };
        open_input.update(cx, |state, cx| state.set_value(path_str, window, cx));
        if let Some(wizard) = &mut self.startup_wizard {
            wizard.last_start_failed = false;
            wizard.cli_retry_path = None;
            wizard.error = None;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn select_wizard_project_mode(
        &mut self,
        mode: ProjectMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(wizard) = &mut self.startup_wizard else {
            return;
        };
        if wizard.step != WizardStep::Step1 || wizard.cancel_confirm_open {
            return;
        }
        wizard.mode = mode;
        wizard.last_start_failed = false;
        wizard.cli_retry_path = None;
        wizard.error = None;
        if !wizard.config_choice_customized {
            let preferred = self
                .user_preferences
                .confirmed
                .as_ref()
                .and_then(|prefs| prefs.startup.default_config.as_ref());
            wizard.config_choice = match (mode, preferred) {
                (_, Some(StartupDefaultConfig::ConfigFile { .. })) => ConfigChoice::ConfigFile,
                (_, Some(StartupDefaultConfig::Defaults)) => ConfigChoice::Defaults,
                (ProjectMode::OpenExisting, _) => ConfigChoice::Project,
                _ => ConfigChoice::Defaults,
            };
        } else if mode != ProjectMode::OpenExisting && wizard.config_choice == ConfigChoice::Project
        {
            wizard.config_choice = ConfigChoice::Defaults;
        }
        let need_prefill = mode == ProjectMode::NewOnDisk;
        let name_input = wizard.new_name_input.clone();
        let file_input = wizard.new_file_input.clone();
        if need_prefill {
            let (Ok(current_name), Ok(current_file)) = (
                editing::input_snapshot(&name_input, window, cx),
                editing::input_snapshot(&file_input, window, cx),
            ) else {
                cx.notify();
                return;
            };
            if current_name.trim().is_empty() && current_file.trim().is_empty() {
                let today = format_today_iso();
                let default_file = format!("{today}.mio");
                if let Some(wizard) = &mut self.startup_wizard {
                    wizard.syncing_inputs = true;
                    wizard.new_file_customized = false;
                    wizard.new_file_custom_dir = None;
                }
                name_input.update(cx, |state, cx| state.set_value(today, window, cx));
                file_input.update(cx, |state, cx| state.set_value(default_file, window, cx));
                if let Some(wizard) = &mut self.startup_wizard {
                    wizard.syncing_inputs = false;
                }
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn remove_selected_wizard_recent(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(wizard) = &self.startup_wizard else {
            return;
        };
        if wizard.step != WizardStep::Step1 || wizard.cancel_confirm_open {
            return;
        }
        let table = wizard.recent_table.clone();
        let selected_path = table.read(cx).selected_row().and_then(|row| {
            table
                .read(cx)
                .delegate()
                .rows
                .get(row)
                .map(|entry| entry.path.clone())
        });
        let Some(path) = selected_path else {
            return;
        };
        self.remove_wizard_recent_path(path, window, cx);
    }

    pub(in crate::workbench) fn remove_wizard_recent_path(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available() || self.file_picker_pending() {
            return;
        }
        if self
            .startup_wizard
            .as_ref()
            .is_some_and(|w| w.pending_recent_save.is_some() || w.step != WizardStep::Step1)
        {
            return;
        }
        let Some(expected_revision) = self
            .user_preferences
            .confirmed
            .as_ref()
            .map(|prefs| prefs.revision)
        else {
            if let Some(wizard) = &mut self.startup_wizard {
                wizard.error = Some("用户偏好尚未加载完成，请稍后重试。".into());
            }
            cx.notify();
            return;
        };
        let open_input = self
            .startup_wizard
            .as_ref()
            .map(|w| w.open_file_input.clone());
        let previous_paths = self
            .user_preferences
            .confirmed
            .as_ref()
            .map(|prefs| {
                prefs
                    .recent_projects
                    .iter()
                    .map(|entry| entry.path.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(ref open_input) = open_input
            && open_input.update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
        {
            if let Some(wizard) = &mut self.startup_wizard {
                wizard.error = Some(editing::COMPOSITION_PENDING.into());
            }
            cx.notify();
            return;
        }
        let operation_id = Uuid::new_v4();
        if !self.enqueue(
            Command::RemoveRecentProject {
                operation_id,
                expected_revision,
                path: path.clone(),
            },
            cx,
        ) {
            if let Some(wizard) = &mut self.startup_wizard {
                wizard.error = self
                    .error
                    .clone()
                    .or_else(|| Some("未能发送移除最近项目命令。".into()));
            }
            cx.notify();
            return;
        }
        if let Some(wizard) = &mut self.startup_wizard {
            wizard.pending_recent_save = Some(PendingRecentSave {
                operation_id,
                expected_revision,
                label: "移除选中最近项目",
                removed_path: Some(path),
                previous_paths,
            });
            wizard.error = None;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn prune_missing_wizard_recent(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available() || self.file_picker_pending() {
            return;
        }
        if self
            .startup_wizard
            .as_ref()
            .is_some_and(|w| w.pending_recent_save.is_some() || w.step != WizardStep::Step1)
        {
            return;
        }
        let Some(confirmed) = self.user_preferences.confirmed.as_ref() else {
            if let Some(wizard) = &mut self.startup_wizard {
                wizard.error = Some("用户偏好尚未加载完成，请稍后重试。".into());
            }
            cx.notify();
            return;
        };
        let expected_revision = confirmed.revision;
        let previous_paths: Vec<PathBuf> = confirmed
            .recent_projects
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        let open_input = self
            .startup_wizard
            .as_ref()
            .map(|w| w.open_file_input.clone());
        if let Some(open_input) = &open_input
            && open_input.update(cx, |s, cx| s.marked_text_range(window, cx).is_some())
        {
            if let Some(wizard) = &mut self.startup_wizard {
                wizard.error = Some(editing::COMPOSITION_PENDING.into());
            }
            cx.notify();
            return;
        }
        let operation_id = Uuid::new_v4();
        if !self.enqueue(
            Command::PruneMissingRecentProjects {
                operation_id,
                expected_revision,
            },
            cx,
        ) {
            if let Some(wizard) = &mut self.startup_wizard {
                wizard.error = self
                    .error
                    .clone()
                    .or_else(|| Some("未能发送清理失效最近项目命令。".into()));
            }
            cx.notify();
            return;
        }
        if let Some(wizard) = &mut self.startup_wizard {
            wizard.pending_recent_save = Some(PendingRecentSave {
                operation_id,
                expected_revision,
                label: "清理失效最近项目",
                removed_path: None,
                previous_paths,
            });
            wizard.error = None;
        }
        cx.notify();
    }
}
