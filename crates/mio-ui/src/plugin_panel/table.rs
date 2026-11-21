use super::*;

impl NativePanel {
    pub(in crate::plugin_panel) fn filter_table(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(table) = self.tables.get(&id) else {
            return;
        };
        let query = match editing::input_snapshot(&table.filter, window, cx) {
            Ok(query) if query.len() <= 1024 => query.to_string(),
            Ok(_) => {
                self.fail("表格筛选条件不能超过 1024 字节。", cx);
                return;
            }
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        self.emit(id, ui_event::Event::Filter(query), cx);
    }

    pub(in crate::plugin_panel) fn page_table(
        &mut self,
        id: u64,
        offset: u64,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        let valid_range = self
            .node(id)
            .and_then(|node| match &node.kind {
                Some(node::Kind::VirtualTable(model)) => {
                    Some(offset == 0 || offset < model.total_rows)
                }
                _ => None,
            })
            .unwrap_or(false);
        if !valid_range {
            self.show_diagnostic("表格行数已更新；旧视图翻页未执行，请重试", cx);
            return;
        }
        let Some(revision) = self
            .tables
            .get(&id)
            .map(|table| table.next_viewport_revision)
        else {
            return;
        };
        let Some(revision) = revision.checked_add(1) else {
            self.fail("表格 viewport revision 已耗尽。", cx);
            return;
        };
        let seq = self.next_ui_seq();
        let table = self.tables.get_mut(&id).unwrap();
        table.next_viewport_revision = revision;
        if table.pending_viewports.len() >= MAX_UI_OVERLAY_HISTORY {
            table.pending_viewports.remove(0);
        }
        table.pending_viewports.push((revision, offset, seq));
        table.requested_offset = Some(offset);
        table.requested_viewport = Some((revision, seq, false));
        table.selected.clear();
        table.anchor = None;
        table.cursor = None;
        self.emit_with_seq(
            id,
            seq,
            ui_event::Event::Viewport(ViewportRequest {
                revision,
                offset,
                limit: panel::MAX_TABLE_ROWS as u32,
            }),
            cx,
        );
    }

    pub(in crate::plugin_panel) fn select_row(
        &mut self,
        id: u64,
        row: usize,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        let Some(node::Kind::VirtualTable(model)) =
            self.node(id).and_then(|node| node.kind.as_ref())
        else {
            return;
        };
        let model = model.clone();
        let Some(record) = model.rows.get(row) else {
            return;
        };
        let table = self.tables.get_mut(&id).unwrap();
        table.cursor = Some(row);
        if model.multi_select && modifiers.shift {
            let anchor = table.anchor.unwrap_or(row).min(model.rows.len() - 1);
            table.selected = model.rows[anchor.min(row)..=anchor.max(row)]
                .iter()
                .map(|row| row.id.clone())
                .collect();
        } else if model.multi_select && modifiers.secondary(cx) {
            if table.selected.contains(&record.id) {
                table.selected.retain(|id| id != &record.id);
            } else {
                table.selected.push(record.id.clone());
            }
            table.anchor = Some(row);
        } else {
            table.selected = vec![record.id.clone()];
            table.anchor = Some(row);
        }
        let row_ids = table.selected.clone();
        self.emit(
            id,
            ui_event::Event::TableSelection(TableSelection { row_ids }),
            cx,
        );
    }

    pub(in crate::plugin_panel) fn table_key(
        &mut self,
        id: u64,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        let Some(node::Kind::VirtualTable(model)) =
            self.node(id).and_then(|node| node.kind.as_ref())
        else {
            return;
        };
        let count = model.rows.len();
        if count == 0 {
            self.key(id, event, cx);
            return;
        }
        let table = &self.tables[&id];
        let row = match event.keystroke.key.as_str() {
            "up" => Some(table.cursor.unwrap_or(1).saturating_sub(1)),
            "down" => Some(table.cursor.map_or(0, |row| (row + 1).min(count - 1))),
            "home" => Some(0),
            "end" => Some(count - 1),
            "a" if event.keystroke.modifiers.secondary(cx) && model.multi_select => {
                let row_ids: Vec<_> = model.rows.iter().map(|row| row.id.clone()).collect();
                let table = self.tables.get_mut(&id).unwrap();
                table.selected = row_ids.clone();
                table.anchor = Some(0);
                table.cursor = Some(count - 1);
                self.emit(
                    id,
                    ui_event::Event::TableSelection(TableSelection { row_ids }),
                    cx,
                );
                None
            }
            _ => None,
        };
        if let Some(row) = row {
            self.select_row(
                id,
                row,
                Modifiers {
                    platform: false,
                    ..event.keystroke.modifiers
                },
                cx,
            );
            self.tables[&id]
                .scroll
                .scroll_to_item(row, ScrollStrategy::Top);
        }
        self.key(id, event, cx);
    }

    pub(in crate::plugin_panel) fn render_table(
        &self,
        node: &Node,
        model: &panel::VirtualTable,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = node.id;
        let table = &self.tables[&id];
        let scale = crate::theme::ui_scale();
        let scaled_col_width = |width: f32| (width * scale).round();
        let total_width: f32 = model
            .columns
            .iter()
            .map(|column| scaled_col_width(column.width))
            .sum();
        let mut header = div()
            .flex()
            .h(crate::theme::scaled_px(24.).max(px(20.)))
            .flex_shrink_0()
            .bg(crate::theme::toolbar_surface())
            .border_b_1()
            .border_color(crate::theme::border());
        for column in &model.columns {
            header = header.child(
                div()
                    .w(px(scaled_col_width(column.width)))
                    .flex_shrink_0()
                    .px(px(4.))
                    .truncate()
                    .child(column.title.clone()),
            );
        }
        let snapshot = self.snapshot.clone();
        let selected = table.selected.clone();
        let weak = cx.weak_entity();
        let rows = uniform_list(
            ("plugin-table-rows", id),
            model.rows.len(),
            move |range, _, _| {
                let row_scale = crate::theme::ui_scale();
                let node::Kind::VirtualTable(table) = snapshot
                    .root
                    .as_ref()
                    .unwrap()
                    .find(id)
                    .unwrap()
                    .kind
                    .as_ref()
                    .unwrap()
                else {
                    unreachable!()
                };
                range
                    .filter_map(|index| table.rows.get(index).map(|row| (index, row)))
                    .map(|(index, row)| {
                        let weak = weak.clone();
                        let mut element = div()
                            .id(("plugin-row", index))
                            .test_support()
                            .role(Role::Row)
                            .aria_label(row.cells.join("，"))
                            .h(theme::row_height())
                            .flex()
                            .items_center()
                            .when(selected.contains(&row.id), |element| {
                                element.bg(theme::selected())
                            })
                            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.tables[&id].focus.focus(window, cx);
                                    this.select_row(id, index, event.modifiers, cx)
                                });
                            });
                        for (cell, column) in row.cells.iter().zip(&table.columns) {
                            element = element.child(
                                div()
                                    .w(px((column.width * row_scale).round()))
                                    .flex_shrink_0()
                                    .px(px(4.))
                                    .truncate()
                                    .child(cell.clone()),
                            );
                        }
                        element
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&table.scroll)
        .flex_1()
        .min_h_0()
        .w(px(total_width));
        let previous = model.offset.saturating_sub(panel::MAX_TABLE_ROWS as u64);
        let next = (model.offset + model.rows.len() as u64).min(model.total_rows);
        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .gap(px(4.))
                    .flex_shrink_0()
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&table.filter).small().disabled(!self.enabled)),
                    )
                    .child(
                        Button::new(("plugin-filter", id))
                            .label("筛选")
                            .tooltip("按关键字筛选当前插件表格数据")
                            .small()
                            .disabled(!self.enabled)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.filter_table(id, window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .id(("plugin-table-scroll", id))
                    .test_support()
                    .role(Role::Table)
                    .track_focus(&table.focus)
                    .tab_index(0)
                    .aria_label(node.accessibility_label.clone())
                    .flex_1()
                    .min_h_0()
                    .overflow_x_scroll()
                    .track_scroll(&table.horizontal)
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                        this.table_key(id, event, cx)
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .h_full()
                            .w(px(total_width))
                            .child(header)
                            .child(rows),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .flex_shrink_0()
                    .child(
                        Button::new(("plugin-previous", id))
                            .label("上一页")
                            .tooltip("加载上一页表格数据")
                            .small()
                            .disabled(
                                !self.enabled
                                    || model.offset == 0
                                    || table.requested_offset.is_some(),
                            )
                            .on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.page_table(id, previous, cx)
                                }),
                            ),
                    )
                    .child(
                        Button::new(("plugin-next", id))
                            .label("下一页")
                            .tooltip("加载下一页表格数据")
                            .small()
                            .disabled(
                                !self.enabled
                                    || next >= model.total_rows
                                    || table.requested_offset.is_some(),
                            )
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.page_table(id, next, cx)),
                            ),
                    )
                    .child(if table.requested_offset.is_some() {
                        "正在读取表格…".into()
                    } else {
                        format!(
                            "{}–{} / {}",
                            if model.rows.is_empty() {
                                0
                            } else {
                                model.offset + 1
                            },
                            next,
                            model.total_rows
                        )
                    }),
            )
            .into_any_element()
    }
}
