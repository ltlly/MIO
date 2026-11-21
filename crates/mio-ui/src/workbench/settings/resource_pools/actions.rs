use mio_core::{IntruderPoolDeleteReceipt, IntruderPoolSelection, Uuid};

use super::*;

fn valid_delete_receipt(kind: &MutationKind, receipt: &IntruderPoolDeleteReceipt) -> bool {
    let MutationKind::Delete {
        pool_id,
        catalog,
        workspace: Some(workspace),
        allow_reassign,
        ..
    } = kind
    else {
        return false;
    };
    if receipt.catalog.validate().is_err() || receipt.workspace.validate_structure().is_err() {
        return false;
    }
    let default_created =
        catalog.default_pool_id.is_none() && receipt.catalog.default_pool_id.is_some();
    if default_created && !allow_reassign {
        return false;
    }
    if catalog.default_pool_id.is_some()
        && receipt.catalog.default_pool_id != catalog.default_pool_id
    {
        return false;
    }
    if receipt.catalog.revision
        != catalog
            .revision
            .saturating_add(1 + u64::from(default_created))
    {
        return false;
    }
    let mut expected_pools: Vec<_> = catalog
        .pools
        .iter()
        .filter(|pool| pool.id != *pool_id)
        .cloned()
        .collect();
    if default_created {
        let Some(default) = receipt
            .catalog
            .default_pool_id
            .and_then(|id| receipt.catalog.find(id))
            .cloned()
        else {
            return false;
        };
        if catalog.find(default.id).is_some() {
            return false;
        }
        expected_pools.insert(0, default);
    }
    if receipt.catalog.pools != expected_pools {
        return false;
    }
    let mut expected = (**workspace).clone();
    let mut reassigned = 0;
    for tab in expected
        .intruder
        .tabs
        .iter_mut()
        .chain(&mut expected.intruder.closed)
    {
        if tab
            .spec
            .resource_pool
            .as_ref()
            .is_some_and(|selection| selection.id() == *pool_id)
        {
            let Some(default) = receipt.catalog.default_pool_id else {
                return false;
            };
            tab.spec.resource_pool = Some(IntruderPoolSelection::Existing(default));
            let Some(revision) = tab.spec.revision.checked_add(1) else {
                return false;
            };
            tab.spec.revision = revision;
            reassigned += 1;
        }
    }
    if reassigned > 0 {
        let Some(revision) = expected.revision.checked_add(1) else {
            return false;
        };
        expected.revision = revision;
    }
    receipt.reassigned_tabs == reassigned
        && receipt.workspace == expected
        && (*allow_reassign || receipt.reassigned_runs == 0 && reassigned == 0)
}

impl Workbench {
    pub(in crate::workbench) fn pool_management_available(&self) -> bool {
        self.project.is_some()
            && self.backend_available()
            && self.transition.is_none()
            && self.quit_attempt.is_none()
            && !self.shutdown_requested
            && self.pending_records == 0
            && self.storage_error.is_none()
    }

    pub(in crate::workbench) fn start_pool_mutation(
        &mut self,
        operation: Uuid,
        kind: MutationKind,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.pool_management_available()
            || !self.intruder_pool_management_available()
            || self.intruder.resource_pools.management.mutation_pending()
        {
            return Err("当前项目或资源池操作尚未结束；输入保留，请稍后再试。".into());
        }
        let project = self.project.unwrap();
        let pending = PoolMutation {
            project,
            operation,
            kind,
            cancel: self.shutdown.child_token(),
            deadline: cx.background_executor().now() + DEADLINE,
            expired: false,
            _timer: None,
        };
        self.intruder.resource_pools.management.error = None;
        self.intruder.resource_pools.management.outcome = None;
        self.intruder.resource_pools.management.pending = Some(pending);
        let timer = cx.background_executor().timer(DEADLINE);
        let owner = self.owner_window;
        let task = cx.spawn(async move |this, cx| {
            timer.await;
            let _ = owner.update(cx, |_, _, cx| {
                let _ = this.update(cx, |view, cx| {
                    if let Some(pending) = &mut view.intruder.resource_pools.management.pending
                        && pending.project == project
                        && pending.operation == operation
                        && cx.background_executor().now() >= pending.deadline
                    {
                        pending.expired = true;
                        pending.cancel.cancel();
                        view.intruder.resource_pools.management.error = Some(
                            "资源池修改确认超时；取消已请求，等待真实结果，未自动重试。".into(),
                        );
                        cx.notify();
                    }
                });
            });
        });
        self.intruder
            .resource_pools
            .management
            .pending
            .as_mut()
            .unwrap()
            ._timer = Some(task);
        Ok(())
    }

    pub(in crate::workbench) fn prepare_pool_delete(
        &mut self,
        project: ProjectKey,
        pool_id: Uuid,
        catalog_revision: u64,
        allow_reassign: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.project != Some(project)
            || self
                .intruder
                .resource_pools
                .catalog
                .as_ref()
                .is_none_or(|catalog| {
                    catalog.revision != catalog_revision
                        || catalog.default_pool_id == Some(pool_id)
                        || catalog.find(pool_id).is_none()
                })
        {
            return Err("待删除资源池或目录已改变；未删除。".into());
        }
        let catalog = self.intruder.resource_pools.catalog.clone().unwrap();
        self.start_pool_mutation(
            Uuid::new_v4(),
            MutationKind::Delete {
                pool_id,
                catalog,
                workspace: None,
                allow_reassign,
                sent: false,
            },
            cx,
        )?;
        if !self.flush_workspace(window, cx) {
            self.intruder.resource_pools.management.pending = None;
            return Err(self
                .error
                .clone()
                .unwrap_or_else(|| "最新草稿无法保存，未删除资源池。".into()));
        }
        self.advance_settings_pool_delete(cx);
        cx.notify();
        Ok(())
    }

    pub(in crate::workbench) fn advance_settings_pool_delete(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = &self.intruder.resource_pools.management.pending else {
            return;
        };
        let MutationKind::Delete {
            pool_id,
            catalog,
            allow_reassign,
            sent: false,
            ..
        } = &pending.kind
        else {
            return;
        };
        if pending.cancel.is_cancelled() || pending.expired || !self.pool_management_available() {
            self.intruder.resource_pools.management.pending = None;
            self.intruder.resource_pools.management.error =
                Some("资源池删除已取消，尚未发送删除命令。".into());
            cx.notify();
            return;
        }
        if self.pending_workspace.is_some()
            || self.workspace_dirty
            || self.pending_project_save.is_some()
            || self.project_saves_in_flight > 0
            || self.file_picker_pending()
            || !self.intruder_pool_management_available()
        {
            return;
        }
        let command = Command::DeleteIntruderResourcePool {
            project: pending.project,
            operation_id: pending.operation,
            pool_id: *pool_id,
            expected_catalog_revision: catalog.revision,
            expected_workspace_revision: self.workspace.revision,
            allow_reassign: *allow_reassign,
            cancel: pending.cancel.clone(),
        };
        if self.enqueue(command, cx) {
            if let MutationKind::Delete {
                sent, workspace, ..
            } = &mut self
                .intruder
                .resource_pools
                .management
                .pending
                .as_mut()
                .unwrap()
                .kind
            {
                *sent = true;
                *workspace = Some(self.workspace.clone());
            }
        } else {
            self.intruder.resource_pools.management.pending = None;
            self.intruder.resource_pools.management.error = self.error.clone();
        }
        cx.notify();
    }

    pub(in crate::workbench) fn on_settings_pool_event(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            Event::IntruderPoolUsage {
                project,
                query_revision,
                result,
            } => {
                self.on_settings_pool_usage(project, query_revision, result, cx);
                return;
            }
            Event::IntruderResourcePoolsUpdated {
                project,
                operation_id,
                result,
            } => {
                let Some(pending) = &self.intruder.resource_pools.management.pending else {
                    return;
                };
                if self.project != Some(project)
                    || pending.project != project
                    || pending.operation != operation_id
                {
                    return;
                }
                let MutationKind::Update { catalog, pools } = &pending.kind else {
                    return;
                };
                if result.as_ref().is_ok_and(|after| {
                    after.revision != catalog.revision.saturating_add(1)
                        || after.pools != *pools
                        || after.default_pool_id != catalog.default_pool_id
                        || after.validate().is_err()
                }) {
                    self.intruder.resource_pools.management.cancel_pending();
                    self.intruder.resource_pools.management.pending = None;
                    let message =
                        "资源池修改回执不匹配；未采用回执，正在重新读取实际目录。".to_owned();
                    self.intruder.resource_pools.management.error = Some(message.clone());
                    self.intruder.resource_pools.management.outcome =
                        Some((operation_id, Err(message)));
                    self.query_intruder_resource_pools(cx);
                    self.query_settings_pool_usage(cx);
                    cx.notify();
                    return;
                }
                let result = result.and_then(|after| {
                    if after.revision != catalog.revision.saturating_add(1)
                        || after.pools != *pools
                        || after.default_pool_id != catalog.default_pool_id
                        || after.validate().is_err()
                    {
                        return Err("资源池修改回执不匹配；实际结果未确认，请重新读取。".into());
                    }
                    Ok(after)
                });
                match result {
                    Ok(catalog) => {
                        self.accept_intruder_resource_pools(project, &catalog, cx);
                        self.intruder.resource_pools.management.pending = None;
                        self.intruder.resource_pools.management.outcome =
                            Some((operation_id, Ok(())));
                        self.intruder.resource_pools.management.error = None;
                        self.query_settings_pool_usage(cx);
                    }
                    Err(error) => {
                        self.intruder.resource_pools.management.pending = None;
                        self.intruder.resource_pools.management.error = Some(error.clone());
                        self.intruder.resource_pools.management.outcome =
                            Some((operation_id, Err(error)));
                        self.query_intruder_resource_pools(cx);
                        self.query_settings_pool_usage(cx);
                    }
                }
            }
            Event::IntruderResourcePoolDeleted {
                project,
                operation_id,
                pool_id,
                result,
            } => {
                let Some(pending) = &self.intruder.resource_pools.management.pending else {
                    return;
                };
                if self.project != Some(project)
                    || pending.project != project
                    || pending.operation != operation_id
                    || !matches!(pending.kind, MutationKind::Delete { pool_id: expected, sent: true, .. } if pool_id == expected)
                {
                    return;
                }
                if let Ok(receipt) = &result
                    && !valid_delete_receipt(&pending.kind, receipt)
                {
                    self.intruder.resource_pools.management.cancel_pending();
                    self.intruder.resource_pools.management.pending = None;
                    self.workspace_dirty = true;
                    let message = "资源池删除回执与提交快照不一致；未采用工作区，请重新打开项目核对实际状态。".to_owned();
                    self.intruder.resource_pools.management.error = Some(message.clone());
                    self.intruder.resource_pools.management.outcome =
                        Some((operation_id, Err(message)));
                    self.query_intruder_resource_pools(cx);
                    self.query_settings_pool_usage(cx);
                    cx.notify();
                    return;
                }
                let MutationKind::Delete {
                    workspace: Some(before),
                    ..
                } = &pending.kind
                else {
                    return;
                };
                let before = before.clone();
                let outcome = match result {
                    Ok(receipt) => self.apply_deleted_pool_state(
                        project,
                        pool_id,
                        &receipt.catalog,
                        (&before, &receipt.workspace),
                        window,
                        cx,
                    ),
                    Err(error) => {
                        if let (Some(catalog), Some(workspace)) = (&error.catalog, &error.workspace)
                        {
                            if let Err(failure) = self.apply_deleted_pool_state(
                                project,
                                pool_id,
                                catalog,
                                (&before, workspace),
                                window,
                                cx,
                            ) {
                                self.fail(failure, cx);
                            }
                        } else if let Some(catalog) = &error.catalog {
                            self.accept_intruder_resource_pools(project, catalog, cx);
                        }
                        Err(format!(
                            "{}；已迁移 {} 个会话任务，默认池{}建立。请读取实际状态后重试。",
                            error.message,
                            error.reassigned_sessions,
                            if error.default_pool_created {
                                "已"
                            } else {
                                "未"
                            }
                        ))
                    }
                };
                self.intruder.resource_pools.management.pending = None;
                self.intruder.resource_pools.management.error = outcome.clone().err();
                self.intruder.resource_pools.management.outcome = Some((operation_id, outcome));
                self.query_settings_pool_usage(cx);
            }
            _ => return,
        }
        cx.notify();
    }
}
