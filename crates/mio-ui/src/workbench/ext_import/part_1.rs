use super::*;

impl Workbench {
    pub(in crate::workbench) fn choose_ext_import(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_ext_repeater();
        self.ext_import.visible = true;
        self.ext_import.browser_visible = false;
        self.focus.focus(window, cx);
        if !self.backend_available()
            || self.ext_import.job.is_some()
            || self.file_picker_pending()
            || self.transition.is_some()
        {
            cx.notify();
            return;
        }
        if matches!(self.ext_import.result, Some(ExtJobOutcome::Inspected(_))) {
            self.release_ext_archive_inspection(AfterRelease::ChooseSource, cx);
            return;
        }
        let Some(project) = self.project else { return };
        let picker = Uuid::new_v4();
        self.ext_import.picker = Some(picker);
        let paths = crate::native_terminate::prompt_for_paths(
            window,
            cx,
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("只读导入 外部归档项目".into()),
            },
        );
        cx.spawn_in(window, async move |this, cx| {
            let result = paths.await;
            let _ = this.update_in(cx, |view, _, cx| {
                if view.ext_import.picker != Some(picker) {
                    return;
                }
                view.ext_import.picker = None;
                cx.notify();
                if view.project != Some(project) || !view.backend_available() {
                    return;
                }
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            view.inspect_ext(path, cx);
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => {
                        view.ext_import.error = Some(format!("无法选择 外部归档项目：{error}"));
                    }
                    Err(_) => {
                        view.ext_import.error = Some("文件选择器已意外关闭。".into());
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::workbench) fn inspect_ext(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.ext_import.blocks_transition() {
            return;
        }
        let Some(project) = self.project else { return };
        let job_id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        self.ext_import.source = Some(path.clone());
        self.ext_import.error = None;
        if !self.enqueue(
            Command::InspectExternal {
                project,
                job_id,
                path,
                cancel: cancel.clone(),
            },
            cx,
        ) {
            self.ext_import.error = self.error.clone();
            return;
        }
        self.ext_import.result = None;
        self.ext_import.progress = None;
        self.ext_import.pending_release = None;
        self.ext_import.job = Some(Job {
            project,
            id: job_id,
            kind: JobKind::Inspect,
            cancel: Some(cancel),
            cancelling: false,
        });
        cx.notify();
    }

    pub(in crate::workbench) fn choose_ext_destination(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available()
            || self.ext_import.job.is_some()
            || self.file_picker_pending()
            || self.ext_import.pending_release.is_some()
            || self.transition.is_some()
        {
            return;
        }
        let (Some(project), Some(ExtJobOutcome::Inspected(inspection))) =
            (self.project, &self.ext_import.result)
        else {
            return;
        };
        let inspection_id = inspection.id;
        let filename = PathBuf::from(&inspection.source_name)
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map_or_else(|| "Imported.mio".into(), |name| format!("{name}.mio"));
        let picker = Uuid::new_v4();
        self.ext_import.picker = Some(picker);
        let destination = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &self.default_directory(),
            Some(&filename),
        );
        cx.spawn_in(window, async move |this, cx| {
            let result = destination.await;
            let _ = this.update_in(cx, |view, _, cx| {
                if view.ext_import.picker != Some(picker) {
                    return;
                }
                view.ext_import.picker = None;
                cx.notify();
                if view.project != Some(project) || !view.backend_available() {
                    return;
                }
                match result {
                    Ok(Ok(Some(path))) => view.convert_ext(inspection_id, path, cx),
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => {
                        view.ext_import.error = Some(format!("无法选择 MIO 目标：{error}"));
                    }
                    Err(_) => {
                        view.ext_import.error = Some("文件选择器已意外关闭。".into());
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn convert_ext(&mut self, inspection_id: Uuid, destination: PathBuf, cx: &mut Context<Self>) {
        let Some(project) = self.project else { return };
        if self.ext_import.job.is_some()
            || self.ext_import.pending_release.is_some()
            || !matches!(
                &self.ext_import.result,
                Some(ExtJobOutcome::Inspected(inspection)) if inspection.id == inspection_id
            )
        {
            return;
        }
        let job_id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        if !self.enqueue(
            Command::ConvertExternal {
                project,
                job_id,
                inspection_id,
                destination: destination.clone(),
                cancel: cancel.clone(),
            },
            cx,
        ) {
            self.ext_import.error = self.error.clone();
            return;
        }
        // Conversion consumes the inspection even on failure; retries must inspect again.
        self.ext_import.result = None;
        self.ext_import.progress = None;
        self.ext_import.error = None;
        self.ext_import.job = Some(Job {
            project,
            id: job_id,
            kind: JobKind::Convert { destination },
            cancel: Some(cancel),
            cancelling: false,
        });
        cx.notify();
    }

    pub(in crate::workbench) fn cancel_ext_import(&mut self, cx: &mut Context<Self>) {
        if let Some(job) = &mut self.ext_import.job
            && let Some(cancel) = &job.cancel
        {
            cancel.cancel();
            job.cancelling = true;
            cx.notify();
        }
    }

    pub(in crate::workbench) fn release_ext_archive_inspection(
        &mut self,
        next: AfterRelease,
        cx: &mut Context<Self>,
    ) {
        if self.ext_import.job.is_some() || self.ext_import.picker.is_some() {
            return;
        }
        let (Some(project), Some(ExtJobOutcome::Inspected(inspection))) =
            (self.project, &self.ext_import.result)
        else {
            return;
        };
        let inspection_id = inspection.id;
        let job_id = Uuid::new_v4();
        self.ext_import.pending_release = Some(next);
        if !self.enqueue(
            Command::ReleaseExtInspection {
                project,
                job_id,
                inspection_id,
            },
            cx,
        ) {
            self.ext_import.error = self.error.clone();
            return;
        }
        self.ext_import.result = None;
        self.ext_import.progress = None;
        self.ext_import.error = None;
        self.ext_import.pending_release = None;
        self.ext_import.job = Some(Job {
            project,
            id: job_id,
            kind: JobKind::Release {
                inspection_id,
                next,
            },
            cancel: None,
            cancelling: false,
        });
        cx.notify();
    }

    pub(in crate::workbench) fn dismiss_ext_import(&mut self, cx: &mut Context<Self>) {
        self.cancel_ext_repeater();
        if matches!(self.ext_import.result, Some(ExtJobOutcome::Inspected(_))) {
            self.release_ext_archive_inspection(AfterRelease::Dismiss, cx);
        } else {
            self.ext_import.visible = false;
            cx.notify();
        }
    }

    pub(in crate::workbench) fn show_ext_report(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_ext_repeater();
        self.ext_import.visible = true;
        self.ext_import.browser_visible = false;
        self.focus.focus(window, cx);
        self.query_ext_report(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn query_ext_report(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project else { return };
        if self.ext_import.report_loading {
            return;
        }
        let query_revision = self.ext_import.report_query + 1;
        if !self.enqueue(
            Command::QueryExtReport {
                project,
                query_revision,
            },
            cx,
        ) {
            self.ext_import.report_error = self.error.clone();
            return;
        }
        self.ext_import.report_query = query_revision;
        self.ext_import.report_requested = true;
        self.ext_import.report_loading = true;
        self.ext_import.report_error = None;
    }

    pub(in crate::workbench) fn ext_project_opened(
        &mut self,
        path: Option<&PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if let Some(job) = &self.ext_import.job
            && let Some(cancel) = &job.cancel
        {
            cancel.cancel();
        }
        let created = path.is_some() && self.ext_import.created_path() == path;
        let result = created.then(|| self.ext_import.result.take()).flatten();
        // Only the matching native panel callback releases its token.
        *self.ext_import = ExtImport {
            visible: created,
            picker: self.ext_import.picker,
            result,
            ..Default::default()
        };
        if created {
            self.query_ext_report(cx);
        }
    }

    pub(in crate::workbench) fn ext_backend_disconnected(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ext_repeater_save_failed("后台服务已断开，未载入归档请求。", cx);
        let job = self.ext_import.job.take();
        if let Some(cancel) = job.as_ref().and_then(|job| job.cancel.as_ref()) {
            cancel.cancel();
        }
        if job.is_some() || matches!(self.ext_import.result, Some(ExtJobOutcome::Inspected(_))) {
            if self.ext_import.visible && self.ext_import.browser_visible {
                self.focus.focus(window, cx);
            }
            let target = match job.as_ref().map(|job| &job.kind) {
                Some(JobKind::Convert { destination }) => format!(
                    "请检查目标 {}；不要覆盖或删除可能已发布的项目。",
                    destination.display()
                ),
                _ => String::new(),
            };
            self.ext_import.result = Some(ExtJobOutcome::Failed {
                message: format!("后台服务已断开；导入结果与临时副本清理未确认。{target}"),
                created_path: None,
            });
            self.ext_import.visible = true;
            self.ext_import.browser_visible = false;
        }
        self.ext_import.pending_release = None;
        self.ext_import.progress = None;
        if self.ext_import.report_loading {
            self.ext_import.report_loading = false;
            self.ext_import.report_error = Some("后台服务已断开，查询结果未确认。".into());
        }
        if let Some(messages) = &mut self.ext_import.messages {
            messages.backend_disconnected(cx);
        }
        cx.notify();
    }
}
