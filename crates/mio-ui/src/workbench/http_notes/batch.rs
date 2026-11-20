use super::*;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::{AnyWindowHandle, Bounds, WeakEntity, size};

mod editor;
mod runtime;
pub(in crate::workbench::http_notes) use editor::BatchEditor;
pub(in crate::workbench::http_notes) use runtime::effective_common;

type NoteId = (Uuid, Uuid);
type NoteState = (Uuid, Uuid, u64, bool);

pub(super) struct BatchNotes {
    id: Uuid,
    project: ProjectKey,
    pub(super) ids: Vec<NoteId>,
    window: Option<AnyWindowHandle>,
    read: Option<BatchRead>,
    write: Option<BatchWrite>,
    states: Vec<NoteState>,
    common: Option<(Vec<NoteId>, String)>,
    error: Option<String>,
    closing: bool,
    saved: Option<(u64, String)>,
}

struct BatchRead {
    operation: Uuid,
    ids: Vec<NoteId>,
    cancel: CancellationToken,
}

struct BatchWrite {
    operation: Uuid,
    changes: Vec<(Uuid, Uuid, u64)>,
    text: String,
    editor_revision: u64,
    local: HashMap<NoteId, String>,
    active: Option<(NoteId, u64)>,
    cancel: CancellationToken,
}

impl BatchNotes {
    pub(in crate::workbench) fn pending(&self) -> bool {
        self.read.is_some() || self.write.is_some()
    }

    pub(in crate::workbench) fn disconnect(&mut self) {
        if let Some(read) = &self.read {
            read.cancel.cancel();
        }
        if let Some(write) = &self.write {
            write.cancel.cancel();
        }
        self.error = Some("后台已断开；批量备注结果未确认，输入仍保留。".into());
    }

    pub(in crate::workbench) fn close(self, cx: &mut App) {
        if let Some(read) = self.read {
            read.cancel.cancel();
        }
        if let Some(write) = self.write {
            write.cancel.cancel();
        }
        if let Some(handle) = self.window {
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            });
        }
    }
}

impl Workbench {
    pub(in crate::workbench) fn block_http_notes_batch(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(batch) = &self.http_notes.batch else {
            return false;
        };
        if let Some(handle) = batch.window {
            cx.defer(move |cx| {
                let _ = crate::chrome::activate_popup_window(handle, cx);
            });
        }
        self.reject_quit(cx);
        self.fail(
            "请先保存或取消 Add notes 窗口中的批量备注；当前输入仍保留。",
            cx,
        );
        true
    }

    pub(in crate::workbench) fn open_http_notes_batch(
        &mut self,
        selected: Vec<HistoryEntryId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.block_http_notes_batch(cx) || !self.backend_available() || self.transition.is_some()
        {
            return;
        }
        let Some(project) = self.project else { return };
        if let Err(error) = self.capture_http_notes(window, cx) {
            self.fail(error, cx);
            return;
        }
        if self.http_notes_pending()
            || self.project_saves_in_flight > 0
            || self.file_picker_pending()
        {
            self.fail(
                "备注或项目保存尚未确认；请在当前操作完成后添加批量备注。",
                cx,
            );
            return;
        }
        let ids: Option<Vec<_>> = selected
            .into_iter()
            .map(|id| match id {
                HistoryEntryId::External { source_id, item_id } => Some((source_id, item_id)),
                HistoryEntryId::Live(flow_id) => Some((Uuid::nil(), flow_id)),
            })
            .collect();
        let Some(ids) = ids.filter(|ids| (2..=500).contains(&ids.len())) else {
            self.fail(
                "批量备注需要选中 2 至 500 条 HTTP 历史记录；未修改任何备注。",
                cx,
            );
            return;
        };
        let id = Uuid::new_v4();
        self.http_notes.batch = Some(BatchNotes {
            id,
            project,
            ids,
            window: None,
            read: None,
            write: None,
            states: Vec::new(),
            common: None,
            error: None,
            closing: false,
            saved: None,
        });
        let owner = cx.entity();
        let owner_window = window.window_handle();
        let scale = theme::ui_scale().max(1.0);
        let target_w = px((415. * scale).round());
        let content_h = px((248. * scale).round());
        let desired_h = if cfg!(test) {
            content_h
        } else {
            content_h + px(crate::chrome::CUSTOM_TITLE_BAR_HEIGHT)
        };
        let bounds = if let Some(display) = window.display(cx).or_else(|| cx.primary_display()) {
            let visible = display.visible_bounds();
            let fitted = size(
                target_w.min(visible.size.width),
                desired_h.min(visible.size.height),
            );
            let mut bounds = Bounds::centered(Some(display.id()), fitted, cx);
            bounds.origin.x = bounds.origin.x.clamp(
                visible.origin.x,
                (visible.right() - fitted.width).max(visible.origin.x),
            );
            bounds.origin.y = bounds.origin.y.clamp(
                visible.origin.y,
                (visible.bottom() - fitted.height).max(visible.origin.y),
            );
            bounds
        } else {
            Bounds::centered(None, size(target_w, desired_h), cx)
        };
        cx.defer(move |cx| {
            if owner
                .read(cx)
                .http_notes
                .batch
                .as_ref()
                .is_none_or(|batch| batch.id != id)
            {
                return;
            }
            let opened = crate::chrome::open_popup_window(
                crate::chrome::mio_window_options(bounds, false, false),
                cx,
                |window, cx| {
                    cx.new(|cx| BatchEditor::new(owner.clone(), owner_window, id, window, cx))
                },
            );
            owner.update(cx, |view, cx| match opened {
                Ok((handle, _)) => {
                    let batch = view.http_notes.batch.as_mut().unwrap();
                    batch.window = Some(handle);
                    let ids = batch.ids.clone();
                    view.read_http_notes_batch(ids, cx);
                }
                Err(error) => {
                    view.http_notes.batch = None;
                    view.fail(format!("无法打开 Add notes：{error}"), cx);
                }
            });
        });
        cx.notify();
    }

    fn read_http_notes_batch(&mut self, ids: Vec<NoteId>, cx: &mut Context<Self>) {
        let Some(batch) = &self.http_notes.batch else {
            return;
        };
        if batch.pending() || batch.closing {
            return;
        }
        let id = batch.id;
        let project = batch.project;
        let operation = Uuid::new_v4();
        let cancel = CancellationToken::new();
        if self.enqueue(
            Command::ReadExtHttpNotesBatch {
                project,
                operation_id: operation,
                ids: ids.clone(),
                cancel: cancel.clone(),
            },
            cx,
        ) {
            let batch = self.http_notes.batch.as_mut().unwrap();
            batch.read = Some(BatchRead {
                operation,
                ids,
                cancel,
            });
            batch.error = None;
            self.http_notes_batch_deadline(id, operation, cx);
        } else {
            self.http_notes.batch.as_mut().unwrap().error = self.error.clone();
        }
        cx.notify();
    }

    fn http_notes_batch_deadline(&self, id: Uuid, operation: Uuid, cx: &mut Context<Self>) {
        cx.spawn(async move |owner, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(30))
                .await;
            let _ = owner.update(cx, |view, cx| {
                let Some(batch) = view
                    .http_notes
                    .batch
                    .as_mut()
                    .filter(|batch| batch.id == id)
                else {
                    return;
                };
                let cancel = batch
                    .read
                    .as_ref()
                    .filter(|read| read.operation == operation)
                    .map(|read| &read.cancel)
                    .or_else(|| {
                        batch
                            .write
                            .as_ref()
                            .filter(|write| write.operation == operation)
                            .map(|write| &write.cancel)
                    });
                if let Some(cancel) = cancel {
                    cancel.cancel();
                    batch.error =
                        Some("批量备注确认超时；已请求取消，输入保留，等待实际结果。".into());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn cancel_http_notes_batch(
        &mut self,
        id: Uuid,
        cx: &mut Context<Self>,
    ) {
        let Some(batch) = self
            .http_notes
            .batch
            .as_mut()
            .filter(|batch| batch.id == id)
        else {
            return;
        };
        batch.closing = true;
        if let Some(read) = &batch.read {
            read.cancel.cancel();
        }
        if let Some(write) = &batch.write {
            write.cancel.cancel();
        }
        if batch.pending() {
            batch.error = Some("已请求取消；等待批量备注的实际结果后关闭。".into());
        }
        cx.notify();
    }

    pub(in crate::workbench) fn finish_http_notes_batch(
        &mut self,
        id: Uuid,
        cx: &mut Context<Self>,
    ) {
        if self
            .http_notes
            .batch
            .as_ref()
            .is_some_and(|batch| batch.id == id && !batch.pending())
        {
            self.http_notes.batch = None;
            self.send_http_notes(false, cx);
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests;
