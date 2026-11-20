use super::*;

impl State {
    pub(super) fn persist_intruder_result(
        &mut self,
        id: Uuid,
        ordinal: u64,
        result: std::result::Result<Box<FlowRecord>, String>,
    ) -> Result<()> {
        self.persist_intruder_result_with_reason_phrase(id, ordinal, result, None)
    }

    pub(super) fn persist_intruder_result_with_reason_phrase(
        &mut self,
        id: Uuid,
        ordinal: u64,
        mut result: std::result::Result<Box<FlowRecord>, String>,
        reason_phrase: Option<&[u8]>,
    ) -> Result<()> {
        let session = self.store.is_session_intruder_run(id);
        let blocked = if session {
            self.intruder
                .recovery
                .iter()
                .any(|mutation| mutation_run(mutation).id == id)
        } else {
            self.unsaved_count() != 0
        };
        let saved = if !blocked {
            self.store.put_intruder_result_with_reason_phrase(
                id,
                ordinal,
                result.as_deref().map_err(String::as_str),
                reason_phrase,
            )
        } else {
            Err(anyhow::anyhow!("先前变更仍待写盘；Intruder 已停止派发"))
        };
        let run = self
            .intruder
            .runs
            .get_mut(&id)
            .context("Intruder 运行任务不存在")?;
        match saved {
            Ok(summary) => run.summary = summary,
            Err(error) => {
                // No socket exists for an uncommitted admission; retain an explicit unsent failure.
                if ordinal == run.summary.dispatched
                    && let Ok(record) = &mut result
                {
                    record.summary.state =
                        FlowState::Failed("登记请求失败，未发送；恢复不会重发请求".into());
                }
                run.summary.revision = next_revision(run.summary.revision)?;
                if ordinal == run.summary.dispatched {
                    run.summary.dispatched += 1;
                }
                if result.as_ref().map_or(true, |record| {
                    !matches!(
                        record.summary.state,
                        FlowState::Pending | FlowState::Intercepted
                    )
                }) {
                    run.summary.completed += 1;
                    if let Some(baseline) = &mut run.summary.baseline
                        && baseline.ordinal == ordinal
                    {
                        baseline.completed = true;
                    }
                }
                run.failure
                    .get_or_insert_with(|| "结果存储失败；已停止派发，恢复不会重发请求".into());
                if result
                    .as_ref()
                    .is_ok_and(|record| record.response.is_some())
                    && let Some(phrase) = reason_phrase
                {
                    self.intruder
                        .recovery_reason_phrases
                        .insert((id, ordinal), phrase.to_vec());
                } else {
                    self.intruder.recovery_reason_phrases.remove(&(id, ordinal));
                }
                self.intruder.recovery.push(IntruderMutation::Result {
                    run: run.summary.clone(),
                    ordinal,
                    result,
                });
                self.intruder.dirty = true;
                self.storage_failed(&error);
                return Err(error);
            }
        }
        if session
            && result.as_ref().is_ok_and(|record| {
                !matches!(
                    record.summary.state,
                    FlowState::Pending | FlowState::Intercepted
                )
            })
        {
            self.intruder.capacity_released();
        }
        self.intruder.dirty = true;
        Ok(())
    }

    pub(super) fn persist_intruder_state(
        &mut self,
        id: Uuid,
        state: IntruderRunState,
    ) -> Result<()> {
        let session = self.store.is_session_intruder_run(id);
        let terminal = state.is_terminal();
        let blocked = if session {
            self.intruder
                .recovery
                .iter()
                .any(|mutation| mutation_run(mutation).id == id)
        } else {
            self.unsaved_count() != 0
        };
        let saved = if !blocked {
            self.store.set_intruder_state(id, state.clone())
        } else {
            Err(anyhow::anyhow!(
                "先前变更仍待写盘；Intruder 状态保留等待保存"
            ))
        };
        let run = self
            .intruder
            .runs
            .get_mut(&id)
            .context("Intruder 运行任务不存在")?;
        match saved {
            Ok(summary) => run.summary = summary,
            Err(error) => {
                if run.summary.state != state {
                    run.summary.revision = next_revision(run.summary.revision)?;
                    run.summary.state = state;
                    self.intruder.recovery.push(IntruderMutation::State {
                        run: run.summary.clone(),
                    });
                }
                self.intruder.dirty = true;
                self.storage_failed(&error);
                return Err(error);
            }
        }
        if session && terminal {
            self.intruder.capacity_released();
        }
        self.intruder.dirty = true;
        Ok(())
    }

    pub(super) async fn complete_intruder(
        &mut self,
        result: std::result::Result<(Id, Completion), JoinError>,
    ) -> Result<()> {
        let task_id = match &result {
            Ok((id, _)) => *id,
            Err(error) => error.id(),
        };
        let active = self
            .intruder
            .requests
            .remove(&task_id)
            .context("Intruder 完成任务身份不匹配")?;
        let run = self
            .intruder
            .runs
            .get_mut(&active.run_id)
            .context("Intruder 完成任务所属 run 不存在")?;
        run.active = run
            .active
            .checked_sub(1)
            .context("Intruder 在途计数不一致")?;
        let (record, reason_phrase) = match result {
            Ok((_, completion)) => {
                ensure!(
                    completion.project == self.project
                        && completion.record.summary.id == active.flow_id,
                    "Intruder 完成结果归属不匹配，未覆盖其它结果"
                );
                (completion.record, completion.reason_phrase)
            }
            Err(error) => {
                run.cancel.cancel();
                let message = format!("Intruder 发送任务异常退出，结果未知，未自动重发：{error}");
                run.failure.get_or_insert_with(|| message.clone());
                let mut record = self.read_flow(active.flow_id)?;
                record.summary.revision = next_revision(record.summary.revision)?;
                record.summary.state = FlowState::Failed(message);
                (record, None)
            }
        };
        let saved = self.persist_intruder_result_with_reason_phrase(
            active.run_id,
            active.ordinal,
            Ok(Box::new(record)),
            reason_phrase.as_deref(),
        );
        self.intruder_history_changed();
        saved
    }

    pub(in crate::service) async fn stop_intruder(&mut self) -> Result<()> {
        self.stop_intruder_until(tokio::time::Instant::now() + Duration::from_secs(3))
            .await
    }

    pub(super) async fn stop_intruder_until(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> Result<()> {
        self.intruder.cancel_all();
        let ids: Vec<_> = self.intruder.runs.keys().copied().collect();
        let mut failure = None;
        for id in &ids {
            if matches!(
                self.intruder.runs[id].summary.state,
                IntruderRunState::Running | IntruderRunState::Paused
            ) && let Err(error) = self.persist_intruder_state(*id, IntruderRunState::Stopping)
            {
                failure = Some(error);
            }
        }
        while !self.intruder.jobs.is_empty() {
            match tokio::time::timeout_at(deadline, self.intruder.jobs.join_next_with_id()).await {
                Ok(Some(result)) => {
                    if let Err(error) = self.complete_intruder(result).await {
                        failure = Some(error);
                    }
                }
                Ok(None) => break,
                Err(_) => anyhow::bail!(
                    "Intruder 仍在回收；保留任务句柄，未确认停止或切换项目，请稍后重试"
                ),
            }
        }
        for id in ids {
            if let Err(error) = self.advance_intruder(id).await {
                failure = Some(error);
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(())
    }

    pub(in crate::service) async fn finish_intruder_shutdown(&mut self) -> Result<()> {
        // Shutdown keeps ownership until every sender has really joined, even after Stop's deadline.
        let mut error = None;
        while self.intruder.has_resources() {
            if let Err(failure) = self.stop_intruder().await {
                error = Some(failure);
            }
        }
        error.map_or(Ok(()), Err)
    }

    /// Recover Session writes in RAM before the caller submits the remaining disk batch.
    pub(in crate::service) fn retry_session_intruder_recovery(&mut self) -> Result<()> {
        let mut sessions = BTreeMap::<Uuid, Vec<IntruderMutation>>::new();
        let mut project = Vec::new();
        for mutation in std::mem::take(&mut self.intruder.recovery) {
            let id = mutation_run(&mutation).id;
            if self.store.is_session_intruder_run(id) {
                sessions.entry(id).or_default().push(mutation);
            } else {
                project.push(mutation);
            }
        }
        self.intruder.recovery = project;
        let mut failure = None;
        for (_, mutations) in sessions {
            match self
                .store
                .retry_session_intruder_mutations(&mutations, &self.cancel)
            {
                Ok(()) => self.intruder.capacity_released(),
                Err(error) => {
                    self.intruder.recovery.extend(mutations);
                    failure.get_or_insert(error);
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }

    pub(super) fn intruder_run(&self, id: Uuid) -> Result<IntruderRunSummary> {
        self.pending_intruder_run(id)
            .map(Ok)
            .unwrap_or_else(|| self.store.read_intruder_run(id))
    }

    pub(in crate::service) fn pending_intruder_run(&self, id: Uuid) -> Option<IntruderRunSummary> {
        if let Some(run) = self.intruder.runs.get(&id) {
            return Some(run.summary.clone());
        }
        self.intruder.recovery.iter().rev().find_map(|mutation| {
            let run = match mutation {
                IntruderMutation::Result { run, .. } | IntruderMutation::State { run } => run,
            };
            (run.id == id).then(|| run.clone())
        })
    }

    pub(in crate::service) fn intruder_run_page(
        &mut self,
        after: Option<Uuid>,
        limit: usize,
    ) -> Result<IntruderRunPage> {
        let stored = self.store.intruder_runs(after, limit)?;
        let mut rows = Vec::with_capacity(stored.rows.len());
        let mut bytes = 2;
        let mut more = stored.next_after.is_some();
        for stored in stored.rows {
            let summary = self.intruder_run(stored.id)?;
            let length = serde_json::to_vec(&summary)?.len() + usize::from(!rows.is_empty());
            ensure!(
                length <= RESULT_PAGE_BYTES - 2,
                "Intruder run 摘要超过分页编码限额"
            );
            if length > RESULT_PAGE_BYTES - bytes {
                more = true;
                break;
            }
            bytes += length;
            rows.push(summary);
        }
        let next_after = more.then(|| rows.last().expect("bounded nonempty Intruder page").id);
        self.intruder.queried();
        Ok(IntruderRunPage { rows, next_after })
    }

    pub(in crate::service) fn intruder_result_page(
        &mut self,
        id: Uuid,
        after: Option<u64>,
        limit: usize,
    ) -> Result<IntruderResultPage> {
        let stored = self.store.intruder_results(id, after, limit)?;
        let run = self.intruder_run(id)?;
        let mut results: BTreeMap<_, _> = stored
            .rows
            .into_iter()
            .map(|row| (row.ordinal, row))
            .collect();
        let mut persisted = stored.persisted;
        for mutation in &self.intruder.recovery {
            let mutation_run = match mutation {
                IntruderMutation::Result { run, .. } | IntruderMutation::State { run } => run,
            };
            if mutation_run.id != id {
                continue;
            }
            persisted = false;
            if let IntruderMutation::Result {
                ordinal, result, ..
            } = mutation
            {
                if after.is_some_and(|cursor| *ordinal <= cursor)
                    || stored.next_after.is_some_and(|last| *ordinal > last)
                {
                    continue;
                }
                results.insert(
                    *ordinal,
                    IntruderResultSummary {
                        ordinal: *ordinal,
                        outcome: match result {
                            Ok(record) => IntruderResultOutcome::Flow(record.summary.clone()),
                            Err(error) => IntruderResultOutcome::Rejected(error.clone()),
                        },
                    },
                );
            }
        }
        let mut rows = Vec::with_capacity(limit);
        let mut bytes = 0;
        let mut more = stored.next_after.is_some();
        for (_, result) in results {
            let length = match &result.outcome {
                IntruderResultOutcome::Flow(summary) => serde_json::to_vec(summary)?.len(),
                IntruderResultOutcome::Rejected(error) => error.len(),
            };
            if rows.len() == limit || (!rows.is_empty() && length > RESULT_PAGE_BYTES - bytes) {
                more = true;
                break;
            }
            ensure!(length <= RESULT_PAGE_BYTES, "Intruder 结果超过分页限额");
            bytes += length;
            rows.push(result);
        }
        let next_after = more.then(|| rows.last().expect("bounded nonempty Intruder page").ordinal);
        self.intruder.queried();
        Ok(IntruderResultPage {
            run,
            rows,
            next_after,
            persisted,
        })
    }

    pub(in crate::service) fn unsaved_intruder_flow(&self, id: FlowId) -> Option<&FlowRecord> {
        self.intruder
            .recovery
            .iter()
            .rev()
            .find_map(|mutation| match mutation {
                IntruderMutation::Result {
                    result: Ok(record), ..
                } if record.summary.id == id => Some(record.as_ref()),
                _ => None,
            })
    }
}
