use std::collections::HashMap;
use std::fmt::Write;

use mio_core::{
    ExtHttpHighlight, FlowId, FlowRecord, FlowSummary, HttpRequest, TargetScope, TrafficSource,
};

use super::*;

pub(in crate::workbench::tools_extra::logger) struct ParsedLoggerUrl {
    pub(in crate::workbench::tools_extra::logger) host: String,
    pub(in crate::workbench::tools_extra::logger) path: String,
    pub(in crate::workbench::tools_extra::logger) query: String,
}

pub(in crate::workbench::tools_extra::logger) fn parse_logger_url(raw: &str) -> ParsedLoggerUrl {
    let trimmed = raw.trim();
    if let Ok(parsed) = url::Url::parse(trimmed) {
        let host = match (parsed.host_str(), parsed.port()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_owned(),
            (None, _) => String::new(),
        };
        let path = parsed.path();
        let path = if path.is_empty() {
            "/".to_owned()
        } else {
            path.to_owned()
        };
        let query = parsed.query().unwrap_or("").to_owned();
        return ParsedLoggerUrl { host, path, query };
    }
    let (scheme, after_scheme) = match trimmed.split_once("://") {
        Some((s, rest)) => (Some(s.to_ascii_lowercase()), rest),
        None => (None, trimmed),
    };
    let (authority, rest) = if scheme.is_some() {
        match after_scheme.find(['/', '?', '#']) {
            Some(idx) => (&after_scheme[..idx], &after_scheme[idx..]),
            None => (after_scheme, ""),
        }
    } else {
        ("", after_scheme)
    };
    let host = match scheme.as_deref() {
        Some("http") => authority.strip_suffix(":80").unwrap_or(authority),
        Some("https") => authority.strip_suffix(":443").unwrap_or(authority),
        _ => authority,
    }
    .to_owned();
    let without_fragment = rest.split('#').next().unwrap_or("");
    let (path_raw, query) = match without_fragment.split_once('?') {
        Some((p, q)) => (p, q.to_owned()),
        None => (without_fragment, String::new()),
    };
    let path = if path_raw.is_empty() {
        "/".to_owned()
    } else {
        path_raw.to_owned()
    };
    ParsedLoggerUrl { host, path, query }
}

pub(in crate::workbench::tools_extra::logger) fn count_query_params(query: &str) -> usize {
    if query.is_empty() {
        return 0;
    }
    query.split('&').filter(|part| !part.is_empty()).count()
}

pub(in crate::workbench::tools_extra::logger) fn count_url_query_params(raw_url: &str) -> usize {
    let parsed = parse_logger_url(raw_url);
    count_query_params(&parsed.query)
}

fn count_json_params(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Object(map) => map
            .values()
            .map(|v| match v {
                serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                    count_json_params(v).max(1)
                }
                _ => 1,
            })
            .sum(),
        serde_json::Value::Array(items) => items.iter().map(count_json_params).sum(),
        _ => 0,
    }
}

pub(in crate::workbench::tools_extra::logger) fn count_request_params(request: &HttpRequest) -> usize {
    let mut count = count_url_query_params(&request.url);
    let mut content_type = String::new();
    for header in &request.headers {
        if header.name.eq_ignore_ascii_case("cookie") {
            let value = String::from_utf8_lossy(&header.value);
            count += value
                .split(';')
                .filter(|part| part.trim().contains('='))
                .count();
        } else if header.name.eq_ignore_ascii_case("content-type") {
            content_type = String::from_utf8_lossy(&header.value).to_ascii_lowercase();
        }
    }
    if !request.body.is_empty() {
        if content_type.contains("application/x-www-form-urlencoded") {
            if let Ok(body_str) = std::str::from_utf8(&request.body) {
                count += body_str.split('&').filter(|p| !p.trim().is_empty()).count();
            }
        } else if (content_type.contains("json")
            || request.body.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{'))
            && let Ok(val) = serde_json::from_slice::<serde_json::Value>(&request.body)
        {
            count += count_json_params(&val);
        }
    }
    count
}

pub(in crate::workbench::tools_extra::logger) fn entry_param_count(
    entry: &FlowSummary,
    param_counts: &HashMap<FlowId, usize>,
) -> usize {
    param_counts
        .get(&entry.id)
        .copied()
        .unwrap_or_else(|| count_url_query_params(&entry.url))
}

pub(in crate::workbench::tools_extra::logger) fn entry_comment(
    flow_id: FlowId,
    live_notes: &HashMap<FlowId, (u64, String)>,
) -> String {
    live_notes
        .get(&flow_id)
        .map(|(_, text)| text.replace("\r\n", " ").replace(['\r', '\n', '\t'], " "))
        .unwrap_or_default()
}

pub(in crate::workbench::tools_extra::logger) fn logger_source_index(source: &TrafficSource) -> usize {
    match source {
        TrafficSource::Proxy => 0,
        TrafficSource::Repeater => 1,
        TrafficSource::Intruder => 2,
        TrafficSource::Scanner => 3,
        TrafficSource::Plugin(_) => 4,
    }
}

pub(in crate::workbench::tools_extra::logger) fn logger_source_label(source: &TrafficSource) -> String {
    match source {
        TrafficSource::Proxy => "Proxy".to_owned(),
        TrafficSource::Repeater => "Repeater".to_owned(),
        TrafficSource::Intruder => "Intruder".to_owned(),
        TrafficSource::Scanner => "Scanner".to_owned(),
        TrafficSource::Plugin(name) if name.is_empty() => "Extensions".to_owned(),
        TrafficSource::Plugin(name) => format!("Extensions({name})"),
    }
}

pub(in crate::workbench::tools_extra::logger) fn capture_filter_summary(max_entries: usize, filters: &LoggerFilters) -> String {
    let mut parts: Vec<String> = Vec::new();
    if filters.capture_sources != [true; 5] {
        let enabled: Vec<&str> = LOGGER_CAPTURE_SOURCE_LABELS
            .iter()
            .enumerate()
            .filter_map(|(idx, lbl)| filters.capture_sources[idx].then_some(*lbl))
            .collect();
        if enabled.is_empty() {
            parts.push("未启用任何工具来源".into());
        } else {
            parts.push(format!("仅捕获 {}", enabled.join("、")));
        }
    }
    if filters.capture_in_scope_only {
        parts.push("仅目标范围内".into());
    }
    if filters.capture_status_classes != [true; 4] {
        let hidden: Vec<&str> = ["2xx", "3xx", "4xx", "5xx"]
            .into_iter()
            .enumerate()
            .filter_map(|(idx, lbl)| (!filters.capture_status_classes[idx]).then_some(lbl))
            .collect();
        if !hidden.is_empty() {
            parts.push(format!("忽略 {} 响应", hidden.join("、")));
        }
    }
    if !filters.capture_filter_script.trim().is_empty() {
        parts.push("捕获脚本".into());
    }
    if parts.is_empty() {
        format!("捕获筛选：记录上限 {max_entries} 条 | 实时捕获所有工具流量")
    } else {
        format!(
            "捕获筛选：记录上限 {max_entries} 条 | {}",
            parts.join("；")
        )
    }
}

pub(in crate::workbench::tools_extra::logger) fn view_filter_summary(
    source_filter: &Option<TrafficSource>,
    filters: &LoggerFilters,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(src) = source_filter {
        parts.push(format!("来源 {}", logger_source_label(src)));
    }
    if filters.in_scope_only {
        parts.push("仅目标范围内".into());
    }
    if filters.status_classes != [true; 4] {
        let hidden: Vec<&str> = ["2xx", "3xx", "4xx", "5xx"]
            .into_iter()
            .enumerate()
            .filter_map(|(idx, lbl)| (!filters.status_classes[idx]).then_some(lbl))
            .collect();
        if !hidden.is_empty() {
            parts.push(format!("隐藏 {} 响应", hidden.join("、")));
        }
    }
    if filters.script_enabled || !filters.view_filter_script.trim().is_empty() {
        parts.push("脚本筛选".into());
    }
    if parts.is_empty() {
        "视图筛选：显示全部条目".into()
    } else {
        format!("视图筛选：{}", parts.join("；"))
    }
}

pub(in crate::workbench::tools_extra::logger) fn matches_logger_source(
    source_filter: &Option<TrafficSource>,
    source: &TrafficSource,
) -> bool {
    match source_filter {
        None => true,
        Some(TrafficSource::Plugin(_)) => matches!(source, TrafficSource::Plugin(_)),
        Some(expected) => source == expected,
    }
}

pub(in crate::workbench::tools_extra::logger) fn is_script_expression(expr: &str) -> bool {
    let trimmed = expr.trim();
    !trimmed.is_empty()
        && (trimmed.contains("==")
            || trimmed.contains("!=")
            || trimmed.contains("&&")
            || trimmed.contains("||")
            || trimmed.contains(">=")
            || trimmed.contains("<=")
            || trimmed.contains("request.")
            || trimmed.contains("response.")
            || trimmed.contains("regex_matches("))
}

fn normalize_logger_filter_script(expr: &str) -> String {
    format!(
        "let status = response.status;\nlet method = request.method;\nlet url = request.url;\nlet host = request.host;\nlet path = request.path;\nlet query = request.query;\nlet length = response.length;\n{expr}"
    )
}

pub(in crate::workbench::tools_extra::logger) fn validate_logger_script(expr: &str) -> Result<(), String> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let normalized = normalize_logger_filter_script(trimmed);
    mio_core::validate_filter_script(&normalized).map_err(|err| err.to_string())
}

pub(in crate::workbench::tools_extra::logger) fn eval_logger_script_with_context(
    entry: &FlowSummary,
    expr: &str,
    req_raw: Option<&[u8]>,
    resp_raw: Option<&[u8]>,
    notes: &str,
    highlighted: bool,
) -> bool {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return true;
    }
    let normalized = normalize_logger_filter_script(trimmed);
    mio_core::eval_http_filter_script(
        &normalized,
        &entry.method,
        &entry.url,
        entry.status,
        entry.response_bytes,
        req_raw,
        resp_raw,
        notes,
        highlighted,
        None,
    )
    .unwrap_or(false)
}

pub(in crate::workbench::tools_extra::logger) fn eval_logger_script(entry: &FlowSummary, expr: &str) -> bool {
    eval_logger_script_with_context(entry, expr, None, None, "", false)
}

pub(in crate::workbench::tools_extra::logger) fn matches_status_classes(status: Option<u16>, status_classes: &[bool; 4]) -> bool {
    if status_classes == &[true; 4] {
        return true;
    }
    let Some(code) = status else {
        return false;
    };
    match code {
        200..=299 => status_classes[0],
        300..=399 => status_classes[1],
        400..=499 => status_classes[2],
        500..=599 => status_classes[3],
        _ => false,
    }
}

pub(in crate::workbench::tools_extra::logger) fn matches_logger_capture_filter(
    entry: &FlowSummary,
    filters: &LoggerFilters,
    scope: &TargetScope,
) -> bool {
    if !filters.capture_sources[logger_source_index(&entry.source)] {
        return false;
    }
    if filters.capture_in_scope_only && !scope.is_url_in_scope(&entry.url) {
        return false;
    }
    if !matches_status_classes(entry.status, &filters.capture_status_classes) {
        return false;
    }
    if !filters.capture_filter_script.trim().is_empty()
        && !eval_logger_script(entry, &filters.capture_filter_script)
    {
        return false;
    }
    true
}

#[allow(clippy::too_many_arguments)]
pub(in crate::workbench::tools_extra::logger) fn filter_and_sort_logger<'a>(
    entries: &'a [FlowSummary],
    source_filter: &Option<TrafficSource>,
    search_term: &str,
    sort: LoggerSort,
    filters: &LoggerFilters,
    scope: &TargetScope,
    live_notes: &HashMap<FlowId, (u64, String)>,
    live_highlights: &HashMap<FlowId, ExtHttpHighlight>,
    loaded_details: &[&FlowRecord],
) -> Vec<(usize, &'a FlowSummary)> {
    let trimmed_search = search_term.trim();
    let trimmed_view_script = filters.view_filter_script.trim();
    let use_search_script = !trimmed_search.is_empty()
        && (filters.script_enabled || is_script_expression(trimmed_search));
    let lower_search = if !trimmed_search.is_empty() && !use_search_script {
        trimmed_search.to_lowercase()
    } else {
        String::new()
    };
    let mut rows: Vec<(usize, &'a FlowSummary)> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            if !matches_logger_source(source_filter, &entry.source) {
                return false;
            }
            if filters.in_scope_only && !scope.is_url_in_scope(&entry.url) {
                return false;
            }
            if !matches_status_classes(entry.status, &filters.status_classes) {
                return false;
            }
            if !trimmed_view_script.is_empty() || use_search_script {
                let notes = live_notes
                    .get(&entry.id)
                    .map(|(_, text)| text.as_str())
                    .unwrap_or("");
                let highlighted = live_highlights
                    .get(&entry.id)
                    .is_some_and(|highlight| (1..=9).contains(&highlight.color));
                let detail_record = loaded_details
                    .iter()
                    .copied()
                    .find(|record| record.summary.id == entry.id);
                let req_raw = detail_record.map(|record| request_to_raw(&record.request));
                let resp_raw = detail_record
                    .and_then(|record| record.response.as_ref().map(response_to_raw));
                if !trimmed_view_script.is_empty()
                    && !eval_logger_script_with_context(
                        entry,
                        trimmed_view_script,
                        req_raw.as_deref(),
                        resp_raw.as_deref(),
                        notes,
                        highlighted,
                    )
                {
                    return false;
                }
                if use_search_script {
                    return eval_logger_script_with_context(
                        entry,
                        trimmed_search,
                        req_raw.as_deref(),
                        resp_raw.as_deref(),
                        notes,
                        highlighted,
                    );
                }
            }
            if trimmed_search.is_empty() {
                return true;
            }
            entry.url.to_lowercase().contains(&lower_search)
                || entry.method.to_lowercase().contains(&lower_search)
                || logger_source_label(&entry.source)
                    .to_lowercase()
                    .contains(&lower_search)
                || entry
                    .status
                    .is_some_and(|code| code.to_string().contains(&lower_search))
                || entry_comment(entry.id, live_notes)
                    .to_lowercase()
                    .contains(&lower_search)
        })
        .map(|(idx, entry)| (idx + 1, entry))
        .collect();
    // Precompute expensive string/URL sort keys once per row (O(N)) instead of O(N log N) inside `sort_by`.
    if let Some((ext_col, ascending)) = filters.extended_sort {
        match ext_col {
            LoggerExtendedSortColumn::Host
            | LoggerExtendedSortColumn::Path
            | LoggerExtendedSortColumn::Query
            | LoggerExtendedSortColumn::Comment => {
                let mut keyed: Vec<(usize, &'a FlowSummary, String)> = rows
                    .into_iter()
                    .map(|(num, entry)| {
                        let key = match ext_col {
                            LoggerExtendedSortColumn::Host => parse_logger_url(&entry.url).host,
                            LoggerExtendedSortColumn::Path => parse_logger_url(&entry.url).path,
                            LoggerExtendedSortColumn::Query => parse_logger_url(&entry.url).query,
                            LoggerExtendedSortColumn::Comment => {
                                entry_comment(entry.id, live_notes)
                            }
                            _ => unreachable!(),
                        };
                        (num, entry, key)
                    })
                    .collect();
                keyed.sort_by(|(num_a, _, key_a), (num_b, _, key_b)| {
                    let ord = key_a.cmp(key_b).then_with(|| num_a.cmp(num_b));
                    if ascending { ord } else { ord.reverse() }
                });
                return keyed
                    .into_iter()
                    .map(|(num, entry, _)| (num, entry))
                    .collect();
            }
            LoggerExtendedSortColumn::ParamCount => {
                let mut keyed: Vec<(usize, &'a FlowSummary, usize)> = rows
                    .into_iter()
                    .map(|(num, entry)| (num, entry, entry_param_count(entry, &filters.param_counts)))
                    .collect();
                keyed.sort_by(|(num_a, _, key_a), (num_b, _, key_b)| {
                    let ord = key_a.cmp(key_b).then_with(|| num_a.cmp(num_b));
                    if ascending { ord } else { ord.reverse() }
                });
                return keyed
                    .into_iter()
                    .map(|(num, entry, _)| (num, entry))
                    .collect();
            }
            LoggerExtendedSortColumn::Time => {
                rows.sort_by(|(num_a, a), (num_b, b)| {
                    let ord = a
                        .started_unix_ms
                        .cmp(&b.started_unix_ms)
                        .then_with(|| num_a.cmp(num_b));
                    if ascending { ord } else { ord.reverse() }
                });
                return rows;
            }
            LoggerExtendedSortColumn::StartResponseTimer => {
                rows.sort_by(|(num_a, a), (num_b, b)| {
                    let ord = a.elapsed_ms.cmp(&b.elapsed_ms).then_with(|| num_a.cmp(num_b));
                    if ascending { ord } else { ord.reverse() }
                });
                return rows;
            }
        }
    }
    let ascending = sort.ascending;
    if sort.column == LoggerSortColumn::Source {
        let mut keyed: Vec<(usize, &'a FlowSummary, String)> = rows
            .into_iter()
            .map(|(num, entry)| (num, entry, logger_source_label(&entry.source)))
            .collect();
        keyed.sort_by(|(num_a, _, key_a), (num_b, _, key_b)| {
            let ord = key_a.cmp(key_b).then_with(|| num_a.cmp(num_b));
            if ascending { ord } else { ord.reverse() }
        });
        return keyed
            .into_iter()
            .map(|(num, entry, _)| (num, entry))
            .collect();
    }
    rows.sort_by(|(num_a, a), (num_b, b)| {
        let ord = match sort.column {
            LoggerSortColumn::Number => num_a.cmp(num_b),
            LoggerSortColumn::Source => unreachable!(),
            LoggerSortColumn::Method => a.method.cmp(&b.method).then_with(|| num_a.cmp(num_b)),
            LoggerSortColumn::Status => a
                .status
                .unwrap_or(0)
                .cmp(&b.status.unwrap_or(0))
                .then_with(|| num_a.cmp(num_b)),
            LoggerSortColumn::Length => a
                .response_bytes
                .cmp(&b.response_bytes)
                .then_with(|| num_a.cmp(num_b)),
        };
        if ascending { ord } else { ord.reverse() }
    });
    rows
}

fn csv_escape(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}

pub(in crate::workbench::tools_extra::logger) fn format_logger_csv(rows: &[(usize, &FlowSummary)]) -> String {
    let mut out = String::from("编号,来源,方法,URL,状态码,长度,耗时\n");
    for &(number, entry) in rows {
        let source = logger_source_label(&entry.source);
        let status = entry
            .status
            .map_or_else(|| entry.state.label().to_owned(), |code| code.to_string());
        let _ = writeln!(
            out,
            "{},{},{},{},{},{},{}",
            number,
            csv_escape(&source),
            csv_escape(&entry.method),
            csv_escape(&entry.url),
            csv_escape(&status),
            entry.response_bytes,
            entry.elapsed_ms,
        );
    }
    out
}
