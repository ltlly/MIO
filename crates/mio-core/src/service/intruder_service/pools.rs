use super::*;

impl State {
    pub(in crate::service) fn intruder_resource_pools(
        &self,
        project: ProjectKey,
    ) -> Result<IntruderResourcePools> {
        self.check_project(project)?;
        Ok(self.intruder.resource_pools.clone())
    }

    pub(in crate::service) fn intruder_pool_usage(
        &self,
        project: ProjectKey,
    ) -> Result<IntruderPoolUsage> {
        self.check_project(project)?;
        let mut usage = self.store.resource_pools().usage(&self.cancel)?;
        let ids: BTreeSet<_> = self
            .intruder
            .runs
            .keys()
            .copied()
            .chain(
                self.intruder
                    .recovery
                    .iter()
                    .map(|mutation| mutation_run(mutation).id),
            )
            .collect();
        for id in ids {
            let stored = self.store.intruder().read_run(id)?;
            let current = self.intruder_run(id)?;
            if stored.resource_pool != current.resource_pool {
                if let Some(pool) = stored.resource_pool {
                    let count = usage
                        .task_counts
                        .get_mut(&pool)
                        .context("Intruder 当前资源池缺失")?;
                    *count = count
                        .checked_sub(1)
                        .context("Intruder 资源池任务计数不一致")?;
                }
                if let Some(pool) = current.resource_pool {
                    let count = usage
                        .task_counts
                        .get_mut(&pool)
                        .context("Intruder 当前资源池缺失")?;
                    *count = count
                        .checked_add(1)
                        .context("Intruder 资源池任务计数溢出")?;
                }
            }
        }
        usage.requires_confirmation = usage
            .task_counts
            .iter()
            .filter_map(|(id, count)| (*count > 0).then_some(*id))
            .collect();
        let workspace = self.store.metadata().workspace()?;
        for tab in workspace
            .intruder
            .tabs
            .iter()
            .chain(&workspace.intruder.closed)
        {
            if let Some(pool) = &tab.spec.resource_pool
                && usage.task_counts.contains_key(&pool.id())
            {
                usage.requires_confirmation.insert(pool.id());
            }
        }
        for request in self.intruder.requests.values() {
            if let Some(pool) = request.resource_pool
                && usage.task_counts.contains_key(&pool)
            {
                usage.requires_confirmation.insert(pool);
            }
        }
        Ok(usage)
    }

    pub(in crate::service) fn delete_intruder_resource_pool(
        &mut self,
        project: ProjectKey,
        operation_id: Uuid,
        pool_id: Uuid,
        expected_revisions: (u64, u64),
        allow_reassign: bool,
        cancel: &CancellationToken,
    ) -> std::result::Result<IntruderPoolDeleteReceipt, IntruderPoolDeleteError> {
        let admitted = self.check_project(project).and_then(|()| {
            self.require_saved()?;
            ensure!(
                !operation_id.is_nil()
                    && !self.quiesced
                    && !self.cancel.is_cancelled()
                    && !cancel.is_cancelled(),
                "Intruder 资源池删除已取消或不可用"
            );
            Ok(())
        });
        if let Err(error) = admitted {
            return Err(IntruderPoolDeleteError {
                message: error.to_string(),
                reassigned_sessions: 0,
                default_pool_created: false,
                catalog: (project == self.project).then(|| self.intruder.resource_pools.clone()),
                workspace: (project == self.project)
                    .then(|| self.store.metadata().workspace().ok())
                    .flatten()
                    .map(Box::new),
            });
        }
        let in_flight = self
            .intruder
            .requests
            .values()
            .any(|request| request.resource_pool == Some(pool_id));
        let result = self.store.resource_pools_mut().delete(
            project,
            pool_id,
            expected_revisions,
            allow_reassign,
            in_flight,
            cancel,
        );
        let catalog = match &result {
            Ok(receipt) => Some(&receipt.catalog),
            Err(error) => error.catalog.as_ref(),
        };
        if let Some(catalog) = catalog {
            self.intruder.resource_pools = catalog.clone();
            self.intruder
                .pool_timings
                .retain(|id, _| catalog.find(*id).is_some());
            for pool in &catalog.pools {
                self.intruder.pool_timings.entry(pool.id).or_default();
            }
        }
        // A partial Session commit must survive subsequent result/lifecycle persistence.
        for (id, active) in &mut self.intruder.runs {
            if let Some(stored) = self.store.intruder().session_summary(*id) {
                active.summary.resource_pool = stored.resource_pool;
                active.summary.resource_pool_revision = stored.resource_pool_revision;
            } else if let Ok(receipt) = &result
                && active.summary.resource_pool == Some(pool_id)
            {
                active.summary.resource_pool = receipt.catalog.default_pool_id;
                // The transaction checked every affected Project revision before commit.
                active.summary.resource_pool_revision += 1;
            }
        }
        self.intruder.dirty = true;
        self.intruder.notified = false;
        result
    }

    pub(in crate::service) fn ensure_intruder_default_pool(
        &mut self,
        project: ProjectKey,
        operation_id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<IntruderResourcePools> {
        self.check_project(project)?;
        self.require_saved()?;
        ensure!(
            !operation_id.is_nil()
                && !self.quiesced
                && !self.cancel.is_cancelled()
                && !cancel.is_cancelled(),
            "Intruder 默认资源池请求已取消或不可用；未创建"
        );
        let catalog = self.store.resource_pools_mut().ensure_default(project, cancel)?;
        for pool in &catalog.pools {
            self.intruder.pool_timings.entry(pool.id).or_default();
        }
        self.intruder.resource_pools = catalog.clone();
        self.intruder.dirty = true;
        self.intruder.notified = false;
        Ok(catalog)
    }

    pub(in crate::service) fn set_intruder_resource_pools(
        &mut self,
        project: ProjectKey,
        operation_id: Uuid,
        expected_revision: u64,
        pools: Vec<IntruderResourcePool>,
        cancel: &CancellationToken,
    ) -> Result<IntruderResourcePools> {
        self.check_project(project)?;
        self.require_saved()?;
        ensure!(
            !operation_id.is_nil()
                && !self.quiesced
                && !self.cancel.is_cancelled()
                && !cancel.is_cancelled(),
            "Intruder 资源池修改已取消或不可用；未保存"
        );
        let retained = |id| pools.iter().any(|pool| pool.id == id);
        ensure!(
            self.intruder
                .requests
                .values()
                .all(|request| request.resource_pool.is_none_or(retained)),
            "Intruder 资源池仍有在途请求；未删除或修改目录"
        );
        ensure!(
            self.intruder
                .runs
                .values()
                .all(|run| run.summary.resource_pool.is_none_or(retained)),
            "Intruder 资源池仍被活动任务引用；未删除或修改目录"
        );
        let timings = self.intruder.prepared_pool_timings(&pools)?;
        let catalog = self
            .store
            .resource_pools_mut()
            .set(project, expected_revision, pools, cancel)?;
        self.intruder.resource_pools = catalog.clone();
        self.intruder.pool_timings = timings;
        self.intruder.dirty = true;
        self.intruder.notified = false;
        Ok(catalog)
    }

    pub(in crate::service) fn set_intruder_resource_pool(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        operation_id: Uuid,
        expected_pool_revision: u64,
        resource_pool: Uuid,
        cancel: &CancellationToken,
    ) -> Result<IntruderRunSummary> {
        self.check_project(project)?;
        self.require_saved()?;
        ensure!(
            !operation_id.is_nil()
                && !self.quiesced
                && !self.cancel.is_cancelled()
                && !cancel.is_cancelled(),
            "Intruder 资源池绑定已取消或不可用；未改变任务"
        );
        ensure!(
            self.intruder.resource_pools.find(resource_pool).is_some(),
            "Intruder 资源池不存在；未改变任务"
        );
        let run = self.store.intruder_mut().set_resource_pool(
            project,
            id,
            expected_pool_revision,
            resource_pool,
            cancel,
        )?;
        if let Some(active) = self.intruder.runs.get_mut(&id) {
            active.summary.resource_pool = run.resource_pool;
            active.summary.resource_pool_revision = run.resource_pool_revision;
        }
        self.intruder.dirty = true;
        self.intruder.notified = false;
        Ok(run)
    }
}
