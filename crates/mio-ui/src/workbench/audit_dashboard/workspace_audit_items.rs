use super::*;

struct AuditItemRow {
    scan_task_id: Option<Uuid>,
    entry_id: Option<HistoryEntryId>,
    flow_id: Option<FlowId>,
    host: String,
    method: String,
    url: String,
    status: String,
    issues: usize,
    progress: String,
}

impl Workbench {
    fn synthetic_audit_item_draft(method: &str, url: &str) -> Draft {
        let trimmed_method = method.trim();
        let clean_method = if trimmed_method.is_empty()
            || trimmed_method == "—"
            || trimmed_method.eq_ignore_ascii_case("SCAN")
        {
            "GET"
        } else {
            trimmed_method
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
                None => {
                    if let Some((auth, q)) = without_frag.split_once('?') {
                        (
                            if auth.is_empty() {
                                "localhost".to_owned()
                            } else {
                                auth.to_owned()
                            },
                            format!("/?{q}"),
                        )
                    } else {
                        (
                            if without_frag.is_empty() {
                                "localhost".to_owned()
                            } else {
                                without_frag.to_owned()
                            },
                            "/".to_owned(),
                        )
                    }
                }
            }
        } else {
            ("localhost".to_owned(), "/".to_owned())
        };
        Draft {
            revision: 1,
            target: url.trim().to_owned(),
            raw: format!("{clean_method} {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").into_bytes(),
            version: HttpVersion::Http11,
        }
    }

    pub(in crate::workbench) fn open_audit_item_in_repeater(
        &mut self,
        flow_id: Option<FlowId>,
        method: &str,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        if !self.flush_workspace(window, cx) {
            return;
        }
        if let Some(fid) = flow_id {
            if self.enqueue(
                Command::OpenRepeater {
                    project,
                    flow_id: fid,
                    original: false,
                },
                cx,
            ) {
                self.cancel_plugin_interactions(cx);
                self.tool = Tool::Repeater;
                self.status = format!("已在 Repeater 中打开：{url}");
                cx.notify();
            }
            return;
        }
        if url.trim().is_empty() {
            return;
        }
        let draft = Self::synthetic_audit_item_draft(method, url);
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
        if let Some(editors) = self.tab_editors.get(&id) {
            editors
                .request
                .state()
                .update(cx, |state, cx| state.focus(window, cx));
        }
        self.status = format!("已在 Repeater 中打开：{url}");
        cx.notify();
    }

    pub(in crate::workbench) fn scan_audit_item(
        &mut self,
        method: &str,
        url: &str,
        mode: AuditTaskMode,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() || url.trim().is_empty() {
            return;
        }
        let draft = Self::synthetic_audit_item_draft(method, url);
        let prefix = match mode {
            AuditTaskMode::PassiveOnly => "被动扫描",
            AuditTaskMode::PassiveAndActive => "主动扫描",
        };
        let title = format!("{prefix} · {url}");
        if self.dispatch_audit_scan(title, draft, mode, cx) {
            self.status = format!("已启动{prefix}：{url}");
            cx.notify();
        }
    }

    pub(in crate::workbench) fn delete_audit_item(
        &mut self,
        scan_task_id: Option<Uuid>,
        entry_id: Option<HistoryEntryId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(task_id) = scan_task_id {
            self.delete_audit_task(task_id, window, cx);
            return;
        }
        let Some(entry_id) = entry_id else {
            return;
        };
        self.audit_dashboard.dismissed_audit_items.insert(entry_id);
        match entry_id {
            HistoryEntryId::Live(flow_id) => {
                if self.audit_can_operate()
                    && self.pending_history_delete.is_none()
                    && let Some(project) = self.project
                {
                    let operation_id = Uuid::new_v4();
                    if self.enqueue(
                        Command::DeleteHttpHistory {
                            project,
                            operation_id,
                            flow_ids: vec![flow_id],
                        },
                        cx,
                    ) {
                        self.pending_history_delete = Some((project, operation_id));
                    }
                }
                self.status = "已删除选中的审计项。".into();
            }
            HistoryEntryId::External { .. } => {
                self.status = "已从审计项列表中移除该条目。".into();
            }
        }
        cx.notify();
    }

    fn collect_dashboard_audit_item_rows(
        &self,
        active_task: Option<&AuditTaskSummary>,
        cx: &Context<Self>,
    ) -> Vec<AuditItemRow> {
        let scope_only_live = active_task.is_none()
            && self.audit_dashboard.live_task_scope == LiveTaskTrafficScope::SuiteScopeOnly;

        if let Some(task) = active_task {
            let matching_issues = self
                .audit_dashboard
                .snapshot
                .findings
                .iter()
                .filter(|f| self.finding_belongs_to_audit_task(f, task))
                .count();
            return vec![AuditItemRow {
                scan_task_id: Some(task.id),
                entry_id: None,
                flow_id: None,
                host: format_history_origin(&task.target_url),
                method: "SCAN".to_owned(),
                url: task.target_url.clone(),
                status: task.state.label().to_owned(),
                issues: matching_issues,
                progress: format!("{}/{}", task.requests_sent, task.total_planned),
            }];
        }

        let findings = &self.audit_dashboard.snapshot.findings;
        self.history
            .read(cx)
            .delegate()
            .rows
            .iter()
            .filter(|row| {
                !self
                    .audit_dashboard
                    .dismissed_audit_items
                    .contains(&row.id())
            })
            .filter_map(|row| match row {
                HistoryRow::Live(flow) => {
                    if scope_only_live && !self.workspace.scope.is_url_in_scope(&flow.url) {
                        return None;
                    }
                    let issue_cnt = findings
                        .iter()
                        .filter(|f| f.flow_id == Some(flow.id) || f.url == flow.url)
                        .count();
                    Some(AuditItemRow {
                        scan_task_id: None,
                        entry_id: Some(HistoryEntryId::Live(flow.id)),
                        flow_id: Some(flow.id),
                        host: format_history_origin(&flow.url),
                        method: flow.method.clone(),
                        url: flow.url.clone(),
                        status: flow
                            .status
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "—".into()),
                        issues: issue_cnt,
                        progress: "已完成".into(),
                    })
                }
                HistoryRow::ExtProxy(ext_archive) => {
                    let req = ext_archive
                        .edited_request
                        .as_ref()
                        .or(ext_archive.request.as_ref());
                    let resp = ext_archive
                        .edited_response
                        .as_ref()
                        .or(ext_archive.response.as_ref());
                    let origin = ext_archive.origin.as_deref().unwrap_or("");
                    let target = req
                        .and_then(|r| r.request_target.as_deref())
                        .unwrap_or("");
                    let full_url = if target.contains("://") {
                        target.to_owned()
                    } else if !origin.is_empty() {
                        format!("{origin}{target}")
                    } else {
                        target.to_owned()
                    };
                    if scope_only_live && !self.workspace.scope.is_url_in_scope(&full_url) {
                        return None;
                    }
                    let issue_cnt = findings
                        .iter()
                        .filter(|f| !full_url.is_empty() && f.url == full_url)
                        .count();
                    Some(AuditItemRow {
                        scan_task_id: None,
                        entry_id: Some(HistoryEntryId::External {
                            source_id: ext_archive.item.source_id,
                            item_id: ext_archive.item.id,
                        }),
                        flow_id: None,
                        host: if !origin.is_empty() {
                            format_history_origin(origin)
                        } else {
                            format_history_origin(&full_url)
                        },
                        method: req
                            .and_then(|r| r.method.as_deref())
                            .unwrap_or("—")
                            .to_owned(),
                        url: full_url,
                        status: resp
                            .and_then(|r| r.status)
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "—".into()),
                        issues: issue_cnt,
                        progress: "已导入".into(),
                    })
                }
            })
            .take(200)
            .collect()
    }

    fn render_dashboard_audit_item_row(
        idx: usize,
        item: AuditItemRow,
        row_hover_bg: Hsla,
        weak: WeakEntity<Workbench>,
    ) -> AnyElement {
        let menu_view = weak;
        let item_task_id = item.scan_task_id;
        let item_entry_id = item.entry_id;
        let item_flow_id = item.flow_id;
        let item_method = item.method.clone();
        let item_url = item.url.clone();
        let row_label = format!("{} {} {}", item.method, item.host, item.url);
        div()
            .id(("dashboard-audit-item-row", idx))
            .test_support()
            .role(Role::Row)
            .aria_label(row_label)
            .h(theme::scaled_px(26.))
            .flex_shrink_0()
            .px(px(12.))
            .border_b_1()
            .border_color(theme::border())
            .flex()
            .items_center()
            .gap(px(8.))
            .cursor_pointer()
            .hover(move |s| s.bg(row_hover_bg))
            .text_size(theme::scaled_px(12.5))
            .child(
                div()
                    .w(theme::scaled_px(44.))
                    .flex_shrink_0()
                    .text_color(theme::muted_foreground())
                    .child((idx + 1).to_string()),
            )
            .child(
                div()
                    .w(theme::scaled_px(64.))
                    .flex_shrink_0()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(item.method),
            )
            .child(
                div()
                    .w(theme::scaled_px(170.))
                    .flex_shrink_0()
                    .truncate()
                    .child(item.host),
            )
            .child(div().flex_1().min_w_0().truncate().child(item.url))
            .child(
                div()
                    .w(theme::scaled_px(64.))
                    .flex_shrink_0()
                    .child(item.status),
            )
            .child(
                div()
                    .w(theme::scaled_px(64.))
                    .flex_shrink_0()
                    .child(item.issues.to_string()),
            )
            .child(
                div()
                    .w(theme::scaled_px(84.))
                    .flex_shrink_0()
                    .text_color(theme::muted_foreground())
                    .child(item.progress),
            )
            .context_menu(move |menu, _, cx| {
                let can_operate = menu_view
                    .upgrade()
                    .is_some_and(|e| e.read(cx).audit_can_operate());
                let has_url = !item_url.trim().is_empty();
                let rep_owner = menu_view.clone();
                let rep_method = item_method.clone();
                let rep_url = item_url.clone();
                let passive_owner = menu_view.clone();
                let passive_method = item_method.clone();
                let passive_url = item_url.clone();
                let active_owner = menu_view.clone();
                let active_method = item_method.clone();
                let active_url = item_url.clone();
                let scope_owner = menu_view.clone();
                let scope_url = item_url.clone();
                let copy_owner = menu_view.clone();
                let copy_url = item_url.clone();
                let del_owner = menu_view.clone();
                menu.item(
                    PopupMenuItem::new("在 Repeater 中打开")
                        .disabled(!can_operate || (!has_url && item_flow_id.is_none()))
                        .on_click(move |_, window, cx| {
                            let _ = rep_owner.update(cx, |v, cx| {
                                v.open_audit_item_in_repeater(
                                    item_flow_id,
                                    &rep_method,
                                    &rep_url,
                                    window,
                                    cx,
                                );
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("被动扫描此项")
                        .disabled(!can_operate || !has_url)
                        .on_click(move |_, _, cx| {
                            let _ = passive_owner.update(cx, |v, cx| {
                                v.scan_audit_item(
                                    &passive_method,
                                    &passive_url,
                                    AuditTaskMode::PassiveOnly,
                                    cx,
                                );
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("主动扫描此项")
                        .disabled(!can_operate || !has_url)
                        .on_click(move |_, _, cx| {
                            let _ = active_owner.update(cx, |v, cx| {
                                v.scan_audit_item(
                                    &active_method,
                                    &active_url,
                                    AuditTaskMode::PassiveAndActive,
                                    cx,
                                );
                            });
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("添加到范围")
                        .disabled(!can_operate || !has_url)
                        .on_click(move |_, _, cx| {
                            let prefix =
                                crate::workbench::target::target_origin_from_url(&scope_url);
                            let _ = scope_owner.update(cx, |v, cx| {
                                if v.workspace.scope.advanced_mode {
                                    v.add_advanced_scope_rule_from_prefix(prefix, true, cx);
                                } else {
                                    v.add_scope_rule(v.project, true, prefix, cx);
                                }
                            });
                        }),
                )
                .item(
                    PopupMenuItem::new("复制 URL")
                        .disabled(!has_url)
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_url.clone()));
                            let _ = copy_owner.update(cx, |v, cx| {
                                v.status = "已复制审计项 URL 到剪贴板。".into();
                                cx.notify();
                            });
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new("删除项")
                        .disabled(item_task_id.is_none() && item_entry_id.is_none())
                        .on_click(move |_, window, cx| {
                            let _ = del_owner.update(cx, |v, cx| {
                                v.delete_audit_item(item_task_id, item_entry_id, window, cx);
                            });
                        }),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_dashboard_audit_items_tab(
        &self,
        active_task: Option<&AuditTaskSummary>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let header_bg: Hsla = rgb(if dark { 0x2f3237 } else { 0xf1f1f1 }).into();
        let row_hover_bg: Hsla = rgb(if dark { 0x2b394f } else { 0xedf2f8 }).into();
        let items = self.collect_dashboard_audit_item_rows(active_task, cx);

        let mut list = div()
            .id("dashboard-audit-items-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col();

        if items.is_empty() {
            list = list.child(
                div()
                    .p(px(16.))
                    .text_color(theme::muted_foreground())
                    .child("暂无已审计项。通过 Proxy 产生流量或启动扫描任务后，审计项将在此实时列出。"),
            );
        } else {
            let weak = cx.weak_entity();
            for (idx, item) in items.into_iter().enumerate() {
                list = list.child(Self::render_dashboard_audit_item_row(
                    idx,
                    item,
                    row_hover_bg,
                    weak.clone(),
                ));
            }
        }

        div()
            .id("dashboard-audit-items-pane")
            .test_support()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .child(
                div()
                    .h(theme::scaled_px(28.))
                    .flex_shrink_0()
                    .px(px(12.))
                    .bg(header_bg)
                    .border_b_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(theme::scaled_px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(div().w(theme::scaled_px(44.)).flex_shrink_0().child("#"))
                    .child(div().w(theme::scaled_px(64.)).flex_shrink_0().child("方法"))
                    .child(
                        div()
                            .w(theme::scaled_px(170.))
                            .flex_shrink_0()
                            .child("主机"),
                    )
                    .child(div().flex_1().min_w_0().child("URL"))
                    .child(div().w(theme::scaled_px(64.)).flex_shrink_0().child("状态"))
                    .child(div().w(theme::scaled_px(64.)).flex_shrink_0().child("漏洞数"))
                    .child(div().w(theme::scaled_px(84.)).flex_shrink_0().child("进度")),
            )
            .child(list)
            .into_any_element()
    }

}
