use std::collections::{BTreeMap, BTreeSet};

use gpui_kit::ClipboardItem;
use mio_core::AuditTaskMode;

use super::engagement::*;
use super::*;

impl Workbench {
    pub(super) fn collect_branch_rows(&self, prefix: &str, cx: &App) -> Vec<HistoryRow> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let history_rows = self.history.read(cx).delegate().rows.clone();
        for row in self.target_pane.items.iter().chain(&history_rows) {
            if self.target_pane.hidden_item_ids.contains(&row.id()) {
                continue;
            }
            let url = row_url(row);
            if !url.is_empty()
                && !is_url_hidden(&url, &self.target_pane.tree_state.hidden_prefixes)
                && url_matches_node_prefix(&url, prefix)
                && seen.insert(row.id())
            {
                out.push(row.clone());
            }
        }
        out
    }

    pub(super) fn collect_loaded_responses_for_scope(
        &self,
        scope_key: Option<&str>,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<(String, Vec<u8>)> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let matches_scope = |url: &str| match scope_key {
            Some(prefix) => url_matches_node_prefix(url, prefix),
            None => true,
        };
        if let Some(row) = self.target_pane.selected_row.as_ref() {
            let url = row_url(row);
            if !url.is_empty() && matches_scope(&url) {
                let raw = self
                    .target_pane
                    .response
                    .bytes(window, cx)
                    .ok()
                    .filter(|b| !b.is_empty())
                    .or_else(|| self.selected_target_response_bytes(cx));
                if let Some(bytes) = raw
                    && seen.insert(url.clone())
                {
                    out.push((url, bytes));
                }
            }
        }
        if let Some(record) = self.target_pane.selected_live_record.as_deref()
            && let Some(resp) = record.response.as_ref()
            && matches_scope(&record.summary.url)
            && seen.insert(record.summary.url.clone())
        {
            out.push((record.summary.url.clone(), response_to_raw(resp)));
        }
        if let Some(detail) = self.detail.as_deref()
            && let Some(resp) = detail.response.as_ref()
            && matches_scope(&detail.summary.url)
            && seen.insert(detail.summary.url.clone())
        {
            out.push((detail.summary.url.clone(), response_to_raw(resp)));
        }
        out
    }

    fn scope_display_label(&self, scope_key: Option<&str>) -> String {
        if let Some(key) = scope_key {
            return self
                .target_pane
                .tree_state
                .nodes
                .iter()
                .find(|n| n.key == key)
                .map(|n| n.url_prefix.clone())
                .unwrap_or_else(|| key.to_owned());
        }
        self.target_pane
            .selected_row
            .as_ref()
            .map(row_url)
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| "当前站点地图".into())
    }

    pub(in crate::workbench) fn run_target_find_comments(
        &mut self,
        scope_key: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = self.scope_display_label(scope_key.as_deref());
        let responses = self.collect_loaded_responses_for_scope(scope_key.as_deref(), window, cx);
        let mut entries = Vec::new();
        for (url, raw) in &responses {
            entries.extend(extract_comments_from_response(url, raw));
        }
        let summary = if entries.is_empty() {
            format!(
                "已扫描 {} 个已加载响应报文，未发现 HTML/JS/CSS 注释；可点击「在全局搜索中查找」检索项目全量历史。",
                responses.len()
            )
        } else {
            let clipboard = entries
                .iter()
                .map(|e| format!("[{}] {} -> {}", e.kind, e.location, e.detail))
                .collect::<Vec<_>>()
                .join("\n");
            cx.write_to_clipboard(ClipboardItem::new_string(clipboard));
            format!(
                "在 {} 个响应中发现 {} 条注释（已复制到剪贴板）。",
                responses.len(),
                entries.len()
            )
        };
        self.status = format!("查找注释：{summary}");
        self.error = None;
        self.target_pane.engagement_report = Some(TargetEngagementReport {
            title: "参与工具 · 查找注释 (Find comments)".into(),
            subtitle: label,
            summary,
            search_query: Some("<!--".into()),
            entries,
        });
        cx.notify();
    }

    pub(in crate::workbench) fn run_target_find_scripts(
        &mut self,
        scope_key: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = self.scope_display_label(scope_key.as_deref());
        let responses = self.collect_loaded_responses_for_scope(scope_key.as_deref(), window, cx);
        let mut entries = Vec::new();
        let mut seen = HashSet::new();
        for (url, raw) in &responses {
            for entry in extract_scripts_from_response(url, raw) {
                if seen.insert(format!("{}:{}", entry.kind, entry.detail)) {
                    entries.push(entry);
                }
            }
        }
        let branch_key = scope_key.clone().or_else(|| {
            self.target_pane
                .selected_row
                .as_ref()
                .map(|r| target_origin_from_url(&row_url(r)))
        });
        if let Some(prefix) = branch_key.as_deref() {
            let row_metadata = self.history.read(cx).delegate().row_metadata.clone();
            for row in self.collect_branch_rows(prefix, cx) {
                let url = row_url(&row);
                let summary =
                    target_row_summary_with_metadata(&row, None, row_metadata.get(&row.id()));
                let lower = url.to_ascii_lowercase();
                if (summary.mime.eq_ignore_ascii_case("Script")
                    || lower.ends_with(".js")
                    || lower.ends_with(".mjs")
                    || lower.contains(".js?"))
                    && seen.insert(format!("ASSET:{url}")) {
                        entries.push(TargetEngagementEntry {
                            kind: "脚本资源".into(),
                            location: format!("{} {}", summary.method, summary.status),
                            detail: url,
                        });
                    }
            }
        }
        let summary = if entries.is_empty() {
            "未在已加载响应与当前主机路径中发现脚本资源；可点击「在全局搜索中查找」检索项目全量历史。".into()
        } else {
            let clipboard = entries
                .iter()
                .map(|e| format!("[{}] {}", e.kind, e.detail))
                .collect::<Vec<_>>()
                .join("\n");
            cx.write_to_clipboard(ClipboardItem::new_string(clipboard));
            format!("共发现 {} 项脚本引用 / 脚本资源（已复制到剪贴板）。", entries.len())
        };
        self.status = format!("查找脚本：{summary}");
        self.error = None;
        self.target_pane.engagement_report = Some(TargetEngagementReport {
            title: "参与工具 · 查找脚本 (Find scripts)".into(),
            subtitle: label,
            summary,
            search_query: Some("<script".into()),
            entries,
        });
        cx.notify();
    }

    pub(in crate::workbench) fn run_target_find_references(
        &mut self,
        scope_key: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = self.scope_display_label(scope_key.as_deref());
        let target_needle = if let Some(key) = scope_key.as_deref() {
            crate::workbench::wb_proxy::extract_url_host(&label)
                .unwrap_or_else(|| key.to_owned())
        } else {
            self.target_pane
                .selected_row
                .as_ref()
                .map(|r| http_history_path_and_query(&row_url(r)))
                .filter(|p| !p.is_empty() && p != "/")
                .unwrap_or_else(|| label.clone())
        };
        let mut entries = Vec::new();
        let mut seen = HashSet::new();

        // 1. Links extracted from loaded responses
        for (src_url, raw) in self.collect_loaded_responses_for_scope(None, window, cx) {
            if let Ok(links) =
                crate::workbench::wb_render_history::extract_response_links_list(&src_url, &raw)
            {
                for link in links {
                    if (link.contains(&target_needle) || src_url.contains(&target_needle))
                        && seen.insert(format!("{src_url}->{link}"))
                    {
                        entries.push(TargetEngagementEntry {
                            kind: "响应链接引用".into(),
                            location: src_url.clone(),
                            detail: link,
                        });
                    }
                }
            }
        }
        // 2. Cross-URL matches across Site Map and HTTP history
        let history_rows = self.history.read(cx).delegate().rows.clone();
        for row in self.target_pane.items.iter().chain(&history_rows) {
            if self.target_pane.hidden_item_ids.contains(&row.id()) {
                continue;
            }
            let url = row_url(row);
            if url.contains(&target_needle) && seen.insert(format!("URL:{url}")) {
                entries.push(TargetEngagementEntry {
                    kind: "站点地图 / 历史匹配".into(),
                    location: row_status_code(row)
                        .map(|s| format!("HTTP {s}"))
                        .unwrap_or_else(|| "未响应".into()),
                    detail: url,
                });
            }
        }
        let summary = if entries.is_empty() {
            format!("未在已加载响应中找到对「{target_needle}」的引用，可点击下方按钮执行全库检索。")
        } else {
            let clipboard = entries
                .iter()
                .map(|e| format!("[{}] {} -> {}", e.kind, e.location, e.detail))
                .collect::<Vec<_>>()
                .join("\n");
            cx.write_to_clipboard(ClipboardItem::new_string(clipboard));
            format!("共找到 {} 条相关引用（已复制到剪贴板）。", entries.len())
        };
        self.status = format!("查找引用：{summary}");
        self.error = None;
        self.target_pane.engagement_report = Some(TargetEngagementReport {
            title: "参与工具 · 查找引用 (Find references)".into(),
            subtitle: label,
            summary,
            search_query: Some(target_needle),
            entries,
        });
        cx.notify();
    }

    pub(in crate::workbench) fn run_target_analyze(
        &mut self,
        scope_key: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prefix = scope_key.unwrap_or_else(|| {
            self.target_pane
                .selected_row
                .as_ref()
                .map(|r| target_origin_from_url(&row_url(r)))
                .unwrap_or_default()
        });
        let label = self.scope_display_label((!prefix.is_empty()).then_some(prefix.as_str()));
        let rows = if prefix.is_empty() {
            self.target_pane
                .items
                .iter()
                .filter(|r| !self.target_pane.hidden_item_ids.contains(&r.id()))
                .cloned()
                .collect()
        } else {
            self.collect_branch_rows(&prefix, cx)
        };
        let row_metadata = self.history.read(cx).delegate().row_metadata.clone();

        let mut unique_urls = BTreeSet::new();
        let mut dynamic_urls = 0usize;
        let mut static_urls = 0usize;
        let mut methods: BTreeMap<String, usize> = BTreeMap::new();
        let mut status_buckets: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut mimes: BTreeMap<String, usize> = BTreeMap::new();
        let mut param_names: BTreeSet<String> = BTreeSet::new();

        for row in &rows {
            let url = row_url(row);
            if url.is_empty() {
                continue;
            }
            let summary =
                target_row_summary_with_metadata(row, None, row_metadata.get(&row.id()));
            unique_urls.insert(url.clone());
            let method = if summary.method.is_empty() {
                "GET".to_owned()
            } else {
                summary.method.to_ascii_uppercase()
            };
            *methods.entry(method.clone()).or_default() += 1;

            let is_dynamic = !summary.params.is_empty()
                || url.contains('?')
                || matches!(method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE");
            if is_dynamic {
                dynamic_urls += 1;
            } else {
                static_urls += 1;
            }

            if let Ok(parsed) = url::Url::parse(&url) {
                for (k, _) in parsed.query_pairs() {
                    if !k.trim().is_empty() {
                        param_names.insert(k.into_owned());
                    }
                }
            }

            let bucket = match row_status_code(row) {
                Some(200..=299) => "2xx",
                Some(300..=399) => "3xx",
                Some(400..=499) => "4xx",
                Some(500..=599) => "5xx",
                Some(_) => "Other",
                None => "Pending",
            };
            *status_buckets.entry(bucket).or_default() += 1;
            let mime = if summary.mime.is_empty() {
                "Unknown".to_owned()
            } else {
                summary.mime
            };
            *mimes.entry(mime).or_default() += 1;
        }

        // Count forms in loaded responses under this scope
        let mut forms_count = 0usize;
        for (_, raw) in self.collect_loaded_responses_for_scope(
            (!prefix.is_empty()).then_some(prefix.as_str()),
            window,
            cx,
        ) {
            let body = String::from_utf8_lossy(split_http_body(&raw));
            let stats = mio_core::html_extract::extract_html_form_stats(&body);
            forms_count += stats.form_count;
            for name in stats.param_names {
                param_names.insert(name);
            }
        }

        let findings = filter_branch_findings(
            &self.audit_dashboard.snapshot.findings,
            (!prefix.is_empty()).then_some(prefix.as_str()),
            &self.target_pane.tree_state.hidden_prefixes,
        );
        let node_count = self
            .target_pane
            .tree_state
            .nodes
            .iter()
            .filter(|n| {
                !self.target_pane.tree_state.is_hidden(n)
                    && (prefix.is_empty()
                        || self
                            .target_pane
                            .tree_state
                            .is_descendant_or_self(n, &prefix))
            })
            .count();

        let format_map = |m: &BTreeMap<String, usize>| -> String {
            if m.is_empty() {
                "无".into()
            } else {
                m.iter()
                    .map(|(k, v)| format!("{k}: {v}"))
                    .collect::<Vec<_>>()
                    .join(" · ")
            }
        };
        let format_buckets = |m: &BTreeMap<&'static str, usize>| -> String {
            if m.is_empty() {
                "无".into()
            } else {
                m.iter()
                    .map(|(k, v)| format!("{k}: {v}"))
                    .collect::<Vec<_>>()
                    .join(" · ")
            }
        };

        let mut entries = vec![
            TargetEngagementEntry {
                kind: "攻击面概览".into(),
                location: "端点与节点".into(),
                detail: format!(
                    "唯一 URL: {} · 树节点: {} · 动态端点: {} · 静态端点: {} · 表单: {}",
                    unique_urls.len(),
                    node_count,
                    dynamic_urls,
                    static_urls,
                    forms_count
                ),
            },
            TargetEngagementEntry {
                kind: "HTTP 方法分布".into(),
                location: format!("共 {} 条请求", rows.len()),
                detail: format_map(&methods),
            },
            TargetEngagementEntry {
                kind: "状态码分布".into(),
                location: "响应统计".into(),
                detail: format_buckets(&status_buckets),
            },
            TargetEngagementEntry {
                kind: "MIME 类型分布".into(),
                location: "内容类型".into(),
                detail: format_map(&mimes),
            },
            TargetEngagementEntry {
                kind: "安全告警统计".into(),
                location: "Issues".into(),
                detail: format!("关联发现 {} 条扫描告警", findings.len()),
            },
        ];
        if !param_names.is_empty() {
            let params_preview: Vec<String> = param_names.iter().take(40).cloned().collect();
            entries.push(TargetEngagementEntry {
                kind: "发现参数名".into(),
                location: format!("共 {} 个唯一参数", param_names.len()),
                detail: params_preview.join(", "),
            });
        }

        let summary = format!(
            "唯一 URL {} 个 · 动态 {} / 静态 {} · 参数 {} 个 · 告警 {} 条",
            unique_urls.len(),
            dynamic_urls,
            static_urls,
            param_names.len(),
            findings.len()
        );
        let clipboard = entries
            .iter()
            .map(|e| format!("[{}] {}: {}", e.kind, e.location, e.detail))
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(ClipboardItem::new_string(clipboard));
        self.status = format!("分析目标完成：{summary}（报告已复制到剪贴板）。");
        self.error = None;
        self.target_pane.engagement_report = Some(TargetEngagementReport {
            title: "参与工具 · 分析目标 (Analyze target)".into(),
            subtitle: label.clone(),
            summary,
            search_query: crate::workbench::wb_proxy::extract_url_host(&label),
            entries,
        });
        cx.notify();
    }

    pub(in crate::workbench) fn start_target_crawl_task(
        &mut self,
        label: &str,
        draft: Draft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() {
            return;
        }
        let target = draft.target.clone();
        let prev_crawl = self.audit_dashboard.crawl_same_origin;
        self.audit_dashboard.crawl_same_origin = true;
        let title = format!("{label} · {target}");
        let ok = self.dispatch_audit_scan(title, draft, AuditTaskMode::PassiveOnly, cx);
        self.audit_dashboard.crawl_same_origin = prev_crawl;
        if ok {
            self.cancel_plugin_interactions(cx);
            self.clear_dashboard_intruder_selection(window, cx);
            self.tool = Tool::Dashboard;
            self.activate_pro_dashboard(cx);
            self.error = None;
            self.status = format!("已启动「{label}」同源爬取与检测任务：{target}");
            cx.notify();
        }
    }

    pub(in crate::workbench) fn run_target_discover_content(
        &mut self,
        scope_key: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(key) = scope_key {
            self.run_target_branch_action(key, BranchItemAction::DiscoverContent, window, cx);
            return;
        }
        // For Contents row: if loaded response has HTML links/forms, extract to Repeater; otherwise start same-origin crawl
        if self
            .selected_target_response_bytes(cx)
            .is_some_and(|b| !b.is_empty())
        {
            self.extract_selected_target_response_links(window, cx);
        } else if let Some(row) = self.target_pane.selected_row.as_ref() {
            let url = row_url(row);
            let draft = self
                .current_transfer_draft("发现内容", window, cx)
                .unwrap_or_else(|_| super::site_map_entries::synthetic_scan_draft("GET", &url));
            self.start_target_crawl_task("发现内容", draft, window, cx);
        } else {
            self.fail("请先在站点地图中选择一条记录。", cx);
        }
    }

}
