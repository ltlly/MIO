use super::*;

impl IntruderSessions {
    pub(in crate::store) fn check_pool_delete_owner(&self, project: ProjectKey) -> Result<()> {
        ensure!(
            self.runs.values().all(|dataset| dataset.owner == project),
            "Intruder Session generation 或项目归属已改变；未删除资源池"
        );
        Ok(())
    }

    pub(in crate::store) fn add_pool_usage(
        &self,
        source: &Transaction<'_>,
        usage: &mut crate::IntruderPoolUsage,
        cancel: &CancellationToken,
    ) -> Result<()> {
        for dataset in self.runs.values() {
            ensure!(!cancel.is_cancelled(), "Intruder 资源池查询已取消");
            validate_ticket(source, dataset.owner.id, dataset)?;
            let mut stored = read_run(&dataset.connection, dataset.run.id)?;
            stored.retention = IntruderRetention::Session;
            ensure!(stored == dataset.run, "Intruder RAM 摘要已改变");
            if let Some(id) = stored.resource_pool {
                let count = usage
                    .task_counts
                    .get_mut(&id)
                    .context("Intruder 当前资源池缺失")?;
                *count = count.checked_add(1).context("Intruder 任务计数溢出")?;
                usage.requires_confirmation.insert(id);
            }
        }
        Ok(())
    }

    pub(in crate::store) fn preflight_pool_delete(
        &mut self,
        source: &Transaction<'_>,
        from: Uuid,
        to: Uuid,
        cancel: &CancellationToken,
    ) -> Result<()> {
        for dataset in self
            .runs
            .values_mut()
            .filter(|d| d.run.resource_pool == Some(from))
        {
            validate_ticket(source, dataset.owner.id, dataset)?;
            // Existing and replacement UUIDs have the same logical byte length.
            ensure!(
                self.logical_bytes
                    .checked_add(self.reserved_bytes)
                    .is_some_and(|n| n <= self.quota),
                "Intruder Session 配额计量无效；未删除资源池"
            );
            reassign_session(dataset, to, cancel, false, false)?;
        }
        Ok(())
    }

    pub(in crate::store) fn reassign_deleted_pool(
        &mut self,
        source: &Transaction<'_>,
        from: Uuid,
        to: Uuid,
        cancel: &CancellationToken,
        changed: &mut u64,
    ) -> Result<()> {
        for dataset in self
            .runs
            .values_mut()
            .filter(|d| d.run.resource_pool == Some(from))
        {
            ensure!(!cancel.is_cancelled(), "Intruder 资源池删除已取消");
            validate_ticket(source, dataset.owner.id, dataset)?;
            #[cfg(test)]
            let fail_commit = self.fail_pool_delete_commit == Some(dataset.run.id);
            #[cfg(not(test))]
            let fail_commit = false;
            #[cfg(test)]
            if fail_commit {
                self.fail_pool_delete_commit = None;
            }
            reassign_session(dataset, to, cancel, true, fail_commit)?;
            *changed += 1;
        }
        Ok(())
    }
}

fn reassign_session(
    dataset: &mut SessionDataset,
    to: Uuid,
    cancel: &CancellationToken,
    commit: bool,
    fail_commit: bool,
) -> Result<()> {
    let mut stored = read_run(&dataset.connection, dataset.run.id)?;
    stored.retention = IntruderRetention::Session;
    ensure!(
        stored == dataset.run,
        "Intruder RAM 摘要已改变；未删除资源池"
    );
    ensure!(
        dataset
            .connection
            .pragma_query_value::<bool, _>(None, "query_only", |r| r.get(0))?,
        "Intruder RAM 只读保护已改变"
    );
    dataset
        .connection
        .pragma_update(None, "query_only", false)?;
    let result = (|| -> Result<IntruderRunSummary> {
        let transaction = dataset.connection.unchecked_transaction()?;
        let before = session::mutable_bytes(&transaction, stored.id, None)?;
        let run = bind_resource_pool(
            &transaction,
            stored.id,
            stored.resource_pool_revision,
            to,
            cancel,
        )?;
        ensure!(
            session::mutable_bytes(&transaction, stored.id, None)? == before,
            "Intruder Session 改绑改变逻辑字节数；未提交"
        );
        transaction.pragma_update(None, "query_only", true)?;
        if commit {
            ensure!(!fail_commit, "injected Session pool delete commit failure");
            ensure!(!cancel.is_cancelled(), "Intruder 资源池删除已取消");
            transaction.commit()?;
        } else {
            transaction.rollback()?;
        }
        Ok(run)
    })();
    if result.is_err() {
        dataset.connection.pragma_update(None, "query_only", true)?;
    }
    let mut run = result?;
    if commit {
        run.retention = IntruderRetention::Session;
        dataset.run = run;
    }
    Ok(())
}

#[cfg(test)]
impl ProjectStore {
    pub(crate) fn fail_session_pool_delete_commit(&mut self, id: Uuid) {
        self.intruder_sessions.fail_pool_delete_commit = Some(id);
    }
}

impl ProjectStore {
    pub(crate) fn session_intruder_summary(&self, id: Uuid) -> Option<&IntruderRunSummary> {
        self.intruder_sessions
            .runs
            .get(&id)
            .map(|dataset| &dataset.run)
    }
}
