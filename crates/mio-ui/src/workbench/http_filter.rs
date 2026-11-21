use gpui_kit::component::input::EditorState;
use gpui_kit::component::switch::Switch;
use gpui_kit::{AnyWindowHandle, Bounds, WeakEntity, size};
use mio_core::HistoryEntryId;
use mio_core::{
    HttpExtensionFilter, HttpHistoryFilter, HttpHistoryQuery, HttpHistoryResult, HttpHistorySort,
};

use super::*;

mod actions;
mod form;
mod render;
use render::filter_summary;

const ONLY_EXTENSIONS: &str = "asp,aspx,jsp,php";
const HIDDEN_EXTENSIONS: &str = "js,gif,jpg,png,ico,css,woff,woff2,ttf,svg";
const DEFAULT_MIME_TYPES: [bool; 8] = [true, true, true, false, true, false, true, true];
const WINDOW_WIDTH: f32 = 910.;
const WINDOW_HEIGHT: f32 = 380.;
const CONTENT_HEIGHT: f32 = WINDOW_HEIGHT - crate::chrome::CUSTOM_TITLE_BAR_HEIGHT;
const MAX_SHIFT_SELECTED_HISTORY: usize = 500;

#[derive(Clone)]
pub(super) struct ExtensionInputs {
    pub(super) only: String,
    pub(super) hide: String,
}
impl Default for ExtensionInputs {
    fn default() -> Self {
        Self {
            only: ONLY_EXTENSIONS.into(),
            hide: HIDDEN_EXTENSIONS.into(),
        }
    }
}

pub(super) struct PendingFilter {
    pub(super) query: HttpHistoryQuery,
    pub(super) wait_revision: u64,
    pub(super) filter: HttpHistoryFilter,
    pub(super) enabled: bool,
    pub(super) inputs: ExtensionInputs,
}

pub(super) struct PendingHistoryShift {
    pub(super) anchor: HistoryEntryId,
    pub(super) lead: HistoryEntryId,
    pub(super) query_revision: u64,
    pub(super) query: HttpHistoryQuery,
    pub(super) range: Option<(usize, usize)>,
    pub(super) total: Option<usize>,
    pub(super) remaining_offsets: Vec<usize>,
    pub(super) collected: Vec<(usize, HistoryRow)>,
    pub(super) stage: HistoryShiftStage,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HistoryShiftStage {
    Discover,
    CollectPages,
}

pub(super) struct HttpFilterState {
    pub(super) applied: HttpHistoryFilter,
    pub(super) applied_url_filter: String,
    pub(super) sort: HttpHistorySort,
    pub(super) enabled: bool,
    pub(crate) continuous_scroll: bool,
    pub(crate) pending_auto_scroll: bool,
    pub(crate) detached_history_window: Option<AnyWindowHandle>,
    pub(super) window: Option<AnyWindowHandle>,
    pub(super) inputs: ExtensionInputs,
    pub(super) script_title: String,
    pub(super) all_columns: Vec<gpui_kit::component::table::Column>,
    pub(super) pending: Option<PendingFilter>,
    pub(super) outcome: Option<(u64, Result<(), String>)>,
    pub(super) history_shift: Option<PendingHistoryShift>,
}

impl Default for HttpFilterState {
    fn default() -> Self {
        Self {
            applied: HttpHistoryFilter {
                mime_types: DEFAULT_MIME_TYPES,
                extensions: HttpExtensionFilter::Hide(HIDDEN_EXTENSIONS.into()),
                ..Default::default()
            },
            applied_url_filter: String::new(),
            sort: HttpHistorySort::default(),
            enabled: true,
            continuous_scroll: false,
            pending_auto_scroll: false,
            detached_history_window: None,
            window: None,
            inputs: ExtensionInputs::default(),
            script_title: "Untitled".into(),
            all_columns: crate::history::HistoryDelegate::all_columns(),
            pending: None,
            outcome: None,
            history_shift: None,
        }
    }
}

impl HttpFilterState {
    pub(in crate::workbench) fn pending_query(&self) -> Option<HttpHistoryQuery> {
        self.pending.as_ref().map(|pending| pending.query.clone())
    }

    pub(in crate::workbench) fn requested(&self) -> (HttpHistoryFilter, HttpHistorySort, bool) {
        self.pending.as_ref().map_or_else(
            || (self.applied.clone(), self.sort, self.enabled),
            |pending| (pending.filter.clone(), pending.query.sort, pending.enabled),
        )
    }
}

pub(super) struct FilterWindow {
    pub(super) owner: WeakEntity<Workbench>,
    pub(super) owner_window: AnyWindowHandle,
    pub(super) project: ProjectKey,
    pub(super) applied: HttpHistoryFilter,
    pub(super) applied_inputs: ExtensionInputs,
    pub(super) applied_script_title: String,
    pub(super) draft: HttpHistoryFilter,
    pub(super) script_mode: bool,
    pub(super) script_title: String,
    pub(super) script_reference_open: bool,
    pub(super) library_overlay: Option<form::LibraryOverlay>,
    pub(super) library_name_input: Entity<InputState>,
    pub(super) library_path: Option<PathBuf>,
    pub(super) only: Entity<InputState>,
    pub(super) hide: Entity<InputState>,
    pub(super) search_term: Entity<InputState>,
    pub(super) listener_port: Entity<InputState>,
    pub(super) filter_script_editor: Entity<EditorState>,
    pub(super) pending: Option<(u64, bool)>,
    pub(super) submitting: bool,
    pub(super) error: Option<String>,
    pub(super) focus: FocusHandle,
    pub(super) _subscription: Subscription,
}

impl Workbench {
    pub(in crate::workbench) fn begin_history_shift_selection(
        &mut self,
        anchor: HistoryEntryId,
        lead: HistoryEntryId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(project) = self.project else {
            return false;
        };
        if !self.backend_available() || self.transition.is_some() {
            return false;
        }
        if self.history_loading {
            self.fail("HTTP 历史正在加载中；请稍后重试范围选择，原选择保持。", cx);
            return false;
        }
        if self.selected_history != Some(lead) && !self.leave_http_notes(window, cx) {
            return false;
        }
        let selected_ids = if anchor == lead {
            vec![anchor]
        } else {
            vec![anchor, lead]
        };
        let offset = self.history.read(cx).delegate().offset;
        let Some(query) = self.build_history_shift_query(project, offset, selected_ids, window, cx)
        else {
            return false;
        };
        if !self.dispatch_history_shift_query(&query, cx) {
            return false;
        }
        let revision = query.query_revision;
        self.http_filter.history_shift = Some(PendingHistoryShift {
            anchor,
            lead,
            query_revision: revision,
            query,
            range: None,
            total: None,
            remaining_offsets: Vec::new(),
            collected: Vec::new(),
            stage: HistoryShiftStage::Discover,
        });
        true
    }

    fn build_history_shift_query(
        &mut self,
        project: ProjectKey,
        offset: usize,
        selected_ids: Vec<HistoryEntryId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<HttpHistoryQuery> {
        let url_filter = match editing::input_snapshot(&self.history_filter, window, cx) {
            Ok(value) => value.to_string(),
            Err(error) => {
                self.fail(error, cx);
                return None;
            }
        };
        if url_filter.len() > 4096 {
            self.fail("URL／方法筛选超过 4096 字节；当前列表保留。", cx);
            return None;
        }
        let revision = self.query_revision + 1;
        let (filter, sort, enabled) = self.http_filter.requested();
        Some(HttpHistoryQuery {
            project,
            query_revision: revision,
            offset,
            limit: HTTP_HISTORY_BATCH_SIZE,
            url_filter,
            filter: if enabled {
                filter
            } else {
                HttpHistoryFilter::default()
            },
            sort,
            selected_ids,
        })
    }

    fn dispatch_history_shift_query(
        &mut self,
        query: &HttpHistoryQuery,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.enqueue(
            Command::QueryHttpHistory {
                query: query.clone(),
            },
            cx,
        ) {
            return false;
        }
        self.query_revision = query.query_revision;
        self.history_loading = true;
        self.history.update(cx, |state, cx| {
            state.delegate_mut().loading = true;
            cx.notify();
        });
        cx.notify();
        true
    }

    fn clear_history_shift(&mut self, cx: &mut Context<Self>) {
        self.http_filter.history_shift = None;
        if self.http_filter.pending.is_none() {
            self.history_loading = false;
            self.history.update(cx, |table, cx| {
                table.delegate_mut().loading = false;
                cx.notify();
            });
        }
        cx.notify();
    }

    fn collect_history_shift_rows(
        collected: &mut Vec<(usize, HistoryRow)>,
        page_offset: usize,
        rows: &[HistoryRow],
        min_rank: usize,
        max_rank: usize,
    ) {
        for (idx, row) in rows.iter().enumerate() {
            let rank = page_offset.saturating_add(idx);
            if (min_rank..=max_rank).contains(&rank) {
                collected.push((rank, row.clone()));
            }
        }
    }

    pub(in crate::workbench) fn history_shift_page_received(
        &mut self,
        query: &HttpHistoryQuery,
        result: &Result<HttpHistoryResult, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self
            .http_filter
            .history_shift
            .as_ref()
            .is_some_and(|shift| &shift.query == query)
        {
            return false;
        }
        let Ok(result) = result else {
            self.clear_history_shift(cx);
            self.fail("HTTP 历史跨页范围选择失败；原选择保留。", cx);
            return true;
        };
        if result.rows.len() > query.limit
            || result.rows.len() > result.total.saturating_sub(query.offset)
        {
            self.clear_history_shift(cx);
            self.fail("HTTP 历史回执数量或选择身份不匹配；原列表保持。", cx);
            return true;
        }
        self.live_http_annotations = result.live_annotations.clone();
        if !result.live_numbers.is_empty() || !result.row_metadata.is_empty() {
            self.history.update(cx, |table, _| {
                let delegate = table.delegate_mut();
                for (flow_id, number) in &result.live_numbers {
                    delegate.live_numbers.insert(*flow_id, *number);
                }
                for (entry_id, meta) in &result.row_metadata {
                    delegate.row_metadata.insert(*entry_id, meta.clone());
                }
            });
        }
        let mut shift = self.http_filter.history_shift.take().unwrap();
        match shift.stage {
            HistoryShiftStage::Discover => {
                let anchor_rank = result
                    .selected_ranks
                    .iter()
                    .find(|(id, _)| *id == shift.anchor)
                    .map(|(_, rank)| *rank);
                let lead_rank = result
                    .selected_ranks
                    .iter()
                    .find(|(id, _)| *id == shift.lead)
                    .map(|(_, rank)| *rank);
                let (Some(anchor_rank), Some(lead_rank)) = (anchor_rank, lead_rank) else {
                    self.clear_history_shift(cx);
                    self.fail(
                        "HTTP 历史范围选择起点或终点不在筛选结果内；原选择保留。",
                        cx,
                    );
                    return true;
                };
                if anchor_rank >= result.total || lead_rank >= result.total {
                    self.clear_history_shift(cx);
                    self.fail("HTTP 历史跨页范围选择越界；原选择保留。", cx);
                    return true;
                }
                let min_rank = anchor_rank.min(lead_rank);
                let max_rank = anchor_rank.max(lead_rank);
                let count = max_rank - min_rank + 1;
                if count > MAX_SHIFT_SELECTED_HISTORY {
                    self.clear_history_shift(cx);
                    self.fail("HTTP 历史最多同时选择 500 条；原选择保持。", cx);
                    return true;
                }
                let batch_size = query.limit.max(1);
                let first_page = (min_rank / batch_size) * batch_size;
                let last_page = (max_rank / batch_size) * batch_size;
                let discover_aligned = query.offset.is_multiple_of(batch_size)
                    && query.offset >= first_page
                    && query.offset <= last_page;
                if discover_aligned {
                    Self::collect_history_shift_rows(
                        &mut shift.collected,
                        query.offset,
                        &result.rows,
                        min_rank,
                        max_rank,
                    );
                }
                let mut remaining_offsets = Vec::new();
                let mut page_offset = first_page;
                while page_offset <= last_page {
                    if !discover_aligned || page_offset != query.offset {
                        remaining_offsets.push(page_offset);
                    }
                    match page_offset.checked_add(batch_size) {
                        Some(next) if next > page_offset => page_offset = next,
                        _ => break,
                    }
                }
                remaining_offsets.reverse();
                shift.range = Some((min_rank, max_rank));
                shift.total = Some(result.total);
                shift.remaining_offsets = remaining_offsets;
                shift.stage = HistoryShiftStage::CollectPages;
            }
            HistoryShiftStage::CollectPages => {
                let Some((min_rank, max_rank)) = shift.range else {
                    self.clear_history_shift(cx);
                    return true;
                };
                if shift.total != Some(result.total) || max_rank >= result.total {
                    self.clear_history_shift(cx);
                    self.fail("HTTP 历史列表在跨页选择期间已变化；原选择保留。", cx);
                    return true;
                }
                Self::collect_history_shift_rows(
                    &mut shift.collected,
                    query.offset,
                    &result.rows,
                    min_rank,
                    max_rank,
                );
            }
        }
        if let Some(next_offset) = shift.remaining_offsets.pop() {
            let mut next_query = shift.query.clone();
            let revision = self.query_revision + 1;
            next_query.query_revision = revision;
            next_query.offset = next_offset;
            next_query.selected_ids.clear();
            if !self.dispatch_history_shift_query(&next_query, cx) {
                self.clear_history_shift(cx);
                return true;
            }
            shift.query_revision = revision;
            shift.query = next_query;
            self.http_filter.history_shift = Some(shift);
            return true;
        }
        let Some((min_rank, max_rank)) = shift.range else {
            self.clear_history_shift(cx);
            return true;
        };
        let count = max_rank - min_rank + 1;
        self.clear_history_shift(cx);
        if shift.collected.len() != count {
            self.fail("HTTP 历史列表在跨页选择期间已变化；原选择保留。", cx);
            return true;
        }
        self.apply_history_range_selection(shift.collected, shift.anchor, shift.lead, window, cx);
        true
    }
}

#[cfg(test)]
mod tests;
