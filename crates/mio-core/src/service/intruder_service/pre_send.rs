use super::*;

impl State {
    pub(super) fn spawn_intruder_pre_send(
        &mut self,
        id: Uuid,
        ordinal: u64,
        record: FlowRecord,
        config: SendConfig,
        response_codes: Vec<u16>,
    ) {
        let session_hooks = self.strict_session_action_hooks();
        let http_hooks = self.tool_http_hooks();
        let sessions = self.sessions.clone();
        let project = self.project;
        let run = self.intruder.runs.get_mut(&id).unwrap();
        run.preparing = true;
        let cancel = run.cancel.clone();
        let job = self.intruder.prep_jobs.spawn(async move {
            let mut record = record;
            let started = Instant::now();
            let deadline = started + config.timeout;
            if sessions
                .apply_to_flow_record(
                    SessionTool::Intruder,
                    &mut record,
                    &config,
                    &cancel,
                    deadline,
                    started,
                )
                .await
                .is_none()
            {
                // First admission of a pre-send terminal failure keeps revision 1.
                record.summary.revision = 1;
                return PrepCompletion {
                    project,
                    run_id: id,
                    ordinal,
                    record,
                    config,
                    response_codes,
                };
            }
            if !session_hooks.is_empty() || !http_hooks.is_empty() {
                let mut current = record.request.clone();
                let flow_id = record.summary.id;
                match plugin_service::hooks::run_pre_send_plugin_hooks(
                    &session_hooks,
                    &http_hooks,
                    flow_id,
                    protocol::SourceDto::Intruder,
                    &mut current,
                    deadline,
                    &cancel,
                )
                .await
                {
                    plugin_service::hooks::PreSendHookOutcome::Proceed { .. } => {
                        plugin_service::hooks::sync_flow_request(&mut record, current);
                    }
                    plugin_service::hooks::PreSendHookOutcome::Dropped => {
                        plugin_service::hooks::sync_flow_request(&mut record, current);
                        record.summary.state = FlowState::Dropped;
                        record.summary.revision = 1;
                        record.summary.elapsed_ms =
                            started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                        return PrepCompletion {
                            project,
                            run_id: id,
                            ordinal,
                            record,
                            config,
                            response_codes,
                        };
                    }
                    plugin_service::hooks::PreSendHookOutcome::Failed(message) => {
                        plugin_service::hooks::sync_flow_request(&mut record, current);
                        record.summary.state = if cancel.is_cancelled() {
                            FlowState::Cancelled
                        } else {
                            FlowState::Failed(message)
                        };
                        record.summary.revision = 1;
                        record.summary.elapsed_ms =
                            started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                        return PrepCompletion {
                            project,
                            run_id: id,
                            ordinal,
                            record,
                            config,
                            response_codes,
                        };
                    }
                }
            }
            PrepCompletion {
                project,
                run_id: id,
                ordinal,
                record,
                config,
                response_codes,
            }
        });
        self.intruder.prep_requests.insert(job.id(), (id, ordinal));
    }

    pub(super) async fn complete_intruder_prep(
        &mut self,
        result: std::result::Result<(Id, PrepCompletion), JoinError>,
    ) -> Result<()> {
        let task_id = match &result {
            Ok((id, _)) => *id,
            Err(error) => error.id(),
        };
        let (run_id, ordinal) = self
            .intruder
            .prep_requests
            .remove(&task_id)
            .context("Intruder 预处理任务身份不匹配")?;
        let Some(run) = self.intruder.runs.get_mut(&run_id) else {
            return Ok(());
        };
        run.preparing = false;
        let prep = match result {
            Ok((_, prep)) => {
                if prep.project != self.project || prep.run_id != run_id || prep.ordinal != ordinal
                {
                    return Ok(());
                }
                prep
            }
            Err(error) => {
                let message = format!("Intruder 会话预处理异常退出，未发送请求：{error}");
                run.failure.get_or_insert_with(|| message.clone());
                run.cancel.cancel();
                self.persist_intruder_state(run_id, IntruderRunState::Stopping)?;
                return Err(anyhow::anyhow!(message));
            }
        };
        self.admit_and_spawn_intruder_send(
            run_id,
            ordinal,
            prep.record,
            prep.config,
            prep.response_codes,
        )
    }
}
