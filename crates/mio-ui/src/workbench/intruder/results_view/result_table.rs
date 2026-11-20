use std::ops::Range;

use gpui_kit::component::{
    menu::PopupMenu,
    table::{Column, ColumnSort, TableDelegate},
};
use mio_core::{FlowState, IntruderResultOutcome, IntruderResultSort, IntruderResultSortColumn};

use super::*;

pub(super) mod menu;

fn highlight_fg(color: u8) -> gpui_kit::Hsla {
    gpui_kit::rgb(if matches!(color, 1 | 6 | 8) {
        0xffffff
    } else {
        0x111111
    })
    .into()
}

fn status_fg(status: u16, hl: Option<u8>) -> gpui_kit::Hsla {
    match hl {
        Some(1 | 6 | 8) => gpui_kit::rgb(0xffffff).into(),
        Some(_) => crate::editing::status_code_color(status, false, true),
        None => crate::editing::status_code_color(status, theme::is_dark(), false),
    }
}

impl ResultTable {
    pub(super) fn logical_ordinal(&self, visual: usize, cx: &App) -> Option<u64> {
        self.target
            .view(cx)?
            .query
            .row_at(visual)
            .map(|row| row.ordinal)
    }

    pub(super) fn visual_row(&self, ordinal: u64, cx: &App) -> Option<usize> {
        self.target.view(cx)?.query.rank_of(ordinal)
    }

    pub(super) fn apply_sort(&mut self, sort: IntruderResultSort) {
        let key: std::borrow::Cow<'static, str> = match sort.column {
            IntruderResultSortColumn::Request => "request".into(),
            IntruderResultSortColumn::Payload => "payload".into(),
            IntruderResultSortColumn::Status => "status".into(),
            IntruderResultSortColumn::ResponseReceived => "response_received".into(),
            IntruderResultSortColumn::Length => "length".into(),
            IntruderResultSortColumn::GrepMatch(idx) => format!("grep_match:{idx}").into(),
            IntruderResultSortColumn::GrepExtract(idx) => format!("grep_extract:{idx}").into(),
            IntruderResultSortColumn::GrepPayload(idx) => format!("grep_payload:{idx}").into(),
        };
        for column in &mut self.columns {
            if column.sort.is_some() {
                column.sort = Some(if column.key.as_ref() != key.as_ref() {
                    ColumnSort::Default
                } else if sort.ascending {
                    ColumnSort::Ascending
                } else {
                    ColumnSort::Descending
                });
            }
        }
    }

    pub(super) fn base_columns() -> Vec<Column> {
        vec![
            Column::new("request", "请求")
                .sortable()
                .width(80.)
                .movable(false),
            Column::new("payload", "Payload").sortable().width(220.),
            Column::new("status", "状态码").sortable().width(95.),
            Column::new("response_received", "响应接收")
                .sortable()
                .width(95.)
                .text_right(),
            Column::new("length", "长度")
                .sortable()
                .width(85.)
                .text_right(),
            Column::new("error", "错误").width(340.),
            Column::new("notes", "Notes").width(160.),
            Column::new("highlight", "Highlight").width(120.),
        ]
    }

    pub(super) fn sync_dynamic_columns(
        &mut self,
        max_positions: usize,
        grep_cfg: Option<&mio_core::IntruderGrepConfig>,
    ) -> bool {
        let mut desired = Self::base_columns();
        if max_positions > 1 {
            for pos_idx in 0..max_positions {
                desired.push(
                    Column::new(
                        SharedString::from(format!("payload_pos:{pos_idx}")),
                        format!("Payload #{}", pos_idx + 1),
                    )
                    .width(140.),
                );
            }
        }
        if let Some(cfg) = grep_cfg {
            for (m_idx, expr) in cfg.match_expressions.iter().enumerate() {
                let short = theme::truncate_cell_preview(expr);
                desired.push(
                    Column::new(
                        SharedString::from(format!("grep_match:{m_idx}")),
                        format!("匹配: {short}"),
                    )
                    .sortable()
                    .width(120.),
                );
            }
            for (e_idx, rule) in cfg.extract_rules.iter().enumerate() {
                let short = theme::truncate_cell_preview(&rule.key_label());
                desired.push(
                    Column::new(
                        SharedString::from(format!("grep_extract:{e_idx}")),
                        format!("提取: {short}"),
                    )
                    .sortable()
                    .width(150.),
                );
            }
            if cfg.grep_payloads {
                let payload_cols = max_positions.max(1);
                for pos_idx in 0..payload_cols {
                    let title = if max_positions > 1 {
                        format!("回显 #{}", pos_idx + 1)
                    } else {
                        "回显 (Payloads)".into()
                    };
                    desired.push(
                        Column::new(SharedString::from(format!("grep_payload:{pos_idx}")), title)
                            .sortable()
                            .width(120.),
                    );
                }
            }
        }
        let same_keys = self.columns.len() == desired.len()
            && self
                .columns
                .iter()
                .zip(&desired)
                .all(|(a, b)| a.key == b.key && a.name == b.name);
        if same_keys {
            false
        } else {
            self.columns = desired;
            true
        }
    }

    pub(super) fn export_loaded_rows_delimited(
        &self,
        csv: bool,
        cx: &App,
    ) -> Option<(String, usize)> {
        self.export_rows_delimited_filtered(None, csv, cx)
    }

    pub(super) fn export_rows_delimited_filtered(
        &self,
        only_ordinals: Option<&std::collections::BTreeSet<u64>>,
        csv: bool,
        cx: &App,
    ) -> Option<(String, usize)> {
        let view = self.target.view(cx).filter(|view| !view.discarded)?;
        let page = view.query.page.as_ref()?;
        if page.total == 0 {
            return None;
        }
        let mut export_keys: Vec<(&str, &str)> = vec![
            ("request", "Request #"),
            ("payload", "Payload"),
            ("status", "Status"),
            ("response_received", "Response received"),
            ("length", "Length"),
            ("highlight", "Highlight"),
            ("notes", "Notes"),
        ];
        for col in &self.columns {
            let key = col.key.as_ref();
            if key.starts_with("payload_pos:")
                || key.starts_with("grep_match:")
                || key.starts_with("grep_extract:")
                || key.starts_with("grep_payload:")
            {
                export_keys.push((key, col.name.as_ref()));
            }
        }
        let sep = if csv { "," } else { "\t" };
        let escape_cell = |val: &str| -> String {
            let clean = val.replace("\r\n", " ").replace(['\r', '\n', '\t'], " ");
            if csv && clean.contains([',', '"']) {
                format!("\"{}\"", clean.replace('"', "\"\""))
            } else {
                clean
            }
        };
        let mut lines = Vec::new();
        let header_line: Vec<String> = export_keys
            .iter()
            .map(|(_, title)| escape_cell(title))
            .collect();
        lines.push(header_line.join(sep));
        let mut exported = 0usize;
        for rank in 0..page.total {
            let Some(row_entry) = view.query.row_at(rank) else {
                continue;
            };
            if only_ordinals.is_some_and(|set| !set.contains(&row_entry.ordinal)) {
                continue;
            }
            let fields: Vec<String> = export_keys
                .iter()
                .map(|(key, _)| {
                    let idx = self
                        .columns
                        .iter()
                        .position(|c| c.key.as_ref() == *key)
                        .expect("export col");
                    escape_cell(&self.cell(rank, idx, cx))
                })
                .collect();
            lines.push(fields.join(sep));
            exported += 1;
        }
        (exported > 0).then(|| (lines.join("\n"), exported))
    }

    pub(super) fn export_loaded_rows_tsv(&self, cx: &App) -> Option<(String, usize)> {
        self.export_loaded_rows_delimited(false, cx)
    }

    fn cell(&self, rank: usize, column: usize, cx: &App) -> String {
        let Some(view) = self.target.view(cx) else {
            return String::new();
        };
        let Some(result) = view.query.row_at(rank) else {
            return if column == 0 {
                "正在读取…".into()
            } else {
                String::new()
            };
        };
        let ordinal = result.ordinal;
        let col_key = self.columns[column].key.as_ref();
        if col_key == "payload" {
            return payload_label(result);
        }
        if let Some(pos_str) = col_key.strip_prefix("payload_pos:")
            && let Ok(pos_idx) = pos_str.parse::<usize>()
        {
            return result
                .payloads
                .get(pos_idx)
                .map(payload_preview)
                .unwrap_or_default();
        }
        let number = view
            .summary
            .as_ref()
            .and_then(|run| run.request_number(ordinal))
            .map_or_else(|| "编号未确认".into(), |n| n.to_string());
        let Some(row) = result.result.as_ref() else {
            return if column == 0 {
                format!("{number} · 未读取")
            } else {
                String::new()
            };
        };
        if let Some(m_str) = col_key.strip_prefix("grep_match:")
            && let Ok(m_idx) = m_str.parse::<usize>()
        {
            let cfg = self.target.workbench.upgrade().and_then(|wb| {
                let tab_id = view.summary.as_ref()?.tab_id;
                wb.read(cx)
                    .workspace
                    .intruder
                    .grep_configs
                    .get(&tab_id)
                    .cloned()
            });
            let expr = cfg.as_ref().and_then(|c| c.match_expressions.get(m_idx));
            if let (Some(cfg), Some(expr)) = (cfg.as_ref(), expr) {
                if view.selected == Some(ordinal)
                    && let Some(raw) = view.detail_response_raw.as_deref()
                {
                    let single = mio_core::IntruderGrepConfig {
                        match_expressions: vec![expr.clone()],
                        match_case_sensitive: cfg.match_case_sensitive,
                        extract_rules: Vec::new(),
                        grep_payloads: false,
                        grep_payloads_case_sensitive: false,
                    };
                    let (matched, _) = mio_core::intruder::evaluate_grep_config(raw, &single);
                    return if matched.is_empty() {
                        "否".into()
                    } else {
                        "是".into()
                    };
                }
                if result.grep_matches.iter().any(|m| m == expr) {
                    return "是".into();
                }
            } else if !result.grep_matches.is_empty() {
                return "是".into();
            }
            if matches!(&row.outcome, IntruderResultOutcome::Flow(flow) if flow.status.is_some()) {
                return "否".into();
            }
            return String::new();
        }
        if let Some(e_str) = col_key.strip_prefix("grep_extract:")
            && let Ok(e_idx) = e_str.parse::<usize>()
        {
            let cfg = self.target.workbench.upgrade().and_then(|wb| {
                let tab_id = view.summary.as_ref()?.tab_id;
                wb.read(cx)
                    .workspace
                    .intruder
                    .grep_configs
                    .get(&tab_id)
                    .cloned()
            });
            let rule = cfg.as_ref().and_then(|c| c.extract_rules.get(e_idx));
            if let Some(rule) = rule {
                let key_label = rule.key_label();
                if let Some((_, val)) = result
                    .grep_extracts
                    .iter()
                    .find(|(k, _)| *k == key_label)
                    .or_else(|| result.grep_extracts.get(e_idx))
                {
                    return theme::truncate_cell_preview(val);
                }
                if view.selected == Some(ordinal)
                    && let Some(raw) = view.detail_response_raw.as_deref()
                {
                    let single = mio_core::IntruderGrepConfig {
                        match_expressions: Vec::new(),
                        match_case_sensitive: false,
                        extract_rules: vec![rule.clone()],
                        grep_payloads: false,
                        grep_payloads_case_sensitive: false,
                    };
                    let pairs = mio_core::intruder::evaluate_grep_extract_pairs(raw, &single);
                    return pairs
                        .into_iter()
                        .next()
                        .map(|(_, val)| theme::truncate_cell_preview(&val))
                        .unwrap_or_default();
                }
            } else if let Some((_, val)) = result.grep_extracts.get(e_idx) {
                return theme::truncate_cell_preview(val);
            }
            return String::new();
        }
        if let Some(p_str) = col_key.strip_prefix("grep_payload:")
            && let Ok(pos_idx) = p_str.parse::<usize>()
        {
            if let Some(hit) = result.grep_payload_hits.get(pos_idx).copied() {
                return if hit { "是".into() } else { "否".into() };
            }
            if view.selected == Some(ordinal)
                && let Some(raw) = view.detail_response_raw.as_deref()
                && let Some(payload) = result.payloads.get(pos_idx)
            {
                let case_sensitive = self
                    .target
                    .workbench
                    .upgrade()
                    .and_then(|wb| {
                        let tab_id = view.summary.as_ref()?.tab_id;
                        wb.read(cx)
                            .workspace
                            .intruder
                            .grep_configs
                            .get(&tab_id)
                            .map(|c| c.grep_payloads_case_sensitive)
                    })
                    .unwrap_or(false);
                let hits = mio_core::intruder::evaluate_grep_payload_hits(
                    raw,
                    &[payload.prefix.as_slice()],
                    case_sensitive,
                );
                if let Some(&hit) = hits.first() {
                    return if hit { "是".into() } else { "否".into() };
                }
            }
            if matches!(&row.outcome, IntruderResultOutcome::Flow(flow) if flow.status.is_some()) {
                return "否".into();
            }
            return String::new();
        }
        match col_key {
            "request" => number,
            "status" => match &row.outcome {
                IntruderResultOutcome::Flow(flow) => {
                    flow.status.map(|s| s.to_string()).unwrap_or_default()
                }
                IntruderResultOutcome::Rejected(_) => String::new(),
            },
            "response_received" => match &row.outcome {
                IntruderResultOutcome::Flow(flow) if flow.status.is_some() => {
                    flow.elapsed_ms.to_string()
                }
                _ => String::new(),
            },
            "length" => match &row.outcome {
                IntruderResultOutcome::Flow(flow) if flow.status.is_some() => result
                    .response_length
                    .unwrap_or(flow.response_bytes)
                    .to_string(),
                _ => String::new(),
            },
            "error" => match &row.outcome {
                IntruderResultOutcome::Flow(flow) => match &flow.state {
                    FlowState::Failed(error) => error.clone(),
                    FlowState::Cancelled | FlowState::Dropped => flow.state.label().into(),
                    _ => String::new(),
                },
                IntruderResultOutcome::Rejected(error) => format!("未发送：{error}"),
            },
            "notes" => theme::truncate_cell_preview(
                &result
                    .notes_preview
                    .replace("\r\n", " ")
                    .replace(['\r', '\n', '\t'], " "),
            ),
            "highlight" => result
                .highlight
                .map_or_else(|| "None".into(), |c| c.to_string()),
            _ => unreachable!("result columns are constructed with their renderers"),
        }
    }
}

fn selected_row_background(color: u8) -> gpui_kit::Hsla {
    if color == 0 && theme::is_dark() {
        return theme::selected();
    }
    gpui_kit::rgb(match color {
        1 => 0xdc3737,
        2 => 0xd99f1f,
        3 => 0xd7d600,
        4 => 0x0dd80a,
        5 => 0x01d7d7,
        6 => 0x3d3cde,
        7 => 0xda9f9f,
        8 => 0xdc33dd,
        9 => 0x8d8d8d,
        _ => 0xeaf1f6,
    })
    .into()
}

impl TableDelegate for ResultTable {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
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

    fn rows_count(&self, cx: &App) -> usize {
        self.target
            .view(cx)
            .filter(|view| !view.discarded)
            .and_then(|view| view.query.page.as_ref())
            .map_or(0, |page| page.total)
    }

    fn perform_sort(
        &mut self,
        column: usize,
        sort: ColumnSort,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let key = self.columns[column].key.as_ref();
        let column = match key {
            "request" => IntruderResultSortColumn::Request,
            "payload" => IntruderResultSortColumn::Payload,
            "status" => IntruderResultSortColumn::Status,
            "response_received" => IntruderResultSortColumn::ResponseReceived,
            "length" => IntruderResultSortColumn::Length,
            _ if key.starts_with("grep_match:") => {
                let Ok(idx) = key["grep_match:".len()..].parse::<usize>() else {
                    return;
                };
                IntruderResultSortColumn::GrepMatch(idx)
            }
            _ if key.starts_with("grep_extract:") => {
                let Ok(idx) = key["grep_extract:".len()..].parse::<usize>() else {
                    return;
                };
                IntruderResultSortColumn::GrepExtract(idx)
            }
            _ if key.starts_with("grep_payload:") => {
                let Ok(idx) = key["grep_payload:".len()..].parse::<usize>() else {
                    return;
                };
                IntruderResultSortColumn::GrepPayload(idx)
            }
            _ => return,
        };
        let target = self.target.clone();
        let handle = window.window_handle();
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                target.update(window, cx, |view, window, cx| {
                    let filter = view.intruder.results.views[&target.view_id]
                        .query
                        .requested()
                        .0;
                    let next_sort = IntruderResultSort {
                        column,
                        ascending: sort == ColumnSort::Ascending,
                    };
                    view.query_intruder_result_view(
                        target.view_id,
                        0,
                        filter,
                        next_sort,
                        window,
                        cx,
                    );
                })
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
        cx: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let (ordinal, selected, highlight) = self
            .target
            .view(cx)
            .and_then(|view| {
                let entry = view.query.row_at(row)?;
                let is_selected = view.selected == Some(entry.ordinal)
                    || view.selected_ordinals.contains(&entry.ordinal);
                Some((
                    Some(entry.ordinal),
                    is_selected,
                    entry.highlight.filter(|c| (1..=9).contains(c)),
                ))
            })
            .unwrap_or((None, false, None));
        let tr = if let Some(ordinal) = ordinal {
            div().id(("intruder-result-row", ordinal as usize))
        } else {
            div().id(("intruder-result-loading-row", row))
        };
        let left_target = self.target.clone();
        let right_target = self.target.clone();
        tr.aria_selected(selected)
            .when(ordinal.is_some(), |this| {
                this.on_mouse_down(gpui_kit::MouseButton::Left, move |event, window, cx| {
                    let shift = event.modifiers.shift;
                    let toggle = event.modifiers.secondary(cx);
                    left_target.update(window, cx, |workbench, _, _| {
                        if let Some(view) = workbench
                            .intruder
                            .results
                            .views
                            .get_mut(&left_target.view_id)
                        {
                            view.pending_pointer =
                                Some((row, gpui_kit::MouseButton::Left, shift, toggle));
                        }
                    });
                })
                .on_mouse_down(
                    gpui_kit::MouseButton::Right,
                    move |event, window, cx| {
                        let shift = event.modifiers.shift;
                        let toggle = event.modifiers.secondary(cx);
                        right_target.update(window, cx, |workbench, _, _| {
                            if let Some(view) = workbench
                                .intruder
                                .results
                                .views
                                .get_mut(&right_target.view_id)
                            {
                                view.pending_pointer =
                                    Some((row, gpui_kit::MouseButton::Right, shift, toggle));
                            }
                        });
                    },
                )
            })
            .when(selected, |this| {
                this.border_1().border_color(theme::control_border())
            })
            .when_some(highlight, |this, color| {
                this.when_some(crate::history::highlight_background(color), |this, bg| {
                    this.bg(bg).text_color(highlight_fg(color))
                })
            })
            .when(selected, |this| {
                let color = highlight.unwrap_or(0);
                this.bg(selected_row_background(color))
                    .text_color(if color == 0 {
                        theme::foreground()
                    } else {
                        highlight_fg(color)
                    })
            })
            .when(!selected && highlight.is_none(), |this| {
                this.hover(|this| {
                    this.bg(gpui_kit::rgb(if theme::is_dark() {
                        0x26334a
                    } else {
                        0xedf4fa
                    }))
                })
            })
    }

    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col_key = self.columns.get(column).map_or("", |col| col.key.as_ref());
        let is_right = matches!(col_key, "response_received" | "length");
        let status_color = (col_key == "status")
            .then(|| {
                let entry = self.target.view(cx)?.query.row_at(row)?;
                let hl = entry.highlight.filter(|c| (1..=9).contains(c));
                let IntruderResultOutcome::Flow(flow) = &entry.result.as_ref()?.outcome else {
                    return None;
                };
                flow.status.map(|code| status_fg(code, hl))
            })
            .flatten();
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
            .child(div().min_w_0().truncate().child(self.cell(row, column, cx)))
    }

    fn cell_label(&self, row: usize, column: usize, cx: &App) -> Option<SharedString> {
        Some(self.cell(row, column, cx).into())
    }

    fn context_menu(
        &mut self,
        row: usize,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        menu::build_result_row_context_menu(self, row, menu, window, cx)
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

    fn visible_rows_changed(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        if !range.is_empty() {
            let view_id = self.target.view_id;
            self.target.update(window, cx, |view, window, cx| {
                view.ensure_intruder_result_range(view_id, range, window, cx)
            });
        }
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let text = self.target.view(cx).map_or("运行已关闭", |view| {
            if view.discarded {
                "此攻击已丢弃"
            } else if view.query.pending.is_some() || view.pending.is_some() {
                "正在读取结果…"
            } else if view.query.error.is_some() || view.error.is_some() {
                "结果读取失败"
            } else {
                "没有符合筛选条件的结果"
            }
        });
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme::muted_foreground())
            .child(text)
    }
}
