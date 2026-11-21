use gpui_kit::component::{
    select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState},
    table::{Column, TableDelegate},
};
use mio_core::{ExtMessageFragment, ExtMessagePage, ExtMessageSummary};

use super::*;

mod items;

const FRAGMENT_BYTES: usize = 64 * 1024;

pub(super) struct Messages {
    source_id: Uuid,
    table: Entity<TableState<MessageDelegate>>,
    collections: Entity<SelectState<SearchableVec<CollectionChoice>>>,
    filter: Option<String>,
    query_revision: u64,
    query_offset: usize,
    previous_offsets: Vec<usize>,
    selection_revision: u64,
    selected: Option<Uuid>,
    requested_offset: u64,
    pub(super) detail: Option<ExtMessageFragment>,
    detail_error: Option<String>,
    pub(super) payload: MessageEditor,
    items: Option<items::Items>,
    pub(in crate::workbench) items_visible: bool,
    _selection: Subscription,
    _collection: Subscription,
}

#[derive(Clone)]
struct CollectionChoice {
    path: String,
    label: SharedString,
}

impl SelectItem for CollectionChoice {
    type Value = String;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &String {
        &self.path
    }
}

impl Messages {
    pub(super) fn refresh_tables(&mut self, soft_wrap: bool, window: &mut Window, cx: &mut App) {
        self.table.update(cx, |table, cx| {
            table.refresh(cx);
            cx.notify();
        });
        if let Some(items) = &mut self.items {
            items.refresh_table(soft_wrap, window, cx);
        }
    }

    pub(super) fn selected_fragment(&self) -> Option<(u64, &ExtMessageFragment)> {
        let fragment = self.detail.as_ref()?;
        if self.items_visible {
            let item = self.items.as_ref()?.detail.as_ref()?;
            if fragment.summary.item_id != item.id
                || fragment.summary.collection != item.collection
                || fragment.summary.item_index != item.index
            {
                return None;
            }
        }
        (self.selected == Some(fragment.summary.id)).then_some((self.selection_revision, fragment))
    }

    fn clear_detail(
        &mut self,
        fallback: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Workbench>,
    ) {
        if self.payload.has_focus(window, cx) {
            fallback.focus(window, cx);
        }
        self.detail = None;
        self.payload.load(&[], window, cx);
    }

    fn clear_rows(
        &mut self,
        fallback: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Workbench>,
    ) {
        self.selected = None;
        self.requested_offset = 0;
        self.detail_error = None;
        self.query_offset = 0;
        self.previous_offsets.clear();
        self.clear_detail(fallback, window, cx);
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.rows.clear();
            delegate.offset = 0;
            delegate.total = 0;
            delegate.next_offset = None;
            delegate.loading = false;
            delegate.error = None;
            table.clear_selection(cx);
            table.refresh(cx);
        });
    }

    fn apply_page(
        &mut self,
        offset: usize,
        page: ExtMessagePage,
        fallback: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Workbench>,
    ) {
        let ExtMessagePage {
            total,
            rows,
            next_offset,
        } = page;
        let current_offset = self.table.read(cx).delegate().offset;
        remember_page(&mut self.previous_offsets, current_offset, offset);
        let selected = self
            .selected
            .and_then(|id| rows.iter().position(|row| row.id == id));
        if selected.is_none() {
            self.selected = None;
            self.detail_error = None;
            self.clear_detail(fallback, window, cx);
        } else if selected.is_some_and(|row| {
            self.detail
                .as_ref()
                .is_some_and(|detail| detail.summary != rows[row])
        }) {
            self.requested_offset = 0;
            self.detail_error = Some("归档消息摘要已变化，请重新读取正文。".into());
            self.clear_detail(fallback, window, cx);
        }
        self.table.update(cx, |state, cx| {
            let delegate = state.delegate_mut();
            delegate.rows = rows;
            delegate.offset = offset;
            delegate.total = total;
            delegate.next_offset = next_offset;
            delegate.loading = false;
            delegate.error = None;
            if let Some(row) = selected {
                if state.selected_row() != Some(row) {
                    state.set_selected_row(row, cx);
                }
            } else {
                state.clear_selection(cx);
            }
            state.refresh(cx);
        });
    }

    pub(super) fn backend_disconnected(&mut self, cx: &mut Context<Workbench>) {
        if let Some(items) = &mut self.items {
            items.backend_disconnected(cx);
        }
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            if delegate.loading {
                delegate.loading = false;
                delegate.error = Some("后台服务已断开，查询结果未确认。".into());
                table.refresh(cx);
            }
        });
        if self.selected.is_some() && self.detail.is_none() {
            self.detail_error = Some("后台服务已断开，正文读取未确认。".into());
        }
    }
}

fn valid_page_range(offset: usize, count: usize, total: usize, next: Option<usize>) -> bool {
    count <= PAGE_SIZE
        && offset.checked_add(count).is_some_and(|end| {
            end <= total
                && match next {
                    Some(next) => next == end && next > offset && next < total,
                    None => end == total,
                }
        })
}

fn remember_page(previous: &mut Vec<usize>, current: usize, offset: usize) {
    if offset == 0 {
        previous.clear();
    } else if offset > current {
        previous.push(current);
    } else if offset < current
        && let Some(index) = previous.iter().rposition(|previous| *previous == offset)
    {
        previous.truncate(index);
    }
}

struct MessageDelegate {
    rows: Vec<ExtMessageSummary>,
    offset: usize,
    total: usize,
    next_offset: Option<usize>,
    loading: bool,
    error: Option<String>,
    columns: Vec<Column>,
}

impl Default for MessageDelegate {
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
                Column::new("role", "字段角色").width(170.),
                Column::new("method", "方法 / 状态").width(90.),
                Column::new("target", "请求目标声明").width(240.),
                Column::new("length", "原始字节").width(100.).text_right(),
                Column::new("version", "声明版本").width(100.),
            ],
        }
    }
}

impl MessageDelegate {
    fn cell(&self, row: usize, column: usize) -> String {
        let Some(message) = self.rows.get(row) else {
            return String::new();
        };
        match self.columns[column].key.as_ref() {
            "number" => (self.offset + row + 1).to_string(),
            "collection" => message.collection.clone(),
            "index" => message.item_index.to_string(),
            "role" => message.role.clone(),
            "method" => message.method.clone().unwrap_or_else(|| {
                message
                    .status
                    .map_or_else(String::new, |status| status.to_string())
            }),
            "target" => message.request_target.clone().unwrap_or_default(),
            "length" => message.raw_bytes.to_string(),
            "version" => message
                .declared_version
                .map_or_else(String::new, |version| version.as_str().into()),
            _ => unreachable!("columns and cell keys are constructed together"),
        }
    }
}

impl TableDelegate for MessageDelegate {
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
                    "正在读取归档消息摘要…".into()
                } else {
                    "当前范围没有已索引消息；覆盖缺口见导入记录。".into()
                }
            },
            |error| format!("读取归档消息失败：{error}"),
        );
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(label)
    }
}

mod controller;
mod render;

#[cfg(test)]
mod tests;
