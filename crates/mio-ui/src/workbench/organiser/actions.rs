use super::table::OrganiserRowSummary;
use super::transfer::item_matches_query;
use super::*;

mod batch;

impl Workbench {
    pub(in crate::workbench) fn sync_organiser_table(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace.organiser_collections.is_empty() {
            let existing: HashSet<mio_core::Uuid> = self
                .workspace
                .organiser_items
                .iter()
                .map(|item| item.id)
                .collect();
            for collection in &mut self.workspace.organiser_collections {
                collection.item_ids.retain(|id| existing.contains(id));
            }
        }
        self.organiser.custom_collections = self
            .workspace
            .organiser_collections
            .iter()
            .map(|c| OrganiserCollection {
                id: c.id,
                name: c.name.clone(),
                item_ids: c.item_ids.iter().copied().collect(),
            })
            .collect();
        if let Some(cid) = self.organiser.active_collection
            && !self
                .organiser
                .custom_collections
                .iter()
                .any(|c| c.id == cid)
        {
            self.organiser.active_collection = None;
        }
        let active_collection_items = self.organiser.active_collection.and_then(|cid| {
            self.organiser
                .custom_collections
                .iter()
                .find(|c| c.id == cid)
                .map(|c| &c.item_ids)
        });
        let status_filter = self.organiser.status_filter;
        let query_lower = self.organiser.search_term.to_lowercase();
        let sort = self.organiser.sort;
        let mut matched: Vec<&OrganiserItem> = self
            .workspace
            .organiser_items
            .iter()
            .filter(|item| {
                if let Some(collection_ids) = active_collection_items {
                    collection_ids.contains(&item.id)
                } else {
                    status_filter.is_none_or(|expected| item.workflow_status == expected)
                }
            })
            .filter(|item| item_matches_query(item, &query_lower))
            .collect();
        matched.sort_by(|left, right| {
            let ordering = match sort.column {
                OrganiserSortColumn::Number => left.number.cmp(&right.number),
                OrganiserSortColumn::Method => left
                    .method
                    .cmp(&right.method)
                    .then_with(|| left.number.cmp(&right.number)),
                OrganiserSortColumn::Url => left
                    .url
                    .cmp(&right.url)
                    .then_with(|| left.number.cmp(&right.number)),
                OrganiserSortColumn::Status => left
                    .status_code
                    .unwrap_or(0)
                    .cmp(&right.status_code.unwrap_or(0))
                    .then_with(|| left.number.cmp(&right.number)),
                OrganiserSortColumn::Length => left
                    .response_len
                    .cmp(&right.response_len)
                    .then_with(|| left.number.cmp(&right.number)),
                OrganiserSortColumn::WorkflowStatus => left
                    .workflow_status
                    .sort_weight()
                    .cmp(&right.workflow_status.sort_weight())
                    .then_with(|| left.number.cmp(&right.number)),
            };
            if sort.ascending {
                ordering
            } else {
                ordering.reverse()
            }
        });
        let rows: Vec<OrganiserRowSummary> = matched
            .into_iter()
            .map(OrganiserRowSummary::from_item)
            .collect();

        let prev_selected = self.organiser.selected_id;
        let exists_in_workspace = prev_selected.is_some_and(|id| {
            self.workspace
                .organiser_items
                .iter()
                .any(|item| item.id == id)
        });
        let visible_in_rows =
            prev_selected.is_some_and(|id| rows.iter().any(|entry| entry.id == id));
        let mut need_reload = false;
        if !exists_in_workspace || (!visible_in_rows && !self.organiser.notes_dirty) {
            let next_id = rows.first().map(|entry| entry.id);
            if next_id != prev_selected {
                self.organiser.selected_id = next_id;
                need_reload = true;
            }
        } else if prev_selected.is_none() && !rows.is_empty() {
            self.organiser.selected_id = Some(rows[0].id);
            need_reload = true;
        }

        let selected_id = self.organiser.selected_id;
        self.organiser
            .selected_ids
            .retain(|id| rows.iter().any(|entry| entry.id == *id));
        if let Some(id) = selected_id {
            if self.organiser.selected_ids.is_empty() {
                self.organiser.selected_ids.insert(id);
            }
        } else {
            self.organiser.selected_ids.clear();
        }
        let selected_ids = self.organiser.selected_ids.clone();
        self.organiser.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.rows = rows;
            delegate.selected_id = selected_id;
            delegate.selected_ids = selected_ids;
            delegate.apply_sort(sort);
            table.refresh(cx);
        });
        if need_reload {
            self.reload_selected_organiser_detail(window, cx);
        } else {
            cx.notify();
        }
    }

    pub(in crate::workbench) fn reload_selected_organiser_detail(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self.organiser.selected_id.and_then(|id| {
            self.workspace
                .organiser_items
                .iter()
                .find(|item| item.id == id)
                .cloned()
        });
        if let Some(item) = selected {
            self.organiser.request.load(&item.request_raw, window, cx);
            self.organiser.response.load(
                item.response_raw.as_deref().unwrap_or_default(),
                window,
                cx,
            );
            let notes = item.notes;
            self.organiser
                .notes
                .update(cx, |state, cx| state.set_value(notes, window, cx));
        } else {
            self.organiser.request.load(b"", window, cx);
            self.organiser.response.load(b"", window, cx);
            self.organiser
                .notes
                .update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.organiser.notes_dirty = false;
        self.clear_context_viewer("organiser", cx);
        cx.notify();
    }

    pub(in crate::workbench) fn select_organiser_visual_row(
        &mut self,
        row: usize,
        context: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self
            .organiser
            .table
            .read(cx)
            .delegate()
            .rows
            .get(row)
            .map(|entry| entry.id);
        if let Some(id) = id {
            if context && self.organiser.selected_ids.contains(&id) {
                return;
            }
            if !context
                && self.organiser.selected_id == Some(id)
                && self.organiser.selected_ids.len() > 1
            {
                return;
            }
            let _ = self.select_organiser_item(id, window, cx);
        }
    }

    pub(in crate::workbench) fn select_organiser_item(
        &mut self,
        id: mio_core::Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.organiser.selected_id == Some(id) && self.organiser.selected_ids.len() <= 1 {
            return true;
        }
        if let Err(err) = self.validate_organiser_composition(window, cx) {
            self.sync_organiser_table(window, cx);
            self.fail(err, cx);
            return false;
        }
        if !self
            .workspace
            .organiser_items
            .iter()
            .any(|item| item.id == id)
        {
            return false;
        }
        if let Some(cid) = self.organiser.active_collection
            && self
                .organiser
                .custom_collections
                .iter()
                .find(|c| c.id == cid)
                .is_none_or(|c| !c.item_ids.contains(&id))
        {
            self.organiser.active_collection = None;
            self.sync_organiser_table(window, cx);
        }
        self.organiser.selected_id = Some(id);
        self.organiser.selection_anchor = Some(id);
        self.organiser.selected_ids.clear();
        self.organiser.selected_ids.insert(id);
        let selected_ids = self.organiser.selected_ids.clone();
        self.organiser.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.selected_id = Some(id);
            delegate.selected_ids = selected_ids;
            table.refresh(cx);
        });
        self.reload_selected_organiser_detail(window, cx);
        true
    }

    pub(in crate::workbench) fn set_organiser_status_filter(
        &mut self,
        filter: Option<OrganiserStatus>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.validate_organiser_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        self.organiser.active_collection = None;
        self.organiser.status_filter = filter;
        self.sync_organiser_table(window, cx);
    }

    pub(in crate::workbench) fn select_organiser_collection(
        &mut self,
        id: Option<mio_core::Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.validate_organiser_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        self.organiser.active_collection = id;
        self.organiser.status_filter = None;
        self.sync_organiser_table(window, cx);
    }

    pub(in crate::workbench) fn toggle_create_organiser_collection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.organiser.creating_collection {
            self.cancel_create_organiser_collection(window, cx);
            return;
        }
        self.organiser.creating_collection = true;
        self.organiser.new_collection_input.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    pub(in crate::workbench) fn cancel_create_organiser_collection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.organiser.creating_collection = false;
        self.organiser
            .new_collection_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
    }

    pub(in crate::workbench) fn confirm_create_organiser_collection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() {
            return;
        }
        let name = match editing::input_snapshot(&self.organiser.new_collection_input, window, cx) {
            Ok(raw) => raw.trim().to_owned(),
            Err(err) => {
                self.fail(err, cx);
                return;
            }
        };
        if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
            self.fail("集合名称不能为空、包含控制字符或超过 256 字节。", cx);
            return;
        }
        if self.workspace.organiser_collections.len() >= mio_core::MAX_ORGANISER_COLLECTIONS {
            self.fail(
                format!(
                    "Organiser 集合数量已达 {} 条上限。",
                    mio_core::MAX_ORGANISER_COLLECTIONS
                ),
                cx,
            );
            return;
        }
        if name.eq_ignore_ascii_case("inbox")
            || name == "收件箱"
            || self
                .workspace
                .organiser_collections
                .iter()
                .any(|c| c.name.eq_ignore_ascii_case(&name))
        {
            self.fail(format!("已存在名为「{name}」的集合。"), cx);
            return;
        }
        let id = mio_core::Uuid::new_v4();
        self.workspace
            .organiser_collections
            .push(mio_core::OrganiserCollection {
                id,
                name: name.clone(),
                item_ids: Vec::new(),
            });
        if let Err(err) = self.workspace.validate_structure() {
            self.workspace.organiser_collections.pop();
            self.fail(format!("未新建集合：{err}"), cx);
            return;
        }
        self.organiser.creating_collection = false;
        self.organiser
            .new_collection_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.organiser.active_collection = Some(id);
        self.organiser.status_filter = None;
        self.error = None;
        self.status = format!("已新建集合「{name}」。");
        self.sync_organiser_table(window, cx);
        self.save_workspace(cx);
    }

    pub(in crate::workbench) fn rename_organiser_collection(
        &mut self,
        id: mio_core::Uuid,
        new_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.organiser_editable() {
            return false;
        }
        let name = new_name.trim().to_owned();
        if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
            self.fail("集合名称不能为空、包含控制字符或超过 256 字节。", cx);
            return false;
        }
        if name.eq_ignore_ascii_case("inbox")
            || name == "收件箱"
            || self
                .workspace
                .organiser_collections
                .iter()
                .any(|c| c.id != id && c.name.eq_ignore_ascii_case(&name))
        {
            self.fail(format!("已存在名为「{name}」的集合。"), cx);
            return false;
        }
        let Some(pos) = self
            .workspace
            .organiser_collections
            .iter()
            .position(|c| c.id == id)
        else {
            return false;
        };
        if self.workspace.organiser_collections[pos].name == name {
            return true;
        }
        let previous = std::mem::replace(
            &mut self.workspace.organiser_collections[pos].name,
            name.clone(),
        );
        if let Err(err) = self.workspace.validate_structure() {
            self.workspace.organiser_collections[pos].name = previous;
            self.fail(format!("未重命名集合：{err}"), cx);
            return false;
        }
        self.error = None;
        self.status = format!("已将集合重命名为「{name}」。");
        self.sync_organiser_table(window, cx);
        self.save_workspace(cx);
        true
    }

    pub(in crate::workbench) fn delete_organiser_collection(
        &mut self,
        id: mio_core::Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() {
            return;
        }
        let Some(pos) = self
            .workspace
            .organiser_collections
            .iter()
            .position(|c| c.id == id)
        else {
            return;
        };
        let removed = self.workspace.organiser_collections.remove(pos);
        if self.organiser.active_collection == Some(id) {
            self.organiser.active_collection = None;
        }
        self.error = None;
        self.status = format!("已删除集合「{}」。", removed.name);
        self.sync_organiser_table(window, cx);
        self.save_workspace(cx);
    }

    pub(in crate::workbench) fn assign_organiser_items_to_collection(
        &mut self,
        collection_id: mio_core::Uuid,
        item_ids: Vec<mio_core::Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() {
            return;
        }
        let existing: HashSet<mio_core::Uuid> = self
            .workspace
            .organiser_items
            .iter()
            .map(|item| item.id)
            .collect();
        let Some(collection) = self
            .workspace
            .organiser_collections
            .iter_mut()
            .find(|c| c.id == collection_id)
        else {
            return;
        };
        let mut added = 0usize;
        for id in item_ids {
            if !existing.contains(&id) || collection.item_ids.contains(&id) {
                continue;
            }
            if collection.item_ids.len() >= mio_core::MAX_ORGANISER_ITEMS {
                break;
            }
            collection.item_ids.push(id);
            added += 1;
        }
        let name = collection.name.clone();
        self.error = None;
        self.status = if added > 0 {
            format!("已将 {added} 条条目添加到集合「{name}」。")
        } else {
            format!("所选条目已在集合「{name}」中。")
        };
        self.sync_organiser_table(window, cx);
        if added > 0 {
            self.save_workspace(cx);
        }
    }

    pub(in crate::workbench) fn remove_organiser_items_from_collection(
        &mut self,
        collection_id: mio_core::Uuid,
        item_ids: Vec<mio_core::Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() {
            return;
        }
        let remove_set: HashSet<mio_core::Uuid> = item_ids.into_iter().collect();
        let Some(collection) = self
            .workspace
            .organiser_collections
            .iter_mut()
            .find(|c| c.id == collection_id)
        else {
            return;
        };
        let before = collection.item_ids.len();
        collection.item_ids.retain(|id| !remove_set.contains(id));
        let removed = before - collection.item_ids.len();
        let name = collection.name.clone();
        self.error = None;
        self.status = if removed > 0 {
            format!("已将 {removed} 条条目移出集合「{name}」。")
        } else {
            format!("所选条目不在集合「{name}」中。")
        };
        self.sync_organiser_table(window, cx);
        if removed > 0 {
            self.save_workspace(cx);
        }
    }

    pub(in crate::workbench) fn export_organiser_collection(
        &mut self,
        collection_id: Option<mio_core::Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() || self.file_picker_pending() {
            return;
        }
        let (collection_name, items): (String, Vec<OrganiserItem>) = match collection_id {
            Some(cid) => {
                let Some(col) = self
                    .workspace
                    .organiser_collections
                    .iter()
                    .find(|c| c.id == cid)
                else {
                    self.fail("未找到要导出的 Organizer 集合。", cx);
                    return;
                };
                let id_set: HashSet<mio_core::Uuid> = col.item_ids.iter().copied().collect();
                let list = self
                    .workspace
                    .organiser_items
                    .iter()
                    .filter(|item| id_set.contains(&item.id))
                    .cloned()
                    .collect();
                (col.name.clone(), list)
            }
            None => (
                "Inbox".to_owned(),
                self.workspace.organiser_items.clone(),
            ),
        };
        let export_doc = serde_json::json!({
            "version": 1,
            "collection": collection_name,
            "count": items.len(),
            "items": items,
        });
        let json_payload = match serde_json::to_vec_pretty(&export_doc) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.fail(format!("序列化集合导出数据失败：{err}"), cx);
                return;
            }
        };
        let safe_name: String = collection_name
            .chars()
            .map(|ch| {
                if ch.is_alphanumeric() || matches!(ch, '-' | '_') {
                    ch
                } else {
                    '_'
                }
            })
            .take(48)
            .collect();
        let suggested = if safe_name.trim_matches('_').is_empty() {
            "organiser-collection.json".to_owned()
        } else {
            format!("organiser-{safe_name}.json")
        };
        let project = self.project;
        let picker = mio_core::Uuid::new_v4();
        self.file_picker = Some(picker);
        cx.notify();
        let prompt = crate::native_terminate::prompt_for_new_path(
            window,
            cx,
            &self.default_directory(),
            Some(&suggested),
        );
        let owner = self.owner_window;
        cx.spawn_in(window, async move |this, cx| {
            let result = prompt.await;
            let pending_path = owner
                .update(cx, |_, _, cx| {
                    this.update(cx, |view, cx| {
                        if !view.finish_file_picker(picker, cx) || !view.backend_available() {
                            return None;
                        }
                        if view.project != project {
                            view.fail("项目已切换，集合导出未写入磁盘。", cx);
                            return None;
                        }
                        match result {
                            Ok(Ok(Some(mut path))) => {
                                if path.extension().is_none() {
                                    path.set_extension("json");
                                }
                                Some(path)
                            }
                            Ok(Ok(None)) => None,
                            Ok(Err(error)) => {
                                view.fail(format!("无法选择保存路径：{error}"), cx);
                                None
                            }
                            Err(_) => {
                                view.fail("文件选择器已意外关闭。", cx);
                                None
                            }
                        }
                    })
                })
                .ok()
                .and_then(Result::ok)
                .flatten();
            let Some(path) = pending_path else {
                return;
            };
            let write_path = path.clone();
            let write_result = cx
                .background_executor()
                .spawn(async move {
                    mio_core::ui_files::write_export_file(&write_path, &json_payload)
                })
                .await;
            let _ = owner.update(cx, |_, _, cx| {
                this.update(cx, |view, cx| {
                    if view.project != project {
                        return;
                    }
                    match write_result {
                        Ok(()) => {
                            view.error = None;
                            view.status = format!(
                                "已导出集合「{collection_name}」（{} 条）到：{}",
                                items.len(),
                                path.display()
                            );
                            cx.notify();
                        }
                        Err(err) => {
                            view.fail(format!("写入集合导出文件失败：{err}"), cx);
                        }
                    }
                })
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn sort_organiser(
        &mut self,
        sort: OrganiserSort,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(err) = self.validate_organiser_composition(window, cx) {
            self.sync_organiser_table(window, cx);
            self.fail(err, cx);
            return;
        }
        self.organiser.sort = sort;
        self.sync_organiser_table(window, cx);
    }

    pub(in crate::workbench) fn set_selected_organiser_status(
        &mut self,
        status: OrganiserStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.organiser.selected_ids.len() > 1 {
            self.set_batch_organiser_status(status, window, cx);
            return;
        }
        if let Some(id) = self.organiser.selected_id {
            self.set_organiser_item_status(id, status, window, cx);
        }
    }

    pub(in crate::workbench) fn set_organiser_item_status(
        &mut self,
        id: mio_core::Uuid,
        status: OrganiserStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() {
            return;
        }
        if let Err(err) = self.validate_organiser_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let Some(item) = self
            .workspace
            .organiser_items
            .iter_mut()
            .find(|item| item.id == id)
        else {
            return;
        };
        if item.workflow_status == status {
            return;
        }
        item.workflow_status = status;
        let number = item.number;
        self.sync_organiser_table(window, cx);
        self.save_workspace(cx);
        self.error = None;
        self.status = format!(
            "已将 Organiser #{number} 状态更新为「{}」。",
            status.label()
        );
        cx.notify();
    }

    pub(in crate::workbench) fn set_selected_organiser_highlight(
        &mut self,
        highlight: Option<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.organiser.selected_ids.len() > 1 {
            self.set_batch_organiser_highlight(highlight, window, cx);
            return;
        }
        if let Some(id) = self.organiser.selected_id {
            self.set_organiser_item_highlight(id, highlight, window, cx);
        }
    }

    pub(in crate::workbench) fn set_organiser_item_highlight(
        &mut self,
        id: mio_core::Uuid,
        highlight: Option<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() || highlight.is_some_and(|c| !(1..=9).contains(&c)) {
            return;
        }
        if let Err(err) = self.validate_organiser_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let Some(item) = self
            .workspace
            .organiser_items
            .iter_mut()
            .find(|item| item.id == id)
        else {
            return;
        };
        if item.highlight == highlight {
            return;
        }
        item.highlight = highlight;
        self.sync_organiser_table(window, cx);
        self.save_workspace(cx);
        self.error = None;
        cx.notify();
    }

    pub(in crate::workbench) fn save_selected_organiser_notes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.organiser_editable() {
            return;
        }
        if let Err(err) = self.validate_organiser_composition(window, cx) {
            self.fail(err, cx);
            return;
        }
        let Some(id) = self.organiser.selected_id else {
            self.fail("请先选中一条 Organiser 条目。", cx);
            return;
        };
        let text = self.organiser.notes.read(cx).content().text().to_owned();
        if text.len() > MAX_ORGANISER_NOTES_BYTES {
            self.fail("备注超过 64 KiB 上限；当前输入仍保留，请缩短后重试。", cx);
            return;
        }
        if text
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
        {
            self.fail("备注包含非法控制字符；当前输入仍保留。", cx);
            return;
        }
        let Some(idx) = self
            .workspace
            .organiser_items
            .iter()
            .position(|item| item.id == id)
        else {
            return;
        };
        let previous = std::mem::replace(
            &mut self.workspace.organiser_items[idx].notes,
            text.to_string(),
        );
        if let Err(err) = validate_organiser_items(&self.workspace.organiser_items) {
            self.workspace.organiser_items[idx].notes = previous;
            self.fail(format!("未保存备注：{err}"), cx);
            return;
        }
        let number = self.workspace.organiser_items[idx].number;
        self.organiser.notes_dirty = false;
        self.sync_organiser_table(window, cx);
        self.save_workspace(cx);
        self.error = None;
        self.status = format!("已保存 Organiser #{number} 备注。");
        cx.notify();
    }
}
