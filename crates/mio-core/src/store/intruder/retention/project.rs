use super::*;

impl store::IntruderRepo<&mut ProjectStore> {
    pub(super) fn move_to_session(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        expected_revision: u64,
        workspace: Workspace,
        cancel: &CancellationToken,
    ) -> Result<IntruderRetentionReceipt> {
        ensure!(
            self.0.intruder_sessions.runs.len() < MAX_SESSION_DATASETS,
            "Intruder Session 达到 {MAX_SESSION_DATASETS} 个数据集限额；未驱逐或写入临时文件"
        );
        let transaction = self
            .0
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        ensure!(
            store::metadata::project_id(&transaction)? == project.id,
            "项目 ID 已改变；数据保留"
        );
        let run = read_run(&transaction, id)?;
        validate_pool_binding(&transaction, &run)?;
        ensure!(run.state.is_terminal(), "Intruder 尚未结束；数据保留");
        ensure!(
            run.revision == expected_revision,
            "Intruder run revision 已改变；数据保留"
        );
        let revision = next_revision(run.revision);
        let previous = store::metadata::workspace_for_update(&transaction)?;
        ensure!(
            previous.revision.checked_add(1) == Some(workspace.revision),
            "工作区已变更；数据保留"
        );
        workspace.validate()?;
        let available = self
            .0
            .intruder_sessions
            .quota
            .checked_sub(self.0.intruder_sessions.logical_bytes)
            .and_then(|bytes| bytes.checked_sub(self.0.intruder_sessions.reserved_bytes))
            .context("Intruder Session 逻辑配额计量不一致")?;
        let logical_bytes = dataset_logical_bytes(&transaction, id, available, cancel)?;
        let admitted = self
            .0
            .intruder_sessions
            .logical_bytes
            .checked_add(logical_bytes)
            .context("Intruder Session 逻辑数据计量溢出")?;
        ensure!(
            admitted
                .checked_add(self.0.intruder_sessions.reserved_bytes)
                .is_some_and(|bytes| bytes <= self.0.intruder_sessions.quota),
            "Intruder Session 逻辑 RAM 数据配额不足（需要 {admitted}，限额 {} 字节）；未复制、驱逐或溢写到磁盘",
            self.0.intruder_sessions.quota
        );
        validate_dataset(&transaction, id, cancel)?;
        let pinned: BTreeSet<_> = previous
            .tabs
            .iter()
            .chain(&previous.closed)
            .chain(&workspace.tabs)
            .chain(&workspace.closed)
            .filter_map(|tab| tab.last_flow)
            .chain(store::plugin_storage::audit_pinned_flows(&transaction)?)
            .collect();
        let mut data = SessionData::load_from_project(&transaction, id, cancel)?;
        let mut disk_flows = BTreeSet::new();
        for flow in data.flow_ids_in_order() {
            if shared_discard_flow(&transaction, id, flow, &pinned)? {
                disk_flows.insert(flow);
            }
            for existing in self.0.intruder_sessions.runs.values() {
                if existing.data.has_flow(flow) {
                    disk_flows.insert(flow);
                    validate_ticket(&transaction, self.0.id, existing)?;
                    ensure!(
                        existing.data.matches_disk_flow(&transaction, flow, true)?,
                        "Intruder Session 已保留不同的同 ID 消息版本；数据保留"
                    );
                }
            }
        }
        ensure!(
            data.logical_bytes()? == logical_bytes,
            "Intruder 源数据类型或内容在 RAM 中改变；数据保留"
        );
        data.run.revision = revision;
        let retained = data.run.clone();
        let dataset = SessionDataset {
            data,
            owner: project,
            run: retained.clone(),
            logical_bytes,
            disk_flows,
            disk_ticket: Some((retained.revision, retained.started_unix_ms)),
            reservations: BTreeMap::new(),
            state_reserve: 0,
            reactivated_baseline: None,
            #[cfg(test)]
            reject_writes: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            write_hook: std::sync::Mutex::new(None),
            #[cfg(test)]
            body_reads: std::sync::atomic::AtomicUsize::new(0),
        };
        validate_workspace_links(&transaction, &self.0.intruder_sessions, &workspace)?;
        // Reserve runtime ownership before disk deletion; a failed transaction removes only this stage.
        self.0.intruder_sessions.runs.insert(id, dataset);
        self.0.intruder_sessions.logical_bytes = admitted;
        let transferred = (|| -> Result<IntruderDiscardReceipt> {
            let receipt = discard_project_run(
                &transaction,
                self.0.id,
                id,
                expected_revision,
                workspace,
                cancel,
            )?;
            ensure!(
                receipt.revision == revision,
                "Intruder 保留切换 revision 不一致"
            );
            transaction.commit()?;
            Ok(receipt)
        })();
        let receipt = match transferred {
            Ok(receipt) => receipt,
            Err(error) => {
                self.0.intruder_sessions.remove(id);
                return Err(error);
            }
        };
        self.0.update_workspace_cache(&receipt.workspace);
        Ok(IntruderRetentionReceipt {
            run: retained,
            retention: IntruderRetention::Session,
            workspace: receipt.workspace,
            session_logical_bytes: logical_bytes,
        })
    }

    pub(super) fn promote_session(
        &mut self,
        id: Uuid,
        expected_revision: u64,
        workspace: Workspace,
        cancel: &CancellationToken,
    ) -> Result<IntruderRetentionReceipt> {
        let dataset = self
            .0
            .intruder_sessions
            .runs
            .get(&id)
            .context("Intruder Session 不存在")?;
        ensure!(
            dataset.run.revision == expected_revision,
            "Intruder run revision 已改变；数据保留"
        );
        let revision = next_revision(expected_revision);
        let transaction = self
            .0
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        validate_ticket(&transaction, self.0.id, dataset)?;
        ensure!(
            dataset.data.logical_bytes()? == dataset.logical_bytes,
            "Intruder RAM 数据计量已改变；数据保留"
        );
        let retained = dataset.data.validate_snapshot(cancel, true)?;
        ensure!(retained == dataset.run, "Intruder RAM 摘要已改变；数据保留");
        let previous = store::metadata::workspace_for_update(&transaction)?;
        ensure!(
            previous.revision.checked_add(1) == Some(workspace.revision),
            "工作区已变更；数据保留"
        );
        workspace.validate()?;
        ensure_no_run_rows(&transaction, id)?;
        dataset.data.write_to_project(
            &transaction,
            &dataset.disk_flows,
            revision,
            cancel,
            |flow| {
                ensure!(
                    dataset.data.matches_disk_flow(&transaction, flow, true)?,
                    "Intruder 共享 flow revision、消息或未知数据已改变；RAM 保留"
                );
                Ok(())
            },
        )?;
        if let Some((ticket_revision, started)) = dataset.disk_ticket {
            let removed = transaction.execute(
                "DELETE FROM main.metadata WHERE key=?1 AND value=?2",
                params![
                    retirement_key(id),
                    ticket_bytes(ticket_revision, started).as_slice()
                ],
            )?;
            ensure!(removed == 1, "Intruder retirement receipt 已改变；RAM 保留");
        }
        let mut run = dataset.run.clone();
        validate_pool_binding(&transaction, &run)?;
        run.revision = revision;
        run.retention = IntruderRetention::Project;
        ensure!(
            validate_dataset_snapshot(&transaction, id, cancel, true)? == run,
            "Intruder 项目副本不完整；RAM 保留"
        );
        validate_workspace_links(&transaction, &self.0.intruder_sessions, &workspace)?;
        store::write_workspace(&transaction, &workspace)?;
        ensure!(!cancel.is_cancelled(), "Intruder 保留切换已取消；RAM 保留");
        transaction.commit()?;
        self.0.update_workspace_cache(&workspace);
        self.0.intruder_sessions.remove(id);
        Ok(IntruderRetentionReceipt {
            run,
            retention: IntruderRetention::Project,
            workspace,
            session_logical_bytes: 0,
        })
    }

    pub(in crate::store) fn discard_session(
        &mut self,
        id: Uuid,
        expected_revision: u64,
        mut workspace: Workspace,
        cancel: &CancellationToken,
    ) -> Result<IntruderDiscardReceipt> {
        ensure!(!cancel.is_cancelled(), "Intruder 丢弃已取消；数据保留");
        let dataset = self
            .0
            .intruder_sessions
            .runs
            .get(&id)
            .context("Intruder Session 不存在")?;
        ensure!(
            dataset.run.revision == expected_revision && dataset.run.state.is_terminal(),
            "Intruder 终态或 revision 已改变；数据保留"
        );
        let revision = next_revision(expected_revision);
        let transaction = self
            .0
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        validate_ticket(&transaction, self.0.id, dataset)?;
        for tab in workspace
            .intruder
            .tabs
            .iter_mut()
            .chain(&mut workspace.intruder.closed)
        {
            if tab.last_run == Some(id) {
                tab.last_run = None;
            }
        }
        let audit_pinned: BTreeSet<_> = store::plugin_storage::audit_pinned_flows(&transaction)?
            .into_iter()
            .collect();
        let mut deleted_flows = 0;
        let mut retained_flows = 0;
        for flow in dataset.data.flow_ids_in_order() {
            ensure!(!cancel.is_cancelled(), "Intruder 丢弃已取消；数据保留");
            let on_disk = has_flow(&transaction, flow)?;
            ensure!(
                !audit_pinned.contains(&flow) || on_disk,
                "Intruder Session-only 流量仍被审计告警引用；数据保留"
            );
            let mut shared = on_disk;
            for other in self
                .0
                .intruder_sessions
                .runs
                .values()
                .filter(|other| other.run.id != id)
            {
                shared |= other.data.has_flow(flow);
            }
            if shared {
                retained_flows += 1;
            } else {
                deleted_flows += 1;
            }
        }
        validate_workspace_links(&transaction, &self.0.intruder_sessions, &workspace)?;
        store::write_workspace(&transaction, &workspace)?;
        let mut retired = dataset.run.clone();
        retired.revision = revision;
        let changed = if let Some((revision, started)) = dataset.disk_ticket {
            transaction.execute(
                "UPDATE main.metadata SET value=?1 WHERE key=?2 AND value=?3",
                params![
                    retirement_bytes(&retired).as_slice(),
                    retirement_key(id),
                    ticket_bytes(revision, started).as_slice()
                ],
            )?
        } else {
            transaction.execute(
                "INSERT INTO main.metadata(key,value) VALUES(?1,?2)",
                params![retirement_key(id), retirement_bytes(&retired).as_slice()],
            )?
        };
        ensure!(changed == 1, "Intruder retirement receipt 已改变；数据保留");
        ensure!(!cancel.is_cancelled(), "Intruder 丢弃已取消；数据保留");
        transaction.commit()?;
        self.0.update_workspace_cache(&workspace);
        // Independently retained disk flows and other Session owners are not owned by this drop.
        self.0.intruder_sessions.remove(id);
        Ok(IntruderDiscardReceipt {
            revision,
            workspace,
            deleted_flows,
            retained_flows,
        })
    }
}
