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

    pub(in crate::workbench) fn toggle_site_map_sort(
        &mut self,
        column: SiteMapSortColumn,
        cx: &mut Context<Self>,
    ) {
        self.target_pane.item_sort = Some(match self.target_pane.item_sort {
            Some(current) if current.column == column => SiteMapSort {
                column,
                ascending: !current.ascending,
            },
            _ => SiteMapSort {
                column,
                ascending: true,
            },
        });
        cx.notify();
    }

    pub(in crate::workbench) fn set_site_map_sort(
        &mut self,
        column: SiteMapSortColumn,
        ascending: bool,
        cx: &mut Context<Self>,
    ) {
        self.target_pane.item_sort = Some(SiteMapSort { column, ascending });
        cx.notify();
    }

    pub(in crate::workbench) fn target_cell_text_for_summary(
        col_ix: usize,
        item_idx: usize,
        summary: &super::mime::TargetRowSummary,
        items: &[HistoryRow],
        live_notes: &HashMap<FlowId, (u64, String)>,
    ) -> String {
        match col_ix {
            0 => summary.host.clone(),
            1 => summary.method.clone(),
            2 => summary.url.clone(),
            3 => summary.params.clone(),
            4 => summary.status.clone(),
            5 => summary.length.clone(),
            6 => summary.mime.clone(),
            7 => summary.title.clone(),
            8 => items
                .get(item_idx)
                .map(|row| match row {
                    HistoryRow::ExtProxy(archive) => archive
                        .notes_preview
                        .replace("\r\n", " ")
                        .replace(['\r', '\n', '\t'], " "),
                    HistoryRow::Live(flow) => live_notes
                        .get(&flow.id)
                        .map(|(_, text)| {
                            text.replace("\r\n", " ").replace(['\r', '\n', '\t'], " ")
                        })
                        .unwrap_or_default(),
                })
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    pub(in crate::workbench) fn copy_target_column(
        &mut self,
        col_ix: usize,
        rows: &[(usize, super::mime::TargetRowSummary)],
        cx: &mut Context<Self>,
    ) {
        let items = &self.target_pane.items;
        let live_notes = self.history.read(cx).delegate().live_notes.borrow();
        let mut lines = Vec::with_capacity(rows.len());
        for (item_idx, summary) in rows {
            lines.push(Self::target_cell_text_for_summary(
                col_ix,
                *item_idx,
                summary,
                items,
                &live_notes,
            ));
        }
        drop(live_notes);
        let count = lines.len();
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(lines.join("\n")));
        self.error = None;
        self.status = format!(
            "已复制「{}」列（{count} 行）到剪贴板。",
            SITEMAP_COL_NAMES.get(col_ix).copied().unwrap_or("Site map")
        );
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_target_column_visibility(
        &mut self,
        col_ix: usize,
        cx: &mut Context<Self>,
    ) {
        if col_ix >= SITEMAP_COL_COUNT {
            return;
        }
        let visible_count = self
            .target_pane
            .item_col_visible
            .iter()
            .filter(|&&v| v)
            .count();
        if self.target_pane.item_col_visible[col_ix] {
            if visible_count > 1 {
                self.target_pane.item_col_visible[col_ix] = false;
            }
        } else {
            self.target_pane.item_col_visible[col_ix] = true;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn fit_target_column_width(
        &mut self,
        col_ix: usize,
        rows: &[(usize, super::mime::TargetRowSummary)],
        cx: &mut Context<Self>,
    ) {
        if col_ix >= SITEMAP_COL_COUNT {
            return;
        }
        let measure = |text: &str| -> f32 {
            text.chars()
                .map(|ch| if ch.is_ascii() { 7.2 } else { 13.5 })
                .sum()
        };
        let mut widest = measure(SITEMAP_COL_NAMES[col_ix]) + 22.0;
        let items = &self.target_pane.items;
        let live_notes = self.history.read(cx).delegate().live_notes.borrow();
        for (item_idx, summary) in rows.iter().take(250) {
            let text =
                Self::target_cell_text_for_summary(col_ix, *item_idx, summary, items, &live_notes);
            let preview = theme::truncate_cell_preview(&text);
            widest = widest.max(measure(&preview));
        }
        drop(live_notes);
        let fitted = (widest + 16.0).round().clamp(44.0, 560.0);
        self.target_pane.item_col_widths[col_ix] = fitted;
        cx.notify();
    }

    pub(in crate::workbench) fn begin_target_column_resize(&mut self, col_ix: usize, start_x: f32) {
        if col_ix >= SITEMAP_COL_COUNT {
            return;
        }
        let start_w = self.target_pane.item_col_widths[col_ix];
        self.target_pane.item_col_resize_origin = Some((col_ix, start_x, start_w));
    }

    pub(in crate::workbench) fn update_target_column_resize(
        &mut self,
        col_ix: usize,
        current_x: f32,
        cx: &mut Context<Self>,
    ) {
        if col_ix >= SITEMAP_COL_COUNT {
            return;
        }
        let Some((drag_col, start_x, start_w)) = self.target_pane.item_col_resize_origin else {
            return;
        };
        if drag_col != col_ix {
            return;
        }
        let scale = theme::ui_scale().max(0.75);
        let delta_unscaled = (current_x - start_x) / scale;
        let next_w = (start_w + delta_unscaled).round().clamp(44.0, 560.0);
        if (self.target_pane.item_col_widths[col_ix] - next_w).abs() >= 0.5 {
            self.target_pane.item_col_widths[col_ix] = next_w;
            cx.notify();
        }
    }

    pub(in crate::workbench) fn reset_target_column_layout(&mut self, cx: &mut Context<Self>) {
        self.target_pane.item_col_widths = SITEMAP_DEFAULT_COL_WIDTHS;
        self.target_pane.item_col_visible = [true; SITEMAP_COL_COUNT];
        self.target_pane.item_col_resize_origin = None;
        cx.notify();
    }

    pub(in crate::workbench) fn render_target_items_header(
        &self,
        sort: Option<SiteMapSort>,
        cached_rows: std::rc::Rc<Vec<(usize, super::mime::TargetRowSummary)>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let header_fg = table_header_fg();
        let header_bg = table_header_bg();
        let hover_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x2f3237 } else { 0xe6e6e6 }).into();
        let col_widths = self.target_pane.item_col_widths;
        let col_visible = self.target_pane.item_col_visible;
        let visible_cols: Vec<usize> = (0..SITEMAP_COL_COUNT)
            .filter(|&col_ix| col_visible[col_ix])
            .collect();
        let flex_col_ix = if col_visible[2] {
            2
        } else {
            visible_cols.last().copied().unwrap_or(0)
        };
        let last_visible_col = visible_cols.last().copied().unwrap_or(0);
        let workbench_id = cx.entity_id();
        let weak = cx.weak_entity();
        let header_defs: [(&'static str, &'static str, Option<SiteMapSortColumn>);
            SITEMAP_COL_COUNT] = [
            (
                "target-sitemap-sort-host",
                "Host",
                Some(SiteMapSortColumn::Host),
            ),
            (
                "target-sitemap-sort-method",
                "Method",
                Some(SiteMapSortColumn::Method),
            ),
            ("target-sitemap-sort-url", "URL", Some(SiteMapSortColumn::Url)),
            (
                "target-sitemap-sort-params",
                "Params",
                Some(SiteMapSortColumn::Params),
            ),
            (
                "target-sitemap-sort-status",
                "Status code",
                Some(SiteMapSortColumn::Status),
            ),
            (
                "target-sitemap-sort-length",
                "Length",
                Some(SiteMapSortColumn::Length),
            ),
            (
                "target-sitemap-sort-mime",
                "MIME type",
                Some(SiteMapSortColumn::Mime),
            ),
            (
                "target-sitemap-sort-title",
                "Title",
                Some(SiteMapSortColumn::Title),
            ),
            ("target-sitemap-col-notes", "Notes", None),
        ];

        let header_cells: Vec<AnyElement> = visible_cols
            .iter()
            .map(|&col_ix| {
                let (id, label, sort_col_opt) = header_defs[col_ix];
                let width = col_widths[col_ix];
                let is_last = col_ix == last_visible_col;
                let active = sort_col_opt.and_then(|col| sort.filter(|s| s.column == col));
                let ctx_view = weak.clone();
                let ctx_rows = std::rc::Rc::clone(&cached_rows);
                let fit_rows = std::rc::Rc::clone(&cached_rows);
                let resize_handle = div()
                    .id(("target-sitemap-col-resize", col_ix))
                    .occlude()
                    .cursor_col_resize()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right_0()
                    .w(px(6.))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if event.click_count == 2 {
                                this.fit_target_column_width(col_ix, &fit_rows, cx);
                            } else {
                                this.begin_target_column_resize(col_ix, event.position.x.as_f32());
                            }
                        }),
                    )
                    .on_drag(
                        ResizeTargetCol {
                            entity_id: workbench_id,
                            col_ix,
                        },
                        |drag, _, _, cx| {
                            cx.stop_propagation();
                            cx.new(|_| *drag)
                        },
                    );

                let mut cell = div()
                    .id(id)
                    .test_support()
                    .role(gpui_kit::accesskit::Role::ColumnHeader)
                    .aria_label(label)
                    .relative()
                    .h_full()
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .overflow_hidden()
                    .bg(header_bg)
                    .text_color(header_fg)
                    .when(!is_last, |c| c.border_r_1().border_color(theme::border()))
                    .hover(move |s| s.bg(hover_bg));
                if let Some(sort_col) = sort_col_opt {
                    cell = cell
                        .cursor_pointer()
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("点击按此列排序")
                                .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_site_map_sort(sort_col, cx);
                        }));
                }
                let cell = cell
                    .context_menu(move |menu, window, cx| {
                        build_target_header_context_menu(
                            ctx_view.clone(),
                            col_ix,
                            sort_col_opt,
                            std::rc::Rc::clone(&ctx_rows),
                            col_visible,
                            menu,
                            window,
                            cx,
                        )
                    })
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(theme::ui_font_size())
                            .font_weight(gpui_kit::FontWeight::NORMAL)
                            .text_color(header_fg)
                            .child(label),
                    )
                    .when_some(active, |row, s| {
                        row.child(
                            Icon::new(if s.ascending {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .size(px(11.))
                            .text_color(theme::muted_foreground())
                            .flex_shrink_0(),
                        )
                    })
                    .child(resize_handle);

                if col_ix == flex_col_ix {
                    cell.flex_1()
                        .min_w(theme::scaled_px(width))
                        .into_any_element()
                } else {
                    cell.w(theme::scaled_px(width))
                        .flex_shrink_0()
                        .into_any_element()
                }
            })
            .collect();

        div()
            .w_full()
            .h(theme::table_header_height())
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_0()
            .border_b_1()
            .border_color(theme::border())
            .bg(header_bg)
            .text_color(header_fg)
            .on_drag_move::<ResizeTargetCol>(cx.listener(
                |this, event: &DragMoveEvent<ResizeTargetCol>, _, cx| {
                    let drag = *event.drag(cx);
                    if drag.entity_id == cx.entity_id() {
                        this.update_target_column_resize(
                            drag.col_ix,
                            event.event.position.x.as_f32(),
                            cx,
                        );
                    }
                },
            ))
            .children(header_cells)
            .into_any_element()
    }

    pub(in crate::workbench) fn render_site_map_issues_pane(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let findings: Vec<AuditFinding> = filter_branch_findings(
            &self.audit_dashboard.snapshot.findings,
            self.target_pane.tree_state.selected_key.as_deref(),
            &self.target_pane.tree_state.hidden_prefixes,
        )
        .into_iter()
        .cloned()
        .collect();
        let active_finding = self
            .target_pane
            .selected_site_map_finding_id
            .and_then(|id| findings.iter().find(|f| f.id == id).cloned())
            .or_else(|| findings.first().cloned());
        let active_id = active_finding.as_ref().map(|f| f.id);
        let mut list = div()
            .id("target-sitemap-issues-list")
            .test_support()
            .h(px(220.))
            .flex_shrink_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::surface());
        if findings.is_empty() {
            list = list.child(
                div()
                    .p(px(12.))
                    .child("当前选中分支下暂无扫描发现 (Issues)。"),
            );
        } else {
            let weak = cx.weak_entity();
            for (idx, finding) in findings.iter().enumerate() {
                let id = finding.id;
                let selected = active_id == Some(id);
                let menu_view = weak.clone();
                let label = format!(
                    "{} {} {}",
                    finding.severity_label, finding.title, finding.url
                );
                list = list.child(
                    div()
                        .id(("target-sitemap-issue-row", idx))
                        .test_support()
                        .aria_label(label)
                        .aria_selected(selected)
                        .h(theme::row_height())
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .cursor_pointer()
                        .bg(if selected {
                            theme::selected()
                        } else {
                            theme::surface()
                        })
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, _, _, cx| {
                                this.target_pane.selected_site_map_finding_id = Some(id);
                                cx.notify();
                            }),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.target_pane.selected_site_map_finding_id = Some(id);
                            cx.notify();
                        }))
                        .context_menu(move |menu, window, cx| {
                            target_issue_row_menu(menu_view.clone(), id, menu, window, cx)
                        })
                        .child(
                            div()
                                .size(px(10.))
                                .rounded_full()
                                .bg(finding_severity_dot(finding.severity_rank)),
                        )
                        .child(
                            div()
                                .w(theme::scaled_px(72.))
                                .truncate()
                                .child(finding.severity_label.clone()),
                        )
                        .child(
                            div()
                                .w(theme::scaled_px(220.))
                                .truncate()
                                .child(finding.title.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(finding.url.clone()),
                        ),
                );
            }
        }
        let detail = div()
            .id("target-sitemap-issue-detail")
            .test_support()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(12.))
            .bg(theme::surface())
            .when_some(active_finding, |pane, finding| {
                let finding_id = finding.id;
                pane.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .id("target-sitemap-issue-title")
                                        .test_support()
                                        .aria_label(finding.title.clone())
                                        .text_size(theme::scaled_px(15.))
                                        .font_weight(gpui_kit::FontWeight::BOLD)
                                        .child(finding.title),
                                )
                                .child(
                                    button("target-sitemap-open-in-dashboard", "在审计面板查看")
                                        .icon(
                                            gpui_kit::component::Icon::new(
                                                gpui_kit::assets::IconName::ExternalLink,
                                            )
                                            .size(px(13.)),
                                        )
                                        .tooltip("跳转到 Dashboard 审计面板并定位到此条漏洞告警")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.tool = Tool::Dashboard;
                                            this.activate_pro_dashboard(cx);
                                            this.select_audit_finding(finding_id, window, cx);
                                        })),
                                ),
                        )
                        .child(format!(
                            "严重度：{} · 置信度：{} · URL：{}",
                            finding.severity_label,
                            crate::workbench::audit_dashboard::confidence_label(finding.confidence),
                            finding.url
                        ))
                        .when_some(finding.parameter, |col, param| {
                            col.child(format!("参数: {param}"))
                        })
                        .child(finding.summary)
                        .when(!finding.evidence.is_empty(), |col| {
                            col.child(format!("证据: {}", finding.evidence))
                        })
                        .when(!finding.remediation.is_empty(), |col| {
                            col.child(format!("修复建议: {}", finding.remediation))
                        }),
                )
            })
            .when(active_id.is_none(), |pane| {
                pane.child("选择一条漏洞发现查看详情。")
            });
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(list)
            .child(detail)
            .into_any_element()
    }

    pub(in crate::workbench) fn execute_branch_action_with_draft(
        &mut self,
        draft: Draft,
        action: BranchItemAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            BranchItemAction::OpenScanLauncher => self.send_draft_to_scanner(
                draft,
                mio_core::AuditTaskMode::PassiveAndActive,
                false,
                window,
                cx,
            ),
            BranchItemAction::ActiveScan => self.send_draft_to_scanner(
                draft,
                mio_core::AuditTaskMode::PassiveAndActive,
                true,
                window,
                cx,
            ),
            BranchItemAction::PassiveScan => self.send_draft_to_scanner(
                draft,
                mio_core::AuditTaskMode::PassiveOnly,
                true,
                window,
                cx,
            ),
            BranchItemAction::DiscoverContent => {
                self.start_target_crawl_task("发现内容", draft, window, cx);
            }
            BranchItemAction::SimulateManualTesting => {
                self.start_target_crawl_task("模拟手动测试", draft, window, cx);
            }
            BranchItemAction::SendToIntruder => self.send_target_history_transfer(true, window, cx),
        }
    }

    pub(in crate::workbench) fn finish_branch_action_from_items(
        &mut self,
        prefix: &str,
        action: BranchItemAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let first_row = self
            .target_pane
            .items
            .iter()
            .find(|row| {
                let url = row_url(row);
                !self.target_pane.hidden_item_ids.contains(&row.id())
                    && !is_url_hidden(&url, &self.target_pane.tree_state.hidden_prefixes)
                    && url_matches_node_prefix(&url, prefix)
            })
            .cloned();
        let Some(row) = first_row else {
            let fallback_url = self
                .target_pane
                .tree_state
                .nodes
                .iter()
                .find(|node| node.key == prefix)
                .map(|node| node.url_prefix.clone())
                .unwrap_or_else(|| prefix.to_owned());
            match action {
                BranchItemAction::SendToIntruder => {
                    self.fail("该站点分支下尚无已捕获的请求记录。", cx);
                }
                _ => {
                    let preview = synthetic_scan_draft("GET", &fallback_url);
                    self.execute_branch_action_with_draft(preview, action, window, cx);
                }
            }
            return;
        };
        if action == BranchItemAction::SendToIntruder {
            self.select_target_row(&row, window, cx);
            if let Some(entry) = self.target_transfer_entry(&row) {
                self.begin_history_transfer(None, vec![entry], true, window, cx);
            } else {
                self.fail("所选分支首项没有可转送的请求。", cx);
            }
            return;
        }
        if self.target_pane.selected_id == Some(row.id())
            && !self.target_pane.request.raw.read(cx).value().is_empty()
        {
            match action {
                BranchItemAction::OpenScanLauncher => self.open_scan_launcher(window, cx),
                BranchItemAction::ActiveScan => self.send_to_scanner(window, cx),
                BranchItemAction::PassiveScan => self.send_to_passive_scanner(window, cx),
                BranchItemAction::DiscoverContent | BranchItemAction::SimulateManualTesting => {
                    if let Ok(draft) = self.current_transfer_draft("启动目标任务", window, cx) {
                        self.execute_branch_action_with_draft(draft, action, window, cx);
                    }
                }
                BranchItemAction::SendToIntruder => {}
            }
            return;
        }
        if let HistoryRow::Live(flow) = &row
            && let Some(detail) = self.detail.as_ref().filter(|d| d.summary.id == flow.id)
        {
            let draft = Draft {
                revision: 1,
                target: detail.request.url.clone(),
                raw: request_to_raw(&detail.request),
                version: detail.request.version,
            };
            self.execute_branch_action_with_draft(draft, action, window, cx);
            return;
        }
        self.select_target_row(&row, window, cx);
        self.target_pane.pending_branch_action = action;
        self.target_pane.pending_branch_scan = Some(row.id());
        self.cancel_plugin_interactions(cx);
        self.clear_dashboard_intruder_selection(window, cx);
        self.tool = Tool::Dashboard;
        self.activate_pro_dashboard(cx);
        cx.notify();
    }

    #[cfg(test)]
    pub(in crate::workbench) fn scan_target_branch(
        &mut self,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_target_branch_action(key, BranchItemAction::ActiveScan, window, cx);
    }

    pub(in crate::workbench) fn run_target_branch_action(
        &mut self,
        key: String,
        action: BranchItemAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let has_matching_row = self.target_pane.items.iter().any(|row| {
            let url = row_url(row);
            !is_url_hidden(&url, &self.target_pane.tree_state.hidden_prefixes)
                && url_matches_node_prefix(&url, &key)
        });
        if has_matching_row {
            self.finish_branch_action_from_items(&key, action, window, cx);
            return;
        }
        let has_history_row = self
            .history
            .read(cx)
            .delegate()
            .rows
            .iter()
            .find(|row| {
                let url = row_url(row);
                !is_url_hidden(&url, &self.target_pane.tree_state.hidden_prefixes)
                    && url_matches_node_prefix(&url, &key)
            })
            .cloned();
        if let Some(row) = has_history_row {
            if action == BranchItemAction::SendToIntruder {
                if let Some(entry) = self.target_transfer_entry(&row) {
                    self.begin_history_transfer(None, vec![entry], true, window, cx);
                }
                return;
            }
            self.select_target_row(&row, window, cx);
            self.target_pane.pending_branch_action = action;
            self.target_pane.pending_branch_scan = Some(row.id());
            return;
        }
        self.select_target_node(Some(key.clone()), window, cx);
        self.target_pane.pending_branch_action = action;
        self.target_pane.pending_branch_scan_prefix = Some(key);
    }

    pub(in crate::workbench) fn open_target_item_notes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let Some(selected_id) = self.target_pane.selected_id else {
            self.fail("请先在站点地图中选择一条记录。", cx);
            return;
        };
        if self.target_is_draft_entry(selected_id) { self.fail("草稿条目没有关联的流量备注。", cx); return; }
        if self.selected_history != Some(selected_id) && !self.leave_http_notes(window, cx) {
            return;
        }
        self.selected_history = Some(selected_id);
        if let HistoryEntryId::Live(flow_id) = selected_id {
            self.selected_flow = Some(flow_id);
        } else {
            self.selected_flow = None;
        }
        self.inspector.open = false;
        self.http_notes.open = true;
        self.select_http_notes(window, cx);
        self.http_notes
            .editor
            .update(cx, |editor, cx| editor.focus(window, cx));
        cx.notify();
    }

    pub(in crate::workbench) fn copy_selected_target_links(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            return;
        }
        let Some(row) = self.target_pane.selected_row.as_ref() else {
            self.fail("请先在站点地图中选择一条记录。", cx);
            return;
        };
        let base_url = row_url(row);
        if base_url.is_empty() {
            self.fail("所选站点地图记录缺少有效 URL。", cx);
            return;
        }
        let raw_response = match self.target_pane.response.bytes(window, cx) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            Ok(_) => {
                self.fail("所选站点地图记录尚无已加载的 HTTP 响应报文。", cx);
                return;
            }
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        let links = match crate::workbench::wb_render_history::extract_response_links_list(
            &base_url,
            &raw_response,
        ) {
            Ok(links) => links,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        if links.is_empty() {
            self.error = None;
            self.status = "选中响应中未提取到有效的页面链接。".into();
            cx.notify();
            return;
        }
        let count = links.len();
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(links.join("\n")));
        self.error = None;
        self.status = format!("已提取并复制 {count} 条链接到剪贴板。");
        cx.notify();
    }

    pub(in crate::workbench) fn set_target_row_highlight(
        &mut self,
        id: HistoryEntryId,
        row: Option<HistoryRow>,
        color: u8,
        cx: &mut Context<Self>,
    ) {
        if self.target_is_draft_entry(id) { self.fail("草稿条目没有关联的流量高亮。", cx); return; }
        if let Some(HistoryRow::ExtProxy(ref archive)) = row {
            let has_in_history = self.history.read(cx).delegate().row_by_id(id).is_some();
            if !has_in_history {
                self.history.update(cx, |table, _| {
                    table
                        .delegate_mut()
                        .selected_rows
                        .insert(id, HistoryRow::ExtProxy(archive.clone()));
                });
            }
            for item in &mut self.target_pane.items {
                if item.id() == id
                    && let HistoryRow::ExtProxy(entry) = item
                {
                    let rev = entry.highlight.as_ref().map_or(0, |h| h.revision);
                    entry.highlight = Some(mio_core::ExtHttpHighlight {
                        revision: rev,
                        color,
                    });
                }
            }
        } else if let HistoryEntryId::Live(flow_id) = id {
            let has_in_history = self.history.read(cx).delegate().row_by_id(id).is_some();
            if !has_in_history
                && let Some(HistoryRow::Live(summary)) = row
            {
                self.history.update(cx, |table, _| {
                    table
                        .delegate_mut()
                        .selected_rows
                        .insert(id, HistoryRow::Live(summary));
                });
            }
            let _ = flow_id;
        }
        self.set_http_highlights(&[id], color, cx);
    }

    pub(in crate::workbench) fn set_target_selected_highlights(
        &mut self,
        color: u8,
        cx: &mut Context<Self>,
    ) {
        let rows = self.selected_target_rows_in_order();
        if rows.len() <= 1 {
            if let Some(row) = rows.first() {
                self.set_target_row_highlight(row.id(), Some(row.clone()), color, cx);
            }
            return;
        }
        let mut ids = Vec::new();
        for row in &rows {
            let id = row.id();
            if self.target_is_draft_entry(id) {
                continue;
            }
            match row {
                HistoryRow::ExtProxy(archive) => {
                    let has_in_history = self.history.read(cx).delegate().row_by_id(id).is_some();
                    if !has_in_history {
                        self.history.update(cx, |table, _| {
                            table
                                .delegate_mut()
                                .selected_rows
                                .insert(id, HistoryRow::ExtProxy(archive.clone()));
                        });
                    }
                    for item in &mut self.target_pane.items {
                        if item.id() == id
                            && let HistoryRow::ExtProxy(entry) = item
                        {
                            let rev = entry.highlight.as_ref().map_or(0, |h| h.revision);
                            entry.highlight = Some(mio_core::ExtHttpHighlight {
                                revision: rev,
                                color,
                            });
                        }
                    }
                }
                HistoryRow::Live(summary) => {
                    let has_in_history = self.history.read(cx).delegate().row_by_id(id).is_some();
                    if !has_in_history {
                        self.history.update(cx, |table, _| {
                            table
                                .delegate_mut()
                                .selected_rows
                                .insert(id, HistoryRow::Live(summary.clone()));
                        });
                    }
                }
            }
            ids.push(id);
        }
        if ids.is_empty() {
            self.fail("草稿条目没有关联的流量高亮。", cx);
            return;
        }
        self.set_http_highlights(&ids, color, cx);
    }

    pub(in crate::workbench) fn copy_target_branch_urls(
        &mut self,
        key: String,
        cx: &mut Context<Self>,
    ) {
        let mut urls = collect_branch_urls(
            &self.target_pane.items,
            &self.target_pane.tree_state.nodes,
            &self.target_pane.tree_state.hidden_prefixes,
            &key,
        );
        let mut seen: HashSet<String> = urls.iter().cloned().collect();
        let history_rows = self.history.read(cx).delegate().rows.clone();
        for row in &history_rows {
            if self.target_pane.hidden_item_ids.contains(&row.id()) {
                continue;
            }
            let url = row_url(row);
            if !url.is_empty()
                && !is_url_hidden(&url, &self.target_pane.tree_state.hidden_prefixes)
                && url_matches_node_prefix(&url, &key)
                && seen.insert(url.clone())
            {
                urls.push(url);
            }
        }
        if urls.is_empty() {
            self.fail("当前分支没有可复制的 URL。", cx);
            return;
        }
        let count = urls.len();
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(urls.join("\n")));
        self.error = None;
        self.status = format!("已复制分支下 {count} 条不重复 URL 到剪贴板。");
        cx.notify();
    }

    pub(in crate::workbench) fn hide_target_branch(
        &mut self,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let normalized = canonical_site_map_url_key(&key).unwrap_or_else(|| key.trim().to_owned());
        if normalized.is_empty() {
            return;
        }
        if !self
            .target_pane
            .tree_state
            .hidden_prefixes
            .iter()
            .any(|existing| existing == &normalized)
        {
            self.target_pane
                .tree_state
                .hidden_prefixes
                .push(normalized.clone());
        }
        if self
            .target_pane
            .tree_state
            .selected_key
            .as_deref()
            .is_some_and(|selected| url_matches_node_prefix(selected, &normalized))
        {
            self.target_pane.tree_state.selected_key = None;
            self.refresh_target_site_map(0, window, cx);
        }
        if self
            .target_pane
            .selected_row
            .as_ref()
            .is_some_and(|row| url_matches_node_prefix(&row_url(row), &normalized))
        {
            self.target_pane.selected_id = None;
            self.target_pane.selected_ids.clear();
            self.target_pane.selection_anchor = None;
            self.target_pane.selected_row = None;
            self.target_pane.selected_response_bytes.clear();
            self.target_pane.selected_live_record = None;
            self.target_pane.request.load(&[], window, cx);
            self.target_pane.response.load(&[], window, cx);
        }
        self.error = None;
        self.status = "已隐藏选中站点分支。".into();
        cx.notify();
    }

    pub(in crate::workbench) fn show_hidden_target_branches(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.target_pane.tree_state.hidden_prefixes.is_empty()
            && self.target_pane.hidden_item_ids.is_empty()
        {
            return;
        }
        self.target_pane.tree_state.hidden_prefixes.clear();
        self.target_pane.hidden_item_ids.clear();
        self.error = None;
        self.status = "已恢复显示所有已隐藏分支与条目。".into();
        self.refresh_target_site_map(0, window, cx);
        cx.notify();
    }
}

pub(in crate::workbench) fn build_target_header_context_menu(
    view: WeakEntity<Workbench>,
    col_ix: usize,
    sort_col_opt: Option<SiteMapSortColumn>,
    rows: std::rc::Rc<Vec<(usize, super::mime::TargetRowSummary)>>,
    col_visible: [bool; SITEMAP_COL_COUNT],
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let visible_count = col_visible.iter().filter(|&&v| v).count();
    let can_hide = col_ix < SITEMAP_COL_COUNT && visible_count > 1;
    let has_rows = !rows.is_empty();

    let v_asc = view.clone();
    let v_desc = view.clone();
    let v_copy = view.clone();
    let v_hide = view.clone();
    let v_fit = view.clone();
    let v_cols = view.clone();
    let v_reset = view;

    let copy_rows = std::rc::Rc::clone(&rows);
    let fit_rows = rows;

    let cols_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        (0..SITEMAP_COL_COUNT).fold(sub.compact().min_w(px(172.)), |sub, idx| {
            let owner = v_cols.clone();
            let checked = col_visible[idx];
            let disabled = checked && visible_count <= 1;
            sub.item(
                PopupMenuItem::new(SITEMAP_COL_NAMES[idx])
                    .checked(checked)
                    .disabled(disabled)
                    .on_click(move |_, _, cx| {
                        let _ = owner.update(cx, |wb, cx| {
                            wb.toggle_target_column_visibility(idx, cx);
                        });
                    }),
            )
        })
    });

    let mut menu = menu.compact();
    if let Some(sort_col) = sort_col_opt {
        menu = menu
            .item(
                PopupMenuItem::new("按此列升序排序 (Sort ascending)").on_click(move |_, _, cx| {
                    let _ = v_asc.update(cx, |wb, cx| wb.set_site_map_sort(sort_col, true, cx));
                }),
            )
            .item(
                PopupMenuItem::new("按此列降序排序 (Sort descending)").on_click(
                    move |_, _, cx| {
                        let _ = v_desc
                            .update(cx, |wb, cx| wb.set_site_map_sort(sort_col, false, cx));
                    },
                ),
            )
            .separator();
    }

    menu.item(
        PopupMenuItem::new("复制整列内容 (Copy column)")
            .disabled(!has_rows)
            .on_click(move |_, _, cx| {
                let _ = v_copy.update(cx, |wb, cx| {
                    wb.copy_target_column(col_ix, &copy_rows, cx);
                });
            }),
    )
    .item(
        PopupMenuItem::new("隐藏此列 (Hide column)")
            .disabled(!can_hide)
            .on_click(move |_, _, cx| {
                let _ = v_hide.update(cx, |wb, cx| {
                    wb.toggle_target_column_visibility(col_ix, cx);
                });
            }),
    )
    .item(
        PopupMenuItem::new("自适应列宽 (Auto-fit column width)").on_click(move |_, _, cx| {
            let _ = v_fit.update(cx, |wb, cx| {
                wb.fit_target_column_width(col_ix, &fit_rows, cx);
            });
        }),
    )
    .separator()
    .item(PopupMenuItem::submenu("显示/隐藏列 (Columns)", cols_submenu))
    .item(
        PopupMenuItem::new("重置列布局 (Reset column layout)").on_click(move |_, _, cx| {
            let _ = v_reset.update(cx, |wb, cx| wb.reset_target_column_layout(cx));
        }),
    )
}
