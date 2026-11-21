use super::*;

impl WebSocketDelegate {
    pub(super) fn default_columns() -> Vec<Column> {
        vec![
            Column::new("number", "#")
                .sortable()
                .sort(ColumnSort::Ascending)
                .width(75.)
                .movable(false),
            Column::new("url", "URL").sortable().width(375.),
            Column::new("direction", "方向").sortable().width(185.),
            Column::new("edited", "已编辑").width(100.),
            Column::new("length", "长度").sortable().width(125.),
            Column::new("notes", "备注").width(185.),
            Column::new("tls", "TLS").width(75.),
            Column::new("time", "时间").sortable().width(150.),
            Column::new("listener", "监听端口").sortable().width(150.),
            Column::new("connection", "WebSocket ID")
                .sortable()
                .width(180.),
        ]
    }

    pub(super) fn new(owner: WeakEntity<Workbench>) -> Self {
        Self {
            owner,
            rows: Vec::new(),
            highlights: HashMap::new(),
            deleted_ids: std::collections::HashSet::new(),
            offset: 0,
            total: 0,
            loading: false,
            error: None,
            visible: 0..0,
            columns: Self::default_columns(),
        }
    }

    pub(super) fn active_sort(&self) -> Option<(SharedString, ColumnSort)> {
        self.columns.iter().find_map(|col| match col.sort {
            Some(sort @ (ColumnSort::Ascending | ColumnSort::Descending)) => {
                Some((col.key.clone(), sort))
            }
            _ => None,
        })
    }

    pub(super) fn apply_sort_key(&mut self, active: Option<(SharedString, ColumnSort)>) {
        let mut applied = false;
        for column in &mut self.columns {
            if column.sort.is_some() {
                if let Some((ref key, sort)) = active
                    && column.key == *key
                {
                    column.sort = Some(sort);
                    applied = true;
                } else {
                    column.sort = Some(ColumnSort::Default);
                }
            }
        }
        if !applied {
            self.reset_sort();
        }
    }

    pub(super) fn reset_sort(&mut self) {
        for column in &mut self.columns {
            if column.sort.is_some() {
                column.sort = Some(if column.key.as_ref() == "number" {
                    ColumnSort::Ascending
                } else {
                    ColumnSort::Default
                });
            }
        }
    }

    pub(super) fn has_explicit_sort(&self) -> bool {
        self.columns.iter().any(|col| match col.sort {
            Some(ColumnSort::Descending) => true,
            Some(ColumnSort::Ascending) => col.key.as_ref() != "number",
            _ => false,
        })
    }

    pub(super) fn sort_loaded_rows(&mut self) {
        let active = self.columns.iter().find_map(|col| match col.sort {
            Some(sort @ (ColumnSort::Ascending | ColumnSort::Descending)) => {
                Some((col.key.clone(), sort == ColumnSort::Ascending))
            }
            _ => None,
        });
        self.rows.sort_by(|left, right| {
            let tie_break = || {
                left.ordinal
                    .cmp(&right.ordinal)
                    .then_with(|| left.id.cmp(&right.id))
            };
            let Some((ref key, ascending)) = active else {
                return tie_break();
            };
            let primary = match key.as_ref() {
                "number" => left.ordinal.cmp(&right.ordinal),
                "url" => left.url.cmp(&right.url),
                "direction" => {
                    direction_label(left.direction).cmp(direction_label(right.direction))
                }
                "length" => left.payload_bytes.cmp(&right.payload_bytes),
                "time" => left.observed_unix_ms.cmp(&right.observed_unix_ms),
                "listener" => left
                    .listener_port
                    .unwrap_or(0)
                    .cmp(&right.listener_port.unwrap_or(0)),
                "connection" => left.connection_id.cmp(&right.connection_id),
                _ => std::cmp::Ordering::Equal,
            };
            let ordered = if ascending {
                primary
            } else {
                primary.reverse()
            };
            ordered.then_with(tie_break)
        });
    }

    pub(super) fn row(&self, row: usize) -> Option<&WebSocketSummary> {
        self.rows.get(row.checked_sub(self.offset)?)
    }

    pub(super) fn cell(&self, row: usize, column: usize) -> String {
        let Some(message) = self.row(row) else {
            return if column == 0 && self.error.is_none() {
                "正在读取…".into()
            } else {
                String::new()
            };
        };
        match self.columns[column].key.as_ref() {
            "number" => message.ordinal.to_string(),
            "url" => message.url.clone(),
            "direction" => direction_label(message.direction).into(),
            "edited" => {
                if message.edited {
                    "✓".into()
                } else {
                    String::new()
                }
            }
            "notes" => message
                .notes_preview
                .replace("\r\n", " ")
                .replace(['\n', '\r', '\t'], " "),
            "length" => message.payload_bytes.to_string(),
            "tls" => match message.url.split_once("://").map(|(scheme, _)| scheme) {
                Some("https" | "wss") => "✓".into(),
                Some("http" | "ws") => String::new(),
                _ => "未知".into(),
            },
            "time" => local_time(message.observed_unix_ms),
            "listener" => message
                .listener_port
                .map(|port| port.to_string())
                .unwrap_or_default(),
            "connection" => message.connection_id.to_string(),
            _ => unreachable!("columns are constructed together with their renderers"),
        }
    }
}

impl TableDelegate for WebSocketDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.total
    }

    fn fit_column(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Option<gpui_kit::Pixels> {
        let col = self.columns.get(col_ix)?.clone();
        let min_w = col.min_width.as_f32().max(44.0);
        let max_w = col.max_width.as_f32().min(520.0).max(min_w);
        let measure = |text: &str| -> f32 {
            text.chars()
                .map(|ch| if ch.is_ascii() { 7.2 } else { 13.5 })
                .sum()
        };
        let mut widest =
            measure(col.name.as_ref()) + if col.sort.is_some() { 18.0 } else { 4.0 };
        let sample_len = self.rows.len().min(250);
        for idx in 0..sample_len {
            let row_ix = self.offset + idx;
            let cell_str = crate::theme::truncate_cell_preview(&self.cell(row_ix, col_ix));
            widest = widest.max(measure(&cell_str));
        }
        let unscaled = (widest + 8.0).round().clamp(min_w, max_w);
        self.columns[col_ix].width = px(unscaled);
        let scale = theme::ui_scale();
        Some(px((unscaled * scale).round()))
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
            left: theme::scaled_px(6.),
            right: theme::scaled_px(6.),
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
        let effective_sort = if matches!(sort, ColumnSort::Default) {
            ColumnSort::Descending
        } else {
            sort
        };
        for (idx, column) in self.columns.iter_mut().enumerate() {
            if column.sort.is_some() {
                column.sort = Some(if idx == col {
                    effective_sort
                } else {
                    ColumnSort::Default
                });
            }
        }
        self.sort_loaded_rows();
        let selected_id = self
            .owner
            .upgrade()
            .and_then(|entity| entity.read(cx).websocket.selected);
        let next_selected = selected_id
            .and_then(|id| self.rows.iter().position(|row| row.id == id))
            .map(|pos| self.offset + pos);
        let owner = self.owner.clone();
        let handle = window.window_handle();
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, _, cx| {
                let _ = owner.update(cx, |view, cx| {
                    view.websocket.table.update(cx, |state, cx| {
                        if let Some(row) = next_selected
                            && state.selected_row() != Some(row)
                        {
                            state.set_selected_row(row, cx);
                        }
                        state.refresh(cx);
                    });
                    cx.notify();
                });
            });
        });
        cx.notify();
    }

    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let row_item = self.row(row);
        let message_id = row_item.map(|item| item.id);
        let hl_color = self
            .row(row)
            .and_then(|item| self.highlights.get(&item.id).copied())
            .filter(|c| (1..=9).contains(c));
        let owner_entity = self.owner.upgrade();
        let is_multi_selected = match (owner_entity.as_ref(), message_id) {
            (Some(entity), Some(id)) => entity.read(cx).websocket.is_id_selected(id),
            _ => false,
        };
        let owner_weak = owner_entity.as_ref().map(gpui_kit::Entity::downgrade);
        div()
            .id(("websocket-row", row))
            .aria_selected(is_multi_selected)
            .when_some(owner_weak, |this, weak| {
                let weak_left = weak.clone();
                this.on_mouse_down(gpui_kit::MouseButton::Left, move |event, _, cx| {
                    let toggle = event.modifiers.secondary(cx);
                    let shift = event.modifiers.shift;
                    let _ = weak_left.update(cx, |view, _| {
                        view.websocket.record_pointer_down(
                            row,
                            gpui_kit::MouseButton::Left,
                            shift,
                            toggle,
                        );
                    });
                })
                .on_mouse_down(gpui_kit::MouseButton::Right, move |event, _, cx| {
                    let toggle = event.modifiers.secondary(cx);
                    let shift = event.modifiers.shift;
                    let _ = weak.update(cx, |view, _| {
                        view.websocket.record_pointer_down(
                            row,
                            gpui_kit::MouseButton::Right,
                            shift,
                            toggle,
                        );
                    });
                })
            })
            .when_some(
                hl_color.and_then(crate::history::highlight_background),
                |this, bg| {
                    this.bg(bg)
                        .text_color(crate::history::highlight_foreground(hl_color.unwrap_or(0)))
                },
            )
            .when(is_multi_selected && hl_color.is_none(), |this| {
                this.bg(theme::selected()).text_color(theme::foreground())
            })
    }

    fn visible_rows_changed(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.visible = range;
        let owner = self.owner.clone();
        let handle = window.window_handle();
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                let _ = owner.update(cx, |view, cx| view.ensure_websocket_visible(window, cx));
            });
        });
    }

    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
            .h_full()
            .w_full()
            .flex()
            .items_center()
            .child(div().min_w_0().truncate().child(self.cell(row, column)))
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
        let owner = self.owner.clone();
        cx.defer(move |cx| {
            let _ = owner.update(cx, |view, cx| {
                view.toggle_websocket_column_visibility(&key, cx);
            });
        });
        true
    }

    fn reset_columns(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> bool {
        let owner = self.owner.clone();
        cx.defer(move |cx| {
            let _ = owner.update(cx, |view, cx| {
                view.restore_default_websocket_columns(cx);
            });
        });
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
        let all_cols = Self::default_columns();
        let owner = self.owner.clone();
        let cols_submenu = PopupMenu::build(window, cx, move |mut sub, _, _| {
            let visible_count = visible_keys.len();
            for col in &all_cols {
                let key = col.key.clone();
                let is_visible = visible_keys.contains(&key);
                let is_number = key.as_ref() == "number";
                let can_toggle = !is_number && (!is_visible || visible_count > 1);
                let toggle_owner = owner.clone();
                sub = sub.item(
                    PopupMenuItem::new(col.name.clone())
                        .checked(is_visible)
                        .disabled(!can_toggle)
                        .on_click(move |_, _, cx| {
                            let _ = toggle_owner.update(cx, |view, cx| {
                                view.toggle_websocket_column_visibility(key.as_ref(), cx);
                            });
                        }),
                );
            }
            let show_all_owner = owner.clone();
            sub.separator().item(
                PopupMenuItem::new("显示全部列 (Show all columns)").on_click(move |_, _, cx| {
                    let _ = show_all_owner.update(cx, |view, cx| {
                        view.restore_default_websocket_columns(cx);
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
        let owner = self.owner.clone();
        cx.defer(move |cx| {
            let _ = owner.update(cx, |view, _| {
                let visible_keys: std::collections::HashSet<SharedString> =
                    visible_cols.iter().map(|c| c.key.clone()).collect();
                let mut visible_iter = visible_cols.into_iter();
                for slot in &mut view.websocket.all_columns {
                    if visible_keys.contains(&slot.key)
                        && let Some(updated) = visible_iter.next()
                    {
                        *slot = updated;
                    }
                }
            });
        });
        cx.notify();
    }

    fn on_column_widths_changed(
        &mut self,
        widths: &[gpui_kit::Pixels],
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let scale = theme::ui_scale().max(1.0);
        for (ix, col) in self.columns.iter_mut().enumerate() {
            if let Some(&w) = widths.get(ix)
                && w > px(0.)
            {
                col.width = px((w.as_f32() / scale).round());
            }
        }
        let updated_cols: Vec<(SharedString, gpui_kit::Pixels)> = self
            .columns
            .iter()
            .map(|c| (c.key.clone(), c.width))
            .collect();
        let owner = self.owner.clone();
        cx.defer(move |cx| {
            let _ = owner.update(cx, |view, _| {
                for (key, w) in &updated_cols {
                    if let Some(slot) = view.websocket.all_columns.iter_mut().find(|c| &c.key == key)
                    {
                        slot.width = *w;
                    }
                }
            });
        });
    }

    fn context_menu(
        &mut self,
        row: usize,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let message_id = self.row(row).map(|item| item.id);
        let target_ids: Vec<Uuid> = if let Some(clicked_id) = message_id {
            if let Some(entity) = self.owner.upgrade() {
                let selected_set = entity.read(cx).websocket.selected_ids_snapshot();
                if selected_set.contains(&clicked_id) && selected_set.len() > 1 {
                    let ordered: Vec<Uuid> = self
                        .rows
                        .iter()
                        .filter(|r| selected_set.contains(&r.id))
                        .map(|r| r.id)
                        .collect();
                    if ordered.is_empty() {
                        vec![clicked_id]
                    } else {
                        ordered
                    }
                } else {
                    vec![clicked_id]
                }
            } else {
                vec![clicked_id]
            }
        } else {
            Vec::new()
        };
        let rep_targets = target_ids.clone();
        let cmp_targets = target_ids.clone();
        let org_targets = target_ids.clone();
        let hl_targets = target_ids.clone();
        let copy_text_targets = target_ids.clone();
        let copy_hex_targets = target_ids.clone();
        let copy_url_targets = target_ids.clone();
        let del_targets = target_ids;
        let row_url = self
            .row(row)
            .map(|item| item.url.clone())
            .filter(|url| !url.is_empty());
        let host_label = row_url
            .as_deref()
            .and_then(|u| u.split_once("://"))
            .map(|(_, tail)| {
                tail.split(['/', '?', '#'])
                    .next()
                    .unwrap_or_default()
                    .split(':')
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            })
            .unwrap_or_default();
        let owner = self.owner.clone();
        let decoder_owner = self.owner.clone();
        let comparer_owner = self.owner.clone();
        let organiser_owner = self.owner.clone();
        let highlight_owner = self.owner.clone();
        let copy_owner = self.owner.clone();
        let copy_text_owner = self.owner.clone();
        let copy_hex_owner = self.owner.clone();
        let notes_owner = self.owner.clone();
        let scope_in_owner = self.owner.clone();
        let scope_ex_owner = self.owner.clone();
        let delete_owner = self.owner.clone();
        let clear_owner = self.owner.clone();
        let save_owner = self.owner.clone();
        let scope_in_url = row_url.clone();
        let scope_ex_url = row_url.clone();
        let has_project = owner.upgrade().is_some_and(|entity| {
            let view = entity.read(cx);
            view.project.is_some() && view.backend_available() && view.transition.is_none()
        });
        let enabled = message_id.is_some() && has_project;
        let can_copy_url = enabled && row_url.is_some();
        let highlight_submenu = PopupMenu::build(window, cx, move |sub, _, _| {
            const NAMES: [&str; 10] = [
                "None", "Red", "Orange", "Yellow", "Green", "Cyan", "Blue", "Pink", "Magenta",
                "Gray",
            ];
            (0..=9u8).fold(sub.min_w(theme::scaled_px(88.)), |sub, color| {
                let hl_owner = highlight_owner.clone();
                let ids = hl_targets.clone();
                let label = if host_label.is_empty() {
                    NAMES[usize::from(color)].to_owned()
                } else {
                    host_label.clone()
                };
                let color_name = NAMES[usize::from(color)];
                sub.item(
                    PopupMenuItem::element(move |_, _| {
                        div()
                            .id(("ws-highlight-color", usize::from(color)))
                            .test_support()
                            .role(gpui_kit::accesskit::Role::MenuItem)
                            .aria_label(format!("Highlight {color_name}"))
                            .w_full()
                            .h(theme::scaled_px(24.).max(px(24.)))
                            .mx_neg_2()
                            .px(theme::scaled_px(8.))
                            .flex()
                            .items_center()
                            .gap(theme::scaled_px(6.))
                            .when_some(crate::history::highlight_background(color), |this, bg| {
                                this.bg(bg)
                            })
                            .text_color(crate::history::highlight_foreground(color))
                            .child(
                                div()
                                    .w(theme::scaled_px(10.))
                                    .h(theme::scaled_px(10.))
                                    .when(color == 0, |this| {
                                        this.border_1()
                                            .border_dashed()
                                            .border_color(rgb(0x888888))
                                    }),
                            )
                            .child(label.clone())
                    })
                    .disabled(!enabled)
                    .on_click(move |_, _, cx| {
                        if ids.is_empty() {
                            return;
                        }
                        let _ = hl_owner.update(cx, |view, cx| {
                            if ids.len() > 1 {
                                view.set_websocket_batch_highlight(&ids, color, cx);
                            } else {
                                view.set_websocket_highlight(ids[0], color, cx);
                            }
                        });
                    }),
                )
            })
        });
        menu.compact()
            .item(
                PopupMenuItem::new("发送到 Repeater")
                    .action(Box::new(OpenInRepeater))
                    .disabled(!enabled)
                    .on_click(move |_, window, cx| {
                        if rep_targets.is_empty() {
                            return;
                        }
                        let _ = owner.update(cx, |view, cx| {
                            view.send_websocket_batch_to_repeater(&rep_targets, window, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("发送到 Decoder")
                    .action(Box::new(crate::SendToDecoder))
                    .disabled(!enabled)
                    .on_click(move |_, window, cx| {
                        let Some(message_id) = message_id else {
                            return;
                        };
                        let _ = decoder_owner.update(cx, |view, cx| {
                            view.send_websocket_to_decoder(Some(message_id), window, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("发送到 Comparer")
                    .action(Box::new(crate::SendToComparerRequest))
                    .disabled(!enabled)
                    .on_click(move |_, window, cx| {
                        if cmp_targets.is_empty() {
                            return;
                        }
                        let _ = comparer_owner.update(cx, |view, cx| {
                            view.send_websocket_batch_to_comparer(&cmp_targets, window, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("发送到 Organizer")
                    .disabled(!enabled)
                    .on_click(move |_, window, cx| {
                        if org_targets.is_empty() {
                            return;
                        }
                        let _ = organiser_owner.update(cx, |view, cx| {
                            view.send_websocket_batch_to_organiser(&org_targets, window, cx);
                        });
                    }),
            )
            .separator()
            .item(PopupMenuItem::submenu("高亮 (Highlight)", highlight_submenu))
            .item(
                PopupMenuItem::new("编辑备注 (Add notes)")
                    .icon(IconName::File)
                    .disabled(!enabled)
                    .on_click(move |_, window, cx| {
                        let Some(message_id) = message_id else {
                            return;
                        };
                        let _ = notes_owner.update(cx, |view, cx| {
                            view.open_websocket_notes_for(Some(message_id), window, cx);
                        });
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new("复制载荷 (Copy payload)")
                    .icon(IconName::Copy)
                    .disabled(!enabled)
                    .on_click(move |_, window, cx| {
                        if copy_text_targets.is_empty() {
                            return;
                        }
                        let _ = copy_text_owner.update(cx, |view, cx| {
                            view.copy_websocket_batch_payloads(
                                &copy_text_targets,
                                false,
                                window,
                                cx,
                            );
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("复制载荷 Hex")
                    .icon(IconName::Copy)
                    .disabled(!enabled)
                    .on_click(move |_, window, cx| {
                        if copy_hex_targets.is_empty() {
                            return;
                        }
                        let _ = copy_hex_owner.update(cx, |view, cx| {
                            view.copy_websocket_batch_payloads(&copy_hex_targets, true, window, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("复制 URL")
                    .icon(IconName::Copy)
                    .action(Box::new(CopyUrl))
                    .disabled(!can_copy_url)
                    .on_click(move |_, _, cx| {
                        let Some(url) = row_url.clone() else {
                            return;
                        };
                        let _ = copy_owner.update(cx, |view, cx| {
                            if view.transition.is_some() || view.project.is_none() {
                                return;
                            }
                            if copy_url_targets.len() > 1 {
                                view.copy_websocket_batch_urls(&copy_url_targets, cx);
                            } else {
                                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(url));
                                view.error = None;
                                view.status = "已复制 URL 到剪贴板。".into();
                                cx.notify();
                            }
                        });
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new("添加到范围")
                    .action(Box::new(crate::AddToScope))
                    .disabled(!can_copy_url)
                    .on_click(move |_, _, cx| {
                        let Some(url) = scope_in_url.clone() else {
                            return;
                        };
                        let _ = scope_in_owner.update(cx, |view, cx| {
                            view.modify_websocket_url_scope(&url, true, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("从范围排除")
                    .action(Box::new(crate::ExcludeFromScope))
                    .disabled(!can_copy_url)
                    .on_click(move |_, _, cx| {
                        let Some(url) = scope_ex_url.clone() else {
                            return;
                        };
                        let _ = scope_ex_owner.update(cx, |view, cx| {
                            view.modify_websocket_url_scope(&url, false, cx);
                        });
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new("删除条目")
                    .icon(IconName::Trash)
                    .disabled(!enabled)
                    .on_click(move |_, window, cx| {
                        if del_targets.is_empty() {
                            return;
                        }
                        let _ = delete_owner.update(cx, |view, cx| {
                            view.delete_websocket_batch_entries(&del_targets, window, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("清空历史")
                    .disabled(!has_project)
                    .on_click(move |_, window, cx| {
                        let _ = clear_owner.update(cx, |view, cx| {
                            view.clear_websocket_history(window, cx);
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("保存条目…")
                    .icon(IconName::Save)
                    .disabled(!enabled)
                    .on_click(move |_, window, cx| {
                        let Some(message_id) = message_id else {
                            return;
                        };
                        let _ = save_owner.update(cx, |view, cx| {
                            view.save_websocket_item_to_file(Some(message_id), window, cx);
                        });
                    }),
            )
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let message = self.error.as_ref().map_or_else(
            || {
                if self.loading {
                    "正在读取 WebSocket 历史记录…".to_owned()
                } else {
                    "没有符合条件的 WebSocket 消息".to_owned()
                }
            },
            |error| format!("无法读取 WebSocket 历史记录：{error}"),
        );
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(message)
    }
}
