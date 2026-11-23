use mio_core::Uuid;

use super::*;

impl Workbench {
    pub(in crate::workbench) fn on_wizard_new_name_changed(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(wizard) = &mut self.startup_wizard else {
            return;
        };
        if wizard.syncing_inputs || wizard.new_file_customized {
            return;
        }
        let name_input = wizard.new_name_input.clone();
        let file_input = wizard.new_file_input.clone();
        let custom_dir = wizard.new_file_custom_dir.clone();
        let Ok(name) = editing::input_snapshot(&name_input, window, cx) else {
            return;
        };
        if file_input.update(cx, |state, cx| {
            state.marked_text_range(window, cx).is_some()
        }) {
            return;
        }
        let derived = derived_mio_file(&name, custom_dir.as_deref());
        if let Some(wizard) = &mut self.startup_wizard {
            wizard.syncing_inputs = true;
        }
        file_input.update(cx, |state, cx| state.set_value(derived, window, cx));
        if let Some(wizard) = &mut self.startup_wizard {
            wizard.syncing_inputs = false;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn choose_wizard_new_file(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available() || self.file_picker_pending() {
            return;
        }
        let Some(wizard) = &self.startup_wizard else {
            return;
        };
        if wizard.step != WizardStep::Step1 {
            return;
        }
        if wizard.mode != ProjectMode::NewOnDisk {
            self.select_wizard_project_mode(ProjectMode::NewOnDisk, window, cx);
        }
        let Some(wizard) = &mut self.startup_wizard else {
            return;
        };
        let name_input = wizard.new_name_input.clone();
        let file_input = wizard.new_file_input.clone();
        let (Ok(name), Ok(_)) = (
            editing::input_snapshot(&name_input, window, cx),
            editing::input_snapshot(&file_input, window, cx),
        ) else {
            if let Some(wizard) = &mut self.startup_wizard {
                wizard.error = Some(editing::COMPOSITION_PENDING.into());
            }
            cx.notify();
            return;
        };
        let suggested = derived_mio_file(
            if name.trim().is_empty() {
                "Untitled"
            } else {
                name.trim()
            },
            None,
        );
        let picker = Uuid::new_v4();
        if let Some(wizard) = &mut self.startup_wizard {
            wizard.picker = Some(picker);
            wizard.error = None;
        }
        cx.notify();
        let directory = self.wizard_base_directory();
        let prompt =
            crate::native_terminate::prompt_for_new_path(window, cx, &directory, Some(&suggested));
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = owner.update(cx, |_, window, cx| {
                let _ = this.update(cx, |view, cx| {
                    let Some(wizard) = &mut view.startup_wizard else {
                        return;
                    };
                    if wizard.picker != Some(picker) {
                        return;
                    }
                    wizard.picker = None;
                    match result {
                        Ok(Ok(Some(path))) => match checked_utf8_path(&path, "新建项目文件") {
                            Ok(text) => {
                                let parent = path.parent().map(PathBuf::from);
                                wizard.new_file_custom_dir = parent;
                                wizard.new_file_customized = true;
                                wizard.syncing_inputs = true;
                                let input = wizard.new_file_input.clone();
                                input.update(cx, |state, cx| state.set_value(text, window, cx));
                                if let Some(wizard) = &mut view.startup_wizard {
                                    wizard.syncing_inputs = false;
                                }
                            }
                            Err(err) => {
                                wizard.error = Some(err);
                            }
                        },
                        Ok(Ok(None)) => {}
                        Ok(Err(err)) => {
                            wizard.error = Some(format!("无法选择新项目路径：{err}"));
                        }
                        Err(_) => {
                            wizard.error = Some("文件选择器已意外关闭。".into());
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn choose_wizard_open_file(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available() || self.file_picker_pending() {
            return;
        }
        let Some(wizard) = &self.startup_wizard else {
            return;
        };
        if wizard.step != WizardStep::Step1 {
            return;
        }
        if wizard.mode != ProjectMode::OpenExisting {
            self.select_wizard_project_mode(ProjectMode::OpenExisting, window, cx);
        }
        let Some(wizard) = &mut self.startup_wizard else {
            return;
        };
        let open_input = wizard.open_file_input.clone();
        if editing::input_snapshot(&open_input, window, cx).is_err() {
            wizard.error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        }
        let picker = Uuid::new_v4();
        wizard.picker = Some(picker);
        wizard.error = None;
        cx.notify();
        let paths = crate::native_terminate::prompt_for_paths(
            window,
            cx,
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("打开项目".into()),
            },
        );
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = paths.await;
            let _ = owner.update(cx, |_, window, cx| {
                let _ = this.update(cx, |view, cx| {
                    let Some(wizard) = &mut view.startup_wizard else {
                        return;
                    };
                    if wizard.picker != Some(picker) {
                        return;
                    }
                    wizard.picker = None;
                    match result {
                        Ok(Ok(Some(selected))) => {
                            if let Some(path) = selected.into_iter().next() {
                                match checked_utf8_path(&path, "项目文件") {
                                    Ok(text) => {
                                        let input = wizard.open_file_input.clone();
                                        input.update(cx, |state, cx| {
                                            state.set_value(text, window, cx)
                                        });
                                    }
                                    Err(err) => {
                                        wizard.error = Some(err);
                                    }
                                }
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(err)) => {
                            wizard.error = Some(format!("无法选择项目文件：{err}"));
                        }
                        Err(_) => {
                            wizard.error = Some("文件选择器已意外关闭。".into());
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn choose_wizard_config_file(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available() || self.file_picker_pending() {
            return;
        }
        let Some(wizard) = &mut self.startup_wizard else {
            return;
        };
        if wizard.step != WizardStep::Step2 {
            return;
        }
        if wizard.config_choice != ConfigChoice::ConfigFile {
            wizard.config_choice = ConfigChoice::ConfigFile;
            wizard.config_choice_customized = true;
            wizard.last_start_failed = false;
            wizard.error = None;
        }
        let config_input = wizard.config_file_input.clone();
        if editing::input_snapshot(&config_input, window, cx).is_err() {
            wizard.error = Some(editing::COMPOSITION_PENDING.into());
            cx.notify();
            return;
        }
        let picker = Uuid::new_v4();
        wizard.picker = Some(picker);
        wizard.error = None;
        cx.notify();
        let paths = crate::native_terminate::prompt_for_paths(
            window,
            cx,
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("选择配置文件".into()),
            },
        );
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = paths.await;
            let _ = owner.update(cx, |_, window, cx| {
                let _ = this.update(cx, |view, cx| {
                    let Some(wizard) = &mut view.startup_wizard else {
                        return;
                    };
                    if wizard.picker != Some(picker) {
                        return;
                    }
                    wizard.picker = None;
                    match result {
                        Ok(Ok(Some(selected))) => {
                            if let Some(path) = selected.into_iter().next() {
                                match checked_utf8_path(&path, "配置文件") {
                                    Ok(text) => {
                                        let input = wizard.config_file_input.clone();
                                        input.update(cx, |state, cx| {
                                            state.set_value(text, window, cx)
                                        });
                                    }
                                    Err(err) => {
                                        wizard.error = Some(err);
                                    }
                                }
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(err)) => {
                            wizard.error = Some(format!("无法选择配置文件：{err}"));
                        }
                        Err(_) => {
                            wizard.error = Some("文件选择器已意外关闭。".into());
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn validate_wizard_step1(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(Option<String>, Option<PathBuf>), String> {
        let Some(wizard) = &self.startup_wizard else {
            return Err("启动向导不可用。".into());
        };
        let mode = wizard.mode;
        let name_input = wizard.new_name_input.clone();
        let new_file_input = wizard.new_file_input.clone();
        let open_file_input = wizard.open_file_input.clone();
        match mode {
            ProjectMode::Temporary => Ok((None, None)),
            ProjectMode::NewOnDisk => {
                let name = editing::input_snapshot(&name_input, window, cx)
                    .map_err(|err| err.to_owned())?;
                let file = editing::input_snapshot(&new_file_input, window, cx)
                    .map_err(|err| err.to_owned())?;
                let trimmed_name = name.trim();
                let trimmed_file = file.trim();
                if trimmed_name.is_empty() {
                    return Err("请输入新建项目名称（Name）。".into());
                }
                if trimmed_name.chars().any(char::is_control) {
                    return Err("项目名称不能包含控制字符。".into());
                }
                if trimmed_file.is_empty() {
                    return Err("请输入或选择新建项目文件（File）。".into());
                }
                let mut raw_path = PathBuf::from(trimmed_file);
                if raw_path.extension().is_none() {
                    raw_path.set_extension("mio");
                }
                if raw_path
                    .extension()
                    .is_some_and(|ext| !ext.eq_ignore_ascii_case("mio"))
                {
                    return Err("新建磁盘项目文件请使用 .mio 后缀。".into());
                }
                let resolved = if raw_path.is_absolute() {
                    raw_path
                } else {
                    self.wizard_base_directory().join(raw_path)
                };
                Ok((Some(trimmed_name.to_owned()), Some(resolved)))
            }
            ProjectMode::OpenExisting => {
                let file = editing::input_snapshot(&open_file_input, window, cx)
                    .map_err(|err| err.to_owned())?;
                let trimmed_file = file.trim();
                if trimmed_file.is_empty() {
                    return Err("请选择或输入要打开的项目文件（File）。".into());
                }
                let raw_path = PathBuf::from(trimmed_file);
                let resolved = if raw_path.is_absolute() {
                    raw_path
                } else {
                    self.wizard_base_directory().join(raw_path)
                };
                Ok((None, Some(resolved)))
            }
        }
    }

    pub(in crate::workbench) fn wizard_base_directory(&self) -> PathBuf {
        std::env::current_dir()
            .ok()
            .filter(|dir| dir != std::path::Path::new("/"))
            .unwrap_or_else(|| self.default_directory())
    }
}
