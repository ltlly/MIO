use super::*;

impl Workbench {
    pub(in crate::workbench) fn open_new_window(&mut self, cx: &mut Context<Self>) {
        match crate::multi_window::WindowLauncher::launch(None, cx) {
            Ok(()) => {
                self.error = None;
                self.status = "已打开新的 MIO 窗口。".into();
                self.record_footer_event(
                    FooterLogSeverity::Info,
                    "已打开新的 MIO 窗口",
                    "新窗口已作为独立 MIO 工作台实例启动。",
                );
                cx.notify();
            }
            Err(error) => {
                let message = format!("无法打开新的 MIO 窗口：{error}");
                if let Some(wizard) = &mut self.startup_wizard
                    && wizard.active
                {
                    wizard.error = Some(message.clone());
                }
                self.fail(message, cx);
            }
        }
    }

    pub(in crate::workbench) fn open_project_path_in_new_window(
        &mut self,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let display_path = path.display().to_string();
        match crate::multi_window::WindowLauncher::launch(Some(&path), cx) {
            Ok(()) => {
                self.error = None;
                self.status = format!("已在新窗口中打开项目：{display_path}");
                self.record_footer_event(
                    FooterLogSeverity::Info,
                    format!("已在新窗口中打开项目：{display_path}"),
                    display_path,
                );
                cx.notify();
            }
            Err(error) => {
                let message = format!("无法在新窗口中打开项目：{error}");
                if let Some(wizard) = &mut self.startup_wizard
                    && wizard.active
                {
                    wizard.error = Some(message.clone());
                }
                self.fail(message, cx);
            }
        }
    }

    pub(in crate::workbench) fn choose_open_in_new_window(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_picker_pending() {
            return;
        }
        let picker = mio_core::Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let paths = crate::native_terminate::prompt_for_paths(
            window,
            cx,
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("在新窗口中打开 MIO 项目".into()),
            },
        );
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = paths.await;
            let _ = owner.update(cx, |_, _, cx| {
                this.update(cx, |view, cx| {
                    if !view.finish_file_picker(picker, cx) {
                        return;
                    }
                    match result {
                        Ok(Ok(Some(selected))) => {
                            if let Some(path) = selected.into_iter().next() {
                                view.open_project_path_in_new_window(path, cx);
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(error)) => {
                            let msg = format!("无法选择要在新窗口打开的项目：{error}");
                            if let Some(wizard) = &mut view.startup_wizard
                                && wizard.active
                            {
                                wizard.error = Some(msg.clone());
                            }
                            view.fail(msg, cx);
                        }
                        Err(_) => {
                            let msg = "文件选择器已意外关闭。".to_owned();
                            if let Some(wizard) = &mut view.startup_wizard
                                && wizard.active
                            {
                                wizard.error = Some(msg.clone());
                            }
                            view.fail(msg, cx);
                        }
                    }
                })
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn default_directory(&self) -> PathBuf {
        self.project_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Documents")))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default()
    }

    pub(in crate::workbench) fn file_picker_pending(&self) -> bool {
        self.file_picker.is_some()
            || self.startup_wizard_picker_pending()
            || self.tab_group_dialog.is_some()
            || self.choosing_save_as
            || self.ext_import.picker_pending()
            || self.intruder_close_blocks_transition()
            || self.intruder_task_name_pending()
    }

    pub(in crate::workbench) fn finish_file_picker(
        &mut self,
        picker: mio_core::Uuid,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.file_picker != Some(picker) {
            return false;
        }
        self.file_picker = None;
        cx.notify();
        true
    }

    pub(in crate::workbench) fn spawn_new_path_picker_raw(
        &mut self,
        suggested_name: Option<&str>,
        allow_test_override: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_result: impl FnOnce(
            &mut Self,
            Result<anyhow::Result<Option<PathBuf>>, ()>,
            &mut Window,
            &mut Context<Self>,
        ) + 'static,
    ) {
        let picker = mio_core::Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        #[cfg(not(test))]
        let rx = {
            let _ = allow_test_override;
            crate::native_terminate::prompt_for_new_path(
                window,
                cx,
                &self.default_directory(),
                suggested_name,
            )
        };
        #[cfg(test)]
        let rx = if allow_test_override && let Some(path) = self.save_picker_override.take() {
            path
        } else {
            crate::native_terminate::prompt_for_new_path(
                window,
                cx,
                &self.default_directory(),
                suggested_name,
            )
        };
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = rx.await.map_err(|_| ());
            let _ = owner.update(cx, |_, window, cx| {
                this.update(cx, |view, cx| {
                    if !view.finish_file_picker(picker, cx) {
                        return;
                    }
                    on_result(view, result, window, cx);
                })
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::workbench) fn spawn_save_path_picker(
        &mut self,
        suggested_name: Option<&str>,
        allow_test_override: bool,
        project_switched_error: &'static str,
        select_error_prefix: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_selected: impl FnOnce(&mut Self, PathBuf, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let project = self.project;
        self.spawn_new_path_picker_raw(
            suggested_name,
            allow_test_override,
            window,
            cx,
            move |view, result, window, cx| {
                if !view.backend_available() {
                    return;
                }
                if view.project != project {
                    view.fail(project_switched_error, cx);
                    return;
                }
                match result {
                    Ok(Ok(Some(path))) => on_selected(view, path, window, cx),
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => view.fail(format!("{select_error_prefix}：{error}"), cx),
                    Err(()) => view.fail("文件选择器已意外关闭。", cx),
                }
            },
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::workbench) fn spawn_export_file_picker(
        &mut self,
        suggested_name: Option<&str>,
        default_ext: Option<&'static str>,
        project_switched_error: &'static str,
        select_error_prefix: &'static str,
        payload: Vec<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_written: impl FnOnce(&mut Self, PathBuf, std::io::Result<()>, &mut Context<Self>) + 'static,
    ) {
        let project = self.project;
        let picker = mio_core::Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &self.default_directory(),
            suggested_name,
        );
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let pending_path = owner
                .update(cx, |_, _, cx| {
                    this.update(cx, |view, cx| {
                        if !view.finish_file_picker(picker, cx) || !view.backend_available() {
                            return None;
                        }
                        if view.project != project {
                            view.fail(project_switched_error, cx);
                            return None;
                        }
                        match result {
                            Ok(Ok(Some(mut path))) => {
                                if let Some(ext) = default_ext
                                    && path.extension().is_none()
                                {
                                    path.set_extension(ext);
                                }
                                Some(path)
                            }
                            Ok(Ok(None)) => None,
                            Ok(Err(error)) => {
                                view.fail(format!("{select_error_prefix}：{error}"), cx);
                                None
                            }
                            Err(_) => {
                                view.fail("文件选择器已意外关闭。", cx);
                                None
                            }
                        }
                    })
                })
                .ok()
                .and_then(Result::ok)
                .flatten();
            let Some(path) = pending_path else {
                return;
            };
            let write_path = path.clone();
            let write_result = cx
                .background_executor()
                .spawn(async move {
                    mio_core::ui_files::write_export_file(&write_path, &payload)
                })
                .await;
            let _ = owner.update(cx, |_, _, cx| {
                this.update(cx, |view, cx| {
                    if view.project != project {
                        return;
                    }
                    on_written(view, path, write_result, cx);
                })
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn spawn_open_path_picker(
        &mut self,
        options: PathPromptOptions,
        project_switched_error: &'static str,
        select_error_prefix: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_selected: impl FnOnce(&mut Self, PathBuf, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let project = self.project;
        let picker = mio_core::Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let paths = crate::native_terminate::prompt_for_paths(window, cx, options);
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = paths.await;
            let _ = owner.update(cx, |_, window, cx| {
                this.update(cx, |view, cx| {
                    if !view.finish_file_picker(picker, cx) || !view.backend_available() {
                        return;
                    }
                    if view.project != project {
                        view.fail(project_switched_error, cx);
                        return;
                    }
                    match result {
                        Ok(Ok(Some(paths))) => {
                            if let Some(path) = paths.into_iter().next() {
                                on_selected(view, path, window, cx);
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(error)) => view.fail(format!("{select_error_prefix}：{error}"), cx),
                        Err(_) => view.fail("文件选择器已意外关闭。", cx),
                    }
                })
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn choose_open(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.file_picker_pending() || !self.backend_available() {
            return;
        }
        if self.ext_import.blocks_transition() {
            self.ext_import.visible = true;
            self.fail(
                "请先取消导入或关闭预检，并等待后台清理完成后再打开其他项目。",
                cx,
            );
            return;
        }
        self.spawn_open_path_picker(
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("打开 MIO 项目".into()),
            },
            "项目已切换，本次文件选择未应用到新项目。",
            "无法选择项目",
            window,
            cx,
            |view, path, window, cx| {
                view.request_transition(AfterSave::OpenProject(path), window, cx);
            },
        );
    }

    pub(in crate::workbench) fn save_project(
        &mut self,
        save_as: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.intruder.resource_pools.management.delete_pending() {
            self.fail("资源池删除尚未确认；当前编辑保留，确认后可保存。", cx);
            return;
        }
        if self.cancel_expired_quit(cx) {
            return;
        }
        if !self.backend_available() {
            if self.after_save.is_some() {
                self.cancel_transition(cx);
            }
            return;
        }
        if self
            .transition
            .as_ref()
            .is_some_and(|phase| *phase != TransitionPhase::Ready)
            || self.file_picker_pending()
        {
            return;
        }
        if self.websocket_notes_pending()
            || self.http_notes_pending()
            || self.http_highlights_pending()
        {
            self.send_websocket_notes(false, cx);
            self.send_http_notes(false, cx);
            self.send_http_highlights(false, cx);
            if self.websocket_notes_pending()
                || self.http_notes_pending()
                || self.http_highlights_pending()
            {
                self.notes_save_wait = Some(save_as);
                self.status = "正在等待注解保存确认；随后保存最新项目。".into();
                cx.notify();
                return;
            }
        }
        self.notes_save_wait = None;
        self.begin_project_save(save_as, window, cx);
    }

    pub(in crate::workbench) fn begin_project_save(
        &mut self,
        save_as: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        if !save_as && self.project_path.is_some() {
            if !self.flush_workspace(window, cx) {
                self.fail_intruder_result_close("项目工作区未保存；结果和当前编辑仍保留。", cx);
                self.cancel_transition(cx);
                return;
            }
            self.pending_project_save = Some(None);
            self.flush_project_save(window, cx);
            return;
        }
        match self.capture_drafts(window, cx) {
            Ok(changed) => self.workspace_dirty |= changed,
            Err(error) => {
                self.cancel_save_as(cx);
                self.fail(error, cx);
                return;
            }
        }
        // Save As can escape an unwritable source; it must not require another source write.
        self.choosing_save_as = true;
        let for_transition = self.after_save.is_some();
        let attempt = self.quit_attempt;
        let result_close = self.intruder_close_save_operation();
        self.spawn_new_path_picker_raw(
            Some("Untitled.mio"),
            true,
            window,
            cx,
            move |view, result, window, cx| {
                view.choosing_save_as = false;
                if !view.intruder_close_save_is_current(result_close) {
                    view.cancel_save_as(cx);
                    view.fail(
                        "结果关闭等待已结束；本次路径未用于保存，当前结果仍保留。",
                        cx,
                    );
                    return;
                }
                if view.project != Some(project) {
                    view.fail("项目已切换，本次文件选择未应用到新项目。", cx);
                    return;
                }
                if for_transition
                    && (view.after_save.is_none() || !view.quit_answer_is_current(attempt, cx))
                {
                    view.cancel_save_as(cx);
                    return;
                }
                match result {
                    Ok(Ok(Some(path))) => match view.capture_drafts(window, cx) {
                        Ok(changed) => {
                            if attempt.is_some() {
                                cx.global::<crate::native_terminate::NativeTerminate>()
                                    .extend_deadline_for_user_interaction();
                            }
                            view.workspace_dirty |= changed;
                            view.pending_project_save = Some(Some(path));
                            view.flush_project_save(window, cx);
                        }
                        Err(error) => {
                            view.cancel_save_as(cx);
                            view.fail(error, cx);
                        }
                    },
                    Ok(Ok(None)) => {
                        view.cancel_save_as(cx);
                    }
                    Ok(Err(error)) => {
                        view.cancel_save_as(cx);
                        view.fail(format!("无法选择保存位置：{error}"), cx);
                    }
                    Err(()) => {
                        view.cancel_save_as(cx);
                        view.fail("文件选择器已意外关闭。", cx);
                    }
                }
            },
        );
    }

    pub(in crate::workbench) fn cancel_save_as(&mut self, cx: &mut Context<Self>) {
        self.notes_save_wait = None;
        if self.intruder_close_save_operation().is_some() {
            self.fail_intruder_result_close(
                "项目文件保存未完成；结果和最新输入仍保留，请再次明确选择。",
                cx,
            );
        }
        if self.transition.is_some() {
            self.cancel_transition(cx);
        } else if self.discard_pending_sends(cx) {
            self.status = "另存为未完成；等待保存的请求已取消，尚未建立网络连接。".into();
        }
        self.send_http_notes(false, cx);
        self.send_http_highlights(false, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn choose_slim_save_as(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available() || self.transition.is_some() || self.file_picker_pending() {
            return;
        }
        let Some(project) = self.project else { return };
        if !self.flush_workspace(window, cx) {
            return;
        }
        let default_name = self
            .project_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|stem| format!("{}-slim.mio", stem.to_string_lossy()))
            .unwrap_or_else(|| "Untitled-slim.mio".into());
        self.spawn_save_path_picker(
            Some(&default_name),
            true,
            "项目已切换，本次文件选择未应用到新项目。",
            "无法选择瘦身保存位置",
            window,
            cx,
            move |view, path, _, cx| {
                view.status = format!("正在执行瘦身另存为：{}…", path.display());
                view.enqueue(
                    Command::SlimSaveProjectAs {
                        project,
                        path,
                        options: mio_core::SlimSaveOptions::default(),
                        cancel: CancellationToken::new(),
                    },
                    cx,
                );
            },
        );
    }

    pub(in crate::workbench) fn export_ca(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.backend_available() || self.transition.is_some() || self.file_picker_pending() {
            return;
        }
        self.spawn_save_path_picker(
            Some("mio-ca.cer"),
            false,
            "项目已切换，本次文件选择未应用到新项目。",
            "无法导出 CA",
            window,
            cx,
            |view, path, _, cx| {
                view.enqueue(Command::ExportCa { path }, cx);
            },
        );
    }
}
