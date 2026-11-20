use std::collections::HashSet;

use super::*;

mod menu;

pub(in crate::workbench) const HIGHLIGHT_NAMES: [&str; 10] = [
    "无", "红色", "橙色", "黄色", "绿色", "青色", "蓝色", "粉色", "品红", "灰色",
];

pub(super) fn high_contrast_highlight_fg(color: u8) -> gpui_kit::Hsla {
    gpui_kit::rgb(match color {
        1 | 6 | 8 => 0xffffff,
        _ => 0x111111,
    })
    .into()
}

fn organiser_status_color(status: u16, hl_color: Option<u8>) -> gpui_kit::Hsla {
    if let Some(c) = hl_color {
        if matches!(c, 1 | 6 | 8) {
            return gpui_kit::rgb(0xffffff).into();
        }
        return crate::editing::status_code_color(status, false, true);
    }
    crate::editing::status_code_color(status, theme::is_dark(), false)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::workbench) struct OrganiserRowSummary {
    pub(in crate::workbench) id: mio_core::Uuid,
    pub(in crate::workbench) number: u64,
    pub(in crate::workbench) added_unix_ms: i64,
    pub(in crate::workbench) source_tool: String,
    pub(in crate::workbench) method: String,
    pub(in crate::workbench) url: String,
    pub(in crate::workbench) status_code: Option<u16>,
    pub(in crate::workbench) response_len: u64,
    pub(in crate::workbench) has_response: bool,
    pub(in crate::workbench) workflow_status: OrganiserStatus,
    pub(in crate::workbench) notes_preview: String,
    pub(in crate::workbench) highlight: Option<u8>,
}

impl OrganiserRowSummary {
    pub(super) fn from_item(item: &OrganiserItem) -> Self {
        Self {
            id: item.id,
            number: item.number,
            added_unix_ms: item.added_unix_ms,
            source_tool: item.source_tool.clone(),
            method: item.method.clone(),
            url: item.url.clone(),
            status_code: item.status_code,
            response_len: item.response_len,
            has_response: item.response_raw.is_some(),
            workflow_status: item.workflow_status,
            notes_preview: item.notes_preview(),
            highlight: item.highlight,
        }
    }
}

pub(in crate::workbench) struct OrganiserTableDelegate {
    owner: WeakEntity<Workbench>,
    pub(in crate::workbench) rows: Vec<OrganiserRowSummary>,
    pub(in crate::workbench) selected_id: Option<mio_core::Uuid>,
    pub(in crate::workbench) selected_ids: HashSet<mio_core::Uuid>,
    columns: Vec<Column>,
    all_columns: Vec<Column>,
}

impl OrganiserTableDelegate {
    pub(super) fn default_columns() -> Vec<Column> {
        vec![
            Column::new("number", "#")
                .sortable()
                .sort(ColumnSort::Ascending)
                .width(54.)
                .movable(false),
            Column::new("time", "时间").width(155.),
            Column::new("source", "来源").width(90.),
            Column::new("method", "方法").sortable().width(68.),
            Column::new("url", "URL").sortable().width(340.),
            Column::new("status", "状态码").sortable().width(78.),
            Column::new("length", "长度")
                .sortable()
                .width(84.)
                .text_right(),
            Column::new("workflow_status", "处理状态")
                .sortable()
                .width(92.),
            Column::new("notes", "备注摘要").width(220.),
        ]
    }

    pub(super) fn new(owner: WeakEntity<Workbench>) -> Self {
        let columns = Self::default_columns();
        Self {
            owner,
            rows: Vec::new(),
            selected_id: None,
            selected_ids: HashSet::new(),
            all_columns: columns.clone(),
            columns,
        }
    }

    pub(super) fn apply_sort(&mut self, sort: OrganiserSort) {
        let key = match sort.column {
            OrganiserSortColumn::Number => "number",
            OrganiserSortColumn::Method => "method",
            OrganiserSortColumn::Url => "url",
            OrganiserSortColumn::Status => "status",
            OrganiserSortColumn::Length => "length",
            OrganiserSortColumn::WorkflowStatus => "workflow_status",
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

    fn cell(&self, row: usize, column: usize) -> String {
        let Some(entry) = self.rows.get(row) else {
            return String::new();
        };
        match self.columns[column].key.as_ref() {
            "number" => entry.number.to_string(),
            "time" => format_local_time(entry.added_unix_ms),
            "source" => entry.source_tool.clone(),
            "method" => entry.method.clone(),
            "url" => entry.url.clone(),
            "status" => entry
                .status_code
                .map(|code| code.to_string())
                .unwrap_or_default(),
            "length" => entry.response_len.to_string(),
            "workflow_status" => entry.workflow_status.label().to_owned(),
            "notes" => entry.notes_preview.clone(),
            _ => String::new(),
        }
    }
}

fn selected_organiser_background(color: u8) -> gpui_kit::Hsla {
    if !matches!(color, 1..=9) {
        return theme::selected();
    }
    let rgb = match color {
        1 => 0xdc3737,
        2 => 0xd99f1f,
        3 => 0xd7d600,
        4 => 0x0dd80a,
        5 => 0x01d7d7,
        6 => 0x3d3cde,
        7 => 0xda9f9f,
        8 => 0xdc33dd,
        9 => 0x8d8d8d,
        _ => return theme::selected(),
    };
    gpui_kit::rgb(rgb).into()
}

pub(super) fn format_local_time(unix_ms: i64) -> String {
    let seconds = unix_ms.div_euclid(1000);
    let Some(local) = crate::local_time::from_unix_seconds(seconds) else {
        return "时间无法显示".into();
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        local.year, local.month, local.day, local.hour, local.min, local.sec,
    )
}

impl TableDelegate for OrganiserTableDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn fit_column(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Option<gpui_kit::Pixels> {
        let col = self.columns.get(col_ix)?.clone();
        let min_w = col.min_width.as_f32().max(40.0);
        let max_w = col.max_width.as_f32().min(520.0).max(min_w);
        let measure = |text: &str| -> f32 {
            text.chars()
                .map(|ch| if ch.is_ascii() { 7.2 } else { 13.5 })
                .sum()
        };
        let mut widest =
            measure(col.name.as_ref()) + if col.sort.is_some() { 18.0 } else { 4.0 };
        let sample_len = self.rows.len().min(250);
        for row_ix in 0..sample_len {
            let cell_str = crate::theme::truncate_cell_preview(&self.cell(row_ix, col_ix));
            widest = widest.max(measure(&cell_str));
        }
        let unscaled = (widest + 8.0).round().clamp(min_w, max_w);
        self.columns[col_ix].width = px(unscaled);
        if let Some(slot) = self.all_columns.iter_mut().find(|c| c.key == col.key) {
            slot.width = px(unscaled);
        }
        let scale = theme::ui_scale();
        Some(px((unscaled * scale).round()))
    }

    fn column(&self, index: usize, _: &App) -> Column {
        let scale = theme::ui_scale();
        let mut col = self.columns[index].clone();
        if (scale - 1.0).abs() > f32::EPSILON {
            let width = px((col.width.as_f32() * scale).round());
            col = col.width(width);
        }
        col.paddings(gpui_kit::Edges {
            top: px(0.),
            bottom: px(0.),
            left: theme::scaled_px(8.),
            right: theme::scaled_px(8.),
        })
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col = self.column(col_ix, cx);
        div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_left()
            .child(col.name)
    }

    fn show_unsorted_sort_icon(&self) -> bool {
        false
    }

    fn sort_on_header_click(&self) -> bool {
        true
    }

    fn perform_sort(
        &mut self,
        col: usize,
        sort: ColumnSort,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let column = match self.columns[col].key.as_ref() {
            "number" => OrganiserSortColumn::Number,
            "method" => OrganiserSortColumn::Method,
            "url" => OrganiserSortColumn::Url,
            "status" => OrganiserSortColumn::Status,
            "length" => OrganiserSortColumn::Length,
            "workflow_status" => OrganiserSortColumn::WorkflowStatus,
            _ => return,
        };
        let owner = self.owner.clone();
        let handle = window.window_handle();
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.sort_organiser(
                        OrganiserSort {
                            column,
                            ascending: sort == ColumnSort::Ascending,
                        },
                        window,
                        cx,
                    );
                });
            });
        });
    }

    fn row_selection_managed_by_delegate(&self) -> bool {
        true
    }

    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let entry = self.rows.get(row);
        let item_id = entry.map(|item| item.id);
        let highlight = entry
            .and_then(|item| item.highlight)
            .filter(|color| (1..=9).contains(color));
        let selected = item_id
            .is_some_and(|id| self.selected_id == Some(id) || self.selected_ids.contains(&id));
        let owner = self.owner.clone();
        div()
            .id(("organiser-tr", row))
            .aria_selected(selected)
            .when(selected, |this| {
                this.border_1().border_color(theme::accent())
            })
            .when_some(item_id, |this, id| {
                let left_owner = owner.clone();
                let right_owner = owner;
                this.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                    let owner = left_owner.clone();
                    let handle = window.window_handle();
                    let shift = event.modifiers.shift;
                    let toggle = event.modifiers.secondary(cx);
                    cx.defer(move |cx| {
                        let _ = handle.update(cx, |_, window, cx| {
                            let _ = owner.update(cx, |view, cx| {
                                view.handle_organiser_row_mouse_down(
                                    id,
                                    MouseButton::Left,
                                    shift,
                                    toggle,
                                    window,
                                    cx,
                                );
                            });
                        });
                    });
                })
                .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                    let owner = right_owner.clone();
                    let handle = window.window_handle();
                    cx.defer(move |cx| {
                        let _ = handle.update(cx, |_, window, cx| {
                            let _ = owner.update(cx, |view, cx| {
                                view.handle_organiser_row_mouse_down(
                                    id,
                                    MouseButton::Right,
                                    false,
                                    false,
                                    window,
                                    cx,
                                );
                            });
                        });
                    });
                })
            })
            .when_some(highlight, |this, color| {
                this.when_some(
                    crate::history::highlight_background(color),
                    |this, background| {
                        this.bg(background)
                            .text_color(high_contrast_highlight_fg(color))
                    },
                )
            })
            .when(selected, |this| {
                let color = highlight.unwrap_or(0);
                this.bg(selected_organiser_background(color))
                    .text_color(if color == 0 {
                        theme::foreground()
                    } else {
                        high_contrast_highlight_fg(color)
                    })
            })
    }

    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col_key = self.columns.get(column).map_or("", |col| col.key.as_ref());
        let is_right = col_key == "length";
        let status_color = if col_key == "status" {
            self.rows.get(row).and_then(|entry| {
                let hl = entry.highlight.filter(|c| (1..=9).contains(c));
                entry
                    .status_code
                    .map(|code| organiser_status_color(code, hl))
            })
        } else {
            None
        };
        let preview = crate::theme::truncate_cell_preview(&self.cell(row, column));
        if column == 0 {
            div()
                .id(("organiser-row", row))
                .test_support()
                .h_full()
                .w_full()
                .flex()
                .items_center()
                .child(div().min_w_0().truncate().child(preview))
                .into_any_element()
        } else {
            div()
                .h_full()
                .w_full()
                .flex()
                .items_center()
                .when(is_right, |this| this.justify_end())
                .when_some(status_color, |this, color| {
                    this.text_color(color)
                        .font_weight(gpui_kit::FontWeight::BOLD)
                })
                .child(div().min_w_0().truncate().child(preview))
                .into_any_element()
        }
    }

    fn cell_label(&self, row: usize, column: usize, _: &App) -> Option<SharedString> {
        Some(self.cell(row, column).into())
    }

    fn cell_text(&self, row: usize, column: usize, _: &App) -> String {
        self.cell(row, column)
    }

    fn hide_column(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> bool {
        if col_ix >= self.columns.len() || self.columns.len() <= 1 {
            return true;
        }
        if self.columns[col_ix].key.as_ref() == "number" {
            return true;
        }
        self.columns.remove(col_ix);
        true
    }

    fn reset_columns(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> bool {
        let sort = self
            .owner
            .upgrade()
            .map(|e| e.read(cx).organiser.sort)
            .unwrap_or_default();
        let defaults = Self::default_columns();
        self.all_columns = defaults.clone();
        self.columns = defaults;
        self.apply_sort(sort);
        true
    }

    fn column_header_context_menu(
        &mut self,
        _col_ix: usize,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let visible_keys: Vec<SharedString> =
            self.columns.iter().map(|c| c.key.clone()).collect();
        let all_cols = self.all_columns.clone();
        let table_entity = cx.entity().clone();
        let owner = self.owner.clone();
        let cols_submenu = PopupMenu::build(window, cx, move |mut sub, _, _| {
            let visible_count = visible_keys.len();
            for col in &all_cols {
                let key = col.key.clone();
                let is_visible = visible_keys.contains(&key);
                let is_number = key.as_ref() == "number";
                let can_toggle = !is_number && (!is_visible || visible_count > 1);
                let toggle_table = table_entity.clone();
                let toggle_owner = owner.clone();
                sub = sub.item(
                    PopupMenuItem::new(col.name.clone())
                        .checked(is_visible)
                        .disabled(!can_toggle)
                        .on_click(move |_, _, cx| {
                            let sort = toggle_owner
                                .upgrade()
                                .map(|e| e.read(cx).organiser.sort)
                                .unwrap_or_default();
                            toggle_table.update(cx, |table, cx| {
                                let delegate = table.delegate_mut();
                                if delegate.columns.iter().any(|c| c.key == key) {
                                    if delegate.columns.len() > 1 {
                                        delegate.columns.retain(|c| c.key != key);
                                    }
                                } else if let Some((target_pos, target_col)) = delegate
                                    .all_columns
                                    .iter()
                                    .enumerate()
                                    .find(|(_, c)| c.key == key)
                                    .map(|(p, c)| (p, c.clone()))
                                {
                                    let insert_ix = delegate
                                        .columns
                                        .iter()
                                        .position(|existing| {
                                            delegate
                                                .all_columns
                                                .iter()
                                                .position(|d| d.key == existing.key)
                                                .is_some_and(|pos| pos > target_pos)
                                        })
                                        .unwrap_or(delegate.columns.len());
                                    delegate.columns.insert(insert_ix, target_col);
                                }
                                delegate.apply_sort(sort);
                                table.refresh(cx);
                            });
                        }),
                );
            }
            let show_all_table = table_entity.clone();
            let show_all_owner = owner.clone();
            sub.separator().item(
                PopupMenuItem::new("显示全部列 (Show all columns)").on_click(move |_, _, cx| {
                    let sort = show_all_owner
                        .upgrade()
                        .map(|e| e.read(cx).organiser.sort)
                        .unwrap_or_default();
                    show_all_table.update(cx, |table, cx| {
                        let delegate = table.delegate_mut();
                        let defaults = Self::default_columns();
                        delegate.all_columns = defaults.clone();
                        delegate.columns = defaults;
                        delegate.apply_sort(sort);
                        table.refresh(cx);
                    });
                }),
            )
        });
        menu.separator()
            .item(PopupMenuItem::submenu("显示/隐藏列 (Columns)", cols_submenu))
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
        let visible_cols = self.columns.clone();
        let visible_keys: HashSet<SharedString> =
            visible_cols.iter().map(|c| c.key.clone()).collect();
        let mut visible_iter = visible_cols.into_iter();
        for slot in &mut self.all_columns {
            if visible_keys.contains(&slot.key)
                && let Some(updated) = visible_iter.next()
            {
                *slot = updated;
            }
        }
        cx.notify();
    }

    fn on_column_widths_changed(
        &mut self,
        widths: &[gpui_kit::Pixels],
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        let scale = theme::ui_scale().max(1.0);
        for (ix, col) in self.columns.iter_mut().enumerate() {
            if let Some(&w) = widths.get(ix)
                && w > px(0.)
            {
                let unscaled = px((w.as_f32() / scale).round());
                col.width = unscaled;
                if let Some(slot) = self.all_columns.iter_mut().find(|c| c.key == col.key) {
                    slot.width = unscaled;
                }
            }
        }
    }

    fn context_menu(
        &mut self,
        row: usize,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let Some(entry) = self.rows.get(row).cloned() else {
            return menu;
        };
        let multi_selected = self.selected_ids.len() > 1 && self.selected_ids.contains(&entry.id);
        menu::build_organiser_context_menu(
            entry,
            multi_selected,
            self.owner.clone(),
            menu,
            window,
            cx,
        )
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
            .id("organiser-table-empty")
            .test_support()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme::muted_foreground())
            .child("暂无符合当前筛选条件的条目")
    }
}
