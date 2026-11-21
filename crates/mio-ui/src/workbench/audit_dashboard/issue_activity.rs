use std::rc::Rc;

use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::resizable::{resizable_panel, v_resizable};
use gpui_kit::{ClickEvent, uniform_list};

#[derive(Clone)]
enum IssueListRow {
    Single {
        raw_idx: usize,
        finding_id: Uuid,
    },
    GroupHeader {
        group_key: String,
        lead_raw_idx: usize,
        lead_finding_id: Uuid,
        member_ids: Vec<Uuid>,
        count: usize,
        expanded: bool,
    },
    GroupChild {
        raw_idx: usize,
        finding_id: Uuid,
    },
}

struct IssueRowRenderSpec<'a> {
    index: usize,
    row_kind: &'a IssueListRow,
    finding: &'a AuditFinding,
    selected_id: Option<Uuid>,
    selected_set: &'a HashSet<Uuid>,
    col_widths: [f32; DASHBOARD_ISSUE_COL_COUNT],
    visible_cols: &'a [usize],
    flex_col_ix: usize,
    zebra: bool,
    stripe_bg: Hsla,
    weak: WeakEntity<Workbench>,
    dashboard_view_weak: WeakEntity<AuditDashboardView>,
    visible_order: Rc<Vec<Uuid>>,
}

fn size_issue_column(
    cell: gpui_kit::Div,
    col_ix: usize,
    flex_col_ix: usize,
    width: f32,
) -> AnyElement {
    if col_ix == flex_col_ix {
        cell.flex_1()
            .min_w(theme::scaled_px(width).min(px(140.)))
            .into_any_element()
    } else {
        cell.w(theme::scaled_px(width))
            .flex_shrink_0()
            .into_any_element()
    }
}

impl Workbench {
    fn collect_filtered_issue_indices(
        &self,
        active_task: Option<&AuditTaskSummary>,
        cx: &Context<Self>,
    ) -> Vec<(usize, Uuid)> {
        let severity_filter = self.audit_dashboard.severity_filter;
        let keyword = self
            .audit_dashboard
            .issue_filter_input
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let scope_only_live = active_task.is_none()
            && self.audit_dashboard.live_task_scope == LiveTaskTrafficScope::SuiteScopeOnly;

        let mut filtered_indices: Vec<(usize, Uuid)> = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .enumerate()
            .filter(|(_, finding)| match active_task {
                Some(task) => self.finding_belongs_to_audit_task(finding, task),
                None if scope_only_live => self.workspace.scope.is_url_in_scope(&finding.url),
                None => true,
            })
            .filter(|(_, finding)| {
                severity_filter.is_none_or(|expected| {
                    !self
                        .audit_dashboard
                        .false_positive_findings
                        .contains(&finding.id)
                        && finding.severity() == expected
                })
            })
            .filter(|(_, finding)| {
                keyword.is_empty()
                    || [
                        finding.title.as_str(),
                        finding.issue_id.as_str(),
                        finding.host.as_str(),
                        finding.url.as_str(),
                        finding.parameter.as_deref().unwrap_or(""),
                        finding.summary.as_str(),
                        finding.evidence.as_str(),
                        self.audit_dashboard
                            .finding_notes
                            .get(&finding.id)
                            .map(String::as_str)
                            .unwrap_or(""),
                    ]
                    .into_iter()
                    .any(|field| field.to_lowercase().contains(&keyword))
            })
            .map(|(idx, finding)| (idx, finding.id))
            .collect();

        if let Some((sort_col, asc)) = self.audit_dashboard.issue_sort {
            let findings = &self.audit_dashboard.snapshot.findings;
            let fp_set = &self.audit_dashboard.false_positive_findings;
            let ver_set = &self.audit_dashboard.verified_confidence_findings;
            filtered_indices.sort_by(|(idx_a, id_a), (idx_b, id_b)| {
                let fa = &findings[*idx_a];
                let fb = &findings[*idx_b];
                let ord = match sort_col {
                    DashboardIssueSortColumn::Severity => {
                        let rank_a = if fp_set.contains(id_a) {
                            0
                        } else {
                            fa.severity().rank()
                        };
                        let rank_b = if fp_set.contains(id_b) {
                            0
                        } else {
                            fb.severity().rank()
                        };
                        rank_a.cmp(&rank_b).then_with(|| fa.title.cmp(&fb.title))
                    }
                    DashboardIssueSortColumn::IssueType => fa.title.cmp(&fb.title),
                    DashboardIssueSortColumn::Host => {
                        let ha = match &fa.parameter {
                            Some(p) if !p.is_empty() => format!("{} ({p})", fa.host),
                            _ => fa.host.clone(),
                        };
                        let hb = match &fb.parameter {
                            Some(p) if !p.is_empty() => format!("{} ({p})", fb.host),
                            _ => fb.host.clone(),
                        };
                        ha.cmp(&hb)
                    }
                    DashboardIssueSortColumn::Url => fa.url.cmp(&fb.url),
                    DashboardIssueSortColumn::Confidence => {
                        let ca = if ver_set.contains(id_a) {
                            4u8
                        } else {
                            match fa.confidence {
                                IssueConfidence::Certain => 3,
                                IssueConfidence::Firm => 2,
                                IssueConfidence::Tentative => 1,
                            }
                        };
                        let cb = if ver_set.contains(id_b) {
                            4u8
                        } else {
                            match fb.confidence {
                                IssueConfidence::Certain => 3,
                                IssueConfidence::Firm => 2,
                                IssueConfidence::Tentative => 1,
                            }
                        };
                        ca.cmp(&cb)
                    }
                }
                .then_with(|| idx_a.cmp(idx_b));
                if asc { ord } else { ord.reverse() }
            });
        }
        filtered_indices
    }

    fn render_issue_filter_bar(
        &self,
        filtered_count: usize,
        grouped_evidence: bool,
        toolbar_scope: AuditReportScope,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let severity_filter = self.audit_dashboard.severity_filter;
        let can_operate = self.audit_can_operate();
        let finding_count = self.audit_dashboard.snapshot.findings.len();
        let loading = self.audit_dashboard.loading;
        let picker_busy = self.file_picker_pending();
        let save_menu_owner = cx.weak_entity();

        toolbar()
            .id("dashboard-issue-filter-bar")
            .test_support()
            .children(
                [
                    ("dashboard-severity-all", "全部", None),
                    ("dashboard-severity-high", "High", Some(IssueSeverity::High)),
                    (
                        "dashboard-severity-medium",
                        "Medium",
                        Some(IssueSeverity::Medium),
                    ),
                    ("dashboard-severity-low", "Low", Some(IssueSeverity::Low)),
                    (
                        "dashboard-severity-info",
                        "Information",
                        Some(IssueSeverity::Information),
                    ),
                ]
                .into_iter()
                .map(|(id, label, target)| {
                    button(id, label)
                        .tooltip(format!("按严重度筛选漏洞活动：{label}"))
                        .selected(severity_filter == target)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.audit_dashboard.severity_filter = target;
                            cx.notify();
                        }))
                }),
            )
            .child(
                div().min_w(px(120.)).max_w(px(210.)).flex_1().child(
                    Input::new(&self.audit_dashboard.issue_filter_input)
                        .id("dashboard-issue-filter")
                        .aria_label("漏洞活动关键字过滤")
                        .small(),
                ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(theme::scaled_px(12.))
                    .text_color(theme::muted_foreground())
                    .child(format!(
                        "显示 {}/{}{}",
                        filtered_count,
                        finding_count,
                        if grouped_evidence { " · 分组" } else { "" }
                    )),
            )
            .child(
                button("dashboard-audit-refresh", "刷新")
                    .icon(Icon::new(IconName::RefreshCw).size(theme::scaled_px(12.)))
                    .tooltip("从后台服务重新同步扫描任务与漏洞列表")
                    .disabled(!can_operate || loading)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(project) = this.project {
                            this.query_audit_state(project, cx);
                        }
                    })),
            )
            .child(
                button("dashboard-copy-json-report", "复制 JSON")
                    .icon(Icon::new(IconName::Copy).size(theme::scaled_px(12.)))
                    .tooltip("将当前漏洞告警导出为结构化 JSON 复制到剪贴板")
                    .disabled(!can_operate)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.copy_audit_report_json_for_scope(toolbar_scope, cx);
                    })),
            )
            .child(
                button("dashboard-copy-html-report", "复制 HTML")
                    .icon(Icon::new(IconName::Download).size(theme::scaled_px(12.)))
                    .tooltip("将当前漏洞告警生成独立 HTML 审计报告复制到剪贴板")
                    .disabled(!can_operate)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.copy_audit_report_html_for_scope(toolbar_scope, cx);
                    })),
            )
            .child(
                button("dashboard-save-report-file", "保存报告…")
                    .icon(Icon::new(IconName::Download).size(theme::scaled_px(12.)))
                    .dropdown_caret(true)
                    .tooltip("将当前漏洞审计报告导出为 HTML / JSON / XML 文件写入磁盘")
                    .disabled(!can_operate || picker_busy)
                    .dropdown_menu(move |menu, _, cx| {
                        let Some(entity) = save_menu_owner.upgrade() else {
                            return menu;
                        };
                        let enabled = {
                            let view = entity.read(cx);
                            view.audit_can_operate() && !view.file_picker_pending()
                        };
                        let html_owner = save_menu_owner.clone();
                        let json_owner = save_menu_owner.clone();
                        let xml_owner = save_menu_owner.clone();
                        menu.item(
                            PopupMenuItem::new("保存为 HTML 审计报告 (.html)…")
                                .disabled(!enabled)
                                .on_click(move |_, window, cx| {
                                    let _ = html_owner.update(cx, |view, cx| {
                                        view.save_audit_report_with_format(
                                            AuditReportFormat::Html,
                                            toolbar_scope,
                                            window,
                                            cx,
                                        );
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("保存为 JSON 审计报告 (.json)…")
                                .disabled(!enabled)
                                .on_click(move |_, window, cx| {
                                    let _ = json_owner.update(cx, |view, cx| {
                                        view.save_audit_report_with_format(
                                            AuditReportFormat::Json,
                                            toolbar_scope,
                                            window,
                                            cx,
                                        );
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("保存为 XML 审计报告 (.xml)…")
                                .disabled(!enabled)
                                .on_click(move |_, window, cx| {
                                    let _ = xml_owner.update(cx, |view, cx| {
                                        view.save_audit_report_with_format(
                                            AuditReportFormat::Xml,
                                            toolbar_scope,
                                            window,
                                            cx,
                                        );
                                    });
                                }),
                        )
                    }),
            )
            .child(
                button("dashboard-clear-issues", "清空")
                    .icon(Icon::new(IconName::Trash).size(theme::scaled_px(12.)))
                    .tooltip("清空当前项目中的所有扫描发现与漏洞告警记录")
                    .disabled(!can_operate || finding_count == 0)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.clear_audit_findings(window, cx);
                    })),
            )
            .into_any_element()
    }

    pub(super) fn render_issue_activity_pane(
        &mut self,
        view_ctx: &DashboardViewContext<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active_task = if self.has_dashboard_intruder_selection() {
            None
        } else {
            self.audit_dashboard
                .selected_audit_task_id
                .and_then(|task_id| {
                    self.audit_dashboard
                        .snapshot
                        .tasks
                        .iter()
                        .find(|t| t.id == task_id)
                        .cloned()
                })
        };
        let active_task_id = active_task.as_ref().map(|t| t.id);
        let selected_id = self.audit_dashboard.selected_finding_id;
        let filtered_indices = self.collect_filtered_issue_indices(active_task.as_ref(), cx);

        let grouped_evidence = self
            .user_preferences
            .confirmed
            .as_ref()
            .map(|p| p.display.issues_grouped_evidence)
            .unwrap_or(self.user_preferences.display.issues_grouped_evidence);

        let selected_finding = selected_id.and_then(|id| {
            self.audit_dashboard
                .snapshot
                .findings
                .iter()
                .find(|f| f.id == id)
                .cloned()
        });
        let toolbar_scope = match active_task_id {
            Some(tid) => AuditReportScope::Task(tid),
            None => AuditReportScope::All,
        };
        let filter_bar = self.render_issue_filter_bar(
            filtered_indices.len(),
            grouped_evidence,
            toolbar_scope,
            cx,
        );

        let task_filter_banner = active_task.as_ref().map(|task| {
            let task_title = if task.title.is_empty() {
                format!("扫描任务 · {}", &task.id.to_string()[..8])
            } else {
                task.title.clone()
            };
            let hint = format!(
                "当前按任务筛选：{}（目标 {}）",
                task_title, task.target_url
            );
            toolbar()
                .id("dashboard-task-filter-banner")
                .test_support()
                .bg(theme::toolbar_surface())
                .child(
                    div()
                        .id("dashboard-task-filter-label")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(hint.clone())
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(hint),
                )
                .child(
                    button("dashboard-show-all-task-issues", "显示全部任务漏洞")
                        .icon(IconName::Eye)
                        .tooltip("清除当前扫描任务关联筛选，展示全部漏洞告警")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.audit_dashboard.selected_audit_task_id = None;
                            cx.notify();
                        })),
                )
                .into_any_element()
        });

        let list_section = self.render_issue_activity_list(
            filtered_indices,
            selected_id,
            grouped_evidence,
            view_ctx,
            cx,
        );
        let detail_section = match selected_finding {
            Some(finding) => self.render_issue_finding_detail(&finding, cx),
            None => div()
                .id("dashboard-issue-detail-empty")
                .test_support()
                .size_full()
                .min_h_0()
                .p(px(16.))
                .text_color(rgb(if theme::is_dark() { 0x9da2a8 } else { 0x666666 }))
                .child("选择上方任意漏洞条目查看修复建议（Advisory）、关联请求响应报文及证据高亮偏移。")
                .into_any_element(),
        };

        div()
            .id("dashboard-issue-activity-pane")
            .test_support()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(filter_bar)
            .when_some(task_filter_banner, |pane, banner| pane.child(banner))
            .child(
                div().flex_1().min_h_0().child(
                    v_resizable("dashboard-issues-vsplit")
                        .child(
                            resizable_panel()
                                .size(px(220.))
                                .size_range(px(110.)..px(720.))
                                .child(list_section),
                        )
                        .child(
                            resizable_panel()
                                .size_range(px(120.)..px(10000.))
                                .child(detail_section),
                        ),
                ),
            )
            .into_any_element()
    }

    fn build_issue_display_rows(
        &self,
        filtered_indices: Vec<(usize, Uuid)>,
        grouped_evidence: bool,
        expanded_issue_groups: &HashSet<String>,
    ) -> Vec<IssueListRow> {
        if !grouped_evidence {
            return filtered_indices
                .into_iter()
                .map(|(raw_idx, finding_id)| IssueListRow::Single {
                    raw_idx,
                    finding_id,
                })
                .collect();
        }
        let mut group_order: Vec<(String, Vec<(usize, Uuid)>)> = Vec::new();
        let mut group_index: HashMap<String, usize> = HashMap::new();
        for (raw_idx, finding_id) in filtered_indices {
            let Some(finding) = self.audit_dashboard.snapshot.findings.get(raw_idx) else {
                continue;
            };
            let key = format!("{}|{}", finding.issue_id, finding.host);
            if let Some(&pos) = group_index.get(&key) {
                group_order[pos].1.push((raw_idx, finding_id));
            } else {
                group_index.insert(key.clone(), group_order.len());
                group_order.push((key, vec![(raw_idx, finding_id)]));
            }
        }
        let mut out = Vec::with_capacity(group_order.len());
        for (group_key, members) in group_order {
            if members.len() == 1 {
                let (raw_idx, finding_id) = members[0];
                out.push(IssueListRow::Single {
                    raw_idx,
                    finding_id,
                });
            } else {
                let (lead_raw_idx, lead_finding_id) = members[0];
                let member_ids: Vec<Uuid> = members.iter().map(|&(_, id)| id).collect();
                let expanded = expanded_issue_groups.contains(&group_key);
                out.push(IssueListRow::GroupHeader {
                    group_key,
                    lead_raw_idx,
                    lead_finding_id,
                    member_ids,
                    count: members.len(),
                    expanded,
                });
                if expanded {
                    for (raw_idx, finding_id) in members {
                        out.push(IssueListRow::GroupChild {
                            raw_idx,
                            finding_id,
                        });
                    }
                }
            }
        }
        out
    }

    fn render_issue_table_header(
        &self,
        visible_cols: &[usize],
        flex_col_ix: usize,
        all_finding_ids: &Rc<Vec<Uuid>>,
        view_ctx: &DashboardViewContext<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let col_widths = self.audit_dashboard.issue_col_widths;
        let col_visible = self.audit_dashboard.issue_col_visible;
        let issue_sort = self.audit_dashboard.issue_sort;
        let dashboard_view_id = view_ctx.view_entity_id;
        let dashboard_view_weak = view_ctx.view.clone();
        let weak = cx.weak_entity();
        let header_defs: [(&'static str, &'static str, DashboardIssueSortColumn);
            DASHBOARD_ISSUE_COL_COUNT] = [
            (
                "dashboard-issue-col-severity",
                "严重度",
                DashboardIssueSortColumn::Severity,
            ),
            (
                "dashboard-issue-col-title",
                "漏洞名称",
                DashboardIssueSortColumn::IssueType,
            ),
            (
                "dashboard-issue-col-host",
                "主机 / 参数",
                DashboardIssueSortColumn::Host,
            ),
            (
                "dashboard-issue-col-url",
                "URL",
                DashboardIssueSortColumn::Url,
            ),
            (
                "dashboard-issue-col-confidence",
                "置信度",
                DashboardIssueSortColumn::Confidence,
            ),
        ];

        let header_cells: Vec<AnyElement> = visible_cols
            .iter()
            .map(|&col_ix| {
                let (id, label, sort_col) = header_defs[col_ix];
                let width = col_widths[col_ix];
                let active_asc = issue_sort
                    .filter(|(c, _)| *c == sort_col)
                    .map(|(_, asc)| asc);
                let ctx_view = weak.clone();
                let ctx_ids = Rc::clone(all_finding_ids);
                let fit_ids = Rc::clone(all_finding_ids);
                let resize_start_view = dashboard_view_weak.clone();
                let resize_handle = div()
                    .id(("dashboard-issue-col-resize", col_ix))
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
                                this.fit_audit_issue_column_width(col_ix, &fit_ids, cx);
                            } else {
                                let start_w = this.audit_dashboard.issue_col_widths[col_ix];
                                let _ = resize_start_view.update(cx, |view, _| {
                                    view.begin_issue_column_resize(
                                        col_ix,
                                        event.position.x.as_f32(),
                                        start_w,
                                    );
                                });
                            }
                        }),
                    )
                    .on_drag(
                        ResizeDashboardIssueCol {
                            entity_id: dashboard_view_id,
                            col_ix,
                        },
                        |drag, _, _, cx| {
                            cx.stop_propagation();
                            cx.new(|_| *drag)
                        },
                    );
                let cell = div()
                    .id(id)
                    .test_support()
                    .role(Role::ColumnHeader)
                    .aria_label(label)
                    .relative()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .pr(px(6.))
                    .overflow_hidden()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_audit_issue_sort(sort_col, cx);
                    }))
                    .context_menu(move |menu, window, cx| {
                        issue_menu::build_issue_header_context_menu(
                            ctx_view.clone(),
                            col_ix,
                            sort_col,
                            Rc::clone(&ctx_ids),
                            col_visible,
                            menu,
                            window,
                            cx,
                        )
                    })
                    .child(div().min_w_0().truncate().child(label))
                    .when_some(active_asc, |c, asc| {
                        c.child(
                            Icon::new(if asc {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .size(theme::scaled_px(11.))
                            .text_color(theme::muted_foreground())
                            .flex_shrink_0(),
                        )
                    })
                    .child(resize_handle);
                if col_ix == flex_col_ix {
                    cell.flex_1()
                        .min_w(theme::scaled_px(width).min(px(140.)))
                        .into_any_element()
                } else {
                    cell.w(theme::scaled_px(width))
                        .flex_shrink_0()
                        .into_any_element()
                }
            })
            .collect();

        let drag_view = dashboard_view_weak;
        div()
            .min_h(theme::scaled_px(28.))
            .h(theme::scaled_px(28.))
            .px(px(10.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .bg(theme::toolbar_surface())
            .border_b_1()
            .border_color(theme::border())
            .text_size(theme::scaled_px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .on_drag_move::<ResizeDashboardIssueCol>(
                move |event: &DragMoveEvent<ResizeDashboardIssueCol>, _, cx| {
                    let drag = *event.drag(cx);
                    if drag.entity_id == dashboard_view_id {
                        let _ = drag_view.update(cx, |view, cx| {
                            view.update_issue_column_resize(
                                drag.col_ix,
                                event.event.position.x.as_f32(),
                                cx,
                            );
                        });
                    }
                },
            )
            .children(header_cells)
            .into_any_element()
    }

    fn render_issue_list_row(&self, spec: IssueRowRenderSpec<'_>) -> AnyElement {
        let finding = spec.finding;
        let id = finding.id;
        let is_child = matches!(spec.row_kind, IssueListRow::GroupChild { .. });
        let (group_info, row_members) = match spec.row_kind {
            IssueListRow::GroupHeader {
                group_key,
                count,
                expanded,
                member_ids,
                ..
            } => (Some((group_key.clone(), *count, *expanded)), member_ids.clone()),
            _ => (None, vec![id]),
        };
        let selected = row_members.iter().any(|mid| spec.selected_set.contains(mid))
            || (spec.selected_set.is_empty() && spec.selected_id == Some(id));
        let is_fp = self.audit_dashboard.false_positive_findings.contains(&id);
        let is_verified = self
            .audit_dashboard
            .verified_confidence_findings
            .contains(&id);
        let has_note = self
            .audit_dashboard
            .finding_notes
            .get(&id)
            .is_some_and(|n| !n.trim().is_empty());
        let hl_color = self
            .audit_dashboard
            .finding_highlights
            .get(&id)
            .copied()
            .unwrap_or(0);
        let (sev_text, badge_bg, badge_fg) = severity_display_style(finding.severity(), is_fp);
        let host_param = match &finding.parameter {
            Some(param) if !param.is_empty() => format!("{} ({param})", finding.host),
            _ => finding.host.clone(),
        };
        let row_label = match &group_info {
            Some((_, grp_count, _)) => {
                format!("{} {} [{}] {}", sev_text, finding.title, grp_count, finding.url)
            }
            None => format!("{} {} {}", sev_text, finding.title, finding.url),
        };
        let row_id = if is_child {
            SharedString::from(format!("dashboard-issue-evidence-row-{id}"))
        } else {
            SharedString::from(format!("dashboard-issue-activity-row-{id}"))
        };
        let row_bg = if let Some(hl_bg) = crate::history::highlight_background(hl_color) {
            hl_bg
        } else if selected {
            theme::selected()
        } else if spec.zebra && spec.index % 2 == 1 {
            spec.stripe_bg
        } else {
            theme::surface()
        };
        let row_fg = if hl_color > 0 {
            crate::history::highlight_foreground(hl_color)
        } else {
            theme::foreground()
        };
        let conf_fg = if hl_color > 0 {
            crate::history::highlight_foreground(hl_color)
        } else {
            rgb(if theme::is_dark() { 0x9da2a8 } else { 0x555555 }).into()
        };

        let mut title_cell = div().flex().items_center().gap(px(5.)).overflow_hidden();
        if let Some((group_key, grp_count, expanded)) = group_info {
            let gk = group_key.clone();
            let chevron_view = spec.dashboard_view_weak;
            title_cell = title_cell
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "dashboard-issue-group-toggle-{id}"
                        )))
                        .test_support()
                        .flex_shrink_0()
                        .size(theme::scaled_px(16.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            let _ = chevron_view.update(cx, |view, cx| {
                                view.toggle_issue_group(&gk, cx);
                            });
                        })
                        .child(
                            Icon::new(if expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size(theme::scaled_px(12.))
                            .text_color(row_fg),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(finding.title.clone()),
                )
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "dashboard-issue-group-count-{id}"
                        )))
                        .test_support()
                        .flex_shrink_0()
                        .px(px(5.))
                        .py(px(1.))
                        .rounded(px(9.))
                        .bg(rgb(if theme::is_dark() {
                            0x3c4047
                        } else {
                            0xe5e7eb
                        }))
                        .text_color(theme::foreground())
                        .text_size(theme::scaled_px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("{grp_count}")),
                );
        } else {
            title_cell = title_cell.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(if is_child {
                        FontWeight::NORMAL
                    } else {
                        FontWeight::SEMIBOLD
                    })
                    .child(finding.title.clone()),
            );
        }
        if has_note {
            title_cell = title_cell.child(
                Icon::new(IconName::StickyNote)
                    .size(theme::scaled_px(12.))
                    .text_color(row_fg),
            );
        }
        let mut title_cell_opt = Some(size_issue_column(
            title_cell,
            1,
            spec.flex_col_ix,
            spec.col_widths[1],
        ));

        let row_cells: Vec<AnyElement> = spec
            .visible_cols
            .iter()
            .map(|&col_ix| {
                let w = spec.col_widths[col_ix];
                match col_ix {
                    0 => size_issue_column(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .overflow_hidden()
                            .child(render_severity_circle_styled(
                                finding.severity(),
                                finding.confidence,
                                is_fp,
                            ))
                            .child(
                                div()
                                    .px(px(5.))
                                    .py(px(1.))
                                    .rounded(px(2.))
                                    .bg(rgb(badge_bg))
                                    .text_color(rgb(badge_fg))
                                    .text_size(theme::scaled_px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(sev_text),
                            ),
                        col_ix,
                        spec.flex_col_ix,
                        w,
                    ),
                    1 => title_cell_opt
                        .take()
                        .unwrap_or_else(|| div().into_any_element()),
                    2 => size_issue_column(
                        div().truncate().child(host_param.clone()),
                        col_ix,
                        spec.flex_col_ix,
                        w,
                    ),
                    3 => size_issue_column(
                        div().truncate().child(finding.url.clone()),
                        col_ix,
                        spec.flex_col_ix,
                        w,
                    ),
                    4 => size_issue_column(
                        div()
                            .truncate()
                            .text_color(conf_fg)
                            .child(confidence_display_label(finding.confidence, is_verified)),
                        col_ix,
                        spec.flex_col_ix,
                        w,
                    ),
                    _ => div().into_any_element(),
                }
            })
            .collect();

        let right_view = spec.weak.clone();
        let click_view = spec.weak.clone();
        let menu_view = spec.weak;
        let right_members = row_members.clone();
        let click_members = row_members;
        let click_order = spec.visible_order;

        div()
            .id(row_id)
            .test_support()
            .role(Role::Row)
            .aria_label(row_label)
            .aria_selected(selected)
            .w_full()
            .h(theme::scaled_px(28.))
            .pl(if is_child { px(26.) } else { px(10.) })
            .pr(px(10.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .cursor_pointer()
            .border_b_1()
            .border_color(if selected && hl_color > 0 {
                mio_blue()
            } else {
                theme::border()
            })
            .bg(row_bg)
            .text_color(row_fg)
            .children(row_cells)
            .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                let _ = right_view.update(cx, |this, cx| {
                    this.handle_audit_issue_row_right_down(&right_members, window, cx);
                });
            })
            .on_click(move |event: &ClickEvent, window, cx| {
                let shift = event.modifiers().shift;
                let toggle = event.modifiers().secondary(cx);
                let _ = click_view.update(cx, |this, cx| {
                    this.handle_audit_issue_row_click(
                        &click_members,
                        &click_order,
                        shift,
                        toggle,
                        true,
                        window,
                        cx,
                    );
                });
            })
            .context_menu(move |menu, window, cx| {
                issue_menu::build_issue_row_menu(menu_view.clone(), id, menu, window, cx)
            })
            .into_any_element()
    }

    pub(super) fn render_issue_activity_list(
        &self,
        filtered_indices: Vec<(usize, Uuid)>,
        selected_id: Option<Uuid>,
        grouped_evidence: bool,
        view_ctx: &DashboardViewContext<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let all_finding_ids: Rc<Vec<Uuid>> =
            Rc::new(filtered_indices.iter().map(|&(_, id)| id).collect());
        let display_rows = self.build_issue_display_rows(
            filtered_indices,
            grouped_evidence,
            view_ctx.expanded_issue_groups,
        );
        let visible_order: Rc<Vec<Uuid>> = Rc::new(
            display_rows
                .iter()
                .map(|row| match row {
                    IssueListRow::Single { finding_id, .. }
                    | IssueListRow::GroupChild { finding_id, .. } => *finding_id,
                    IssueListRow::GroupHeader {
                        lead_finding_id, ..
                    } => *lead_finding_id,
                })
                .collect(),
        );

        let col_widths = self.audit_dashboard.issue_col_widths;
        let col_visible = self.audit_dashboard.issue_col_visible;
        let visible_cols: Vec<usize> = (0..DASHBOARD_ISSUE_COL_COUNT)
            .filter(|&i| col_visible[i])
            .collect();
        let flex_col_ix = if col_visible[3] {
            3
        } else {
            visible_cols.last().copied().unwrap_or(0)
        };
        let header = self.render_issue_table_header(
            &visible_cols,
            flex_col_ix,
            &all_finding_ids,
            view_ctx,
            cx,
        );

        let mut rows = div()
            .id("dashboard-issue-rows")
            .test_support()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(header);

        if display_rows.is_empty() {
            rows = rows.child(
                div()
                    .id("dashboard-issue-empty")
                    .test_support()
                    .p(px(14.))
                    .text_color(rgb(if theme::is_dark() { 0x9da2a8 } else { 0x666666 }))
                    .child(if self.audit_dashboard.snapshot.findings.is_empty() {
                        "尚无实时漏洞告警。可通过实时被动审计捕获 Proxy/Repeater 流量，或点击「新建扫描」发起主动/被动扫描。"
                    } else {
                        "当前筛选条件下没有匹配的漏洞告警。"
                    }),
            );
        } else {
            let weak = cx.weak_entity();
            let dashboard_view_weak = view_ctx.view.clone();
            let display_rc = Rc::new(display_rows);
            let count = display_rc.len();
            let selected_set_rc = Rc::new(self.audit_dashboard.selected_finding_ids.clone());
            let zebra = theme::table_zebra_stripe();
            let stripe_bg: Hsla = rgb(if theme::is_dark() { 0x25272b } else { 0xfafafa }).into();
            rows = rows.child(
                uniform_list("dashboard-issue-list", count, move |range, _, cx| {
                    let Some(owner) = weak.upgrade() else {
                        return Vec::new();
                    };
                    let view = owner.read(cx);
                    range
                        .filter_map(|index| {
                            let row_kind = display_rc.get(index)?;
                            let (raw_idx, finding_id) = match row_kind {
                                IssueListRow::Single {
                                    raw_idx,
                                    finding_id,
                                }
                                | IssueListRow::GroupChild {
                                    raw_idx,
                                    finding_id,
                                } => (*raw_idx, *finding_id),
                                IssueListRow::GroupHeader {
                                    lead_raw_idx,
                                    lead_finding_id,
                                    ..
                                } => (*lead_raw_idx, *lead_finding_id),
                            };
                            let finding = view
                                .audit_dashboard
                                .snapshot
                                .findings
                                .get(raw_idx)
                                .filter(|item| item.id == finding_id)?;
                            Some(view.render_issue_list_row(IssueRowRenderSpec {
                                index,
                                row_kind,
                                finding,
                                selected_id,
                                selected_set: &selected_set_rc,
                                col_widths,
                                visible_cols: &visible_cols,
                                flex_col_ix,
                                zebra,
                                stripe_bg,
                                weak: weak.clone(),
                                dashboard_view_weak: dashboard_view_weak.clone(),
                                visible_order: Rc::clone(&visible_order),
                            }))
                        })
                        .collect::<Vec<_>>()
                })
                .w_full()
                .flex_1()
                .min_h_0(),
            );
        }
        rows.into_any_element()
    }
}
