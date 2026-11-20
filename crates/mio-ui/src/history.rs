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
    gpui_kit::rgb(if matches!(color, 1 | 6) {
        0xffffff
    } else {
        0x000000
    })
    .into()
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
        let key = match sort.column {
            HttpHistorySortColumn::Number => "number",
            HttpHistorySortColumn::Host => "host",
            HttpHistorySortColumn::Method => "method",
            HttpHistorySortColumn::Url => "url",
            HttpHistorySortColumn::Params => "params",
            HttpHistorySortColumn::Edited => "edited",
            HttpHistorySortColumn::Status => "status",
            HttpHistorySortColumn::Length => "length",
            HttpHistorySortColumn::MimeType => "mime",
            HttpHistorySortColumn::Extension => "extension",
            HttpHistorySortColumn::Title => "title",
            HttpHistorySortColumn::Notes => "notes",
            HttpHistorySortColumn::Tls => "tls",
            HttpHistorySortColumn::Ip => "ip",
            HttpHistorySortColumn::Cookies => "cookies",
            HttpHistorySortColumn::Time => "time",
            HttpHistorySortColumn::ListenerPort => "listener_port",
            HttpHistorySortColumn::StartResponseTimer => "start_response_timer",
        };
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
        if let HistoryRow::ExtProxy(row) = entry {
            let ExtHistoryRow {
                item,
                request,
                edited_request,
                response,
                edited_response,
                origin,
                ..
            } = row.as_ref();
            let ext_path_and_query = || {
                request
                    .as_ref()
                    .and_then(|request| request.request_target.as_deref())
                    .map(http_history_path_and_query)
                    .unwrap_or_default()
            };
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
                .and_then(|request| request.method.as_deref())
                .unwrap_or("");
            return match self.columns[column].key.as_ref() {
                "number" => item.index.saturating_add(1).to_string(),
                "host" => format_ext_host_origin(origin_str),
                "method" => method.to_owned(),
                "url" => ext_path_and_query(),
                "params" => {
                    if let Some(m) = meta {
                        m.params_cell_text().into()
                    } else if url_has_non_empty_query(&ext_path_and_query()) {
                        "✓".into()
                    } else {
                        String::new()
                    }
                }
                "edited" => {
                    let is_edited = meta.map_or_else(
                        || edited_request.is_some() || edited_response.is_some(),
                        |m| m.edited || m.has_original_request || m.has_original_response,
                    );
                    if is_edited {
                        "✓".into()
                    } else {
                        String::new()
                    }
                }
                "status" => response
                    .as_ref()
                    .and_then(|response| response.status)
                    .map(|status| status.to_string())
                    .unwrap_or_default(),
                "length" => meta
                    .and_then(|m| m.response_length)
                    .or_else(|| response.as_ref().map(|r| r.raw_bytes))
                    .map(|len| len.to_string())
                    .unwrap_or_default(),
                "mime" => meta
                    .map(|m| format_history_mime_display(&m.mime_type))
                    .unwrap_or_default(),
                "extension" => extract_path_extension(&ext_path_and_query()),
                "title" => meta
                    .map(|m| {
                        if !m.title.is_empty() || m.title_error.is_none() {
                            m.title.clone()
                        } else {
                            "标题读取失败".into()
                        }
                    })
                    .unwrap_or_default(),
                "time" => row.time_error.as_ref().map_or_else(
                    || row.time_unix_ms.map(local_time).unwrap_or_default(),
                    |_| "时间读取失败".into(),
                ),
                "notes" => row.notes_error.as_ref().map_or_else(
                    || {
                        row.notes_preview
                            .replace("\r\n", " ")
                            .replace(['\r', '\n', '\t'], " ")
                    },
                    |_| "备注读取失败".into(),
                ),
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
            };
        }
        let HistoryRow::Live(flow) = entry else {
            unreachable!()
        };
        match self.columns[column].key.as_ref() {
            "number" => self
                .live_numbers
                .get(&flow.id)
                .copied()
                .unwrap_or((self.offset + row + 1) as u64)
                .to_string(),
            "host" => format_ext_host_origin(&flow.url),
            "method" => flow.method.clone(),
            "url" => http_history_path_and_query(&flow.url),
            "params" => {
                if let Some(m) = meta {
                    m.params_cell_text().into()
                } else if url_has_non_empty_query(&http_history_path_and_query(&flow.url)) {
                    "✓".into()
                } else {
                    String::new()
                }
            }
            "edited" => {
                if meta.is_some_and(|m| m.edited || m.has_original_request || m.has_original_response) {
                    "✓".into()
                } else {
                    String::new()
                }
            }
            "status" => flow
                .status
                .map(|status| status.to_string())
                .unwrap_or_default(),
            "length" => meta
                .and_then(|m| m.response_length)
                .or_else(|| flow.status.is_some().then_some(flow.response_bytes))
                .map(|len| len.to_string())
                .unwrap_or_default(),
            "mime" => meta
                .map(|m| format_history_mime_display(&m.mime_type))
                .unwrap_or_default(),
            "extension" => extract_path_extension(&http_history_path_and_query(&flow.url)),
            "title" => meta
                .map(|m| {
                    if !m.title.is_empty() || m.title_error.is_none() {
                        m.title.clone()
                    } else {
                        "标题读取失败".into()
                    }
                })
                .unwrap_or_default(),
            "time" => {
                if meta.is_some_and(|m| !m.time_known) {
                    String::new()
                } else {
                    local_time(flow.started_unix_ms)
                }
            }
            "notes" => self
                .live_notes
                .borrow()
                .get(&flow.id)
                .map(|(_, text)| text.replace("\r\n", " ").replace(['\r', '\n', '\t'], " "))
                .unwrap_or_default(),
            "tls" => {
                if flow
                    .url
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
                .unwrap_or_else(|| extract_ip_literal(&flow.url)),
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
