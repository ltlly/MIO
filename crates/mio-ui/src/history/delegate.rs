use super::*;
use gpui_kit::TestSupportExt;
use gpui_kit::assets::IconName;
use gpui_kit::component::Sizable;

fn row_highlight_color(delegate: &HistoryDelegate, row: usize) -> Option<u8> {
    delegate
        .rows
        .get(row)
        .and_then(|entry| match entry {
            HistoryRow::ExtProxy(row) => row.highlight.as_ref().map(|h| h.color),
            HistoryRow::Live(flow) => delegate
                .live_highlights
                .borrow()
                .get(&flow.id)
                .map(|h| h.color),
        })
        .filter(|color| (1..=9).contains(color))
}

const HIGHLIGHT_COMBO_NAMES: [&str; 10] = [
    "NONE", "RED", "ORANGE", "YELLOW", "GREEN", "CYAN", "BLUE", "PINK", "MAGENTA", "GRAY",
];

const MIO_ADD_TO_SITEMAP_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M3.5 9.2V3h3.3"/><path d="M3.5 7h3.3"/><rect x="1.6" y="9.2" width="3.8" height="3.2" rx="0.6"/><rect x="6.8" y="1.6" width="5.4" height="2.8" rx="0.6"/><rect x="6.8" y="5.6" width="5.4" height="2.8" rx="0.6"/></svg>"#;

fn normalize_history_menu_url(raw: &str) -> String {
    if let Ok(mut parsed) = url::Url::parse(raw) {
        let default_port = match parsed.scheme() {
            "http" if parsed.port() == Some(80) => true,
            "https" if parsed.port() == Some(443) => true,
            _ => false,
        };
        if default_port {
            let _ = parsed.set_port(None);
        }
        return parsed.into();
    }
    raw.to_owned()
}

fn estimate_column_text_width(text: &str) -> f32 {
    text.chars()
        .map(|ch| if ch.is_ascii() { 7.2 } else { 13.5 })
        .sum()
}

impl TableDelegate for HistoryDelegate {
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
        let min_w = col.min_width.as_f32().max(36.0);
        let max_w = col.max_width.as_f32().min(520.0).max(min_w);
        let mut widest = estimate_column_text_width(col.name.as_ref())
            + if col.sort.is_some() { 18.0 } else { 4.0 };
        let is_number = col.key.as_ref() == "number";
        let sample_len = self.rows.len().min(250);
        for row_ix in 0..sample_len {
            let cell_str = crate::theme::truncate_cell_preview(&self.cell(row_ix, col_ix));
            let extra = if is_number { 8.0 } else { 0.0 };
            widest = widest.max(estimate_column_text_width(&cell_str) + extra);
        }
        let unscaled = (widest + 6.0).round().clamp(min_w, max_w);
        self.columns[col_ix].width = gpui_kit::px(unscaled);
        let scale = crate::theme::ui_scale();
        Some(gpui_kit::px((unscaled * scale).round()))
    }
    fn column(&self, index: usize, _: &App) -> Column {
        let scale = crate::theme::ui_scale();
        let mut col = self.columns[index].clone();
        if (scale - 1.0).abs() > f32::EPSILON {
            let width = col.width.as_f32();
            col = col.width(gpui_kit::px((width * scale).round()));
        }
        if let Some(paddings) = col.paddings {
            col.paddings(gpui_kit::Edges {
                top: gpui_kit::px(0.),
                bottom: gpui_kit::px(0.),
                left: crate::theme::scaled_px(paddings.left.as_f32()),
                right: crate::theme::scaled_px(paddings.right.as_f32()),
            })
        } else {
            col.paddings(gpui_kit::Edges {
                top: gpui_kit::px(0.),
                bottom: gpui_kit::px(0.),
                left: crate::theme::scaled_px(4.),
                right: crate::theme::scaled_px(4.),
            })
        }
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
        let Some(column) = super::sort_column_from_key(self.columns[col].key.as_ref()) else {
            return;
        };
        let next_sort = HttpHistorySort {
            column,
            ascending: sort == ColumnSort::Ascending,
        };
        self.apply_sort(next_sort);
        let Some(owner) = self.owner.clone() else {
            return;
        };
        let handle = window.window_handle();
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    view.sort_http_history(next_sort, window, cx)
                })
            });
        });
    }

    fn row_selection_managed_by_delegate(&self) -> bool {
        true
    }

    fn has_more(&self, _: &App) -> bool {
        !self.loading
            && self.offset == 0
            && self.rows.len() >= HTTP_HISTORY_BATCH_SIZE
            && self.rows.len() < self.total
            && self.rows.len() < 100_000
    }

    fn load_more_threshold(&self) -> usize {
        100
    }

    fn load_more(&mut self, window: &mut Window, cx: &mut Context<TableState<Self>>) {
        if self.loading
            || self.offset != 0
            || self.rows.len() < HTTP_HISTORY_BATCH_SIZE
            || self.rows.len() >= self.total
            || self.rows.len() >= 100_000
        {
            return;
        }
        let Some(owner) = self.owner.clone() else {
            return;
        };
        self.loading = true;
        let handle = window.window_handle();
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                owner.update(cx, |view, cx| {
                    view.load_more_http_history(window, cx);
                })
            });
        });
    }

    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let highlight = self.rows.get(row).and_then(|row| match row {
            HistoryRow::ExtProxy(row) => row.highlight.clone(),
            HistoryRow::Live(flow) => self.live_highlights.borrow().get(&flow.id).cloned(),
        });
        let hl_color = highlight
            .as_ref()
            .map(|h| h.color)
            .filter(|c| (1..=9).contains(c));
        let id = self.rows.get(row).map(HistoryRow::id);
        let selected = id.is_some_and(|id| self.selected_ids.contains(&id));
        let lead = id == self.selection_lead;
        let owner = self.owner.clone();
        div()
            .id(("http-history-row", row))
            .relative()
            .aria_selected(selected)
            .map(|this| {
                if selected && lead {
                    this.border_t_1()
                        .border_b_1()
                        .border_color(gpui_kit::rgb(0x26649d))
                } else {
                    this.border_b_0()
                }
            })
            .when(selected && lead, |this| {
                this.child(
                    div()
                        .id(("history-row-lead-left-border", row))
                        .test_support()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(gpui_kit::px(1.))
                        .bg(gpui_kit::rgb(0x2f6499)),
                )
            })
            .when_some(id.zip(owner), |this, (id, owner)| {
                let right_owner = owner.clone();
                this.on_mouse_down(gpui_kit::MouseButton::Left, move |event, window, cx| {
                    let owner = owner.clone();
                    let handle = window.window_handle();
                    let modifiers = event.modifiers;
                    cx.defer(move |cx| {
                        let _ = handle.update(cx, |_, window, cx| {
                            owner.update(cx, |view, cx| {
                                view.history_pointer_selection(
                                    id,
                                    gpui_kit::MouseButton::Left,
                                    modifiers,
                                    window,
                                    cx,
                                )
                            })
                        });
                    });
                })
                .on_mouse_down(
                    gpui_kit::MouseButton::Right,
                    move |event, window, cx| {
                        let owner = right_owner.clone();
                        let handle = window.window_handle();
                        let modifiers = event.modifiers;
                        cx.defer(move |cx| {
                            let _ = handle.update(cx, |_, window, cx| {
                                owner.update(cx, |view, cx| {
                                    view.history_pointer_selection(
                                        id,
                                        gpui_kit::MouseButton::Right,
                                        modifiers,
                                        window,
                                        cx,
                                    )
                                })
                            });
                        });
                    },
                )
            })
            .when_some(hl_color, |this, color| {
                this.when_some(highlight_background(color), |this, background| {
                    this.bg(background)
                        .text_color(highlight_foreground(color))
                })
            })
            .when(selected, |this| {
                let color = hl_color.unwrap_or(0);
                let bg = if color == 0 && crate::theme::is_dark() {
                    crate::theme::selected()
                } else {
                    selected_highlight_background(color, lead)
                };
                let fg = if color == 0 {
                    crate::theme::foreground()
                } else {
                    highlight_foreground(color)
                };
                this.bg(bg).text_color(fg)
            })
    }

    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        use gpui_kit::component::button::{Button, ButtonRounded, ButtonVariants};
        use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
        let hl_color = row_highlight_color(self, row);
        let text = crate::theme::truncate_cell_preview(&self.cell(row, column));
        let fg = match hl_color {
            Some(color) => highlight_foreground(color),
            None => crate::theme::foreground(),
        };
        let entry_id = self.rows.get(row).map(HistoryRow::id);
        let is_number_col = self.columns.get(column).is_some_and(|col| col.key.as_ref() == "number");
        let is_lead_number_cell = is_number_col
            && entry_id.is_some_and(|id| self.selected_ids.contains(&id) && self.selection_lead == Some(id));
        let combo_owner = if is_lead_number_cell {
            self.owner.clone()
        } else {
            None
        };
        let targets = if combo_owner.is_some() {
            self.highlight_targets(row)
        } else {
            Vec::new()
        };
        let active_color = hl_color.unwrap_or(0);
        let can_highlight = combo_owner
            .as_ref()
            .and_then(WeakEntity::upgrade)
            .is_some_and(|entity| entity.read(cx).project.is_some());
        let show_combo = combo_owner
            .as_ref()
            .is_some_and(|_| can_highlight && !targets.is_empty());
        let is_combo_open = show_combo && self.open_highlight_combo_row == Some(row);
        let number_label = text.clone();
        let display_text: SharedString = if is_combo_open {
            "...".into()
        } else {
            text.into()
        };
        let cell_fg = if is_combo_open {
            gpui_kit::rgb(0x000000).into()
        } else {
            fg
        };
        let td = div()
            .h_full()
            .w_full()
            .when(is_number_col, |this| this.relative())
            .when(is_combo_open, |this| {
                this.text_color(gpui_kit::rgb(0x000000)).child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(gpui_kit::px(-5.))
                        .right_0()
                        .bg(gpui_kit::rgb(0xffffff))
                        .border_1()
                        .border_color(gpui_kit::rgb(0x2b2b2b)),
                )
            })
            .flex()
            .items_center()
            .justify_between()
            .gap(if is_number_col {
                gpui_kit::px(0.)
            } else {
                crate::theme::scaled_px(2.)
            })
            .font_weight(gpui_kit::FontWeight::NORMAL)
            .when(hl_color.is_none() && !is_combo_open, |this| {
                this.text_color(crate::theme::foreground())
            })
            .child(
                div()
                    .id(("history-cell-text", (row << 8) | column))
                    .test_support()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .when(is_number_col && show_combo, |this| this.pr(gpui_kit::px(6.)))
                    .truncate()
                    .child(display_text),
            );
        if is_number_col {
            let table_entity = cx.entity();
            td.when_some(combo_owner.filter(|_| can_highlight && !targets.is_empty()), |this, owner| {
                this.child(
                    div()
                        .absolute()
                        .right_0()
                        .top_0()
                        .bottom_0()
                        .w(gpui_kit::px(6.))
                        .child(
                            Button::new(("history-row-highlight-combo", row))
                                .ghost()
                                .small()
                                .rounded(ButtonRounded::None)
                                .w(gpui_kit::px(6.))
                                .h_full()
                                .p_0()
                                .flex_shrink_0()
                                .when(is_combo_open, |b| {
                                    b.icon(
                                        gpui_kit::component::Icon::default()
                                            .data(b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 6 5\" fill=\"currentColor\"><path d=\"M0.5 1h5L3 4.2Z\"/></svg>")
                                            .size(gpui_kit::px(5.))
                                            .text_color(cell_fg),
                                    )
                                })
                                .accessibility_label("Highlight row color")
                                .dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                                    (0..=9u8).fold(
                                        menu.flush_swatches(),
                                        |menu, color| {
                                            let owner = owner.clone();
                                            let ids = targets.clone();
                                            let color_name = HIGHLIGHT_COMBO_NAMES[usize::from(color)];
                                            let swatch_label = number_label.clone();
                                            menu.item(
                                                PopupMenuItem::element(move |_, _| {
                                                    div()
                                                        .id(("http-highlight-color", usize::from(color)))
                                                        .test_support()
                                                        .role(gpui_kit::accesskit::Role::MenuItem)
                                                        .aria_label(color_name)
                                                        .w(gpui_kit::px(50.))
                                                        .h(gpui_kit::px(21.))
                                                        .px(gpui_kit::px(4.))
                                                        .flex()
                                                        .items_center()
                                                        .when(color == 0, |this| {
                                                            this.bg(gpui_kit::rgb(0xffffff))
                                                        })
                                                        .when_some(
                                                            highlight_background(color),
                                                            |this, background| this.bg(background),
                                                        )
                                                        .when(color == active_color, |this| {
                                                            this.border_1()
                                                                .border_color(highlight_foreground(color))
                                                        })
                                                        .text_color(highlight_foreground(color))
                                                        .whitespace_nowrap()
                                                        .child(swatch_label.clone())
                                                })
                                                .on_click(move |_, _, cx| {
                                                    let _ = owner.update(cx, |view, cx| {
                                                        if view.project.is_some() {
                                                            view.set_http_highlights(&ids, color, cx);
                                                        }
                                                    });
                                                }),
                                            )
                                        },
                                    )
                                })
                                .offset(gpui_kit::px(1.))
                                .margin(gpui_kit::px(0.))
                                .trigger_style(gpui_kit::StyleRefinement::default().size_full())
                                .on_open_change(move |open, _, cx| {
                                    let is_open = *open;
                                    table_entity.update(cx, |table, cx| {
                                        let current = table.delegate().open_highlight_combo_row;
                                        let next = if is_open {
                                            Some(row)
                                        } else if current == Some(row) {
                                            None
                                        } else {
                                            current
                                        };
                                        if current != next {
                                            table.delegate_mut().open_highlight_combo_row = next;
                                            cx.notify();
                                        }
                                    });
                                }),
                        ),
                )
            })
        } else {
            td
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
        cx: &mut Context<TableState<Self>>,
    ) -> bool {
        let Some(col) = self.columns.get(col_ix) else {
            return true;
        };
        if col.key.as_ref() == "number" || self.columns.len() <= 1 {
            return true;
        }
        let key = col.key.to_string();
        if let Some(owner) = self.owner.clone() {
            cx.defer(move |cx| {
                let _ = owner.update(cx, |view, cx| {
                    if view
                        .history
                        .read(cx)
                        .delegate()
                        .columns
                        .iter()
                        .any(|c| c.key.as_ref() == key)
                    {
                        view.toggle_history_column_visibility(&key, cx);
                    }
                });
            });
        } else {
            self.columns.remove(col_ix);
        }
        true
    }

    fn reset_columns(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> bool {
        if let Some(owner) = self.owner.clone() {
            cx.defer(move |cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.restore_default_history_columns(cx);
                });
            });
        } else {
            self.columns = Self::default().columns;
        }
        true
    }

    fn column_header_context_menu(
        &mut self,
        _col_ix: usize,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        use gpui_kit::component::menu::PopupMenuItem;

        let visible_keys: Vec<SharedString> =
            self.columns.iter().map(|c| c.key.clone()).collect();
        let all_cols = Self::all_columns();
        let owner = self.owner.clone();
        let table_entity = cx.entity().clone();
        let cols_submenu = PopupMenu::build(window, cx, move |mut sub, _, _| {
            let visible_count = visible_keys.len();
            for col in &all_cols {
                let key = col.key.clone();
                let is_visible = visible_keys.contains(&key);
                let is_number = key.as_ref() == "number";
                let can_toggle = !is_number && (!is_visible || visible_count > 1);
                let toggle_owner = owner.clone();
                let toggle_table = table_entity.clone();
                sub = sub.item(
                    PopupMenuItem::new(col.name.clone())
                        .checked(is_visible)
                        .disabled(!can_toggle)
                        .on_click(move |_, _, cx| {
                            if let Some(owner) = &toggle_owner {
                                let _ = owner.update(cx, |view, cx| {
                                    view.toggle_history_column_visibility(key.as_ref(), cx);
                                });
                            } else {
                                toggle_table.update(cx, |table, cx| {
                                    let delegate = table.delegate_mut();
                                    if delegate.columns.iter().any(|c| c.key == key) {
                                        if delegate.columns.len() > 1 {
                                            delegate.columns.retain(|c| c.key != key);
                                        }
                                    } else if let Some(def) =
                                        Self::all_columns().into_iter().find(|c| c.key == key)
                                    {
                                        delegate.columns.push(def);
                                    }
                                    table.refresh(cx);
                                });
                            }
                        }),
                );
            }
            let show_all_owner = owner.clone();
            let show_all_table = table_entity.clone();
            sub.separator().item(
                PopupMenuItem::new("显示全部列 (Show all columns)").on_click(move |_, _, cx| {
                    if let Some(owner) = &show_all_owner {
                        let _ = owner.update(cx, |view, cx| {
                            view.restore_default_history_columns(cx);
                        });
                    } else {
                        show_all_table.update(cx, |table, cx| {
                            table.delegate_mut().columns = Self::default().columns;
                            table.refresh(cx);
                        });
                    }
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
        if let Some(owner) = self.owner.clone() {
            cx.defer(move |cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.sync_history_all_columns(cx);
                });
            });
        }
        cx.notify();
    }

    fn on_column_widths_changed(
        &mut self,
        widths: &[gpui_kit::Pixels],
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let scale = crate::theme::ui_scale().max(1.0);
        for (ix, col) in self.columns.iter_mut().enumerate() {
            if let Some(&w) = widths.get(ix)
                && w > gpui_kit::px(0.)
            {
                col.width = gpui_kit::px((w.as_f32() / scale).round());
            }
        }
        if let Some(owner) = self.owner.clone() {
            cx.defer(move |cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.sync_history_all_columns(cx);
                });
            });
        }
    }

    fn context_menu(
        &mut self,
        row: usize,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        use gpui_kit::component::menu::PopupMenuItem;
        let targets = self.highlight_targets(row);
        let clicked_row = self.rows.get(row).cloned();
        let has_row = self.rows.get(row).is_some();
        let can_operate = has_row
            && self
                .owner
                .as_ref()
                .and_then(WeakEntity::upgrade)
                .is_some_and(|entity| entity.read(cx).can_operate_history_menu());
        let can_copy_har = can_operate
            && self
                .owner
                .as_ref()
                .and_then(WeakEntity::upgrade)
                .is_some_and(|entity| entity.read(cx).can_copy_proxy_row_har(self.rows.get(row)));
        let raw_row_url = self
            .rows
            .get(row)
            .and_then(|entry| match entry {
                HistoryRow::Live(flow) => (!flow.url.is_empty()).then(|| flow.url.clone()),
                HistoryRow::ExtProxy(archive) => {
                    let req = archive.request.as_ref().or(archive.edited_request.as_ref())?;
                    let target = req.request_target.as_deref().unwrap_or_default();
                    if target.is_empty() {
                        None
                    } else if target.starts_with('/') {
                        let origin = archive
                            .origin
                            .as_deref()
                            .map(format_ext_host_origin)
                            .unwrap_or_default();
                        (!origin.is_empty()).then(|| format!("{origin}{target}"))
                    } else {
                        Some(target.to_owned())
                    }
                }
            });
        let row_in_scope = raw_row_url.as_deref().is_some_and(|url| {
            self.owner
                .as_ref()
                .and_then(WeakEntity::upgrade)
                .is_some_and(|entity| entity.read(cx).is_history_url_in_scope(url))
        });
        let header_url_summary = raw_row_url
            .map(|raw| normalize_history_menu_url(&raw))
            .map(|full| {
                let chars: Vec<char> = full.chars().collect();
                if chars.len() <= 58 {
                    full
                } else {
                    let prefix: String = chars[..27].iter().collect();
                    let suffix: String = chars[chars.len() - 28..].iter().collect();
                    format!("{prefix}...{suffix}")
                }
            })
            .unwrap_or_else(|| "HTTP history item".into());
        let (existing_findings, plugin_items): (
            Vec<(mio_core::Uuid, String)>,
            Vec<(mio_core::Uuid, String)>,
        ) = self
            .owner
            .as_ref()
            .and_then(WeakEntity::upgrade)
            .map(|entity| {
                let view = entity.read(cx);
                (
                    view.existing_audit_finding_ids(),
                    view.plugin_context_menu_items(),
                )
            })
            .unwrap_or_default();
        let passive_owner = self.owner.clone();
        let launcher_owner = self.owner.clone();
        let links_owner = self.owner.clone();
        let browser_owner = self.owner.clone();
        let req_browser_owner = self.owner.clone();
        let resp_browser_owner = self.owner.clone();
        let save_xml_direct_owner = self.owner.clone();
        let org_owner = self.owner.clone();
        let ai_owner = self.owner.clone();
        let issue_owner = self.owner.clone();
        let attach_issue_owner = self.owner.clone();
        let new_win_owner = self.owner.clone();
        let doc_owner = self.owner.clone();
        let engage_refs_owner = self.owner.clone();
        let engage_discover_owner = self.owner.clone();
        let engage_sched_owner = self.owner.clone();
        let engage_csrf_owner = self.owner.clone();
        let engage_links_owner = self.owner.clone();
        let engage_har_owner = self.owner.clone();
        let plugin_owner = self.owner.clone();
        let row_for_new_issue = clicked_row.clone();
        let row_for_attach = clicked_row.clone();
        let attach_existing_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
            if existing_findings.is_empty() {
                sub.item(PopupMenuItem::new("暂无现有问题").disabled(true))
            } else {
                existing_findings.into_iter().fold(
                    sub.scrollable(true)
                        .max_h(crate::theme::scaled_px(320.)),
                    |sub, (finding_id, title)| {
                        let owner = attach_issue_owner.clone();
                        let row_attach = row_for_attach.clone();
                        sub.item(
                            PopupMenuItem::new(title)
                                .disabled(!can_operate)
                                .on_click(move |_, _, cx| {
                                    if let Some(owner) = &owner {
                                        let _ = owner.update(cx, |view, cx| {
                                            view.attach_selected_history_to_issue(
                                                finding_id,
                                                row_attach.clone(),
                                                cx,
                                            );
                                        });
                                    }
                                }),
                        )
                    },
                )
            }
        });
        let record_issue_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
            let owner = issue_owner.clone();
            let row_create = row_for_new_issue.clone();
            sub.item(
                PopupMenuItem::new("创建新问题 (Create a new issue)")
                    .disabled(!can_operate)
                    .on_click(move |_, _, cx| {
                        if let Some(owner) = &owner {
                            let _ = owner.update(cx, |view, cx| {
                                view.record_manual_issue_from_history(row_create.clone(), cx);
                            });
                        }
                    }),
            )
            .item(PopupMenuItem::submenu(
                "添加到现有问题 (Add to existing issue)",
                attach_existing_submenu,
            ))
        });
        let req_browser_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
            let cur_owner = req_browser_owner.clone();
            let sys_owner = browser_owner.clone();
            sub.item(
                PopupMenuItem::new("在原始会话中 (In original session)")
                    .disabled(!can_operate)
                    .on_click(move |_, window, cx| {
                        if let Some(owner) = &sys_owner {
                            let _ = owner.update(cx, |view, cx| {
                                view.open_current_request_in_browser_with_session(true, window, cx);
                            });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new("在当前浏览器会话中 (In current session)")
                    .disabled(!can_operate)
                    .on_click(move |_, window, cx| {
                        if let Some(owner) = &cur_owner {
                            let _ = owner.update(cx, |view, cx| {
                                view.open_current_request_in_configured_browser(window, cx);
                            });
                        }
                    }),
            )
        });
        let engagement_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
            let refs_o = engage_refs_owner.clone();
            let discover_o = engage_discover_owner.clone();
            let sched_o = engage_sched_owner.clone();
            let csrf_o = engage_csrf_owner.clone();
            let links_o = engage_links_owner.clone();
            let har_o = engage_har_owner.clone();
            sub.item(
                PopupMenuItem::new("查找引用 (Find references)")
                    .disabled(!can_operate)
                    .on_click(move |_, window, cx| {
                        if let Some(owner) = &refs_o {
                            let _ = owner.update(cx, |view, cx| {
                                view.engagement_find_references(window, cx);
                            });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new("发现内容 (Discover content)")
                    .disabled(!can_operate)
                    .on_click(move |_, window, cx| {
                        if let Some(owner) = &discover_o {
                            let _ = owner.update(cx, |view, cx| {
                                view.engagement_discover_content(window, cx);
                            });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new("计划任务 (Schedule task)")
                    .disabled(!can_operate)
                    .on_click(move |_, window, cx| {
                        if let Some(owner) = &sched_o {
                            let _ = owner.update(cx, |view, cx| {
                                view.engagement_schedule_task(window, cx);
                            });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new("生成 CSRF PoC (Generate CSRF PoC)")
                    .disabled(!can_operate)
                    .on_click(move |_, window, cx| {
                        if let Some(owner) = &csrf_o {
                            let _ = owner.update(cx, |view, cx| {
                                view.engagement_generate_csrf_poc(window, cx);
                            });
                        }
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new(if row_in_scope {
                    "从范围中排除 (Exclude from scope)"
                } else {
                    "添加到范围"
                })
                .action(if row_in_scope {
                    Box::new(crate::ExcludeFromScope) as Box<dyn gpui_kit::Action>
                } else {
                    Box::new(AddToScope) as Box<dyn gpui_kit::Action>
                })
                .disabled(!can_operate),
            )
            .item(
                PopupMenuItem::new("提取页面链接 (Copy links in response)")
                    .disabled(!can_operate)
                    .on_click(move |_, window, cx| {
                        if let Some(owner) = &links_o {
                            let _ = owner.update(cx, |view, cx| {
                                view.copy_selected_history_links(window, cx);
                            });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new("复制为 HAR (JSON)")
                    .disabled(!can_copy_har)
                    .on_click(move |_, window, cx| {
                        if let Some(owner) = &har_o {
                            let _ = owner.update(cx, |view, cx| {
                                view.copy_selected_flow_har_from_history(window, cx);
                            });
                        }
                    }),
            )
        });
        let menu = menu
            .compact()
            .with_size(gpui_kit::component::Size::Size(gpui_kit::px(21.)))
            .scrollable(true)
            .max_h(gpui_kit::px(618.))
            .item(
                PopupMenuItem::element(move |_, _| {
                    div()
                        .id(("history-row-url-header", row))
                        .test_support()
                        .role(gpui_kit::accesskit::Role::MenuItem)
                        .aria_label(header_url_summary.clone())
                        .w_full()
                        .min_h(gpui_kit::px(21.))
                        .flex()
                        .items_center()
                        .font_weight(gpui_kit::FontWeight::BOLD)
                        .text_color(crate::theme::foreground())
                        .child(header_url_summary.clone())
                })
                .icon(gpui_kit::component::Icon::empty())
                .disabled(true),
            )
            .separator()
            .map(|menu| {
                if row_in_scope {
                    menu.menu_with_icon(
                        "从范围中排除 (Exclude from scope)",
                        IconName::Ban,
                        Box::new(crate::ExcludeFromScope),
                    )
                } else {
                    menu.menu_with_icon("添加到范围", IconName::CirclePlus, Box::new(AddToScope))
                }
            })
            .separator()
            .item(
                PopupMenuItem::new("打开扫描启动器 (Scan)")
                    .disabled(!can_operate)
                    .on_click(move |_, window, cx| {
                        if let Some(owner) = &launcher_owner {
                            let _ = owner.update(cx, |view, cx| {
                                view.open_scan_launcher(window, cx);
                            });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new("发起被动扫描 (Passive Scan)")
                    .disabled(!can_operate)
                    .on_click(move |_, window, cx| {
                        if let Some(owner) = &passive_owner {
                            let _ = owner.update(cx, |view, cx| {
                                view.send_to_passive_scanner(window, cx);
                            });
                        }
                    }),
            )
            .menu("发起主动扫描 (Active Scan)", Box::new(SendToScanner))
            .separator()
            .menu("发送到 Intruder", Box::new(OpenInIntruder))
            .menu("发送到 Repeater", Box::new(OpenInRepeater))
            .menu("发送到 Sequencer", Box::new(SendToSequencer))
            .menu("发送到 Comparer (请求)", Box::new(SendToComparerRequest))
            .menu("发送到 Comparer (响应)", Box::new(SendToComparerResponse))
            .menu("发送到 Decoder", Box::new(SendToDecoder))
            .item(
                PopupMenuItem::element(move |_, _| {
                    let key = gpui_kit::Keystroke::parse(if cfg!(target_os = "macos") {
                        "cmd-o"
                    } else {
                        "ctrl-o"
                    })
                    .ok()
                    .map(|ks| {
                        gpui_kit::component::kbd::Kbd::new(ks)
                            .p_0()
                            .flex_nowrap()
                            .border_0()
                            .bg(gpui_kit::transparent_black())
                    });
                    div()
                        .id(("history-row-to-organiser", row))
                        .test_support()
                        .role(gpui_kit::accesskit::Role::MenuItem)
                        .aria_label("发送到 Organizer")
                        .w_full()
                        .min_h(gpui_kit::px(21.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(gpui_kit::px(12.))
                        .child("发送到 Organizer")
                        .children(key)
                })
                .action(Box::new(SendToOrganiser))
                .disabled(!can_operate)
                .on_click(move |_, window, cx| {
                    if let Some(owner) = &org_owner {
                        let _ = owner.update(cx, |view, cx| {
                            view.send_selected_to_organiser_from_history(window, cx);
                        });
                    }
                }),
            )
            .item(
                PopupMenuItem::element(move |_, _| {
                    let key = gpui_kit::Keystroke::parse(if cfg!(target_os = "macos") {
                        "cmd-g"
                    } else {
                        "ctrl-g"
                    })
                    .ok()
                    .map(|ks| {
                        gpui_kit::component::kbd::Kbd::new(ks)
                            .p_0()
                            .flex_nowrap()
                            .border_0()
                            .bg(gpui_kit::transparent_black())
                    });
                    div()
                        .id(("history-row-to-mio-at", row))
                        .test_support()
                        .role(gpui_kit::accesskit::Role::MenuItem)
                        .aria_label("发送到 MioAT")
                        .w_full()
                        .min_h(gpui_kit::px(21.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(gpui_kit::px(12.))
                        .child("发送到 MioAT")
                        .children(key)
                })
                .icon(IconName::Zap)
                .disabled(!can_operate)
                .on_click(move |_, _, cx| {
                    if let Some(owner) = &ai_owner {
                        let _ = owner.update(cx, |view, cx| {
                            view.send_selected_to_mio_at(cx);
                        });
                    }
                }),
            )
            .separator()
            .item(PopupMenuItem::submenu(
                "记录问题 (Record an issue)",
                record_issue_submenu,
            ))
            .separator()
            .item(
                PopupMenuItem::new("在浏览器中打开响应 (Open response in browser)")
                    .disabled(!can_operate)
                    .on_click(move |_, window, cx| {
                        if let Some(owner) = &resp_browser_owner {
                            let _ = owner.update(cx, |view, cx| {
                                view.open_current_response_in_browser(window, cx);
                            });
                        }
                    }),
            )
            .item(PopupMenuItem::submenu(
                "在浏览器中发起请求 (Request in browser)",
                req_browser_submenu,
            ))
            .separator()
            .item(PopupMenuItem::submenu(
                "辅助测试工具 (Engagement tools)",
                engagement_submenu,
            ))
            .when(self.rows.get(row).is_some(), |menu| {
                menu.separator().menu_with_icon("添加备注", IconName::File, Box::new(AddHttpNotes))
            });
        let menu = if self.rows.get(row).is_some()
            && let Some(owner) = self.owner.clone()
        {
            let label = match &self.rows[row] {
                HistoryRow::ExtProxy(row) => row
                    .origin
                    .as_deref()
                    .and_then(|origin| origin.split_once("://"))
                    .map(|(_, authority)| {
                        if authority.starts_with('[') {
                            authority
                                .split_once(']')
                                .map_or(authority, |(host, _)| host.trim_start_matches('['))
                                .to_owned()
                        } else {
                            authority.split(':').next().unwrap_or(authority).to_owned()
                        }
                    })
                    .unwrap_or_default(),
                HistoryRow::Live(flow) => flow
                    .url
                    .split_once("://")
                    .and_then(|(_, tail)| tail.split('/').next())
                    .map(|authority| {
                        if authority.starts_with('[') {
                            authority
                                .split_once(']')
                                .map_or(authority, |(host, _)| host.trim_start_matches('['))
                                .to_owned()
                        } else {
                            authority.split(':').next().unwrap_or(authority).to_owned()
                        }
                    })
                    .unwrap_or_default(),
            };
            Workbench::http_highlight_menu(owner, targets, label, menu, window, cx)
        } else {
            menu
        };
        let menu = menu
            .separator()
            .menu_with_icon(
                "删除选中记录",
                IconName::Trash,
                Box::new(crate::DeleteSelectedHttpHistory),
            )
            .separator()
            .menu("清空实时历史记录", Box::new(crate::ClearLiveProxyHistory))
            .separator()
            .item(
                PopupMenuItem::new("显示新历史记录窗口 (Show new history window)")
                    .disabled(!can_operate)
                    .on_click(move |_, window, cx| {
                        if let Some(owner) = &new_win_owner {
                            let _ = owner.update(cx, |view, cx| {
                                view.open_detached_history_window(window, cx);
                            });
                        }
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new("复制 URL")
                    .icon(IconName::Copy)
                    .disabled(!can_operate)
                    .on_click(move |_, window, cx| {
                        window.dispatch_action(Box::new(CopyUrl), cx);
                    }),
            )
            .menu_with_icon("复制为 curl 命令", IconName::Copy, Box::new(CopyCurlCommand))
            .item(
                PopupMenuItem::new("提取页面链接 (Copy links in response)")
                    .icon(IconName::Copy)
                    .disabled(!can_operate)
                    .on_click(move |_, window, cx| {
                        if let Some(owner) = &links_owner {
                            let _ = owner.update(cx, |view, cx| {
                                view.copy_selected_history_links(window, cx);
                            });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new("导出选中项到文件 (Save item)")
                    .icon(IconName::Save)
                    .disabled(!can_operate)
                    .on_click(move |_, window, cx| {
                        if let Some(owner) = &save_xml_direct_owner {
                            let _ = owner.update(cx, |view, cx| {
                                view.save_selected_history_item_to_file(true, window, cx);
                            });
                        }
                    }),
            )
            .separator()
            .menu_with_icon(
                "添加到站点地图 (Add to site map)",
                gpui_kit::component::Icon::default().data(MIO_ADD_TO_SITEMAP_SVG),
                Box::new(crate::AddToSiteMap),
            )
            .map(|mut menu| {
                if !plugin_items.is_empty() {
                    menu = menu.separator();
                    for (instance_id, plugin_id) in &plugin_items {
                        let owner = plugin_owner.clone();
                        let id = *instance_id;
                        menu = menu.item(
                            PopupMenuItem::new(format!("插件动作：{plugin_id}"))
                                .disabled(!can_operate)
                                .on_click(move |_, window, cx| {
                                    if let Some(owner) = &owner {
                                        let _ = owner.update(cx, |view, cx| {
                                            view.invoke_plugin_context_menu_from_history(
                                                id, window, cx,
                                            );
                                        });
                                    }
                                }),
                        );
                    }
                }
                menu
            })
            .separator()
            .item(
                PopupMenuItem::new("Proxy 历史记录文档 (Proxy history documentation)")
                    .on_click(move |_, _, cx| {
                        if let Some(owner) = &doc_owner {
                            let _ = owner.update(cx, |view, cx| {
                                view.open_proxy_history_documentation(cx);
                            });
                        }
                    }),
            );
        menu
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div().size_full()
    }
}
