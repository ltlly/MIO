use gpui_kit::assets::IconName;

use super::*;

impl Workbench {
    pub(in crate::workbench::ext_import::messages) fn on_ext_item_event(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(messages) = &mut self.ext_import.messages else {
            return;
        };
        if !messages.items_visible {
            return;
        }
        let Some(items) = &mut messages.items else {
            return;
        };
        match event {
            Event::ExtItems {
                project,
                query_revision,
                source_id,
                collection,
                offset,
                result,
            } if self.project == Some(project)
                && source_id == messages.source_id
                && query_revision == items.query_revision
                && collection == items.filter
                && offset == items.query_offset =>
            {
                match result {
                    Ok(page) => {
                        let mut ids = HashSet::new();
                        let valid =
                            valid_page_range(offset, page.rows.len(), page.total, page.next_offset)
                                && page.rows.iter().all(|row| {
                                    row.source_id == source_id
                                        && valid_summary(row)
                                        && collection
                                            .as_ref()
                                            .is_none_or(|path| &row.collection == path)
                                        && ids.insert(row.id)
                                })
                                && page.rows.windows(2).all(|rows| {
                                    (&rows[0].collection, rows[0].index)
                                        < (&rows[1].collection, rows[1].index)
                                });
                        if !valid {
                            items.table.update(cx, |table, cx| {
                                table.delegate_mut().loading = false;
                                table.delegate_mut().error =
                                    Some("归档条目归属、位置或分页范围不一致，请重试。".into());
                                table.refresh(cx);
                            });
                            cx.notify();
                            return;
                        }
                        let selected = items
                            .selected
                            .and_then(|id| page.rows.iter().position(|row| row.id == id));
                        let unchanged = selected.is_some_and(|index| {
                            items
                                .table
                                .read(cx)
                                .delegate()
                                .rows
                                .iter()
                                .any(|row| row == &page.rows[index])
                        });
                        if !unchanged {
                            items.selected = None;
                            items.detail_error = None;
                            items.clear_detail(&self.focus, window, cx);
                        }
                        let current = items.table.read(cx).delegate().offset;
                        remember_page(&mut items.previous_offsets, current, offset);
                        items.table.update(cx, |table, cx| {
                            let delegate = table.delegate_mut();
                            delegate.rows = page.rows;
                            delegate.offset = offset;
                            delegate.total = page.total;
                            delegate.next_offset = page.next_offset;
                            delegate.loading = false;
                            delegate.error = None;
                            if let Some(row) = selected.filter(|_| unchanged) {
                                if table.selected_row() != Some(row) {
                                    table.set_selected_row(row, cx);
                                }
                            } else {
                                table.clear_selection(cx);
                            }
                            table.refresh(cx);
                        });
                        if !unchanged {
                            messages.clear_rows(&self.focus, window, cx);
                        }
                    }
                    Err(error) => items.table.update(cx, |table, cx| {
                        table.delegate_mut().loading = false;
                        table.delegate_mut().error = Some(error);
                        table.refresh(cx);
                    }),
                }
            }
            Event::ExtItem {
                project,
                selection_revision,
                source_id,
                item_id,
                message_offset,
                result,
            } if self.project == Some(project)
                && source_id == messages.source_id
                && selection_revision == items.selection_revision
                && items.selected == Some(item_id)
                && message_offset == items.requested_message_offset =>
            {
                match result {
                    Ok(detail) => {
                        let valid = items
                            .table
                            .read(cx)
                            .delegate()
                            .rows
                            .iter()
                            .find(|row| row.id == item_id)
                            .is_some_and(|row| valid_detail(&detail, row, message_offset));
                        if !valid {
                            messages.item_read_failed(
                                "条目元数据、消息归属或分页范围与当前摘要不一致，请重试。".into(),
                                &self.focus,
                                window,
                                cx,
                            );
                            cx.notify();
                            return;
                        }
                        let text = String::from_utf8(detail.item.metadata_json.clone())
                            .expect("metadata UTF-8 was validated");
                        items
                            .metadata
                            .update(cx, |state, cx| state.set_value(text, window, cx));
                        items.detail = Some(detail.item);
                        items.detail_error = None;
                        messages.apply_page(
                            message_offset,
                            detail.messages,
                            &self.focus,
                            window,
                            cx,
                        );
                    }
                    Err(error) => messages.item_read_failed(error, &self.focus, window, cx),
                }
            }
            _ => {}
        }
        cx.notify();
    }

    pub(in crate::workbench::ext_import::messages) fn render_ext_items(
        &self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let items = self
            .ext_import
            .messages
            .as_ref()
            .and_then(|messages| messages.items.as_ref())
            .expect("item browser exists");
        let delegate = items.table.read(cx).delegate();
        let offset = delegate.offset;
        let count = delegate.rows.len();
        let total = delegate.total;
        let loading = delegate.loading;
        let error = delegate.error.clone();
        let refresh_offset = if error.is_some() {
            items.query_offset
        } else {
            offset
        };
        let previous = items
            .previous_offsets
            .last()
            .copied()
            .filter(|_| offset != 0);
        let next = delegate.next_offset;
        let refresh_filter = items.filter.clone();
        let previous_filter = items.filter.clone();
        let next_filter = items.filter.clone();
        let metadata = if let Some(item) = &items.detail {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(toolbar().child(format!(
                    "原位置 {} · {} · 原始 JSON",
                    item.index, item.state
                )))
                .child(
                    div()
                        .px(px(8.))
                        .py(px(4.))
                        .text_color(theme::muted_foreground())
                        .child("只读字段观察，最多 256 KiB；未恢复的语义见导入记录。"),
                )
                .child(
                    div().flex_1().min_h_0().child(
                        Editor::new(&items.metadata)
                            .readonly(true)
                            .size_full()
                            .bordered(false)
                            .aria_label("外部归档条目原始元数据 JSON"),
                    ),
                )
                .into_any_element()
        } else {
            let mut view = div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .text_color(theme::muted_foreground())
                .child(items.detail_error.as_ref().map_or_else(
                    || {
                        if items.selected.is_some() {
                            "正在读取条目元数据与消息摘要…".into()
                        } else {
                            "选择条目，按需查看原始元数据。".into()
                        }
                    },
                    |error| format!("读取条目失败：{error}"),
                ));
            if items.detail_error.is_some()
                && let Some(id) = items.selected
            {
                let offset = items.requested_message_offset;
                view = view.child(
                    button("ext-item-retry", "重试读取条目")
                        .icon(IconName::RefreshCw)
                        .tooltip("重新从项目归档存储读取该条目的 JSON 元数据与关联消息列表")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.read_ext_item(id, offset, window, cx)
                        })),
                );
            }
            view.into_any_element()
        };
        let messages = if items.selected.is_some() {
            self.render_ext_message_contents(cx)
        } else {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme::muted_foreground())
                .child("选择条目后仅显示该槽位关联的归档消息；不会从其它历史记录拼补当前草稿。")
                .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .child(
                div()
                    .w(px(500.))
                    .flex_none()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .border_color(theme::border())
                    .child(toolbar().child("全部条目 · 保留 null、failed、empty 与重复引用的位置"))
                    .child(
                        toolbar()
                            .child(
                                button("ext-items-all", "全部集合")
                                    .icon(IconName::ListFilter)
                                    .tooltip("清除集合筛选条件并显示所有来源集合中的原始归档条目")
                                    .selected(items.filter.is_none())
                                    .disabled(loading)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.query_ext_items(None, 0, window, cx)
                                    })),
                            )
                            .child(
                                div().flex_1().min_w_0().child(
                                    Select::new(&items.collections)
                                        .id("ext-item-collection-select")
                                        .accessibility_label("筛选 外部归档条目来源集合")
                                        .placeholder("全部集合")
                                        .search_placeholder("搜索完整集合路径…")
                                        .cleanable(true)
                                        .disabled(loading)
                                        .w_full()
                                        .menu_max_h(px(300.))
                                        .scroll_bounce(false)
                                        .small(),
                                ),
                            ),
                    )
                    .child(
                        toolbar()
                            .child(
                                button(
                                    "ext-items-refresh",
                                    if error.is_some() {
                                        "重试查询"
                                    } else {
                                        "刷新"
                                    },
                                )
                                .icon(IconName::RefreshCw)
                                .tooltip("重新从项目归档存储查询当前分页的条目列表")
                                .disabled(loading)
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.query_ext_items(
                                            refresh_filter.clone(),
                                            refresh_offset,
                                            window,
                                            cx,
                                        )
                                    },
                                )),
                            )
                            .child(div().flex_1())
                            .child(
                                button("ext-items-previous", "上一页")
                                    .icon(IconName::ChevronLeft)
                                    .tooltip("加载上一页归档条目摘要")
                                    .disabled(previous.is_none() || loading)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if let Some(offset) = previous {
                                            this.query_ext_items(
                                                previous_filter.clone(),
                                                offset,
                                                window,
                                                cx,
                                            );
                                        }
                                    })),
                            )
                            .child(format!(
                                "{}–{} / {total}",
                                if count == 0 { 0 } else { offset + 1 },
                                offset + count
                            ))
                            .child(
                                button("ext-items-next", "下一页")
                                    .icon(IconName::ChevronRight)
                                    .tooltip("加载下一页归档条目摘要")
                                    .disabled(next.is_none() || loading)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if let Some(offset) = next {
                                            this.query_ext_items(
                                                next_filter.clone(),
                                                offset,
                                                window,
                                                cx,
                                            );
                                        }
                                    })),
                            ),
                    )
                    .when_some(error.as_ref().filter(|_| count > 0), |this, error| {
                        this.child(
                            div()
                                .px(px(8.))
                                .text_color(theme::danger_fg())
                                .child(format!("查询失败，保留上次结果：{error}")),
                        )
                    })
                    .child(
                        div()
                            .id("ext-items-table")
                            .h(px(220.))
                            .min_h(px(120.))
                            .child(
                                DataTable::new(&items.table)
                                    .with_size(theme::row_height())
                                    .header_row_height(theme::row_height())
                                    .bordered(false)
                                    .scroll_bounce(false)
                                    .accessibility_label("External 全部归档条目"),
                            ),
                    )
                    .child(div().flex_1().min_h_0().child(metadata)),
            )
            .child(div().flex_1().min_w_0().min_h_0().child(messages))
            .into_any_element()
    }
}
