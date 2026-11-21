use std::collections::{BTreeMap, BTreeSet};

use super::*;

pub(super) const MAX_COMPARER_ITEMS: usize = 32;
pub(super) const MAX_COMPARER_ITEM_BYTES: usize = 256 * 1024;
pub(super) const MAX_COMPARE_FILE_BYTES: usize = 4 * 1024 * 1024;

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

pub(super) fn split_http_body(raw: &[u8]) -> &[u8] {
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

pub(super) fn extract_comments_from_response(url: &str, raw: &[u8]) -> Vec<TargetEngagementEntry> {
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

pub(super) fn extract_scripts_from_response(url: &str, raw: &[u8]) -> Vec<TargetEngagementEntry> {
    let body = String::from_utf8_lossy(split_http_body(raw));
    mio_core::html_extract::extract_scripts_from_html(url, &body)
        .into_iter()
        .map(|script| TargetEngagementEntry {
            kind: if script.is_external {
                "外部脚本".into()
            } else {
                format!("内联脚本 #{}", script.index + 1)
            },
            location: url.to_owned(),
            detail: script.detail,
        })
        .collect()
}

pub(super) fn build_multi_item_ext_xml(
    items: &[(String, String, Option<u16>, String, Vec<u8>, Vec<u8>, String)],
) -> Result<String, String> {
    let now = crate::history::local_time(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
    );
    let export_items: Vec<mio_core::export_xml::XmlHttpExportItem<'_>> = items
        .iter()
        .map(
            |(url, method, status, time_str, req_bytes, resp_bytes, comment)| {
                mio_core::export_xml::XmlHttpExportItem {
                    url,
                    method,
                    status: *status,
                    time_str,
                    request_bytes: req_bytes,
                    response_bytes: resp_bytes,
                    comment,
                }
            },
        )
        .collect();
    Ok(mio_core::export_xml::build_http_items_xml(
        &now,
        &export_items,
    ))
}

pub(super) fn parse_external_site_map_file(text: &str) -> Result<BTreeMap<String, String>, String> {
    let trimmed = text.trim();
    let mut map = BTreeMap::new();
    if trimmed.starts_with('<') {
        return mio_core::export_xml::parse_xml_site_map_entries(trimmed);
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

pub(super) fn compute_site_map_diff(
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
