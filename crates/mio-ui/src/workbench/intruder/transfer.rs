use super::*;

mod copy_scope;

impl Workbench {
    pub(in crate::workbench) fn open_in_intruder(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.start_history_transfer(true, window, cx) {
            return;
        }
        if self.transition.is_some()
            || self.backend_disconnected
            || !self.flush_workspace(window, cx)
        {
            return;
        }
        if self.tool == Tool::Proxy && self.proxy_page == ProxyPage::History {
            if matches!(self.selected_history, Some(HistoryEntryId::External { .. })) {
                self.open_ext_history_transfer(true, window, cx);
                return;
            }
            let (Some(project), Some(flow_id)) = (self.project, self.selected_flow) else {
                return;
            };
            let Some(revision) = self.selection_revision.checked_add(1) else {
                self.fail("消息读取 revision 已耗尽；请重新打开项目。", cx);
                return;
            };
            if self.enqueue(
                Command::ReadFlow {
                    project,
                    selection_revision: revision,
                    flow_id,
                },
                cx,
            ) {
                self.selection_revision = revision;
                self.intruder
                    .transfer_reads
                    .insert(revision, (flow_id, self.history_original_view));
                self.status = "正在读取发送到 Intruder 的请求…".into();
            }
            cx.notify();
            return;
        }
        match self.current_transfer_draft("发送到 Intruder", window, cx) {
            Ok(draft) => self.open_intruder_draft(draft, window, cx),
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn has_intruder_transfer(&self, revision: u64) -> bool {
        self.intruder.transfer_reads.contains_key(&revision)
    }

    pub(in crate::workbench) fn receive_intruder_transfer_error(
        &mut self,
        revision: u64,
        flow_id: FlowId,
        message: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self
            .intruder
            .transfer_reads
            .get(&revision)
            .is_some_and(|(expected, _)| *expected == flow_id)
        {
            return false;
        }
        self.intruder.transfer_reads.remove(&revision);
        self.fail(
            format!("无法读取发送到 Intruder 的请求：{message}；未创建标签。"),
            cx,
        );
        true
    }

    pub(in crate::workbench) fn receive_intruder_transfer(
        &mut self,
        revision: u64,
        record: &FlowRecord,
        original: &OriginalMessages,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(&(flow_id, use_original)) = self.intruder.transfer_reads.get(&revision) else {
            return;
        };
        if record.summary.id != flow_id {
            return;
        }
        self.intruder.transfer_reads.remove(&revision);
        if self.transition.is_some() {
            self.fail("项目正在切换；读取的请求未自动带入新项目。", cx);
            return;
        }
        let request = if use_original {
            original.request.as_ref().unwrap_or(&record.request)
        } else {
            &record.request
        };
        self.open_intruder_draft(
            Draft {
                revision: 1,
                target: request.url.clone(),
                raw: request_to_raw(request),
                version: request.version,
            },
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn current_transfer_draft(
        &self,
        action_label: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Draft, String> {
        match self.tool {
            Tool::Intruder => {
                let id = self
                    .workspace
                    .intruder
                    .active
                    .ok_or("没有选中的 Intruder 请求。")?;
                Ok(self.snapshot_intruder_spec(id, window, cx)?.draft)
            }
            Tool::Repeater => self
                .workspace
                .tabs
                .iter()
                .find(|tab| Some(tab.id) == self.workspace.active)
                .map(|tab| tab.draft.clone())
                .ok_or_else(|| "没有选中的 Repeater 请求。".into()),
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => {
                let pending = self
                    .intercepts
                    .iter()
                    .find(|pending| Some(pending.id()) == self.selected_intercept)
                    .ok_or("没有选中的拦截请求。")?;
                match &pending.target {
                    InterceptTarget::Http { stage, record, .. } => Ok(Draft {
                        revision: 1,
                        target: record.request.url.clone(),
                        raw: if matches!(stage, InterceptStage::Request) {
                            pending.editor.bytes(window, cx)?
                        } else {
                            request_to_raw(&record.request)
                        },
                        version: record.request.version,
                    }),
                    InterceptTarget::WebSocket { .. } => Err(format!(
                        "当前选中的是 WebSocket 消息，暂不支持{action_label}。"
                    )),
                }
            }
            Tool::Logger => {
                let flow_id = self
                    .extra_tools
                    .logger_selected
                    .ok_or("没有选中的 Logger 流量记录。")?;
                let (record, _) = self
                    .extra_tools
                    .logger_loaded_detail
                    .as_ref()
                    .filter(|(record, _)| record.summary.id == flow_id)
                    .ok_or("Logger 请求详情尚未就绪。")?;
                let editor_bytes = self.extra_tools.logger_request.bytes(window, cx)?;
                Ok(Draft {
                    revision: 1,
                    target: record.request.url.clone(),
                    raw: if editor_bytes.is_empty() {
                        request_to_raw(&record.request)
                    } else {
                        editor_bytes
                    },
                    version: record.request.version,
                })
            }
            Tool::Proxy if self.proxy_page == ProxyPage::History => {
                if let Some(record) = self.detail.as_ref() {
                    let raw = self.history_request.bytes(window, cx)?;
                    return Ok(Draft {
                        revision: 1,
                        target: record.request.url.clone(),
                        raw: if raw.is_empty() {
                            request_to_raw(&record.request)
                        } else {
                            raw
                        },
                        version: record.request.version,
                    });
                }
                if let Some(HistoryRow::ExtProxy(row)) = self
                    .imported_history
                    .row
                    .as_ref()
                    .filter(|row| self.selected_history == Some(row.id()))
                {
                    let raw = self.history_request.bytes(window, cx)?;
                    if !raw.is_empty() {
                        let req = row.request.as_ref().or(row.edited_request.as_ref());
                        let target = req
                            .and_then(|r| r.request_target.as_deref())
                            .map(|t| {
                                if t.starts_with('/') {
                                    format!("{}{t}", row.origin.as_deref().unwrap_or_default())
                                } else {
                                    t.to_owned()
                                }
                            })
                            .unwrap_or_default();
                        return Ok(Draft {
                            revision: 1,
                            target,
                            raw,
                            version: HttpVersion::Http11,
                        });
                    }
                }
                Err("请先选中一条已加载详情的 HTTP 历史记录。".into())
            }
            Tool::Target => {
                let row = self
                    .target_pane
                    .selected_row
                    .as_ref()
                    .ok_or("没有选中的站点地图请求。")?;
                let raw = self.target_pane.request.bytes(window, cx)?;
                if raw.is_empty() {
                    return Err("站点地图请求报文尚未加载完成。".into());
                }
                let target = match row {
                    HistoryRow::Live(summary) => summary.url.clone(),
                    HistoryRow::ExtProxy(archive) => {
                        let req = archive.request.as_ref().or(archive.edited_request.as_ref());
                        req.and_then(|r| r.request_target.as_deref())
                            .map(|t| {
                                if t.starts_with('/') {
                                    format!("{}{t}", archive.origin.as_deref().unwrap_or_default())
                                } else {
                                    t.to_owned()
                                }
                            })
                            .unwrap_or_default()
                    }
                };
                Ok(Draft {
                    revision: 1,
                    target,
                    raw,
                    version: self
                        .target_pane
                        .selected_explicit_detail
                        .as_ref()
                        .map(|detail| detail.record.request.version)
                        .or_else(|| {
                            self.target_pane
                                .selected_live_record
                                .as_ref()
                                .map(|record| record.request.version)
                        })
                        .or_else(|| match row {
                            HistoryRow::ExtProxy(archive) => archive
                                .edited_request
                                .as_ref()
                                .or(archive.request.as_ref())
                                .and_then(|request| request.declared_version),
                            _ => None,
                        })
                        .ok_or("站点地图请求版本尚未读取完成。")?,
                })
            }
            Tool::Sequencer => {
                let pane = &self.sequencer_pane;
                let raw = pane.request.bytes(window, cx)?;
                if raw.is_empty() {
                    return Err("Sequencer 采样源请求为空。".into());
                }
                let target = editing::input_snapshot(&pane.target_url, window, cx)
                    .map_err(str::to_owned)?
                    .trim()
                    .to_owned();
                Ok(Draft {
                    revision: 1,
                    target,
                    raw,
                    version: pane.request_version,
                })
            }
            Tool::Dashboard => {
                if self.audit_dashboard.new_scan_open {
                    let pane = &self.audit_dashboard;
                    let raw = pane.scan_request_editor.bytes(window, cx)?;
                    let target = editing::input_snapshot(&pane.scan_url_input, window, cx)
                        .map_err(str::to_owned)?
                        .trim()
                        .to_owned();
                    if raw.is_empty() {
                        return Err("扫描向导种子请求为空。".into());
                    }
                    return Ok(Draft {
                        revision: 1,
                        target,
                        raw,
                        version: pane.scan_version,
                    });
                }
                if !self.has_dashboard_intruder_selection()
                    && let Some(finding_id) = self.audit_dashboard.selected_finding_id
                    && let Some(finding) = self
                        .audit_dashboard
                        .snapshot
                        .findings
                        .iter()
                        .find(|finding| finding.id == finding_id)
                    && let Some(record) = self
                        .audit_dashboard
                        .loaded_flow
                        .as_ref()
                        .filter(|record| Some(record.summary.id) == finding.flow_id)
                {
                    let editor_bytes = self
                        .audit_dashboard
                        .issue_request_editor
                        .bytes(window, cx)?;
                    return Ok(Draft {
                        revision: 1,
                        target: record.request.url.clone(),
                        raw: if editor_bytes.is_empty() {
                            request_to_raw(&record.request)
                        } else {
                            editor_bytes
                        },
                        version: record.request.version,
                    });
                }
                Err("当前 Dashboard 没有活动的扫描或漏洞关联请求。".into())
            }
            Tool::Organiser => {
                let item = self
                    .organiser
                    .selected_id
                    .and_then(|id| {
                        self.workspace
                            .organiser_items
                            .iter()
                            .find(|item| item.id == id)
                    })
                    .ok_or("没有选中的 Organiser 条目。")?;
                let target = if !item.target.is_empty() {
                    item.target.clone()
                } else if let Some((scheme, tail)) = item.url.split_once("://") {
                    let authority_end = tail.find(['/', '?', '#']).unwrap_or(tail.len());
                    format!("{scheme}://{}", &tail[..authority_end])
                } else {
                    item.target.clone()
                };
                Ok(Draft {
                    revision: 1,
                    target,
                    raw: item.request_raw.clone(),
                    version: item.http_version,
                })
            }
            _ => Err("当前上下文没有可发送的 HTTP 请求。".into()),
        }
    }

    pub(in crate::workbench) fn send_to_sequencer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() {
            return;
        }
        let draft = match self.current_transfer_draft("发送到 Sequencer", window, cx) {
            Ok(draft) => draft,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        self.cancel_plugin_interactions(cx);
        self.send_draft_to_sequencer(draft, window, cx);
    }

    pub(in crate::workbench) fn send_to_scanner(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() {
            return;
        }
        let draft = match self.current_transfer_draft("发起主动扫描", window, cx) {
            Ok(draft) => draft,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        self.cancel_plugin_interactions(cx);
        self.send_draft_to_scanner(
            draft,
            mio_core::AuditTaskMode::PassiveAndActive,
            true,
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn copy_draft_to_repeater(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.flush_workspace(window, cx)
        {
            return;
        }
        let mut draft = match self.current_transfer_draft("发送到 Repeater", window, cx) {
            Ok(draft) => draft,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        draft.revision = 1;
        let id = Uuid::new_v4();
        let previous = self.workspace.active;
        let title = self.next_repeater_tab_title(None, Some(&draft.raw));
        self.workspace.tabs.push(WorkTab {
            id,
            title,
            group: None,
            draft,
            last_flow: None,
            archive_response: None,
            history: None,
        });
        self.workspace.active = Some(id);
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.tabs.pop();
            self.workspace.active = previous;
            self.fail(format!("未创建 Repeater 标签：{error}"), cx);
            return;
        }
        self.cancel_plugin_interactions(cx);
        self.sync_tab_editors(window, cx);
        self.save_workspace(cx);
        self.tool = Tool::Repeater;
        self.tab_editors[&id]
            .request
            .state()
            .update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }
}
