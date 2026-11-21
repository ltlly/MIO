use super::*;

impl Workbench {
    pub(in crate::workbench) fn toggle_site_map_sort(
        &mut self,
        column: SiteMapSortColumn,
        cx: &mut Context<Self>,
    ) {
        self.target_pane.item_sort = Some(match self.target_pane.item_sort {
            Some(current) if current.column == column => SiteMapSort {
                column,
                ascending: !current.ascending,
            },
            _ => SiteMapSort {
                column,
                ascending: true,
            },
        });
        cx.notify();
    }

    pub(in crate::workbench) fn set_site_map_sort(
        &mut self,
        column: SiteMapSortColumn,
        ascending: bool,
        cx: &mut Context<Self>,
    ) {
        self.target_pane.item_sort = Some(SiteMapSort { column, ascending });
        cx.notify();
    }

    pub(in crate::workbench) fn target_cell_text_for_summary(
        col_ix: usize,
        item_idx: usize,
        summary: &super::mime::TargetRowSummary,
        items: &[HistoryRow],
        live_notes: &HashMap<FlowId, (u64, String)>,
    ) -> String {
        match col_ix {
            0 => summary.host.clone(),
            1 => summary.method.clone(),
            2 => summary.url.clone(),
            3 => summary.params.clone(),
            4 => summary.status.clone(),
            5 => summary.length.clone(),
            6 => summary.mime.clone(),
            7 => summary.title.clone(),
            8 => items
                .get(item_idx)
                .map(|row| match row {
                    HistoryRow::ExtProxy(archive) => archive
                        .notes_preview
                        .replace("\r\n", " ")
                        .replace(['\r', '\n', '\t'], " "),
                    HistoryRow::Live(flow) => live_notes
                        .get(&flow.id)
                        .map(|(_, text)| {
                            text.replace("\r\n", " ").replace(['\r', '\n', '\t'], " ")
                        })
                        .unwrap_or_default(),
                })
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    pub(in crate::workbench) fn copy_target_column(
        &mut self,
        col_ix: usize,
        rows: &[(usize, super::mime::TargetRowSummary)],
        cx: &mut Context<Self>,
    ) {
        let items = &self.target_pane.items;
        let live_notes = self.history.read(cx).delegate().live_notes.borrow();
        let mut lines = Vec::with_capacity(rows.len());
        for (item_idx, summary) in rows {
            lines.push(Self::target_cell_text_for_summary(
                col_ix,
                *item_idx,
                summary,
                items,
                &live_notes,
            ));
        }
        drop(live_notes);
        let count = lines.len();
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(lines.join("\n")));
        self.error = None;
        self.status = format!(
            "已复制「{}」列（{count} 行）到剪贴板。",
            SITEMAP_COL_NAMES.get(col_ix).copied().unwrap_or("Site map")
        );
        cx.notify();
    }

    pub(in crate::workbench) fn toggle_target_column_visibility(
        &mut self,
        col_ix: usize,
        cx: &mut Context<Self>,
    ) {
        if col_ix >= SITEMAP_COL_COUNT {
            return;
        }
        let visible_count = self
            .target_pane
            .item_col_visible
            .iter()
            .filter(|&&v| v)
            .count();
        if self.target_pane.item_col_visible[col_ix] {
            if visible_count > 1 {
                self.target_pane.item_col_visible[col_ix] = false;
            }
        } else {
            self.target_pane.item_col_visible[col_ix] = true;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn fit_target_column_width(
        &mut self,
        col_ix: usize,
        rows: &[(usize, super::mime::TargetRowSummary)],
        cx: &mut Context<Self>,
    ) {
        if col_ix >= SITEMAP_COL_COUNT {
            return;
        }
        let measure = |text: &str| -> f32 {
            text.chars()
                .map(|ch| if ch.is_ascii() { 7.2 } else { 13.5 })
                .sum()
        };
        let mut widest = measure(SITEMAP_COL_NAMES[col_ix]) + 22.0;
        let items = &self.target_pane.items;
        let live_notes = self.history.read(cx).delegate().live_notes.borrow();
        for (item_idx, summary) in rows.iter().take(250) {
            let text =
                Self::target_cell_text_for_summary(col_ix, *item_idx, summary, items, &live_notes);
            let preview = theme::truncate_cell_preview(&text);
            widest = widest.max(measure(&preview));
        }
        drop(live_notes);
        let fitted = (widest + 16.0).round().clamp(44.0, 560.0);
        self.target_pane.item_col_widths[col_ix] = fitted;
        cx.notify();
    }

    pub(in crate::workbench) fn reset_target_column_layout(&mut self, cx: &mut Context<Self>) {
        self.target_pane.item_col_widths = SITEMAP_DEFAULT_COL_WIDTHS;
        self.target_pane.item_col_visible = [true; SITEMAP_COL_COUNT];
        self.target_view
            .update(cx, |view, cx| view.clear_column_resize(cx));
        cx.notify();
    }

    pub(super) fn render_target_items_header(
        &self,
        sort: Option<SiteMapSort>,
        cached_rows: std::rc::Rc<Vec<(usize, super::mime::TargetRowSummary)>>,
        view_ctx: &TargetViewContext<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let header_fg = table_header_fg();
        let header_bg = table_header_bg();
        let hover_bg: gpui_kit::Hsla =
            rgb(if theme::is_dark() { 0x2f3237 } else { 0xe6e6e6 }).into();
        let col_widths = self.target_pane.item_col_widths;
        let col_visible = self.target_pane.item_col_visible;
        let visible_cols: Vec<usize> = (0..SITEMAP_COL_COUNT)
            .filter(|&col_ix| col_visible[col_ix])
            .collect();
        let flex_col_ix = if col_visible[2] {
            2
        } else {
            visible_cols.last().copied().unwrap_or(0)
        };
        let last_visible_col = visible_cols.last().copied().unwrap_or(0);
        let target_view_id = view_ctx.view_entity_id;
        let target_view_weak = view_ctx.view.clone();
        let weak = cx.weak_entity();
        let header_defs: [(&'static str, &'static str, Option<SiteMapSortColumn>);
            SITEMAP_COL_COUNT] = [
            (
                "target-sitemap-sort-host",
                "Host",
                Some(SiteMapSortColumn::Host),
            ),
            (
                "target-sitemap-sort-method",
                "Method",
                Some(SiteMapSortColumn::Method),
            ),
            ("target-sitemap-sort-url", "URL", Some(SiteMapSortColumn::Url)),
            (
                "target-sitemap-sort-params",
                "Params",
                Some(SiteMapSortColumn::Params),
            ),
            (
                "target-sitemap-sort-status",
                "Status code",
                Some(SiteMapSortColumn::Status),
            ),
            (
                "target-sitemap-sort-length",
                "Length",
                Some(SiteMapSortColumn::Length),
            ),
            (
                "target-sitemap-sort-mime",
                "MIME type",
                Some(SiteMapSortColumn::Mime),
            ),
            (
                "target-sitemap-sort-title",
                "Title",
                Some(SiteMapSortColumn::Title),
            ),
            ("target-sitemap-col-notes", "Notes", None),
        ];

        let header_cells: Vec<AnyElement> = visible_cols
            .iter()
            .map(|&col_ix| {
                let (id, label, sort_col_opt) = header_defs[col_ix];
                let width = col_widths[col_ix];
                let is_last = col_ix == last_visible_col;
                let active = sort_col_opt.and_then(|col| sort.filter(|s| s.column == col));
                let ctx_view = weak.clone();
                let ctx_rows = std::rc::Rc::clone(&cached_rows);
                let fit_rows = std::rc::Rc::clone(&cached_rows);
                let resize_start_view = target_view_weak.clone();
                let resize_handle = div()
                    .id(("target-sitemap-col-resize", col_ix))
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
                                this.fit_target_column_width(col_ix, &fit_rows, cx);
                            } else {
                                let start_w = this.target_pane.item_col_widths[col_ix];
                                let _ = resize_start_view.update(cx, |view, _| {
                                    view.begin_column_resize(
                                        col_ix,
                                        event.position.x.as_f32(),
                                        start_w,
                                    );
                                });
                            }
                        }),
                    )
                    .on_drag(
                        ResizeTargetCol {
                            entity_id: target_view_id,
                            col_ix,
                        },
                        |drag, _, _, cx| {
                            cx.stop_propagation();
                            cx.new(|_| *drag)
                        },
                    );

                let mut cell = div()
                    .id(id)
                    .test_support()
                    .role(gpui_kit::accesskit::Role::ColumnHeader)
                    .aria_label(label)
                    .relative()
                    .h_full()
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .overflow_hidden()
                    .bg(header_bg)
                    .text_color(header_fg)
                    .when(!is_last, |c| c.border_r_1().border_color(theme::border()))
                    .hover(move |s| s.bg(hover_bg));
                if let Some(sort_col) = sort_col_opt {
                    cell = cell
                        .cursor_pointer()
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("点击按此列排序")
                                .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_site_map_sort(sort_col, cx);
                        }));
                }
                let cell = cell
                    .context_menu(move |menu, window, cx| {
                        build_target_header_context_menu(
                            ctx_view.clone(),
                            col_ix,
                            sort_col_opt,
                            std::rc::Rc::clone(&ctx_rows),
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
                            .font_weight(gpui_kit::FontWeight::NORMAL)
                            .text_color(header_fg)
                            .child(label),
                    )
                    .when_some(active, |row, s| {
                        row.child(
                            Icon::new(if s.ascending {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .size(px(11.))
                            .text_color(theme::muted_foreground())
                            .flex_shrink_0(),
                        )
                    })
                    .child(resize_handle);

                if col_ix == flex_col_ix {
                    cell.flex_1()
                        .min_w(theme::scaled_px(width))
                        .into_any_element()
                } else {
                    cell.w(theme::scaled_px(width))
                        .flex_shrink_0()
                        .into_any_element()
                }
            })
            .collect();

        let drag_view = target_view_weak;
        div()
            .w_full()
            .h(theme::table_header_height())
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_0()
            .border_b_1()
            .border_color(theme::border())
            .bg(header_bg)
            .text_color(header_fg)
            .on_drag_move::<ResizeTargetCol>(
                move |event: &DragMoveEvent<ResizeTargetCol>, _, cx| {
                    let drag = *event.drag(cx);
                    if drag.entity_id == target_view_id {
                        let _ = drag_view.update(cx, |view, cx| {
                            view.update_column_resize(
                                drag.col_ix,
                                event.event.position.x.as_f32(),
                                cx,
                            );
                        });
                    }
                },
            )
            .children(header_cells)
            .into_any_element()
    }
}

pub(in crate::workbench) fn build_target_header_context_menu(
    view: WeakEntity<Workbench>,
    col_ix: usize,
    sort_col_opt: Option<SiteMapSortColumn>,
    rows: std::rc::Rc<Vec<(usize, super::mime::TargetRowSummary)>>,
    col_visible: [bool; SITEMAP_COL_COUNT],
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let visible_count = col_visible.iter().filter(|&&v| v).count();
    let can_hide = col_ix < SITEMAP_COL_COUNT && visible_count > 1;
    let has_rows = !rows.is_empty();

    let v_asc = view.clone();
    let v_desc = view.clone();
    let v_copy = view.clone();
    let v_hide = view.clone();
    let v_fit = view.clone();
    let v_cols = view.clone();
    let v_reset = view;

    let copy_rows = std::rc::Rc::clone(&rows);
    let fit_rows = rows;

    let cols_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
        (0..SITEMAP_COL_COUNT).fold(sub.compact().min_w(px(172.)), |sub, idx| {
            let owner = v_cols.clone();
            let checked = col_visible[idx];
            let disabled = checked && visible_count <= 1;
            sub.item(
                PopupMenuItem::new(SITEMAP_COL_NAMES[idx])
                    .checked(checked)
                    .disabled(disabled)
                    .on_click(move |_, _, cx| {
                        let _ = owner.update(cx, |wb, cx| {
                            wb.toggle_target_column_visibility(idx, cx);
                        });
                    }),
            )
        })
    });

    let mut menu = menu.compact();
    if let Some(sort_col) = sort_col_opt {
        menu = menu
            .item(
                PopupMenuItem::new("按此列升序排序 (Sort ascending)").on_click(move |_, _, cx| {
                    let _ = v_asc.update(cx, |wb, cx| wb.set_site_map_sort(sort_col, true, cx));
                }),
            )
            .item(
                PopupMenuItem::new("按此列降序排序 (Sort descending)").on_click(
                    move |_, _, cx| {
                        let _ = v_desc
                            .update(cx, |wb, cx| wb.set_site_map_sort(sort_col, false, cx));
                    },
                ),
            )
            .separator();
    }

    menu.item(
        PopupMenuItem::new("复制整列内容 (Copy column)")
            .disabled(!has_rows)
            .on_click(move |_, _, cx| {
                let _ = v_copy.update(cx, |wb, cx| {
                    wb.copy_target_column(col_ix, &copy_rows, cx);
                });
            }),
    )
    .item(
        PopupMenuItem::new("隐藏此列 (Hide column)")
            .disabled(!can_hide)
            .on_click(move |_, _, cx| {
                let _ = v_hide.update(cx, |wb, cx| {
                    wb.toggle_target_column_visibility(col_ix, cx);
                });
            }),
    )
    .item(
        PopupMenuItem::new("自适应列宽 (Auto-fit column width)").on_click(move |_, _, cx| {
            let _ = v_fit.update(cx, |wb, cx| {
                wb.fit_target_column_width(col_ix, &fit_rows, cx);
            });
        }),
    )
    .separator()
    .item(PopupMenuItem::submenu("显示/隐藏列 (Columns)", cols_submenu))
    .item(
        PopupMenuItem::new("重置列布局 (Reset column layout)").on_click(move |_, _, cx| {
            let _ = v_reset.update(cx, |wb, cx| wb.reset_target_column_layout(cx));
        }),
    )
}
