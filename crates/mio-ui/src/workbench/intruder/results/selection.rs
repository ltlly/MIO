use mio_core::{IntruderResultOutcome, IntruderRunState};

use super::*;

impl Workbench {
    pub(in crate::workbench) fn refresh_intruder_results(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.backend_disconnected || self.project.is_none() {
            return;
        }
        let page = &self.intruder.results.runs;
        if page.loaded && page.dirty && page.pending.is_none() && page.error.is_none() {
            self.query_intruder_runs(page.after, cx);
        }
        let dirty: Vec<_> = self
            .intruder
            .results
            .views
            .iter()
            .filter_map(|(id, view)| {
                let control = view.stop_requested
                    || view.control_pending.is_some()
                    || self.intruder_result_close_pending(view.run_id);
                (view.dirty
                    && view.pending.is_none()
                    && (control || view.query.pending.is_none() && view.query.error.is_none())
                    && view.queued_query.is_none()
                    && view.error.is_none())
                .then_some((
                    *id,
                    control,
                    view.query.page.as_ref().map_or(0, |page| page.offset),
                ))
            })
            .collect();
        for (id, control, offset) in dirty {
            if control {
                self.query_intruder_results(id, None, window, cx);
            } else {
                let (filter, sort) = self.intruder.results.views[&id].query.requested();
                self.query_intruder_result_view(id, offset, filter, sort, window, cx);
            }
        }

        // Active runs are bounded by core admission. Never scan historical pages to confirm Stop.
        let active: Vec<_> = self.intruder.runs.keys().copied().collect();
        self.intruder.results.probes.retain(|id, probe| {
            probe.pending.is_some()
                || active.contains(id)
                || self.intruder.baseline_requests.contains_key(id)
        });
        for id in active {
            if self
                .intruder
                .results
                .views
                .values()
                .any(|view| view.run_id == id)
            {
                continue;
            }
            let probe = self.intruder.results.probes.entry(id).or_default();
            if probe.dirty && probe.pending.is_none() && probe.error.is_none() {
                self.query_intruder_run_status(id, cx);
            }
        }
    }

    pub(in crate::workbench::intruder) fn intruder_run_status_pending(&self, run_id: Uuid) -> bool {
        self.intruder
            .results
            .probes
            .get(&run_id)
            .is_some_and(|probe| probe.pending.is_some())
    }

    pub(in crate::workbench::intruder) fn query_intruder_run_status(
        &mut self,
        run_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        let Some(revision) = self.next_intruder_query(cx) else {
            return;
        };
        let probe = self.intruder.results.probes.entry(run_id).or_default();
        probe.dirty = false;
        probe.pending = None;
        probe.error = None;
        if self.enqueue(
            Command::QueryIntruderResults {
                project,
                run_id,
                query_revision: revision,
                after: None,
                limit: 1,
            },
            cx,
        ) {
            self.intruder
                .results
                .probes
                .get_mut(&run_id)
                .unwrap()
                .pending = Some(revision);
        } else {
            self.intruder.results.probes.get_mut(&run_id).unwrap().error = self.error.clone();
        }
    }

    pub(in crate::workbench) fn select_intruder_result(
        &mut self,
        view_id: Uuid,
        ordinal: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none()
            || self
                .intruder
                .results
                .views
                .get(&view_id)
                .is_none_or(|view| view.discarded)
        {
            return;
        }
        let Some(revision) = self.next_intruder_query(cx) else {
            return;
        };
        self.intruder
            .results
            .views
            .get_mut(&view_id)
            .unwrap()
            .queued_selection = Some(revision);
        if self.defer_intruder_result_window(view_id, window, cx, move |view, window, cx| {
            view.read_intruder_selection(view_id, ordinal, revision, window, cx);
        }) {
            return;
        }
        self.read_intruder_selection(view_id, ordinal, revision, window, cx);
    }

    pub(in crate::workbench) fn read_intruder_selection(
        &mut self,
        view_id: Uuid,
        ordinal: u64,
        revision: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        let Some(view) = self.intruder.results.views.get_mut(&view_id) else {
            return;
        };
        if view.queued_selection != Some(revision) {
            return;
        }
        view.queued_selection = None;
        let preserved_multi = (view.selected_ordinals.len() > 1
            && view.selected_ordinals.contains(&ordinal))
        .then(|| (view.selected_ordinals.clone(), view.selection_anchor));
        let Some(row) = view.row(ordinal) else {
            if view.query.page.is_some() {
                if let Some(rank) = view.query.rank_of(ordinal) {
                    self.ensure_intruder_result_rank(view_id, rank, window, cx);
                }
                return;
            }
            if view.summary.as_ref().is_some_and(|run| {
                ordinal == run.dispatched
                    && run
                        .baseline
                        .is_some_and(|baseline| baseline.ordinal == ordinal)
            }) {
                view.clear_detail(window, cx);
                view.selected = Some(ordinal);
                if let Some((ordinals, anchor)) = preserved_multi {
                    view.selected_ordinals = ordinals;
                    view.selection_anchor = anchor;
                } else {
                    view.selected_ordinals.insert(ordinal);
                    view.selection_anchor = Some(ordinal);
                }
                return;
            }
            if view
                .summary
                .as_ref()
                .is_none_or(|run| ordinal >= run.dispatched)
            {
                return;
            }
            let after = ordinal.checked_sub(1);
            let reading = view.pending.is_some() && view.after == after;
            view.clear_detail(window, cx);
            if self.backend_disconnected {
                view.detail_error = Some("后台服务已断开，消息读取未确认。".into());
            } else {
                if !reading {
                    self.query_intruder_results(view_id, after, window, cx);
                }
                let view = self.intruder.results.views.get_mut(&view_id).unwrap();
                view.selected = Some(ordinal);
                if let Some((ordinals, anchor)) = preserved_multi {
                    view.selected_ordinals = ordinals;
                    view.selection_anchor = anchor;
                } else {
                    view.selected_ordinals.insert(ordinal);
                    view.selection_anchor = Some(ordinal);
                }
            }
            cx.notify();
            return;
        };
        let outcome = row.outcome.clone();
        if let IntruderResultOutcome::Flow(flow) = &outcome
            && view.selected == Some(ordinal)
            && view.detail_error.is_none()
            && (view.detail.as_ref().is_some_and(|detail| {
                detail.summary.id == flow.id && detail.summary.revision >= flow.revision
            }) || view
                .detail_pending
                .is_some_and(|read| read.flow_id == flow.id && read.flow_revision >= flow.revision))
        {
            if view.selected_ordinals.is_empty() {
                view.selected_ordinals.insert(ordinal);
                view.selection_anchor = Some(ordinal);
            }
            return;
        }
        view.clear_detail(window, cx);
        view.selected = Some(ordinal);
        if let Some((ordinals, anchor)) = preserved_multi {
            view.selected_ordinals = ordinals;
            view.selection_anchor = anchor;
        } else {
            view.selected_ordinals.insert(ordinal);
            view.selection_anchor = Some(ordinal);
        }
        self.refresh_pending_intruder_selection(view_id, window, cx);
        let view = self.intruder.results.views.get_mut(&view_id).unwrap();
        let IntruderResultOutcome::Flow(flow) = outcome else {
            if let IntruderResultOutcome::Rejected(reason) = outcome {
                view.detail_error = Some(format!("未发送：{reason}"));
            }
            cx.notify();
            return;
        };
        let Some(revision) = self.selection_revision.checked_add(1) else {
            view.detail_error = Some("消息选择 revision 已耗尽，请重新打开项目。".into());
            cx.notify();
            return;
        };
        self.selection_revision = revision;
        if self.enqueue(
            Command::ReadFlow {
                project,
                selection_revision: revision,
                flow_id: flow.id,
            },
            cx,
        ) {
            self.intruder
                .results
                .views
                .get_mut(&view_id)
                .unwrap()
                .detail_pending = Some(DetailRead {
                selection_revision: revision,
                flow_id: flow.id,
                flow_revision: flow.revision,
            });
        } else {
            self.intruder
                .results
                .views
                .get_mut(&view_id)
                .unwrap()
                .detail_error = self.error.clone();
        }
        cx.notify();
    }

    pub(in crate::workbench) fn has_intruder_result_read(&self, selection_revision: u64) -> bool {
        self.intruder.results.views.values().any(|view| {
            view.detail_pending
                .is_some_and(|read| read.selection_revision == selection_revision)
        })
    }

    pub(in crate::workbench) fn on_intruder_detail_error(
        &mut self,
        selection_revision: u64,
        flow_id: FlowId,
        message: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(view) = self.intruder.results.views.values_mut().find(|view| {
            view.detail_pending.is_some_and(|read| {
                read.selection_revision == selection_revision && read.flow_id == flow_id
            })
        }) else {
            return false;
        };
        view.detail_pending = None;
        view.detail_error = Some(format!("结果消息读取失败：{message}"));
        cx.notify();
        true
    }

    pub(in crate::workbench::intruder) fn set_intruder_paused(
        &mut self,
        view_id: Uuid,
        paused: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else { return };
        let Some(view) = self.intruder.results.views.get(&view_id) else {
            return;
        };
        let run_id = view.run_id;
        let Some(active) = self.intruder.runs.get(&run_id) else {
            return;
        };
        if self.backend_disconnected
            || self.transition.is_some()
            || self.intruder_result_close_pending(run_id)
            || self.intruder_baseline_pending(run_id)
            || self.intruder.results.views.values().any(|view| {
                view.run_id == run_id && (view.stop_requested || view.control_pending.is_some())
            })
            || active.cancel.is_cancelled()
            || !matches!(
                (&active.summary.state, paused),
                (IntruderRunState::Running, true) | (IntruderRunState::Paused, false)
            )
        {
            return;
        }
        let pending = RunControl {
            operation_id: Uuid::new_v4(),
            paused,
            revision: active.summary.revision,
        };
        let accepted = self.enqueue(
            Command::SetIntruderPaused {
                project,
                run_id,
                operation_id: pending.operation_id,
                paused,
            },
            cx,
        );
        for view in self
            .intruder
            .results
            .views
            .values_mut()
            .filter(|view| view.run_id == run_id)
        {
            view.control_pending = accepted.then_some(pending);
            view.control_error = (!accepted).then(|| {
                format!(
                    "{}命令未入队；运行状态未改变，请明确重试。",
                    if paused { "暂停" } else { "恢复" }
                )
            });
        }
        cx.notify();
    }

    pub(in crate::workbench::intruder) fn remove_intruder_result_rows(
        &mut self,
        view_id: Uuid,
        ordinals: &[u64],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ordinals.is_empty() || self.transition.is_some() {
            return;
        }
        let Some(view) = self.intruder.results.views.get_mut(&view_id) else {
            return;
        };
        if view.discarded {
            return;
        }
        let ord_set: std::collections::BTreeSet<u64> = ordinals.iter().copied().collect();
        view.rows.retain(|row| !ord_set.contains(&row.ordinal));
        let removed = view.query.remove_ordinals(&ord_set);
        view.selected_ordinals.retain(|ord| !ord_set.contains(ord));
        if view
            .selection_anchor
            .is_some_and(|anchor| ord_set.contains(&anchor))
        {
            view.selection_anchor = view.selected_ordinals.iter().next().copied();
        }
        if view.selected.is_some_and(|sel| ord_set.contains(&sel)) {
            let remaining = view.selected_ordinals.clone();
            let remaining_anchor = view.selection_anchor;
            view.clear_detail(window, cx);
            if let Some(&next_ord) = remaining.iter().next() {
                view.selected_ordinals = remaining;
                view.selection_anchor = remaining_anchor;
                self.select_intruder_result(view_id, next_ord, window, cx);
            }
        }
        self.error = None;
        self.status = format!(
            "已从结果列表移除 {} 条攻击结果。",
            removed.max(ordinals.len())
        );
        cx.notify();
    }
}
