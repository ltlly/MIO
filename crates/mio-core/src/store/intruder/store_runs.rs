use super::*;

impl ProjectStore {
    #[cfg(test)]
    pub fn create_intruder_run(
        &mut self,
        id: Uuid,
        tab_id: TabId,
        spec: &IntruderSpec,
        started_unix_ms: i64,
    ) -> Result<IntruderRunSummary> {
        self.create_project_intruder_run_with_pool_catalog(
            id,
            tab_id,
            spec,
            started_unix_ms,
            &CancellationToken::new(),
        )
        .map(|(run, _)| run)
    }

    pub(in crate::store::intruder) fn create_project_intruder_run_with_pool_catalog(
        &mut self,
        id: Uuid,
        tab_id: TabId,
        spec: &IntruderSpec,
        started_unix_ms: i64,
        cancel: &CancellationToken,
    ) -> Result<(IntruderRunSummary, crate::IntruderResourcePools)> {
        ensure!(
            !cancel.is_cancelled(),
            "Intruder Start 已取消；未创建资源池或运行"
        );
        ensure!(
            !self.intruder_sessions.contains(id),
            "Intruder Session run ID 已存在；未重新开始"
        );
        let total = spec.validate()?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        ensure!(
            super::super::metadata::project_id(&transaction)? == self.id,
            "Intruder 项目归属已改变"
        );
        validate_retirements(&transaction)?;
        ensure_not_discarded(&transaction, id)?;
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM intruder_runs WHERE id=?1)",
            [id.to_string()],
            |row| row.get(0),
        )?;
        ensure!(!exists, "Intruder run ID 已存在；未重新开始或重发请求");
        let workspace = super::super::metadata::workspace(&transaction)?;
        let tab = workspace
            .intruder
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .context("Intruder 工作标签未保存或已关闭；未开始")?;
        let payload_configs = workspace
            .intruder
            .payload_configs
            .get(&tab_id)
            .cloned()
            .unwrap_or_default();
        let has_pipeline = payload_configs
            .iter()
            .any(|cfg| cfg.processing_rules.iter().any(|r| r.enabled));
        ensure!(
            tab.spec == *spec
                || (has_pipeline
                    && tab.spec.revision == spec.revision
                    && tab.spec.draft == spec.draft
                    && tab.spec.attack_type == spec.attack_type
                    && tab.spec.positions == spec.positions
                    && tab.spec.max_concurrent_requests == spec.max_concurrent_requests
                    && tab.spec.request_timeout_ms == spec.request_timeout_ms
                    && tab.spec.update_host_header == spec.update_host_header
                    && tab.spec.save_to_project == spec.save_to_project
                    && tab.spec.baseline_request == spec.baseline_request
                    && tab.spec.resource_pool == spec.resource_pool),
            "Intruder 配置与当前已保存 revision 不一致；未开始旧配置"
        );
        let attack_settings = workspace
            .intruder
            .attack_settings
            .get(&tab_id)
            .cloned()
            .unwrap_or_default();
        let (pool, catalog) = super::super::resource_pools::resolve_start(
            &transaction,
            spec.resource_pool.as_ref(),
            cancel,
        )?;
        let run = insert_intruder_run_with_settings(
            &transaction,
            id,
            tab_id,
            spec,
            started_unix_ms,
            total,
            pool,
            attack_settings,
            payload_configs,
        )?;
        ensure!(
            !cancel.is_cancelled(),
            "Intruder Start 已取消；未创建资源池或运行"
        );
        transaction.commit()?;
        Ok((run, catalog))
    }

    /// The caller must exclude live senders and unsaved recovery before entering this transaction.
    pub fn discard_intruder_run(
        &mut self,
        id: Uuid,
        expected_revision: u64,
        workspace: Workspace,
        cancel: &CancellationToken,
    ) -> Result<IntruderDiscardReceipt> {
        ensure!(!cancel.is_cancelled(), "Intruder 丢弃已取消；数据保留");
        if self.intruder_sessions.contains(id) {
            return self.discard_session_intruder(id, expected_revision, workspace, cancel);
        }
        self.validate_session_workspace(&workspace)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let receipt = discard_project_run(
            &transaction,
            self.id,
            id,
            expected_revision,
            workspace,
            cancel,
        )?;
        transaction.commit()?;
        Ok(receipt)
    }

    pub fn read_intruder_run(&self, id: Uuid) -> Result<IntruderRunSummary> {
        let (connection, retention) = self.intruder_connection(id)?;
        let mut run = read_run(connection, id)?;
        run.retention = retention;
        Ok(run)
    }

    /// The caller excludes pending result recovery before changing an independent binding.
    pub fn set_intruder_resource_pool(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        expected_pool_revision: u64,
        pool_id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<IntruderRunSummary> {
        self.check_intruder_session_owner(project, id)?;
        if self.intruder_sessions.contains(id) {
            return self.set_session_intruder_resource_pool(
                id,
                expected_pool_revision,
                pool_id,
                cancel,
            );
        }
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        ensure!(
            super::super::metadata::project_id(&transaction)? == project.id,
            "Intruder 项目归属已改变"
        );
        let catalog = super::super::resource_pools::read(&transaction)?;
        ensure!(
            catalog.find(pool_id).is_some(),
            "Intruder 资源池不存在；未绑定"
        );
        let run = bind_resource_pool(&transaction, id, expected_pool_revision, pool_id, cancel)?;
        transaction.commit()?;
        Ok(run)
    }

    #[cfg(test)]
    pub(crate) fn test_intruder_connection(&self, id: Uuid) -> Result<&Connection> {
        self.intruder_connection(id)
            .map(|(connection, _)| connection)
    }

    /// The caller excludes pending recovery before this explicit admission.
    pub fn request_intruder_baseline(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        expected_revision: u64,
        cancel: &CancellationToken,
    ) -> Result<IntruderRunSummary> {
        self.request_intruder_baseline_with(project, id, expected_revision, cancel, |_| Ok(()))
            .map(|(run, ())| run)
    }

    /// Prepares the sender from the single validated owned spec before committing its slot.
    pub(crate) fn request_intruder_baseline_with<T>(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        expected_revision: u64,
        cancel: &CancellationToken,
        prepare: impl FnOnce(IntruderSpec) -> Result<T>,
    ) -> Result<(IntruderRunSummary, T)> {
        self.check_intruder_session_owner(project, id)?;
        if self.intruder_sessions.contains(id) {
            return self.request_session_intruder_baseline(id, expected_revision, cancel, prepare);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        ensure!(
            super::super::metadata::project_id(&transaction)? == project.id,
            "Intruder 项目归属已改变；未追加基准请求"
        );
        validate_pool_binding(&transaction, &read_run(&transaction, id)?)?;
        let prepared = request_baseline(&transaction, id, expected_revision, cancel, prepare)?;
        transaction.commit()?;
        Ok(prepared)
    }

    pub fn find_intruder_run(&self, id: Uuid) -> Result<Option<IntruderRunSummary>> {
        let (connection, retention) = self.intruder_connection(id)?;
        Ok(find_run(connection, id)?.map(|mut run| {
            run.retention = retention;
            run
        }))
    }

    pub fn intruder_runs(&self, after: Option<Uuid>, limit: usize) -> Result<IntruderRunPage> {
        ensure!((1..=500).contains(&limit), "Intruder 分页大小必须为 1–500");
        let transaction = self.connection.unchecked_transaction()?;
        let cursor = after
            .map(|id| {
                self.session_intruder_cursor(id)
                    .map(Ok)
                    .unwrap_or_else(|| run_cursor(&transaction, id))
            })
            .transpose()?;
        let mut output = Vec::with_capacity(limit);
        // Account for the array delimiters and the comma between encoded summaries.
        let mut bytes = 2;
        let mut more = false;
        {
            let mut query = transaction.prepare(&run_page_query(cursor.is_some()))?;
            let mut rows = if let Some(cursor) = cursor {
                query.query(params![
                    i64::try_from(limit + 1)?,
                    cursor.0,
                    cursor.1.to_string()
                ])?
            } else {
                query.query([i64::try_from(limit + 1)?])?
            };
            while let Some(row) = rows.next()? {
                if output.len() == limit {
                    more = true;
                    break;
                }
                let run = decode_run(row)?;
                ensure_not_discarded(&transaction, run.id)?;
                let size = serde_json::to_vec(&run)?.len() + usize::from(!output.is_empty());
                ensure!(size <= PAGE_BYTES - 2, "Intruder run 摘要超过分页编码限额");
                if size > PAGE_BYTES - bytes {
                    more = true;
                    break;
                }
                bytes += size;
                output.push(run);
            }
        }
        let next_after = more.then(|| output.last().expect("nonempty bounded page").id);
        let page = self.merge_session_intruder_runs(
            &transaction,
            cursor,
            limit,
            IntruderRunPage {
                rows: output,
                next_after,
            },
        )?;
        transaction.commit()?;
        Ok(page)
    }

    pub fn read_intruder_spec(&self, id: Uuid) -> Result<IntruderSpec> {
        let (connection, _) = self.intruder_connection(id)?;
        let transaction = connection.unchecked_transaction()?;
        let spec = read_spec(&transaction, id)?;
        transaction.commit()?;
        Ok(spec)
    }

    pub(crate) fn read_intruder_run_settings_only(
        &self,
        id: Uuid,
    ) -> Result<(
        crate::IntruderAttackSettings,
        Vec<crate::IntruderPayloadSetConfig>,
    )> {
        let (connection, _) = self.intruder_connection(id)?;
        read_run_settings_only(connection, id)
    }

    pub(crate) fn read_intruder_spec_with_settings(
        &self,
        id: Uuid,
    ) -> Result<(
        IntruderSpec,
        crate::IntruderAttackSettings,
        Vec<crate::IntruderPayloadSetConfig>,
    )> {
        let (connection, _) = self.intruder_connection(id)?;
        let transaction = connection.unchecked_transaction()?;
        let out = read_spec_with_settings(&transaction, id)?;
        transaction.commit()?;
        Ok(out)
    }
}
