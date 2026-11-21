use super::*;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;

const MIO_SEV_BANG_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="currentColor" fill-rule="evenodd" d="M12 2 A10 10 0 1 0 12 22 A10 10 0 1 0 12 2 Z M10.9 6.5 H13.1 V13.5 H10.9 Z M10.9 15.3 H13.1 V17.5 H10.9 Z"/></svg>"#;

const MIO_SEV_QUESTION_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="currentColor" fill-rule="evenodd" d="M12 2 A10 10 0 1 0 12 22 A10 10 0 1 0 12 2 Z M10.9 15.5 H13.1 V17.5 H10.9 Z M10.9 13.7 V12.3 C10.9 11.4 11.5 10.8 12.2 10.4 C12.9 10.0 13.3 9.6 13.3 9.0 C13.3 8.2 12.7 7.7 11.9 7.7 C11.1 7.7 10.5 8.2 10.4 9.0 L8.5 8.8 C8.7 7.1 10.1 6.0 12.0 6.0 C13.9 6.0 15.3 7.1 15.3 8.9 C15.3 10.2 14.5 11.0 13.6 11.5 C13.0 11.9 12.8 12.2 12.8 12.8 V13.7 Z"/></svg>"#;

const MIO_SEV_INFO_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="currentColor" fill-rule="evenodd" d="M12 2 A10 10 0 1 0 12 22 A10 10 0 1 0 12 2 Z M10.9 6.3 H13.1 V8.5 H10.9 Z M10.9 10.3 H13.1 V17.5 H10.9 Z"/></svg>"#;

const MIO_SEV_FP_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="currentColor" fill-rule="evenodd" d="M12 2 A10 10 0 1 0 12 22 A10 10 0 1 0 12 2 Z M8.2 9.6 L9.6 8.2 L12 10.6 L14.4 8.2 L15.8 9.6 L13.4 12 L15.8 14.4 L14.4 15.8 L12 13.4 L9.6 15.8 L8.2 14.4 L10.6 12 Z"/></svg>"#;

pub(super) const ISSUE_HIGHLIGHT_LABELS: [&str; 10] = [
    "无 (None)",
    "红色 (Red)",
    "橙色 (Orange)",
    "黄色 (Yellow)",
    "绿色 (Green)",
    "青色 (Cyan)",
    "蓝色 (Blue)",
    "粉色 (Pink)",
    "品红 (Magenta)",
    "灰色 (Gray)",
];

fn severity_menu_icon(
    severity: IssueSeverity,
    confidence: IssueConfidence,
    is_false_positive: bool,
) -> Icon {
    let (svg, hex) = if is_false_positive {
        (MIO_SEV_FP_SVG, 0x8c96a0)
    } else {
        let color = match severity {
            IssueSeverity::High => 0xcc2929,
            IssueSeverity::Medium => 0xe86c3a,
            IssueSeverity::Low => 0x3a71a8,
            IssueSeverity::Information => 0x8c96a0,
        };
        let svg = match severity {
            IssueSeverity::Information => MIO_SEV_INFO_SVG,
            _ => match confidence {
                IssueConfidence::Tentative => MIO_SEV_QUESTION_SVG,
                IssueConfidence::Certain | IssueConfidence::Firm => MIO_SEV_BANG_SVG,
            },
        };
        (svg, color)
    };
    Icon::default()
        .data(svg)
        .size(theme::scaled_px(15.))
        .text_color(rgb(hex))
}

pub(super) fn format_finding_context_text(
    finding: &AuditFinding,
    is_false_positive: bool,
    is_verified: bool,
    note: Option<&str>,
    loaded_flow: Option<&FlowRecord>,
) -> String {
    let severity_text = if is_false_positive {
        "False positive (误报)"
    } else {
        finding.severity().bilingual_label()
    };
    let confidence_text = confidence_display_label(finding.confidence, is_verified);
    let param_text = finding.parameter.as_deref().unwrap_or("无");
    let mut out = format!(
        "问题标题：{}\n问题类型 ID：{}\n严重性：{severity_text}\n置信度：{confidence_text}\n来源：{}\n主机：{}\nURL：{}\n参数：{param_text}\n\n问题摘要：\n{}\n\n漏洞证据：\n{}\n\n修复建议：\n{}",
        finding.title,
        finding.issue_id,
        phase_label(finding.phase),
        finding.host,
        finding.url,
        finding.summary,
        finding.evidence,
        finding.remediation,
    );
    if let Some(note_text) = note.filter(|n| !n.trim().is_empty()) {
        out.push_str("\n\n备注：\n");
        out.push_str(note_text.trim());
    }
    if let Some(flow) = loaded_flow {
        let req_raw = request_to_raw(&flow.request);
        if !req_raw.is_empty() {
            let preview: String = String::from_utf8_lossy(&req_raw).chars().take(4096).collect();
            out.push_str("\n\n关联 HTTP 请求：\n");
            out.push_str(&preview);
        }
        if let Some(resp) = &flow.response {
            let resp_raw = response_to_raw(resp);
            if !resp_raw.is_empty() {
                let preview: String =
                    String::from_utf8_lossy(&resp_raw).chars().take(4096).collect();
                out.push_str("\n\n关联 HTTP 响应：\n");
                out.push_str(&preview);
            }
        }
    }
    out
}

pub(super) fn build_issue_row_menu(
    view: WeakEntity<Workbench>,
    finding_id: Uuid,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(entity) = view.upgrade() else {
        return menu;
    };
    let (
        can_operate,
        picker_busy,
        title,
        current_sev,
        current_conf,
        is_fp,
        is_verified,
        can_restore_sev,
        can_restore_conf,
        current_hl,
        target_ids,
        has_flows,
    ) = {
        let wb = entity.read(cx);
        let finding = wb
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id);
        let (title, sev, conf) = finding.map_or(
            (
                "未知问题".to_owned(),
                IssueSeverity::Information,
                IssueConfidence::Tentative,
            ),
            |f| (f.title.clone(), f.severity(), f.confidence),
        );
        let is_fp = wb.audit_dashboard.false_positive_findings.contains(&finding_id);
        let is_verified = wb
            .audit_dashboard
            .verified_confidence_findings
            .contains(&finding_id);
        let orig = wb.audit_dashboard.original_triage.get(&finding_id).copied();
        let can_restore_sev = is_fp || orig.is_some_and(|(orig_sev, _)| orig_sev != sev);
        let can_restore_conf = is_verified || orig.is_some_and(|(_, orig_conf)| orig_conf != conf);
        let hl = wb
            .audit_dashboard
            .finding_highlights
            .get(&finding_id)
            .copied()
            .unwrap_or(0);
        let target_ids = wb.audit_selected_target_ids(finding_id);
        let has_flows = wb
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .any(|f| target_ids.contains(&f.id) && f.flow_id.is_some());
        (
            wb.audit_can_operate(),
            wb.file_picker_pending(),
            title,
            sev,
            conf,
            is_fp,
            is_verified,
            can_restore_sev,
            can_restore_conf,
            hl,
            target_ids,
            has_flows,
        )
    };

    let multi = target_ids.len() > 1;
    let header_title = if multi {
        format!("已选中 {} 条问题", target_ids.len())
    } else {
        title.clone()
    };
    let header_icon = severity_menu_icon(current_sev, current_conf, is_fp);
    let explore_view = view.clone();
    let goto_view = view.clone();
    let report_view = view.clone();
    let del_view = view.clone();
    let sev_view = view.clone();
    let conf_view = view.clone();
    let rep_view = view.clone();
    let intr_view = view.clone();
    let org_view = view.clone();
    let comparer_view = view.clone();
    let copy_url_view = view.clone();
    let mio_at_view = view.clone();
    let notes_view = view.clone();
    let hl_view = view;

    let del_ids = target_ids.clone();
    let sev_ids = target_ids.clone();
    let conf_ids = target_ids.clone();
    let rep_ids = target_ids.clone();
    let intr_ids = target_ids.clone();
    let org_ids = target_ids.clone();
    let cmp_ids = target_ids.clone();
    let url_ids = target_ids.clone();
    let hl_ids = target_ids;

    let highlight_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        (0..=9u8).fold(sub.min_w(theme::scaled_px(136.)), |sub, color| {
            let owner = hl_view.clone();
            let ids = hl_ids.clone();
            let label = ISSUE_HIGHLIGHT_LABELS[usize::from(color)];
            let checked = current_hl == color;
            sub.item(
                PopupMenuItem::element(move |_, _| {
                    let fg = if color == 0 {
                        theme::foreground()
                    } else {
                        crate::history::highlight_foreground(color)
                    };
                    div()
                        .id(("dashboard-issue-highlight-color", usize::from(color)))
                        .test_support()
                        .role(Role::MenuItem)
                        .aria_label(format!("高亮 {label}"))
                        .w_full()
                        .h(theme::scaled_px(24.).max(px(24.)))
                        .mx_neg_2()
                        .px(theme::scaled_px(8.))
                        .flex()
                        .items_center()
                        .gap(theme::scaled_px(6.))
                        .when_some(crate::history::highlight_background(color), |this, bg| {
                            this.bg(bg)
                        })
                        .text_color(fg)
                        .child(
                            div()
                                .w(theme::scaled_px(10.))
                                .h(theme::scaled_px(10.))
                                .rounded(px(2.))
                                .when(color == 0, |this| {
                                    this.border_1().border_dashed().border_color(rgb(0x888888))
                                }),
                        )
                        .child(label)
                })
                .checked(checked)
                .on_click(move |_, _, cx| {
                    let _ = owner.update(cx, |v, cx| {
                        v.set_audit_findings_highlight_batch(&ids, color, cx);
                    });
                }),
            )
        })
    });

    menu.item(
        PopupMenuItem::element(move |_, _| {
            div()
                .max_w(px(240.))
                .truncate()
                .font_weight(FontWeight::BOLD)
                .text_color(theme::foreground())
                .child(header_title.clone())
        })
        .icon(header_icon)
        .disabled(true),
    )
    .item(
        PopupMenuItem::new("探索问题 (Explore issue)")
            .icon(
                Icon::new(IconName::Sparkles)
                    .size(theme::scaled_px(14.))
                    .text_color(rgb(0x6d28d9)),
            )
            .on_click(move |_, window, cx| {
                let _ = explore_view.update(cx, |v, cx| {
                    v.explore_audit_finding(finding_id, window, cx);
                });
            }),
    )
    .item(
        PopupMenuItem::new("跳转到问题 (Go to issue)").on_click(move |_, window, cx| {
            let _ = goto_view.update(cx, |v, cx| {
                v.go_to_audit_finding_in_target(finding_id, window, cx);
            });
        }),
    )
    .item(
        PopupMenuItem::new("报告问题 (Report issue)")
            .disabled(!can_operate || picker_busy)
            .on_click(move |_, window, cx| {
                let _ = report_view.update(cx, |v, cx| {
                    v.save_audit_report_with_format(
                        AuditReportFormat::Html,
                        AuditReportScope::SingleFinding(finding_id),
                        window,
                        cx,
                    );
                });
            }),
    )
    .item(
        PopupMenuItem::new("删除问题 (Delete issue)")
            .disabled(!can_operate)
            .on_click(move |_, window, cx| {
                let _ = del_view.update(cx, |v, cx| {
                    v.delete_audit_findings_batch(&del_ids, window, cx);
                });
            }),
    )
    .separator()
    .submenu("设置严重性 (Set severity)", window, cx, move |sub, _, _| {
        let fp_owner = sev_view.clone();
        let fp_ids = sev_ids.clone();
        let restore_owner = sev_view.clone();
        let restore_ids = sev_ids.clone();
        let sub = [
            ("高 (High)", IssueSeverity::High),
            ("中 (Medium)", IssueSeverity::Medium),
            ("低 (Low)", IssueSeverity::Low),
            ("信息 (Information)", IssueSeverity::Information),
        ]
        .into_iter()
        .fold(sub, |sub, (label, sev)| {
            let owner = sev_view.clone();
            let ids = sev_ids.clone();
            sub.item(
                PopupMenuItem::new(label)
                    .checked(!multi && !is_fp && current_sev == sev)
                    .disabled(!can_operate)
                    .on_click(move |_, _, cx| {
                        let _ = owner.update(cx, |v, cx| {
                            v.set_audit_findings_severity_batch(&ids, sev, cx);
                        });
                    }),
            )
        });
        sub.item(
            PopupMenuItem::new("误报 (False positive)")
                .checked(!multi && is_fp)
                .disabled(!can_operate)
                .on_click(move |_, _, cx| {
                    let _ = fp_owner.update(cx, |v, cx| {
                        v.mark_audit_findings_false_positive_batch(&fp_ids, cx);
                    });
                }),
        )
        .separator()
        .item(
            PopupMenuItem::new("恢复原始值 (Restore original value)")
                .disabled(!can_operate || (!multi && !can_restore_sev))
                .on_click(move |_, _, cx| {
                    let _ = restore_owner.update(cx, |v, cx| {
                        v.restore_audit_findings_severity_batch(&restore_ids, cx);
                    });
                }),
        )
    })
    .submenu("设置置信度 (Set confidence)", window, cx, move |sub, _, _| {
        let verified_owner = conf_view.clone();
        let verified_ids = conf_ids.clone();
        let restore_owner = conf_view.clone();
        let restore_ids = conf_ids.clone();
        let sub = sub.item(
            PopupMenuItem::new("已验证 (Verified)")
                .checked(!multi && is_verified)
                .disabled(!can_operate)
                .on_click(move |_, _, cx| {
                    let _ = verified_owner.update(cx, |v, cx| {
                        v.mark_audit_findings_verified_batch(&verified_ids, cx);
                    });
                }),
        );
        let sub = [
            ("确定 (Certain)", IssueConfidence::Certain),
            ("明确 (Firm)", IssueConfidence::Firm),
            ("推测 (Tentative)", IssueConfidence::Tentative),
        ]
        .into_iter()
        .fold(sub, |sub, (label, conf)| {
            let owner = conf_view.clone();
            let ids = conf_ids.clone();
            sub.item(
                PopupMenuItem::new(label)
                    .checked(!multi && !is_verified && current_conf == conf)
                    .disabled(!can_operate)
                    .on_click(move |_, _, cx| {
                        let _ = owner.update(cx, |v, cx| {
                            v.set_audit_findings_confidence_batch(&ids, conf, cx);
                        });
                    }),
            )
        });
        sub.separator().item(
            PopupMenuItem::new("恢复原始值 (Restore original value)")
                .disabled(!can_operate || (!multi && !can_restore_conf))
                .on_click(move |_, _, cx| {
                    let _ = restore_owner.update(cx, |v, cx| {
                        v.restore_audit_findings_confidence_batch(&restore_ids, cx);
                    });
                }),
        )
    })
    .separator()
    .item(
        PopupMenuItem::new("发送到 Repeater")
            .disabled(!can_operate || !has_flows)
            .on_click(move |_, window, cx| {
                let _ = rep_view.update(cx, |v, cx| {
                    v.start_audit_findings_batch_transfer(
                        &rep_ids,
                        DashboardBatchTransferKind::Repeater,
                        window,
                        cx,
                    );
                });
            }),
    )
    .item(
        PopupMenuItem::new("发送到 Intruder")
            .disabled(!can_operate || !has_flows)
            .on_click(move |_, window, cx| {
                let _ = intr_view.update(cx, |v, cx| {
                    v.start_audit_findings_batch_transfer(
                        &intr_ids,
                        DashboardBatchTransferKind::Intruder,
                        window,
                        cx,
                    );
                });
            }),
    )
    .item(
        PopupMenuItem::new("发送到 Organizer")
            .disabled(!can_operate || !has_flows)
            .on_click(move |_, window, cx| {
                let _ = org_view.update(cx, |v, cx| {
                    v.start_audit_findings_batch_transfer(
                        &org_ids,
                        DashboardBatchTransferKind::Organiser,
                        window,
                        cx,
                    );
                });
            }),
    )
    .item(
        PopupMenuItem::new("发送到 Comparer")
            .disabled(!can_operate)
            .on_click(move |_, window, cx| {
                let _ = comparer_view.update(cx, |v, cx| {
                    v.send_audit_findings_to_comparer_batch(&cmp_ids, window, cx);
                });
            }),
    )
    .item(
        PopupMenuItem::element(move |_, _| {
            let key = gpui_kit::Keystroke::parse(if cfg!(target_os = "macos") {
                "cmd-g"
            } else {
                "ctrl-g"
            })
            .ok()
            .map(|ks| {
                gpui_kit::component::kbd::Kbd::new(ks)
                    .p_0()
                    .flex_nowrap()
                    .border_0()
                    .bg(gpui_kit::transparent_black())
            });
            div()
                .id("dashboard-issue-to-mio-at")
                .test_support()
                .role(Role::MenuItem)
                .aria_label("发送到 MioAT")
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child("发送到 MioAT")
                .children(key)
        })
        .icon(Icon::new(IconName::Zap).size(theme::scaled_px(14.)))
        .on_click(move |_, _, cx| {
            let _ = mio_at_view.update(cx, |v, cx| {
                v.send_audit_finding_to_mio_ai(finding_id, cx);
            });
        }),
    )
    .separator()
    .item(
        PopupMenuItem::new("复制 URL")
            .icon(Icon::new(IconName::Copy).size(theme::scaled_px(14.)))
            .on_click(move |_, _, cx| {
                let _ = copy_url_view.update(cx, |v, cx| {
                    v.copy_audit_findings_urls_batch(&url_ids, cx);
                });
            }),
    )
    .item(
        PopupMenuItem::new("添加备注 (Add notes)")
            .icon(Icon::new(IconName::StickyNote).size(theme::scaled_px(14.)))
            .on_click(move |_, window, cx| {
                let _ = notes_view.update(cx, |v, cx| {
                    v.open_audit_finding_note_editor(finding_id, window, cx);
                });
            }),
    )
    .item(PopupMenuItem::submenu("高亮 (Highlight)", highlight_submenu))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_issue_header_context_menu(
    view: WeakEntity<Workbench>,
    col_ix: usize,
    sort_col: DashboardIssueSortColumn,
    finding_ids: std::rc::Rc<Vec<Uuid>>,
    col_visible: [bool; DASHBOARD_ISSUE_COL_COUNT],
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let visible_count = col_visible.iter().filter(|&&v| v).count();
    let can_hide = col_ix < DASHBOARD_ISSUE_COL_COUNT && visible_count > 1;
    let has_rows = !finding_ids.is_empty();

    let v_asc = view.clone();
    let v_desc = view.clone();
    let v_copy = view.clone();
    let v_hide = view.clone();
    let v_fit = view.clone();
    let v_cols = view.clone();
    let v_reset = view;

    let copy_ids = std::rc::Rc::clone(&finding_ids);
    let fit_ids = finding_ids;

    let cols_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        (0..DASHBOARD_ISSUE_COL_COUNT).fold(sub.compact().min_w(px(164.)), |sub, idx| {
            let owner = v_cols.clone();
            let checked = col_visible[idx];
            let disabled = checked && visible_count <= 1;
            sub.item(
                PopupMenuItem::new(DASHBOARD_ISSUE_COL_NAMES[idx])
                    .checked(checked)
                    .disabled(disabled)
                    .on_click(move |_, _, cx| {
                        let _ = owner.update(cx, |wb, cx| {
                            wb.toggle_audit_issue_column_visibility(idx, cx);
                        });
                    }),
            )
        })
    });

    menu.compact()
        .item(
            PopupMenuItem::new("按此列升序排序 (Sort ascending)").on_click(move |_, _, cx| {
                let _ = v_asc.update(cx, |wb, cx| wb.set_audit_issue_sort(sort_col, true, cx));
            }),
        )
        .item(
            PopupMenuItem::new("按此列降序排序 (Sort descending)").on_click(move |_, _, cx| {
                let _ = v_desc.update(cx, |wb, cx| wb.set_audit_issue_sort(sort_col, false, cx));
            }),
        )
        .separator()
        .item(
            PopupMenuItem::new("复制整列内容 (Copy column)")
                .disabled(!has_rows)
                .on_click(move |_, _, cx| {
                    let _ = v_copy.update(cx, |wb, cx| {
                        wb.copy_audit_issue_column(col_ix, &copy_ids, cx);
                    });
                }),
        )
        .item(
            PopupMenuItem::new("隐藏此列 (Hide column)")
                .disabled(!can_hide)
                .on_click(move |_, _, cx| {
                    let _ = v_hide.update(cx, |wb, cx| {
                        wb.toggle_audit_issue_column_visibility(col_ix, cx);
                    });
                }),
        )
        .item(
            PopupMenuItem::new("自适应列宽 (Auto-fit column width)").on_click(move |_, _, cx| {
                let _ = v_fit.update(cx, |wb, cx| {
                    wb.fit_audit_issue_column_width(col_ix, &fit_ids, cx);
                });
            }),
        )
        .separator()
        .item(PopupMenuItem::submenu("显示/隐藏列 (Columns)", cols_submenu))
        .item(
            PopupMenuItem::new("重置列布局 (Reset column layout)").on_click(move |_, _, cx| {
                let _ = v_reset.update(cx, |wb, cx| wb.reset_audit_issue_column_layout(cx));
            }),
        )
}
