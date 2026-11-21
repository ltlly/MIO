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
}
