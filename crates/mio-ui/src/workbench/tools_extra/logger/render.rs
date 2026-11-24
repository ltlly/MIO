use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_kit::component::resizable::{resizable_panel, v_resizable};
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
use gpui_kit::{ClickEvent, DragMoveEvent, MouseButton, MouseDownEvent, uniform_list};
use mio_core::TrafficSource;

use super::*;

mod filter_bars;

#[derive(Clone, Copy)]
enum LoggerHeaderSort {
    Primary(LoggerSortColumn),
    Extended(LoggerExtendedSortColumn),
}

struct LoggerRowsCacheKey {
    entries_len: usize,
    first_id: Option<FlowId>,
    last_id: Option<FlowId>,
    entries_sig: u64,
    source_filter: Option<TrafficSource>,
    search_term: String,
    sort: LoggerSort,
    extended_sort: Option<(LoggerExtendedSortColumn, bool)>,
    status_classes: [bool; 4],
    in_scope_only: bool,
    scope: Option<mio_core::TargetScope>,
    script_enabled: bool,
    view_filter_script: String,
    param_counts_sig: (usize, usize),
    live_notes_sig: (usize, u64),
    live_highlights_sig: (usize, u64),
    loaded_details_sig: [Option<(FlowId, u64)>; 2],
}

pub(in crate::workbench::tools_extra) struct LoggerRowsCacheEntry {
    key: LoggerRowsCacheKey,
    rows: Rc<Vec<(usize, FlowId)>>,
}

fn logger_entries_fingerprint(entries: &[mio_core::FlowSummary]) -> u64 {
    let mut acc = entries.len() as u64;
    for entry in entries {
        let id_raw = entry.id.as_u128();
        let mut x = (id_raw as u64)
            ^ ((id_raw >> 64) as u64)
            ^ entry.revision.rotate_left(13)
            ^ (u64::from(entry.status.unwrap_or(0)) << 48)
            ^ entry.response_bytes.rotate_left(7)
            ^ entry.elapsed_ms.rotate_left(29)
            ^ (entry.started_unix_ms as u64).rotate_left(3)
            ^ (entry.url.len() as u64).rotate_left(19)
            ^ (entry.method.len() as u64).rotate_left(41);
        x ^= x >> 30;
        x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        acc = acc.rotate_left(5).wrapping_add(x);
    }
    acc
}

fn logger_live_notes_fingerprint(
    live_notes: &std::collections::HashMap<FlowId, (u64, String)>,
) -> (usize, u64) {
    use std::hash::{Hash, Hasher};
    let mut sum = 0u64;
    for (id, (rev, text)) in live_notes {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        id.hash(&mut h);
        rev.hash(&mut h);
        text.hash(&mut h);
        sum = sum.wrapping_add(h.finish());
    }
    (live_notes.len(), sum)
}

fn logger_live_highlights_fingerprint(
    live_highlights: &std::collections::HashMap<FlowId, mio_core::ExtHttpHighlight>,
) -> (usize, u64) {
    use std::hash::{Hash, Hasher};
    let mut sum = 0u64;
    for (id, highlight) in live_highlights {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        id.hash(&mut h);
        highlight.revision.hash(&mut h);
        highlight.color.hash(&mut h);
        sum = sum.wrapping_add(h.finish());
    }
    (live_highlights.len(), sum)
}


impl Workbench {
    #[inline(never)]
    pub(in crate::workbench) fn render_logger(
        &self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = theme::is_dark();
        let source_filter = self.extra_tools.logger_source_filter.clone();
        let search_term = self
            .extra_tools
            .logger_search
            .read(cx)
            .value()
            .trim()
            .to_string();

        let entries = &self.extra_tools.logger_entries;
        let entries_len = entries.len();
        let first_id = entries.first().map(|e| e.id);
        let last_id = entries.last().map(|e| e.id);
        let entries_sig = logger_entries_fingerprint(entries);
        let workbench_id = cx.entity_id();
        let filters = &self.extra_tools.logger_filters;
        let sort = self.extra_tools.logger_sort;
        let extended_sort = filters.extended_sort;
        let use_search_script = !search_term.is_empty()
            && (filters.script_enabled || table::is_script_expression(&search_term));
        let has_script = use_search_script || !filters.view_filter_script.trim().is_empty();
        let needs_notes_sig = has_script
            || !search_term.is_empty()
            || matches!(extended_sort, Some((LoggerExtendedSortColumn::Comment, _)));
        let param_counts_sig = if matches!(
            extended_sort,
            Some((LoggerExtendedSortColumn::ParamCount, _))
        ) {
            (
                filters.param_counts.len(),
                filters
                    .param_counts
                    .values()
                    .copied()
                    .fold(0usize, usize::wrapping_add),
            )
        } else {
            (0, 0)
        };
        let (live_notes_sig, live_highlights_sig) = if needs_notes_sig || has_script {
            let delegate = self.history.read(cx).delegate();
            let notes_sig = if needs_notes_sig {
                let live_notes = delegate.live_notes.borrow();
                logger_live_notes_fingerprint(&live_notes)
            } else {
                (0, 0)
            };
            let highlights_sig = if has_script {
                let live_highlights = delegate.live_highlights.borrow();
                logger_live_highlights_fingerprint(&live_highlights)
            } else {
                (0, 0)
            };
            (notes_sig, highlights_sig)
        } else {
            ((0, 0), (0, 0))
        };
        let loaded_details_sig = if has_script {
            [
                self.extra_tools
                    .logger_loaded_detail
                    .as_deref()
                    .map(|(record, _)| (record.summary.id, record.summary.revision)),
                self.detail
                    .as_deref()
                    .map(|record| (record.summary.id, record.summary.revision)),
            ]
        } else {
            [None, None]
        };

        let cached_rows = self
            .extra_tools
            .logger_visible_rows_cache
            .borrow()
            .as_ref()
            .filter(|entry| {
                let k = &entry.key;
                k.entries_len == entries_len
                    && k.first_id == first_id
                    && k.last_id == last_id
                    && k.entries_sig == entries_sig
                    && k.source_filter == source_filter
                    && k.search_term == search_term
                    && k.sort == sort
                    && k.extended_sort == extended_sort
                    && k.status_classes == filters.status_classes
                    && k.in_scope_only == filters.in_scope_only
                    && k.script_enabled == filters.script_enabled
                    && k.view_filter_script == filters.view_filter_script
                    && k.param_counts_sig == param_counts_sig
                    && k.live_notes_sig == live_notes_sig
                    && k.live_highlights_sig == live_highlights_sig
                    && k.loaded_details_sig == loaded_details_sig
                    && (!filters.in_scope_only || k.scope.as_ref() == Some(&self.workspace.scope))
            })
            .map(|entry| Rc::clone(&entry.rows));

        let rows_rc = if let Some(hit) = cached_rows {
            hit
        } else {
            let computed = {
                let loaded_details = self.logger_loaded_detail_records();
                let delegate = self.history.read(cx).delegate();
                let live_notes = delegate.live_notes.borrow();
                let live_highlights = delegate.live_highlights.borrow();
                let visible_rows = table::filter_and_sort_logger(
                    entries,
                    &source_filter,
                    &search_term,
                    sort,
                    filters,
                    &self.workspace.scope,
                    &live_notes,
                    &live_highlights,
                    &loaded_details,
                );
                Rc::new(
                    visible_rows
                        .into_iter()
                        .map(|(number, entry)| (number, entry.id))
                        .collect::<Vec<_>>(),
                )
            };
            *self.extra_tools.logger_visible_rows_cache.borrow_mut() = Some(LoggerRowsCacheEntry {
                key: LoggerRowsCacheKey {
                    entries_len,
                    first_id,
                    last_id,
                    entries_sig,
                    source_filter: source_filter.clone(),
                    search_term: search_term.clone(),
                    sort,
                    extended_sort,
                    status_classes: filters.status_classes,
                    in_scope_only: filters.in_scope_only,
                    scope: filters.in_scope_only.then(|| self.workspace.scope.clone()),
                    script_enabled: filters.script_enabled,
                    view_filter_script: filters.view_filter_script.clone(),
                    param_counts_sig,
                    live_notes_sig,
                    live_highlights_sig,
                    loaded_details_sig,
                },
                rows: Rc::clone(&computed),
            });
            computed
        };
        let table_head_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xf1f1f1 }).into();
        let table_head_fg: gpui_kit::Hsla = rgb(if dark { 0xd4d4d4 } else { 0x141414 }).into();
        let table_head_hover: gpui_kit::Hsla = rgb(if dark { 0x2f3237 } else { 0xe6e6e6 }).into();
        let cell_px = theme::scaled_px(6.);
        let (selected_set, col_widths, col_visible) = self
            .extra_tools
            .logger_ui_table
            .borrow_mut()
            .snapshot(self.extra_tools.logger_selected, entries);
        let visible_cols: Vec<usize> = (0..table::LOGGER_COL_COUNT)
            .filter(|&col_ix| col_visible[col_ix])
            .collect();
        let last_visible_col = visible_cols.last().copied().unwrap_or(0);
        let total_table_w = theme::scaled_px(
            visible_cols
                .iter()
                .map(|&col_ix| col_widths[col_ix])
                .sum::<f32>()
                .max(260.),
        );
        let visible_ids: Rc<Vec<FlowId>> =
            Rc::new(rows_rc.iter().map(|&(_, flow_id)| flow_id).collect());
        let selected_set_rc = Rc::new(selected_set);

        let header_defs: [(&'static str, &'static str, &'static str, LoggerHeaderSort);
            table::LOGGER_COL_COUNT] = [
            (
                "logger-sort-number",
                "#",
                "按请求序号升序或降序排列",
                LoggerHeaderSort::Primary(LoggerSortColumn::Number),
            ),
            (
                "logger-sort-time",
                "Time",
                "按请求发起时间排序",
                LoggerHeaderSort::Extended(LoggerExtendedSortColumn::Time),
            ),
            (
                "logger-sort-source",
                "Tool",
                "按发起请求的工具来源模块排序",
                LoggerHeaderSort::Primary(LoggerSortColumn::Source),
            ),
            (
                "logger-sort-method",
                "Method",
                "按 HTTP 请求方法排序",
                LoggerHeaderSort::Primary(LoggerSortColumn::Method),
            ),
            (
                "logger-sort-host",
                "Host",
                "按请求主机名排序",
                LoggerHeaderSort::Extended(LoggerExtendedSortColumn::Host),
            ),
            (
                "logger-sort-path",
                "Path",
                "按请求路径排序",
                LoggerHeaderSort::Extended(LoggerExtendedSortColumn::Path),
            ),
            (
                "logger-sort-query",
                "Query",
                "按查询参数字符串排序",
                LoggerHeaderSort::Extended(LoggerExtendedSortColumn::Query),
            ),
            (
                "logger-sort-params",
                "Param count",
                "按请求参数数量排序",
                LoggerHeaderSort::Extended(LoggerExtendedSortColumn::ParamCount),
            ),
            (
                "logger-sort-status",
                "Status code",
                "按 HTTP 响应状态码排序",
                LoggerHeaderSort::Primary(LoggerSortColumn::Status),
            ),
            (
                "logger-sort-length",
                "Length",
                "按响应字节长度排序",
                LoggerHeaderSort::Primary(LoggerSortColumn::Length),
            ),
            (
                "logger-sort-timer",
                "Start response timer",
                "按响应耗时 (ms) 排序",
                LoggerHeaderSort::Extended(LoggerExtendedSortColumn::StartResponseTimer),
            ),
            (
                "logger-sort-comment",
                "Comment",
                "按备注内容排序",
                LoggerHeaderSort::Extended(LoggerExtendedSortColumn::Comment),
            ),
        ];

        let weak = cx.weak_entity();
        let header_cells: Vec<AnyElement> = visible_cols
            .iter()
            .map(|&col_ix| {
                let (id, title, tip, target) = header_defs[col_ix];
                let width = col_widths[col_ix];
                let is_last = col_ix == last_visible_col;
                let active_asc = match (extended_sort, target) {
                    (Some((ext_col, asc)), LoggerHeaderSort::Extended(col)) if ext_col == col => {
                        Some(asc)
                    }
                    (None, LoggerHeaderSort::Primary(col)) if sort.column == col => {
                        Some(sort.ascending)
                    }
                    _ => None,
                };
                let ctx_view = weak.clone();
                let ctx_rows = Rc::clone(&rows_rc);
                let fit_rows = Rc::clone(&rows_rc);
                let resize_handle = div()
                    .id(("logger-col-resize", col_ix))
                    .occlude()
                    .cursor_col_resize()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right_0()
                    .w(px(6.))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if event.click_count == 2 {
                                this.fit_logger_column_width(col_ix, &fit_rows, cx);
                            } else {
                                this.begin_logger_column_resize(
                                    col_ix,
                                    event.position.x.as_f32(),
                                    cx,
                                );
                            }
                        }),
                    )
                    .on_drag(
                        table::ResizeLoggerCol {
                            entity_id: workbench_id,
                            col_ix,
                        },
                        |drag, _, _, cx| {
                            cx.stop_propagation();
                            cx.new(|_| *drag)
                        },
                    );
                let cell = div()
                    .id(id)
                    .test_support()
                    .role(gpui_kit::accesskit::Role::ColumnHeader)
                    .aria_label(title)
                    .relative()
                    .h_full()
                    .px(cell_px)
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .overflow_hidden()
                    .cursor_pointer()
                    .when(!is_last, |c| c.border_r_1().border_color(theme::border()))
                    .hover(move |s| s.bg(table_head_hover))
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| match target {
                        LoggerHeaderSort::Primary(col) => this.toggle_logger_sort(col, cx),
                        LoggerHeaderSort::Extended(col) => {
                            this.toggle_logger_extended_sort(col, cx)
                        }
                    }))
                    .context_menu(move |menu, window, cx| {
                        table::build_logger_header_context_menu(
                            ctx_view.clone(),
                            col_ix,
                            Rc::clone(&ctx_rows),
                            col_visible,
                            menu,
                            window,
                            cx,
                        )
                    })
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(theme::ui_font_size())
                            .text_color(table_head_fg)
                            .child(title),
                    )
                    .when_some(active_asc, |c, asc| {
                        c.child(
                            Icon::new(if asc {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .size(theme::scaled_px(12.))
                            .text_color(table_head_fg)
                            .flex_shrink_0(),
                        )
                    })
                    .child(resize_handle);
                if is_last {
                    cell.min_w(theme::scaled_px(width))
                        .flex_1()
                        .into_any_element()
                } else {
                    cell.w(theme::scaled_px(width))
                        .flex_shrink_0()
                        .into_any_element()
                }
            })
            .collect();

        let header_bar = div()
            .id("logger-sort-bar")
            .test_support()
            .min_w(total_table_w)
            .w_full()
            .h(theme::table_header_height())
            .flex_shrink_0()
            .flex()
            .items_center()
            .bg(table_head_bg)
            .text_color(table_head_fg)
            .border_b_1()
            .border_color(theme::border())
            .on_drag_move::<table::ResizeLoggerCol>(cx.listener(
                |this, event: &DragMoveEvent<table::ResizeLoggerCol>, _, cx| {
                    let drag = *event.drag(cx);
                    if drag.entity_id == cx.entity_id() {
                        this.update_logger_column_resize(
                            drag.col_ix,
                            event.event.position.x.as_f32(),
                            cx,
                        );
                    }
                },
            ))
            .children(header_cells);

        let selected_id = self.extra_tools.logger_selected;
        let row_count = rows_rc.len();
        let row_even_bg: gpui_kit::Hsla = rgb(if dark { 0x25272b } else { 0xfafafa }).into();
        let row_hover_bg: gpui_kit::Hsla = rgb(if dark { 0x26334a } else { 0xeeeeee }).into();
        let row_h = theme::scaled_px(21.);
        let zebra = theme::table_zebra_stripe();

        let list = uniform_list("logger-entries", row_count, move |range, _, cx| {
            let Some(owner) = weak.upgrade() else {
                return Vec::new();
            };
            let workbench = owner.read(cx);
            let entries = &workbench.extra_tools.logger_entries;
            let param_counts = &workbench.extra_tools.logger_filters.param_counts;
            let delegate = workbench.history.read(cx).delegate();
            let live_notes = delegate.live_notes.borrow();
            let live_highlights = delegate.live_highlights.borrow();
            let items = range
                .filter_map(|index| {
                    let &(number, flow_id) = rows_rc.get(index)?;
                    let entry = entries
                        .get(number.checked_sub(1)?)
                        .filter(|e| e.id == flow_id)
                        .or_else(|| entries.iter().find(|e| e.id == flow_id))?;
                    Some((index, number, entry))
                })
                .map(|(index, number, entry)| {
                    let flow_id = entry.id;
                    let selected = selected_set_rc.contains(&flow_id)
                        || (selected_set_rc.is_empty() && selected_id == Some(flow_id));
                    let parsed = table::parse_logger_url(&entry.url);
                    let time_text = crate::history::local_time(entry.started_unix_ms);
                    let tool_text = table::logger_source_label(&entry.source);
                    let param_text = table::entry_param_count(entry, param_counts).to_string();
                    let status_text = entry
                        .status
                        .map(|code| code.to_string())
                        .unwrap_or_default();
                    let length_text = if entry.status.is_some() || entry.response_bytes > 0 {
                        entry.response_bytes.to_string()
                    } else {
                        String::new()
                    };
                    let timer_text = if entry.status.is_some() || entry.elapsed_ms > 0 {
                        entry.elapsed_ms.to_string()
                    } else {
                        String::new()
                    };
                    let comment_text = table::entry_comment(flow_id, &live_notes);
                    let highlight_color = live_highlights
                        .get(&flow_id)
                        .map(|h| h.color)
                        .filter(|c| (1..=9).contains(c));
                    let row_bg = if selected {
                        theme::selected()
                    } else if let Some(hl) =
                        highlight_color.and_then(crate::history::highlight_background)
                    {
                        hl
                    } else if zebra && index % 2 == 1 {
                        row_even_bg
                    } else {
                        theme::surface()
                    };
                    let row_fg = if selected {
                        theme::foreground()
                    } else if let Some(hl) = highlight_color {
                        crate::history::highlight_foreground(hl)
                    } else {
                        theme::foreground()
                    };
                    let right_view = weak.clone();
                    let click_view = weak.clone();
                    let menu_view = weak.clone();
                    let row_visible_ids = Rc::clone(&visible_ids);
                    let row_url = entry.url.clone();
                    let mut row_cells = Vec::with_capacity(visible_cols.len());
                    for &col_ix in &visible_cols {
                        let text = match col_ix {
                            0 => number.to_string(),
                            1 => time_text.clone(),
                            2 => tool_text.clone(),
                            3 => entry.method.clone(),
                            4 => theme::truncate_cell_preview(&parsed.host),
                            5 => theme::truncate_cell_preview(&parsed.path),
                            6 => theme::truncate_cell_preview(&parsed.query),
                            7 => param_text.clone(),
                            8 => status_text.clone(),
                            9 => length_text.clone(),
                            10 => timer_text.clone(),
                            11 => theme::truncate_cell_preview(&comment_text),
                            _ => String::new(),
                        };
                        let w = col_widths[col_ix];
                        let cell = if col_ix == last_visible_col {
                            div()
                                .min_w(theme::scaled_px(w))
                                .flex_1()
                                .px(cell_px)
                                .truncate()
                                .child(text)
                        } else {
                            div()
                                .w(theme::scaled_px(w))
                                .flex_shrink_0()
                                .px(cell_px)
                                .truncate()
                                .child(text)
                        };
                        row_cells.push(cell);
                    }
                    div()
                        .id(("logger-row", index))
                        .test_support()
                        .aria_selected(selected)
                        .min_w(total_table_w)
                        .w_full()
                        .h(row_h)
                        .flex()
                        .items_center()
                        .text_size(theme::ui_font_size())
                        .text_color(row_fg)
                        .bg(row_bg)
                        .when(!selected && highlight_color.is_none(), |r| {
                            r.hover(move |s| s.bg(row_hover_bg))
                        })
                        .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                            let _ = right_view.update(cx, |this, cx| {
                                this.handle_logger_row_right_down(flow_id, window, cx);
                            });
                        })
                        .on_click(move |event: &ClickEvent, window, cx| {
                            let shift = event.modifiers().shift;
                            let toggle = event.modifiers().secondary(cx);
                            let _ = click_view.update(cx, |this, cx| {
                                this.handle_logger_row_click(
                                    flow_id,
                                    &row_visible_ids,
                                    shift,
                                    toggle,
                                    window,
                                    cx,
                                );
                            });
                        })
                        .context_menu(move |menu, window, cx| {
                            table::logger_row_menu(
                                menu_view.clone(),
                                flow_id,
                                row_url.clone(),
                                menu,
                                window,
                                cx,
                            )
                        })
                        .children(row_cells)
                })
                .collect::<Vec<_>>();
            drop(live_highlights);
            drop(live_notes);
            items
        })
        .track_scroll(&self.extra_tools.logger_filters.scroll_handle)
        .min_w(total_table_w)
        .w_full()
        .flex_1()
        .min_h_0()
        .bg(theme::surface());

        let sb_size = px(10.);
        let v_sb_width = px(11.);
        let h_sb_height = px(11.);
        let sb_track_bg: gpui_kit::Hsla = rgb(if dark { 0x2b2d30 } else { 0xf5f5f5 }).into();
        let sb_thumb_bg: gpui_kit::Hsla = rgb(if dark { 0x5a5d62 } else { 0xd4d4d4 }).into();
        let sb_edge: gpui_kit::Hsla = rgb(if dark { 0x35383d } else { 0xe8e8e8 }).into();

        let top_table = div()
            .id("logger-table-container")
            .relative()
            .size_full()
            .min_h_0()
            .bg(theme::surface())
            .border_t_1()
            .border_color(theme::border())
            .child(
                div()
                    .size_full()
                    .min_h_0()
                    .pr(v_sb_width)
                    .pb(h_sb_height)
                    .child(
                        div()
                            .id("logger-table-hscroll")
                            .size_full()
                            .min_h_0()
                            .overflow_x_scroll()
                            .track_scroll(&self.extra_tools.logger_filters.h_scroll_handle)
                            .child(
                                div()
                                    .h_full()
                                    .min_h_0()
                                    .min_w(total_table_w)
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .child(header_bar)
                                    .child(list),
                            ),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .w(v_sb_width)
                    .h(theme::table_header_height())
                    .bg(table_head_bg)
                    .border_l_1()
                    .border_b_1()
                    .border_color(theme::border()),
            )
            .child(
                div()
                    .id("logger-vertical-scrollbar")
                    .test_support()
                    .absolute()
                    .top(theme::table_header_height())
                    .right_0()
                    .bottom(h_sb_height)
                    .w(v_sb_width)
                    .bg(sb_track_bg)
                    .border_l_1()
                    .border_color(sb_edge)
                    .child(
                        Scrollbar::vertical(&self.extra_tools.logger_filters.scroll_handle)
                            .viewport_from_layout()
                            .mode(ScrollbarMode::Always)
                            .styles(move |styles| {
                                styles
                                    .track(|s| s.width(sb_size).bg(sb_track_bg))
                                    .track_hover(|s| s.width(sb_size).bg(sb_track_bg))
                                    .track_active(|s| s.width(sb_size).bg(sb_track_bg))
                                    .thumb(|s| {
                                        s.width(px(6.))
                                            .inset(px(2.))
                                            .radius(px(3.))
                                            .min_length(px(24.))
                                            .bg(sb_thumb_bg)
                                    })
                                    .thumb_hover(|s| {
                                        s.width(px(6.))
                                            .inset(px(2.))
                                            .radius(px(3.))
                                            .min_length(px(24.))
                                            .bg(sb_thumb_bg)
                                    })
                                    .thumb_active(|s| {
                                        s.width(px(6.))
                                            .inset(px(2.))
                                            .radius(px(3.))
                                            .min_length(px(24.))
                                            .bg(sb_thumb_bg)
                                    })
                            })
                            .max_fps(60),
                    ),
            )
            .child(
                div()
                    .id("logger-horizontal-scrollbar")
                    .test_support()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(h_sb_height)
                    .bg(sb_track_bg)
                    .border_t_1()
                    .border_color(sb_edge)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .right(v_sb_width)
                            .child(
                                Scrollbar::horizontal(
                                    &self.extra_tools.logger_filters.h_scroll_handle,
                                )
                                .viewport_from_layout()
                                .mode(ScrollbarMode::Always)
                                .styles(move |styles| {
                                    styles
                                        .track(|s| s.width(sb_size).bg(sb_track_bg))
                                        .track_hover(|s| s.width(sb_size).bg(sb_track_bg))
                                        .track_active(|s| s.width(sb_size).bg(sb_track_bg))
                                        .thumb(|s| {
                                            s.width(px(6.))
                                                .inset(px(2.))
                                                .radius(px(3.))
                                                .min_length(px(24.))
                                                .bg(sb_thumb_bg)
                                        })
                                        .thumb_hover(|s| {
                                            s.width(px(6.))
                                                .inset(px(2.))
                                                .radius(px(3.))
                                                .min_length(px(24.))
                                                .bg(sb_thumb_bg)
                                        })
                                        .thumb_active(|s| {
                                            s.width(px(6.))
                                                .inset(px(2.))
                                                .radius(px(3.))
                                                .min_length(px(24.))
                                                .bg(sb_thumb_bg)
                                        })
                                }),
                            ),
                    ),
            );

        let bottom_pane: AnyElement = if self.extra_tools.logger_selected.is_some() {
            div()
                .size_full()
                .min_h_0()
                .child(self.render_http_notes(
                    self.render_context_message_pair(
                        "logger",
                        &self.extra_tools.logger_request,
                        &self.extra_tools.logger_response,
                        true,
                        cx,
                    ),
                    cx,
                ))
                .into_any_element()
        } else {
            div()
                .id("logger-empty-detail")
                .test_support()
                .size_full()
                .min_h_0()
                .bg(theme::surface())
                .into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::surface())
            .child(self.render_logger_filter_header(&search_term, cx))
            .child(
                div().flex_1().min_h_0().child(
                    v_resizable("logger-main-vsplit")
                        .child(
                            resizable_panel()
                                .size(px(270.))
                                .size_range(px(110.)..px(800.))
                                .child(top_table),
                        )
                        .child(
                            resizable_panel()
                                .size_range(px(160.)..px(10000.))
                                .child(bottom_pane),
                        ),
                ),
            )
            .into_any_element()
    }
}
