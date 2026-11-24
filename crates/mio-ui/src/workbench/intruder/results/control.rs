use super::*;

impl Workbench {
    pub(in crate::workbench) fn stop_intruder_run(&mut self, run_id: Uuid, cx: &mut Context<Self>) {
        let Some(project) = self.project else { return };
        let baseline_pending = self
            .intruder
            .baseline_requests
            .get_mut(&run_id)
            .is_some_and(|pending| {
                pending.cancel.cancel();
                pending.stop_requested = true;
                true
            });
        let active = self.intruder.runs.get(&run_id);
        let summary = active.map(|run| &run.summary).or_else(|| {
            self.intruder
                .results
                .views
                .values()
                .filter(|view| view.run_id == run_id)
                .filter_map(|view| view.summary.as_ref())
                .max_by_key(|run| run.revision)
        });
        if summary.is_none() && !baseline_pending {
            self.fail("未找到此 Intruder 运行，停止结果未确认。", cx);
            return;
        }
        if summary.is_some_and(|summary| summary.state.is_terminal()) && !baseline_pending {
            return;
        }
        let had_token = active.is_some();
        if let Some(run) = active {
            run.cancel.cancel();
        }
        for view in self
            .intruder
            .results
            .views
            .values_mut()
            .filter(|view| view.run_id == run_id)
        {
            view.stop_requested = true;
            view.control_pending = None;
            view.control_error = None;
            view.dirty = true;
            view.error = None;
        }
        if let Some(probe) = self.intruder.results.probes.get_mut(&run_id) {
            probe.dirty = true;
            probe.error = None;
        }
        self.status = if self.enqueue(Command::StopIntruder { project, run_id }, cx) {
            "已请求停止 Intruder；正在等待终态确认，未自动重发。"
        } else if had_token {
            "Intruder 停止命令未入队；已取消本地发送 token，停止结果仍未确认。"
        } else {
            "Intruder 停止命令未入队，停止结果未确认；请稍后重试。"
        }
        .into();
        for view in self
            .intruder
            .results
            .views
            .values_mut()
            .filter(|view| view.run_id == run_id)
        {
            view.control_error = Some(self.status.clone());
        }
        cx.notify();
    }

    pub(in crate::workbench::intruder) fn update_intruder_run_summary(
        &mut self,
        summary: &IntruderRunSummary,
    ) {
        self.intruder_close_summary_changed(summary);
        let current = self
            .intruder
            .runs
            .get(&summary.id)
            .map(|run| &run.summary)
            .into_iter()
            .chain(
                self.intruder
                    .results
                    .views
                    .values()
                    .filter(|view| view.run_id == summary.id && !view.discarded)
                    .filter_map(|view| view.summary.as_ref()),
            )
            .chain(
                self.intruder
                    .results
                    .runs
                    .rows
                    .iter()
                    .filter(|run| run.id == summary.id),
            )
            .all(|before| same_run(before, summary) && before.revision <= summary.revision);
        if current {
            match summary.retention {
                mio_core::IntruderRetention::Session => {
                    self.intruder.session_runs.insert(summary.id);
                }
                mio_core::IntruderRetention::Project => {
                    self.intruder.session_runs.remove(&summary.id);
                }
            }
        }
        for view in self
            .intruder
            .results
            .views
            .values_mut()
            .filter(|view| view.run_id == summary.id)
        {
            if view.discarded
                || view
                    .summary
                    .as_ref()
                    .is_some_and(|before| !same_run(before, summary))
            {
                continue;
            }
            let previous_retention = view.summary.as_ref().map(|before| before.retention);
            if view.merge_summary(summary) {
                view.dirty = true;
            }
            let merged = view.summary.as_ref().unwrap();
            if previous_retention.is_some_and(|before| before != merged.retention) {
                view.persisted = None;
                view.dirty = true;
                view.error = None;
            }
            if merged.state.is_terminal() {
                view.stop_requested = false;
                view.control_pending = None;
                view.control_error = None;
            }
        }
        for row in &mut self.intruder.results.runs.rows {
            merge_run_summary(row, summary);
        }
        if let Some(active) = self.intruder.runs.get_mut(&summary.id)
            && same_run(&active.summary, summary)
        {
            merge_run_summary(&mut active.summary, summary);
            if active.summary.state.is_terminal() {
                self.intruder.runs.remove(&summary.id);
                if !self.intruder_baseline_pending(summary.id) {
                    self.intruder.results.probes.remove(&summary.id);
                }
            }
        }
    }

    pub(in crate::workbench::intruder) fn invalidate_discarded_intruder_results(
        &mut self,
        run_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        self.intruder.baseline_requests.remove(&run_id);
        self.intruder.pool_changes.remove(&run_id);
        self.intruder.runs.remove(&run_id);
        self.intruder.results.probes.remove(&run_id);
        self.intruder
            .results
            .cached_notes
            .retain(|(id, _), _| *id != run_id);
        let page = &mut self.intruder.results.runs;
        page.rows.retain(|run| run.id != run_id);
        page.pending = None;
        page.dirty = true;
        page.error = None;
        let project = self.project;
        let weak = cx.weak_entity();
        for (view_id, view) in self
            .intruder
            .results
            .views
            .iter_mut()
            .filter(|(_, view)| view.run_id == run_id)
        {
            view.discarded = true;
            view.query.pending = None;
            view.query.page = None;
            view.query.visible_rows.clear();
            view.query.error = Some("此攻击已丢弃，结果不再可用。".into());
            view.pending = None;
            view.queued_query = None;
            view.queued_selection = None;
            view.spec_pending = None;
            view.detail_pending = None;
            view.control_pending = None;
            view.stop_requested = false;
            view.dirty = false;
            view.selected = None;
            view.detail = None;
            view.detail_response_raw = None;
            view.detail_grep_summary = None;
            view.grep_operation = None;
            view.grep_task = None;
            view.spec = None;
            view.spec_error = Some("此攻击已丢弃，冻结配置不再可用。".into());
            view.persisted = None;
            view.next_after = None;
            view.rows.clear();
            view.baseline_row = None;
            view.close_status = None;
            view.error = Some("此攻击已丢弃，结果不再可用。".into());
            if let Some(handle) = view.query.filter_window.take() {
                cx.defer(move |cx| {
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                });
            }
            if let Some(handle) = view.notes_window.take() {
                cx.defer(move |cx| {
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                });
            }
            view.notes_ordinal = None;
            view.notes_outcomes.clear();
            if let Some(project) = project {
                let (view_id, owner, editor_id) =
                    (*view_id, view.owner, view.request.raw.entity_id());
                let weak = weak.clone();
                cx.defer(move |cx| {
                    let _ = owner.update(cx, |_, window, cx| {
                        let _ = weak.update(cx, |workbench, cx| {
                            if workbench
                                .owns_intruder_result_window(view_id, project, owner, editor_id)
                                && workbench.intruder.results.views[&view_id].discarded
                            {
                                workbench
                                    .intruder
                                    .results
                                    .views
                                    .get_mut(&view_id)
                                    .unwrap()
                                    .clear_detail(window, cx);
                                cx.notify();
                            }
                        });
                    });
                });
            }
        }
        self.cancel_invalidated_result_transfer("此攻击结果已丢弃。", cx);
        self.history_loading = false;
        self.history_dirty = true;
        cx.notify();
    }

    pub(in crate::workbench) fn intruder_results_backend_disconnected(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        self.fail_intruder_result_close("后台服务已断开；丢弃未确认，数据和窗口仍保留。", cx);
        let page = &mut self.intruder.results.runs;
        page.dirty = false;
        if page.pending.take().is_some() {
            page.error = Some("后台服务已断开，运行列表读取未确认。".into());
        }
        for (_, pending) in self.intruder.results.pending_highlights.drain() {
            if let Some(view) = self.intruder.results.views.get_mut(&pending.view_id) {
                view.query.error =
                    Some("后台服务已断开，结果高亮修改未确认；当前列表保留。".into());
            }
        }
        for (_, pending) in self.intruder.results.pending_notes.drain() {
            if let Some(view) = self.intruder.results.views.get_mut(&pending.view_id) {
                view.record_notes_outcome(
                    pending.operation_id,
                    Err("后台服务已断开，结果备注修改未确认；草稿保留。".into()),
                );
            }
        }
        for view in self.intruder.results.views.values_mut() {
            if let Some(pending) = view.query.pending.take() {
                let error = "后台服务已断开，结果筛选未确认；当前列表保留。".to_owned();
                view.query.error = Some(error.clone());
                view.query.outcome = Some((pending.query.query_revision, Err(error)));
            }
            view.dirty = false;
            view.queued_selection = None;
            let queued = view.queued_query.take().is_some();
            if view.pending.take().is_some() || queued {
                view.error = Some("后台服务已断开，结果读取未确认。".into());
            }
            if view.detail_pending.take().is_some() {
                view.detail_error = Some("后台服务已断开，消息读取未确认。".into());
            }
            if view.spec_pending.take().is_some() {
                view.spec_error = Some("后台服务已断开，冻结配置读取未确认。".into());
            }
            if view.control_pending.take().is_some() || view.stop_requested {
                view.control_error = Some("后台服务已断开，运行控制未确认；未自动重发。".into());
            }
        }
        self.intruder.results.probes.clear();
        cx.notify();
    }
}
