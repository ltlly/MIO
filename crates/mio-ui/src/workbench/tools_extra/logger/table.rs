use std::collections::{HashMap, HashSet, VecDeque};

use mio_core::{FlowId, FlowRecord, FlowSummary};
use super::*;

mod actions;
mod filters;
mod menus;

pub(super) use filters::*;
pub(super) use menus::{build_logger_header_context_menu, logger_row_menu};

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
pub(in crate::workbench::tools_extra) struct LoggerUiTableState {
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

impl LoggerUiTableState {
    pub(super) fn snapshot(
        &mut self,
        primary_selected: Option<FlowId>,
        entries: &[FlowSummary],
    ) -> (HashSet<FlowId>, [f32; LOGGER_COL_COUNT], [bool; LOGGER_COL_COUNT]) {
        if primary_selected.is_none() || entries.is_empty() {
            self.selected_set.clear();
            self.anchor_id = None;
            self.explicit_empty = None;
        } else {
            self.selected_set
                .retain(|id| entries.iter().any(|e| e.id == *id));
            if let Some(sel) = primary_selected
                && self.selected_set.is_empty()
                && self.explicit_empty != Some(sel)
            {
                self.explicit_empty = None;
                self.selected_set.insert(sel);
            }
        }
        (self.selected_set.clone(), self.col_widths, self.col_visible)
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
