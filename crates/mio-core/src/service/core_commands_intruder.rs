use super::*;

impl State {
    pub(super) async fn command_intruder(&mut self, command: Command) -> Result<()> {
        match command {
            Command::AutoIntruderPositions {
                project,
                operation_id,
                raw,
                selection,
                placement,
            } => {
                let positions = async {
                    self.check_project(project)?;
                    tokio::task::spawn_blocking(move || {
                        intruder::automatic_positions(&raw, selection, placement)
                    })
                    .await
                    .context("Intruder 自动位置解析任务失败")?
                }
                .await
                .map_err(|error| error.to_string());
                self.queue_intruder_event(Event::IntruderPositions {
                    project,
                    operation_id,
                    positions,
                });
            }
            Command::StartIntruder {
                project,
                run_id,
                tab_id,
                spec,
                cancel,
            } => {
                self.start_intruder(project, run_id, tab_id, *spec, cancel)
                    .await;
            }
            Command::StopIntruder { project, run_id } => {
                if let Err(error) = self.request_intruder_stop(project, run_id) {
                    self.queue_intruder_event(Event::Error {
                        project: Some(project),
                        operation: "stop_intruder",
                        message: error.to_string(),
                    });
                }
            }
            Command::SetIntruderPaused {
                project,
                run_id,
                operation_id,
                paused,
            } => {
                let result = self
                    .set_intruder_paused(project, run_id, paused)
                    .map_err(|error| error.to_string());
                self.queue_intruder_event(Event::IntruderRunControl {
                    project,
                    run_id,
                    operation_id,
                    result,
                });
            }
            Command::SetIntruderRetention {
                project,
                run_id,
                operation_id,
                expected_revision,
                retention,
                workspace,
            } => {
                let result = self
                    .set_intruder_retention(
                        project,
                        run_id,
                        expected_revision,
                        retention,
                        *workspace,
                    )
                    .map_err(|error| error.to_string());
                let changed = result.is_ok();
                if let Ok(receipt) = &result {
                    self.plugin_workspace_changed(&receipt.workspace).await;
                }
                self.queue_intruder_event(Event::IntruderRetentionChanged {
                    project,
                    run_id,
                    operation_id,
                    result,
                });
                if changed {
                    self.intruder_discarded();
                }
            }
            Command::RequestIntruderBaseline {
                project,
                run_id,
                operation_id,
                expected_revision,
                cancel,
            } => {
                let result = self
                    .request_intruder_baseline(
                        project,
                        run_id,
                        operation_id,
                        expected_revision,
                        &cancel,
                    )
                    .map_err(|error| error.to_string());
                self.queue_intruder_event(Event::IntruderBaselineRequested {
                    project,
                    run_id,
                    operation_id,
                    result,
                });
            }
            Command::QueryIntruderResourcePools {
                project,
                query_revision,
            } => {
                let result = self
                    .intruder_resource_pools(project)
                    .map_err(|error| error.to_string());
                self.queue_intruder_event(Event::IntruderResourcePools {
                    project,
                    query_revision,
                    result,
                });
            }
            Command::QueryIntruderPoolUsage {
                project,
                query_revision,
            } => {
                let result = self
                    .intruder_pool_usage(project)
                    .map_err(|error| error.to_string());
                self.queue_intruder_event(Event::IntruderPoolUsage {
                    project,
                    query_revision,
                    result,
                });
            }
            Command::DeleteIntruderResourcePool {
                project,
                operation_id,
                pool_id,
                expected_catalog_revision,
                expected_workspace_revision,
                allow_reassign,
                cancel,
            } => {
                let result = self.delete_intruder_resource_pool(
                    project,
                    operation_id,
                    pool_id,
                    (expected_catalog_revision, expected_workspace_revision),
                    allow_reassign,
                    &cancel,
                );
                self.queue_intruder_event(Event::IntruderResourcePoolDeleted {
                    project,
                    operation_id,
                    pool_id,
                    result,
                });
            }
            Command::EnsureIntruderDefaultPool {
                project,
                operation_id,
                cancel,
            } => {
                let result = self
                    .ensure_intruder_default_pool(project, operation_id, &cancel)
                    .map_err(|error| error.to_string());
                self.queue_intruder_event(Event::IntruderDefaultPoolReady {
                    project,
                    operation_id,
                    result,
                });
            }
            Command::SetIntruderResourcePools {
                project,
                operation_id,
                expected_revision,
                pools,
                cancel,
            } => {
                let result = self
                    .set_intruder_resource_pools(
                        project,
                        operation_id,
                        expected_revision,
                        pools,
                        &cancel,
                    )
                    .map_err(|error| error.to_string());
                self.queue_intruder_event(Event::IntruderResourcePoolsUpdated {
                    project,
                    operation_id,
                    result,
                });
            }
            Command::SetIntruderResourcePool {
                project,
                run_id,
                operation_id,
                expected_pool_revision,
                resource_pool,
                cancel,
            } => {
                let result = self
                    .set_intruder_resource_pool(
                        project,
                        run_id,
                        operation_id,
                        expected_pool_revision,
                        resource_pool,
                        &cancel,
                    )
                    .map_err(|error| error.to_string());
                self.queue_intruder_event(Event::IntruderResourcePoolChanged {
                    project,
                    run_id,
                    operation_id,
                    result,
                });
            }
            Command::RenameIntruder {
                project,
                run_id,
                operation_id,
                expected_title_revision,
                title,
            } => {
                let result = self
                    .rename_intruder(
                        project,
                        run_id,
                        operation_id,
                        expected_title_revision,
                        &title,
                    )
                    .map_err(|error| error.to_string());
                self.queue_intruder_event(Event::IntruderRenamed {
                    project,
                    run_id,
                    operation_id,
                    result,
                });
            }
            Command::DiscardIntruder {
                project,
                run_id,
                operation_id,
                expected_revision,
                workspace,
            } => {
                let result = self
                    .discard_intruder(project, run_id, expected_revision, *workspace)
                    .map_err(|error| error.to_string());
                let changed = result.is_ok();
                if let Ok(receipt) = &result {
                    self.plugin_workspace_changed(&receipt.workspace).await;
                }
                self.queue_intruder_event(Event::IntruderDiscarded {
                    project,
                    run_id,
                    operation_id,
                    result,
                });
                if changed {
                    self.intruder_discarded();
                }
            }
            Command::QueryIntruderResults {
                project,
                run_id,
                query_revision,
                after,
                limit,
            } => {
                let result = self
                    .check_project(project)
                    .and_then(|()| self.intruder_result_page(run_id, after, limit))
                    .map_err(|error| error.to_string());
                self.queue_intruder_event(Event::IntruderResults {
                    project,
                    run_id,
                    query_revision,
                    result,
                });
            }
            Command::QueryIntruderResultView { query, cancel } => {
                self.query_intruder_result_view(query, cancel).await;
            }
            Command::SetIntruderResultNotes {
                project,
                run_id,
                ordinal,
                expected_notes_revision,
                notes,
                operation_id,
            } => {
                let result = self
                    .set_intruder_result_notes(
                        project,
                        run_id,
                        ordinal,
                        expected_notes_revision,
                        notes,
                    )
                    .map_err(|error| error.to_string());
                let (notes_revision, notes) = match &result {
                    Ok((revision, notes)) => (*revision, notes.clone()),
                    Err(_) => (0, Vec::new()),
                };
                self.queue_intruder_event(Event::IntruderResultNotes {
                    project,
                    operation_id,
                    run_id,
                    ordinal,
                    notes_revision,
                    notes,
                    result: result.map(|_| ()),
                });
            }
            Command::SetIntruderResultHighlight {
                project,
                run_id,
                entries,
                highlight,
                operation_id,
            } => {
                let result = self
                    .set_intruder_result_highlight(project, run_id, entries.clone(), highlight)
                    .map_err(|error| error.to_string());
                let updated = result.clone().unwrap_or_default();
                self.queue_intruder_event(Event::IntruderResultHighlight {
                    project,
                    operation_id,
                    run_id,
                    updated,
                    highlight,
                    result: result.map(|_| ()),
                });
            }
            Command::QueryIntruderRuns {
                project,
                query_revision,
                after,
                limit,
            } => {
                let result = self
                    .check_project(project)
                    .and_then(|()| self.intruder_run_page(after, limit))
                    .map_err(|error| error.to_string());
                self.queue_intruder_event(Event::IntruderRuns {
                    project,
                    query_revision,
                    result,
                });
            }
            Command::ReadIntruderSpec {
                project,
                run_id,
                selection_revision,
            } => {
                let result = self
                    .check_project(project)
                    .and_then(|()| self.store.read_intruder_spec(run_id))
                    .map(Box::new)
                    .map_err(|error| error.to_string());
                self.queue_intruder_event(Event::IntruderSpec {
                    project,
                    run_id,
                    selection_revision,
                    result,
                });
            }
            _ => unreachable!("routed by State::command"),
        }
        Ok(())
    }
}
