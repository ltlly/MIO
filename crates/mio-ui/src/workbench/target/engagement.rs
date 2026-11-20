use std::collections::{BTreeMap, BTreeSet};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use gpui_kit::ClipboardItem;
use mio_core::{
    AuditCommand, AuditStateSnapshot, AuditTaskMode,
    audit_engine::{
        AuditPhase, IssueConfidence, render_audit_report_html, render_audit_report_json,
    },
};

use super::*;

const MAX_COMPARER_ITEMS: usize = 32;
const MAX_COMPARER_ITEM_BYTES: usize = 256 * 1024;
const MAX_COMPARE_FILE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(in crate::workbench) struct TargetEngagementEntry {
    pub(in crate::workbench) kind: String,
    pub(in crate::workbench) location: String,
    pub(in crate::workbench) detail: String,
}

#[derive(Clone, Debug)]
pub(in crate::workbench) struct TargetEngagementReport {
    pub(in crate::workbench) title: String,
    pub(in crate::workbench) subtitle: String,
    pub(in crate::workbench) summary: String,
    pub(in crate::workbench) search_query: Option<String>,
    pub(in crate::workbench) entries: Vec<TargetEngagementEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workbench) enum TargetCompareDiffKind {
    OnlyLeft,
    OnlyRight,
    Different,
    Same,
}

#[derive(Clone, Debug)]
pub(in crate::workbench) struct TargetCompareDiffRow {
    pub(in crate::workbench) path: String,
    pub(in crate::workbench) left_summary: String,
    pub(in crate::workbench) right_summary: String,
    pub(in crate::workbench) kind: TargetCompareDiffKind,
}

#[derive(Clone, Debug)]
pub(in crate::workbench) struct TargetCompareState {
    pub(in crate::workbench) available_origins: Vec<String>,
    pub(in crate::workbench) left_origin: String,
    pub(in crate::workbench) right_origin: Option<String>,
    pub(in crate::workbench) external_label: Option<String>,
    pub(in crate::workbench) external_map: BTreeMap<String, String>,
    pub(in crate::workbench) rows: Vec<TargetCompareDiffRow>,
    pub(in crate::workbench) only_left: usize,
    pub(in crate::workbench) only_right: usize,
    pub(in crate::workbench) different: usize,
    pub(in crate::workbench) same: usize,
}

fn split_http_body(raw: &[u8]) -> &[u8] {
    if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
        &raw[pos + 4..]
    } else if raw.starts_with(b"HTTP/")
        && let Some(pos) = raw.windows(2).position(|w| w == b"\n\n")
    {
        &raw[pos + 2..]
    } else {
        raw
    }
}

fn extract_comments_from_response(url: &str, raw: &[u8]) -> Vec<TargetEngagementEntry> {
    let body = String::from_utf8_lossy(split_http_body(raw));
    let mut entries = Vec::new();
    let mut seen = HashSet::new();

    // 1. HTML comments <!-- ... -->
    let mut rest = body.as_ref();
    let mut offset = 0usize;
    while let Some(start) = rest.find("<!--") {
        let abs_start = offset + start;
        let after = &rest[start + 4..];
        let (comment_body, advance) = if let Some(end) = after.find("-->") {
            (&after[..end], start + 4 + end + 3)
        } else {
            (after, rest.len())
        };
        let cleaned = comment_body
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if !cleaned.is_empty() {
            let line = body[..abs_start].bytes().filter(|&b| b == b'\n').count() + 1;
            let snippet: String = cleaned.chars().take(180).collect();
            let dedup = format!("HTML:{snippet}");
            if seen.insert(dedup) {
                entries.push(TargetEngagementEntry {
                    kind: "HTML 注释".into(),
                    location: format!("{url} (第 {line} 行)"),
                    detail: format!("<!-- {snippet} -->"),
                });
            }
        }
        offset += advance;
        rest = &rest[advance..];
    }

    // 2. Block comments /* ... */
    let mut rest = body.as_ref();
    let mut offset = 0usize;
    while let Some(start) = rest.find("/*") {
        let abs_start = offset + start;
        let after = &rest[start + 2..];
        let Some(end) = after.find("*/") else {
            break;
        };
        let cleaned = after[..end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let advance = start + 2 + end + 2;
        if !cleaned.is_empty() {
            let line = body[..abs_start].bytes().filter(|&b| b == b'\n').count() + 1;
            let snippet: String = cleaned.chars().take(180).collect();
            let dedup = format!("BLOCK:{snippet}");
            if seen.insert(dedup) {
                entries.push(TargetEngagementEntry {
                    kind: "块注释".into(),
                    location: format!("{url} (第 {line} 行)"),
                    detail: format!("/* {snippet} */"),
                });
            }
        }
        offset += advance;
        rest = &rest[advance..];
    }

    // 3. Single-line // comments in JS/CSS responses
    let mime = super::mime::mime_from_raw_response(raw).unwrap_or("");
    if mime == "Script" || mime == "CSS" || url.to_ascii_lowercase().ends_with(".js") {
        for (line_idx, line_text) in body.lines().enumerate() {
            let trimmed = line_text.trim();
            if trimmed.starts_with("//") {
                let text = trimmed.trim_start_matches('/').trim();
                if !text.is_empty() {
                    let snippet: String = text.chars().take(180).collect();
                    let dedup = format!("LINE:{snippet}");
                    if seen.insert(dedup) {
                        entries.push(TargetEngagementEntry {
                            kind: "单行注释".into(),
                            location: format!("{url} (第 {} 行)", line_idx + 1),
                            detail: format!("// {snippet}"),
                        });
                    }
                }
            }
        }
    }

    entries
}

fn extract_scripts_from_response(url: &str, raw: &[u8]) -> Vec<TargetEngagementEntry> {
    let body = String::from_utf8_lossy(split_http_body(raw));
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let base = url::Url::parse(url).ok();
    let document = scraper::Html::parse_document(&body);
    if let Ok(selector) = scraper::Selector::parse("script") {
        for (idx, el) in document.select(&selector).enumerate() {
            if let Some(src) = el.value().attr("src") {
                let resolved = base
                    .as_ref()
                    .and_then(|b| b.join(src).ok())
                    .map(|u| u.to_string())
                    .unwrap_or_else(|| src.to_owned());
                if seen.insert(format!("SRC:{resolved}")) {
                    entries.push(TargetEngagementEntry {
                        kind: "外部脚本".into(),
                        location: url.to_owned(),
                        detail: resolved,
                    });
                }
            } else {
                let inline = el
                    .text()
                    .collect::<String>()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                if !inline.is_empty() {
                    let snippet: String = inline.chars().take(160).collect();
                    if seen.insert(format!("INLINE:{snippet}")) {
                        entries.push(TargetEngagementEntry {
                            kind: format!("内联脚本 #{}", idx + 1),
                            location: url.to_owned(),
                            detail: snippet,
                        });
                    }
                }
            }
        }
    }
    entries
}

fn build_multi_item_ext_xml(
    items: &[(String, String, Option<u16>, String, Vec<u8>, Vec<u8>, String)],
) -> Result<String, String> {
    use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event as XmlEvent};

    let now = crate::history::local_time(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
    );
    let mut writer = quick_xml::Writer::new(Vec::new());
    writer
        .write_event(XmlEvent::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
        .map_err(|e| e.to_string())?;
    let mut root = BytesStart::new("items");
    root.push_attribute(("mioVersion", "2026.10.1"));
    root.push_attribute(("exportTime", now.as_str()));
    writer
        .write_event(XmlEvent::Start(root))
        .map_err(|e| e.to_string())?;

    for (url, method, status, time_str, req_bytes, resp_bytes, comment) in items {
        let Ok(parsed) = url::Url::parse(url) else {
            continue;
        };
        if !matches!(parsed.scheme(), "http" | "https") {
            continue;
        }
        let Some(host) = parsed.host_str() else {
            continue;
        };
        let port = parsed.port_or_known_default().unwrap_or(80);
        let mut path = parsed.path().to_owned();
        if let Some(q) = parsed.query() {
            path.push('?');
            path.push_str(q);
        }
        let extension = parsed
            .path_segments()
            .and_then(Iterator::last)
            .and_then(|seg| seg.rsplit_once('.').map(|(_, ext)| ext))
            .unwrap_or("");
        writer
            .write_event(XmlEvent::Start(BytesStart::new("item")))
            .map_err(|e| e.to_string())?;
        for (tag, val) in [
            ("time", time_str.clone()),
            ("url", url.clone()),
            ("host", host.to_owned()),
            ("port", port.to_string()),
            ("protocol", parsed.scheme().to_owned()),
            ("method", method.clone()),
            ("path", path),
            ("extension", extension.to_owned()),
        ] {
            writer
                .create_element(tag)
                .write_text_content(BytesText::new(&val))
                .map_err(|e| e.to_string())?;
        }
        writer
            .create_element("request")
            .with_attribute(("base64", "true"))
            .write_text_content(BytesText::new(&BASE64_STANDARD.encode(req_bytes)))
            .map_err(|e| e.to_string())?;
        writer
            .create_element("status")
            .write_text_content(BytesText::new(
                &status.map(|s| s.to_string()).unwrap_or_default(),
            ))
            .map_err(|e| e.to_string())?;
        writer
            .create_element("responselength")
            .write_text_content(BytesText::new(&resp_bytes.len().to_string()))
            .map_err(|e| e.to_string())?;
        writer
            .create_element("response")
            .with_attribute(("base64", "true"))
            .write_text_content(BytesText::new(&BASE64_STANDARD.encode(resp_bytes)))
            .map_err(|e| e.to_string())?;
        writer
            .create_element("comment")
            .write_text_content(BytesText::new(comment))
            .map_err(|e| e.to_string())?;
        writer
            .write_event(XmlEvent::End(BytesEnd::new("item")))
            .map_err(|e| e.to_string())?;
    }

    writer
        .write_event(XmlEvent::End(BytesEnd::new("items")))
        .map_err(|e| e.to_string())?;
    let xml = String::from_utf8(writer.into_inner()).map_err(|e| e.to_string())?;
    roxmltree::Document::parse(&xml).map_err(|e| format!("XML 导出内容无效：{e}"))?;
    Ok(xml)
}

fn parse_external_site_map_file(text: &str) -> Result<BTreeMap<String, String>, String> {
    let trimmed = text.trim();
    let mut map = BTreeMap::new();
    if trimmed.starts_with('<') {
        let doc = roxmltree::Document::parse(trimmed)
            .map_err(|e| format!("无法解析 MIO XML 站点地图文件：{e}"))?;
        for item in doc.descendants().filter(|n| n.has_tag_name("item")) {
            let path = item
                .children()
                .find(|c| c.has_tag_name("path"))
                .and_then(|c| c.text())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    item.children()
                        .find(|c| c.has_tag_name("url"))
                        .and_then(|c| c.text())
                        .map(http_history_path_and_query)
                });
            let Some(path) = path else {
                continue;
            };
            let method = item
                .children()
                .find(|c| c.has_tag_name("method"))
                .and_then(|c| c.text())
                .unwrap_or("GET")
                .trim();
            let status = item
                .children()
                .find(|c| c.has_tag_name("status"))
                .and_then(|c| c.text())
                .unwrap_or("-")
                .trim();
            let len = item
                .children()
                .find(|c| c.has_tag_name("responselength"))
                .and_then(|c| c.text())
                .unwrap_or("0")
                .trim();
            map.insert(path, format!("{method} · status={status} · len={len}"));
        }
        return Ok(map);
    }
    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        let val: serde_json::Value = serde_json::from_str(trimmed)
            .map_err(|e| format!("无法解析 JSON 站点地图文件：{e}"))?;
        let arr = val
            .as_array()
            .or_else(|| val.get("items").and_then(serde_json::Value::as_array))
            .or_else(|| {
                val.get("log")
                    .and_then(|l| l.get("entries"))
                    .and_then(serde_json::Value::as_array)
            });
        if let Some(entries) = arr {
            for entry in entries {
                let url = entry
                    .get("url")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| {
                        entry
                            .get("request")
                            .and_then(|r| r.get("url"))
                            .and_then(serde_json::Value::as_str)
                    })
                    .unwrap_or("");
                if url.is_empty() {
                    continue;
                }
                let path = http_history_path_and_query(url);
                let method = entry
                    .get("method")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| {
                        entry
                            .get("request")
                            .and_then(|r| r.get("method"))
                            .and_then(serde_json::Value::as_str)
                    })
                    .unwrap_or("GET");
                let status = entry
                    .get("status")
                    .and_then(serde_json::Value::as_u64)
                    .or_else(|| {
                        entry
                            .get("response")
                            .and_then(|r| r.get("status"))
                            .and_then(serde_json::Value::as_u64)
                    })
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "-".into());
                let len = entry
                    .get("length")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                map.insert(path, format!("{method} · status={status} · len={len}"));
            }
        }
        return Ok(map);
    }
    // Plain line-delimited URL list fallback
    for line in trimmed
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let path = if line.contains("://") {
            http_history_path_and_query(line)
        } else {
            line.to_owned()
        };
        map.insert(path, "URL".into());
    }
    Ok(map)
}

fn compute_site_map_diff(
    left: &BTreeMap<String, String>,
    right: &BTreeMap<String, String>,
) -> (Vec<TargetCompareDiffRow>, usize, usize, usize, usize) {
    let mut keys = BTreeSet::new();
    keys.extend(left.keys().cloned());
    keys.extend(right.keys().cloned());
    let mut rows = Vec::with_capacity(keys.len());
    let (mut only_left, mut only_right, mut different, mut same) = (0, 0, 0, 0);
    for path in keys {
        match (left.get(&path), right.get(&path)) {
            (Some(l), None) => {
                only_left += 1;
                rows.push(TargetCompareDiffRow {
                    path,
                    left_summary: l.clone(),
                    right_summary: "—".into(),
                    kind: TargetCompareDiffKind::OnlyLeft,
                });
            }
            (None, Some(r)) => {
                only_right += 1;
                rows.push(TargetCompareDiffRow {
                    path,
                    left_summary: "—".into(),
                    right_summary: r.clone(),
                    kind: TargetCompareDiffKind::OnlyRight,
                });
            }
            (Some(l), Some(r)) if l != r => {
                different += 1;
                rows.push(TargetCompareDiffRow {
                    path,
                    left_summary: l.clone(),
                    right_summary: r.clone(),
                    kind: TargetCompareDiffKind::Different,
                });
            }
            (Some(l), Some(r)) => {
                same += 1;
                rows.push(TargetCompareDiffRow {
                    path,
                    left_summary: l.clone(),
                    right_summary: r.clone(),
                    kind: TargetCompareDiffKind::Same,
                });
            }
            (None, None) => {}
        }
    }
    (rows, only_left, only_right, different, same)
}

impl Workbench {
    // --- Tree branch expansion & collapse ---

    pub(in crate::workbench) fn expand_target_branch(
        &mut self,
        key: String,
        cx: &mut Context<Self>,
    ) {
        let expanded = self.target_pane.tree_state.expand_branch(&key);
        self.error = None;
        self.status = if expanded > 0 {
            format!("已展开分支及 {expanded} 个子节点。")
        } else {
            "该分支已全部展开。".into()
        };
        cx.notify();
    }

    pub(in crate::workbench) fn expand_target_requested_items(
        &mut self,
        key: String,
        cx: &mut Context<Self>,
    ) {
        let mut requested_keys = HashSet::new();
        let history_rows = self.history.read(cx).delegate().rows.clone();
        for row in self.target_pane.items.iter().chain(&history_rows) {
            if self.target_pane.hidden_item_ids.contains(&row.id()) {
                continue;
            }
            let url = row_url(row);
            if row_status_code(row).is_some()
                && !is_url_hidden(&url, &self.target_pane.tree_state.hidden_prefixes)
                && url_matches_node_prefix(&url, &key)
                && let Some(canon) = canonical_site_map_url_key(&url)
            {
                requested_keys.insert(canon);
            }
        }
        let candidate_node_keys: Vec<String> = self
            .target_pane
            .tree_state
            .nodes
            .iter()
            .filter(|node| {
                !self.target_pane.tree_state.is_hidden(node)
                    && self.target_pane.tree_state.is_descendant_or_self(node, &key)
            })
            .filter(|node| {
                if !requested_keys.is_empty() {
                    let node_canon =
                        canonical_site_map_url_key(&node.key).unwrap_or_else(|| node.key.clone());
                    requested_keys.contains(&node_canon)
                } else {
                    node.direct_count > 0
                }
            })
            .map(|node| node.key.clone())
            .collect();

        let mut expanded = 0usize;
        for target_node in &candidate_node_keys {
            expanded += self
                .target_pane
                .tree_state
                .expand_ancestors_within(target_node, &key);
        }
        self.error = None;
        self.status = if candidate_node_keys.is_empty() {
            "该分支下暂无已请求项可展开。".into()
        } else if expanded > 0 {
            format!("已展开 {} 个包含已请求项的节点。", expanded)
        } else {
            "该分支下的已请求项均已处于展开状态。".into()
        };
        cx.notify();
    }

    pub(in crate::workbench) fn collapse_target_branch(
        &mut self,
        key: String,
        cx: &mut Context<Self>,
    ) {
        let collapsed = self.target_pane.tree_state.collapse_branch(&key);
        self.error = None;
        self.status = if collapsed > 0 {
            format!("已折叠分支及 {collapsed} 个子节点。")
        } else {
            "该分支已处于折叠状态。".into()
        };
        cx.notify();
    }

    // --- Branch issues report & delete ---

    pub(in crate::workbench) fn save_branch_audit_report_to_file(
        &mut self,
        key: String,
        as_html: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() || self.file_picker_pending() {
            return;
        }
        let findings: Vec<AuditFinding> = filter_branch_findings(
            &self.audit_dashboard.snapshot.findings,
            Some(&key),
            &self.target_pane.tree_state.hidden_prefixes,
        )
        .into_iter()
        .cloned()
        .collect();
        if findings.is_empty() {
            self.fail("当前选中分支下暂无可导出的漏洞告警。", cx);
            return;
        }
        let snapshot = AuditStateSnapshot {
            revision: self.audit_dashboard.snapshot.revision,
            live_passive_enabled: self.audit_dashboard.snapshot.live_passive_enabled,
            live_passive_crawl_enabled: self.audit_dashboard.snapshot.live_passive_crawl_enabled,
            tasks: Vec::new(),
            findings,
        };
        let payload = if as_html {
            render_audit_report_html(&snapshot).into_bytes()
        } else {
            match render_audit_report_json(&snapshot) {
                Ok(bytes) => bytes,
                Err(err) => {
                    self.fail(format!("生成分支 JSON 报告失败：{err}"), cx);
                    return;
                }
            }
        };
        let finding_count = snapshot.findings.len();
        let suggested = if as_html {
            "site-map-branch-issues.html"
        } else {
            "site-map-branch-issues.json"
        };
        let project = self.project;
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &self.default_directory(),
            Some(suggested),
        );
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = owner.update(cx, |_, _, cx| {
                this.update(cx, |view, cx| {
                    if !view.finish_file_picker(picker, cx) || !view.backend_available() {
                        return;
                    }
                    if view.project != project {
                        view.fail("项目已切换，分支报告未写入磁盘。", cx);
                        return;
                    }
                    match result {
                        Ok(Ok(Some(mut path))) => {
                            if path.extension().is_none() {
                                path.set_extension(if as_html { "html" } else { "json" });
                            }
                            match std::fs::write(&path, &payload) {
                                Ok(()) => {
                                    view.error = None;
                                    view.status = format!(
                                        "已保存分支漏洞报告到文件：{}（共 {} 条告警）",
                                        path.display(),
                                        finding_count
                                    );
                                    cx.notify();
                                }
                                Err(err) => {
                                    view.fail(format!("写入分支漏洞报告失败：{err}"), cx);
                                }
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(err)) => view.fail(format!("无法选择报告保存路径：{err}"), cx),
                        Err(_) => view.fail("文件选择器已意外关闭。", cx),
                    }
                })
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn delete_branch_audit_findings(
        &mut self,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.audit_can_operate() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let ids: Vec<Uuid> = filter_branch_findings(
            &self.audit_dashboard.snapshot.findings,
            Some(&key),
            &self.target_pane.tree_state.hidden_prefixes,
        )
        .into_iter()
        .map(|f| f.id)
        .collect();
        if ids.is_empty() {
            self.fail("当前选中分支下暂无可删除的漏洞告警。", cx);
            return;
        }
        let mut deleted = 0usize;
        for finding_id in &ids {
            if self.enqueue(
                Command::Audit(AuditCommand::DeleteFinding {
                    project,
                    finding_id: *finding_id,
                }),
                cx,
            ) {
                deleted += 1;
            }
        }
        let id_set: HashSet<Uuid> = ids.into_iter().collect();
        self.audit_dashboard
            .snapshot
            .findings
            .retain(|f| !id_set.contains(&f.id));
        if self
            .target_pane
            .selected_site_map_finding_id
            .is_some_and(|sel| id_set.contains(&sel))
        {
            self.target_pane.selected_site_map_finding_id = None;
        }
        self.reconcile_audit_selection(window, cx);
        self.error = None;
        self.status = format!("已删除该分支下的 {deleted} 条漏洞告警。");
        cx.notify();
    }

    // --- Single Site Map Issue row actions ---

    fn remember_finding_original(&mut self, finding: &AuditFinding) {
        self.target_pane
            .finding_originals
            .entry(finding.id)
            .or_insert((finding.severity(), finding.confidence));
    }

    pub(in crate::workbench) fn set_target_finding_severity(
        &mut self,
        finding_id: Uuid,
        severity: MioIssueSeverity,
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
        self.remember_finding_original(&finding);
        self.set_audit_finding_triage(finding_id, severity, finding.confidence, cx);
        self.status = format!("已将问题严重性设置为 {}。", severity.label());
        cx.notify();
    }

    pub(in crate::workbench) fn restore_target_finding_severity(
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
        let orig_sev = self
            .target_pane
            .finding_originals
            .get(&finding_id)
            .map(|(sev, _)| *sev)
            .or_else(|| {
                issue_definitions()
                    .iter()
                    .find(|def| def.id == finding.issue_id)
                    .map(|def| def.typical_severity)
            })
            .unwrap_or_else(|| finding.severity());
        self.set_audit_finding_triage(finding_id, orig_sev, finding.confidence, cx);
        self.status = format!("已恢复问题原始严重性（{}）。", orig_sev.label());
        cx.notify();
    }

    pub(in crate::workbench) fn set_target_finding_confidence(
        &mut self,
        finding_id: Uuid,
        confidence: IssueConfidence,
        verified: bool,
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
        self.remember_finding_original(&finding);
        if verified {
            self.target_pane.verified_findings.insert(finding_id);
        } else {
            self.target_pane.verified_findings.remove(&finding_id);
        }
        self.set_audit_finding_triage(finding_id, finding.severity(), confidence, cx);
        self.status = if verified {
            "已将问题置信度设置为已验证 (Certain)。".into()
        } else {
            format!(
                "已将问题置信度设置为 {}。",
                crate::workbench::audit_dashboard::confidence_label(confidence)
            )
        };
        cx.notify();
    }

    pub(in crate::workbench) fn restore_target_finding_confidence(
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
        let orig_conf = self
            .target_pane
            .finding_originals
            .get(&finding_id)
            .map(|(_, conf)| *conf)
            .unwrap_or(finding.confidence);
        self.target_pane.verified_findings.remove(&finding_id);
        self.set_audit_finding_triage(finding_id, finding.severity(), orig_conf, cx);
        self.status = format!(
            "已恢复问题原始置信度（{}）。",
            crate::workbench::audit_dashboard::confidence_label(orig_conf)
        );
        cx.notify();
    }

    pub(in crate::workbench) fn delete_target_site_map_finding(
        &mut self,
        finding_id: Uuid,
        false_positive: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.target_pane.verified_findings.remove(&finding_id);
        self.target_pane.finding_originals.remove(&finding_id);
        if self.target_pane.selected_site_map_finding_id == Some(finding_id) {
            self.target_pane.selected_site_map_finding_id = None;
        }
        self.delete_audit_finding(finding_id, window, cx);
        self.status = if false_positive {
            "已将该问题标记为误报并移除。".into()
        } else {
            "已删除所选问题。".into()
        };
        cx.notify();
    }

    fn push_text_to_comparer(
        &mut self,
        label: &str,
        bytes: Vec<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if bytes.len() > MAX_COMPARER_ITEM_BYTES {
            self.fail(
                format!(
                    "Comparer 单项超过 {} KiB 上限，未添加。",
                    MAX_COMPARER_ITEM_BYTES / 1024
                ),
                cx,
            );
            return false;
        }
        if self.extra_tools.comparer_items.len() >= MAX_COMPARER_ITEMS {
            self.fail(
                format!("Comparer 最多保存 {MAX_COMPARER_ITEMS} 项，未添加。"),
                cx,
            );
            return false;
        }
        let id = self
            .extra_tools
            .comparer_items
            .last()
            .map_or(1, |item| item.id + 1);
        self.extra_tools
            .comparer_items
            .push(tools_extra::ComparerItem {
                id,
                label: format!("{id}: {label} ({} bytes)", bytes.len()),
                bytes,
            });
        let count = self.extra_tools.comparer_items.len();
        self.extra_tools.comparer_selected = count - 1;
        if count >= 2 {
            self.extra_tools.left_index = count - 2;
            self.extra_tools.right_index = count - 1;
        }
        self.refresh_comparer(window, cx);
        true
    }

    pub(in crate::workbench) fn send_target_issue_to_comparer(
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
            self.fail("所选漏洞告警已不存在。", cx);
            return;
        };
        let mut text = format!(
            "Issue: {}\nIssue ID: {}\nSeverity: {}\nConfidence: {}\nHost: {}\nURL: {}\n",
            finding.title,
            finding.issue_id,
            finding.severity_label,
            crate::workbench::audit_dashboard::confidence_label(finding.confidence),
            finding.host,
            finding.url
        );
        if let Some(param) = &finding.parameter {
            text.push_str(&format!("Parameter: {param}\n"));
        }
        text.push_str(&format!("\nSummary:\n{}\n", finding.summary));
        if !finding.evidence.is_empty() {
            text.push_str(&format!("\nEvidence:\n{}\n", finding.evidence));
        }
        if !finding.remediation.is_empty() {
            text.push_str(&format!("\nRemediation:\n{}\n", finding.remediation));
        }
        if self.push_text_to_comparer(&finding.title, text.into_bytes(), window, cx) {
            self.cancel_plugin_interactions(cx);
            self.tool = Tool::Comparer;
            self.focus.focus(window, cx);
            self.error = None;
            self.status = format!("已将问题「{}」发送到 Comparer。", finding.title);
            cx.notify();
        }
    }

    pub(in crate::workbench) fn send_target_issue_to_mio_at(
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
            self.fail("所选漏洞告警已不存在。", cx);
            return;
        };
        let mut context = format!(
            "Target Issue Analysis\nTitle: {}\nIssue ID: {}\nSeverity: {}\nConfidence: {}\nURL: {}\n",
            finding.title,
            finding.issue_id,
            finding.severity_label,
            crate::workbench::audit_dashboard::confidence_label(finding.confidence),
            finding.url,
        );
        if let Some(param) = &finding.parameter {
            context.push_str(&format!("Parameter: {param}\n"));
        }
        context.push_str(&format!("Summary: {}\n", finding.summary));
        if !finding.evidence.is_empty() {
            context.push_str(&format!("Evidence: {}\n", finding.evidence));
        }
        if !finding.remediation.is_empty() {
            context.push_str(&format!("Remediation: {}\n", finding.remediation));
        }
        self.open_mio_ai_with_context(finding.title, context, cx);
    }

    // --- Contents row MioAT / CSRF PoC / Record Issue / Delete / Save ---

    pub(in crate::workbench) fn send_selected_target_to_mio_at(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.target_pane.selected_row.as_ref() else {
            self.fail("请先在站点地图中选择一条记录。", cx);
            return;
        };
        let url = row_url(row);
        let req_bytes = self
            .target_pane
            .request
            .bytes(window, cx)
            .unwrap_or_default();
        let resp_bytes = self
            .selected_target_response_bytes(cx)
            .unwrap_or_default();
        let mut ctx = format!("Target Site map URL: {url}\n");
        if !req_bytes.is_empty() {
            ctx.push_str("\n--- Request ---\n");
            let req_text = String::from_utf8_lossy(&req_bytes);
            ctx.push_str(&req_text.chars().take(8000).collect::<String>());
        }
        if !resp_bytes.is_empty() {
            ctx.push_str("\n\n--- Response ---\n");
            let resp_text = String::from_utf8_lossy(&resp_bytes);
            ctx.push_str(&resp_text.chars().take(8000).collect::<String>());
        }
        let title = if url.is_empty() {
            "Site map 条目".to_owned()
        } else {
            url
        };
        self.open_mio_ai_with_context(title, ctx, cx);
    }

    pub(in crate::workbench) fn open_target_csrf_poc_modal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.target_pane.selected_row.as_ref() else {
            self.fail("请先在站点地图中选择一条记录。", cx);
            return;
        };
        let url = row_url(row);
        let mut raw_request = self
            .target_pane
            .request
            .bytes(window, cx)
            .unwrap_or_default();
        if raw_request.is_empty() {
            let method = match row {
                HistoryRow::Live(s) => s.method.as_str(),
                HistoryRow::ExtProxy(a) => a
                    .edited_request
                    .as_ref()
                    .or(a.request.as_ref())
                    .and_then(|r| r.method.as_deref())
                    .unwrap_or("POST"),
            };
            raw_request = super::site_map_ext::synthetic_scan_draft(method, &url).raw;
        }
        self.open_csrf_poc_modal(&url, &raw_request, cx);
    }

    pub(in crate::workbench) fn record_manual_issue_from_target(&mut self, cx: &mut Context<Self>) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let Some(row) = self.target_pane.selected_row.as_ref() else {
            self.fail("请先在站点地图中选择一条记录。", cx);
            return;
        };
        let url = row_url(row);
        if url.trim().is_empty() {
            self.fail("所选站点地图记录缺少有效的请求 URL。", cx);
            return;
        }
        let host = url::Url::parse(&url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_owned))
            .unwrap_or_default();
        let row_id = row.id();
        let flow_id = match row_id {
            HistoryEntryId::Live(fid) if !self.target_is_draft_entry(row_id) => Some(fid),
            _ => None,
        };
        let now_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let finding = AuditFinding {
            id: Uuid::new_v4(),
            issue_id: "0x00800100".into(),
            title: format!("自定义问题 · {url}"),
            severity_label: IssueSeverity::Information.label().into(),
            severity_rank: IssueSeverity::Information.rank(),
            confidence: IssueConfidence::Certain,
            phase: AuditPhase::Passive,
            host,
            url: url.clone(),
            parameter: None,
            flow_id,
            summary: format!("从 Site map 创建的自定义问题（{url}）。"),
            evidence: url.clone(),
            request_highlights: Vec::new(),
            response_highlights: Vec::new(),
            remediation: "核查关联的 HTTP 请求与响应报文并补充修复建议。".into(),
            detected_unix_ms: now_unix_ms,
        };
        if self.enqueue(
            Command::Audit(AuditCommand::RecordManualFinding {
                project,
                finding: Box::new(finding),
            }),
            cx,
        ) {
            self.error = None;
            self.status = format!("已记录新问题：{url}");
            cx.notify();
        }
    }

    pub(in crate::workbench) fn attach_target_row_to_issue(
        &mut self,
        finding_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let Some(row) = self.target_pane.selected_row.as_ref() else {
            self.fail("请先在站点地图中选择一条记录。", cx);
            return;
        };
        let url = row_url(row);
        if url.trim().is_empty() {
            self.fail("所选站点地图记录缺少有效的请求 URL。", cx);
            return;
        }
        let row_id = row.id();
        let flow_id = match row_id {
            HistoryEntryId::Live(fid) if !self.target_is_draft_entry(row_id) => Some(fid),
            _ => None,
        };
        if self.enqueue(
            Command::Audit(AuditCommand::AttachFlowToFinding {
                project,
                finding_id,
                flow_id,
                url: url.clone(),
                evidence: format!("Site map 关联流量：{url}"),
            }),
            cx,
        ) {
            self.error = None;
            self.status = format!("已将所选站点地图记录附加到已有问题：{url}");
            cx.notify();
        }
    }

    pub(in crate::workbench) fn delete_selected_target_item(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut target_ids: HashSet<HistoryEntryId> = self.target_pane.selected_ids.clone();
        if let Some(sel) = self.target_pane.selected_id {
            target_ids.insert(sel);
        }
        if target_ids.is_empty() {
            self.fail("请先在站点地图中选择一条记录。", cx);
            return;
        }
        let count = target_ids.len();
        for id in &target_ids {
            self.target_pane.hidden_item_ids.insert(*id);
        }
        let before = self.target_pane.items.len();
        self.target_pane
            .items
            .retain(|row| !target_ids.contains(&row.id()));
        let removed = before.saturating_sub(self.target_pane.items.len());
        if removed > 0 {
            self.target_pane.total_items = self.target_pane.total_items.saturating_sub(removed);
        }
        self.target_pane.selected_id = None;
        self.target_pane.selected_ids.clear();
        self.target_pane.selection_anchor = None;
        self.target_pane.selected_row = None;
        self.target_pane.selected_response_bytes.clear();
        self.target_pane.selected_live_record = None;
        self.target_pane.selected_explicit_detail = None;
        self.target_pane.request.load(&[], window, cx);
        self.target_pane.response.load(&[], window, cx);
        self.error = None;
        self.status = if count > 1 {
            format!("已从站点地图删除 {count} 个所选项。")
        } else {
            "已从站点地图删除所选项。".into()
        };
        cx.notify();
    }

    pub(in crate::workbench) fn save_selected_target_item_to_file(
        &mut self,
        as_xml: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || self.project.is_none()
            || !self.backend_available()
            || self.file_picker_pending()
        {
            return;
        }
        let Some(row) = self.target_pane.selected_row.clone() else {
            self.fail("请先在站点地图中选择一条记录。", cx);
            return;
        };
        let url = row_url(&row);
        if url.trim().is_empty() {
            self.fail("所选站点地图记录缺少有效 URL。", cx);
            return;
        }
        let req_bytes = self
            .target_pane
            .request
            .bytes(window, cx)
            .unwrap_or_default();
        let resp_bytes = self
            .selected_target_response_bytes(cx)
            .unwrap_or_default();
        let summary = target_row_summary_with_metadata(&row, Some(&resp_bytes), None);
        let status_code = row_status_code(&row);
        let payload = if as_xml {
            match crate::workbench::wb_render_history::build_ext_items_xml(
                &url,
                if summary.method.is_empty() {
                    "GET"
                } else {
                    &summary.method
                },
                status_code,
                "",
                &req_bytes,
                &resp_bytes,
                "",
            ) {
                Ok(xml) => xml.into_bytes(),
                Err(err) => {
                    self.fail(err, cx);
                    return;
                }
            }
        } else if let Some(record) = self.target_pane.selected_live_record.as_deref() {
            match mio_core::export_flows_to_har(std::slice::from_ref(record)) {
                Ok(bytes) => bytes,
                Err(err) => {
                    self.fail(err.to_string(), cx);
                    return;
                }
            }
        } else {
            let item = serde_json::json!({
                "url": url,
                "host": summary.host,
                "method": summary.method,
                "status": status_code,
                "length": resp_bytes.len(),
                "mime": summary.mime,
                "title": summary.title,
                "request_base64": BASE64_STANDARD.encode(&req_bytes),
                "response_base64": BASE64_STANDARD.encode(&resp_bytes),
            });
            match serde_json::to_vec_pretty(&item) {
                Ok(bytes) => bytes,
                Err(err) => {
                    self.fail(format!("序列化 JSON 失败：{err}"), cx);
                    return;
                }
            }
        };
        let suggested = if as_xml {
            "site-map-item.xml"
        } else if self.target_pane.selected_live_record.is_some() {
            "site-map-item.har"
        } else {
            "site-map-item.json"
        };
        self.prompt_and_write_target_file(
            suggested,
            if as_xml { "xml" } else { "json" },
            payload,
            "已导出站点地图条目到文件",
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn save_target_branch_items_to_file(
        &mut self,
        key: String,
        as_xml: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || self.project.is_none()
            || !self.backend_available()
            || self.file_picker_pending()
        {
            return;
        }
        let rows = self.collect_branch_rows(&key, cx);
        let selected_id = self.target_pane.selected_id;
        let selected_req = self
            .target_pane
            .request
            .bytes(window, cx)
            .unwrap_or_default();
        let selected_resp = self
            .selected_target_response_bytes(cx)
            .unwrap_or_default();

        let mut export_items = Vec::new();
        for row in &rows {
            let url = row_url(row);
            if url.is_empty() {
                continue;
            }
            let summary = target_row_summary_with_metadata(row, None, None);
            let method = if summary.method.is_empty() {
                "GET".to_owned()
            } else {
                summary.method
            };
            let status = row_status_code(row);
            let (req, resp) = if selected_id == Some(row.id()) {
                (selected_req.clone(), selected_resp.clone())
            } else {
                (
                    super::site_map_ext::synthetic_scan_draft(&method, &url).raw,
                    Vec::new(),
                )
            };
            export_items.push((url, method, status, String::new(), req, resp, String::new()));
        }
        if export_items.is_empty() {
            for url in collect_branch_urls(
                &self.target_pane.items,
                &self.target_pane.tree_state.nodes,
                &self.target_pane.tree_state.hidden_prefixes,
                &key,
            ) {
                let req = super::site_map_ext::synthetic_scan_draft("GET", &url).raw;
                export_items.push((
                    url,
                    "GET".into(),
                    None,
                    String::new(),
                    req,
                    Vec::new(),
                    String::new(),
                ));
            }
        }
        if export_items.is_empty() {
            self.fail("当前分支下没有可保存的站点地图条目。", cx);
            return;
        }
        let count = export_items.len();
        let payload = if as_xml {
            match build_multi_item_ext_xml(&export_items) {
                Ok(xml) => xml.into_bytes(),
                Err(err) => {
                    self.fail(err, cx);
                    return;
                }
            }
        } else {
            let json_items: Vec<serde_json::Value> = export_items
                .iter()
                .map(|(url, method, status, _, req, resp, _)| {
                    serde_json::json!({
                        "url": url,
                        "method": method,
                        "status": status,
                        "length": resp.len(),
                        "request_base64": BASE64_STANDARD.encode(req),
                        "response_base64": BASE64_STANDARD.encode(resp),
                    })
                })
                .collect();
            match serde_json::to_vec_pretty(&serde_json::json!({
                "branch": key,
                "count": count,
                "items": json_items,
            })) {
                Ok(bytes) => bytes,
                Err(err) => {
                    self.fail(format!("序列化 JSON 失败：{err}"), cx);
                    return;
                }
            }
        };
        let suggested = if as_xml {
            "site-map-items.xml"
        } else {
            "site-map-items.json"
        };
        self.prompt_and_write_target_file(
            suggested,
            if as_xml { "xml" } else { "json" },
            payload,
            &format!("已保存 {count} 条站点地图项到文件"),
            window,
            cx,
        );
    }

    fn prompt_and_write_target_file(
        &mut self,
        suggested_name: &str,
        default_ext: &'static str,
        payload: Vec<u8>,
        success_prefix: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self.project;
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        let success_prefix = success_prefix.to_owned();
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &self.default_directory(),
            Some(suggested_name),
        );
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let _ = owner.update(cx, |_, _, cx| {
                this.update(cx, |view, cx| {
                    if !view.finish_file_picker(picker, cx) || !view.backend_available() {
                        return;
                    }
                    if view.project != project {
                        view.fail("项目已切换，导出文件未写入磁盘。", cx);
                        return;
                    }
                    match result {
                        Ok(Ok(Some(mut path))) => {
                            if path.extension().is_none() {
                                path.set_extension(default_ext);
                            }
                            match std::fs::write(&path, &payload) {
                                Ok(()) => {
                                    view.error = None;
                                    view.status =
                                        format!("{success_prefix}：{}", path.display());
                                    cx.notify();
                                }
                                Err(err) => view.fail(format!("写入导出文件失败：{err}"), cx),
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(err)) => view.fail(format!("无法选择保存路径：{err}"), cx),
                        Err(_) => view.fail("文件选择器已意外关闭。", cx),
                    }
                })
            });
        })
        .detach();
    }

    // --- Branch copy links ---

    pub(in crate::workbench) fn copy_target_branch_links(
        &mut self,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut seen = HashSet::new();
        let mut links = Vec::new();
        for (url, raw) in self.collect_loaded_responses_for_scope(Some(&key), window, cx) {
            if let Ok(found) =
                crate::workbench::wb_render_history::extract_response_links_list(&url, &raw)
            {
                for link in found {
                    if seen.insert(link.clone()) {
                        links.push(link);
                    }
                }
            }
        }
        // Also include known child/branch URLs from site map items & nodes under this host
        for url in collect_branch_urls(
            &self.target_pane.items,
            &self.target_pane.tree_state.nodes,
            &self.target_pane.tree_state.hidden_prefixes,
            &key,
        ) {
            if seen.insert(url.clone()) {
                links.push(url);
            }
        }
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
                links.push(url);
            }
        }
        if links.is_empty() {
            self.fail("当前主机分支中未提取到可复制的链接。", cx);
            return;
        }
        let count = links.len();
        cx.write_to_clipboard(ClipboardItem::new_string(links.join("\n")));
        self.error = None;
        self.status = format!("已复制该主机中的 {count} 条链接到剪贴板。");
        cx.notify();
    }

    // --- Engagement tools ---

    pub(in crate::workbench) fn open_global_search_with_term(
        &mut self,
        term: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let term = term.trim().to_owned();
        self.global_search.input.update(cx, |input, cx| {
            input.set_value(term, window, cx);
        });
        self.open_global_search(window, cx);
    }

    fn collect_branch_rows(&self, prefix: &str, cx: &App) -> Vec<HistoryRow> {
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

    fn collect_loaded_responses_for_scope(
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
                if summary.mime.eq_ignore_ascii_case("Script")
                    || lower.ends_with(".js")
                    || lower.ends_with(".mjs")
                    || lower.contains(".js?")
                {
                    if seen.insert(format!("ASSET:{url}")) {
                        entries.push(TargetEngagementEntry {
                            kind: "脚本资源".into(),
                            location: format!("{} {}", summary.method, summary.status),
                            detail: url,
                        });
                    }
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
            let doc = scraper::Html::parse_document(&body);
            if let Ok(sel) = scraper::Selector::parse("form") {
                forms_count += doc.select(&sel).count();
            }
            if let Ok(sel) = scraper::Selector::parse("input[name], select[name], textarea[name]") {
                for el in doc.select(&sel) {
                    if let Some(name) = el.value().attr("name")
                        && !name.trim().is_empty()
                    {
                        param_names.insert(name.trim().to_owned());
                    }
                }
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
                .unwrap_or_else(|_| super::site_map_ext::synthetic_scan_draft("GET", &url));
            self.start_target_crawl_task("发现内容", draft, window, cx);
        } else {
            self.fail("请先在站点地图中选择一条记录。", cx);
        }
    }

    // --- Compare site maps ---

    fn origin_path_map(&self, origin_key: &str, cx: &App) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        let row_metadata = self.history.read(cx).delegate().row_metadata.clone();
        for row in self.collect_branch_rows(origin_key, cx) {
            let url = row_url(&row);
            if url.is_empty() {
                continue;
            }
            let summary =
                target_row_summary_with_metadata(&row, None, row_metadata.get(&row.id()));
            let path = if summary.url.is_empty() {
                http_history_path_and_query(&url)
            } else {
                summary.url
            };
            let method = if summary.method.is_empty() {
                "GET".into()
            } else {
                summary.method
            };
            let status = if summary.status.is_empty() {
                "-".into()
            } else {
                summary.status
            };
            let len = if summary.length.is_empty() {
                "0".into()
            } else {
                summary.length
            };
            map.insert(path, format!("{method} · status={status} · len={len}"));
        }
        if map.is_empty() {
            for node in &self.target_pane.tree_state.nodes {
                if !self.target_pane.tree_state.is_hidden(node)
                    && self
                        .target_pane
                        .tree_state
                        .is_descendant_or_self(node, origin_key)
                {
                    let path = http_history_path_and_query(&node.url_prefix);
                    map.entry(path)
                        .or_insert_with(|| format!("node · count={}", node.direct_count));
                }
            }
        }
        map
    }

    pub(in crate::workbench) fn open_compare_site_maps(
        &mut self,
        preferred_origin: Option<String>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let origins: Vec<String> = self
            .target_pane
            .tree_state
            .nodes
            .iter()
            .filter(|n| {
                n.kind == SiteMapNodeKind::Origin && !self.target_pane.tree_state.is_hidden(n)
            })
            .map(|n| n.key.clone())
            .collect();
        let left_origin = preferred_origin
            .and_then(|p| {
                let canon = canonical_site_map_url_key(&p).unwrap_or(p.clone());
                origins
                    .iter()
                    .find(|o| **o == canon || url_matches_node_prefix(&p, o))
                    .cloned()
                    .or(Some(p))
            })
            .or_else(|| origins.first().cloned())
            .unwrap_or_default();
        let right_origin = origins.iter().find(|o| **o != left_origin).cloned();
        let left_map = if left_origin.is_empty() {
            BTreeMap::new()
        } else {
            self.origin_path_map(&left_origin, cx)
        };
        let right_map = right_origin
            .as_deref()
            .map(|r| self.origin_path_map(r, cx))
            .unwrap_or_default();
        let (rows, only_left, only_right, different, same) =
            compute_site_map_diff(&left_map, &right_map);
        self.target_pane.compare_modal = Some(TargetCompareState {
            available_origins: origins,
            left_origin,
            right_origin,
            external_label: None,
            external_map: BTreeMap::new(),
            rows,
            only_left,
            only_right,
            different,
            same,
        });
        self.error = None;
        self.status = "已打开站点地图对比视图。".into();
        cx.notify();
    }

    pub(in crate::workbench) fn select_compare_site_map_right_origin(
        &mut self,
        origin: String,
        cx: &mut Context<Self>,
    ) {
        let Some(left_origin) = self
            .target_pane
            .compare_modal
            .as_ref()
            .map(|s| s.left_origin.clone())
        else {
            return;
        };
        let left_map = self.origin_path_map(&left_origin, cx);
        let right_map = self.origin_path_map(&origin, cx);
        let (rows, only_left, only_right, different, same) =
            compute_site_map_diff(&left_map, &right_map);
        if let Some(state) = self.target_pane.compare_modal.as_mut() {
            state.right_origin = Some(origin);
            state.external_label = None;
            state.external_map.clear();
            state.rows = rows;
            state.only_left = only_left;
            state.only_right = only_right;
            state.different = different;
            state.same = same;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn load_compare_site_map_file(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() || self.file_picker_pending() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let paths = crate::native_terminate::prompt_for_paths(
            window,
            cx,
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("加载待对比的站点地图文件 (XML / JSON / URL 列表)".into()),
            },
        );
        cx.spawn_in(window, async move |this, cx| {
            let paths = paths.await;
            let _ = this.update_in(cx, |view, _, cx| {
                if !view.finish_file_picker(picker, cx) || view.project != Some(project) {
                    return;
                }
                match paths {
                    Ok(Ok(Some(paths))) => {
                        let Some(path) = paths.into_iter().next() else {
                            return;
                        };
                        match std::fs::read(&path) {
                            Ok(bytes) if bytes.len() > MAX_COMPARE_FILE_BYTES => {
                                view.fail("站点地图对比文件超过 4 MB 上限。", cx);
                            }
                            Ok(bytes) => match String::from_utf8(bytes) {
                                Ok(text) => match parse_external_site_map_file(&text) {
                                    Ok(ext_map) => {
                                        let left_origin = view
                                            .target_pane
                                            .compare_modal
                                            .as_ref()
                                            .map(|s| s.left_origin.clone())
                                            .unwrap_or_default();
                                        let left_map = view.origin_path_map(&left_origin, cx);
                                        let (rows, only_left, only_right, different, same) =
                                            compute_site_map_diff(&left_map, &ext_map);
                                        let name = path
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .unwrap_or("外部站点地图")
                                            .to_owned();
                                        if let Some(state) =
                                            view.target_pane.compare_modal.as_mut()
                                        {
                                            state.right_origin = None;
                                            state.external_label = Some(name.clone());
                                            state.external_map = ext_map;
                                            state.rows = rows;
                                            state.only_left = only_left;
                                            state.only_right = only_right;
                                            state.different = different;
                                            state.same = same;
                                        }
                                        view.error = None;
                                        view.status = format!("已加载对比快照：{name}");
                                        cx.notify();
                                    }
                                    Err(err) => view.fail(err, cx),
                                },
                                Err(_) => view.fail("所选站点地图文件不是有效的 UTF-8 文本。", cx),
                            },
                            Err(err) => view.fail(format!("读取对比文件失败：{err}"), cx),
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(err)) => view.fail(format!("选择对比文件失败：{err}"), cx),
                    Err(_) => view.fail("文件选择器已关闭。", cx),
                }
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn send_compare_site_maps_to_comparer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.target_pane.compare_modal.clone() else {
            return;
        };
        let left_map = self.origin_path_map(&state.left_origin, cx);
        let (right_label, right_map) = if let Some(ext) = state.external_label.clone() {
            (ext, state.external_map)
        } else if let Some(ref right_origin) = state.right_origin {
            (right_origin.clone(), self.origin_path_map(right_origin, cx))
        } else {
            ("空站点地图".to_owned(), BTreeMap::new())
        };
        let format_manifest = |m: &BTreeMap<String, String>| -> Vec<u8> {
            m.iter()
                .map(|(path, meta)| format!("{path}  [{meta}]"))
                .collect::<Vec<_>>()
                .join("\n")
                .into_bytes()
        };
        let left_label = if state.left_origin.is_empty() {
            "Site map A".to_owned()
        } else {
            format!("Site map: {}", state.left_origin)
        };
        let right_title = format!("Site map: {right_label}");
        let ok_l = self.push_text_to_comparer(&left_label, format_manifest(&left_map), window, cx);
        let ok_r = if ok_l {
            self.push_text_to_comparer(&right_title, format_manifest(&right_map), window, cx)
        } else {
            false
        };
        if ok_l && ok_r {
            self.target_pane.compare_modal = None;
            self.cancel_plugin_interactions(cx);
            self.tool = Tool::Comparer;
            self.focus.focus(window, cx);
            self.error = None;
            self.status = "已将两个站点地图清单发送到 Comparer 进行逐行对比。".into();
            cx.notify();
        }
    }

    // --- Detached Site Map window ---

    pub(in crate::workbench) fn open_detached_site_map_window(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project else {
            return;
        };
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let owner = cx.entity();
        let bounds = gpui_kit::Bounds::centered(None, gpui_kit::size(px(1100.), px(720.)), cx);
        cx.defer(move |cx| {
            if owner.read(cx).project != Some(project) {
                return;
            }
            if let Some(existing) = owner.read(cx).target_pane.detached_site_map_window
                && cx.windows().contains(&existing)
                && crate::chrome::activate_popup_window(existing, cx)
            {
                return;
            }
            let opened = crate::chrome::open_popup_window(
                crate::chrome::mio_window_options(bounds, false, false),
                cx,
                |window, cx| cx.new(|cx| DetachedSiteMapWindow::new(owner.clone(), window, cx)),
            );
            owner.update(cx, |view, cx| match opened {
                Ok((handle, _)) => {
                    view.target_pane.detached_site_map_window = Some(handle);
                    view.error = None;
                    view.status = "已在新窗口中打开站点地图。".into();
                    cx.notify();
                }
                Err(error) => view.fail(format!("无法打开新站点地图窗口：{error}"), cx),
            });
        });
    }

    // --- Overlay Renderers (only rendered when explicitly opened!) ---

    pub(in crate::workbench) fn render_target_engagement_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let report = self.target_pane.engagement_report.as_ref()?;
        let title = report.title.clone();
        let subtitle = report.subtitle.clone();
        let summary = report.summary.clone();
        let search_query = report.search_query.clone();
        let entries = report.entries.clone();
        let copy_text = entries
            .iter()
            .map(|e| format!("[{}] {} -> {}", e.kind, e.location, e.detail))
            .collect::<Vec<_>>()
            .join("\n");
        let dark = theme::is_dark();

        let search_btn = search_query.map(|query| {
            button("target-engagement-global-search", "在全局搜索中查找")
                .icon(Icon::new(IconName::Search).size(px(12.)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.target_pane.engagement_report = None;
                    this.open_global_search_with_term(query.clone(), window, cx);
                }))
        });
        let copy_btn = (!copy_text.is_empty()).then(|| {
            button("target-engagement-copy", "复制全部结果")
                .icon(Icon::new(IconName::Copy).size(px(12.)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
                    this.status = "已复制参与工具分析结果到剪贴板。".into();
                    cx.notify();
                }))
        });
        let close_top_btn = Button::new("target-engagement-close")
            .ghost()
            .small()
            .compact()
            .icon(IconName::Close)
            .accessibility_label("关闭参与工具结果")
            .on_click(cx.listener(|this, _, _, cx| {
                this.target_pane.engagement_report = None;
                cx.notify();
            }));
        let close_bottom_btn = button("target-engagement-done", "关闭")
            .primary()
            .on_click(cx.listener(|this, _, _, cx| {
                this.target_pane.engagement_report = None;
                cx.notify();
            }));

        Some(
            div()
                .id("target-engagement-overlay")
                .test_support()
                .absolute()
                .inset_0()
                .bg(gpui_kit::hsla(0., 0., 0., 0.35))
                .flex()
                .items_center()
                .justify_center()
                .p(px(24.))
                .child(
                    div()
                        .id("target-engagement-dialog")
                        .test_support()
                        .w(px(720.))
                        .max_w_full()
                        .h(px(460.))
                        .max_h_full()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::surface())
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(
                            div()
                                .px(px(14.))
                                .py(px(10.))
                                .border_b_1()
                                .border_color(theme::border())
                                .bg(theme::toolbar_surface())
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.))
                                        .child(
                                            div()
                                                .text_size(theme::scaled_px(14.))
                                                .font_weight(gpui_kit::FontWeight::BOLD)
                                                .child(title),
                                        )
                                        .child(
                                            div()
                                                .text_size(theme::scaled_px(12.))
                                                .text_color(theme::muted_foreground())
                                                .truncate()
                                                .child(subtitle),
                                        ),
                                )
                                .child(close_top_btn),
                        )
                        .child(
                            div()
                                .px(px(14.))
                                .py(px(8.))
                                .border_b_1()
                                .border_color(theme::border())
                                .bg(rgb(if dark { 0x25272b } else { 0xf8f9fa }))
                                .text_size(theme::ui_font_size())
                                .child(summary),
                        )
                        .child(
                            div()
                                .id("target-engagement-entries")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .p(px(10.))
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .when(entries.is_empty(), |box_el| {
                                    box_el.child(
                                        div()
                                            .p(px(10.))
                                            .text_color(theme::muted_foreground())
                                            .child("当前范围内暂无直接匹配项。"),
                                    )
                                })
                                .children(entries.into_iter().enumerate().map(|(idx, entry)| {
                                    div()
                                        .id(("target-engagement-entry", idx))
                                        .p(px(8.))
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(theme::border())
                                        .bg(theme::surface())
                                        .flex()
                                        .flex_col()
                                        .gap(px(3.))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .gap(px(8.))
                                                .child(
                                                    div()
                                                        .font_weight(gpui_kit::FontWeight::BOLD)
                                                        .text_size(theme::scaled_px(12.))
                                                        .child(entry.kind),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(theme::scaled_px(11.5))
                                                        .text_color(theme::muted_foreground())
                                                        .truncate()
                                                        .child(entry.location),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .font_family(theme::mono_font_family())
                                                .text_size(theme::scaled_px(12.))
                                                .child(entry.detail),
                                        )
                                })),
                        )
                        .child(
                            div()
                                .px(px(14.))
                                .py(px(8.))
                                .border_t_1()
                                .border_color(theme::border())
                                .bg(theme::toolbar_surface())
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap(px(8.))
                                .when_some(search_btn, |bar, btn| bar.child(btn))
                                .when_some(copy_btn, |bar, btn| bar.child(btn))
                                .child(close_bottom_btn),
                        ),
                )
                .into_any_element(),
        )
    }

    pub(in crate::workbench) fn render_target_compare_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let state = self.target_pane.compare_modal.as_ref()?;
        let left_label = if state.left_origin.is_empty() {
            "当前站点".to_owned()
        } else {
            state.left_origin.clone()
        };
        let right_label = state
            .external_label
            .clone()
            .or_else(|| state.right_origin.clone())
            .unwrap_or_else(|| "未选择对比目标".into());
        let other_origins: Vec<String> = state
            .available_origins
            .iter()
            .filter(|o| **o != state.left_origin)
            .cloned()
            .collect();
        let active_right = state.right_origin.clone();
        let rows = state.rows.clone();
        let stats = format!(
            "仅在源站点：{} · 仅在目标站点：{} · 状态/长度不同：{} · 完全一致：{}",
            state.only_left, state.only_right, state.different, state.same
        );
        let dark = theme::is_dark();
        let origin_buttons: Vec<Button> = other_origins
            .into_iter()
            .enumerate()
            .map(|(idx, origin)| {
                let selected = active_right.as_deref() == Some(origin.as_str());
                let o_click = origin.clone();
                button(
                    SharedString::from(format!("target-compare-origin-{idx}")),
                    origin,
                )
                .selected(selected)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_compare_site_map_right_origin(o_click.clone(), cx);
                }))
            })
            .collect();
        let has_origin_buttons = !origin_buttons.is_empty();
        let close_top_btn = Button::new("target-compare-close")
            .ghost()
            .small()
            .compact()
            .icon(IconName::Close)
            .accessibility_label("关闭站点地图对比")
            .on_click(cx.listener(|this, _, _, cx| {
                this.target_pane.compare_modal = None;
                cx.notify();
            }));
        let load_file_btn = button("target-compare-load-file", "从文件加载快照…")
            .icon(Icon::new(IconName::Upload).size(px(12.)))
            .on_click(cx.listener(|this, _, window, cx| {
                this.load_compare_site_map_file(window, cx);
            }));
        let to_comparer_btn = button("target-compare-to-comparer", "发送到 Comparer 对比")
            .icon(Icon::new(IconName::GitCompare).size(px(12.)))
            .on_click(cx.listener(|this, _, window, cx| {
                this.send_compare_site_maps_to_comparer(window, cx);
            }));
        let close_bottom_btn = button("target-compare-done", "关闭")
            .primary()
            .on_click(cx.listener(|this, _, _, cx| {
                this.target_pane.compare_modal = None;
                cx.notify();
            }));

        Some(
            div()
                .id("target-compare-overlay")
                .test_support()
                .absolute()
                .inset_0()
                .bg(gpui_kit::hsla(0., 0., 0., 0.35))
                .flex()
                .items_center()
                .justify_center()
                .p(px(24.))
                .child(
                    div()
                        .id("target-compare-dialog")
                        .test_support()
                        .w(px(820.))
                        .max_w_full()
                        .h(px(520.))
                        .max_h_full()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::surface())
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(
                            div()
                                .px(px(14.))
                                .py(px(10.))
                                .border_b_1()
                                .border_color(theme::border())
                                .bg(theme::toolbar_surface())
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(theme::scaled_px(14.))
                                        .font_weight(gpui_kit::FontWeight::BOLD)
                                        .child("比较站点地图 (Compare site maps)"),
                                )
                                .child(close_top_btn),
                        )
                        .child(
                            div()
                                .px(px(14.))
                                .py(px(8.))
                                .border_b_1()
                                .border_color(theme::border())
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap(px(8.))
                                        .child(
                                            div()
                                                .text_size(theme::ui_font_size())
                                                .child(format!("源站点：{left_label}   ↔   对比目标：{right_label}")),
                                        )
                                        .child(load_file_btn),
                                )
                                .when(has_origin_buttons, |col| {
                                    col.child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .items_center()
                                            .gap(px(6.))
                                            .child(
                                                div()
                                                    .text_size(theme::scaled_px(12.))
                                                    .text_color(theme::muted_foreground())
                                                    .child("切换对比主机："),
                                            )
                                            .children(origin_buttons),
                                    )
                                })
                                .child(
                                    div()
                                        .text_size(theme::scaled_px(12.))
                                        .text_color(theme::muted_foreground())
                                        .child(stats),
                                ),
                        )
                        .child(
                            div()
                                .id("target-compare-rows")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .when(rows.is_empty(), |el| {
                                    el.child(
                                        div()
                                            .p(px(16.))
                                            .text_color(theme::muted_foreground())
                                            .child("暂无可对比路径；可从文件加载 XML/JSON 站点地图快照或捕获多个主机后重试。"),
                                    )
                                })
                                .children(rows.into_iter().enumerate().map(|(idx, row)| {
                                    let (badge, row_bg) = match row.kind {
                                        TargetCompareDiffKind::OnlyLeft => (
                                            "仅源站点",
                                            rgb(if dark { 0x1f3626 } else { 0xedf7ed }),
                                        ),
                                        TargetCompareDiffKind::OnlyRight => (
                                            "仅目标",
                                            rgb(if dark { 0x3a2424 } else { 0xfdecea }),
                                        ),
                                        TargetCompareDiffKind::Different => (
                                            "有差异",
                                            rgb(if dark { 0x3b311e } else { 0xfff8e1 }),
                                        ),
                                        TargetCompareDiffKind::Same => (
                                            "一致",
                                            rgb(if dark { 0x25272b } else { 0xffffff }),
                                        ),
                                    };
                                    div()
                                        .id(("target-compare-row", idx))
                                        .px(px(12.))
                                        .py(px(5.))
                                        .border_b_1()
                                        .border_color(theme::border())
                                        .bg(row_bg)
                                        .flex()
                                        .items_center()
                                        .gap(px(10.))
                                        .child(
                                            div()
                                                .w(px(64.))
                                                .flex_shrink_0()
                                                .font_weight(gpui_kit::FontWeight::BOLD)
                                                .text_size(theme::scaled_px(11.5))
                                                .child(badge),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .font_family(theme::mono_font_family())
                                                .text_size(theme::scaled_px(12.))
                                                .child(row.path),
                                        )
                                        .child(
                                            div()
                                                .w(px(190.))
                                                .flex_shrink_0()
                                                .truncate()
                                                .text_size(theme::scaled_px(11.5))
                                                .child(row.left_summary),
                                        )
                                        .child(
                                            div()
                                                .w(px(190.))
                                                .flex_shrink_0()
                                                .truncate()
                                                .text_size(theme::scaled_px(11.5))
                                                .child(row.right_summary),
                                        )
                                })),
                        )
                        .child(
                            div()
                                .px(px(14.))
                                .py(px(8.))
                                .border_t_1()
                                .border_color(theme::border())
                                .bg(theme::toolbar_surface())
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap(px(8.))
                                .child(to_comparer_btn)
                                .child(close_bottom_btn),
                        ),
                )
                .into_any_element(),
        )
    }
}

struct DetachedSiteMapWindow {
    owner: WeakEntity<Workbench>,
    project: Option<ProjectKey>,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl DetachedSiteMapWindow {
    fn new(owner: Entity<Workbench>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let project = owner.read(cx).project;
        let handle = window.window_handle();
        let subscription = cx.observe_in(&owner, window, move |this, owner, window, cx| {
            if owner.read(cx).project != this.project {
                window.remove_window();
                return;
            }
            cx.notify();
        });
        let released = owner.downgrade();
        cx.on_release(move |_, cx| {
            cx.defer(move |cx| {
                let _ = released.update(cx, |view, _| {
                    if view.target_pane.detached_site_map_window == Some(handle) {
                        view.target_pane.detached_site_map_window = None;
                    }
                });
            });
        })
        .detach();
        window.set_window_title("Site map");
        Self {
            owner: owner.downgrade(),
            project,
            focus: cx.focus_handle(),
            _subscription: subscription,
        }
    }
}

impl Render for DetachedSiteMapWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(owner) = self.owner.upgrade() else {
            return div()
                .id("target-sitemap-detached-empty")
                .size_full()
                .into_any_element();
        };
        let title_bar = crate::chrome::render_title_bar(
            "target-sitemap-detached",
            "Site map",
            crate::chrome::TitleBarControls::DIALOG,
            |window, _| window.remove_window(),
            window,
            cx,
        );
        let (nodes, rows, req_preview, resp_preview) = {
            let view = owner.read(cx);
            let nodes: Vec<(u8, String, usize)> = view
                .target_pane
                .tree_state
                .visible_nodes()
                .into_iter()
                .take(300)
                .map(|n| (n.depth, n.label.clone(), n.subtree_count))
                .collect();
            let rows: Vec<(String, String, String, String)> = view
                .target_pane
                .items
                .iter()
                .filter(|r| !view.target_pane.hidden_item_ids.contains(&r.id()))
                .take(200)
                .map(|row| {
                    let s = target_row_summary_with_metadata(row, None, None);
                    (s.host, s.method, s.url, s.status)
                })
                .collect();
            (
                nodes,
                rows,
                view.target_pane.request.raw.read(cx).value().to_string(),
                view.target_pane.response.raw.read(cx).value().to_string(),
            )
        };
        let body = div()
            .size_full()
            .flex()
            .child(
                div()
                    .id("target-sitemap-detached-tree")
                    .w(px(260.))
                    .h_full()
                    .overflow_y_scroll()
                    .border_r_1()
                    .border_color(theme::border())
                    .flex()
                    .flex_col()
                    .py(px(4.))
                    .children(nodes.into_iter().map(|(depth, label, count)| {
                        div()
                            .h(px(22.))
                            .pl(px(8. + f32::from(depth) * 14.))
                            .pr(px(8.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().flex_1().min_w_0().truncate().child(label))
                            .child(
                                div()
                                    .text_size(theme::scaled_px(11.))
                                    .text_color(theme::muted_foreground())
                                    .child(count.to_string()),
                            )
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("target-sitemap-detached-items")
                            .h(px(280.))
                            .overflow_y_scroll()
                            .border_b_1()
                            .border_color(theme::border())
                            .flex()
                            .flex_col()
                            .children(rows.into_iter().map(|(host, method, url, status)| {
                                div()
                                    .h(theme::row_height())
                                    .px(px(10.))
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .border_b_1()
                                    .border_color(theme::border())
                                    .child(div().w(px(140.)).truncate().child(host))
                                    .child(div().w(px(54.)).child(method))
                                    .child(div().w(px(48.)).child(status))
                                    .child(div().flex_1().min_w_0().truncate().child(url))
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .child(
                                div()
                                    .id("target-sitemap-detached-request")
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .p(px(10.))
                                    .overflow_y_scroll()
                                    .border_r_1()
                                    .border_color(theme::border())
                                    .font_family(theme::mono_font_family())
                                    .text_size(theme::editor_font_size())
                                    .child(if req_preview.is_empty() {
                                        "Request".into()
                                    } else {
                                        req_preview
                                    }),
                            )
                            .child(
                                div()
                                    .id("target-sitemap-detached-response")
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .p(px(10.))
                                    .overflow_y_scroll()
                                    .font_family(theme::mono_font_family())
                                    .text_size(theme::editor_font_size())
                                    .child(if resp_preview.is_empty() {
                                        "Response".into()
                                    } else {
                                        resp_preview
                                    }),
                            ),
                    ),
            );
        div()
            .id("target-sitemap-detached-window")
            .test_support()
            .size_full()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .font_family(theme::ui_font_family())
            .text_size(theme::ui_font_size())
            .text_color(theme::foreground())
            .bg(theme::background())
            .child(title_bar)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}
