use gpui_kit::component::input::EditorState;
use mio_core::{ExtItem, ExtItemDetail, ExtItemSummary};

use super::*;

const METADATA_BYTES: usize = 256 * 1024;
const ITEM_FIELD_BYTES: usize = 4096;

pub(super) struct Items {
    table: Entity<TableState<ItemDelegate>>,
    collections: Entity<SelectState<SearchableVec<CollectionChoice>>>,
    filter: Option<String>,
    query_revision: u64,
    query_offset: usize,
    previous_offsets: Vec<usize>,
    selection_revision: u64,
    selected: Option<Uuid>,
    requested_message_offset: usize,
    pub(super) detail: Option<ExtItem>,
    detail_error: Option<String>,
    metadata: Entity<EditorState>,
    _selection: Subscription,
    _collection: Subscription,
}

impl Items {
    pub(super) fn clear_selection(
        &mut self,
        fallback: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Workbench>,
    ) {
        self.selected = None;
        self.requested_message_offset = 0;
        self.detail_error = None;
        self.clear_detail(fallback, window, cx);
        self.table.update(cx, |table, cx| {
            table.clear_selection(cx);
            table.refresh(cx);
        });
    }

    pub(super) fn clear_detail(
        &mut self,
        fallback: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Workbench>,
    ) {
        if self.metadata.focus_handle(cx).is_focused(window) {
            fallback.focus(window, cx);
        }
        self.detail = None;
        self.metadata
            .update(cx, |state, cx| state.set_value("", window, cx));
    }

    pub(super) fn backend_disconnected(&mut self, cx: &mut Context<Workbench>) {
        self.table.update(cx, |table, cx| {
            if table.delegate().loading {
                table.delegate_mut().loading = false;
                table.delegate_mut().error = Some("后台服务已断开，条目查询未确认。".into());
                table.refresh(cx);
            }
        });
        if self.selected.is_some() && self.detail.is_none() {
            self.detail_error = Some("后台服务已断开，条目读取未确认。".into());
        }
    }

    pub(super) fn refresh_table(&mut self, soft_wrap: bool, window: &mut Window, cx: &mut App) {
        self.metadata
            .update(cx, |state, cx| state.set_soft_wrap(soft_wrap, window, cx));
        self.table.update(cx, |table, cx| {
            table.refresh(cx);
            cx.notify();
        });
    }
}

struct ItemDelegate {
    rows: Vec<ExtItemSummary>,
    offset: usize,
    total: usize,
    next_offset: Option<usize>,
    loading: bool,
    error: Option<String>,
    columns: Vec<Column>,
}

impl Default for ItemDelegate {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            offset: 0,
            total: 0,
            next_offset: None,
            loading: false,
            error: None,
            columns: vec![
                Column::new("number", "#").width(45.).movable(false),
                Column::new("collection", "来源集合").width(170.),
                Column::new("index", "原位置").width(75.).text_right(),
                Column::new("state", "读取状态").width(110.),
                Column::new("messages", "消息数").width(75.).text_right(),
                Column::new("address", "对象地址").width(125.).text_right(),
            ],
        }
    }
}

impl ItemDelegate {
    fn cell(&self, row: usize, column: usize) -> String {
        let Some(item) = self.rows.get(row) else {
            return String::new();
        };
        match self.columns[column].key.as_ref() {
            "number" => (self.offset + row + 1).to_string(),
            "collection" => item.collection.clone(),
            "index" => item.index.to_string(),
            "state" => item.state.clone(),
            "messages" => item.message_count.to_string(),
            "address" => item
                .object_address
                .map_or_else(|| "null".into(), |address| address.to_string()),
            _ => unreachable!("columns and cell keys are constructed together"),
        }
    }
}

impl TableDelegate for ItemDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, index: usize, _: &App) -> Column {
        let scale = theme::ui_scale();
        let mut col = self.columns[index].clone();
        if (scale - 1.0).abs() > f32::EPSILON {
            let width = col.width.as_f32();
            col = col.width(px((width * scale).round()));
        }
        col.paddings(gpui_kit::Edges {
            top: px(0.),
            bottom: px(0.),
            left: theme::scaled_px(8.),
            right: theme::scaled_px(8.),
        })
    }

    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div().truncate().child(self.cell(row, column))
    }

    fn cell_label(&self, row: usize, column: usize, _: &App) -> Option<SharedString> {
        Some(self.cell(row, column).into())
    }

    fn move_column(
        &mut self,
        from: usize,
        to: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let column = self.columns.remove(from);
        self.columns.insert(to, column);
        cx.notify();
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let label = self.error.as_ref().map_or_else(
            || {
                if self.loading {
                    "正在读取归档条目摘要…".into()
                } else {
                    "当前集合没有归档条目；集合状态和覆盖缺口见导入记录。".into()
                }
            },
            |error| format!("读取归档条目失败：{error}"),
        );
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(label)
    }
}

impl Messages {
    fn item_read_failed(
        &mut self,
        error: String,
        fallback: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Workbench>,
    ) {
        if let Some(items) = &mut self.items {
            items.detail_error = Some(error.clone());
            items.clear_detail(fallback, window, cx);
        }
        self.selected = None;
        self.detail_error = None;
        self.clear_detail(fallback, window, cx);
        self.table.update(cx, |table, cx| {
            table.delegate_mut().loading = false;
            table.delegate_mut().error = Some(error);
            table.clear_selection(cx);
            table.refresh(cx);
        });
    }
}

fn valid_summary(row: &ExtItemSummary) -> bool {
    !row.collection.is_empty()
        && row.collection.len() <= ITEM_FIELD_BYTES
        && !row.state.is_empty()
        && row.state.len() <= ITEM_FIELD_BYTES
        && i64::try_from(row.index).is_ok()
        && row
            .object_address
            .is_none_or(|address| i64::try_from(address).is_ok())
        && i64::try_from(row.message_count).is_ok()
}

fn valid_detail(detail: &ExtItemDetail, summary: &ExtItemSummary, offset: usize) -> bool {
    let item = &detail.item;
    let page = &detail.messages;
    item.id == summary.id
        && item.source_id == summary.source_id
        && item.collection == summary.collection
        && item.index == summary.index
        && item.object_address == summary.object_address
        && item.state == summary.state
        && item.metadata_json.len() <= METADATA_BYTES
        && std::str::from_utf8(&item.metadata_json).is_ok()
        && page.total == summary.message_count
        && valid_page_range(offset, page.rows.len(), page.total, page.next_offset)
        && page.rows.iter().all(|row| {
            row.item_id == item.id
                && row.source_id == item.source_id
                && row.collection == item.collection
                && row.item_index == item.index
        })
        && page
            .rows
            .iter()
            .map(|row| row.id)
            .collect::<HashSet<_>>()
            .len()
            == page.rows.len()
}

mod controller;
mod events;
mod render;

#[cfg(test)]
mod tests;
