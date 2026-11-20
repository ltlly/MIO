use super::*;

impl ProjectStore {
    pub(in crate::store) fn intruder_connection(
        &self,
        id: Uuid,
    ) -> Result<(&Connection, IntruderRetention)> {
        if let Some(dataset) = self.intruder_sessions.runs.get(&id) {
            if dataset.disk_ticket.is_some() {
                let transaction = self.connection.unchecked_transaction()?;
                validate_ticket(&transaction, self.id, dataset)?;
                transaction.commit()?;
            } else {
                ensure!(
                    dataset.owner.id == self.id,
                    "Intruder Session 项目归属已改变"
                );
            }
            return Ok((&dataset.connection, IntruderRetention::Session));
        }
        Ok((&self.connection, IntruderRetention::Project))
    }

    pub(in crate::store) fn merge_session_intruder_runs(
        &self,
        connection: &Transaction<'_>,
        cursor: Option<(i64, Uuid)>,
        limit: usize,
        disk: IntruderRunPage,
    ) -> Result<IntruderRunPage> {
        if self.intruder_sessions.runs.is_empty() {
            return Ok(disk);
        }
        let mut sessions = Vec::with_capacity(self.intruder_sessions.runs.len());
        for dataset in self.intruder_sessions.runs.values() {
            validate_ticket(connection, self.id, dataset)?;
            if cursor.is_none_or(|cursor| (dataset.run.started_unix_ms, dataset.run.id) > cursor) {
                sessions.push(&dataset.run);
            }
        }
        sessions.sort_unstable_by_key(|run| (run.started_unix_ms, run.id));
        let mut sessions = sessions.into_iter().peekable();
        let mut stored = disk.rows.into_iter().peekable();
        let mut rows = Vec::with_capacity(limit);
        let mut bytes = 2;
        let more = loop {
            // The next disk row is unknown when its byte-bounded page ended early.
            if stored.peek().is_none() && disk.next_after.is_some() {
                break true;
            }
            let candidate = match (stored.peek(), sessions.peek()) {
                (Some(disk), Some(session)) => {
                    (session.started_unix_ms, session.id) < (disk.started_unix_ms, disk.id)
                }
                (Some(_), None) => false,
                (None, Some(_)) => true,
                (None, None) => break false,
            };
            if rows.len() == limit {
                break true;
            }
            let run = if candidate {
                *sessions.peek().unwrap()
            } else {
                stored.peek().unwrap()
            };
            let length = serde_json::to_vec(run)?.len() + usize::from(!rows.is_empty());
            ensure!(
                length <= PAGE_BYTES - 2,
                "Intruder run 摘要超过分页编码限额"
            );
            if length > PAGE_BYTES - bytes {
                break true;
            }
            bytes += length;
            rows.push(if candidate {
                sessions.next().unwrap().clone()
            } else {
                stored.next().unwrap()
            });
        };
        let next_after = more.then(|| rows.last().expect("bounded nonempty Intruder page").id);
        Ok(IntruderRunPage { rows, next_after })
    }

    pub(in crate::store) fn validate_session_intruder_summaries(
        &self,
        id: Uuid,
        rows: &[IntruderResultSummary],
    ) -> Result<()> {
        self.with_session_intruder_query(id, |validate| {
            for row in rows {
                validate(row)?;
            }
            Ok(())
        })
    }

    /// Keep one project snapshot while querying a Session's separate in-memory connection.
    pub(in crate::store) fn with_session_intruder_query<T>(
        &self,
        id: Uuid,
        query: impl FnOnce(&dyn Fn(&IntruderResultSummary) -> Result<()>) -> Result<T>,
    ) -> Result<T> {
        let dataset = self
            .intruder_sessions
            .runs
            .get(&id)
            .context("Intruder Session 不存在")?;
        let transaction = self.connection.unchecked_transaction()?;
        validate_ticket(&transaction, self.id, dataset)?;
        let result = query(&|row| {
            if let IntruderResultOutcome::Flow(summary) = &row.outcome {
                self.validate_session_disk_flow(dataset, summary.id, false)?;
            }
            Ok(())
        })?;
        transaction.commit()?;
        Ok(result)
    }

    fn validate_session_disk_flow(
        &self,
        dataset: &SessionDataset,
        flow: FlowId,
        messages: bool,
    ) -> Result<()> {
        if has_flow(&self.connection, flow)? {
            ensure!(
                dataset.disk_flows.contains(&flow),
                "Intruder Session flow ID 与项目数据冲突"
            );
            ensure!(
                same_flow(&self.connection, &dataset.connection, flow, messages)?,
                "Intruder 共享 flow revision 或消息已改变；未返回不一致的数据"
            );
        }
        Ok(())
    }

    fn session_with_flow(&self, id: FlowId) -> Result<Option<&SessionDataset>> {
        if self.intruder_sessions.runs.is_empty() {
            return Ok(None);
        }
        let transaction = self.connection.unchecked_transaction()?;
        let mut found: Option<&SessionDataset> = None;
        for dataset in self.intruder_sessions.runs.values() {
            if !has_flow(&dataset.connection, id)? {
                continue;
            }
            validate_ticket(&transaction, self.id, dataset)?;
            self.validate_session_disk_flow(dataset, id, true)?;
            if let Some(previous) = found {
                ensure!(
                    same_flow(&previous.connection, &dataset.connection, id, true)?,
                    "Intruder Session 的共享 flow 消息不一致"
                );
            }
            found = Some(dataset);
        }
        transaction.commit()?;
        Ok(found)
    }

    pub(in crate::store) fn read_session_intruder_flow(
        &self,
        id: FlowId,
    ) -> Result<Option<FlowRecord>> {
        self.session_with_flow(id)?
            .map(|dataset| {
                store::read_record(&dataset.connection, id)?.context("Intruder Session 流量缺失")
            })
            .transpose()
    }

    pub(in crate::store) fn original_session_intruder_flow(
        &self,
        id: FlowId,
    ) -> Result<Option<OriginalMessages>> {
        self.session_with_flow(id)?
            .map(|dataset| store::read_original(&dataset.connection, id))
            .transpose()
    }

    pub(in crate::store) fn validate_session_workspace(&self, workspace: &Workspace) -> Result<()> {
        validate_workspace_links(&self.connection, &self.intruder_sessions, workspace)
    }

    pub fn reject_session_intruder_flow_write(&self, id: FlowId) -> Result<()> {
        for dataset in self.intruder_sessions.runs.values() {
            ensure!(
                !has_flow(&dataset.connection, id)?,
                "Intruder Session 流量不能被覆盖或通过普通保存写回项目；请显式转换保留方式"
            );
        }
        Ok(())
    }
}
