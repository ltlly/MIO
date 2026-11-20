use mio_core::{IntruderRunPage, IntruderRunState};

use super::*;

impl Workbench {
    pub(in crate::workbench) fn on_intruder_result_event(
        &mut self,
        event: &Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let result_window = match event {
            Event::IntruderResultView { query, .. } if self.project == Some(query.project) => self
                .intruder
                .results
                .views
                .get(&query.view_id)
                .filter(|view| {
                    view.run_id == query.run_id
                        && view
                            .query
                            .pending
                            .as_ref()
                            .is_some_and(|pending| &pending.query == query)
                })
                .map(|view| (query.view_id, view.owner)),
            Event::IntruderResults {
                project,
                run_id,
                query_revision,
                ..
            } if self.project == Some(*project) => self
                .intruder
                .results
                .views
                .iter()
                .find(|(_, view)| view.run_id == *run_id && view.pending == Some(*query_revision))
                .map(|(view_id, view)| (*view_id, view.owner)),
            Event::FlowDetail {
                project,
                selection_revision,
                record,
                ..
            } if self.project == Some(*project) => self
                .intruder
                .results
                .views
                .iter()
                .find(|(_, view)| {
                    view.detail_pending.is_some_and(|read| {
                        read.selection_revision == *selection_revision
                            && read.flow_id == record.summary.id
                    })
                })
                .map(|(view_id, view)| (*view_id, view.owner)),
            _ => None,
        };
        if let Some((view_id, owner)) = result_window
            && owner != window.window_handle()
        {
            let event = event.clone();
            self.defer_intruder_result_window(view_id, window, cx, move |view, window, cx| {
                view.on_intruder_result_event(&event, window, cx);
                view.refresh_intruder_results(window, cx);
            });
            return true;
        }
        match event {
            Event::IntruderResultView { query, result } => {
                return self.receive_intruder_result_view(query, result, window, cx);
            }
            Event::IntruderRunControl {
                project,
                run_id,
                operation_id,
                result,
            } if self.project == Some(*project) => {
                let Some(view) = self.intruder.results.views.values().find(|view| {
                    view.run_id == *run_id
                        && view
                            .control_pending
                            .is_some_and(|pending| pending.operation_id == *operation_id)
                }) else {
                    return false;
                };
                let pending = view.control_pending.unwrap();
                let valid = result.as_ref().map_err(Clone::clone).and_then(|summary| {
                    if summary.id != *run_id
                        || summary.revision < pending.revision
                        || view
                            .summary
                            .as_ref()
                            .is_none_or(|before| !same_run(before, summary))
                        || !(summary.state.is_terminal()
                            || matches!(
                                (&summary.state, pending.paused),
                                (IntruderRunState::Paused, true)
                                    | (IntruderRunState::Running, false)
                            ))
                    {
                        Err("Intruder 运行控制确认的身份、revision 或状态不一致。".to_owned())
                    } else {
                        Ok(summary)
                    }
                });
                for view in self.intruder.results.views.values_mut().filter(|view| {
                    view.run_id == *run_id
                        && view
                            .control_pending
                            .is_some_and(|pending| pending.operation_id == *operation_id)
                }) {
                    view.control_pending = None;
                    view.control_error = valid.as_ref().err().cloned();
                    view.dirty = true;
                }
                if let Ok(summary) = valid {
                    self.update_intruder_run_summary(summary);
                }
            }
            Event::IntruderChanged { project } if self.project == Some(*project) => {
                self.intruder.results.runs.dirty = true;
                for view in self.intruder.results.views.values_mut() {
                    view.dirty = true;
                }
                for probe in self.intruder.results.probes.values_mut() {
                    probe.dirty = true;
                }
                self.has_traffic = true;
            }
            Event::IntruderRuns {
                project,
                query_revision,
                result,
            } if self.project == Some(*project)
                && self.intruder.results.runs.pending == Some(*query_revision) =>
            {
                self.intruder.results.runs.pending = None;
                match result {
                    Ok(IntruderRunPage { rows, next_after }) => {
                        let mut rows = rows.clone();
                        for summary in &mut rows {
                            let run_id = summary.id;
                            for before in self
                                .intruder
                                .runs
                                .get(&run_id)
                                .map(|run| &run.summary)
                                .into_iter()
                                .chain(
                                    self.intruder
                                        .results
                                        .views
                                        .values()
                                        .filter(|view| view.run_id == run_id)
                                        .filter_map(|view| view.summary.as_ref()),
                                )
                                .chain(self.intruder.results.runs.rows.iter())
                            {
                                if same_run(before, summary) {
                                    let mut merged = before.clone();
                                    merge_run_summary(&mut merged, summary);
                                    *summary = merged;
                                }
                            }
                            self.update_intruder_run_summary(summary);
                        }
                        let page = &mut self.intruder.results.runs;
                        page.rows = rows;
                        page.next_after = *next_after;
                        page.error = None;
                    }
                    Err(error) => self.intruder.results.runs.error = Some(error.clone()),
                }
            }
            Event::IntruderResults {
                project,
                run_id,
                query_revision,
                result,
            } if self.project == Some(*project) => {
                let view_id = self.intruder.results.views.iter().find_map(|(id, view)| {
                    (view.run_id == *run_id && view.pending == Some(*query_revision)).then_some(*id)
                });
                let probe = self
                    .intruder
                    .results
                    .probes
                    .get(run_id)
                    .is_some_and(|probe| probe.pending == Some(*query_revision));
                if view_id.is_none() && !probe {
                    return false;
                }
                let previous = self
                    .intruder
                    .runs
                    .get(run_id)
                    .map(|run| &run.summary)
                    .into_iter()
                    .chain(
                        self.intruder
                            .results
                            .views
                            .values()
                            .filter(|view| view.run_id == *run_id)
                            .filter_map(|view| view.summary.as_ref()),
                    )
                    .max_by_key(|run| run.revision);
                if result.as_ref().is_ok_and(|page| {
                    previous.is_some_and(|before| {
                        same_run(before, &page.run) && before.revision > page.run.revision
                    })
                }) {
                    // A control ACK can overtake a page deferred to its owning window.
                    if let Ok(page) = result {
                        self.update_intruder_run_summary(&page.run);
                    }
                    if let Some(view_id) = view_id {
                        let view = self.intruder.results.views.get_mut(&view_id).unwrap();
                        view.pending = None;
                        view.dirty = true;
                        view.error = None;
                    }
                    if probe {
                        let probe = self.intruder.results.probes.get_mut(run_id).unwrap();
                        probe.pending = None;
                        probe.dirty = true;
                        probe.error = None;
                    }
                    cx.notify();
                    return true;
                }
                let valid = result.as_ref().map_err(Clone::clone).and_then(|page| {
                    if page.run.id != *run_id
                        || previous.is_some_and(|before| {
                            !same_run(before, &page.run) || before.revision > page.run.revision
                        })
                    {
                        Err("Intruder 结果的运行身份或 revision 不一致，请重试读取。".to_owned())
                    } else {
                        Ok(page)
                    }
                });
                if probe {
                    let probe = self.intruder.results.probes.get_mut(run_id).unwrap();
                    probe.pending = None;
                    probe.error = valid.as_ref().err().cloned();
                }
                if let Some(view_id) = view_id {
                    let mut read_selection = None;
                    let view = self.intruder.results.views.get_mut(&view_id).unwrap();
                    view.pending = None;
                    match &valid {
                        Ok(page) => {
                            view.merge_summary(&page.run);
                            if let Some(baseline) = page.run.baseline
                                && let Some(row) =
                                    page.rows.iter().find(|row| row.ordinal == baseline.ordinal)
                            {
                                view.baseline_row = Some(row.clone());
                            }
                            if view.query.page.is_none() {
                                view.rows.clone_from(&page.rows);
                                view.next_after = page.next_after;
                            }
                            view.persisted = Some(page.persisted);
                            view.error = None;
                            if view
                                .spec
                                .as_ref()
                                .is_some_and(|spec| spec.revision != page.run.spec_revision)
                            {
                                view.spec = None;
                                view.spec_error =
                                    Some("冻结配置 revision 与此运行不一致，请重试读取。".into());
                            }
                            if view.query.page.is_none()
                                && let Some(ordinal) = view.selected
                            {
                                if view.row(ordinal).is_some() {
                                    read_selection = Some(ordinal);
                                } else {
                                    view.clear_detail(window, cx);
                                    if page
                                        .run
                                        .baseline
                                        .is_some_and(|baseline| baseline.ordinal == ordinal)
                                    {
                                        view.selected = Some(ordinal);
                                    }
                                }
                            }
                        }
                        Err(error) => view.error = Some(error.clone()),
                    }
                    if let Some(ordinal) = read_selection {
                        self.select_intruder_result(view_id, ordinal, window, cx);
                    }
                }
                match valid {
                    Ok(page) => {
                        self.confirm_intruder_baseline_stop(&page.run, *query_revision);
                        self.update_intruder_run_summary(&page.run);
                        if let Some(view_id) = view_id
                            && !self.intruder_result_close_pending(*run_id)
                        {
                            let view = self.intruder.results.views.get_mut(&view_id).unwrap();
                            if !view.stop_requested && view.control_pending.is_none() {
                                view.refresh_display_if_changed();
                            }
                        }
                        if !page.persisted
                            && page.run.retention == mio_core::IntruderRetention::Project
                        {
                            self.status =
                                "Intruder Project 结果尚未保存（待写盘）；保存仅重试写盘，不会重发请求。".into();
                        }
                    }
                    Err(error) if probe => {
                        self.fail(format!("Intruder 停止/进度未确认：{error}"), cx)
                    }
                    Err(error) => {
                        if let Some(view_id) = view_id {
                            self.fail_intruder_close_read(view_id, error, cx);
                        }
                    }
                }
            }
            _ => return self.on_intruder_detail_or_annotation_event(event, window, cx),
        }
        self.advance_intruder_result_close(cx);
        cx.notify();
        true
    }
}
