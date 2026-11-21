use std::collections::HashSet;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::{
    WeakEntity,
    component::{
        menu::{PopupMenu, PopupMenuItem},
        table::{Column, ColumnSort, TableDelegate},
    },
};
use mio_core::{MAX_RECENT_PROJECTS, RecentProject};

use super::*;

impl Default for RecentProjectsDelegate {
    fn default() -> Self {
        Self {
            owner: None,
            columns: [
                Column::new("name", "名称")
                    .width(px(124.))
                    .sortable()
                    .movable(false),
                Column::new("file", "文件")
                    .width(px(306.))
                    .sortable()
                    .movable(false),
                Column::new("last_accessed", "最后访问")
                    .width(px(126.))
                    .sort(ColumnSort::Descending)
                    .movable(false),
            ],
            rows: Vec::new(),
            missing_paths: HashSet::new(),
            selected_path: None,
            sort_column: RecentSortColumn::LastAccessed,
            ascending: false,
        }
    }
}

impl RecentProjectsDelegate {
    pub(super) fn with_owner(mut self, owner: WeakEntity<Workbench>) -> Self {
        self.owner = Some(owner);
        self
    }

    pub(super) fn set_rows(&mut self, mut rows: Vec<RecentProject>) {
        if rows.len() > MAX_RECENT_PROJECTS {
            rows.truncate(MAX_RECENT_PROJECTS);
        }
        self.missing_paths = rows
            .iter()
            .filter(|entry| {
                matches!(
                    std::fs::metadata(&entry.path),
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound
                )
            })
            .map(|entry| entry.path.clone())
            .collect();
        self.rows = rows;
        self.sort_rows();
    }

    pub(super) fn missing_count(&self) -> usize {
        self.missing_paths.len()
    }

    pub(super) fn is_missing(&self, path: &Path) -> bool {
        self.missing_paths.contains(path)
    }

    fn sort_rows(&mut self) {
        let sort_column = self.sort_column;
        let ascending = self.ascending;
        self.rows.sort_by(|left, right| {
            let ordering = match sort_column {
                RecentSortColumn::Name => left
                    .name
                    .cmp(&right.name)
                    .then_with(|| left.path.cmp(&right.path)),
                RecentSortColumn::File => left
                    .path
                    .cmp(&right.path)
                    .then_with(|| left.name.cmp(&right.name)),
                RecentSortColumn::LastAccessed => left
                    .last_accessed_unix_ms
                    .cmp(&right.last_accessed_unix_ms)
                    .then_with(|| left.path.cmp(&right.path)),
            };
            if ascending {
                ordering
            } else {
                ordering.reverse()
            }
        });
        for (idx, col) in self.columns.iter_mut().enumerate() {
            let active = matches!(
                (idx, self.sort_column),
                (0, RecentSortColumn::Name)
                    | (1, RecentSortColumn::File)
                    | (2, RecentSortColumn::LastAccessed)
            );
            col.sort = Some(if !active {
                ColumnSort::Default
            } else if self.ascending {
                ColumnSort::Ascending
            } else {
                ColumnSort::Descending
            });
        }
    }

    fn cell_text(&self, row: usize, column: usize) -> String {
        let Some(item) = self.rows.get(row) else {
            return String::new();
        };
        match column {
            0 => item.name.clone(),
            1 => {
                let raw = item.path.to_string_lossy();
                if self.is_missing(&item.path) {
                    format!("{raw}（文件不存在）")
                } else {
                    raw.into_owned()
                }
            }
            2 => format_local_date_cn(item.last_accessed_unix_ms),
            _ => String::new(),
        }
    }
}

impl TableDelegate for RecentProjectsDelegate {
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
            let width = px((col.width.as_f32() * scale).round());
            col = col.width(width);
        }
        col.paddings(gpui_kit::Edges {
            top: px(0.),
            right: theme::scaled_px(8.),
            bottom: px(0.),
            left: theme::scaled_px(8.),
        })
    }

    fn show_unsorted_sort_icon(&self) -> bool {
        false
    }

    fn sort_on_header_click(&self) -> bool {
        true
    }

    fn render_header(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let dark = theme::is_dark();
        div()
            .id("wizard-recent-projects-header")
            .bg(rgb(if dark { 0x25272b } else { 0xeaeaea }))
            .border_b_1()
            .border_color(wizard_border())
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col = self.column(col_ix, cx);
        let dark = theme::is_dark();
        div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_left()
            .font_family(theme::ui_font_family())
            .text_size(theme::ui_font_size())
            .font_weight(gpui_kit::FontWeight::NORMAL)
            .text_color(rgb(if dark { 0xd4d4d4 } else { 0x141414 }))
            .child(col.name)
    }

    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div().w_0().h_full()
    }

    fn perform_sort(
        &mut self,
        col: usize,
        sort: ColumnSort,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.sort_column = match col {
            0 => RecentSortColumn::Name,
            1 => RecentSortColumn::File,
            _ => RecentSortColumn::LastAccessed,
        };
        self.ascending = sort == ColumnSort::Ascending;
        self.sort_rows();
        let target_row = self
            .selected_path
            .as_ref()
            .and_then(|path| self.rows.iter().position(|entry| &entry.path == path));
        let table_weak = cx.weak_entity();
        cx.notify();
        let owner = self.owner.clone();
        let handle = window.window_handle();
        cx.defer(move |cx| {
            let _ = table_weak.update(cx, |table, cx| {
                if let Some(row) = target_row
                    && table.selected_row().is_some()
                {
                    table.set_selected_row_without_scroll(row, cx);
                }
            });
            if let Some(owner) = owner {
                let _ = handle.update(cx, |_, _, cx| {
                    let _ = owner.update(cx, |_, cx| cx.notify());
                });
            }
        });
    }

    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        div().id(("wizard-recent-project-row", row))
    }

    fn context_menu(
        &mut self,
        row: usize,
        menu: PopupMenu,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        if let Some(path) = self.rows.get(row).map(|item| item.path.clone()) {
            let owner = self.owner.clone();
            return menu.item(
                PopupMenuItem::new("从最近列表移除").on_click(move |_, window, cx| {
                    if let Some(owner) = owner.clone() {
                        let _ = owner.update(cx, |view, cx| {
                            view.remove_wizard_recent_path(path.clone(), window, cx);
                        });
                    }
                }),
            );
        }
        menu
    }

    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let is_missing_col = column == 1
            && self
                .rows
                .get(row)
                .is_some_and(|item| self.is_missing(&item.path));
        let color = if is_missing_col {
            rgb(if theme::is_dark() { 0xf0883e } else { 0x9a6700 }).into()
        } else {
            theme::foreground()
        };
        div()
            .flex_1()
            .min_w_0()
            .font_family(theme::ui_font_family())
            .text_size(theme::ui_font_size())
            .line_height(gpui_kit::relative(1.25))
            .text_color(color)
            .truncate()
            .child(self.cell_text(row, column))
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div().size_full().bg(theme::surface())
    }

    fn cell_label(&self, row: usize, column: usize, _: &App) -> Option<SharedString> {
        Some(self.cell_text(row, column).into())
    }
}

fn current_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

pub(super) fn format_today_iso() -> String {
    let seconds = current_unix_ms().div_euclid(1000);
    let Some(local) = crate::local_time::from_unix_seconds(seconds) else {
        return "Untitled".into();
    };
    let year = local.year;
    let month = local.month.clamp(1, 12);
    let day = local.day.clamp(1, 31);
    format!("{year:04}-{month:02}-{day:02}")
}

fn format_local_date_cn(unix_ms: i64) -> String {
    let seconds = unix_ms.div_euclid(1000);
    let Some(local) = crate::local_time::from_unix_seconds(seconds) else {
        return "时间未知".into();
    };
    let year = local.year;
    let month = local.month.clamp(1, 12);
    let day = local.day.clamp(1, 31);
    format!("{year}年{month}月{day}日")
}

pub(super) fn checked_utf8_path(path: &std::path::Path, label: &str) -> Result<String, String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{label}路径不是有效的 UTF-8，无法填入输入框。"))
}

pub(super) fn derived_mio_file(name: &str, custom_dir: Option<&std::path::Path>) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let filename = if trimmed.ends_with(".mio") || trimmed.ends_with(".MIO") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}.mio")
    };
    match custom_dir {
        Some(dir) => dir.join(filename).to_string_lossy().into_owned(),
        None => filename,
    }
}
