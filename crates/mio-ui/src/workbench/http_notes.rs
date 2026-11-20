use super::*;
use gpui_kit::component::input::EditorState;
use mio_core::{
    ExtHttpNotes, ExtHttpNotesChange, ExtHttpNotesError, HistoryEntryId, HistoryRow, Uuid,
};

const MAX_DRAFTS: usize = 64;
const MAX_NOTE_BYTES: usize = 64 * 1024;

mod batch;
mod drafts;
pub(in crate::workbench) mod render;

pub(super) struct HttpNotes {
    pub(super) open: bool,
    pub(super) preview_mode: bool,
    pub(super) editor: Entity<EditorState>,
    pub(super) detached_window: Option<gpui_kit::AnyWindowHandle>,
    pub(super) detached_owner_window: Option<gpui_kit::AnyWindowHandle>,
    pub(in crate::workbench) active: Option<(Uuid, Uuid)>,
    stored: Option<ExtHttpNotes>,
    read_revision: u64,
    reading: bool,
    error: Option<String>,
    drafts: HashMap<(Uuid, Uuid), Draft>,
    batch: Option<batch::BatchNotes>,
    _subscription: Subscription,
}

struct Draft {
    text: String,
    stored: ExtHttpNotes,
    pending: Option<Pending>,
    error: Option<String>,
}
struct Pending {
    operation: Uuid,
    text: String,
    cancel: CancellationToken,
}

impl HttpNotes {
    pub(in crate::workbench) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("plaintext")
                .line_number(false)
                .soft_wrap(true)
                .auto_close(false)
                .autocorrect(false)
        });
        let subscription = cx.subscribe(&editor, |view, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                match view.capture_http_notes_app(cx) {
                    Ok(()) => view.send_http_notes(false, cx),
                    Err(error) => {
                        view.http_notes.error = Some(error);
                        cx.notify();
                    }
                }
            }
        });
        Self {
            open: false,
            preview_mode: false,
            editor,
            detached_window: None,
            detached_owner_window: None,
            active: None,
            stored: None,
            read_revision: 0,
            reading: false,
            error: None,
            drafts: HashMap::new(),
            batch: None,
            _subscription: subscription,
        }
    }

    pub(in crate::workbench) fn disconnect(&mut self) {
        self.reading = false;
        if let Some(batch) = &mut self.batch {
            batch.disconnect();
        }
        for draft in self.drafts.values_mut() {
            if let Some(pending) = &draft.pending {
                pending.cancel.cancel();
            }
            draft.error = Some("后台服务已断开；备注保存结果未确认，草稿仍保留。".into());
        }
        if self.active.is_some() {
            self.error = Some("后台服务已断开；备注草稿仍保留。".into());
        }
    }

    pub(in crate::workbench) fn reset(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(batch) = self.batch.take() {
            batch.close(cx);
        }
        if let Some(handle) = self.detached_window.take() {
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            });
        }
        self.detached_owner_window = None;
        for draft in self.drafts.values() {
            if let Some(pending) = &draft.pending {
                pending.cancel.cancel();
            }
        }
        self.drafts.clear();
        self.active = None;
        self.stored = None;
        self.reading = false;
        self.error = None;
        self.preview_mode = false;
        self.editor
            .update(cx, |editor, cx| editor.set_value("", window, cx));
    }

    pub(in crate::workbench) fn evict_deleted_live(
        &mut self,
        deleted: &HashSet<FlowId>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.drafts.retain(|&(source_id, item_id), draft| {
            let remove = source_id.is_nil() && deleted.contains(&item_id);
            if remove && let Some(pending) = &draft.pending {
                pending.cancel.cancel();
            }
            !remove
        });
        if self
            .active
            .is_some_and(|(source_id, item_id)| source_id.is_nil() && deleted.contains(&item_id))
        {
            self.open = false;
            self.active = None;
            self.stored = None;
            self.reading = false;
            self.error = None;
            self.preview_mode = false;
            self.editor
                .update(cx, |editor, cx| editor.set_value("", window, cx));
        }
    }
}

impl Workbench {
    pub(in crate::workbench) fn selected_http_notes(&self) -> Option<(Uuid, Uuid)> {
        if self.ext_import.visible {
            return None;
        }
        match self.tool {
            Tool::Proxy if self.proxy_page == ProxyPage::History => match self.selected_history {
                Some(HistoryEntryId::External { source_id, item_id }) => Some((source_id, item_id)),
                Some(HistoryEntryId::Live(flow_id)) => Some((Uuid::nil(), flow_id)),
                None => match self.imported_history.row.as_ref().map(HistoryRow::id) {
                    Some(HistoryEntryId::External { source_id, item_id }) => Some((source_id, item_id)),
                    Some(HistoryEntryId::Live(flow_id)) => Some((Uuid::nil(), flow_id)),
                    None => self
                        .detail
                        .as_ref()
                        .map(|record| (Uuid::nil(), record.summary.id)),
                },
            },
            Tool::Proxy if self.proxy_page == ProxyPage::Intercept => self
                .selected_intercept
                .and_then(|id| self.intercepts.iter().find(|pending| pending.id() == id))
                .and_then(|pending| match &pending.target {
                    InterceptTarget::Http { record, .. } => Some((Uuid::nil(), record.summary.id)),
                    InterceptTarget::WebSocket { .. } => None,
                }),
            Tool::Target if self.target_pane.page == target::TargetPage::SiteMap => self
                .target_pane
                .selected_id
                .or_else(|| self.target_pane.selected_row.as_ref().map(HistoryRow::id))
                .filter(|id| !self.target_is_draft_entry(*id))
                .map(|entry_id| match entry_id {
                    HistoryEntryId::External { source_id, item_id } => (source_id, item_id),
                    HistoryEntryId::Live(flow_id) => (Uuid::nil(), flow_id),
                }),
            Tool::Logger => self
                .extra_tools
                .logger_selected
                .map(|flow_id| (Uuid::nil(), flow_id)),
            Tool::Dashboard if !self.audit_dashboard.new_scan_open => self
                .audit_dashboard
                .selected_finding_id
                .and_then(|finding_id| {
                    self.audit_dashboard
                        .snapshot
                        .findings
                        .iter()
                        .find(|finding| finding.id == finding_id)
                        .and_then(|finding| finding.flow_id)
                })
                .map(|flow_id| (Uuid::nil(), flow_id)),
            _ => None,
        }
    }

    pub(in crate::workbench) fn reset_http_notes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.http_notes.reset(window, cx);
        self.history
            .read(cx)
            .delegate()
            .live_notes
            .borrow_mut()
            .clear();
    }

    pub(in crate::workbench) fn http_notes_disconnected(&mut self, cx: &mut Context<Self>) {
        self.http_notes.disconnect();
        cx.notify();
    }

    pub(in crate::workbench) fn open_http_notes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tool == Tool::Proxy && self.proxy_page == ProxyPage::History {
            let ids: Vec<_> = self
                .history
                .read(cx)
                .delegate()
                .selection_rows()
                .into_iter()
                .map(HistoryRow::id)
                .collect();
            if ids.len() > 1 {
                self.open_http_notes_batch(ids, window, cx);
                return;
            }
        }
        if self.selected_http_notes().is_none() || !self.leave_http_notes(window, cx) {
            return;
        }
        if !self.dismiss_inspector_for_current_selection(window, cx) {
            return;
        }
        self.http_notes.open = true;
        self.select_http_notes(window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn validate_http_notes_composition(
        &self,
        _window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        if self.http_notes.editor.read(cx).has_marked_text() {
            return Err(editing::COMPOSITION_PENDING.to_owned());
        }
        Ok(())
    }

    pub(in crate::workbench) fn http_notes_pending(&self) -> bool {
        self.http_notes
            .batch
            .as_ref()
            .is_some_and(batch::BatchNotes::pending)
            || self
                .http_notes
                .drafts
                .values()
                .any(|draft| draft.pending.is_some())
    }

    pub(in crate::workbench) fn snapshot_http_notes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Vec<ExtHttpNotesChange>, String> {
        self.capture_http_notes(window, cx)?;
        if self.http_notes_pending() {
            return Err("备注保存仍在确认；请等待当前回执后保存项目。".into());
        }
        Ok(self
            .http_notes
            .drafts
            .iter()
            .map(|(id, draft)| ExtHttpNotesChange {
                source_id: id.0,
                item_id: id.1,
                expected_notes_revision: draft.stored.revision,
                text: draft.text.clone(),
            })
            .collect())
    }

    pub(in crate::workbench) fn accept_saved_http_notes(
        &mut self,
        submitted: &[ExtHttpNotesChange],
        receipts: &[(Uuid, Uuid, u64)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if submitted.len() != receipts.len()
            || submitted.iter().any(|change| {
                receipts
                    .iter()
                    .filter(|(source_id, item_id, revision)| {
                        *source_id == change.source_id
                            && *item_id == change.item_id
                            && *revision >= change.expected_notes_revision
                            && *revision <= change.expected_notes_revision.saturating_add(1)
                    })
                    .count()
                    != 1
            })
        {
            return Err("项目保存的备注回执不匹配；本地草稿仍保留。".into());
        }
        for change in submitted {
            let revision = receipts
                .iter()
                .find(|(source_id, item_id, _)| {
                    *source_id == change.source_id && *item_id == change.item_id
                })
                .unwrap()
                .2;
            let id = (change.source_id, change.item_id);
            let stored = ExtHttpNotes {
                revision,
                text: change.text.clone(),
            };
            if let Some(draft) = self.http_notes.drafts.get_mut(&id) {
                draft.stored = stored.clone();
                draft.error = None;
                if draft.pending.is_none() && draft.text == change.text {
                    self.http_notes.drafts.remove(&id);
                }
            }
            if self.http_notes.active == Some(id) {
                self.http_notes.stored = Some(stored.clone());
                self.http_notes.error = None;
            }
            self.update_http_notes_summary(id, &stored, cx);
        }
        let _ = self.capture_http_notes(window, cx);
        self.history_dirty = true;
        cx.notify();
        Ok(())
    }

    fn update_http_notes_summary(
        &mut self,
        id: (Uuid, Uuid),
        stored: &ExtHttpNotes,
        cx: &mut Context<Self>,
    ) {
        let preview: String = stored.text.chars().take(256).collect();
        if let Some(HistoryRow::ExtProxy(row)) = self.imported_history.row.as_mut()
            && (row.item.source_id, row.item.id) == id
            && row.notes_revision <= stored.revision
        {
            row.notes_revision = stored.revision;
            row.notes_preview = preview.clone();
            row.notes_error = None;
        }
        self.history.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            if id.0.is_nil()
                && delegate
                    .live_notes
                    .borrow()
                    .get(&id.1)
                    .is_none_or(|(revision, _)| *revision <= stored.revision)
            {
                delegate
                    .live_notes
                    .borrow_mut()
                    .insert(id.1, (stored.revision, preview.clone()));
            }
            for row in delegate.cached_rows_mut() {
                if let HistoryRow::ExtProxy(row) = row
                    && (row.item.source_id, row.item.id) == id
                    && row.notes_revision <= stored.revision
                {
                    row.notes_revision = stored.revision;
                    row.notes_preview = preview.clone();
                    row.notes_error = None;
                }
            }
            table.refresh(cx);
        });
    }

    pub(in crate::workbench) fn refresh_http_notes_rows(&self, rows: &mut [HistoryRow], cx: &App) {
        if self.project.is_some() {
            let delegate = self.history.read(cx).delegate();
            for (flow_id, notes_revision, notes_preview, _) in &self.live_http_annotations {
                let flow_id = *flow_id;
                let mut notes_revision = *notes_revision;
                let mut notes_preview = notes_preview.clone();
                let id = (Uuid::nil(), flow_id);
                let stored = self
                    .http_notes
                    .drafts
                    .get(&id)
                    .map(|draft| &draft.stored)
                    .or_else(|| {
                        (self.http_notes.active == Some(id))
                            .then_some(self.http_notes.stored.as_ref())
                            .flatten()
                    });
                if let Some(stored) = stored
                    && notes_revision < stored.revision
                {
                    notes_revision = stored.revision;
                    notes_preview = stored.text.chars().take(256).collect();
                }
                if delegate
                    .live_notes
                    .borrow()
                    .get(&flow_id)
                    .is_none_or(|(rev, _)| *rev <= notes_revision)
                {
                    delegate
                        .live_notes
                        .borrow_mut()
                        .insert(flow_id, (notes_revision, notes_preview));
                }
            }
        }
        for row in rows {
            let HistoryRow::ExtProxy(row) = row else {
                continue;
            };
            let id = (row.item.source_id, row.item.id);
            let stored = self
                .http_notes
                .drafts
                .get(&id)
                .map(|draft| &draft.stored)
                .or_else(|| {
                    (self.http_notes.active == Some(id))
                        .then_some(self.http_notes.stored.as_ref())
                        .flatten()
                });
            if let Some(stored) = stored
                && row.notes_revision < stored.revision
            {
                row.notes_revision = stored.revision;
                row.notes_preview = stored.text.chars().take(256).collect();
                row.notes_error = None;
            }
            if let Some(HistoryRow::ExtProxy(previous)) = self
                .history
                .read(cx)
                .delegate()
                .row_by_id(HistoryEntryId::External {
                    source_id: id.0,
                    item_id: id.1,
                })
                && row.notes_revision < previous.notes_revision
            {
                row.notes_revision = previous.notes_revision;
                row.notes_preview = previous.notes_preview.clone();
                row.notes_error = previous.notes_error.clone();
            }
        }
    }

    pub(in crate::workbench) fn capture_http_notes(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.capture_http_notes_app(cx)
    }

    pub(in crate::workbench) fn capture_http_notes_app(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let notes = &mut self.http_notes;
        let (Some(id), Some(stored)) = (notes.active, notes.stored.clone()) else {
            return Ok(());
        };
        if notes.editor.read(cx).has_marked_text() {
            return Err(editing::COMPOSITION_PENDING.to_owned());
        }
        let text = notes.editor.read(cx).value().to_string();
        if text.len() > MAX_NOTE_BYTES {
            return Err("备注超过 64 KiB；当前输入仍保留，请缩短后重试。".into());
        }
        if let Some(draft) = notes.drafts.get_mut(&id) {
            if draft.text != text {
                draft.text = text;
                draft.error = None;
            }
            let owned_by_project_save = self.project_saves_in_flight > 0
                && self.project_http_notes_save.as_ref().is_some_and(|batch| {
                    batch
                        .iter()
                        .any(|change| (change.source_id, change.item_id) == id)
                });
            if draft.pending.is_none() && !owned_by_project_save && draft.text == draft.stored.text
            {
                notes.drafts.remove(&id);
            }
        } else if text != stored.text {
            if notes.drafts.len() >= MAX_DRAFTS {
                return Err("已有 64 条备注待保存；当前输入仍保留，请先重试保存。".into());
            }
            notes.drafts.insert(
                id,
                Draft {
                    text,
                    stored,
                    pending: None,
                    error: None,
                },
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
