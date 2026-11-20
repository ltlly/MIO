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
        let output = match completion.result {
            Ok(output) => output,
            Err(error) => {
                self.rollback_plugin_binding(&operation);
                if operation.cleanup_failed
                    && let Some(id) = operation.instance
                {
                    let retired = self.take_plugin_family(id, &error);
                    self.plugin_job_start(
                        operation_id,
                        Some(id),
                        operation.cancel,
                        operation.report,
                        retire(retired, None, None, error),
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
                        retire(vec![(candidate.instance, vec![])], None, None, message),
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
                let source_valid = candidate
                    .instance
                    .source
                    .is_none_or(|source| self.plugins.instances.contains_key(&source));
                let replace_valid = candidate
                    .replace
                    .is_none_or(|id| self.plugins.instances.contains_key(&id));
                if invalid || !source_valid || !replace_valid {
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
                            None,
                            None,
                            "项目、来源插件或加载请求已改变；候选实例已回收".into(),
                        ),
                    );
                } else if let Some(id) = candidate.replace {
                    let retired = self.take_plugin_family(id, "插件已重载");
                    self.plugin_job_start(
                        operation_id,
                        Some(id),
                        operation.cancel,
                        operation.report,
                        retire(retired, Some(candidate), None, "插件已重载".into()),
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
                                let inst = self.plugin_instance(identity.instance_id)?;
                                ensure!(
                                    inst.resources.get(&tab_id) == Some(&revision),
                                    "插件资源已撤权；打开结果未应用"
                                );
                                Ok(())
                            });
                            match valid {
                                Err(error) => Err(error.to_string()),
                                Ok(()) => {
                                    self.plugin_panel(identity, panel, vec![]).await;
                                    let inst = &self.plugins.instances[&identity.instance_id];
                                    Ok(if loading {
                                        PluginOutcome::Loaded {
                                            info: inst.info.clone(),
                                            panel: inst.panel.clone(),
                                        }
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
                                .and_then(|_| self.plugin_instance(identity.instance_id))
                            {
                                Ok(instance)
                                    if instance.resources.get(&tab_id) == Some(&revision) =>
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
                if result.is_err() {
                    self.rollback_plugin_binding(&operation);
                }
                if operation.cleanup_failed
                    && result.is_err()
                    && let Some(id) = operation.instance
                {
                    let reason = result.err().unwrap();
                    let retired = self.take_plugin_family(id, &reason);
                    self.plugin_job_start(
                        operation_id,
                        Some(id),
                        operation.cancel,
                        operation.report,
                        retire(retired, None, None, reason),
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
            open,
            context_open,
            response_open,
        } = candidate;
        let id = instance.info.identity.instance_id;
        let info = instance.info.clone();
        let panel = instance.panel.clone();
        let auto_reload = instance.auto_reload;
        let is_suite = info.tab_id.is_none() && context_open.is_none() && response_open.is_none();
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
        if let Some((tab_id, revision)) = open {
            if let Err(error) = self.start_plugin_open(
                operation_id,
                id,
                tab_id,
                revision,
                true,
                operation.cancel.clone(),
            ) {
                let reason = error.to_string();
                let retired = self.take_plugin_family(id, &reason);
                self.plugin_job_start(
                    operation_id,
                    Some(id),
                    operation.cancel,
                    operation.report,
                    retire(retired, None, None, reason),
                );
            }
        } else if context_open.is_some() || response_open.is_some() {
            let (context_id, revision, read_only) = context_open
                .as_ref()
                .map(|(c, r, ro, _)| (*c, *r, *ro))
                .unwrap_or_else(|| {
                    let (c, r, ro, _) = response_open.as_ref().unwrap();
                    (*c, *r, *ro)
                });
            self.plugins
                .context_viewers
                .insert(id, (context_id, revision, read_only));
            let handle = self.plugins.instances[&id].worker.handle();
            let identity = info.identity;
            let task_cancel = operation.cancel.clone();
            self.plugin_job_start(operation_id, Some(id), operation.cancel, operation.report, async move {
                let opened = async {
                    if let Some((_, _, _, req)) = context_open {
                        let panel = handle.open_request(context_id, revision, read_only, req).await?;
                        Ok(Output::ContextOpened { identity, context_id, revision, panel })
                    } else {
                        let (_, _, _, resp) = response_open.unwrap();
                        let panel = handle.open_response(context_id, revision, read_only, resp).await?;
                        Ok(Output::ResponseOpened { identity, context_id, revision, panel })
                    }
                };
                tokio::pin!(opened);
                tokio::select! {
                    biased;
                    _ = task_cancel.cancelled() => { handle.cancel(); anyhow::bail!("打开插件上下文已取消；实例正在回收"); }
                    result = &mut opened => result,
                }
            });
            if let Some(op) = self.plugins.operations.get_mut(&operation_id) {
                op.cleanup_failed = true;
            }
        } else if autoload {
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
