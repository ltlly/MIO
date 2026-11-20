use super::*;

impl ProjectStore {
    pub(super) fn move_intruder_to_session(
        &mut self,
        project: ProjectKey,
        id: Uuid,
        expected_revision: u64,
        workspace: Workspace,
        cancel: &CancellationToken,
    ) -> Result<IntruderRetentionReceipt> {
        ensure!(
            self.intruder_sessions.runs.len() < MAX_SESSION_DATASETS,
            "Intruder Session 达到 {MAX_SESSION_DATASETS} 个数据集限额；未驱逐或写入临时文件"
        );
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        store::schema_guard::validate_intruder_discard(&transaction)?;
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
        let revision = next_revision(run.revision)?;
        // Admission must leave a revision available for an explicit promotion or discard.
        next_revision(revision)?;
        let previous = store::metadata::workspace_for_update(&transaction)?;
        ensure!(
            previous.revision.checked_add(1) == Some(workspace.revision),
            "工作区已变更；数据保留"
        );
        workspace.validate()?;
        let available = self
            .intruder_sessions
            .quota
            .checked_sub(self.intruder_sessions.logical_bytes)
            .and_then(|bytes| bytes.checked_sub(self.intruder_sessions.reserved_bytes))
            .context("Intruder Session 逻辑配额计量不一致")?;
        let logical_bytes = dataset_logical_bytes(&transaction, id, available, cancel)?;
        let admitted = self
            .intruder_sessions
            .logical_bytes
            .checked_add(logical_bytes)
            .context("Intruder Session 逻辑数据计量溢出")?;
        ensure!(
            admitted
                .checked_add(self.intruder_sessions.reserved_bytes)
                .is_some_and(|bytes| bytes <= self.intruder_sessions.quota),
            "Intruder Session 逻辑 RAM 数据配额不足（需要 {admitted}，限额 {} 字节）；未复制、驱逐或溢写到磁盘",
            self.intruder_sessions.quota
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
        let mut disk_flows = BTreeSet::new();
        visit_flows(&transaction, id, |flow| {
            if shared_discard_flow(&transaction, id, flow, &pinned)? {
                disk_flows.insert(flow);
            }
            for existing in self.intruder_sessions.runs.values() {
                if has_flow(&existing.connection, flow)? {
                    disk_flows.insert(flow);
                    validate_ticket(&transaction, self.id, existing)?;
                    ensure!(
                        same_flow(&transaction, &existing.connection, flow, true)?,
                        "Intruder Session 已保留不同的同 ID 消息版本；数据保留"
                    );
                }
            }
            Ok(())
        })?;
        let connection = session_connection()?;
        let memory = connection.unchecked_transaction()?;
        for &(table, columns, selector) in DATA {
            copy_rows(&transaction, &memory, table, columns, selector, id, cancel)?;
        }
        // Flow selectors need the result links copied after their foreign-key parents.
        for &(table, columns, selector) in DATA {
            ensure!(
                same_rows(&transaction, &memory, &select(table, columns, selector), id)?,
                "Intruder 源数据类型或内容在 RAM 中改变；数据保留"
            );
        }
        ensure!(
            validate_dataset(&memory, id, cancel)? == run,
            "Intruder RAM 快照摘要不一致"
        );
        let changed = memory.execute(
            "UPDATE main.intruder_runs SET revision=?1 WHERE id=?2 AND revision=?3",
            params![revision, id.to_string(), expected_revision],
        )?;
        ensure!(changed == 1, "Intruder RAM revision 冲突");
        memory.commit()?;
        connection.pragma_update(None, "query_only", true)?;
        let mut retained = run;
        retained.revision = revision;
        retained.retention = IntruderRetention::Session;
        let dataset = SessionDataset {
            connection,
            owner: project,
            run: retained.clone(),
            logical_bytes,
            disk_flows,
            disk_ticket: Some((retained.revision, retained.started_unix_ms)),
            reservations: BTreeMap::new(),
            state_reserve: 0,
            reactivated_baseline: None,
        };
        validate_workspace_links(&transaction, &self.intruder_sessions, &workspace)?;
        // Reserve runtime ownership before disk deletion; a failed transaction removes only this stage.
        self.intruder_sessions.runs.insert(id, dataset);
        self.intruder_sessions.logical_bytes = admitted;
        let transferred = (|| -> Result<IntruderDiscardReceipt> {
            let receipt = discard_project_run(
                &transaction,
                self.id,
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
                self.intruder_sessions.remove(id);
                return Err(error);
            }
        };
        Ok(IntruderRetentionReceipt {
            run: retained,
            retention: IntruderRetention::Session,
            workspace: receipt.workspace,
            session_logical_bytes: logical_bytes,
        })
    }

    pub(super) fn promote_session_intruder(
        &mut self,
        id: Uuid,
        expected_revision: u64,
        workspace: Workspace,
        cancel: &CancellationToken,
    ) -> Result<IntruderRetentionReceipt> {
        let dataset = self
            .intruder_sessions
            .runs
            .get(&id)
            .context("Intruder Session 不存在")?;
        ensure!(
            dataset.run.revision == expected_revision,
            "Intruder run revision 已改变；数据保留"
        );
        let revision = next_revision(expected_revision)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        store::schema_guard::validate_intruder_discard(&transaction)?;
        validate_ticket(&transaction, self.id, dataset)?;
        ensure!(
            dataset_logical_bytes(&dataset.connection, id, dataset.logical_bytes, cancel)?
                == dataset.logical_bytes,
            "Intruder RAM 数据计量已改变；数据保留"
        );
        let mut retained = validate_dataset_snapshot(&dataset.connection, id, cancel, true)?;
        retained.retention = IntruderRetention::Session;
        ensure!(retained == dataset.run, "Intruder RAM 摘要已改变；数据保留");
        let previous = store::metadata::workspace_for_update(&transaction)?;
        ensure!(
            previous.revision.checked_add(1) == Some(workspace.revision),
            "工作区已变更；数据保留"
        );
        workspace.validate()?;
        ensure_no_run_rows(&transaction, id)?;
        for &(table, columns, selector) in &DATA[..2] {
            copy_rows(
                &dataset.connection,
                &transaction,
                table,
                columns,
                selector,
                id,
                cancel,
            )?;
        }
        visit_flows(&dataset.connection, id, |flow| {
            ensure!(!cancel.is_cancelled(), "Intruder 保留切换已取消；数据保留");
            if has_flow(&transaction, flow)? {
                ensure!(
                    dataset.disk_flows.contains(&flow),
                    "Intruder flow ID 被其它数据占用；RAM 保留"
                );
                ensure!(
                    same_flow(&transaction, &dataset.connection, flow, true)?,
                    "Intruder 共享 flow revision、消息或未知数据已改变；RAM 保留"
                );
            } else {
                let orphan: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM main.messages WHERE flow=?1)
                     OR EXISTS(SELECT 1 FROM main.intruder_results WHERE flow=?1)
                     OR EXISTS(SELECT 1 FROM main.websocket_messages WHERE flow=?1)
                     OR EXISTS(SELECT 1 FROM main.websocket_connections WHERE flow=?1)
                     OR EXISTS(SELECT 1 FROM main.repeater_history WHERE flow=?1)",
                    [flow.to_string()],
                    |row| row.get(0),
                )?;
                ensure!(!orphan, "Intruder flow 存在无主引用；RAM 保留");
                copy_rows(
                    &dataset.connection,
                    &transaction,
                    "flows",
                    DATA[2].1,
                    "id=?1",
                    flow,
                    cancel,
                )?;
                copy_rows(
                    &dataset.connection,
                    &transaction,
                    "messages",
                    DATA[3].1,
                    "flow=?1 ORDER BY side",
                    flow,
                    cancel,
                )?;
            }
            Ok(())
        })?;
        copy_rows(
            &dataset.connection,
            &transaction,
            DATA[4].0,
            DATA[4].1,
            DATA[4].2,
            id,
            cancel,
        )?;
        if let Some((revision, started)) = dataset.disk_ticket {
            let removed = transaction.execute(
                "DELETE FROM main.metadata WHERE key=?1 AND value=?2",
                params![
                    retirement_key(id),
                    ticket_bytes(revision, started).as_slice()
                ],
            )?;
            ensure!(removed == 1, "Intruder retirement receipt 已改变；RAM 保留");
        }
        let changed = transaction.execute(
            "UPDATE main.intruder_runs SET revision=?1 WHERE id=?2 AND revision=?3",
            params![revision, id.to_string(), expected_revision],
        )?;
        ensure!(changed == 1, "Intruder 项目 revision 冲突；RAM 保留");
        let mut run = dataset.run.clone();
        validate_pool_binding(&transaction, &run)?;
        run.revision = revision;
        run.retention = IntruderRetention::Project;
        ensure!(
            validate_dataset_snapshot(&transaction, id, cancel, true)? == run,
            "Intruder 项目副本不完整；RAM 保留"
        );
        validate_workspace_links(&transaction, &self.intruder_sessions, &workspace)?;
        store::write_workspace(&transaction, &workspace)?;
        ensure!(!cancel.is_cancelled(), "Intruder 保留切换已取消；RAM 保留");
        transaction.commit()?;
        self.intruder_sessions.remove(id);
        Ok(IntruderRetentionReceipt {
            run,
            retention: IntruderRetention::Project,
            workspace,
            session_logical_bytes: 0,
        })
    }

    pub(in crate::store) fn discard_session_intruder(
        &mut self,
        id: Uuid,
        expected_revision: u64,
        mut workspace: Workspace,
        cancel: &CancellationToken,
    ) -> Result<IntruderDiscardReceipt> {
        ensure!(!cancel.is_cancelled(), "Intruder 丢弃已取消；数据保留");
        let dataset = self
            .intruder_sessions
            .runs
            .get(&id)
            .context("Intruder Session 不存在")?;
        ensure!(
            dataset.run.revision == expected_revision && dataset.run.state.is_terminal(),
            "Intruder 终态或 revision 已改变；数据保留"
        );
        let revision = next_revision(expected_revision)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        store::schema_guard::validate_intruder_discard(&transaction)?;
        validate_ticket(&transaction, self.id, dataset)?;
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
        visit_flows(&dataset.connection, id, |flow| {
            ensure!(!cancel.is_cancelled(), "Intruder 丢弃已取消；数据保留");
            let on_disk = has_flow(&transaction, flow)?;
            ensure!(
                !audit_pinned.contains(&flow) || on_disk,
                "Intruder Session-only 流量仍被审计告警引用；数据保留"
            );
            let mut shared = on_disk;
            for other in self
                .intruder_sessions
                .runs
                .values()
                .filter(|other| other.run.id != id)
            {
                shared |= has_flow(&other.connection, flow)?;
            }
            if shared {
                retained_flows += 1;
            } else {
                deleted_flows += 1;
            }
            Ok(())
        })?;
        validate_workspace_links(&transaction, &self.intruder_sessions, &workspace)?;
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
        // Independently retained disk flows and other Session owners are not owned by this drop.
        self.intruder_sessions.remove(id);
        Ok(IntruderDiscardReceipt {
            revision,
            workspace,
            deleted_flows,
            retained_flows,
        })
    }
}
