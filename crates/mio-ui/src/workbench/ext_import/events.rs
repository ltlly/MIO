use super::*;

impl Workbench {
    pub(in crate::workbench) fn on_ext_event(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            Event::ExtProgress {
                project,
                job_id,
                phase,
                completed,
                total,
            } if self.project == Some(project)
                && self
                    .ext_import
                    .job
                    .as_ref()
                    .is_some_and(|job| job.project == project && job.id == job_id) =>
            {
                self.ext_import.progress = Some((phase, completed, total));
            }
            Event::ExtJob {
                project,
                job_id,
                outcome,
            } if self.project == Some(project)
                && self
                    .ext_import
                    .job
                    .as_ref()
                    .is_some_and(|job| job.project == project && job.id == job_id) =>
            {
                let job = self
                    .ext_import
                    .job
                    .take()
                    .expect("matched current import job");
                let cancelled = job.cancelling
                    || job
                        .cancel
                        .as_ref()
                        .is_some_and(CancellationToken::is_cancelled);
                self.ext_import.progress = None;
                self.ext_import.error = None;
                match (&job.kind, &outcome) {
                    (JobKind::Inspect, ExtJobOutcome::Inspected(_)) => {
                        self.ext_import.result = Some(outcome);
                        if cancelled {
                            self.release_ext_archive_inspection(AfterRelease::Cancelled, cx);
                        }
                    }
                    (JobKind::Convert { .. }, ExtJobOutcome::Imported { path, .. }) => {
                        let path = path.clone();
                        self.ext_import.result = Some(outcome);
                        self.ext_import.error = None;
                        self.request_transition(AfterSave::OpenProject(path), window, cx);
                    }
                    (
                        JobKind::Release {
                            inspection_id,
                            next,
                        },
                        ExtJobOutcome::Released {
                            inspection_id: released,
                        },
                    ) if inspection_id == released => match next {
                        AfterRelease::Dismiss => self.ext_import.visible = false,
                        AfterRelease::ChooseSource => self.choose_ext_import(window, cx),
                        AfterRelease::Cancelled => {
                            self.ext_import.result = Some(ExtJobOutcome::Cancelled);
                        }
                    },
                    (_, ExtJobOutcome::Failed { .. })
                    | (JobKind::Inspect | JobKind::Convert { .. }, ExtJobOutcome::Cancelled) => {
                        self.ext_import.result = Some(outcome);
                    }
                    _ => {
                        self.ext_import.error =
                            Some("后台结果与当前导入任务不匹配，仍在等待其终态。".into());
                        self.ext_import.job = Some(job);
                    }
                }
            }
            Event::ExtReport {
                project,
                query_revision,
                result,
            } if self.project == Some(project)
                && query_revision == self.ext_import.report_query =>
            {
                self.ext_import.report_loading = false;
                match result {
                    Ok(report) => {
                        if self.ext_import.report.as_ref().map(|source| source.id)
                            != report.as_ref().map(|source| source.id)
                        {
                            if self.ext_import.visible && self.ext_import.browser_visible {
                                self.focus.focus(window, cx);
                            }
                            self.ext_import.messages = None;
                            self.cancel_ext_repeater();
                            self.ext_import.browser_visible = false;
                        }
                        self.ext_import.report = report;
                        self.ext_import.report_error = None;
                    }
                    Err(message) => self.ext_import.report_error = Some(message),
                }
            }
            event @ (Event::ExtItems { .. }
            | Event::ExtItem { .. }
            | Event::ExtMessages { .. }
            | Event::ExtMessage { .. }) => {
                self.on_ext_message_event(event, window, cx);
            }
            event @ Event::ExtRepeaterOpened { .. } => {
                self.on_ext_repeater_event(event, window, cx);
            }
            _ => {}
        }
        cx.notify();
    }
}
