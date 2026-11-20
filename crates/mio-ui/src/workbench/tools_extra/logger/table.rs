use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::{FontWeight, WeakEntity};
use mio_core::{
    AuditCommand, AuditFinding, AuditPhase, ExtHttpHighlight, ExtHttpHighlightChange, FlowId,
    FlowRecord, FlowSummary, HistoryEntryId, HistoryTransferEntry, HttpRequest, IssueConfidence,
    OrganiserItem, TargetScope, TrafficSource, Uuid, next_organiser_number,
    validate_organiser_items,
};
use tokio_util::sync::CancellationToken;

use super::*;

const LOGGER_HIGHLIGHT_NAMES: [&str; 10] = [
    "无", "红色", "橙色", "黄色", "绿色", "青色", "蓝色", "粉色", "品红", "灰色",
];

pub(super) const LOGGER_COL_COUNT: usize = 12;

pub(super) const LOGGER_DEFAULT_COL_WIDTHS: [f32; LOGGER_COL_COUNT] = [
    42., 148., 74., 62., 148., 148., 148., 102., 92., 62., 150., 140.,
];

pub(super) const LOGGER_COL_NAMES: [&str; LOGGER_COL_COUNT] = [
    "#",
    "Time",
    "Tool",
    "Method",
    "Host",
    "Path",
    "Query",
    "Param count",
    "Status code",
    "Length",
    "Start response timer",
    "Comment",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LoggerBatchKind {
    Organiser,
    Comparer { response: bool },
}

#[derive(Clone, Debug)]
pub(super) struct PendingLoggerBatchOp {
    pub(super) kind: LoggerBatchKind,
    pub(super) remaining: VecDeque<FlowId>,
    pub(super) collected: Vec<FlowRecord>,
}

#[derive(Clone)]
pub(super) struct LoggerUiTableState {
    pub(super) owner: Option<WeakEntity<Workbench>>,
    pub(super) selected_set: HashSet<FlowId>,
    pub(super) anchor_id: Option<FlowId>,
    pub(super) explicit_empty: Option<FlowId>,
    pub(super) col_widths: [f32; LOGGER_COL_COUNT],
    pub(super) col_visible: [bool; LOGGER_COL_COUNT],
    pub(super) resize_drag: Option<(usize, f32, f32)>,
    pub(super) pending_batch: Option<PendingLoggerBatchOp>,
}

impl Default for LoggerUiTableState {
    fn default() -> Self {
        Self {
            owner: None,
            selected_set: HashSet::new(),
            anchor_id: None,
            explicit_empty: None,
            col_widths: LOGGER_DEFAULT_COL_WIDTHS,
            col_visible: [true; LOGGER_COL_COUNT],
            resize_drag: None,
            pending_batch: None,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct ResizeLoggerCol {
    pub(super) entity_id: gpui_kit::EntityId,
    pub(super) col_ix: usize,
}

impl gpui_kit::Render for ResizeLoggerCol {
    fn render(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        gpui_kit::Empty
    }
}

std::thread_local! {
    static LOGGER_UI_TABLE_STATE: std::cell::RefCell<HashMap<gpui_kit::EntityId, LoggerUiTableState>> =
        std::cell::RefCell::new(HashMap::new());
}

pub(super) fn logger_visible_column_flags(workbench_id: gpui_kit::EntityId) -> [bool; LOGGER_COL_COUNT] {
    LOGGER_UI_TABLE_STATE.with(|map| {
        map.borrow()
            .get(&workbench_id)
            .map(|st| st.col_visible)
            .unwrap_or([true; LOGGER_COL_COUNT])
    })
}

pub(super) fn logger_ui_state_snapshot(
    workbench_id: gpui_kit::EntityId,
    owner: WeakEntity<Workbench>,
    primary_selected: Option<FlowId>,
    entries: &[FlowSummary],
) -> (HashSet<FlowId>, [f32; LOGGER_COL_COUNT], [bool; LOGGER_COL_COUNT]) {
    LOGGER_UI_TABLE_STATE.with(|map| {
        let mut map = map.borrow_mut();
        map.retain(|&id, st| {
            id == workbench_id || st.owner.as_ref().is_some_and(|w| w.upgrade().is_some())
        });
        let st = map.entry(workbench_id).or_default();
        st.owner = Some(owner);
        if primary_selected.is_none() || entries.is_empty() {
            st.selected_set.clear();
            st.anchor_id = None;
            st.explicit_empty = None;
        } else {
            st.selected_set
                .retain(|id| entries.iter().any(|e| e.id == *id));
            if let Some(sel) = primary_selected
                && st.selected_set.is_empty()
                && st.explicit_empty != Some(sel)
            {
                st.explicit_empty = None;
                st.selected_set.insert(sel);
            }
        }
        (st.selected_set.clone(), st.col_widths, st.col_visible)
    })
}

pub(super) fn logger_cell_text_for_entry(
    col_ix: usize,
    number: usize,
    entry: &FlowSummary,
    param_counts: &HashMap<FlowId, usize>,
    live_notes: &HashMap<FlowId, (u64, String)>,
) -> String {
    match col_ix {
        0 => number.to_string(),
        1 => crate::history::local_time(entry.started_unix_ms),
        2 => logger_source_label(&entry.source),
        3 => entry.method.clone(),
        4 => parse_logger_url(&entry.url).host,
        5 => parse_logger_url(&entry.url).path,
        6 => parse_logger_url(&entry.url).query,
        7 => entry_param_count(entry, param_counts).to_string(),
        8 => entry.status.map(|c| c.to_string()).unwrap_or_default(),
        9 => {
            if entry.status.is_some() || entry.response_bytes > 0 {
                entry.response_bytes.to_string()
            } else {
                String::new()
            }
        }
        10 => {
            if entry.status.is_some() || entry.elapsed_ms > 0 {
                entry.elapsed_ms.to_string()
            } else {
                String::new()
            }
        }
        11 => entry_comment(entry.id, live_notes),
        _ => String::new(),
    }
}

const MIO_ADD_TO_SITEMAP_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><circle cx="3.2" cy="11.2" r="1.3"/><path d="M3.2 9.9V3.2h5.1"/><path d="M3.2 7.2h5.1"/><rect x="8.3" y="1.9" width="3.8" height="2.6" rx="0.6"/><rect x="8.3" y="5.9" width="3.8" height="2.6" rx="0.6"/></svg>"#;

pub(super) struct ParsedLoggerUrl {
    pub(super) host: String,
    pub(super) path: String,
    pub(super) query: String,
}

pub(super) fn parse_logger_url(raw: &str) -> ParsedLoggerUrl {
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

pub(super) fn count_query_params(query: &str) -> usize {
    if query.is_empty() {
        return 0;
    }
    query.split('&').filter(|part| !part.is_empty()).count()
}

pub(super) fn count_url_query_params(raw_url: &str) -> usize {
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

pub(super) fn count_request_params(request: &HttpRequest) -> usize {
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

pub(super) fn entry_param_count(
    entry: &FlowSummary,
    param_counts: &HashMap<FlowId, usize>,
) -> usize {
    param_counts
        .get(&entry.id)
        .copied()
        .unwrap_or_else(|| count_url_query_params(&entry.url))
}

pub(super) fn entry_comment(
    flow_id: FlowId,
    live_notes: &HashMap<FlowId, (u64, String)>,
) -> String {
    live_notes
        .get(&flow_id)
        .map(|(_, text)| text.replace("\r\n", " ").replace(['\r', '\n', '\t'], " "))
        .unwrap_or_default()
}

pub(super) fn logger_source_index(source: &TrafficSource) -> usize {
    match source {
        TrafficSource::Proxy => 0,
        TrafficSource::Repeater => 1,
        TrafficSource::Intruder => 2,
        TrafficSource::Scanner => 3,
        TrafficSource::Plugin(_) => 4,
    }
}

pub(super) fn logger_source_label(source: &TrafficSource) -> String {
    match source {
        TrafficSource::Proxy => "Proxy".to_owned(),
        TrafficSource::Repeater => "Repeater".to_owned(),
        TrafficSource::Intruder => "Intruder".to_owned(),
        TrafficSource::Scanner => "Scanner".to_owned(),
        TrafficSource::Plugin(name) if name.is_empty() => "Extensions".to_owned(),
        TrafficSource::Plugin(name) => format!("Extensions({name})"),
    }
}

pub(super) fn capture_filter_summary(max_entries: usize, filters: &LoggerFilters) -> String {
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

pub(super) fn view_filter_summary(
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

pub(super) fn matches_logger_source(
    source_filter: &Option<TrafficSource>,
    source: &TrafficSource,
) -> bool {
    match source_filter {
        None => true,
        Some(TrafficSource::Plugin(_)) => matches!(source, TrafficSource::Plugin(_)),
        Some(expected) => source == expected,
    }
}

pub(super) fn is_script_expression(expr: &str) -> bool {
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

pub(super) fn validate_logger_script(expr: &str) -> Result<(), String> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let normalized = normalize_logger_filter_script(trimmed);
    mio_core::validate_filter_script(&normalized).map_err(|err| err.to_string())
}

pub(super) fn eval_logger_script(entry: &FlowSummary, expr: &str) -> bool {
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
        None,
        None,
        "",
        false,
        None,
    )
    .unwrap_or(false)
}

pub(super) fn matches_status_classes(status: Option<u16>, status_classes: &[bool; 4]) -> bool {
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

pub(super) fn matches_logger_capture_filter(
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

pub(super) fn filter_and_sort_logger<'a>(
    entries: &'a [FlowSummary],
    source_filter: &Option<TrafficSource>,
    search_term: &str,
    sort: LoggerSort,
    filters: &LoggerFilters,
    scope: &TargetScope,
    live_notes: &HashMap<FlowId, (u64, String)>,
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
            if !trimmed_view_script.is_empty() && !eval_logger_script(entry, trimmed_view_script) {
                return false;
            }
            if trimmed_search.is_empty() {
                return true;
            }
            if use_search_script {
                eval_logger_script(entry, trimmed_search)
            } else {
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
            }
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
            LoggerSortColumn::Url => a.url.cmp(&b.url).then_with(|| num_a.cmp(num_b)),
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

pub(super) fn format_logger_csv(rows: &[(usize, &FlowSummary)]) -> String {
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

fn strip_default_port(url: &str) -> String {
    let trimmed = url.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return trimmed.to_owned();
    };
    let default_port = if scheme.eq_ignore_ascii_case("http") {
        ":80"
    } else if scheme.eq_ignore_ascii_case("https") {
        ":443"
    } else {
        return trimmed.to_owned();
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    if let Some(stripped_authority) = authority.strip_suffix(default_port) {
        format!("{scheme}://{stripped_authority}{tail}")
    } else {
        trimmed.to_owned()
    }
}

fn truncate_menu_url(url: &str) -> String {
    let normalized = strip_default_port(url);
    let chars: Vec<char> = normalized.chars().collect();
    if chars.is_empty() {
        return "Logger item".into();
    }
    if chars.len() <= 58 {
        normalized
    } else {
        let prefix: String = chars[..27].iter().collect();
        let suffix: String = chars[chars.len() - 28..].iter().collect();
        format!("{prefix}...{suffix}")
    }
}

fn menu_shortcut_row(
    id: impl Into<gpui_kit::ElementId>,
    label: &'static str,
    shortcut: &'static str,
) -> impl gpui_kit::IntoElement {
    let key = gpui_kit::Keystroke::parse(shortcut).ok().map(|ks| {
        gpui_kit::component::kbd::Kbd::new(ks)
            .p_0()
            .flex_nowrap()
            .border_0()
            .bg(gpui_kit::transparent_black())
    });
    div()
        .id(id)
        .test_support()
        .role(gpui_kit::accesskit::Role::MenuItem)
        .aria_label(label)
        .w_full()
        .min_h(gpui_kit::px(21.))
        .flex()
        .items_center()
        .justify_between()
        .gap(gpui_kit::px(12.))
        .child(label)
        .children(key)
}

fn high_contrast_logger_highlight_fg(color: u8) -> gpui_kit::Hsla {
    gpui_kit::rgb(match color {
        1 | 6 | 8 => 0xffffff,
        _ => 0x111111,
    })
    .into()
}

impl Workbench {
    fn selected_logger_summary(&self) -> Option<FlowSummary> {
        let id = self.extra_tools.logger_selected?;
        self.extra_tools
            .logger_entries
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
    }

    pub(super) fn send_logger_selected_to_mio_at(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(flow_id) = self.extra_tools.logger_selected else {
            self.fail("请先选择一条 Logger 流量记录。", cx);
            return;
        };
        let summary = self.selected_logger_summary();
        let detail = self
            .extra_tools
            .logger_loaded_detail
            .as_ref()
            .filter(|(record, _)| record.summary.id == flow_id);
        let url = detail
            .map(|(record, _)| record.request.url.clone())
            .or_else(|| summary.as_ref().map(|s| s.url.clone()))
            .unwrap_or_default();
        let method = detail
            .map(|(record, _)| record.request.method.clone())
            .or_else(|| summary.as_ref().map(|s| s.method.clone()))
            .unwrap_or_else(|| "GET".into());
        let status = detail
            .and_then(|(record, _)| record.response.as_ref().map(|r| r.status))
            .or_else(|| summary.as_ref().and_then(|s| s.status));
        let req_raw = detail
            .map(|(record, _)| request_to_raw(&record.request))
            .or_else(|| self.extra_tools.logger_request.bytes(window, cx).ok())
            .unwrap_or_default();
        let resp_raw = detail
            .and_then(|(record, _)| record.response.as_ref().map(response_to_raw))
            .or_else(|| self.extra_tools.logger_response.bytes(window, cx).ok())
            .unwrap_or_default();
        let req_preview = String::from_utf8_lossy(&req_raw[..req_raw.len().min(4096)]);
        let resp_preview = String::from_utf8_lossy(&resp_raw[..resp_raw.len().min(4096)]);
        let ipc_path = mio_core::env_ipc_socket_path()
            .unwrap_or_else(mio_core::default_ipc_socket_path)
            .display()
            .to_string();
        let mut ctx_buf = format!(
            "Logger Flow: {flow_id}\nURL: {method} {url}\nStatus: {}\nCLI: mio-cli --socket \"{ipc_path}\" flows get {flow_id} --json",
            status.map_or_else(|| "-".into(), |c| c.to_string()),
        );
        if !req_preview.is_empty() {
            let _ = write!(ctx_buf, "\n\n--- Request ---\n{req_preview}");
        }
        if !resp_preview.is_empty() {
            let _ = write!(ctx_buf, "\n\n--- Response ---\n{resp_preview}");
        }
        let title = if url.is_empty() {
            format!("Logger #{flow_id}")
        } else {
            format!("{method} {url}")
        };
        self.open_mio_ai_with_context(title, ctx_buf, cx);
    }

    pub(super) fn record_manual_issue_from_logger(&mut self, cx: &mut Context<Self>) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let Some(flow_id) = self.extra_tools.logger_selected else {
            self.fail("请先选择一条 Logger 流量记录。", cx);
            return;
        };
        let url = self
            .extra_tools
            .logger_loaded_detail
            .as_ref()
            .filter(|(r, _)| r.summary.id == flow_id)
            .map(|(r, _)| r.request.url.clone())
            .or_else(|| self.selected_logger_summary().map(|s| s.url))
            .unwrap_or_default();
        if url.is_empty() {
            self.fail("所选 Logger 记录缺少有效的请求 URL。", cx);
            return;
        }
        let host = url::Url::parse(&url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_owned))
            .unwrap_or_default();
        let now_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let finding = AuditFinding {
            id: Uuid::new_v4(),
            issue_id: "0x00800100".into(),
            title: format!("自定义问题 · {url}"),
            severity_label: "信息".into(),
            severity_rank: 1,
            confidence: IssueConfidence::Certain,
            phase: AuditPhase::Passive,
            host,
            url: url.clone(),
            parameter: None,
            flow_id: Some(flow_id),
            summary: format!("从 Logger 流量记录创建的自定义问题（{url}）。"),
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

    pub(super) fn attach_logger_flow_to_issue(
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
        let Some(flow_id) = self.extra_tools.logger_selected else {
            self.fail("请先选择一条 Logger 流量记录。", cx);
            return;
        };
        let url = self
            .extra_tools
            .logger_loaded_detail
            .as_ref()
            .filter(|(r, _)| r.summary.id == flow_id)
            .map(|(r, _)| r.request.url.clone())
            .or_else(|| self.selected_logger_summary().map(|s| s.url))
            .unwrap_or_default();
        if url.is_empty() {
            self.fail("所选 Logger 记录缺少有效的请求 URL。", cx);
            return;
        }
        if self.enqueue(
            Command::Audit(AuditCommand::AttachFlowToFinding {
                project,
                finding_id,
                flow_id: Some(flow_id),
                url: url.clone(),
                evidence: format!("Logger 关联流量：{url}"),
            }),
            cx,
        ) {
            self.error = None;
            self.status = format!("已将当前 Logger 记录附加到现有问题：{url}");
            cx.notify();
        }
    }

    pub(super) fn find_logger_references(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(flow_id) = self.extra_tools.logger_selected else {
            self.fail("请先选择一条 Logger 流量记录。", cx);
            return;
        };
        let url = self
            .extra_tools
            .logger_loaded_detail
            .as_ref()
            .filter(|(r, _)| r.summary.id == flow_id)
            .map(|(r, _)| r.request.url.clone())
            .or_else(|| self.selected_logger_summary().map(|s| s.url))
            .unwrap_or_default();
        if url.is_empty() {
            self.fail("所选 Logger 记录缺少可检索的 URL。", cx);
            return;
        }
        let parsed = parse_logger_url(&url);
        let needle = if parsed.path != "/" && !parsed.path.is_empty() {
            parsed.path
        } else if !parsed.host.is_empty() {
            parsed.host
        } else {
            url
        };
        self.global_search
            .input
            .update(cx, |input, cx| input.set_value(needle, window, cx));
        self.open_global_search(window, cx);
        self.query_global_search(window, cx);
    }

    pub(super) fn generate_logger_csrf_poc(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(flow_id) = self.extra_tools.logger_selected else {
            self.fail("请先选择一条 Logger 流量记录。", cx);
            return;
        };
        let (url, raw_request) = if let Some((record, _)) = self
            .extra_tools
            .logger_loaded_detail
            .as_ref()
            .filter(|(r, _)| r.summary.id == flow_id)
        {
            (record.request.url.clone(), request_to_raw(&record.request))
        } else {
            let url = self
                .selected_logger_summary()
                .map(|s| s.url)
                .unwrap_or_default();
            let raw = match self.extra_tools.logger_request.bytes(window, cx) {
                Ok(bytes) if !bytes.is_empty() => bytes,
                Ok(_) => {
                    self.fail("当前 Logger 请求详情尚未读取完成，无法生成 CSRF PoC。", cx);
                    return;
                }
                Err(error) => {
                    self.fail(error, cx);
                    return;
                }
            };
            (url, raw)
        };
        if raw_request.is_empty() {
            self.fail("当前 Logger 请求正文为空，无法生成 CSRF PoC。", cx);
            return;
        }
        self.open_csrf_poc_modal(&url, &raw_request, cx);
    }

    pub(super) fn set_logger_flow_highlight(
        &mut self,
        flow_id: FlowId,
        color: u8,
        cx: &mut Context<Self>,
    ) {
        if color > 9 || self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let entry_id = HistoryEntryId::Live(flow_id);
        let in_history_cache = self
            .history
            .read(cx)
            .delegate()
            .row_by_id(entry_id)
            .is_some();
        if in_history_cache {
            self.set_http_highlights(&[entry_id], color, cx);
            return;
        }
        let prev = self
            .history
            .read(cx)
            .delegate()
            .live_highlights
            .borrow()
            .get(&flow_id)
            .cloned()
            .unwrap_or(ExtHttpHighlight {
                revision: 0,
                color: 0,
            });
        let next_rev = prev.revision.saturating_add(1);
        self.history.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.live_highlights.borrow_mut().insert(
                flow_id,
                ExtHttpHighlight {
                    revision: next_rev,
                    color,
                },
            );
            table.refresh(cx);
        });
        let _ = self.enqueue(
            Command::SetExtHttpHighlights {
                project,
                operation_id: Uuid::new_v4(),
                changes: vec![ExtHttpHighlightChange {
                    source_id: Uuid::nil(),
                    item_id: flow_id,
                    expected_highlight_revision: prev.revision,
                    color,
                }],
                cancel: CancellationToken::new(),
            },
            cx,
        );
        self.error = None;
        self.status = if color == 0 {
            "已清除 Logger 条目高亮。".into()
        } else {
            format!(
                "已将 Logger 条目标记为「{}」高亮。",
                LOGGER_HIGHLIGHT_NAMES[usize::from(color)]
            )
        };
        cx.notify();
    }

    pub(super) fn save_selected_logger_item_to_file(
        &mut self,
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
        let Some(flow_id) = self.extra_tools.logger_selected else {
            self.fail("请先选择一条 Logger 流量记录。", cx);
            return;
        };
        let Some((record, _)) = self
            .extra_tools
            .logger_loaded_detail
            .as_ref()
            .filter(|(r, _)| r.summary.id == flow_id)
            .cloned()
        else {
            self.fail("Logger 流量详情尚未读取完成，请稍后重试保存条目。", cx);
            return;
        };
        let comment = {
            let live_notes = self.history.read(cx).delegate().live_notes.borrow();
            entry_comment(flow_id, &live_notes)
        };
        let url = if !record.request.url.is_empty() {
            record.request.url.clone()
        } else {
            record.summary.url.clone()
        };
        let method = if !record.request.method.is_empty() {
            record.request.method.clone()
        } else {
            record.summary.method.clone()
        };
        let status = record
            .response
            .as_ref()
            .map(|r| r.status)
            .or(record.summary.status);
        let time_str = crate::history::local_time(record.summary.started_unix_ms);
        let req_bytes = request_to_raw(&record.request);
        let resp_bytes = record
            .response
            .as_ref()
            .map(response_to_raw)
            .unwrap_or_default();
        let xml_payload = match crate::workbench::wb_render_history::build_ext_items_xml(
            &url,
            &method,
            status,
            &time_str,
            &req_bytes,
            &resp_bytes,
            &comment,
        ) {
            Ok(xml) => xml.into_bytes(),
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let project = self.project;
        let picker = Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &self.default_directory(),
            Some("logger-item.xml"),
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
                        view.fail("项目已切换，本次条目导出未写入磁盘。", cx);
                        return;
                    }
                    match result {
                        Ok(Ok(Some(mut path))) => {
                            let is_har = path
                                .extension()
                                .and_then(|ext| ext.to_str())
                                .is_some_and(|ext| ext.eq_ignore_ascii_case("har"));
                            let bytes = if is_har {
                                match mio_core::export_flows_to_har(std::slice::from_ref(&record)) {
                                    Ok(har) => har,
                                    Err(err) => {
                                        view.fail(format!("导出 HAR 失败：{err}"), cx);
                                        return;
                                    }
                                }
                            } else {
                                if path.extension().is_none() {
                                    path.set_extension("xml");
                                }
                                xml_payload.clone()
                            };
                            match std::fs::write(&path, &bytes) {
                                Ok(()) => {
                                    view.error = None;
                                    view.status = format!(
                                        "已保存 Logger 条目到文件：{}",
                                        path.display()
                                    );
                                    cx.notify();
                                }
                                Err(err) => {
                                    view.fail(format!("写入导出文件失败：{err}"), cx);
                                }
                            }
                        }
                        Ok(Ok(None)) => {}
                        Ok(Err(error)) => view.fail(format!("无法选择保存路径：{error}"), cx),
                        Err(_) => view.fail("文件选择器已意外关闭。", cx),
                    }
                })
            });
        })
        .detach();
    }

    pub(super) fn handle_logger_row_click(
        &mut self,
        flow_id: FlowId,
        visible_ids: &[FlowId],
        shift: bool,
        toggle: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let wb_id = cx.entity_id();
        let weak = cx.weak_entity();
        let cur_selected = self.extra_tools.logger_selected;
        if shift {
            let anchor = LOGGER_UI_TABLE_STATE.with(|map| {
                let map_ref = map.borrow();
                map_ref.get(&wb_id).and_then(|st| st.anchor_id)
            });
            let anchor = anchor.or(cur_selected).unwrap_or(flow_id);
            let start_pos = visible_ids.iter().position(|id| *id == anchor);
            let end_pos = visible_ids.iter().position(|id| *id == flow_id);
            LOGGER_UI_TABLE_STATE.with(|map| {
                let mut map_mut = map.borrow_mut();
                let st = map_mut.entry(wb_id).or_default();
                st.owner = Some(weak);
                st.explicit_empty = None;
                if let (Some(s), Some(e)) = (start_pos, end_pos) {
                    let (lo, hi) = (s.min(e), s.max(e));
                    st.selected_set = visible_ids[lo..=hi].iter().copied().collect();
                } else {
                    st.selected_set.insert(flow_id);
                }
                if st.anchor_id.is_none() {
                    st.anchor_id = Some(anchor);
                }
            });
            self.select_logger_entry(flow_id, window, cx);
            return;
        }
        if toggle {
            let mut next_primary = Some(flow_id);
            let mut deselected_last = false;
            LOGGER_UI_TABLE_STATE.with(|map| {
                let mut map_mut = map.borrow_mut();
                let st = map_mut.entry(wb_id).or_default();
                st.owner = Some(weak);
                if st.selected_set.is_empty()
                    && st.explicit_empty != cur_selected
                    && let Some(cur) = cur_selected
                {
                    st.selected_set.insert(cur);
                }
                if st.selected_set.contains(&flow_id) {
                    st.selected_set.remove(&flow_id);
                    if st.selected_set.is_empty() {
                        st.anchor_id = None;
                        st.explicit_empty = None;
                        next_primary = None;
                        deselected_last = true;
                    } else if cur_selected == Some(flow_id) {
                        next_primary = visible_ids
                            .iter()
                            .copied()
                            .find(|id| st.selected_set.contains(id))
                            .or_else(|| st.selected_set.iter().copied().next());
                    } else {
                        next_primary = cur_selected;
                    }
                } else {
                    st.explicit_empty = None;
                    st.selected_set.insert(flow_id);
                    st.anchor_id = Some(flow_id);
                }
            });
            if deselected_last {
                if self.extra_tools.logger_selected.is_some() && !self.leave_http_notes(window, cx)
                {
                    return;
                }
                self.extra_tools.logger_selected = None;
                self.extra_tools.logger_pending_read = None;
                self.extra_tools.logger_loaded_detail = None;
                self.extra_tools.logger_request.load(b"", window, cx);
                self.extra_tools.logger_response.load(b"", window, cx);
                self.clear_context_viewer("logger", cx);
                if self.http_notes.open {
                    self.select_http_notes(window, cx);
                }
                cx.notify();
            } else if let Some(pid) = next_primary {
                self.select_logger_entry(pid, window, cx);
            } else {
                cx.notify();
            }
            return;
        }
        LOGGER_UI_TABLE_STATE.with(|map| {
            let mut map_mut = map.borrow_mut();
            let st = map_mut.entry(wb_id).or_default();
            st.owner = Some(weak);
            st.explicit_empty = None;
            st.selected_set.clear();
            st.selected_set.insert(flow_id);
            st.anchor_id = Some(flow_id);
        });
        self.select_logger_entry(flow_id, window, cx);
    }

    pub(super) fn handle_logger_row_right_down(
        &mut self,
        flow_id: FlowId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let wb_id = cx.entity_id();
        let weak = cx.weak_entity();
        let already_in_multi = LOGGER_UI_TABLE_STATE.with(|map| {
            let map_ref = map.borrow();
            map_ref
                .get(&wb_id)
                .is_some_and(|st| st.selected_set.contains(&flow_id) && st.selected_set.len() > 1)
        });
        if already_in_multi {
            cx.notify();
            return;
        }
        LOGGER_UI_TABLE_STATE.with(|map| {
            let mut map_mut = map.borrow_mut();
            let st = map_mut.entry(wb_id).or_default();
            st.owner = Some(weak);
            st.explicit_empty = None;
            st.selected_set.clear();
            st.selected_set.insert(flow_id);
            st.anchor_id = Some(flow_id);
        });
        if self.extra_tools.logger_selected != Some(flow_id) {
            self.select_logger_entry(flow_id, window, cx);
        }
    }

    pub(super) fn set_logger_col_sort(
        &mut self,
        col_ix: usize,
        ascending: bool,
        cx: &mut Context<Self>,
    ) {
        match col_ix {
            0 => {
                self.extra_tools.logger_filters.extended_sort = None;
                self.extra_tools.logger_sort = LoggerSort {
                    column: LoggerSortColumn::Number,
                    ascending,
                };
            }
            1 => {
                self.extra_tools.logger_filters.extended_sort =
                    Some((LoggerExtendedSortColumn::Time, ascending));
            }
            2 => {
                self.extra_tools.logger_filters.extended_sort = None;
                self.extra_tools.logger_sort = LoggerSort {
                    column: LoggerSortColumn::Source,
                    ascending,
                };
            }
            3 => {
                self.extra_tools.logger_filters.extended_sort = None;
                self.extra_tools.logger_sort = LoggerSort {
                    column: LoggerSortColumn::Method,
                    ascending,
                };
            }
            4 => {
                self.extra_tools.logger_filters.extended_sort =
                    Some((LoggerExtendedSortColumn::Host, ascending));
            }
            5 => {
                self.extra_tools.logger_filters.extended_sort =
                    Some((LoggerExtendedSortColumn::Path, ascending));
            }
            6 => {
                self.extra_tools.logger_filters.extended_sort =
                    Some((LoggerExtendedSortColumn::Query, ascending));
            }
            7 => {
                self.extra_tools.logger_filters.extended_sort =
                    Some((LoggerExtendedSortColumn::ParamCount, ascending));
            }
            8 => {
                self.extra_tools.logger_filters.extended_sort = None;
                self.extra_tools.logger_sort = LoggerSort {
                    column: LoggerSortColumn::Status,
                    ascending,
                };
            }
            9 => {
                self.extra_tools.logger_filters.extended_sort = None;
                self.extra_tools.logger_sort = LoggerSort {
                    column: LoggerSortColumn::Length,
                    ascending,
                };
            }
            10 => {
                self.extra_tools.logger_filters.extended_sort =
                    Some((LoggerExtendedSortColumn::StartResponseTimer, ascending));
            }
            11 => {
                self.extra_tools.logger_filters.extended_sort =
                    Some((LoggerExtendedSortColumn::Comment, ascending));
            }
            _ => return,
        }
        cx.notify();
    }

    pub(super) fn copy_logger_column(
        &mut self,
        col_ix: usize,
        rows: &[(usize, FlowId)],
        cx: &mut Context<Self>,
    ) {
        let entries = &self.extra_tools.logger_entries;
        let param_counts = &self.extra_tools.logger_filters.param_counts;
        let live_notes = self.history.read(cx).delegate().live_notes.borrow();
        let mut lines = Vec::with_capacity(rows.len());
        for &(number, flow_id) in rows {
            if let Some(entry) = entries
                .get(number.saturating_sub(1))
                .filter(|e| e.id == flow_id)
                .or_else(|| entries.iter().find(|e| e.id == flow_id))
            {
                lines.push(logger_cell_text_for_entry(
                    col_ix,
                    number,
                    entry,
                    param_counts,
                    &live_notes,
                ));
            }
        }
        drop(live_notes);
        let count = lines.len();
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(lines.join("\n")));
        self.error = None;
        self.status = format!(
            "已复制「{}」列（{count} 行）到剪贴板。",
            LOGGER_COL_NAMES.get(col_ix).copied().unwrap_or("Logger")
        );
        cx.notify();
    }

    pub(super) fn toggle_logger_column_visibility(
        &mut self,
        col_ix: usize,
        cx: &mut Context<Self>,
    ) {
        if col_ix == 0 || col_ix >= LOGGER_COL_COUNT {
            return;
        }
        let wb_id = cx.entity_id();
        let weak = cx.weak_entity();
        LOGGER_UI_TABLE_STATE.with(|map| {
            let mut map_mut = map.borrow_mut();
            let st = map_mut.entry(wb_id).or_default();
            st.owner = Some(weak);
            let visible_count = st.col_visible.iter().filter(|&&v| v).count();
            if st.col_visible[col_ix] {
                if visible_count > 1 {
                    st.col_visible[col_ix] = false;
                }
            } else {
                st.col_visible[col_ix] = true;
            }
        });
        cx.notify();
    }

    pub(super) fn fit_logger_column_width(
        &mut self,
        col_ix: usize,
        rows: &[(usize, FlowId)],
        cx: &mut Context<Self>,
    ) {
        if col_ix >= LOGGER_COL_COUNT {
            return;
        }
        let measure = |text: &str| -> f32 {
            text.chars()
                .map(|ch| if ch.is_ascii() { 7.2 } else { 13.5 })
                .sum()
        };
        let mut widest = measure(LOGGER_COL_NAMES[col_ix]) + 20.0;
        let entries = &self.extra_tools.logger_entries;
        let param_counts = &self.extra_tools.logger_filters.param_counts;
        let live_notes = self.history.read(cx).delegate().live_notes.borrow();
        for &(number, flow_id) in rows.iter().take(250) {
            if let Some(entry) = entries
                .get(number.saturating_sub(1))
                .filter(|e| e.id == flow_id)
                .or_else(|| entries.iter().find(|e| e.id == flow_id))
            {
                let text = logger_cell_text_for_entry(
                    col_ix,
                    number,
                    entry,
                    param_counts,
                    &live_notes,
                );
                let preview = theme::truncate_cell_preview(&text);
                widest = widest.max(measure(&preview));
            }
        }
        drop(live_notes);
        let fitted = (widest + 16.0).round().clamp(38.0, 520.0);
        let wb_id = cx.entity_id();
        let weak = cx.weak_entity();
        LOGGER_UI_TABLE_STATE.with(|map| {
            let mut map_mut = map.borrow_mut();
            let st = map_mut.entry(wb_id).or_default();
            st.owner = Some(weak);
            st.col_widths[col_ix] = fitted;
        });
        cx.notify();
    }

    pub(super) fn begin_logger_column_resize(
        &mut self,
        col_ix: usize,
        start_x: f32,
        cx: &mut Context<Self>,
    ) {
        if col_ix >= LOGGER_COL_COUNT {
            return;
        }
        let wb_id = cx.entity_id();
        let weak = cx.weak_entity();
        LOGGER_UI_TABLE_STATE.with(|map| {
            let mut map_mut = map.borrow_mut();
            let st = map_mut.entry(wb_id).or_default();
            st.owner = Some(weak);
            let start_w = st.col_widths[col_ix];
            st.resize_drag = Some((col_ix, start_x, start_w));
        });
    }

    pub(super) fn update_logger_column_resize(
        &mut self,
        col_ix: usize,
        current_x: f32,
        cx: &mut Context<Self>,
    ) {
        if col_ix >= LOGGER_COL_COUNT {
            return;
        }
        let wb_id = cx.entity_id();
        let scale = theme::ui_scale().max(0.75);
        let mut changed = false;
        LOGGER_UI_TABLE_STATE.with(|map| {
            let mut map_mut = map.borrow_mut();
            let st = map_mut.entry(wb_id).or_default();
            if let Some((drag_col, start_x, start_w)) = st.resize_drag
                && drag_col == col_ix
            {
                let delta_unscaled = (current_x - start_x) / scale;
                let next_w = (start_w + delta_unscaled).round().clamp(38.0, 520.0);
                if (st.col_widths[col_ix] - next_w).abs() >= 0.5 {
                    st.col_widths[col_ix] = next_w;
                    changed = true;
                }
            }
        });
        if changed {
            cx.notify();
        }
    }

    pub(super) fn adjust_logger_column_width(
        &mut self,
        col_ix: usize,
        delta: f32,
        cx: &mut Context<Self>,
    ) {
        if col_ix >= LOGGER_COL_COUNT {
            return;
        }
        let wb_id = cx.entity_id();
        let weak = cx.weak_entity();
        LOGGER_UI_TABLE_STATE.with(|map| {
            let mut map_mut = map.borrow_mut();
            let st = map_mut.entry(wb_id).or_default();
            st.owner = Some(weak);
            st.col_widths[col_ix] = (st.col_widths[col_ix] + delta).clamp(38.0, 520.0);
        });
        cx.notify();
    }

    pub(super) fn reset_logger_column_layout(&mut self, cx: &mut Context<Self>) {
        let wb_id = cx.entity_id();
        let weak = cx.weak_entity();
        LOGGER_UI_TABLE_STATE.with(|map| {
            let mut map_mut = map.borrow_mut();
            let st = map_mut.entry(wb_id).or_default();
            st.owner = Some(weak);
            st.col_widths = LOGGER_DEFAULT_COL_WIDTHS;
            st.col_visible = [true; LOGGER_COL_COUNT];
            st.resize_drag = None;
        });
        cx.notify();
    }

    pub(in crate::workbench) fn clear_logger_pending_batch(&mut self, cx: &Context<Self>) {
        let wb_id = cx.entity_id();
        LOGGER_UI_TABLE_STATE.with(|map| {
            if let Some(st) = map.borrow_mut().get_mut(&wb_id) {
                st.pending_batch = None;
            }
        });
    }

    pub(in crate::workbench) fn clear_logger_selection_state(&mut self, cx: &Context<Self>) {
        let wb_id = cx.entity_id();
        LOGGER_UI_TABLE_STATE.with(|map| {
            if let Some(st) = map.borrow_mut().get_mut(&wb_id) {
                st.selected_set.clear();
                st.anchor_id = None;
                st.explicit_empty = None;
                st.pending_batch = None;
            }
        });
    }

    pub(super) fn logger_selected_target_ids(
        &self,
        workbench_id: gpui_kit::EntityId,
        fallback_id: FlowId,
    ) -> Vec<FlowId> {
        let selected_set = LOGGER_UI_TABLE_STATE.with(|map| {
            map.borrow()
                .get(&workbench_id)
                .map(|st| st.selected_set.clone())
                .unwrap_or_default()
        });
        if selected_set.contains(&fallback_id) && selected_set.len() > 1 {
            let ordered: Vec<FlowId> = self
                .extra_tools
                .logger_entries
                .iter()
                .map(|e| e.id)
                .filter(|id| selected_set.contains(id))
                .collect();
            if !ordered.is_empty() {
                return ordered;
            }
        }
        vec![fallback_id]
    }

    fn logger_live_transfer_entries(&self, ids: &[FlowId]) -> Vec<HistoryTransferEntry> {
        let id_set: HashSet<FlowId> = ids.iter().copied().collect();
        self.extra_tools
            .logger_entries
            .iter()
            .filter(|entry| id_set.contains(&entry.id) && entry.revision > 0)
            .map(|entry| HistoryTransferEntry::Live {
                flow_id: entry.id,
                revision: entry.revision,
                original: false,
            })
            .collect()
    }

    pub(super) fn send_logger_batch_to_repeater(
        &mut self,
        ids: &[FlowId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.len() == 1
            && self.extra_tools.logger_selected == Some(ids[0])
            && self
                .extra_tools
                .logger_loaded_detail
                .as_ref()
                .is_some_and(|(r, _)| r.summary.id == ids[0])
        {
            self.copy_draft_to_repeater(window, cx);
            return;
        }
        let entries = self.logger_live_transfer_entries(ids);
        if entries.is_empty() {
            self.fail("所选 Logger 记录没有可转送的已落盘流量。", cx);
            return;
        }
        self.begin_history_transfer(None, entries, false, window, cx);
    }

    pub(super) fn send_logger_batch_to_intruder(
        &mut self,
        ids: &[FlowId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.len() == 1
            && self.extra_tools.logger_selected == Some(ids[0])
            && self
                .extra_tools
                .logger_loaded_detail
                .as_ref()
                .is_some_and(|(r, _)| r.summary.id == ids[0])
        {
            self.open_in_intruder(window, cx);
            return;
        }
        let entries = self.logger_live_transfer_entries(ids);
        if entries.is_empty() {
            self.fail("所选 Logger 记录没有可转送的已落盘流量。", cx);
            return;
        }
        self.begin_history_transfer(None, entries, true, window, cx);
    }

    fn cached_logger_flow_record(&self, flow_id: FlowId) -> Option<FlowRecord> {
        self.extra_tools
            .logger_loaded_detail
            .as_ref()
            .filter(|(rec, _)| rec.summary.id == flow_id)
            .map(|(rec, _)| rec.clone())
            .or_else(|| {
                self.detail
                    .as_deref()
                    .filter(|rec| rec.summary.id == flow_id)
                    .cloned()
            })
    }

    pub(in crate::workbench) fn advance_logger_batch_op(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.project.is_none() || !self.backend_available() {
            self.clear_logger_pending_batch(cx);
            return;
        }
        let wb_id = cx.entity_id();
        let mut next_to_read = None;
        let mut finished = None;
        loop {
            let front = LOGGER_UI_TABLE_STATE.with(|map| {
                map.borrow()
                    .get(&wb_id)
                    .and_then(|st| st.pending_batch.as_ref())
                    .and_then(|b| b.remaining.front().copied())
            });
            let Some(front_id) = front else {
                finished = LOGGER_UI_TABLE_STATE.with(|map| {
                    map.borrow_mut()
                        .get_mut(&wb_id)
                        .and_then(|st| st.pending_batch.take())
                });
                break;
            };
            if let Some(record) = self.cached_logger_flow_record(front_id) {
                LOGGER_UI_TABLE_STATE.with(|map| {
                    if let Some(st) = map.borrow_mut().get_mut(&wb_id)
                        && let Some(batch) = st.pending_batch.as_mut()
                        && batch.remaining.front() == Some(&front_id)
                    {
                        batch.collected.push(record);
                        batch.remaining.pop_front();
                    }
                });
            } else {
                next_to_read = Some(front_id);
                break;
            }
        }
        if let Some(target_id) = next_to_read {
            self.select_logger_entry(target_id, window, cx);
            return;
        }
        let Some(done) = finished else {
            return;
        };
        if done.collected.is_empty() {
            self.fail("所选 Logger 记录没有可用的流量详情。", cx);
            return;
        }
        match done.kind {
            LoggerBatchKind::Organiser => {
                if !self.organiser_editable() {
                    return;
                }
                if let Err(err) = self.validate_organiser_composition(window, cx) {
                    self.fail(err, cx);
                    return;
                }
                if !self.flush_workspace(window, cx) {
                    return;
                }
                let mut added = 0usize;
                let mut first_id = None;
                for record in &done.collected {
                    let Ok(number) = next_organiser_number(&self.workspace.organiser_items) else {
                        break;
                    };
                    let Ok(item) = OrganiserItem::from_flow_record(number, "Logger", record) else {
                        continue;
                    };
                    let item_id = item.id;
                    self.workspace.organiser_items.push(item);
                    if validate_organiser_items(&self.workspace.organiser_items)
                        .and_then(|()| self.workspace.validate_structure())
                        .is_err()
                    {
                        self.workspace.organiser_items.pop();
                        break;
                    }
                    first_id.get_or_insert(item_id);
                    added += 1;
                }
                if added == 0 {
                    self.fail("未能将所选 Logger 记录添加到 Organiser。", cx);
                    return;
                }
                let had_selection = self.organiser.selected_id.is_some();
                if !had_selection {
                    self.organiser.selected_id = first_id;
                }
                self.sync_organiser_table(window, cx);
                if !had_selection {
                    self.reload_selected_organiser_detail(window, cx);
                }
                self.save_workspace(cx);
                self.error = None;
                self.status = format!("已批量添加 {added} 条 Logger 记录到 Organiser。");
                cx.notify();
            }
            LoggerBatchKind::Comparer { response } => {
                const MAX_COMPARER_ITEMS: usize = 32;
                const MAX_COMPARER_ITEM_BYTES: usize = 256 * 1024;
                let mut added = 0usize;
                for record in &done.collected {
                    let bytes = if response {
                        record
                            .response
                            .as_ref()
                            .map(response_to_raw)
                            .unwrap_or_default()
                    } else {
                        request_to_raw(&record.request)
                    };
                    if bytes.is_empty() || bytes.len() > MAX_COMPARER_ITEM_BYTES {
                        continue;
                    }
                    if self.extra_tools.comparer_items.len() >= MAX_COMPARER_ITEMS {
                        break;
                    }
                    let id = self
                        .extra_tools
                        .comparer_items
                        .last()
                        .map_or(1, |item| item.id + 1);
                    let kind = if response { "响应" } else { "请求" };
                    self.extra_tools
                        .comparer_items
                        .push(crate::workbench::tools_extra::ComparerItem {
                            id,
                            label: format!("{id}: Logger {kind} ({} bytes)", bytes.len()),
                            bytes,
                        });
                    added += 1;
                }
                if added == 0 {
                    self.fail("所选 Logger 记录没有可发送到 Comparer 的有效报文。", cx);
                    return;
                }
                let count = self.extra_tools.comparer_items.len();
                self.extra_tools.comparer_selected = count - 1;
                if count >= 2 {
                    self.extra_tools.left_index = count - 2;
                    self.extra_tools.right_index = count - 1;
                }
                self.refresh_comparer(window, cx);
                self.cancel_plugin_interactions(cx);
                self.tool = Tool::Comparer;
                self.focus.focus(window, cx);
                self.error = None;
                self.status = format!("已批量发送 {added} 条 Logger 报文到 Comparer。");
                cx.notify();
            }
        }
    }

    fn start_logger_batch_op(
        &mut self,
        ids: &[FlowId],
        kind: LoggerBatchKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty()
            || self.transition.is_some()
            || self.project.is_none()
            || !self.backend_available()
        {
            return;
        }
        let wb_id = cx.entity_id();
        let weak = cx.weak_entity();
        LOGGER_UI_TABLE_STATE.with(|map| {
            let mut map_mut = map.borrow_mut();
            let st = map_mut.entry(wb_id).or_default();
            st.owner = Some(weak);
            st.pending_batch = Some(PendingLoggerBatchOp {
                kind,
                remaining: ids.iter().copied().collect(),
                collected: Vec::with_capacity(ids.len()),
            });
        });
        self.advance_logger_batch_op(window, cx);
    }

    pub(super) fn send_logger_batch_to_organiser(
        &mut self,
        ids: &[FlowId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.len() <= 1 {
            self.send_selected_to_organiser(window, cx);
            return;
        }
        self.start_logger_batch_op(ids, LoggerBatchKind::Organiser, window, cx);
    }

    pub(super) fn send_logger_batch_to_comparer(
        &mut self,
        ids: &[FlowId],
        response: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.len() <= 1 {
            self.send_to_comparer(response, window, cx);
            return;
        }
        self.start_logger_batch_op(ids, LoggerBatchKind::Comparer { response }, window, cx);
    }

    pub(super) fn copy_logger_batch_urls(
        &mut self,
        ids: &[FlowId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.len() <= 1 {
            self.copy_url(window, cx);
            return;
        }
        let id_set: HashSet<FlowId> = ids.iter().copied().collect();
        let urls: Vec<String> = self
            .extra_tools
            .logger_entries
            .iter()
            .filter(|e| id_set.contains(&e.id) && !e.url.is_empty())
            .map(|e| e.url.clone())
            .collect();
        if urls.is_empty() {
            self.fail("所选 Logger 记录没有可复制的 URL。", cx);
            return;
        }
        let count = urls.len();
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(urls.join("\n")));
        self.error = None;
        self.status = format!("已复制 {count} 条 Logger URL 到剪贴板。");
        cx.notify();
    }

    pub(super) fn export_logger_selected_csv(&mut self, ids: &[FlowId], cx: &mut Context<Self>) {
        let id_set: HashSet<FlowId> = ids.iter().copied().collect();
        let rows: Vec<(usize, &FlowSummary)> = self
            .extra_tools
            .logger_entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| id_set.contains(&entry.id))
            .map(|(idx, entry)| (idx + 1, entry))
            .collect();
        if rows.is_empty() {
            self.fail("没有选中的 Logger 记录可导出。", cx);
            return;
        }
        let count = rows.len();
        let csv = format_logger_csv(&rows);
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(csv));
        self.error = None;
        self.status = format!("已复制选中的 {count} 条 Logger 记录 (CSV) 到剪贴板。");
        cx.notify();
    }

    pub(super) fn set_logger_batch_highlight(
        &mut self,
        ids: &[FlowId],
        color: u8,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() || color > 9 || self.transition.is_some() || !self.backend_available() {
            return;
        }
        if ids.len() == 1 {
            self.set_logger_flow_highlight(ids[0], color, cx);
            return;
        }
        let Some(project) = self.project else {
            return;
        };
        let mut changes = Vec::with_capacity(ids.len());
        self.history.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            let mut live_hl = delegate.live_highlights.borrow_mut();
            for &flow_id in ids {
                let prev = live_hl
                    .get(&flow_id)
                    .cloned()
                    .unwrap_or(ExtHttpHighlight {
                        revision: 0,
                        color: 0,
                    });
                let next_rev = prev.revision.saturating_add(1);
                live_hl.insert(
                    flow_id,
                    ExtHttpHighlight {
                        revision: next_rev,
                        color,
                    },
                );
                changes.push(ExtHttpHighlightChange {
                    source_id: Uuid::nil(),
                    item_id: flow_id,
                    expected_highlight_revision: prev.revision,
                    color,
                });
            }
            drop(live_hl);
            table.refresh(cx);
        });
        let count = changes.len();
        let _ = self.enqueue(
            Command::SetExtHttpHighlights {
                project,
                operation_id: Uuid::new_v4(),
                changes,
                cancel: CancellationToken::new(),
            },
            cx,
        );
        self.error = None;
        self.status = if color == 0 {
            format!("已批量清除 {count} 条 Logger 条目高亮。")
        } else {
            format!(
                "已批量将 {count} 条 Logger 条目标记为「{}」高亮。",
                LOGGER_HIGHLIGHT_NAMES[usize::from(color)]
            )
        };
        cx.notify();
    }

    pub(super) fn delete_logger_selected_entries(
        &mut self,
        ids: &[FlowId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() {
            return;
        }
        let remove_set: HashSet<FlowId> = ids.iter().copied().collect();
        let cur_selected = self.extra_tools.logger_selected;
        let selected_removed = cur_selected.is_some_and(|id| remove_set.contains(&id));
        if selected_removed && !self.leave_http_notes(window, cx) {
            return;
        }
        let next_selected = if selected_removed {
            let cur_pos = cur_selected
                .and_then(|sel| {
                    self.extra_tools
                        .logger_entries
                        .iter()
                        .position(|e| e.id == sel)
                })
                .unwrap_or(0);
            self.extra_tools.logger_entries[cur_pos..]
                .iter()
                .find(|e| !remove_set.contains(&e.id))
                .or_else(|| {
                    self.extra_tools.logger_entries[..cur_pos]
                        .iter()
                        .rev()
                        .find(|e| !remove_set.contains(&e.id))
                })
                .map(|e| e.id)
        } else {
            cur_selected
        };
        let before = self.extra_tools.logger_entries.len();
        self.extra_tools
            .logger_entries
            .retain(|entry| !remove_set.contains(&entry.id));
        let removed = before.saturating_sub(self.extra_tools.logger_entries.len());
        for id in &remove_set {
            self.extra_tools.logger_filters.param_counts.remove(id);
        }
        let wb_id = cx.entity_id();
        LOGGER_UI_TABLE_STATE.with(|map| {
            if let Some(st) = map.borrow_mut().get_mut(&wb_id) {
                st.selected_set.retain(|id| !remove_set.contains(id));
                if st.anchor_id.is_some_and(|id| remove_set.contains(&id)) {
                    st.anchor_id = next_selected;
                }
                st.pending_batch = None;
            }
        });
        if selected_removed {
            if let Some(next_id) = next_selected {
                self.select_logger_entry(next_id, window, cx);
            } else {
                self.extra_tools.logger_selected = None;
                self.extra_tools.logger_pending_read = None;
                self.extra_tools.logger_loaded_detail = None;
                self.extra_tools.logger_request.load(b"", window, cx);
                self.extra_tools.logger_response.load(b"", window, cx);
                self.clear_context_viewer("logger", cx);
                if self.http_notes.open {
                    self.select_http_notes(window, cx);
                }
            }
        }
        if removed > 0 {
            self.error = None;
            self.status = format!("已从 Logger 视图移除 {removed} 条记录。");
        }
        cx.notify();
    }
}

pub(super) fn build_logger_header_context_menu(
    view: WeakEntity<Workbench>,
    col_ix: usize,
    rows_rc: Rc<Vec<(usize, FlowId)>>,
    col_visible: [bool; LOGGER_COL_COUNT],
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let visible_count = col_visible.iter().filter(|&&v| v).count();
    let can_hide = col_ix != 0 && col_ix < LOGGER_COL_COUNT && visible_count > 1;
    let has_rows = !rows_rc.is_empty();

    let v_asc = view.clone();
    let v_desc = view.clone();
    let v_copy = view.clone();
    let v_hide = view.clone();
    let v_fit = view.clone();
    let v_widen = view.clone();
    let v_narrow = view.clone();
    let v_cols = view.clone();
    let v_reset = view;

    let copy_rows = Rc::clone(&rows_rc);
    let fit_rows = rows_rc;

    let cols_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        (0..LOGGER_COL_COUNT).fold(sub.compact().min_w(px(180.)), |sub, idx| {
            let owner = v_cols.clone();
            let is_vis = col_visible[idx];
            let disabled = idx == 0 || (is_vis && visible_count <= 1);
            sub.item(
                PopupMenuItem::new(LOGGER_COL_NAMES[idx])
                    .checked(is_vis)
                    .disabled(disabled)
                    .on_click(move |_, _, cx| {
                        let _ = owner.update(cx, |wb, cx| {
                            wb.toggle_logger_column_visibility(idx, cx);
                        });
                    }),
            )
        })
    });

    menu.compact()
        .item(
            PopupMenuItem::new("按此列升序排序 (Sort ascending)").on_click(move |_, _, cx| {
                let _ = v_asc.update(cx, |wb, cx| wb.set_logger_col_sort(col_ix, true, cx));
            }),
        )
        .item(
            PopupMenuItem::new("按此列降序排序 (Sort descending)").on_click(move |_, _, cx| {
                let _ = v_desc.update(cx, |wb, cx| wb.set_logger_col_sort(col_ix, false, cx));
            }),
        )
        .separator()
        .item(
            PopupMenuItem::new("复制整列内容 (Copy column)")
                .disabled(!has_rows)
                .on_click(move |_, _, cx| {
                    let _ = v_copy.update(cx, |wb, cx| {
                        wb.copy_logger_column(col_ix, &copy_rows, cx);
                    });
                }),
        )
        .item(
            PopupMenuItem::new("隐藏此列 (Hide column)")
                .disabled(!can_hide)
                .on_click(move |_, _, cx| {
                    let _ = v_hide.update(cx, |wb, cx| {
                        wb.toggle_logger_column_visibility(col_ix, cx);
                    });
                }),
        )
        .item(
            PopupMenuItem::new("自适应列宽 (Auto-fit column width)").on_click(move |_, _, cx| {
                let _ = v_fit.update(cx, |wb, cx| {
                    wb.fit_logger_column_width(col_ix, &fit_rows, cx);
                });
            }),
        )
        .item(
            PopupMenuItem::new("加宽列 (+24px)").on_click(move |_, _, cx| {
                let _ = v_widen.update(cx, |wb, cx| {
                    wb.adjust_logger_column_width(col_ix, 24.0, cx);
                });
            }),
        )
        .item(
            PopupMenuItem::new("缩窄列 (-24px)").on_click(move |_, _, cx| {
                let _ = v_narrow.update(cx, |wb, cx| {
                    wb.adjust_logger_column_width(col_ix, -24.0, cx);
                });
            }),
        )
        .separator()
        .item(PopupMenuItem::submenu("显示/隐藏列 (Columns)", cols_submenu))
        .item(
            PopupMenuItem::new("重置列布局 (Reset column layout)").on_click(move |_, _, cx| {
                let _ = v_reset.update(cx, |wb, cx| wb.reset_logger_column_layout(cx));
            }),
        )
}

pub(super) fn logger_row_menu(
    view: WeakEntity<Workbench>,
    flow_id: FlowId,
    row_url: String,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(entity) = view.upgrade() else {
        return menu;
    };
    let (available, can_export_har, existing_findings, current_highlight, in_scope, target_ids) = {
        let workbench = entity.read(cx);
        let available = workbench.project.is_some()
            && workbench.backend_available()
            && workbench.transition.is_none();
        let can_export_har = available
            && workbench.extra_tools.logger_selected.is_some_and(|sel_id| {
                workbench
                    .extra_tools
                    .logger_loaded_detail
                    .as_ref()
                    .is_some_and(|(record, _)| record.summary.id == sel_id)
            });
        let findings = workbench.existing_audit_finding_ids();
        let hl = workbench
            .history
            .read(cx)
            .delegate()
            .live_highlights
            .borrow()
            .get(&flow_id)
            .map(|h| h.color)
            .filter(|c| *c <= 9)
            .unwrap_or(0);
        let in_scope = workbench.workspace.scope.has_enabled_includes()
            && workbench.workspace.scope.is_url_in_scope(&row_url);
        let target_ids = workbench.logger_selected_target_ids(entity.entity_id(), flow_id);
        (available, can_export_har, findings, hl, in_scope, target_ids)
    };

    let multi = target_ids.len() > 1;
    let header_url = if multi {
        format!("已选中 {} 条日志记录", target_ids.len())
    } else {
        truncate_menu_url(&row_url)
    };
    let mod_key = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };

    let attach_owner = view.clone();
    let existing_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        if existing_findings.is_empty() {
            sub.item(PopupMenuItem::new("暂无现有问题").disabled(true))
        } else {
            existing_findings
                .into_iter()
                .fold(sub, |sub, (fid, title)| {
                    let owner = attach_owner.clone();
                    sub.item(
                        PopupMenuItem::new(title)
                            .disabled(!available)
                            .on_click(move |_, _, cx| {
                                let _ = owner.update(cx, |v, cx| {
                                    v.attach_logger_flow_to_issue(fid, cx);
                                });
                            }),
                    )
                })
        }
    });

    let issue_owner = view.clone();
    let record_issue_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        let o_new = issue_owner.clone();
        sub.item(
            PopupMenuItem::new("创建新问题")
                .disabled(!available)
                .on_click(move |_, _, cx| {
                    let _ = o_new.update(cx, |v, cx| v.record_manual_issue_from_logger(cx));
                }),
        )
        .item(PopupMenuItem::submenu("添加到现有问题", existing_submenu))
    });

    let req_browser_owner = view.clone();
    let req_browser_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        let o_orig = req_browser_owner.clone();
        let o_curr = req_browser_owner.clone();
        sub.item(
            PopupMenuItem::new("使用原始会话")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ = o_orig.update(cx, |v, cx| {
                        v.open_current_request_in_browser_with_session(true, window, cx);
                    });
                }),
        )
        .item(
            PopupMenuItem::new("使用当前会话")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ = o_curr.update(cx, |v, cx| {
                        v.open_current_request_in_browser_with_session(false, window, cx);
                    });
                }),
        )
    });

    let engage_owner = view.clone();
    let engagement_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        let o_refs = engage_owner.clone();
        let o_csrf = engage_owner.clone();
        let o_har = engage_owner.clone();
        sub.item(
            PopupMenuItem::new("查找引用")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ = o_refs.update(cx, |v, cx| v.find_logger_references(window, cx));
                }),
        )
        .item(
            PopupMenuItem::new("生成 CSRF PoC")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ = o_csrf.update(cx, |v, cx| v.generate_logger_csrf_poc(window, cx));
                }),
        )
        .separator()
        .item(
            PopupMenuItem::new("复制为 HAR (JSON)")
                .disabled(!can_export_har)
                .on_click(move |_, window, cx| {
                    let _ = o_har.update(cx, |v, cx| v.copy_selected_flow_har(window, cx));
                }),
        )
    });

    let hl_owner = view.clone();
    let hl_ids = target_ids.clone();
    let highlight_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        (0u8..=9).fold(sub.min_w(theme::scaled_px(116.)), |sub, color| {
            let owner = hl_owner.clone();
            let ids = hl_ids.clone();
            let name = LOGGER_HIGHLIGHT_NAMES[usize::from(color)];
            let checked = current_highlight == color;
            sub.item(
                PopupMenuItem::element(move |_, _| {
                    div()
                        .id(("logger-highlight-color", usize::from(color)))
                        .test_support()
                        .role(gpui_kit::accesskit::Role::MenuItem)
                        .aria_label(format!("高亮 {name}"))
                        .w_full()
                        .min_h(theme::scaled_px(24.))
                        .mx_neg_2()
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .when_some(
                            crate::history::highlight_background(color),
                            |this, bg| this.bg(bg),
                        )
                        .text_color(if color == 0 {
                            theme::foreground()
                        } else {
                            high_contrast_logger_highlight_fg(color)
                        })
                        .child(
                            div()
                                .w(px(10.))
                                .h(px(10.))
                                .when(color == 0, |this| {
                                    this.border_1()
                                        .border_dashed()
                                        .border_color(rgb(0x888888))
                                })
                                .when_some(
                                    crate::history::highlight_background(color),
                                    |this, swatch| {
                                        this.bg(swatch)
                                            .border_1()
                                            .border_color(rgb(0x444444))
                                    },
                                ),
                        )
                        .child(if checked {
                            format!("{name} ✓")
                        } else {
                            name.to_owned()
                        })
                })
                .disabled(!available)
                .on_click(move |_, _, cx| {
                    let _ = owner.update(cx, |v, cx| {
                        v.set_logger_batch_highlight(&ids, color, cx);
                    });
                }),
            )
        })
    });

    let add_item =
        |menu: PopupMenu,
         label: &'static str,
         icon: Option<IconName>,
         action: fn(&mut Workbench, &mut Window, &mut Context<Workbench>)| {
            let owner = view.clone();
            let mut item = PopupMenuItem::new(label)
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ = owner.update(cx, |view, cx| action(view, window, cx));
                });
            if let Some(ic) = icon {
                item = item.icon(ic);
            }
            menu.item(item)
        };

    let o_intruder = view.clone();
    let o_repeater = view.clone();
    let o_cmp_req = view.clone();
    let o_cmp_resp = view.clone();
    let o_org = view.clone();
    let o_ai = view.clone();
    let o_del = view.clone();
    let o_sel_csv = view.clone();
    let o_urls = view.clone();

    let ids_intruder = target_ids.clone();
    let ids_repeater = target_ids.clone();
    let ids_cmp_req = target_ids.clone();
    let ids_cmp_resp = target_ids.clone();
    let ids_org = target_ids.clone();
    let ids_del = target_ids.clone();
    let ids_sel_csv = target_ids.clone();
    let ids_urls = target_ids;

    let menu = menu
        .compact()
        .scrollable(true)
        .max_h(theme::scaled_px(620.))
        .item(
            PopupMenuItem::element(move |_, _| {
                div()
                    .id("logger-row-url-header")
                    .test_support()
                    .role(gpui_kit::accesskit::Role::MenuItem)
                    .aria_label(header_url.clone())
                    .w_full()
                    .min_h(gpui_kit::px(21.))
                    .flex()
                    .items_center()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::foreground())
                    .child(header_url.clone())
            })
            .disabled(true),
        )
        .separator();
    let menu = if in_scope {
        add_item(
            menu,
            "从范围排除",
            Some(IconName::CircleMinus),
            Workbench::exclude_from_scope,
        )
    } else {
        add_item(
            menu,
            "添加到范围",
            Some(IconName::CirclePlus),
            Workbench::add_to_scope,
        )
    }
    .separator();
    let menu = add_item(menu, "扫描 (Scan)", None, Workbench::open_scan_launcher);
    let menu = add_item(
        menu,
        "被动扫描所选消息",
        None,
        Workbench::send_to_passive_scanner,
    );
    let menu = add_item(menu, "主动扫描所选消息", None, Workbench::send_to_scanner)
        .separator()
        .item(
            PopupMenuItem::element(move |_, _| {
                menu_shortcut_row(
                    "logger-ctx-intruder",
                    "发送到 Intruder",
                    if mod_key == "cmd" { "cmd-i" } else { "ctrl-i" },
                )
            })
            .disabled(!available)
            .on_click(move |_, window, cx| {
                let _ = o_intruder.update(cx, |v, cx| {
                    v.send_logger_batch_to_intruder(&ids_intruder, window, cx);
                });
            }),
        )
        .item(
            PopupMenuItem::element(move |_, _| {
                menu_shortcut_row(
                    "logger-ctx-repeater",
                    "发送到 Repeater",
                    if mod_key == "cmd" { "cmd-r" } else { "ctrl-r" },
                )
            })
            .disabled(!available)
            .on_click(move |_, window, cx| {
                let _ = o_repeater.update(cx, |v, cx| {
                    v.send_logger_batch_to_repeater(&ids_repeater, window, cx);
                });
            }),
        );
    let menu = add_item(menu, "发送到 Sequencer", None, Workbench::send_to_sequencer)
        .item(
            PopupMenuItem::new("发送到 Comparer (请求)")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ = o_cmp_req.update(cx, |v, cx| {
                        v.send_logger_batch_to_comparer(&ids_cmp_req, false, window, cx);
                    });
                }),
        )
        .item(
            PopupMenuItem::new("发送到 Comparer (响应)")
                .disabled(!available)
                .on_click(move |_, window, cx| {
                    let _ = o_cmp_resp.update(cx, |v, cx| {
                        v.send_logger_batch_to_comparer(&ids_cmp_resp, true, window, cx);
                    });
                }),
        );
    let menu = add_item(menu, "发送到 Decoder", None, Workbench::send_to_decoder)
        .item(
            PopupMenuItem::element(move |_, _| {
                menu_shortcut_row(
                    "logger-ctx-organizer",
                    "发送到 Organizer",
                    if mod_key == "cmd" { "cmd-o" } else { "ctrl-o" },
                )
            })
            .disabled(!available)
            .on_click(move |_, window, cx| {
                let _ = o_org.update(cx, |v, cx| {
                    v.send_logger_batch_to_organiser(&ids_org, window, cx);
                });
            }),
        )
        .item(
            PopupMenuItem::element(move |_, _| {
                menu_shortcut_row(
                    "logger-ctx-mio-at",
                    "发送到 MioAT",
                    if mod_key == "cmd" { "cmd-g" } else { "ctrl-g" },
                )
            })
            .icon(IconName::Zap)
            .disabled(!available)
            .on_click(move |_, window, cx| {
                let _ = o_ai.update(cx, |v, cx| v.send_logger_selected_to_mio_at(window, cx));
            }),
        )
        .separator()
        .item(PopupMenuItem::submenu("记录问题", record_issue_submenu))
        .separator();
    let menu = add_item(
        menu,
        "在浏览器中打开响应",
        None,
        Workbench::open_current_response_in_browser,
    )
    .item(PopupMenuItem::submenu("在浏览器中请求", req_browser_submenu))
    .separator()
    .item(PopupMenuItem::submenu("参与工具", engagement_submenu))
    .separator();
    let menu = add_item(
        menu,
        "编辑备注",
        Some(IconName::File),
        Workbench::open_http_notes,
    )
    .item(PopupMenuItem::submenu("高亮", highlight_submenu))
    .separator()
    .item(
        PopupMenuItem::new("删除选中记录 (Delete selected)")
            .icon(IconName::Trash)
            .disabled(!available)
            .on_click(move |_, window, cx| {
                let _ = o_del.update(cx, |v, cx| {
                    v.delete_logger_selected_entries(&ids_del, window, cx);
                });
            }),
    );
    let menu = add_item(
        menu,
        "清空日志 (Clear log)",
        None,
        Workbench::clear_logger_entries,
    )
    .separator();
    let menu = if multi {
        menu.item(
            PopupMenuItem::new("导出选中项为 CSV")
                .disabled(!available)
                .on_click(move |_, _, cx| {
                    let _ = o_sel_csv.update(cx, |v, cx| {
                        v.export_logger_selected_csv(&ids_sel_csv, cx);
                    });
                }),
        )
    } else {
        menu
    };
    let menu = add_item(
        menu,
        "导出为 CSV (Export as CSV)",
        None,
        Workbench::export_logger_csv,
    )
    .item(
        PopupMenuItem::new("复制 URL")
            .icon(IconName::Copy)
            .disabled(!available)
            .on_click(move |_, window, cx| {
                let _ = o_urls.update(cx, |v, cx| {
                    v.copy_logger_batch_urls(&ids_urls, window, cx);
                });
            }),
    );
    let menu = add_item(
        menu,
        "复制为 curl 命令 (bash)",
        Some(IconName::Copy),
        Workbench::copy_curl_command,
    );
    let menu = add_item(
        menu,
        "复制响应中的链接",
        Some(IconName::Copy),
        Workbench::copy_links_in_response,
    );
    let menu = add_item(
        menu,
        "保存条目 (Save item)",
        Some(IconName::Save),
        Workbench::save_selected_logger_item_to_file,
    )
    .separator();
    let sitemap_owner = view.clone();
    menu.item(
        PopupMenuItem::new("添加到站点地图 (Add to site map)")
            .icon(gpui_kit::component::Icon::default().data(MIO_ADD_TO_SITEMAP_SVG))
            .disabled(!available)
            .on_click(move |_, window, cx| {
                let _ = sitemap_owner.update(cx, |v, cx| v.add_to_site_map(window, cx));
            }),
    )
}
