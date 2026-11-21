use gpui_kit::component::{
    menu::PopupMenu,
    table::{Column, ColumnSort, TableDelegate, TableState},
};
use gpui_kit::{App, Context, IntoElement, SharedString, WeakEntity, Window, div, prelude::*};
use mio_core::{
    ExtHistoryRow, ExtHttpHighlight, FlowId, HistoryEntryId, HistoryRow, HttpHistorySort,
    HttpHistoryRowMetadata, HttpHistorySortColumn, http_history_path_and_query,
};
#[cfg(test)]
use mio_core::TrafficSource;
use std::collections::{HashMap, HashSet};

use crate::workbench::Workbench;
use crate::{
    AddHttpNotes, AddToScope, CopyCurlCommand, CopyUrl, OpenInIntruder,
    OpenInRepeater, SendToComparerRequest, SendToComparerResponse, SendToDecoder, SendToOrganiser,
    SendToScanner, SendToSequencer,
};

mod delegate;
#[cfg(test)]
mod tests;

pub(crate) const PAGE_SIZE: usize = 200;
pub(crate) const HTTP_HISTORY_BATCH_SIZE: usize = 5_000;

fn format_ext_host_origin(raw: &str) -> String {
    let Some((scheme_raw, rest)) = raw.split_once("://") else {
        return String::new();
    };
    let scheme = scheme_raw.to_ascii_lowercase();
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return String::new();
    }
    let stripped = if scheme == "http" {
        authority.strip_suffix(":80").unwrap_or(authority)
    } else if scheme == "https" {
        authority.strip_suffix(":443").unwrap_or(authority)
    } else {
        authority
    };
    format!("{scheme}://{stripped}")
}

fn extract_path_extension(path_and_query: &str) -> String {
    let path = path_and_query.split(['?', '#']).next().unwrap_or("");
    let Some(segment) = path.rsplit('/').next().filter(|s| !s.is_empty()) else {
        return String::new();
    };
    let Some((_, ext)) = segment.rsplit_once('.') else {
        return String::new();
    };
    if !ext.is_empty()
        && ext.len() <= 16
        && ext
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        ext.to_owned()
    } else {
        String::new()
    }
}

fn url_has_non_empty_query(path_and_query: &str) -> bool {
    path_and_query
        .split_once('?')
        .is_some_and(|(_, query)| !query.split('#').next().unwrap_or("").is_empty())
}

fn extract_ip_literal(origin_or_url: &str) -> String {
    let Some((_, rest)) = origin_or_url.split_once("://") else {
        return String::new();
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = if authority.starts_with('[') {
        authority
            .split_once(']')
            .map_or("", |(h, _)| h.trim_start_matches('['))
    } else {
        authority.split(':').next().unwrap_or("")
    };
    if host.parse::<std::net::IpAddr>().is_ok() {
        host.to_owned()
    } else {
        String::new()
    }
}

pub(crate) fn highlight_background(color: u8) -> Option<gpui_kit::Hsla> {
    let rgb = match color {
        1 => 0xff6464,
        2 => 0xffc864,
        3 => 0xffff64,
        4 => 0x64ff64,
        5 => 0x64ffff,
        6 => 0x6364ff,
        7 => 0xffc8c8,
        8 => 0xff64ff,
        9 => 0xb4b4b4,
        _ => return None,
    };
    Some(gpui_kit::rgb(rgb).into())
}

pub(crate) fn highlight_foreground(color: u8) -> gpui_kit::Hsla {
    gpui_kit::rgb(if matches!(color, 1 | 6 | 8) {
        0xffffff
    } else {
        0x111111
    })
    .into()
}

const HISTORY_SORT_COLUMNS: [(&str, HttpHistorySortColumn); 18] = [
    ("number", HttpHistorySortColumn::Number),
    ("host", HttpHistorySortColumn::Host),
    ("method", HttpHistorySortColumn::Method),
    ("url", HttpHistorySortColumn::Url),
    ("params", HttpHistorySortColumn::Params),
    ("edited", HttpHistorySortColumn::Edited),
    ("status", HttpHistorySortColumn::Status),
    ("length", HttpHistorySortColumn::Length),
    ("mime", HttpHistorySortColumn::MimeType),
    ("extension", HttpHistorySortColumn::Extension),
    ("title", HttpHistorySortColumn::Title),
    ("notes", HttpHistorySortColumn::Notes),
    ("tls", HttpHistorySortColumn::Tls),
    ("ip", HttpHistorySortColumn::Ip),
    ("cookies", HttpHistorySortColumn::Cookies),
    ("time", HttpHistorySortColumn::Time),
    ("listener_port", HttpHistorySortColumn::ListenerPort),
    ("start_response_timer", HttpHistorySortColumn::StartResponseTimer),
];

pub(super) fn sort_column_key(column: HttpHistorySortColumn) -> &'static str {
    HISTORY_SORT_COLUMNS
        .iter()
        .find_map(|&(key, sort_col)| (sort_col == column).then_some(key))
        .unwrap_or("number")
}

pub(super) fn sort_column_from_key(key: &str) -> Option<HttpHistorySortColumn> {
    HISTORY_SORT_COLUMNS
        .iter()
        .find_map(|&(col_key, sort_col)| (col_key == key).then_some(sort_col))
}

fn selected_highlight_background(color: u8, lead: bool) -> gpui_kit::Hsla {
    let rgb = match (color, lead) {
        (0, true) => 0xcadaf0,
        (0, false) => 0xc8dcee,
        (1, true) => 0xdc3737,
        (2, true) => 0xd99f1f,
        (3, true) => 0xd7d600,
        (4, true) => 0x0dd80a,
        (5, true) => 0x01d7d7,
        (6, true) => 0x3d3cde,
        (7, true) => 0xda9f9f,
        (8, true) => 0xdc33dd,
        (9, true) => 0x8d8d8d,
        (1, false) => 0xdf4443,
        (5, false) => 0x43dfdf,
        (6, false) => 0x4443df,
        (7, false) => 0xdfa8a8,
        _ => return highlight_background(color).unwrap_or_else(|| gpui_kit::rgb(0xeaf1f6).into()),
    };
    gpui_kit::rgb(rgb).into()
}

pub(crate) struct HistoryDelegate {
    pub rows: Vec<HistoryRow>,
    pub offset: usize,
    pub total: usize,
    pub loading: bool,
    pub previous_offsets: Vec<usize>,
    pub selected_ids: HashSet<HistoryEntryId>,
    pub selected_rows: HashMap<HistoryEntryId, HistoryRow>,
    pub selected_order: Vec<HistoryEntryId>,
    pub selected_ranks: HashMap<HistoryEntryId, usize>,
    pub selection_anchor: Option<HistoryEntryId>,
    pub selection_lead: Option<HistoryEntryId>,
    pub open_highlight_combo_row: Option<usize>,
    pub live_numbers: HashMap<FlowId, u64>,
    pub row_metadata: HashMap<HistoryEntryId, HttpHistoryRowMetadata>,
    pub live_notes: std::cell::RefCell<HashMap<FlowId, (u64, String)>>,
    pub live_highlights: std::cell::RefCell<HashMap<FlowId, ExtHttpHighlight>>,
    pub live_highlight_errors: std::cell::RefCell<HashMap<FlowId, String>>,
    owner: Option<WeakEntity<Workbench>>,
    pub(crate) columns: Vec<Column>,
}

impl Default for HistoryDelegate {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            offset: 0,
            total: 0,
            loading: false,
            previous_offsets: Vec::new(),
            selected_ids: HashSet::new(),
            selected_rows: HashMap::new(),
            selected_order: Vec::new(),
            selected_ranks: HashMap::new(),
            selection_anchor: None,
            selection_lead: None,
            open_highlight_combo_row: None,
            live_numbers: HashMap::new(),
            row_metadata: HashMap::new(),
            live_notes: std::cell::RefCell::new(HashMap::new()),
            live_highlights: std::cell::RefCell::new(HashMap::new()),
            live_highlight_errors: std::cell::RefCell::new(HashMap::new()),
            owner: None,
            columns: vec![
                Column::new("number", "#")
                    .descending()
                    .width(45.)
                    .movable(false)
                    .paddings(gpui_kit::Edges {
                        top: gpui_kit::px(0.),
                        right: gpui_kit::px(0.),
                        bottom: gpui_kit::px(0.),
                        left: gpui_kit::px(5.),
                    }),
                Column::new("host", "Host").sortable().width(187.),
                Column::new("method", "Method").sortable().width(60.),
                Column::new("url", "URL").sortable().width(509.),
                Column::new("params", "Params").sortable().width(60.),
                Column::new("edited", "Edited").sortable().width(60.),
                Column::new("status", "Status code").sortable().width(90.),
                Column::new("length", "Length")
                    .sortable()
                    .width(60.),
                Column::new("mime", "MIME type").sortable().width(75.),
                Column::new("extension", "Extension").sortable().width(75.),
                Column::new("title", "Title").sortable().width(150.),
                Column::new("notes", "Notes").sortable().width(112.),
                Column::new("tls", "TLS").sortable().width(45.),
                Column::new("ip", "IP").sortable().width(120.),
                Column::new("cookies", "Cookies").sortable().width(120.),
                Column::new("time", "Time").sortable().width(90.),
                Column::new("listener_port", "Listener port")
                    .sortable()
                    .width(90.),
                Column::new("start_response_timer", "Start response timer")
                    .sortable()
                    .width(90.),
            ],
        }
    }
}

impl HistoryDelegate {
    pub(crate) fn all_columns() -> Vec<Column> {
        Self::default().columns
    }

    pub(crate) fn with_owner(mut self, owner: WeakEntity<Workbench>) -> Self {
        self.owner = Some(owner);
        self
    }

    pub(crate) fn apply_sort(&mut self, sort: HttpHistorySort) {
        let key = sort_column_key(sort.column);
        for column in &mut self.columns {
            if column.sort.is_some() {
                column.sort = Some(if column.key.as_ref() != key {
                    ColumnSort::Default
                } else if sort.ascending {
                    ColumnSort::Ascending
                } else {
                    ColumnSort::Descending
                });
            }
        }
    }

    pub(crate) fn row_by_id(&self, id: HistoryEntryId) -> Option<&HistoryRow> {
        self.rows
            .iter()
            .find(|row| row.id() == id)
            .or_else(|| self.selected_rows.get(&id))
    }

    pub(crate) fn cached_rows_mut(&mut self) -> impl Iterator<Item = &mut HistoryRow> {
        self.rows.iter_mut().chain(self.selected_rows.values_mut())
    }

    pub(crate) fn selection_rows(&self) -> Vec<&HistoryRow> {
        let mut seen = HashSet::new();
        self.selected_order
            .iter()
            .copied()
            .chain(self.rows.iter().map(HistoryRow::id))
            .filter(|id| self.selected_ids.contains(id) && seen.insert(*id))
            .filter_map(|id| self.row_by_id(id))
            .collect()
    }

    fn highlight_targets(&self, row: usize) -> Vec<HistoryEntryId> {
        let Some(clicked) = self.rows.get(row).map(HistoryRow::id) else {
            return Vec::new();
        };
        if !self.selected_ids.contains(&clicked) {
            return vec![clicked];
        }
        self.selection_rows()
            .into_iter()
            .map(HistoryRow::id)
            .collect()
    }

    fn cell(&self, row: usize, column: usize) -> String {
        let Some(entry) = self.rows.get(row) else {
            return String::new();
        };
        let meta = self.row_metadata.get(&entry.id());
        let (number, origin_str, method, edited_fallback, status, length_fallback) = match entry {
            HistoryRow::ExtProxy(row_data) => {
                let ExtHistoryRow {
                    item,
                    request,
                    edited_request,
                    response,
                    edited_response,
                    origin,
                    ..
                } = row_data.as_ref();
                let origin_str = origin
                    .as_deref()
                    .or_else(|| {
                        request
                            .as_ref()
                            .and_then(|r| r.request_target.as_deref())
                            .filter(|t| t.contains("://"))
                    })
                    .unwrap_or("");
                let method = request
                    .as_ref()
                    .and_then(|r| r.method.as_deref())
                    .unwrap_or("");
                (
                    item.index.saturating_add(1),
                    origin_str,
                    method,
                    edited_request.is_some() || edited_response.is_some(),
                    response.as_ref().and_then(|r| r.status),
                    response.as_ref().map(|r| r.raw_bytes),
                )
            }
            HistoryRow::Live(flow) => (
                self.live_numbers
                    .get(&flow.id)
                    .copied()
                    .unwrap_or((self.offset + row + 1) as u64),
                flow.url.as_str(),
                flow.method.as_str(),
                false,
                flow.status,
                flow.status.is_some().then_some(flow.response_bytes),
            ),
        };
        let path_and_query = || match entry {
            HistoryRow::ExtProxy(row_data) => row_data
                .request
                .as_ref()
                .and_then(|r| r.request_target.as_deref())
                .map(http_history_path_and_query)
                .unwrap_or_default(),
            HistoryRow::Live(flow) => http_history_path_and_query(&flow.url),
        };
        let clean_notes = |text: &str| text.replace("\r\n", " ").replace(['\r', '\n', '\t'], " ");
        match self.columns[column].key.as_ref() {
            "number" => number.to_string(),
            "host" => format_ext_host_origin(origin_str),
            "method" => method.to_owned(),
            "url" => path_and_query(),
            "params" => {
                if let Some(m) = meta {
                    m.params_cell_text().into()
                } else if url_has_non_empty_query(&path_and_query()) {
                    "✓".into()
                } else {
                    String::new()
                }
            }
            "edited" => {
                let is_edited = meta.map_or(edited_fallback, |m| {
                    m.edited || m.has_original_request || m.has_original_response
                });
                if is_edited { "✓".into() } else { String::new() }
            }
            "status" => status.map(|s| s.to_string()).unwrap_or_default(),
            "length" => meta
                .and_then(|m| m.response_length)
                .or(length_fallback)
                .map(|len| len.to_string())
                .unwrap_or_default(),
            "mime" => meta
                .map(|m| format_history_mime_display(&m.mime_type))
                .unwrap_or_default(),
            "extension" => extract_path_extension(&path_and_query()),
            "title" => meta
                .map(|m| {
                    if !m.title.is_empty() || m.title_error.is_none() {
                        m.title.clone()
                    } else {
                        "标题读取失败".into()
                    }
                })
                .unwrap_or_default(),
            "time" => match entry {
                HistoryRow::ExtProxy(row_data) => row_data.time_error.as_ref().map_or_else(
                    || row_data.time_unix_ms.map(local_time).unwrap_or_default(),
                    |_| "时间读取失败".into(),
                ),
                HistoryRow::Live(flow) => {
                    if meta.is_some_and(|m| !m.time_known) {
                        String::new()
                    } else {
                        local_time(flow.started_unix_ms)
                    }
                }
            },
            "notes" => match entry {
                HistoryRow::ExtProxy(row_data) => row_data
                    .notes_error
                    .as_ref()
                    .map_or_else(|| clean_notes(&row_data.notes_preview), |_| "备注读取失败".into()),
                HistoryRow::Live(flow) => self
                    .live_notes
                    .borrow()
                    .get(&flow.id)
                    .map(|(_, text)| clean_notes(text))
                    .unwrap_or_default(),
            },
            "tls" => {
                if origin_str
                    .split_once("://")
                    .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("https"))
                {
                    "✓".into()
                } else {
                    String::new()
                }
            }
            "ip" => meta
                .map(|m| m.remote_ip.as_str())
                .filter(|ip| !ip.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| extract_ip_literal(origin_str)),
            "cookies" => meta
                .map(|m| {
                    if !m.cookies.is_empty() || m.cookies_error.is_none() {
                        m.cookies.clone()
                    } else {
                        "Cookie 解析失败".into()
                    }
                })
                .unwrap_or_default(),
            "listener_port" => meta
                .and_then(|m| m.listener_port)
                .map(|port| port.to_string())
                .unwrap_or_default(),
            "start_response_timer" => meta
                .and_then(|m| m.response_received_ms)
                .map(|ms| ms.to_string())
                .unwrap_or_default(),
            _ => unreachable!("columns are constructed together with their renderers"),
        }
    }
}

pub(crate) fn format_history_mime_display(mime: &str) -> String {
    match mime.trim().to_ascii_lowercase().as_str() {
        "html" => "HTML".into(),
        "xml" => "XML".into(),
        "css" => "CSS".into(),
        "json" => "JSON".into(),
        _ => mime.to_owned(),
    }
}

pub(crate) fn local_time(unix_ms: i64) -> String {
    let seconds = unix_ms.div_euclid(1000);
    let Some(local) = crate::local_time::from_unix_seconds(seconds) else {
        return "时间无法显示".into();
    };
    format!(
        "{:02}:{:02}:{:02} {} {}月 {}",
        local.hour, local.min, local.sec, local.day, local.month, local.year,
    )
}
