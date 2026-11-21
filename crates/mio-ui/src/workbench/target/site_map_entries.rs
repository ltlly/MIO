use super::*;
use gpui_kit::component::menu::DropdownMenu as _;

pub(super) fn finding_severity_dot(rank: u8) -> gpui_kit::Hsla {
    match rank {
        3.. => rgb(0xd93025).into(),
        2 => rgb(0xe37400).into(),
        1 => rgb(0x1a73e8).into(),
        _ => rgb(0x5f6368).into(),
    }
}

pub(super) fn synthetic_scan_draft(method: &str, url: &str) -> Draft {
    let clean_method = if method.trim().is_empty() {
        "GET"
    } else {
        method.trim()
    };
    let (host, path) = if let Some((_, rest)) = url.trim().split_once("://") {
        let without_frag = rest.split('#').next().unwrap_or(rest);
        match without_frag.split_once('/') {
            Some((auth, tail)) => (
                if auth.is_empty() {
                    "localhost".to_owned()
                } else {
                    auth.to_owned()
                },
                format!("/{tail}"),
            ),
            None => (
                if without_frag.is_empty() {
                    "localhost".to_owned()
                } else {
                    without_frag.to_owned()
                },
                "/".to_owned(),
            ),
        }
    } else {
        ("localhost".to_owned(), "/".to_owned())
    };
    Draft {
        revision: 1,
        target: url.to_owned(),
        raw: format!("{clean_method} {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").into_bytes(),
        version: HttpVersion::Http11,
    }
}

impl Workbench {
    pub(in crate::workbench) fn target_is_draft_entry(&self, id: HistoryEntryId) -> bool {
        matches!(id, HistoryEntryId::Live(flow_id) if self.target_pane.explicit_entries.get(&id).is_some_and(|(entry_id, _)| *entry_id == flow_id))
    }
    pub(in crate::workbench) fn add_to_site_map(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.transition.is_some() || !self.backend_available() || self.shutdown_requested { return; }
        let Some(project) = self.project else { return; };
        if !self.target_pane.pending_adds.is_empty() {
            self.fail("正在持久化站点地图条目，请等待操作完成。", cx); return;
        }
        if !self.leave_http_notes(window, cx) || !self.flush_workspace(window, cx) { return; }
        let result = (|| -> Result<SiteMapEntryInput, String> {
            match self.tool {
                Tool::Proxy if self.proxy_page == ProxyPage::History => {
                    let id = self.selected_history.ok_or("请先选择一条 HTTP 历史记录。")?;
                    let row = self.history.read(cx).delegate().row_by_id(id).cloned()
                        .or_else(|| self.imported_history.row.as_ref().filter(|row| row.id() == id).cloned())
                        .ok_or("所选历史摘要不可用。")?;
                    let entry = match row {
                        HistoryRow::Live(summary) => HistoryTransferEntry::Live { flow_id: summary.id, revision: summary.revision, original: self.history_original_view },
                        HistoryRow::ExtProxy(archive) => {
                            let request = self.imported_history_request().filter(|request| request.item_id == archive.item.id)
                                .or(archive.edited_request.as_ref()).or(archive.request.as_ref()).ok_or("归档条目缺少请求。")?;
                            HistoryTransferEntry::External { source_id: archive.item.source_id, item_id: archive.item.id, message_id: request.id }
                        }
                    };
                    Ok(SiteMapEntryInput::History(entry))
                }
                Tool::Logger => {
                    let id = self.extra_tools.logger_selected.ok_or("请先选择 Logger 流量。")?;
                    let record = self.extra_tools.logger_loaded_detail.as_deref().filter(|(record, _)| record.summary.id == id).ok_or("Logger 流量详情尚未读取完成。")?;
                    Ok(SiteMapEntryInput::History(HistoryTransferEntry::Live { flow_id: id, revision: record.0.summary.revision, original: false }))
                }
                Tool::Target => {
                    let row = self.target_pane.selected_row.as_ref().ok_or("请先选择站点地图请求。")?;
                    if self.target_pane.explicit_entries.contains_key(&row.id()) { return Err("所选请求已经属于站点地图。".into()); }
                    Ok(SiteMapEntryInput::History(self.target_transfer_entry(row).ok_or("所选请求不可转送。")?))
                }
                _ => {
                    let draft = self.current_transfer_draft("添加到站点地图", window, cx)?;
                    let (owner_id, source, response_raw) = match self.tool {
                        Tool::Repeater => {
                            let id = self.workspace.active.ok_or("请选择 HTTP Repeater 标签。")?;
                            let editors = self.tab_editors.get(&id).ok_or("Repeater 编辑器不可用。")?;
                            let raw = editors.response.bytes(window, cx)?;
                            (id, mio_core::TrafficSource::Repeater, (!raw.is_empty()).then_some(raw))
                        }
                        Tool::Intruder => (self.workspace.intruder.active.ok_or("请选择 Intruder 标签。")?, mio_core::TrafficSource::Intruder, None),
                        Tool::Proxy => {
                            let pending = self.intercepts.iter().find(|pending| Some(pending.id()) == self.selected_intercept).ok_or("请选择 HTTP 拦截消息。")?;
                            let InterceptTarget::Http { stage, record, response_head_only, .. } = &pending.target else { return Err("站点地图仅支持 HTTP 消息。".into()); };
                            if *response_head_only { return Err("响应正文仍在读取，站点地图需要完整消息。".into()); }
                            let raw = if *stage == InterceptStage::Response { Some(pending.editor.bytes(window, cx)?) } else { record.response.as_ref().map(response_to_raw) };
                            (record.summary.id, record.summary.source.clone(), raw)
                        }
                        Tool::Organiser => {
                            let id = self.organiser.selected_id.ok_or("请选择 Organiser 条目。")?;
                            let item = self.workspace.organiser_items.iter().find(|item| item.id == id).ok_or("Organiser 条目已变化。")?;
                            (id, mio_core::TrafficSource::Plugin("Organiser".into()), item.response_raw.clone())
                        }
                        Tool::Dashboard if self.audit_dashboard.new_scan_open => (Uuid::new_v4(), mio_core::TrafficSource::Scanner, None),
                        Tool::Dashboard => {
                            let record = self.audit_dashboard.loaded_flow.as_ref().ok_or("漏洞关联流量尚未读取完成。")?;
                            return Ok(SiteMapEntryInput::History(HistoryTransferEntry::Live { flow_id: record.summary.id, revision: record.summary.revision, original: false }));
                        }
                        Tool::Sequencer => (Uuid::new_v4(), mio_core::TrafficSource::Plugin("Sequencer".into()), None),
                        _ => return Err("当前工具没有可添加的 HTTP 请求。".into()),
                    };
                    Ok(SiteMapEntryInput::Draft { owner_id, source, draft, response_raw })
                }
            }
        })();
        let entry = match result { Ok(entry) => entry, Err(error) => { self.fail(error, cx); return; } };
        let operation_id = Uuid::new_v4();
        let workspace_revision = self.workspace.revision;
        let cancel = self.shutdown.child_token();
        if self.enqueue(Command::AddSiteMapEntry { project, operation_id, workspace_revision, entry, cancel: cancel.clone() }, cx) {
            self.target_pane.pending_adds.insert(operation_id, (workspace_revision, cancel));
            self.status = "正在添加站点地图条目。".into();
            cx.notify();
        }
    }

    pub(in crate::workbench) fn on_site_map_entry_added(&mut self, project: ProjectKey, operation_id: Uuid, workspace_revision: u64, result: Result<SiteMapEntryReceipt, String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.project != Some(project) || self.target_pane.pending_adds.get(&operation_id).is_none_or(|(revision, _)| *revision != workspace_revision) { return; }
        self.target_pane.pending_adds.remove(&operation_id);
        match result {
            Ok(receipt) => self.apply_added_site_map_entry(receipt, window, cx),
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn apply_added_site_map_entry(&mut self, receipt: SiteMapEntryReceipt, window: &mut Window, cx: &mut Context<Self>) {
        self.open_site_map_explicit_entry(receipt.row, receipt.entry_id, receipt.revision, Some(receipt.node_key), window, cx);
        self.error = None;
        self.status = "已添加到站点地图。".into();
        cx.notify();
    }

    pub(in crate::workbench) fn open_site_map_explicit_entry(&mut self, row: HistoryRow, entry_id: Uuid, revision: u64, node_key: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.target_pane.explicit_entries.insert(row.id(), (entry_id, revision));
        self.target_pane.page = TargetPage::SiteMap;
        self.target_pane.site_map_subview = SiteMapSubView::Contents;
        self.target_pane.in_scope_only = false;
        self.target_pane.hide_not_found = false;
        self.target_pane.hide_css_image_binary = false;
        self.target_pane.hide_empty_folders = false;
        self.target_pane.status_classes = [true; 4];
        self.target_pane.search.update(cx, |state, cx| state.set_value("", window, cx));
        self.target_pane.tree_state.hidden_prefixes.retain(|prefix| !url_matches_node_prefix(&row_url(&row), prefix));
        self.target_pane.tree_state.selected_key = node_key;
        self.tool = Tool::Target;
        self.select_target_row(&row, window, cx);
        self.refresh_target_site_map(0, window, cx);
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::workbench) fn on_site_map_entry_read(&mut self, project: ProjectKey, selection_revision: u64, entry_id: Uuid, revision: u64, result: Result<SiteMapEntryDetail, String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.project != Some(project) { return; }
        let Some(TargetRead::SiteMap { row_id, entry_id: expected, revision: expected_revision }) = self.target_pane.pending_reads.get(&selection_revision).cloned() else { return; };
        if expected != entry_id || expected_revision != revision { return; }
        self.target_pane.pending_reads.remove(&selection_revision);
        if self.target_pane.selected_id != Some(row_id) || self.target_pane.explicit_entries.get(&row_id) != Some(&(entry_id, revision)) { return; }
        match result {
            Ok(detail) if detail.row_id == row_id => {
                self.target_pane.request.load(&detail.request_raw, window, cx);
                self.target_pane.selected_response_bytes = detail.response_raw.clone().unwrap_or_default();
                self.target_pane.response.load(&self.target_pane.selected_response_bytes, window, cx);
                self.target_pane.selected_live_record = match row_id { HistoryEntryId::Live(_) => Some(Box::new(detail.record.clone())), _ => None };
                if self.target_pane.pending_branch_scan == Some(row_id) {
                    self.target_pane.pending_branch_scan = None;
                    let draft = Draft { revision: 1, target: detail.record.request.url.clone(), raw: detail.request_raw.clone(), version: detail.record.request.version };
                    let action = self.target_pane.pending_branch_action;
                    self.execute_branch_action_with_draft(draft, action, window, cx);
                }
                self.target_pane.selected_explicit_detail = Some(detail);
                self.target_pane.site_map_error = None;
                if self.inspector.open {
                    self.sync_inspector_selection(window, cx);
                }
            }
            Ok(_) => self.target_pane.site_map_error = Some("站点地图详情来源身份不一致。".into()),
            Err(error) => self.target_pane.site_map_error = Some(error),
        }
        cx.notify();
    }

    pub(in crate::workbench) fn render_target_provenance_menu(&self, slot: super::wb_editors::EditorSlot, cx: &mut Context<Self>) -> Option<AnyElement> {
        let response = match slot { super::wb_editors::EditorSlot::TargetRequest => false, super::wb_editors::EditorSlot::TargetResponse => true, _ => return None };
        let id = slot.id();
        let detail = self.target_pane.selected_explicit_detail.as_ref()?;
        let original = if response { detail.original_response_raw.as_ref() } else { detail.original_request_raw.as_ref() }?;
        let current = if response { detail.response_raw.as_ref()? } else { &detail.request_raw };
        if original == current { return None; }
        let original = original.clone();
        let current = current.clone();
        let active_raw = if response {
            self.target_pane.response.bytes_lossless(cx)
        } else {
            self.target_pane.request.bytes_lossless(cx)
        };
        let showing_original = active_raw == original;
        let orig_label = if response { "原始响应" } else { "原始请求" };
        let curr_label = if response { "最终响应" } else { "最终请求" };
        let owner = cx.weak_entity();
        Some(
            super::wb_editors::message_version_trigger(
                SharedString::from(format!("{id}-version")),
                if showing_original { orig_label } else { curr_label },
                "在原始报文与修改后的最终报文之间切换",
            )
            .dropdown_menu(move |menu, _, _| {
                [(true, orig_label, original.clone()), (false, curr_label, current.clone())]
                    .into_iter()
                    .fold(menu.compact().min_w(px(140.)), |menu, (is_orig, label, raw)| {
                        let owner = owner.clone();
                        menu.item(
                            PopupMenuItem::new(label)
                                .checked(showing_original == is_orig)
                                .on_click(move |_, window, cx| {
                                    owner
                                        .update(cx, |view, cx| {
                                            if response {
                                                view.target_pane.response.load(&raw, window, cx);
                                            } else {
                                                view.target_pane.request.load(&raw, window, cx);
                                            }
                                            cx.notify();
                                        })
                                        .expect("站点地图所属工作台必须存在");
                                }),
                        )
                    })
            })
            .into_any_element(),
        )
    }
    pub(in crate::workbench) fn render_target_tabs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let page = self.target_pane.page;
        let sitemap_tab = wb_render_chrome::secondary_tab(
            "target-tab-sitemap",
            "站点地图",
            page == TargetPage::SiteMap,
            window,
            cx,
        )
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(
                "按域名与目录层级浏览已捕获和发现的目标站点结构",
            )
            .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| {
            this.target_pane.page = TargetPage::SiteMap;
            this.refresh_target_site_map(0, window, cx);
            cx.notify();
        }));
        let scope_tab = wb_render_chrome::secondary_tab(
            "target-tab-scope",
            "目标范围",
            page == TargetPage::Scope,
            window,
            cx,
        )
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(
                "配置测试目标包含 (Include) 与排除 (Exclude) 范围规则",
            )
            .build(window, cx)
        })
        .on_click(cx.listener(|this, _, _, cx| {
            this.target_pane.page = TargetPage::Scope;
            cx.notify();
        }));
        let issues_tab = wb_render_chrome::secondary_tab(
            "target-tab-issues",
            "漏洞定义",
            page == TargetPage::Issues,
            window,
            cx,
        )
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(
                "查阅内置安全漏洞知识库、CWE 分类与修复建议参考",
            )
            .build(window, cx)
        })
        .on_click(cx.listener(|this, _, _, cx| {
            this.target_pane.page = TargetPage::Issues;
            this.ensure_default_issue_selected();
            cx.notify();
        }));
        wb_render_chrome::secondary_tab_bar()
            .child(sitemap_tab)
            .child(scope_tab)
            .child(issues_tab)
            .into_any_element()
    }
}
