use super::*;

fn response_has_non_identity_content_encoding(resp: &mio_core::HttpResponse) -> bool {
    resp.headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case("content-encoding")
            && header.value.split(|&b| b == b',').any(|part| {
                let token = part.trim_ascii();
                !token.is_empty() && !token.eq_ignore_ascii_case(b"identity")
            })
    })
}

impl Workbench {
    pub(in crate::workbench) fn on_send_and_system_event(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ControlFlow<()> {
        match event {
            Event::SendStarted {
                project,
                tab_id,
                run_id,
                draft_revision,
            } if self.project == Some(project) => {
                let Some(send) = self
                    .sending
                    .get(&tab_id)
                    .filter(|send| send.run_id == run_id && send.draft_revision == draft_revision)
                else {
                    return ControlFlow::Break(());
                };
                if self.transition.is_none() {
                    self.status = if send.cancel.is_cancelled() {
                        "Repeater 正在取消；等待后台确认。"
                    } else {
                        "Repeater 正在发送…"
                    }
                    .into();
                }
                self.invalidate_repeater_history(tab_id, cx);
                self.refresh_repeater_history(window, cx);
            }
            Event::SendProgress {
                project,
                tab_id,
                run_id,
                draft_revision,
                response,
                elapsed_ms,
            } if self.project == Some(project) => {
                let Some(send) = self
                    .sending
                    .get(&tab_id)
                    .filter(|send| send.run_id == run_id && send.draft_revision == draft_revision)
                else {
                    return ControlFlow::Break(());
                };
                if self.workspace.tabs.iter().any(|tab| {
                    tab.id == tab_id
                        && tab.history.as_ref().and_then(|h| h.cursor.as_ref())
                            == send.history_cursor.as_ref()
                }) && let Some(editors) = self.tab_editors.get_mut(&tab_id)
                {
                    let defer_compressed_preview =
                        self.workspace.repeater_settings.unpack_compressed_responses
                            && response_has_non_identity_content_encoding(&response);
                    if !defer_compressed_preview {
                        let was_pretty = editors.response.pretty_active;
                        let preserve_hex = editors.response.view == MessageView::Hex
                            && !editors.response.auto_hex_fallback;
                        editors
                            .response
                            .load(&response_to_raw(&response), window, cx);
                        if was_pretty {
                            let _ = editors.response.switch_pretty(window, cx);
                        } else if preserve_hex && editors.response.view != MessageView::Hex {
                            let _ = editors.response.switch(MessageView::Hex, window, cx);
                        }
                    }
                    editors.response_revision = Some(draft_revision);
                    let result = format!(
                        "正在接收… · {} ms · {} 字节",
                        elapsed_ms,
                        response.body.len()
                    );
                    if self.transition.is_none() && !send.cancel.is_cancelled() {
                        self.status = format!("Repeater {result}");
                    }
                    editors.result = Some(result);
                    cx.notify();
                }
            }
            Event::SendFinished {
                project,
                tab_id,
                draft_revision,
                record,
                persisted,
            } if self.project == Some(project) => {
                self.record_logger_finished(&record, window, cx);
                if self.sending.get(&tab_id).is_none_or(|send| {
                    send.run_id != record.summary.id || send.draft_revision != draft_revision
                }) {
                    return ControlFlow::Break(());
                }
                let send = self.sending.remove(&tab_id).unwrap();
                if let Some(tab) = self.workspace.tabs.iter_mut().find(|tab| {
                    tab.id == tab_id
                        && tab
                            .history
                            .as_ref()
                            .and_then(|history| history.cursor.as_ref())
                            == send.history_cursor.as_ref()
                }) {
                    tab.last_flow = Some(record.summary.id);
                    tab.archive_response = None;
                    tab.history.get_or_insert_with(Default::default).cursor =
                        Some(mio_core::RepeaterHistoryCursor {
                            entry: mio_core::RepeaterHistoryEntryId::Live(record.summary.id),
                            draft_revision,
                        });
                    self.invalidate_repeater_history(tab_id, cx);
                    self.invalidate_archive_response(tab_id, cx);
                    self.repeater_reads.retain(|_, id| *id != tab_id);
                    let mut result = match &record.summary.state {
                        mio_core::FlowState::Failed(reason) => format!("失败：{reason}"),
                        state => format!(
                            "{} · {} ms · {} 字节",
                            state.label(),
                            record.summary.elapsed_ms,
                            record.summary.response_bytes
                        ),
                    };
                    if !persisted {
                        result.push_str("（尚未写入项目）");
                    }
                    if self.transition.is_none() {
                        self.status = format!("Repeater {result}");
                    }
                    if let Some(editors) = self.tab_editors.get_mut(&tab_id) {
                        let was_pretty = editors.response.pretty_active;
                        let preserve_hex = editors.response.view == MessageView::Hex
                            && !editors.response.auto_hex_fallback;
                        editors.response.load(
                            &record
                                .response
                                .as_ref()
                                .map(response_to_raw)
                                .unwrap_or_default(),
                            window,
                            cx,
                        );
                        if was_pretty {
                            let _ = editors.response.switch_pretty(window, cx);
                        } else if preserve_hex && editors.response.view != MessageView::Hex {
                            let _ = editors.response.switch(MessageView::Hex, window, cx);
                        }
                        editors.response_revision = Some(draft_revision);
                        editors.loaded_flow = Some(record.clone());
                        editors.result = Some(result);
                    }
                    if persisted {
                        self.save_workspace(cx);
                    } else {
                        self.workspace_dirty = true;
                    }
                    self.refresh_repeater_history(window, cx);
                    if self.close_after_send.remove(&tab_id) {
                        self.close_tab(tab_id, window, cx);
                    }
                    self.repeater_group_send_finished(
                        tab_id,
                        send.cancel.is_cancelled(),
                        window,
                        cx,
                    );
                } else {
                    self.finish_repeater_history(
                        tab_id,
                        draft_revision,
                        &record,
                        persisted,
                        window,
                        cx,
                    );
                    self.repeater_group_send_finished(
                        tab_id,
                        send.cancel.is_cancelled(),
                        window,
                        cx,
                    );
                }
            }
            Event::SendRejected {
                project,
                tab_id,
                run_id,
                draft_revision,
                message,
            } if self.project == Some(project) => {
                if self.sending.get(&tab_id).is_some_and(|send| {
                    send.run_id == run_id && send.draft_revision == draft_revision
                }) {
                    let send = self.sending.remove(&tab_id).unwrap();
                    self.refresh_archive_response(window, cx);
                    if self.transition.is_none() {
                        self.status = "Repeater 未发送".into();
                    }
                    self.fail(message, cx);
                    if self.close_after_send.remove(&tab_id) {
                        self.close_tab(tab_id, window, cx);
                    }
                    self.repeater_group_send_finished(
                        tab_id,
                        send.cancel.is_cancelled(),
                        window,
                        cx,
                    );
                }
            }
            Event::CaExported { path } => {
                self.status = format!("CA 已导出到 {}；未安装或修改系统信任", path.display());
            }
            Event::CaRegenerated { fingerprint } => {
                self.last_ca_fingerprint = Some(fingerprint.clone());
                self.status = format!("CA 证书已更新（SHA-256: {fingerprint}）");
            }
            Event::ServerCertObserved { project, cert } if self.project == Some(project) => {
                if !self.observed_server_certs.iter().any(|c| {
                    c.host == cert.host
                        && c.port == cert.port
                        && c.sha256_fingerprint == cert.sha256_fingerprint
                }) {
                    if self.observed_server_certs.len() >= 512 {
                        self.observed_server_certs.remove(0);
                    }
                    self.observed_server_certs.push(cert);
                }
            }
            Event::SessionTraceRecorded { project, entry } if self.project == Some(project) => {
                if !self.session_tracer_paused {
                    if self.session_traces.len() >= 500 {
                        self.session_traces.remove(0);
                    }
                    self.session_traces.push(entry);
                }
            }
            Event::TlsPassThroughAutoAdded {
                project,
                host,
                port,
            } if self.project == Some(project) => {
                let target = match port {
                    Some(p) => format!("{host}:{p}"),
                    None => host,
                };
                self.status = format!("客户端 TLS 握手失败，已自动加入 TLS 透传规则：{target}");
            }
            Event::UpdateStatus(status) => {
                match &status {
                    mio_core::UpdateStatus::Available(info) => {
                        self.status = format!(
                            "发现新版本 v{}（可在设置 > 软件更新中下载安装）",
                            info.latest_version
                        );
                    }
                    mio_core::UpdateStatus::UpToDate { current_version } => {
                        self.status = format!("当前已是最新版本 v{current_version}");
                    }
                    mio_core::UpdateStatus::RestartRequired {
                        version,
                        relaunched: true,
                    } => {
                        self.status = format!("v{version} 安装完成，正在重启…");
                        self.update_status = status;
                        cx.notify();
                        #[cfg(not(test))]
                        {
                            let owner = cx.weak_entity();
                            cx.defer(move |cx| {
                                let should_quit = owner
                                    .update(cx, |view, _| !view.shutdown_requested)
                                    .unwrap_or(true);
                                if should_quit {
                                    if let Some(native) =
                                        cx.try_global::<crate::native_terminate::NativeTerminate>()
                                    {
                                        native.request();
                                    } else {
                                        cx.quit();
                                    }
                                }
                            });
                        }
                        return ControlFlow::Continue(());
                    }
                    mio_core::UpdateStatus::Error(err) => {
                        self.status = format!("检查或安装更新失败：{err}");
                    }
                    _ => {}
                }
                self.update_status = status;
                cx.notify();
            }
            Event::PluginOperation {
                project,
                operation_id,
                result,
            } => {
                self.plugin_operation(project, operation_id, result, window, cx);
            }
            Event::PluginPanel {
                identity,
                snapshot,
                editor_bases,
            } => {
                self.plugin_panel(identity, snapshot, &editor_bases, window, cx);
            }
            Event::PluginDiagnostic { identity, message } => {
                self.plugin_diagnostic(identity, message, false, cx);
            }
            Event::PluginStopped { identity, reason } => {
                self.plugin_diagnostic(identity, reason, true, cx);
            }
            Event::PluginWorkspaceRequest(request) => {
                self.plugin_workspace_request(request, window, cx);
            }
            Event::InstalledPlugins {
                project,
                plugins_dir,
                extensions_disabled,
                plugins,
            } => {
                self.handle_installed_plugins(
                    project,
                    plugins_dir,
                    extensions_disabled,
                    plugins,
                    window,
                    cx,
                );
            }
            Event::PluginAutoLoaded {
                project,
                info,
                panel,
                auto_reload,
                order,
            } => {
                self.handle_plugin_auto_loaded(project, info, panel, auto_reload, order, window, cx);
            }
            Event::PluginAutoLoadFailed {
                project,
                plugin_id,
                message,
            } => {
                self.handle_plugin_auto_load_failed(project, plugin_id, message, cx);
            }
            Event::StorageState {
                project,
                pending_records,
                message,
            } if self.project == Some(project) => {
                self.pending_records = pending_records;
                self.storage_error =
                    message.or_else(|| (pending_records > 0).then(|| "流量尚未写入项目。".into()));
                if pending_records > 0 {
                    self.ext_repeater_save_failed("流量尚未写入项目，未载入归档请求。", cx);
                    self.has_traffic = true;
                    self.pending_sends.clear();
                    self.intruder_save_failed(cx);
                    self.plugins_save_failed(cx);
                    self.cancel_transition(cx);
                }
            }
            Event::Error {
                project,
                operation,
                message,
            } if project.is_none() || self.project == project => {
                if matches!(operation, "start_proxy" | "stop_proxy" | "set_intercept") {
                    self.proxy_busy = self
                        .proxy_cancel
                        .as_ref()
                        .is_some_and(CancellationToken::is_cancelled);
                    if self.proxy_busy {
                        self.status = "正在停止 Proxy；等待后台确认。".into();
                    }
                }
                if operation == "query_history" {
                    self.history_loading = false;
                    self.history_dirty = false;
                }
                if matches!(operation, "decide" | "decide_websocket") {
                    for pending in &mut self.intercepts {
                        pending.deciding = false;
                    }
                }
                if matches!(operation, "save_project" | "save_workspace")
                    && !self.intruder_close_owns_workspace()
                {
                    self.fail_intruder_result_close(
                        format!("{operation}：{message}；未丢弃。"),
                        cx,
                    );
                }
                if operation == "save_project" {
                    self.project_saves_in_flight = self.project_saves_in_flight.saturating_sub(1);
                    self.project_notes_save = None;
                    self.project_http_notes_save = None;
                    self.project_http_highlights_save = None;
                    self.notes_save_wait = None;
                    self.project_save_status = "项目保存失败".into();
                    if self.transition != Some(TransitionPhase::Quiescing) {
                        self.cancel_transition(cx);
                    }
                    if self.saving_project_snapshot {
                        self.pending_workspace = None;
                        self.pending_proxy_interception = None;
                        self.saving_project_snapshot = false;
                        self.workspace_dirty = true;
                        self.pending_sends.clear();
                        self.intruder_save_failed(cx);
                        self.plugins_save_failed(cx);
                    }
                }
                if operation == "save_workspace" && !self.intruder_close_owns_workspace() {
                    self.cancel_history_transfer("工作区保存失败；未转送所选请求。", cx);
                    self.tab_groups_save_failed(&message, cx);
                    self.ext_repeater_save_failed("当前工作区保存失败，未载入归档请求。", cx);
                    self.history_workspace_rejected(
                        "当前工作区保存失败，未切换 Repeater 历史。",
                        cx,
                    );
                    self.workspace_dirty = true;
                    self.pending_workspace = None;
                    self.pending_proxy_interception = None;
                    self.pending_sends.clear();
                    self.intruder_save_failed(cx);
                    self.plugins_save_failed(cx);
                    if !self.choosing_save_as
                        && !self
                            .pending_project_save
                            .as_ref()
                            .is_some_and(Option::is_some)
                        && self.transition != Some(TransitionPhase::Quiescing)
                    {
                        self.pending_project_save = None;
                        self.cancel_transition(cx);
                    }
                    self.flush_project_save(window, cx);
                }
                if operation == "quiesce"
                    || (matches!(operation, "start_project" | "new_project" | "open_project")
                        && self.transition == Some(TransitionPhase::Switching))
                {
                    self.cancel_transition(cx);
                }
                if operation == "start_project" {
                    self.startup_wizard_start_failed(&message, cx);
                }
                self.fail(format!("{operation}：{message}"), cx);
            }
            event @ (Event::ExtHttpNotes { .. } | Event::ExtHttpNotesChanged { .. }) => {
                self.on_http_notes(event, window, cx)
            }
            event @ Event::HistoryTransferRead { .. } => {
                self.on_history_transfer(event, window, cx)
            }
            event @ Event::ExtHttpHighlightsChanged { .. } => self.on_http_highlights(event, cx),
            event
            @ (Event::ExtHttpNotesBatch { .. } | Event::ExtHttpNotesBatchChanged { .. }) => {
                self.on_http_notes_batch(event, window, cx)
            }
            Event::Stopped => {
                self.fail_intruder_result_close("后台正在结束；丢弃未确认，数据和窗口仍保留。", cx);
                self.status = "后台正在结束；等待线程与资源回收确认。".into();
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }
}
