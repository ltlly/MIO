use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonVariants;

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

    #[inline(never)]
    pub(in crate::workbench) fn render_ext_import(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.ext_import.browser_visible && self.ext_import.messages.is_some() {
            return self.render_ext_messages(cx);
        }
        let import = &self.ext_import;
        let busy = import.job.is_some() || self.file_picker_pending();
        let mut content = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(16.))
            .child(div().text_size(theme::scaled_px(18.)).child("外部归档项目导入"))
            .child("仅导入 Proxy 历史抓包与 Repeater 标签和历史记录，源 外部归档文件保持只读。");

        if let Some(job) = &import.job {
            let description = if job.cancelling {
                "正在取消；等待后台回收任务与临时文件…"
            } else {
                match job.kind {
                    JobKind::Inspect => "正在只读检查 外部归档项目…",
                    JobKind::Convert { .. } => "正在创建新的 MIO 项目…",
                    JobKind::Release { .. } => "正在释放只读预检副本，等待清理确认…",
                }
            };
            content = content.child(div().child(description));
            if let Some((phase, completed, total)) = &import.progress {
                content = content.child(format!(
                    "{} · 已处理 {}{}",
                    phase_label(*phase),
                    completed,
                    total.map_or_else(String::new, |total| format!(" / {total}")),
                ));
            }
            if job.cancel.is_some() {
                content = content.child(
                    button("ext-cancel", "取消导入")
                        .icon(IconName::Ban)
                        .tooltip("取消正在运行的 External 只读预检或转换任务并回收临时副本")
                        .disabled(job.cancelling)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_ext_import(cx))),
                );
            }
        } else if import.picker.is_some() {
            content = content.child("请在原生文件选择器中选择路径，或取消返回。");
        }

        match &import.result {
            Some(ExtJobOutcome::Inspected(inspection)) => {
                content = content
                    .child(format!("已读取源文件：{}", inspection.source_name))
                    .child(render_report(&inspection.report, "ext-preview-report"));
                if let Some(next) = import.pending_release {
                    content = content.child(
                        button("ext-release-retry", "重试释放预检副本")
                            .icon(IconName::Trash)
                            .tooltip("重试释放并清理本次只读预检生成的临时快照副本")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.release_ext_archive_inspection(next, cx)
                            })),
                    );
                } else {
                    content = content.child(
                        button("ext-convert", "选择新 MIO 目标并转换…")
                            .primary()
                            .icon(IconName::FileInput)
                            .tooltip("选择新建 .mio 项目保存路径并将 Proxy 历史抓包与 Repeater 导入")
                            .disabled(busy || !self.backend_available())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.choose_ext_destination(window, cx)
                            })),
                    );
                }
            }
            Some(ExtJobOutcome::Imported {
                path,
                report,
                total_bytes,
            }) => {
                let opened = self.project_path.as_ref() == Some(path);
                content = content
                    .child(if opened {
                        format!("已创建并打开：{}", path.display())
                    } else {
                        format!("已创建，尚未打开：{}", path.display())
                    })
                    .child(format!("MIO 项目总磁盘占用：{total_bytes} 字节"));
                if !opened {
                    content = content.child(render_report(report, "ext-created-report"));
                    let path = path.clone();
                    content = content
                        .child("当前工作区尚未切换；取消切换或保存失败不会删除已创建的 MIO 项目。")
                        .child(
                            button("ext-open-created", "打开已创建项目")
                                .primary()
                                .icon(IconName::FolderOpen)
                                .tooltip("保存当前工作区并在工作台中打开刚转换生成的 MIO 项目文件")
                                .disabled(self.transition.is_some())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.request_transition(
                                        AfterSave::OpenProject(path.clone()),
                                        window,
                                        cx,
                                    )
                                })),
                        );
                }
            }
            Some(ExtJobOutcome::Cancelled) => {
                content =
                    content.child("导入已取消，后台已确认任务和临时资源回收；当前项目未被替换。");
            }
            Some(ExtJobOutcome::Failed {
                message,
                created_path,
            }) => {
                content = content.child(
                    div()
                        .text_color(theme::danger_fg())
                        .child(format!("导入未完成：{message}")),
                );
                if let Some(path) = created_path {
                    let path = path.clone();
                    content = content
                        .child(format!(
                            "目标已经发布，未自动删除：{}。需打开检查其状态。",
                            path.display()
                        ))
                        .child(
                            button("ext-open-created", "检查并打开已发布目标")
                                .icon(IconName::FolderOpen)
                                .tooltip("切换到已发布的 MIO 目标项目文件以核查导入完整性")
                                .disabled(self.transition.is_some())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.request_transition(
                                        AfterSave::OpenProject(path.clone()),
                                        window,
                                        cx,
                                    )
                                })),
                        );
                }
            }
            Some(ExtJobOutcome::Released { .. }) | None => {}
        }
        if !busy
            && !matches!(import.result, Some(ExtJobOutcome::Inspected(_)))
            && let Some(path) = &import.source
            && (import.error.is_some()
                || matches!(
                    import.result,
                    Some(ExtJobOutcome::Failed { .. } | ExtJobOutcome::Cancelled)
                ))
        {
            let path = path.clone();
            content = content.child(
                button("ext-inspect-retry", "重新只读预检")
                    .icon(IconName::RefreshCw)
                    .tooltip("对上次选择的 .ext_archive 源文件重新执行只读结构与覆盖率预检")
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.inspect_ext(path.clone(), cx)),
                    ),
            );
        }
        if let Some(error) = &import.error {
            content = content.child(div().text_color(theme::danger_fg()).child(error.clone()));
        }

        content = content.child(
            div()
                .border_t_1()
                .border_color(theme::border())
                .pt(px(10.))
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child("当前 MIO 项目的持久导入记录"),
        );
        if import.report_loading {
            content = content.child(
                div()
                    .text_color(theme::muted_foreground())
                    .child("正在读取导入记录…"),
            );
        }
        if let Some(error) = &import.report_error {
            content = content.child(
                div()
                    .text_color(theme::danger_fg())
                    .child(format!("读取导入记录失败：{error}")),
            );
        }
        if let Some(source) = &import.report {
            content = content
                .child(format!("归档来源 ID：{}", source.id))
                .child(render_report(&source.report, "ext-project-report"));
        } else if import.report_requested && !import.report_loading && import.report_error.is_none()
        {
            content = content.child(
                div()
                    .text_color(theme::muted_foreground())
                    .child("当前 MIO 项目没有 外部导入记录。"),
            );
        }
        content = content.child(
            button("ext-report-refresh", "读取 / 刷新导入记录")
                .icon(IconName::RefreshCw)
                .tooltip("从当前 MIO 项目存储中重新查询持久化的 外部导入摘要与覆盖率报告")
                .disabled(import.report_loading || self.project.is_none())
                .on_click(cx.listener(|this, _, _, cx| this.query_ext_report(cx))),
        );

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                toolbar()
                    .child(
                        button("ext-choose-source", "选择 外部源项目…")
                            .icon(IconName::Upload)
                            .tooltip("选择本地 外部归档项目文件进行只读一致性快照与语义覆盖率预检")
                            .disabled(
                                busy || self.transition.is_some() || !self.backend_available(),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.choose_ext_import(window, cx)
                            })),
                    )
                    .when(import.report.is_some(), |this| {
                        this.child(
                            button("ext-browse-items", "浏览全部条目")
                                .icon(IconName::Database)
                                .tooltip("按源集合与原始槽位索引浏览全部归档条目及只读 JSON 元数据")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.show_ext_items(window, cx)
                                })),
                        )
                        .child(
                            button("ext-browse-messages", "浏览已索引消息")
                                .icon(IconName::FileSearch)
                                .tooltip("按集合筛选并按需分段查阅已索引的原始 HTTP 请求与响应字节")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.show_ext_messages(window, cx)
                                })),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        button(
                            "ext-dismiss",
                            if import.job.is_some() {
                                "返回工作台（任务继续）"
                            } else {
                                "返回工作台"
                            },
                        )
                        .icon(IconName::ArrowLeft)
                        .tooltip("关闭 外部导入与归档视图并返回主工作台")
                        .disabled(import.picker.is_some())
                        .on_click(cx.listener(|this, _, _, cx| this.dismiss_ext_import(cx))),
                    ),
            )
            .child(
                div()
                    .id("ext-import-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(content),
            )
            .into_any_element()
    }
}
