use super::*;

impl State {
    pub(crate) async fn plugin_job(&mut self, result: Result<Completion, JoinError>) {
        let completion = match result {
            Ok(completion) => completion,
            Err(error) => {
                let Some(operation_id) = self
                    .plugins
                    .operations
                    .iter()
                    .find_map(|(id, op)| (op.task == error.id()).then_some(*id))
                else {
                    return;
                };
                Completion {
                    operation_id,
                    result: Err(format!("插件后台任务异常退出：{error}")),
                }
            }
        };
        let operation_id = completion.operation_id;
        let Some(operation) = self.plugins.operations.remove(&operation_id) else {
            return;
        };
        let close_succeeded = match &completion.result {
            Ok(Output::Retired { retired, .. }) => retired.iter().all(|r| r.result.is_ok()),
            Ok(_) => true,
            Err(_) => false,
        };
        if let Some(key) = operation.closing_panel
            && (close_succeeded || !self.plugins.instances.contains_key(&key.0))
        {
            self.plugins.closing_panels.remove(&key);
        }
        let output = match completion.result {
            Ok(output) => output,
            Err(error) => {
                if let Some(virtual_id) = operation.multiplexed_cleanup_on_fail
                    && let Some((child, parent_handle)) =
                        self.take_multiplexed_child(virtual_id, &error)
                    && let Some(handle) = parent_handle
                {
                    let panel_id = child.panel_id;
                    let source_instance = child.source_instance;
                    let res_id = child.primary_resource;
                    let pending_revoke = handle.begin_revoke_panel_resource(panel_id, res_id, true);
                    self.plugins.closing_panels.insert((source_instance, panel_id));
                    let revoke_op = Uuid::new_v4();
                    self.plugin_job_start(
                        revoke_op,
                        Some(source_instance),
                        CancellationToken::new(),
                        false,
                        async move {
                            pending_revoke.wait().await?;
                            Ok(Output::Revoked)
                        },
                    );
                    if let Some(op) = self.plugins.operations.get_mut(&revoke_op) {
                        op.closing_panel = Some((source_instance, panel_id));
                    }
                }
                self.rollback_plugin_binding(&operation);
                if operation.cleanup_failed
                    && let Some(id) = operation.instance
                {
                    let (retired, retired_multiplexed) = self.take_plugin_family(id, &error);
                    self.plugin_job_start(
                        operation_id,
                        Some(id),
                        operation.cancel,
                        operation.report,
                        retire(retired, retired_multiplexed, None, None, error),
                    );
                } else if operation.report {
                    self.plugin_result(operation.project, operation_id, Err(error))
                        .await;
                } else if let Some(plugin_id) = operation.autoload_plugin_id {
                    if !operation.cancel.is_cancelled()
                        && operation.project == self.project
                        && !self.quiesced
                        && !self.plugins.stopping
                    {
                        self.emit(Event::PluginAutoLoadFailed {
                            project: operation.project,
                            plugin_id,
                            message: error,
                        })
                        .await;
                    }
                } else if let Some(id) = operation.instance {
                    self.emit(Event::PluginDiagnostic {
                        identity: ChannelIdentity {
                            instance_id: id,
                            project: operation.project,
                        },
                        message: error,
                    })
                    .await;
                }
                return;
            }
        };
        // Retirement must persist joined network results even when the initiating operation was cancelled.
        if let Output::Retired {
            retired,
            retired_multiplexed,
            candidate,
            disabled,
            uninstalled_plugin_id,
            reason,
        } = output
        {
            let mut errors = Vec::new();
            for retired in retired {
                for (call, flow_id, result) in retired.network {
                    if let Err(error) = self.finish_plugin_network(call, flow_id, result).await {
                        errors.push(error.to_string());
                    }
                }
                if let Err(error) = retired.result {
                    errors.push(error);
                }
                self.emit(Event::PluginStopped {
                    identity: retired.identity,
                    reason: reason.clone(),
                })
                .await;
            }
            for identity in retired_multiplexed {
                self.emit(Event::PluginStopped {
                    identity,
                    reason: reason.clone(),
                })
                .await;
            }
            if !errors.is_empty() {
                let error = errors.join("；");
                if self.plugins.stopping {
                    self.plugins.cleanup_error.get_or_insert(error.clone());
                }
                if !operation.report {
                    self.error("plugin_cleanup", &error).await;
                }
            }
            if let Some(mut candidate) = candidate {
                if !errors.is_empty()
                    || operation.cancel.is_cancelled()
                    || self.plugins.stopping
                    || self.quiesced
                    || operation.project != self.project
                {
                    if let Some(commit) = candidate.persist_install.take() {
                        commit.staging.rollback();
                    }
                    let message = if errors.is_empty() {
                        "插件加载已取消；候选实例未激活".into()
                    } else {
                        errors.join("；")
                    };
                    self.plugin_job_start(
                        operation_id,
                        None,
                        operation.cancel,
                        operation.report,
                        retire(vec![(candidate.instance, vec![])], vec![], None, None, message),
                    );
                } else {
                    self.activate_plugin(operation_id, *candidate, operation)
                        .await;
                }
            } else if operation.report {
                let result = if !errors.is_empty() {
                    Err(errors.join("；"))
                } else if let Some(plugin_id) = uninstalled_plugin_id {
                    Ok(PluginOutcome::Uninstalled {
                        plugin_id,
                        identity: disabled,
                    })
                } else if let Some(identity) = disabled {
                    Ok(PluginOutcome::Disabled { identity })
                } else {
                    Err(reason)
                };
                self.plugin_result(operation.project, operation_id, result)
                    .await;
            }
            return;
        }
        let invalid = operation.cancel.is_cancelled()
            || operation.project != self.project
            || self.quiesced
            || self.plugins.stopping;
        match output {
            Output::Started(mut candidate) => {
                let replace_valid = candidate
                    .replace
                    .is_none_or(|id| self.plugins.instances.contains_key(&id));
                if invalid || !replace_valid {
                    if let Some(commit) = candidate.persist_install.take() {
                        commit.staging.rollback();
                    }
                    self.plugin_job_start(
                        operation_id,
                        None,
                        operation.cancel,
                        operation.report,
                        retire(
                            vec![(candidate.instance, vec![])],
                            vec![],
                            None,
                            None,
                            "项目、来源插件或加载请求已改变；候选实例已回收".into(),
                        ),
                    );
                } else if let Some(id) = candidate.replace {
                    let (retired, retired_multiplexed) =
                        self.take_plugin_family(id, "插件已重载");
                    self.plugin_job_start(
                        operation_id,
                        Some(id),
                        operation.cancel,
                        operation.report,
                        retire(
                            retired,
                            retired_multiplexed,
                            Some(candidate),
                            None,
                            "插件已重载".into(),
                        ),
                    );
                } else {
                    self.activate_plugin(operation_id, *candidate, operation)
                        .await;
                }
            }
            output => {
                let result = if invalid {
                    Err("插件操作已取消或项目已切换；迟到结果未应用".into())
                } else {
                    match output {
                        Output::Inspected(package) => {
                            if self.plugins.inspections.len() >= 16 {
                                Err("待确认插件包达到 16 个限额；请先加载已检查的包".into())
                            } else {
                                // Re-inspecting a directory replaces only its unused confirmation record.
                                self.plugins
                                    .inspections
                                    .retain(|_, old| old.directory != package.directory);
                                let id = Uuid::new_v4();
                                let inspection = PluginInspection {
                                    id,
                                    directory: package.directory.clone(),
                                    executable: package.executable.clone(),
                                    manifest: package.manifest.clone(),
                                };
                                self.plugins.inspections.insert(id, package);
                                Ok(PluginOutcome::Inspected(inspection))
                            }
                        }
                        Output::Opened {
                            identity,
                            tab_id,
                            revision,
                            loading,
                            panel,
                        } => {
                            let valid = self.plugin_tab(tab_id, revision).and_then(|_| {
                                let (_, _, _, _, resources, _) =
                                    self.plugin_target(identity.instance_id)?;
                                ensure!(
                                    resources.get(&tab_id) == Some(&revision),
                                    "插件资源已撤权；打开结果未应用"
                                );
                                Ok(())
                            });
                            match valid {
                                Err(error) => Err(error.to_string()),
                                Ok(()) => {
                                    let Some((info, panel)) =
                                        self.update_opened_panel(identity, panel).await
                                    else {
                                        return self
                                            .plugin_result(
                                                operation.project,
                                                operation_id,
                                                Err("插件实例已禁用或重载".into()),
                                            )
                                            .await;
                                    };
                                    Ok(if loading {
                                        PluginOutcome::Loaded { info, panel }
                                    } else {
                                        PluginOutcome::Opened {
                                            identity,
                                            tab_id,
                                            draft_revision: revision,
                                        }
                                    })
                                }
                            }
                        }
                        Output::Committed {
                            identity,
                            tab_id,
                            revision,
                            request,
                        } => {
                            match self
                                .plugin_tab(tab_id, revision)
                                .and_then(|_| self.plugin_target(identity.instance_id))
                            {
                                Ok((_, _, _, _, resources, _))
                                    if resources.get(&tab_id) == Some(&revision) =>
                                {
                                    Ok(PluginOutcome::Committed {
                                        identity,
                                        tab_id,
                                        draft_revision: revision,
                                        request,
                                    })
                                }
                                Ok(_) => Err("插件资源已撤权；未发送旧稿".into()),
                                Err(error) => Err(error.to_string()),
                            }
                        }
                        Output::ContextOpened {
                            identity,
                            context_id,
                            revision,
                            panel,
                        } => {
                            self.finish_viewer_open(identity, context_id, revision, panel, false)
                                .await
                        }
                        Output::ContextCommitted {
                            identity,
                            context_id,
                            revision,
                            request,
                        } => self
                            .check_viewer_commit(identity, context_id, revision)
                            .map(|()| PluginOutcome::ContextCommitted {
                                identity,
                                context_id,
                                revision,
                                request,
                            }),
                        Output::ResponseOpened {
                            identity,
                            context_id,
                            revision,
                            panel,
                        } => {
                            self.finish_viewer_open(identity, context_id, revision, panel, true)
                                .await
                        }
                        Output::ResponseCommitted {
                            identity,
                            context_id,
                            revision,
                            response,
                        } => self
                            .check_viewer_commit(identity, context_id, revision)
                            .map(|()| PluginOutcome::ResponseCommitted {
                                identity,
                                context_id,
                                revision,
                                response,
                            }),
                        Output::ContextMenuInvoked {
                            identity,
                            status_message,
                            log_line,
                            open_repeater,
                            replace_request,
                        } => Ok(PluginOutcome::ContextMenuInvoked {
                            identity,
                            status_message,
                            log_line,
                            open_repeater,
                            replace_request,
                        }),
                        Output::PayloadsGenerated {
                            identity,
                            tab_id,
                            payloads,
                        } => Ok(PluginOutcome::PayloadsGenerated {
                            identity,
                            tab_id,
                            payloads,
                        }),
                        Output::PayloadsProcessed {
                            identity,
                            tab_id,
                            position,
                            payloads,
                        } => Ok(PluginOutcome::PayloadsProcessed {
                            identity,
                            tab_id,
                            position,
                            payloads,
                        }),
                        Output::Revoked => Ok(PluginOutcome::EventQueued),
                        Output::Started(_) | Output::Retired { .. } => unreachable!(),
                        Output::PassiveChecked {
                            project,
                            flow_id,
                            url,
                            response,
                            findings,
                        } => {
                            if !invalid {
                                self.finish_passive_checked(
                                    project, flow_id, url, *response, findings,
                                )
                                .await;
                            }
                            return;
                        }
                    }
                };
                if result.is_err()
                    && let Some(virtual_id) = operation.multiplexed_cleanup_on_fail
                    && let Some((child, parent_handle)) =
                        self.take_multiplexed_child(virtual_id, "插件子面板初始化失败")
                    && let Some(handle) = parent_handle
                {
                    let panel_id = child.panel_id;
                    let source_instance = child.source_instance;
                    let res_id = child.primary_resource;
                    let pending_revoke = handle.begin_revoke_panel_resource(panel_id, res_id, true);
                    self.plugins.closing_panels.insert((source_instance, panel_id));
                    let revoke_op = Uuid::new_v4();
                    self.plugin_job_start(
                        revoke_op,
                        Some(source_instance),
                        CancellationToken::new(),
                        false,
                        async move {
                            pending_revoke.wait().await?;
                            Ok(Output::Revoked)
                        },
                    );
                    if let Some(op) = self.plugins.operations.get_mut(&revoke_op) {
                        op.closing_panel = Some((source_instance, panel_id));
                    }
                }
                if result.is_err() {
                    self.rollback_plugin_binding(&operation);
                }
                if operation.cleanup_failed
                    && result.is_err()
                    && let Some(id) = operation.instance
                {
                    let reason = result.err().unwrap();
                    let (retired, retired_multiplexed) = self.take_plugin_family(id, &reason);
                    self.plugin_job_start(
                        operation_id,
                        Some(id),
                        operation.cancel,
                        operation.report,
                        retire(retired, retired_multiplexed, None, None, reason),
                    );
                } else if operation.report {
                    self.plugin_result(operation.project, operation_id, result)
                        .await;
                }
            }
        }
    }

    pub(super) async fn activate_plugin(
        &mut self,
        operation_id: Uuid,
        candidate: Candidate,
        operation: Operation,
    ) {
        let Candidate {
            instance,
            replace,
            reloaded,
            autoload,
            enable_installed,
            persist_install,
        } = candidate;
        let id = instance.info.identity.instance_id;
        let info = instance.info.clone();
        let panel = instance.panel.clone();
        let auto_reload = instance.auto_reload;
        let is_suite = info.tab_id.is_none();
        if let Some(commit) = persist_install {
            let entry = InstalledPluginEntry {
                id: info.manifest.id.clone(),
                version: info.manifest.version.clone(),
                engine: info.manifest.engine.clone(),
                package_dir: commit.staging.package_dir.clone(),
                original_source: commit.original_source,
                grants: info.grants.clone(),
                enabled: true,
                auto_reload,
            };
            match self.plugins.registry.upsert(entry) {
                Ok(()) => commit.staging.commit(),
                Err(err) => {
                    commit.staging.rollback();
                    self.error("plugin_registry", err).await;
                }
            }
        } else if enable_installed {
            if let Err(err) = self.plugins.registry.set_enabled(&info.manifest.id, true) {
                self.error("plugin_registry", err).await;
            }
        } else if reloaded
            && is_suite
            && let Err(err) = self.plugins.registry.update_loaded_metadata(
                &info.manifest.id,
                &info.manifest.version,
                &info.grants,
            )
        {
            self.error("plugin_registry", err).await;
        }
        self.plugins.instances.insert(id, instance);
        if is_suite {
            if !self.plugins.order.contains(&id) {
                self.plugins.order.push(id);
            }
            self.sort_plugin_order_by_registry();
            self.sync_proxy_plugin_hooks();
        }
        if autoload {
            self.emit(Event::PluginAutoLoaded {
                project: operation.project,
                info,
                panel,
                auto_reload,
                order: self.plugins.order.clone(),
            })
            .await;
        } else if operation.report {
            let outcome = if enable_installed {
                PluginOutcome::InstalledEnabled {
                    info,
                    panel,
                    auto_reload,
                    order: self.plugins.order.clone(),
                }
            } else if let (true, Some(old_instance)) = (reloaded, replace) {
                PluginOutcome::Reloaded {
                    old_instance,
                    info,
                    panel,
                }
            } else {
                PluginOutcome::Loaded { info, panel }
            };
            self.plugin_result(operation.project, operation_id, Ok(outcome))
                .await;
        }
    }
}
