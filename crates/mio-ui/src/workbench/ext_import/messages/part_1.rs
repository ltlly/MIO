use super::*;

impl Workbench {
    pub(in crate::workbench::ext_import) fn show_ext_messages(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let created = self.ensure_ext_messages(window, cx);
        let Some(messages) = &mut self.ext_import.messages else {
            return;
        };
        let changed_scope = messages.items_visible;
        if changed_scope {
            let Some(revision) = messages.query_revision.checked_add(1) else {
                self.fail("归档消息查询 revision 已耗尽，请重开项目。", cx);
                return;
            };
            messages.query_revision = revision;
            messages.items_visible = false;
            messages.clear_rows(&self.focus, window, cx);
            if let Some(items) = &mut messages.items {
                items.clear_selection(&self.focus, window, cx);
            }
            self.focus.focus(window, cx);
        }
        self.ext_import.browser_visible = true;
        if created || changed_scope {
            self.cancel_ext_repeater();
            let filter = self
                .ext_import
                .messages
                .as_ref()
                .and_then(|messages| messages.filter.clone());
            self.query_ext_messages(filter, 0, window, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn ensure_ext_messages(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(source) = &self.ext_import.report else {
            return false;
        };
        let source_id = source.id;
        if self
            .ext_import
            .messages
            .as_ref()
            .is_none_or(|messages| messages.source_id != source_id)
        {
            let table = cx.new(|cx| {
                TableState::new(MessageDelegate::default(), window, cx)
                    .sortable(false)
                    .col_selectable(false)
            });
            let selection = cx.subscribe_in(&table, window, |this, table, event, window, cx| {
                let row = match event {
                    TableEvent::SelectRow(row) | TableEvent::DoubleClickedRow(row) => Some(*row),
                    TableEvent::RightClickedRow(row) => *row,
                    _ => None,
                };
                if let Some(id) =
                    row.and_then(|row| table.read(cx).delegate().rows.get(row).map(|row| row.id))
                {
                    let offset = this
                        .ext_import
                        .messages
                        .as_ref()
                        .filter(|messages| messages.selected == Some(id))
                        .map_or(0, |messages| messages.requested_offset);
                    this.read_ext_message(id, offset, window, cx);
                }
            });
            let choices: Vec<_> = source
                .report
                .collections
                .iter()
                .map(|collection| CollectionChoice {
                    path: collection.path.clone(),
                    label: format!("{} · 消息 {}", collection.path, collection.message_count)
                        .into(),
                })
                .collect();
            let collections = cx.new(|cx| {
                SelectState::new(SearchableVec::new(choices), None, window, cx).searchable(true)
            });
            let collection =
                cx.subscribe_in(&collections, window, |this, select, event, window, cx| {
                    let SelectEvent::Confirm(value) = event;
                    this.query_ext_messages(value.clone(), 0, window, cx);
                    if let Some(messages) = &this.ext_import.messages {
                        select.update(cx, |select, cx| match &messages.filter {
                            Some(path) => select.set_selected_value(path, window, cx),
                            None => select.set_selected_index(None, window, cx),
                        });
                    }
                });
            self.ext_import.messages = Some(Messages {
                source_id,
                table,
                collections,
                filter: None,
                query_revision: 0,
                query_offset: 0,
                previous_offsets: Vec::new(),
                selection_revision: 0,
                selected: None,
                requested_offset: 0,
                detail: None,
                detail_error: None,
                payload: MessageEditor::new(window, cx),
                items: None,
                items_visible: false,
                _selection: selection,
                _collection: collection,
            });
            return true;
        }
        false
    }

    pub(in crate::workbench) fn query_ext_messages(
        &mut self,
        filter: Option<String>,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .ext_import
            .messages
            .as_ref()
            .is_some_and(|messages| messages.items_visible)
        {
            self.query_ext_item_messages(offset, window, cx);
            return;
        }
        let (Some(project), Some(messages)) = (self.project, self.ext_import.messages.as_ref())
        else {
            return;
        };
        let Some(query_revision) = messages.query_revision.checked_add(1) else {
            self.fail("归档消息查询 revision 已耗尽，请重开项目。", cx);
            return;
        };
        let source_id = messages.source_id;
        if !self.enqueue(
            Command::QueryExtMessages {
                project,
                query_revision,
                source_id,
                collection: filter.clone(),
                offset,
                limit: PAGE_SIZE,
            },
            cx,
        ) {
            if let Some(messages) = &mut self.ext_import.messages {
                messages.table.update(cx, |state, cx| {
                    state.delegate_mut().error = self.error.clone();
                    state.refresh(cx);
                });
            }
            return;
        }
        self.cancel_ext_repeater();
        let messages = self
            .ext_import
            .messages
            .as_mut()
            .expect("message browser exists");
        let changed_filter = messages.filter != filter;
        messages.filter = filter;
        messages.query_revision = query_revision;
        messages.query_offset = offset;
        messages
            .collections
            .update(cx, |select, cx| match &messages.filter {
                Some(path) => select.set_selected_value(path, window, cx),
                None => select.set_selected_index(None, window, cx),
            });
        if changed_filter {
            messages.selected = None;
            messages.detail_error = None;
            messages.clear_detail(&self.focus, window, cx);
        }
        messages.table.update(cx, |state, cx| {
            let delegate = state.delegate_mut();
            delegate.loading = true;
            delegate.error = None;
            if changed_filter {
                delegate.rows.clear();
                delegate.offset = offset;
                delegate.total = 0;
                delegate.next_offset = None;
                state.clear_selection(cx);
            }
            state.refresh(cx);
        });
        cx.notify();
    }

    pub(in crate::workbench) fn read_ext_message(
        &mut self,
        message_id: Uuid,
        offset: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(project), Some(messages)) = (self.project, self.ext_import.messages.as_mut())
        else {
            return;
        };
        if messages.items_visible
            && messages
                .items
                .as_ref()
                .is_none_or(|items| items.detail.is_none())
        {
            return;
        }
        let Some(remaining) = messages
            .table
            .read(cx)
            .delegate()
            .rows
            .iter()
            .find(|row| row.id == message_id)
            .and_then(|row| row.raw_bytes.checked_sub(offset))
        else {
            self.fail("归档消息范围不在当前摘要内，请刷新后重试。", cx);
            return;
        };
        let limit = remaining.min(FRAGMENT_BYTES as u64) as usize;
        if messages.selected == Some(message_id)
            && messages.requested_offset == offset
            && messages.detail_error.is_none()
        {
            return;
        }
        let Some(selection_revision) = messages.selection_revision.checked_add(1) else {
            self.fail("归档消息选择 revision 已耗尽，请重开项目。", cx);
            return;
        };
        self.ext_import.repeater = Default::default();
        messages.selected = Some(message_id);
        messages.requested_offset = offset;
        messages.selection_revision = selection_revision;
        messages.detail_error = None;
        messages.clear_detail(&self.focus, window, cx);
        if !self.enqueue(
            Command::ReadExtMessage {
                project,
                selection_revision,
                message_id,
                offset,
                limit,
            },
            cx,
        ) {
            self.ext_import
                .messages
                .as_mut()
                .expect("message browser exists")
                .detail_error = self.error.clone();
        }
        cx.notify();
    }

    pub(in crate::workbench::ext_import) fn on_ext_message_event(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(&event, Event::ExtItems { .. } | Event::ExtItem { .. }) {
            self.on_ext_item_event(event, window, cx);
            return;
        }
        let Some(messages) = &mut self.ext_import.messages else {
            return;
        };
        match event {
            Event::ExtMessages {
                project,
                query_revision,
                source_id,
                collection,
                offset,
                result,
            } if self.project == Some(project)
                && !messages.items_visible
                && query_revision == messages.query_revision
                && source_id == messages.source_id
                && collection == messages.filter
                && offset == messages.query_offset =>
            {
                match result {
                    Ok(page) => {
                        let valid =
                            valid_page_range(offset, page.rows.len(), page.total, page.next_offset)
                                && page.rows.iter().all(|row| {
                                    row.source_id == source_id
                                        && collection
                                            .as_ref()
                                            .is_none_or(|path| &row.collection == path)
                                });
                        if !valid {
                            messages.table.update(cx, |state, cx| {
                                let delegate = state.delegate_mut();
                                delegate.loading = false;
                                delegate.error =
                                    Some("归档消息分页范围或续页位置不一致，请重试。".into());
                                state.refresh(cx);
                            });
                            cx.notify();
                            return;
                        }
                        messages.apply_page(offset, page, &self.focus, window, cx);
                    }
                    Err(error) => messages.table.update(cx, |state, cx| {
                        state.delegate_mut().loading = false;
                        state.delegate_mut().error = Some(error);
                        state.refresh(cx);
                    }),
                }
            }
            Event::ExtMessage {
                project,
                selection_revision,
                message_id,
                offset,
                result,
            } if self.project == Some(project)
                && selection_revision == messages.selection_revision
                && messages.selected == Some(message_id)
                && offset == messages.requested_offset =>
            {
                match result {
                    Ok(fragment) => {
                        let valid = fragment.summary.id == message_id
                            && fragment.summary.source_id == messages.source_id
                            && fragment.offset == offset
                            && fragment.summary.raw_bytes.checked_sub(offset).is_some_and(
                                |remaining| {
                                    fragment.bytes.len() as u64
                                        == remaining.min(FRAGMENT_BYTES as u64)
                                },
                            )
                            && messages
                                .table
                                .read(cx)
                                .delegate()
                                .rows
                                .iter()
                                .any(|row| row == &fragment.summary);
                        if valid {
                            messages.payload.load(&fragment.bytes, window, cx);
                            messages.detail = Some(fragment);
                            messages.detail_error = None;
                        } else {
                            messages.clear_detail(&self.focus, window, cx);
                            messages.detail_error =
                                Some("正文片段与当前归档消息或字节范围不一致，请重新读取。".into());
                        }
                    }
                    Err(message) => {
                        messages.clear_detail(&self.focus, window, cx);
                        messages.detail_error = Some(message);
                    }
                }
            }
            _ => {}
        }
        cx.notify();
    }
}
