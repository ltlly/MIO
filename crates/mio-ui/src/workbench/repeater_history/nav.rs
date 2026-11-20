use super::*;

impl Workbench {
    pub(in crate::workbench) fn finish_repeater_history(
        &mut self,
        tab_id: TabId,
        draft_revision: u64,
        record: &FlowRecord,
        persisted: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.invalidate_repeater_history(tab_id, cx);
        self.invalidate_archive_response(tab_id, cx);
        self.repeater_reads.retain(|_, id| *id != tab_id);
        let mut result = match &record.summary.state {
            mio_core::FlowState::Failed(reason) => format!("失败：{reason}"),
            state => format!(
                "{} · {} ms · {} 字节",
                state.label(),
                record.summary.elapsed_ms,
                record.summary.response_bytes
            ),
        };
        if !persisted {
            result.push_str("（尚未写入项目）");
        }
        if self.transition.is_none() {
            self.status = format!("Repeater {result}");
        }
        if self.close_after_send.remove(&tab_id) {
            self.close_tab(tab_id, window, cx);
            return;
        }
        let Some(tab) = self
            .workspace
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .cloned()
        else {
            return;
        };
        let clean = if let Some(editors) = self.tab_editors.get(&tab_id) {
            (tab.draft.revision == draft_revision
                || tab
                    .history
                    .as_ref()
                    .and_then(|history| history.cursor.as_ref())
                    .is_some_and(|cursor| cursor.draft_revision == tab.draft.revision))
                && !editors.dirty
                && editors
                    .request
                    .bytes(window, cx)
                    .is_ok_and(|raw| raw == tab.draft.raw)
                && editing::input_snapshot(&editors.target, window, cx)
                    .is_ok_and(|target| target.as_ref() == tab.draft.target)
        } else {
            false
        };
        if !persisted {
            self.workspace_dirty = true;
        }
        if clean && self.transition.is_none() {
            self.begin_history_selection(
                tab,
                None,
                Some(RepeaterHistoryEntryId::Live(record.summary.id)),
                cx,
            );
            if self.workspace_dirty {
                self.save_workspace(cx);
            }
            self.resume_history_navigation(window, cx);
        } else {
            if let Some(editors) = self.tab_editors.get_mut(&tab_id) {
                editors.result = Some(format!(
                    "{result}；新结果已记录，当前消息保留，可从历史选择。"
                ));
            }
            self.refresh_repeater_history(window, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn resume_history_navigation(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pending_workspace.is_some()
            || self.workspace_dirty
            || self
                .repeater_history
                .pending
                .values()
                .any(|pending| pending.workspace_revision.is_some())
        {
            return;
        }
        let ids: Vec<_> = self.repeater_history.pending.keys().copied().collect();
        for tab_id in ids {
            let pending = &self.repeater_history.pending[&tab_id];
            if self.project != Some(pending.project)
                || self.transition.is_some()
                || (pending.position.is_some()
                    && (self.workspace.active != Some(tab_id) || self.tool != Tool::Repeater))
                || self.workspace.tabs.iter().find(|tab| tab.id == tab_id) != Some(&pending.base)
            {
                self.history_selection_failed(
                    tab_id,
                    "历史选择或当前草稿已改变，未应用旧导航。".into(),
                    cx,
                );
                continue;
            }
            let Some(entry) = pending.entry else { continue };
            let workspace_revision = self.workspace.revision;
            let command = Command::SelectRepeaterHistory {
                project: pending.project,
                operation_id: pending.operation,
                tab_id,
                entry,
                workspace_revision,
                draft_revision: pending.base.draft.revision,
                cancel: pending.cancel.clone(),
            };
            if self.enqueue(command, cx) {
                self.repeater_history
                    .pending
                    .get_mut(&tab_id)
                    .unwrap()
                    .workspace_revision = Some(workspace_revision);
            } else {
                self.history_selection_failed(
                    tab_id,
                    self.error
                        .clone()
                        .unwrap_or_else(|| "历史导航未发送。".into()),
                    cx,
                );
            }
            cx.notify();
            break;
        }
    }

    pub(in crate::workbench) fn on_repeater_history(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            Event::RepeaterHistoryPage {
                project,
                query_revision,
                tab_id,
                result,
            } if self.project == Some(project)
                && self.repeater_history.owner == Some((project, tab_id))
                && self.repeater_history.query_revision == query_revision =>
            {
                self.repeater_history.querying = false;
                match result {
                    Ok(page)
                        if page.start == self.repeater_history.query_start
                            && page.rows.len() <= HISTORY_PAGE_SIZE
                            && page
                                .start
                                .checked_add(page.rows.len() as u64)
                                .is_some_and(|end| end <= page.total)
                            && page
                                .current_position
                                .is_none_or(|position| position < page.total)
                            && page
                                .rows
                                .iter()
                                .enumerate()
                                .all(|(index, row)| row.position == page.start + index as u64) =>
                    {
                        if let Some(pending) = self.repeater_history.pending.get_mut(&tab_id)
                            && let Some(position) = pending.position
                        {
                            pending.entry = page
                                .rows
                                .iter()
                                .find(|row| row.position == position)
                                .map(|row| row.id);
                            if pending.entry.is_none() {
                                self.history_navigation_failed(
                                    "目标历史项已改变，请重新选择。".into(),
                                    cx,
                                );
                                return;
                            }
                        }
                        self.repeater_history.page = Some(page);
                        self.repeater_history.error = None;
                        self.resume_history_navigation(window, cx);
                        self.refresh_history_menu(window, cx);
                    }
                    Ok(_) => self.history_navigation_failed(
                        "Repeater 历史页范围不匹配，当前消息保留。".into(),
                        cx,
                    ),
                    Err(error) => self.history_navigation_failed(error, cx),
                }
            }
            Event::RepeaterHistorySelected {
                project,
                operation_id,
                tab_id,
                entry,
                workspace_revision,
                draft_revision,
                result,
            } => {
                let Some(pending) = self.repeater_history.pending.get(&tab_id) else {
                    return;
                };
                if self.project != Some(project)
                    || pending.project != project
                    || pending.operation != operation_id
                    || pending.entry != Some(entry)
                    || pending.workspace_revision != Some(workspace_revision)
                    || pending.base.draft.revision != draft_revision
                {
                    return;
                }
                if pending.position.is_none() {
                    let unchanged = self.tab_editors.get(&tab_id).is_some_and(|editors| {
                        !editors.dirty
                            && editors
                                .request
                                .bytes(window, cx)
                                .is_ok_and(|raw| raw == pending.base.draft.raw)
                            && editing::input_snapshot(&editors.target, window, cx)
                                .is_ok_and(|target| target.as_ref() == pending.base.draft.target)
                    });
                    if !unchanged {
                        self.history_selection_failed(
                            tab_id,
                            "新结果已记录；读取期间新增的编辑或输入法组词仍保留。".into(),
                            cx,
                        );
                        return;
                    }
                } else {
                    match self.capture_drafts(window, cx) {
                        Ok(changed) => self.workspace_dirty |= changed,
                        Err(error) => {
                            self.history_selection_failed(tab_id, error, cx);
                            return;
                        }
                    }
                }
                let pending = self.repeater_history.pending.remove(&tab_id).unwrap();
                if (pending.position.is_some()
                    && (self.workspace.active != Some(tab_id) || self.tool != Tool::Repeater))
                    || self.transition.is_some()
                    || self.workspace.tabs.iter().find(|tab| tab.id == tab_id)
                        != Some(&pending.base)
                {
                    self.history_selection_failed(
                        tab_id,
                        "读取期间当前草稿或选择已变化，未覆盖新输入。".into(),
                        cx,
                    );
                    return;
                }
                let tab = match result {
                    Ok(tab) => tab,
                    Err(error) => {
                        self.history_selection_failed(tab_id, error, cx);
                        return;
                    }
                };
                let same_source = tab
                    .history
                    .as_ref()
                    .and_then(|history| history.source.as_ref())
                    == pending
                        .base
                        .history
                        .as_ref()
                        .and_then(|history| history.source.as_ref());
                let valid = tab.id == tab_id
                    && tab.title == pending.base.title
                    && tab.group == pending.base.group
                    && Some(tab.draft.revision) == draft_revision.checked_add(1)
                    && same_source
                    && tab
                        .history
                        .as_ref()
                        .and_then(|history| history.cursor.as_ref())
                        .is_some_and(|cursor| {
                            cursor.entry == entry && cursor.draft_revision == tab.draft.revision
                        })
                    && match entry {
                        RepeaterHistoryEntryId::Live(id) => {
                            tab.last_flow == Some(id) && tab.archive_response.is_none()
                        }
                        RepeaterHistoryEntryId::Archive(_) => tab.last_flow.is_none(),
                    }
                    && tab.archive_response.as_ref().is_none_or(|reference| {
                        reference.draft_revision == tab.draft.revision
                            && tab
                                .history
                                .as_ref()
                                .and_then(|history| history.source.as_ref())
                                .is_some_and(|source| source.source_id == reference.source_id)
                    });
                if !valid {
                    self.history_selection_failed(
                        tab_id,
                        "历史提案与请求的标签、条目或 revision 不一致。".into(),
                        cx,
                    );
                    return;
                }
                let mut workspace = self.workspace.clone();
                *workspace
                    .tabs
                    .iter_mut()
                    .find(|current| current.id == tab_id)
                    .unwrap() = tab.clone();
                if let Err(error) = workspace.validate_structure() {
                    self.history_selection_failed(tab_id, error.to_string(), cx);
                    return;
                }
                self.workspace = workspace;
                let visible = self.tool == Tool::Repeater && self.workspace.active == Some(tab_id);
                if visible {
                    self.focus.focus(window, cx);
                }
                if let Some(editors) = self.tab_editors.get_mut(&tab_id) {
                    editors.response.load(&[], window, cx);
                    editors.response_revision = None;
                    editors.loaded_flow = None;
                    editors.result = None;
                }
                self.repeater_reads.retain(|_, id| *id != tab_id);
                self.invalidate_archive_response(tab_id, cx);
                self.sync_tab_editors(window, cx);
                if let Some(flow_id) = tab.last_flow {
                    if let Some(revision) = self.selection_revision.checked_add(1) {
                        self.selection_revision = revision;
                        if self.enqueue(
                            Command::ReadFlow {
                                project,
                                selection_revision: revision,
                                flow_id,
                            },
                            cx,
                        ) {
                            self.repeater_reads.insert(revision, tab_id);
                        }
                    } else {
                        self.fail("响应读取 revision 已耗尽。", cx);
                    }
                }
                if self
                    .repeater_history
                    .owner
                    .is_some_and(|(_, id)| id == tab_id)
                {
                    self.repeater_history.error = None;
                    if let Some(page) = self.repeater_history.page.as_mut() {
                        page.current_position = pending.position;
                    }
                }
                if visible {
                    self.status = if pending.position.is_none() {
                        "已显示本次发送结果。"
                    } else {
                        "已切换 Repeater 历史；未发送。"
                    }
                    .into();
                }
                self.save_workspace(cx);
                if pending.position.is_none()
                    && self
                        .repeater_history
                        .owner
                        .is_some_and(|(_, id)| id == tab_id)
                {
                    self.repeater_history.page = None;
                    self.refresh_repeater_history(window, cx);
                }
            }
            _ => {}
        }
        cx.notify();
    }
}
