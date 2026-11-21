use super::site_map_entries::{finding_severity_dot, synthetic_scan_draft};
use super::*;

impl Workbench {
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
