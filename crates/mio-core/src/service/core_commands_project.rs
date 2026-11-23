use super::*;

impl State {
    pub(super) async fn command_project_workspace(&mut self, command: Command) -> Result<()> {
        Box::pin(self.command_project_workspace_inner(command)).await
    }

    async fn command_project_workspace_inner(&mut self, command: Command) -> Result<()> {
        match command {
            Command::Quiesce { project } => {
                self.check_project(project)?;
                self.quiesced = true;
                let result = self.stop_resources().await;
                self.flush_ws_repeater_events().await;
                self.flush_sequencer_events().await;
                self.flush_audit_signals().await;
                self.flush_collaborator_events().await;
                self.storage_state().await;
                if let Err(error) = result {
                    if self.unsaved_count() == 0
                        || error.downcast_ref::<mio_proxy::ProxyError>().is_some()
                        || !self.repeater.jobs.is_empty()
                        || !self.ext_archive.can_quiesce()
                        || self.intruder.has_resources()
                        || self.ws_repeater.has_resources()
                        || self.sequencer.has_resources()
                        || self.audit.has_resources()
                        || self.collaborator.has_resources()
                        || self.queries.has_workers()
                    {
                        return Err(error);
                    }
                    self.error("quiesce", error).await;
                }
                self.emit(Event::Quiesced { project }).await;
            }
            Command::Resume { project } => {
                self.check_project(project)?;
                self.quiesced = false;
            }
            Command::StartProject {
                path,
                create_new,
                options,
            } => {
                self.require_saved()?;
                self.store.intruder().require_sessions_resolved()?;
                let project_name =
                    crate::store::startup_config::normalize_project_name(options.name.as_deref())?;
                let effective_config_mode =
                    if create_new && matches!(options.config_mode, StartupDefaultConfig::Project) {
                        ensure!(
                            path.is_none(),
                            "新建磁盘项目尚未包含已保存的项目设置；请选择默认设置或配置文件"
                        );
                        StartupDefaultConfig::Defaults
                    } else {
                        options.config_mode.clone()
                    };
                let loaded_config = match &effective_config_mode {
                    StartupDefaultConfig::ConfigFile { path } => {
                        Some(crate::store::startup_config::load_config_file(path)?)
                    }
                    StartupDefaultConfig::Defaults | StartupDefaultConfig::Project => None,
                };
                let pause_automated_tasks =
                    options.pause_automated_tasks || !options.trust_project_file;
                let disable_extensions = options.disable_extensions || !options.trust_project_file;
                let (next, next_lock, reused_current_lock) = if create_new {
                    let (lock, reused) = if let Some(path) = &path {
                        self.require_ext_released()?;
                        self.store.check_path_outside_temporary(path)?;
                        ensure!(
                            !crate::is_external_archive_file(path),
                            "不能覆盖外部归档项目文件"
                        );
                        crate::store::check_new_project_path(path)?;
                        self.acquire_project_lock_for_switch(path)?
                    } else {
                        (None, false)
                    };
                    let mut store = ProjectStore::create(None)?;
                    store.metadata_mut().apply_startup_options(
                        project_name,
                        &effective_config_mode,
                        loaded_config.as_ref(),
                        &self.cancel,
                    )?;
                    store.checkpoint()?;
                    if let Some(path) = &path {
                        store.save(Some(path))?;
                    }
                    (store, lock, reused)
                } else {
                    let path = path.context("打开已有项目必须提供文件路径")?;
                    self.require_ext_released()?;
                    self.store.check_path_outside_temporary(&path)?;
                    let (mut store, lock, reused) = if crate::is_external_archive_file(&path) {
                        (ProjectStore::open_ext(&path, &self.cancel)?, None, false)
                    } else {
                        let (lock, reused) = self.acquire_project_lock_for_switch(&path)?;
                        (ProjectStore::open(&path)?, lock, reused)
                    };
                    store.metadata_mut().apply_startup_options(
                        project_name,
                        &effective_config_mode,
                        loaded_config.as_ref(),
                        &self.cancel,
                    )?;
                    (store, lock, reused)
                };
                self.replace_project_with(
                    next,
                    next_lock,
                    reused_current_lock,
                    pause_automated_tasks,
                    Some(disable_extensions),
                )
                .await?;
            }
            Command::NewProject { path } => {
                self.require_saved()?;
                self.store.intruder().require_sessions_resolved()?;
                let (next_lock, reused_current_lock) = if let Some(path) = &path {
                    self.require_ext_released()?;
                    self.store.check_path_outside_temporary(path)?;
                    ensure!(
                        !crate::is_external_archive_file(path),
                        "不能覆盖外部归档项目文件"
                    );
                    crate::store::check_new_project_path(path)?;
                    self.acquire_project_lock_for_switch(path)?
                } else {
                    (None, false)
                };
                let next = ProjectStore::create(path.as_deref())?;
                self.replace_project_with(next, next_lock, reused_current_lock, false, Some(false))
                    .await?;
            }
            Command::OpenProject { path } => {
                self.require_saved()?;
                self.store.intruder().require_sessions_resolved()?;
                self.require_ext_released()?;
                self.store.check_path_outside_temporary(&path)?;
                let (next, next_lock, reused_current_lock) =
                    if crate::is_external_archive_file(&path) {
                        (ProjectStore::open_ext(&path, &self.cancel)?, None, false)
                    } else {
                        let (lock, reused) = self.acquire_project_lock_for_switch(&path)?;
                        (ProjectStore::open(&path)?, lock, reused)
                    };
                self.replace_project_with(next, next_lock, reused_current_lock, false, None)
                    .await?;
            }
            Command::SaveProject {
                project,
                path,
                workspace,
                websocket_notes,
                ext_http_notes,
                ext_http_highlights,
            } => {
                self.check_project(project)?;
                self.require_ext_released()?;
                self.retry_session_intruder_recovery()?;
                let query_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
                self.queries.cancel_all();
                self.stop_websocket_query_until(query_deadline).await?;
                self.stop_http_query_until(query_deadline).await?;
                let destination_changed = path
                    .as_deref()
                    .is_some_and(|target| self.store.path.as_deref() != Some(target));
                let destination_lock = match path
                    .as_deref()
                    .filter(|target| self.store.path.as_deref() != Some(*target))
                {
                    Some(target) => {
                        ensure!(
                            !crate::is_external_archive_file(target),
                            "不能覆盖外部归档项目文件"
                        );
                        crate::store::check_new_project_path(target)?;
                        self.store.check_path_outside_temporary(target)?;
                        Some(project_lock::acquire_active_project_lock(target)?)
                    }
                    None => None,
                };
                let prev_workspace = workspace
                    .is_some()
                    .then(|| self.store.metadata().workspace().ok())
                    .flatten();
                let (path, total_bytes, (websocket_notes, ext_http_notes, ext_http_highlights)) =
                    self.store.save_with_notes(
                        path.as_deref(),
                        (
                            workspace.as_ref(),
                            &websocket_notes,
                            &ext_http_notes,
                            &ext_http_highlights,
                        ),
                        self.storage.recovery.make_contiguous(),
                        self.storage.websocket_recovery.make_contiguous(),
                        &self.intruder.recovery,
                        &self.cancel,
                    )?;
                if let Some(new_lock) = destination_lock {
                    self.project_lock = Some(new_lock);
                }
                self.storage.clear_recovery();
                self.intruder.recovery.clear();
                self.store.cancel_queries_with(self.cancel.clone());
                self.ai_ipc.refresh_cancel_if_idle(&self.cancel);
                self.flush_all_provenance_to_store();
                if let Some(workspace) = workspace {
                    Box::pin(self.publish_saved_workspace(
                        project,
                        prev_workspace.as_ref(),
                        workspace,
                    ))
                    .await?;
                }
                self.storage_state().await;
                self.history_changed().await;
                self.websocket_history_changed().await;
                let saved_path = path.clone();
                self.emit(Event::ProjectSaved {
                    project: self.project,
                    path,
                    total_bytes,
                    websocket_notes,
                    ext_http_notes,
                    ext_http_highlights,
                })
                .await;
                self.refresh_project_disk_bytes();
                if destination_changed {
                    self.record_recent_project(&saved_path).await;
                }
            }
            Command::SlimSaveProjectAs {
                project,
                path,
                options,
                cancel,
            } => {
                self.check_project(project)?;
                ensure!(
                    self.store.path.as_deref() != Some(path.as_path()),
                    "瘦身另存为必须选择新的目标路径，不会原地覆盖当前项目"
                );
                let pending_prov = self.storage.provenance.snapshot_all();
                let combined_cancel = self.cancel.child_token();
                let cancel_watcher = cancel.clone();
                let child_trigger = combined_cancel.clone();
                let abort_guard = tokio::spawn(async move {
                    cancel_watcher.cancelled().await;
                    child_trigger.cancel();
                });
                let result = project_lock::acquire_active_project_lock(&path)
                    .and_then(|_slim_lock| {
                        self.store.slim_save_as(
                            &path,
                            &options,
                            self.storage.recovery.make_contiguous(),
                            self.storage.websocket_recovery.make_contiguous(),
                            &self.intruder.recovery,
                            &pending_prov,
                            &combined_cancel,
                        )
                    })
                    .map_err(|err| err.to_string());
                abort_guard.abort();
                self.emit(Event::ProjectSlimSaved {
                    project: self.project,
                    path,
                    result,
                })
                .await;
            }
            Command::SaveWorkspace { project, workspace } => {
                self.check_project(project)?;
                let attempted_revision = workspace.revision;
                match self.apply_save_workspace(project, workspace).await {
                    Ok(()) => {}
                    Err(error) => {
                        self.emit(Event::WorkspaceRejected {
                            project,
                            revision: attempted_revision,
                            stored: self.store.metadata().workspace()?,
                            message: error.to_string(),
                        })
                        .await
                    }
                }
            }
            Command::ExportCa { path } => {
                let ca = self.authority().await?;
                let use_pem = path.extension().is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("pem") || ext.eq_ignore_ascii_case("crt")
                });
                let certificate = if use_pem {
                    ca.certificate_pem().into_bytes()
                } else {
                    ca.certificate_der()
                };
                let output = path.clone();
                tokio::task::spawn_blocking(move || authority::export(&certificate, &output))
                    .await??;
                self.emit(Event::CaExported { path }).await;
            }
            Command::RegenerateCa => {
                let ca = tokio::task::spawn_blocking(authority::regenerate).await??;
                let fingerprint = authority::fingerprint_sha256(&ca.certificate_der());
                self.proxy.ca = Some(ca);
                if self.proxy.is_running() {
                    let workspace = self.store.metadata().workspace()?;
                    self.reload_running_proxy_from_workspace(&workspace).await?;
                }
                self.emit(Event::CaRegenerated { fingerprint }).await;
            }
            Command::ImportCa { cert_pem, key_pem } => {
                let ca = tokio::task::spawn_blocking(move || -> Result<MitmAuthority> {
                    let ca = MitmAuthority::from_pem_pair(&cert_pem, &key_pem)?;
                    authority::save_authority(&ca)?;
                    Ok(ca)
                })
                .await??;
                let fingerprint = authority::fingerprint_sha256(&ca.certificate_der());
                self.proxy.ca = Some(ca);
                if self.proxy.is_running() {
                    let workspace = self.store.metadata().workspace()?;
                    self.reload_running_proxy_from_workspace(&workspace).await?;
                }
                self.emit(Event::CaRegenerated { fingerprint }).await;
            }
            Command::CheckForUpdates { manual } => {
                let fallback_route = match self.updater_fallback_route() {
                    Ok(route) => route,
                    Err(err) => {
                        if manual {
                            self.emit(Event::UpdateStatus(UpdateStatus::Error(format!(
                                "{err:#}"
                            ))))
                            .await;
                        }
                        return Err(err);
                    }
                };
                let events = self.events.clone();
                let cancel = self.cancel.child_token();
                tokio::spawn(async move {
                    if manual {
                        let _ = events.send(Event::UpdateStatus(UpdateStatus::Checking)).await;
                    }
                    let api_url = crate::updater::resolved_api_url();
                    let status = crate::updater::check_for_updates_with_route(
                        &api_url,
                        crate::updater::CURRENT_VERSION,
                        fallback_route,
                        &cancel,
                    )
                    .await;
                    if manual || matches!(status, UpdateStatus::Available(_)) {
                        let _ = events.send(Event::UpdateStatus(status)).await;
                    }
                });
            }
            Command::InstallUpdate { info } => {
                let fallback_route = match self.updater_fallback_route() {
                    Ok(route) => route,
                    Err(err) => {
                        self.emit(Event::UpdateStatus(UpdateStatus::Error(format!(
                            "{err:#}"
                        ))))
                        .await;
                        return Err(err);
                    }
                };
                let events = self.events.clone();
                let cancel = self.cancel.child_token();
                tokio::spawn(async move {
                    let tx = events.clone();
                    let terminal = crate::updater::download_and_install_update_with_route(
                        &info,
                        fallback_route,
                        move |progress| {
                            let _ = tx.try_send(Event::UpdateStatus(progress));
                        },
                        &cancel,
                    )
                    .await;
                    let _ = events.send(Event::UpdateStatus(terminal)).await;
                });
            }
            _ => unreachable!("routed by State::command"),
        }
        Ok(())
    }

    fn updater_fallback_route(&self) -> Result<mio_proxy::UpstreamRoute> {
        let ws = self.store.metadata().workspace()?;
        let tls = self.preferences.effective_upstream_tls(&ws.upstream_tls)?;
        ws.upstream_proxy.to_route_with_tls(tls)
    }

    pub(super) async fn apply_save_workspace(
        &mut self,
        project: ProjectKey,
        workspace: Workspace,
    ) -> Result<()> {
        let prev_workspace = self.store.metadata().workspace().ok();
        let save_outcome = match self.store.metadata_mut().save_workspace(&workspace) {
            Ok(()) => Ok(()),
            Err(first_err) => {
                let matched_ipc_tab = workspace
                    .tabs
                    .iter()
                    .find(|t| {
                        self.ai_ipc
                            .repeater_revisions
                            .contains(&(t.id, workspace.revision))
                    })
                    .map(|t| t.id);
                if let Some(tab_id) = matched_ipc_tab
                    && self
                        .store
                        .metadata()
                        .workspace()
                        .is_ok_and(|stored| stored.revision == workspace.revision)
                {
                    self.ai_ipc
                        .repeater_revisions
                        .remove(&(tab_id, workspace.revision));
                    self.store.metadata_mut().reconcile_ipc_workspace(&workspace)
                } else {
                    Err(first_err)
                }
            }
        };
        save_outcome?;
        Box::pin(self.publish_saved_workspace(project, prev_workspace.as_ref(), workspace)).await?;
        self.refresh_project_disk_bytes();
        Ok(())
    }

    async fn publish_saved_workspace(
        &mut self,
        project: ProjectKey,
        prev_workspace: Option<&Workspace>,
        workspace: Workspace,
    ) -> Result<()> {
        if prev_workspace.is_none_or(|prev| prev.upstream_proxy != workspace.upstream_proxy) {
            workspace
                .upstream_proxy
                .sync_user_level_cache(&mut self.user_level_connections_cache);
        }
        self.sync_compiled_client_certs(&workspace).await;
        self.sync_ws_repeater_workspace(&workspace);
        self.sync_sessions_workspace(&workspace);
        let has_running_proxy = self.proxy.is_running();
        if has_running_proxy && workspace_requires_proxy_rebind(prev_workspace, &workspace) {
            self.reload_running_proxy_from_workspace(&workspace).await?;
        } else if has_running_proxy {
            self.apply_proxy_processing_from_workspace(&workspace)?;
        }
        self.plugin_workspace_changed(&workspace).await;
        self.emit(Event::WorkspaceChanged { project, workspace })
            .await;
        Ok(())
    }

    fn apply_proxy_processing_from_workspace(&self, workspace: &Workspace) -> Result<()> {
        let ssl_matcher =
            crate::http::build_ssl_pass_through_matcher(&workspace.ssl_pass_through_rules)?;
        let ws_transform = crate::http::build_websocket_transform_with_scope(
            workspace.ws_match_and_replace_rules.clone(),
            workspace.proxy_messages.ws_match_replace_in_scope_only,
            workspace.scope.clone(),
        )?;
        let drop_scope_fn = core_commands_proxy::build_drop_out_of_scope_fn(workspace.scope.clone());
        for proxy in self.proxy.handles() {
            proxy.set_processing(
                crate::http::build_flow_transform_with_scope_and_provenance(
                    workspace.match_and_replace_rules.clone(),
                    workspace.proxy_messages,
                    workspace.scope.clone(),
                    Some(self.storage.provenance.clone()),
                )?,
                Some(crate::http::build_intercept_filter(
                    workspace.proxy_interception.clone(),
                    workspace.scope.clone(),
                )?),
                Some(crate::http::build_websocket_intercept_filter(
                    workspace.proxy_websocket_interception,
                    workspace.scope.clone(),
                )?),
            );
            proxy.set_ssl_pass_through(ssl_matcher.clone());
            proxy.set_websocket_transform(ws_transform.clone());
            proxy.set_drop_out_of_scope(drop_scope_fn.clone());
        }
        Ok(())
    }
}

fn workspace_requires_proxy_rebind(prev: Option<&Workspace>, next: &Workspace) -> bool {
    prev.is_none_or(|prev| {
        prev.upstream_proxy != next.upstream_proxy
            || prev.upstream_tls != next.upstream_tls
            || prev.client_certificates != next.client_certificates
            || prev.proxy_listeners != next.proxy_listeners
            || prev.proxy_messages.misc != next.proxy_messages.misc
            || prev.network_http != next.network_http
            || prev.repeater_settings.request_timeout_secs
                != next.repeater_settings.request_timeout_secs
            || prev.repeater_settings.streaming_response_timeout_secs
                != next.repeater_settings.streaming_response_timeout_secs
    })
}
