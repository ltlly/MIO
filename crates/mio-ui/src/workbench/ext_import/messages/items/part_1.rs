use super::*;

impl Workbench {
    pub(in crate::workbench::ext_import) fn show_ext_items(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.ext_import.report.is_none() {
            return;
        }
        self.ensure_ext_messages(window, cx);
        self.cancel_ext_repeater();
        let messages = self
            .ext_import
            .messages
            .as_ref()
            .expect("message browser exists");
        let changed_scope = !messages.items_visible;
        let created = messages.items.is_none();
        if created {
            let choices: Vec<_> = self
                .ext_import
                .report
                .as_ref()
                .expect("report exists")
                .report
                .collections
                .iter()
                .map(|collection| CollectionChoice {
                    path: collection.path.clone(),
                    label: format!(
                        "{} · 条目 {} · {}",
                        collection.path, collection.indexed_items, collection.state
                    )
                    .into(),
                })
                .collect();
            let table = cx.new(|cx| {
                TableState::new(ItemDelegate::default(), window, cx)
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
                    let Some(items) = this
                        .ext_import
                        .messages
                        .as_ref()
                        .and_then(|messages| messages.items.as_ref())
                    else {
                        return;
                    };
                    if items.selected == Some(id) && items.detail_error.is_none() {
                        return;
                    }
                    let offset = if items.selected == Some(id) {
                        items.requested_message_offset
                    } else {
                        0
                    };
                    this.read_ext_item(id, offset, window, cx);
                }
            });
            let collections = cx.new(|cx| {
                SelectState::new(SearchableVec::new(choices), None, window, cx).searchable(true)
            });
            let collection = cx.subscribe_in(&collections, window, |this, _, event, window, cx| {
                let SelectEvent::Confirm(value) = event;
                this.query_ext_items(value.clone(), 0, window, cx);
            });
            self.ext_import
                .messages
                .as_mut()
                .expect("message browser exists")
                .items = Some(Items {
                table,
                collections,
                filter: None,
                query_revision: 0,
                query_offset: 0,
                previous_offsets: Vec::new(),
                selection_revision: 0,
                selected: None,
                requested_message_offset: 0,
                detail: None,
                detail_error: None,
                metadata: editing::new_editor(window, cx),
                _selection: selection,
                _collection: collection,
            });
        }
        let messages = self
            .ext_import
            .messages
            .as_mut()
            .expect("message browser exists");
        if changed_scope {
            messages.clear_rows(&self.focus, window, cx);
            messages
                .items
                .as_mut()
                .expect("item browser exists")
                .clear_selection(&self.focus, window, cx);
            self.focus.focus(window, cx);
        }
        messages.items_visible = true;
        self.ext_import.browser_visible = true;
        if created || changed_scope {
            let filter = messages
                .items
                .as_ref()
                .and_then(|items| items.filter.clone());
            self.query_ext_items(filter, 0, window, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn query_ext_items(
        &mut self,
        filter: Option<String>,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(project), Some(messages)) = (self.project, self.ext_import.messages.as_ref())
        else {
            return;
        };
        if !messages.items_visible {
            return;
        }
        let Some(items) = &messages.items else {
            return;
        };
        let Some(revision) = items.query_revision.checked_add(1) else {
            self.fail("归档条目查询 revision 已耗尽，请重开项目。", cx);
            return;
        };
        let source_id = messages.source_id;
        let changed_filter = items.filter != filter;
        let changed_scope = changed_filter || items.query_offset != offset;
        self.cancel_ext_repeater();
        let messages = self
            .ext_import
            .messages
            .as_mut()
            .expect("message browser exists");
        if changed_scope {
            messages.clear_rows(&self.focus, window, cx);
        }
        let items = messages.items.as_mut().expect("item browser exists");
        if changed_scope {
            items.selected = None;
            items.detail_error = None;
            items.clear_detail(&self.focus, window, cx);
        }
        items.query_revision = revision;
        items.query_offset = offset;
        items.filter = filter.clone();
        items.collections.update(cx, |select, cx| match &filter {
            Some(path) => select.set_selected_value(path, window, cx),
            None => select.set_selected_index(None, window, cx),
        });
        if changed_filter {
            items.previous_offsets.clear();
        }
        items.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.loading = true;
            delegate.error = None;
            if changed_filter {
                delegate.rows.clear();
                delegate.offset = 0;
                delegate.total = 0;
                delegate.next_offset = None;
            }
            if changed_scope {
                table.clear_selection(cx);
            }
            table.refresh(cx);
        });
        if !self.enqueue(
            Command::QueryExtItems {
                project,
                query_revision: revision,
                source_id,
                collection: filter,
                offset,
                limit: PAGE_SIZE,
            },
            cx,
        ) {
            let error = self.error.clone();
            self.ext_import
                .messages
                .as_mut()
                .unwrap()
                .items
                .as_mut()
                .unwrap()
                .table
                .update(cx, |table, cx| {
                    table.delegate_mut().loading = false;
                    table.delegate_mut().error = error;
                    table.refresh(cx);
                });
        }
        cx.notify();
    }

    pub(in crate::workbench::ext_import::messages) fn query_ext_item_messages(
        &mut self,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self
            .ext_import
            .messages
            .as_ref()
            .and_then(|messages| messages.items.as_ref())
            .and_then(|items| items.selected)
        {
            self.read_ext_item(id, offset, window, cx);
        }
    }

    pub(in crate::workbench) fn read_ext_item(
        &mut self,
        id: Uuid,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(project), Some(messages)) = (self.project, self.ext_import.messages.as_ref())
        else {
            return;
        };
        if !messages.items_visible {
            return;
        }
        let Some(items) = &messages.items else {
            return;
        };
        if !items
            .table
            .read(cx)
            .delegate()
            .rows
            .iter()
            .any(|row| row.id == id && offset <= row.message_count)
        {
            self.fail("归档条目或消息页不在当前摘要范围内，请刷新后重试。", cx);
            return;
        }
        let Some(revision) = items.selection_revision.checked_add(1) else {
            self.fail("归档条目选择 revision 已耗尽，请重开项目。", cx);
            return;
        };
        let changed_item = items.selected != Some(id);
        let source_id = messages.source_id;
        self.cancel_ext_repeater();
        let messages = self
            .ext_import
            .messages
            .as_mut()
            .expect("message browser exists");
        if changed_item {
            messages.clear_rows(&self.focus, window, cx);
        }
        messages.selected = None;
        messages.detail_error = None;
        messages.clear_detail(&self.focus, window, cx);
        messages.query_offset = offset;
        messages.table.update(cx, |table, cx| {
            table.delegate_mut().loading = true;
            table.delegate_mut().error = None;
            table.clear_selection(cx);
            table.refresh(cx);
        });
        let items = messages.items.as_mut().expect("item browser exists");
        items.selected = Some(id);
        items.selection_revision = revision;
        items.requested_message_offset = offset;
        items.detail_error = None;
        items.clear_detail(&self.focus, window, cx);
        if !self.enqueue(
            Command::ReadExtItem {
                project,
                selection_revision: revision,
                source_id,
                item_id: id,
                message_offset: offset,
                message_limit: PAGE_SIZE,
            },
            cx,
        ) {
            let error = self.error.clone().expect("failed enqueue reports an error");
            self.ext_import
                .messages
                .as_mut()
                .unwrap()
                .item_read_failed(error, &self.focus, window, cx);
        }
        cx.notify();
    }
}
