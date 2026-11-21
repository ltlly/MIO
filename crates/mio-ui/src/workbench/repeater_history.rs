use gpui_kit::component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use mio_core::{RepeaterHistoryEntryId, RepeaterHistoryPage};

use super::*;

mod menu;
mod nav;

const HISTORY_PAGE_SIZE: usize = 64;

#[derive(Default)]
pub(super) struct HistoryNavigation {
    pub(super) owner: Option<(ProjectKey, TabId)>,
    pub(super) query_revision: u64,
    pub(super) query_start: u64,
    pub(super) querying: bool,
    pub(super) page: Option<RepeaterHistoryPage>,
    pub(super) error: Option<String>,
    pub(super) pending: HashMap<TabId, Selection>,
    pub(super) menu: Option<(TabId, bool, gpui_kit::WeakEntity<PopupMenu>)>,
}

pub(in crate::workbench) struct Selection {
    pub(super) project: ProjectKey,
    pub(super) operation: mio_core::Uuid,
    pub(super) position: Option<u64>,
    pub(super) entry: Option<RepeaterHistoryEntryId>,
    pub(super) base: WorkTab,
    pub(super) workspace_revision: Option<u64>,
    pub(super) cancel: CancellationToken,
}

impl Workbench {
    pub(in crate::workbench) fn refresh_repeater_history(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available() {
            return;
        }
        let owner = if self.tool == Tool::Repeater {
            self.project
                .zip(self.workspace.active)
                .filter(|(_, id)| self.workspace.tabs.iter().any(|tab| tab.id == *id))
        } else {
            None
        };
        if self.repeater_history.owner != owner {
            if let Some((_, previous)) = self.repeater_history.owner
                && self
                    .repeater_history
                    .pending
                    .get(&previous)
                    .is_some_and(|pending| pending.position.is_some())
                && let Some(pending) = self.repeater_history.pending.remove(&previous)
            {
                pending.cancel.cancel();
            }
            self.repeater_history.owner = owner;
            self.repeater_history.page = None;
            self.repeater_history.querying = false;
            self.repeater_history.error = None;
            self.repeater_history.menu = None;
        }
        if owner.is_some()
            && self.repeater_history.page.is_none()
            && !self.repeater_history.querying
            && self.repeater_history.error.is_none()
        {
            self.repeater_history.querying = true;
            let revision = self.repeater_history.query_revision;
            let view = cx.weak_entity();
            cx.defer(move |cx| {
                let _ = view.update(cx, |view, cx| {
                    if view.repeater_history.owner != owner
                        || view.repeater_history.query_revision != revision
                        || !view.repeater_history.querying
                    {
                        return;
                    }
                    view.repeater_history.querying = false;
                    if !view.backend_available()
                        || view.pending_workspace.is_some()
                        || view.workspace_dirty
                        || view.transition.is_some()
                    {
                        cx.notify();
                        return;
                    }
                    let previous_error = view.error.clone();
                    view.query_repeater_history(0, cx);
                    if view.repeater_history.querying && view.error.is_none() {
                        view.error = previous_error;
                    }
                });
            });
        }
    }

    pub(in crate::workbench) fn query_repeater_history(
        &mut self,
        start: u64,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available() {
            return;
        }
        let Some((project, tab_id)) = self.repeater_history.owner else {
            return;
        };
        self.repeater_history.query_revision += 1;
        let revision = self.repeater_history.query_revision;
        self.repeater_history.query_start = start;
        self.repeater_history.querying = false;
        if self.enqueue(
            Command::QueryRepeaterHistory {
                project,
                query_revision: revision,
                tab_id,
                start,
                limit: HISTORY_PAGE_SIZE,
            },
            cx,
        ) {
            self.repeater_history.querying = true;
            self.repeater_history.error = None;
        } else {
            self.history_navigation_failed(
                self.error
                    .clone()
                    .unwrap_or_else(|| "历史查询未发送。".into()),
                cx,
            );
        }
        cx.notify();
    }

    pub(in crate::workbench) fn history_selection_failed(
        &mut self,
        tab_id: TabId,
        error: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(pending) = self.repeater_history.pending.remove(&tab_id) {
            pending.cancel.cancel();
        }
        if self
            .repeater_history
            .owner
            .is_some_and(|(_, id)| id == tab_id)
        {
            self.repeater_history.error = Some(error.clone());
        }
        if let Some(editors) = self.tab_editors.get_mut(&tab_id) {
            editors.result = Some(error.clone());
        }
        self.fail(error, cx);
        if !self.repeater_history.pending.is_empty() {
            let owner = self.owner_window;
            let view = cx.weak_entity();
            cx.defer(move |cx| {
                let _ = owner.update(cx, |_, window, cx| {
                    let _ = view.update(cx, |view, cx| view.resume_history_navigation(window, cx));
                });
            });
        }
    }

    pub(in crate::workbench) fn history_navigation_failed(
        &mut self,
        error: String,
        cx: &mut Context<Self>,
    ) {
        if let Some((_, id)) = self.repeater_history.owner {
            self.history_selection_failed(id, error, cx);
        } else {
            self.fail(error, cx);
        }
    }

    pub(in crate::workbench) fn invalidate_repeater_history(
        &mut self,
        tab_id: TabId,
        cx: &mut Context<Self>,
    ) {
        if let Some(pending) = self.repeater_history.pending.remove(&tab_id) {
            pending.cancel.cancel();
        }
        if self
            .repeater_history
            .owner
            .is_some_and(|(_, id)| id == tab_id)
        {
            self.repeater_history.page = None;
            self.repeater_history.querying = false;
            self.repeater_history.error = None;
            self.repeater_history.query_revision =
                self.repeater_history.query_revision.saturating_add(1);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn history_disconnected(&mut self, cx: &mut Context<Self>) {
        self.repeater_history.querying = false;
        for (_, pending) in self.repeater_history.pending.drain() {
            pending.cancel.cancel();
        }
        if self.repeater_history.owner.is_some() {
            self.history_navigation_failed("后台已断开；历史读取未确认，当前消息保留。".into(), cx);
        }
    }

    pub(in crate::workbench) fn history_workspace_rejected(
        &mut self,
        message: &str,
        cx: &mut Context<Self>,
    ) {
        let pending = std::mem::take(&mut self.repeater_history.pending);
        for (id, selection) in pending {
            selection.cancel.cancel();
            self.history_selection_failed(
                id,
                format!("工作区保存失败；未切换历史，当前消息保留：{message}"),
                cx,
            );
        }
    }

    pub(in crate::workbench) fn navigate_repeater_history(
        &mut self,
        tab_id: TabId,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tool != Tool::Repeater
            || self.workspace.active != Some(tab_id)
            || self.transition.is_some()
            || !self.backend_available()
            || self.repeater_history.pending.contains_key(&tab_id)
            || self.repeater_history.querying
        {
            return;
        }
        let Some(page) = &self.repeater_history.page else {
            return;
        };
        let Some(current) = page.current_position else {
            return;
        };
        let position = if forward {
            current.checked_add(1).filter(|next| *next < page.total)
        } else {
            current.checked_sub(1)
        };
        let Some(position) = position else { return };
        let entry = page
            .rows
            .iter()
            .find(|row| row.position == position)
            .map(|row| row.id);
        self.select_history_position(tab_id, position, entry, window, cx);
    }

    pub(in crate::workbench) fn select_history_position(
        &mut self,
        tab_id: TabId,
        position: u64,
        entry: Option<RepeaterHistoryEntryId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.active != Some(tab_id)
            || self.tool != Tool::Repeater
            || self.transition.is_some()
            || self.repeater_history.pending.contains_key(&tab_id)
        {
            return;
        }
        if !self.flush_workspace(window, cx) {
            return;
        }
        let Some(base) = self
            .workspace
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .cloned()
        else {
            return;
        };
        self.cancel_plugin_interactions(cx);
        self.begin_history_selection(base, Some(position), entry, cx);
        if entry.is_none() {
            self.query_repeater_history(
                position.saturating_sub((HISTORY_PAGE_SIZE / 2) as u64),
                cx,
            );
        }
        self.resume_history_navigation(window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn begin_history_selection(
        &mut self,
        base: WorkTab,
        position: Option<u64>,
        entry: Option<RepeaterHistoryEntryId>,
        cx: &mut Context<Self>,
    ) {
        let operation = mio_core::Uuid::new_v4();
        let cancel = CancellationToken::new();
        let Some(project) = self.project else { return };
        let tab_id = base.id;
        self.repeater_history.pending.insert(
            tab_id,
            Selection {
                project,
                operation,
                position,
                entry,
                base,
                workspace_revision: None,
                cancel: cancel.clone(),
            },
        );
        if self
            .repeater_history
            .owner
            .is_some_and(|(_, id)| id == tab_id)
        {
            self.repeater_history.error = None;
        }
        let owner = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(30))
                .await;
            let _ = owner.update(cx, |view, cx| {
                if view
                    .repeater_history
                    .pending
                    .get(&tab_id)
                    .is_some_and(|pending| pending.operation == operation)
                {
                    cancel.cancel();
                    view.history_selection_failed(
                        tab_id,
                        "Repeater 历史读取超时，当前消息保留；可重试。".into(),
                        cx,
                    );
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests;
