use mio_core::Uuid;

use super::*;

impl Workbench {
    pub(in crate::workbench) fn start_intruder(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.backend_disconnected
            || self.transition.is_some()
            || self.choosing_save_as
            || self.pending_project_save.is_some()
            || self.saving_project_snapshot
        {
            self.fail("项目正在保存、切换或后台已断开；Intruder 未开始。", cx);
            return;
        }
        let (Some(project), Some(id)) = (self.project, self.workspace.intruder.active) else {
            return;
        };
        if self.intruder.starts.contains_key(&id) {
            return;
        }
        if self.pending_records > 0 {
            self.fail(
                "仍有流量尚未写入项目，请先保存或另存为恢复存储；Intruder 未开始。",
                cx,
            );
            return;
        }
        match self.capture_drafts(window, cx) {
            Ok(changed) => self.workspace_dirty |= changed,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        }
        let Some(tab) = self.workspace.intruder.tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        match tab.spec.validate() {
            Ok(total) if total == u64::from(tab.spec.baseline_request) => {
                self.fail("Intruder payload 列表为空，无法生成请求；未开始。", cx);
                return;
            }
            Ok(_) => {}
            Err(error) => {
                self.fail(
                    format!("Intruder 配置无效：{error}；输入仍保留，未开始。"),
                    cx,
                );
                return;
            }
        }
        let pending = PendingStart {
            project,
            run_id: Uuid::new_v4(),
            spec: tab.spec.clone(),
            // An ACK already in flight before Start cannot authorize this new action.
            after_revision: self.pending_workspace.unwrap_or(self.workspace.revision),
            phase: StartPhase::Saving,
            cancel: self.shutdown.child_token(),
        };
        self.intruder.starts.insert(id, pending);
        if self.save_workspace(cx) {
            self.status = "正在保存 Intruder 配置；保存确认后开始。".into();
        } else if let Some(pending) = self.intruder.starts.remove(&id) {
            pending.cancel.cancel();
            self.status = "Intruder 配置未保存，未开始；请明确重试。".into();
        }
        cx.notify();
    }

    pub(in crate::workbench) fn intruder_workspace_acknowledged(
        &mut self,
        saved: &Workspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let waiting: Vec<_> = self
            .intruder
            .starts
            .iter()
            .filter_map(|(id, pending)| {
                (pending.phase == StartPhase::Saving && saved.revision > pending.after_revision)
                    .then_some(*id)
            })
            .collect();
        for id in waiting {
            let pending = self
                .intruder
                .starts
                .remove(&id)
                .expect("collected pending start");
            let current = self.snapshot_intruder_spec(id, window, cx);
            let matches = self.project == Some(pending.project)
                && !self.backend_disconnected
                && self.transition.is_none()
                && !self.choosing_save_as
                && self.pending_project_save.is_none()
                && !self.saving_project_snapshot
                && self.pending_records == 0
                && !pending.cancel.is_cancelled()
                && current.as_ref() == Ok(&pending.spec)
                && saved
                    .intruder
                    .tabs
                    .iter()
                    .any(|tab| tab.id == id && tab.spec == pending.spec);
            if !matches {
                pending.cancel.cancel();
                if let Some(editors) = self.intruder.editors.get_mut(&id) {
                    editors.dirty = true;
                }
                self.fail(current.err().unwrap_or_else(|| {
                    "等待保存时 Intruder 输入、标签或项目已改变；未开始，请再次点击「开始攻击」。".into()
                }), cx);
                continue;
            }
            if self.enqueue(
                Command::StartIntruder {
                    project: pending.project,
                    run_id: pending.run_id,
                    tab_id: id,
                    spec: Box::new(pending.spec.clone()),
                    cancel: pending.cancel.clone(),
                },
                cx,
            ) {
                self.intruder.starts.insert(
                    id,
                    PendingStart {
                        phase: StartPhase::Starting,
                        ..pending
                    },
                );
                self.status = "Intruder 正在确认开始…".into();
            } else {
                pending.cancel.cancel();
                self.status = "Intruder 未开始；后台队列未接受请求，未自动重试。".into();
            }
        }
    }

    pub(in crate::workbench) fn intruder_workspace_rejected(
        &mut self,
        stored: &Workspace,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        self.intruder_save_failed(cx);
        let mut error = None;
        for tab in self
            .workspace
            .intruder
            .tabs
            .iter_mut()
            .chain(&mut self.workspace.intruder.closed)
        {
            if let Some(previous) = stored
                .intruder
                .tabs
                .iter()
                .chain(&stored.intruder.closed)
                .find(|previous| previous.id == tab.id)
            {
                match rebased_spec(&tab.spec, &previous.spec) {
                    Ok(next) => tab.spec = next,
                    Err(failure) => {
                        error.get_or_insert(failure);
                    }
                }
            }
        }
        error
    }

    pub(in crate::workbench) fn on_intruder_event(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Event::IntruderStarted {
            project,
            run_id,
            tab_id,
            spec_revision,
            result,
        } = event
        {
            if self.project != Some(project)
                || !self.intruder.starts.get(&tab_id).is_some_and(|pending| {
                    pending.phase == StartPhase::Starting
                        && pending.project == project
                        && pending.run_id == run_id
                        && pending.spec.revision == spec_revision
                })
            {
                return;
            }
            let pending = self
                .intruder
                .starts
                .remove(&tab_id)
                .expect("matched pending start");
            match result {
                Ok(summary)
                    if summary.id == run_id
                        && summary.tab_id == tab_id
                        && summary.spec_revision == spec_revision
                        && summary.baseline
                            == pending.spec.baseline_request.then_some(
                                mio_core::IntruderBaseline {
                                    ordinal: 0,
                                    completed: false,
                                },
                            )
                        && summary.payload_progress().is_some()
                        && summary.resource_pool
                            == pending
                                .spec
                                .resource_pool
                                .as_ref()
                                .map(mio_core::IntruderPoolSelection::id)
                        && summary.resource_pool_revision <= i64::MAX as u64
                        && (summary.resource_pool.is_some()
                            == (summary.resource_pool_revision > 0))
                        && summary.retention
                            == if pending.spec.save_to_project {
                                mio_core::IntruderRetention::Project
                            } else {
                                mio_core::IntruderRetention::Session
                            } =>
                {
                    let project_backed = summary.retention == mio_core::IntruderRetention::Project;
                    let created_pool = match &pending.spec.resource_pool {
                        Some(mio_core::IntruderPoolSelection::Create(pool)) => Some(pool),
                        _ => None,
                    };
                    let next_pool = created_pool.and_then(|created| {
                        if self.intruder.editors.get(&tab_id).is_some_and(|editors| {
                            !editors.pool_form.creating || editors.pool_form.id != created.id
                        }) {
                            return None;
                        }
                        let tab = self
                            .workspace
                            .intruder
                            .tabs
                            .iter()
                            .chain(&self.workspace.intruder.closed)
                            .find(|tab| tab.id == tab_id)?;
                        let Some(mio_core::IntruderPoolSelection::Create(current)) =
                            &tab.spec.resource_pool
                        else {
                            return None;
                        };
                        if current.id != created.id {
                            return None;
                        }
                        let snapshot = match self.intruder.editors.get(&tab_id) {
                            Some(editors) if editors.pool_form.has_composition(window, cx) => None,
                            Some(editors) => editors.pool_form.snapshot(window, cx).ok(),
                            None => Some(current.clone()),
                        };
                        Some(if snapshot.as_ref() == Some(created) {
                            mio_core::IntruderPoolSelection::Existing(created.id)
                        } else {
                            let mut pool = snapshot.unwrap_or_else(|| current.clone());
                            pool.id = Uuid::new_v4();
                            mio_core::IntruderPoolSelection::Create(pool)
                        })
                    });
                    // A Session run must block project exit before its first result page arrives.
                    self.update_intruder_run_summary(&summary);
                    self.intruder.runs.insert(
                        run_id,
                        IntruderRun {
                            summary,
                            cancel: pending.cancel,
                        },
                    );
                    let mut pool_error = None;
                    let mut pool_applied = false;
                    if let Some(tab) = self
                        .workspace
                        .intruder
                        .tabs
                        .iter_mut()
                        .chain(&mut self.workspace.intruder.closed)
                        .find(|tab| tab.id == tab_id)
                    {
                        tab.last_run = project_backed.then_some(run_id);
                        if let Some(selection) = &next_pool {
                            let mut next = tab.spec.clone();
                            next.resource_pool = Some(selection.clone());
                            match revised_spec(&tab.spec, next) {
                                Ok(next) => {
                                    tab.spec = next;
                                    pool_applied = true;
                                }
                                Err(error) => pool_error = Some(error),
                            }
                        }
                        self.save_workspace(cx);
                    }
                    if pool_applied
                        && let Some(created) = created_pool
                        && let Some(editors) = self.intruder.editors.get_mut(&tab_id)
                    {
                        editors.pool_form.existing = Some(created.id);
                        editors.pool_form.creating =
                            matches!(next_pool, Some(mio_core::IntruderPoolSelection::Create(_)));
                        if let Some(mio_core::IntruderPoolSelection::Create(pool)) = &next_pool {
                            editors.pool_form.id = pool.id;
                            if pool.retained_inputs.is_some() {
                                editors.pool_form.accept_retained_inputs();
                            }
                        }
                    }
                    if pending.spec.resource_pool.is_some() {
                        self.query_intruder_resource_pools(cx);
                    }
                    if let Some(error) = pool_error {
                        self.fail(error, cx);
                    }
                    self.open_intruder_results_window(run_id, cx);
                    self.status = if self.intruder.runs[&run_id].cancel.is_cancelled() {
                        "Intruder 已确认开始并已请求取消；停止结果尚未确认。"
                    } else if !project_backed {
                        "Intruder 已开始，攻击结果仅保留在当前会话。"
                    } else if self.project_path.is_none() {
                        "Intruder 已开始，结果保存在临时项目中，尚未写入磁盘项目文件。"
                    } else {
                        "Intruder 已开始。"
                    }
                    .into();
                }
                Ok(_) => {
                    pending.cancel.cancel();
                    self.fail(
                        "Intruder 开始确认的身份不一致；已请求取消，未自动重发。",
                        cx,
                    );
                }
                Err(error) => {
                    pending.cancel.cancel();
                    self.fail(format!("Intruder 未开始：{error}；未自动重发。"), cx);
                }
            }
        }
    }
}
