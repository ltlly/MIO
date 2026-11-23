use super::*;

impl Workbench {
    pub(in crate::workbench) fn explore_audit_finding(
        &mut self,
        finding_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.audit_dashboard.detail_tab = IssueDetailTab::Advisory;
        self.select_audit_finding_in_place(finding_id, true, window, cx);
    }

    pub(in crate::workbench) fn go_to_audit_finding_in_target(
        &mut self,
        finding_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(finding) = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id)
            .cloned()
        else {
            return;
        };
        let origin_key = crate::workbench::target::target_origin_from_url(&finding.url);
        let matched_key = self
            .target_pane
            .tree_state
            .nodes
            .iter()
            .find(|node| {
                !node.key.is_empty()
                    && !self.target_pane.tree_state.is_hidden(node)
                    && node.kind == mio_core::SiteMapNodeKind::Origin
                    && crate::workbench::target::url_matches_node_prefix(&finding.url, &node.key)
            })
            .map(|node| node.key.clone())
            .or_else(|| (!origin_key.is_empty()).then_some(origin_key));
        if let Some(key) = &matched_key {
            self.target_pane.tree_state.expanded.insert(key.clone());
        }
        self.target_pane.page = crate::workbench::target::TargetPage::SiteMap;
        self.target_pane.site_map_subview = crate::workbench::target::SiteMapSubView::Issues;
        self.target_pane.selected_site_map_finding_id = Some(finding_id);
        self.extensions.active_top_suite_tab = None;
        self.cancel_plugin_interactions(cx);
        self.tool = Tool::Target;
        self.select_target_node(matched_key, window, cx);
        self.status = format!("已跳转到 Target Site map 问题：{}", finding.title);
        cx.notify();
    }

    pub(in crate::workbench) fn ensure_original_triage_recorded(&mut self, finding_id: Uuid) {
        if self.audit_dashboard.original_triage.contains_key(&finding_id) {
            return;
        }
        if let Some(finding) = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id)
        {
            self.audit_dashboard
                .original_triage
                .insert(finding_id, (finding.severity(), finding.confidence));
        }
    }

    pub(in crate::workbench) fn set_audit_finding_severity_override(
        &mut self,
        finding_id: Uuid,
        severity: IssueSeverity,
        cx: &mut Context<Self>,
    ) {
        let Some(current_conf) = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id)
            .map(|f| f.confidence)
        else {
            return;
        };
        self.ensure_original_triage_recorded(finding_id);
        self.audit_dashboard
            .false_positive_findings
            .remove(&finding_id);
        self.set_audit_finding_triage(finding_id, severity, current_conf, cx);
    }

    pub(in crate::workbench) fn mark_audit_finding_false_positive(
        &mut self,
        finding_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        let Some(current_conf) = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id)
            .map(|f| f.confidence)
        else {
            return;
        };
        self.ensure_original_triage_recorded(finding_id);
        self.audit_dashboard
            .false_positive_findings
            .insert(finding_id);
        self.set_audit_finding_triage(finding_id, IssueSeverity::Information, current_conf, cx);
        self.status = "已将问题标记为误报 (False positive)。".into();
        cx.notify();
    }

    pub(in crate::workbench) fn restore_audit_finding_severity(
        &mut self,
        finding_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        let Some(current_conf) = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id)
            .map(|f| f.confidence)
        else {
            return;
        };
        let Some((orig_sev, _)) = self
            .audit_dashboard
            .original_triage
            .get(&finding_id)
            .copied()
        else {
            self.audit_dashboard
                .false_positive_findings
                .remove(&finding_id);
            cx.notify();
            return;
        };
        self.audit_dashboard
            .false_positive_findings
            .remove(&finding_id);
        self.set_audit_finding_triage(finding_id, orig_sev, current_conf, cx);
        self.status = format!("已恢复问题原始严重性：{}。", orig_sev.bilingual_label());
        cx.notify();
    }

    pub(in crate::workbench) fn set_audit_finding_confidence_override(
        &mut self,
        finding_id: Uuid,
        confidence: IssueConfidence,
        cx: &mut Context<Self>,
    ) {
        let Some(current_sev) = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id)
            .map(|f| f.severity())
        else {
            return;
        };
        self.ensure_original_triage_recorded(finding_id);
        self.audit_dashboard
            .verified_confidence_findings
            .remove(&finding_id);
        self.set_audit_finding_triage(finding_id, current_sev, confidence, cx);
    }

    pub(in crate::workbench) fn mark_audit_finding_verified(
        &mut self,
        finding_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        let Some(current_sev) = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id)
            .map(|f| f.severity())
        else {
            return;
        };
        self.ensure_original_triage_recorded(finding_id);
        self.audit_dashboard
            .verified_confidence_findings
            .insert(finding_id);
        self.set_audit_finding_triage(finding_id, current_sev, IssueConfidence::Certain, cx);
        self.status = "已将问题置信度设为已验证 (Verified)。".into();
        cx.notify();
    }

    pub(in crate::workbench) fn restore_audit_finding_confidence(
        &mut self,
        finding_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        let Some(current_sev) = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id)
            .map(|f| f.severity())
        else {
            return;
        };
        let Some((_, orig_conf)) = self
            .audit_dashboard
            .original_triage
            .get(&finding_id)
            .copied()
        else {
            self.audit_dashboard
                .verified_confidence_findings
                .remove(&finding_id);
            cx.notify();
            return;
        };
        self.audit_dashboard
            .verified_confidence_findings
            .remove(&finding_id);
        self.set_audit_finding_triage(finding_id, current_sev, orig_conf, cx);
        self.status = format!("已恢复问题原始置信度：{}。", confidence_label(orig_conf));
        cx.notify();
    }

    pub(in crate::workbench) fn send_audit_finding_to_comparer(
        &mut self,
        finding_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() {
            return;
        }
        let Some(finding) = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id)
            .cloned()
        else {
            return;
        };
        const MAX_COMPARER_ITEMS: usize = 64;
        const MAX_COMPARER_ITEM_BYTES: usize = 4 * 1024 * 1024;

        let mut payloads: Vec<(String, Vec<u8>)> = Vec::new();
        let loaded_matches = self
            .audit_dashboard
            .loaded_flow
            .as_ref()
            .filter(|flow| finding.flow_id == Some(flow.summary.id));
        if let Some(flow) = loaded_matches {
            let req_raw = request_to_raw(&flow.request);
            if !req_raw.is_empty() {
                payloads.push((format!("{} (请求)", finding.title), req_raw));
            }
            if let Some(resp) = &flow.response {
                let resp_raw = response_to_raw(resp);
                if !resp_raw.is_empty() {
                    payloads.push((format!("{} (响应)", finding.title), resp_raw));
                }
            }
        }
        if payloads.is_empty() {
            let text = format_finding_context_text(
                &finding,
                self.audit_dashboard
                    .false_positive_findings
                    .contains(&finding.id),
                self.audit_dashboard
                    .verified_confidence_findings
                    .contains(&finding.id),
                self.audit_dashboard
                    .finding_notes
                    .get(&finding.id)
                    .map(String::as_str),
                None,
            );
            payloads.push((finding.title.clone(), text.into_bytes()));
        }

        let mut added = 0usize;
        for (label, bytes) in payloads {
            if bytes.len() > MAX_COMPARER_ITEM_BYTES {
                self.fail(
                    format!(
                        "Comparer 单项超过 {} KiB 上限，未添加。",
                        MAX_COMPARER_ITEM_BYTES / 1024
                    ),
                    cx,
                );
                break;
            }
            if self.extra_tools.comparer_items.len() >= MAX_COMPARER_ITEMS {
                self.fail(
                    format!("Comparer 最多保存 {MAX_COMPARER_ITEMS} 项，未添加。"),
                    cx,
                );
                break;
            }
            let id = self
                .extra_tools
                .comparer_items
                .last()
                .map_or(1, |item| item.id + 1);
            self.extra_tools
                .comparer_items
                .push(crate::workbench::tools_extra::ComparerItem {
                    id,
                    label: format!("{id}: {label} ({} bytes)", bytes.len()),
                    bytes,
                });
            added += 1;
        }
        if added > 0 {
            let count = self.extra_tools.comparer_items.len();
            self.extra_tools.comparer_selected = count - 1;
            if count >= 2 {
                self.extra_tools.left_index = count - 2;
                self.extra_tools.right_index = count - 1;
            }
            self.refresh_comparer(window, cx);
            self.cancel_plugin_interactions(cx);
            self.tool = Tool::Comparer;
            self.status = format!(
                "已将问题「{}」发送到 Comparer（新增 {} 项）。",
                finding.title, added
            );
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    pub(in crate::workbench) fn send_audit_finding_to_mio_ai(
        &mut self,
        finding_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        let Some(finding) = self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .find(|f| f.id == finding_id)
            .cloned()
        else {
            return;
        };
        let loaded_flow = self
            .audit_dashboard
            .loaded_flow
            .as_deref()
            .filter(|flow| finding.flow_id == Some(flow.summary.id));
        let context = format_finding_context_text(
            &finding,
            self.audit_dashboard
                .false_positive_findings
                .contains(&finding.id),
            self.audit_dashboard
                .verified_confidence_findings
                .contains(&finding.id),
            self.audit_dashboard
                .finding_notes
                .get(&finding.id)
                .map(String::as_str),
            loaded_flow,
        );
        self.open_mio_ai_with_context(finding.title, context, cx);
    }

    pub(in crate::workbench) fn open_audit_finding_note_editor(
        &mut self,
        finding_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .audit_dashboard
            .snapshot
            .findings
            .iter()
            .any(|f| f.id == finding_id)
        {
            return;
        }
        let existing = self
            .audit_dashboard
            .finding_notes
            .get(&finding_id)
            .cloned()
            .unwrap_or_default();
        self.audit_dashboard.editing_note_finding_id = Some(finding_id);
        self.audit_dashboard
            .finding_note_input
            .update(cx, |state, cx| {
                state.set_value(existing, window, cx);
                state.focus(window, cx);
            });
        cx.notify();
    }

    pub(in crate::workbench) fn save_audit_finding_note(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(finding_id) = self.audit_dashboard.editing_note_finding_id else {
            return;
        };
        let value = match editing::input_snapshot(&self.audit_dashboard.finding_note_input, window, cx)
        {
            Ok(val) => val.trim().to_owned(),
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        if value.is_empty() {
            self.audit_dashboard.finding_notes.remove(&finding_id);
            self.status = "已清除问题备注。".into();
        } else {
            self.audit_dashboard
                .finding_notes
                .insert(finding_id, value);
            self.status = "已保存问题备注。".into();
        }
        self.audit_dashboard.editing_note_finding_id = None;
        cx.notify();
    }

    pub(in crate::workbench) fn cancel_audit_finding_note(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.audit_dashboard.editing_note_finding_id = None;
        self.audit_dashboard
            .finding_note_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
    }

    pub(in crate::workbench) fn set_audit_finding_highlight(
        &mut self,
        finding_id: Uuid,
        color: u8,
        cx: &mut Context<Self>,
    ) {
        if color == 0 {
            self.audit_dashboard.finding_highlights.remove(&finding_id);
            self.status = "已清除问题高亮。".into();
        } else if color <= 9 {
            self.audit_dashboard
                .finding_highlights
                .insert(finding_id, color);
            let name = ISSUE_HIGHLIGHT_LABELS[usize::from(color)];
            self.status = format!("已设置问题高亮：{name}。");
        }
        cx.notify();
    }

}
