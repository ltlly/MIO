use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use mio_core::{ExtHistoryRow, ExtMessageSummary, HistoryEntryId, HistoryRow};

use super::*;

mod render;

const FRAGMENT_BYTES: usize = 64 * 1024;
const MAX_MESSAGE_BYTES: u64 = 16 * 1024 * 1024 + 128 * 1024;

#[derive(Default)]
pub(super) struct ImportedHistory {
    pub(super) row: Option<HistoryRow>,
    pending: Option<Pending>,
    request_ready: bool,
    request_edited: bool,
    response_ready: bool,
    response_edited: bool,
    request_error: Option<ReadError>,
    response_error: Option<ReadError>,
}

struct ReadError {
    message: String,
    oversized: bool,
}

struct Pending {
    project: ProjectKey,
    selected: HistoryEntryId,
    revision: u64,
    summary: ExtMessageSummary,
    response: bool,
    offset: u64,
    limit: usize,
    bytes: Vec<u8>,
}

impl Workbench {
    pub(in crate::workbench) fn imported_history_request(&self) -> Option<&ExtMessageSummary> {
        let row = self.imported_history.row.as_ref()?;
        if self
            .selected_history
            .is_some_and(|selected| selected != row.id())
        {
            return None;
        }
        let HistoryRow::ExtProxy(row) = row else {
            return None;
        };
        let ExtHistoryRow {
            request,
            edited_request,
            ..
        } = row.as_ref();
        if self.imported_history.request_edited {
            edited_request.as_ref()
        } else {
            request.as_ref()
        }
    }

    pub(in crate::workbench) fn imported_history_request_ready(&self) -> bool {
        self.imported_history.request_ready && self.imported_history_request().is_some()
    }

    pub(in crate::workbench) fn imported_history_response_ready(&self) -> bool {
        self.imported_history.response_ready && self.imported_history.response_error.is_none()
    }

    pub(in crate::workbench) fn reset_imported_history(
        &mut self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.imported_history = ImportedHistory::default();
    }

    pub(in crate::workbench) fn refresh_imported_history(
        &mut self,
        row: &HistoryRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let HistoryRow::ExtProxy(_) = row else {
            self.reset_imported_history(window, cx);
            return;
        };
        let same_messages = self.imported_history.row.as_ref().is_some_and(|previous| {
            let (HistoryRow::ExtProxy(previous), HistoryRow::ExtProxy(current)) = (previous, row)
            else {
                return false;
            };
            previous.item.id == current.item.id
                && previous.item.source_id == current.item.source_id
                && previous.request == current.request
                && previous.edited_request == current.edited_request
                && previous.response == current.response
                && previous.edited_response == current.edited_response
                && previous.origin == current.origin
        });
        if same_messages {
            self.imported_history.row = Some(row.clone());
        } else {
            let request_edited = self.imported_history.request_edited
                && self
                    .imported_history
                    .row
                    .as_ref()
                    .is_some_and(|previous| previous.id() == row.id())
                && matches!(
                    row,
                    HistoryRow::ExtProxy(row) if row.edited_request.is_some()
                );
            let response_edited = self.imported_history.response_edited
                && self
                    .imported_history
                    .row
                    .as_ref()
                    .is_some_and(|previous| previous.id() == row.id())
                && matches!(row, HistoryRow::ExtProxy(row) if row.edited_response.is_some());
            self.imported_history = ImportedHistory {
                row: Some(row.clone()),
                request_edited,
                response_edited,
                ..Default::default()
            };
            if self.history_request.has_focus(window, cx)
                || self.history_response.has_focus(window, cx)
            {
                self.focus.focus(window, cx);
            }
            self.history_request.load(&[], window, cx);
            self.history_response.load(&[], window, cx);
        }
        if self.imported_history.pending.is_some() || !self.backend_available() {
            return;
        }
        let HistoryRow::ExtProxy(archive) = row else {
            unreachable!()
        };
        let request = if self.imported_history.request_edited {
            &archive.edited_request
        } else {
            &archive.request
        };
        let response = if self.imported_history.response_edited {
            &archive.edited_response
        } else {
            &archive.response
        };
        for is_response in [false, true] {
            let (ready, error, summary, role) = if is_response {
                (
                    self.imported_history.response_ready,
                    &self.imported_history.response_error,
                    response,
                    if self.imported_history.response_edited {
                        "edited_response"
                    } else {
                        "response"
                    },
                )
            } else {
                (
                    self.imported_history.request_ready,
                    &self.imported_history.request_error,
                    request,
                    if self.imported_history.request_edited {
                        "edited_request"
                    } else {
                        "request"
                    },
                )
            };
            if ready || error.is_some() {
                continue;
            }
            let Some(summary) = summary else {
                if is_response {
                    self.imported_history.response_ready = true;
                } else {
                    self.imported_history.request_ready = true;
                }
                continue;
            };
            if summary.source_id != archive.item.source_id
                || summary.item_id != archive.item.id
                || summary.role != role
            {
                self.set_imported_read_error(
                    is_response,
                    "归档消息摘要与所选条目或请求/响应角色不一致。".into(),
                    false,
                );
                continue;
            }
            if summary.raw_bytes > MAX_MESSAGE_BYTES {
                self.set_imported_read_error(is_response, "归档消息超过当前编辑器的 16 MiB + 128 KiB 上限；未截断，可在 外部导入记录中完整分片查看。".into(), true);
                continue;
            }
            let Some(project) = self.project else { return };
            let Some(revision) = self.selection_revision.checked_add(1) else {
                self.set_imported_read_error(
                    is_response,
                    "消息读取 revision 已耗尽，请重新打开项目。".into(),
                    false,
                );
                continue;
            };
            self.selection_revision = revision;
            self.imported_history.pending = Some(Pending {
                project,
                selected: row.id(),
                revision,
                summary: summary.clone(),
                response: is_response,
                offset: 0,
                limit: summary.raw_bytes.min(FRAGMENT_BYTES as u64) as usize,
                bytes: Vec::new(),
            });
            self.send_imported_fragment(cx);
            break;
        }
        cx.notify();
    }

    fn set_imported_read_error(&mut self, response: bool, message: String, oversized: bool) {
        let error = Some(ReadError { message, oversized });
        if response {
            self.imported_history.response_error = error;
        } else {
            self.imported_history.request_error = error;
        }
    }

    fn send_imported_fragment(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = &self.imported_history.pending else {
            return;
        };
        let response = pending.response;
        let command = Command::ReadExtMessage {
            project: pending.project,
            selection_revision: pending.revision,
            message_id: pending.summary.id,
            offset: pending.offset,
            limit: pending.limit,
        };
        if !self.enqueue(command, cx) {
            self.imported_history.pending = None;
            self.set_imported_read_error(
                response,
                self.error
                    .clone()
                    .unwrap_or_else(|| "后台未接受归档读取。".into()),
                false,
            );
        }
        cx.notify();
    }

    pub(in crate::workbench) fn imported_history_disconnected(&mut self, cx: &mut Context<Self>) {
        if let Some(pending) = self.imported_history.pending.take() {
            self.set_imported_read_error(
                pending.response,
                "后台已断开，归档消息读取未确认；已读取内容保留。".into(),
                false,
            );
            cx.notify();
        }
    }

    pub(in crate::workbench) fn on_imported_history(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Event::ExtMessage {
            project,
            selection_revision,
            message_id,
            offset,
            result,
        } = event
        else {
            return false;
        };
        let Some(pending) = &self.imported_history.pending else {
            return false;
        };
        if pending.project != project
            || pending.revision != selection_revision
            || pending.summary.id != message_id
            || pending.offset != offset
        {
            return false;
        }
        if self.project != Some(project) || self.selected_history != Some(pending.selected) {
            self.imported_history.pending = None;
            return true;
        }
        let is_response = pending.response;
        let fragment = match result {
            Ok(fragment) => fragment,
            Err(error) => {
                self.imported_history.pending = None;
                self.set_imported_read_error(is_response, error, false);
                if let Some(row) = self.imported_history.row.clone() {
                    self.refresh_imported_history(&row, window, cx);
                }
                cx.notify();
                return true;
            }
        };
        if fragment.summary != pending.summary
            || fragment.offset != offset
            || fragment.bytes.len() != pending.limit
            || offset
                .checked_add(fragment.bytes.len() as u64)
                .is_none_or(|end| end > pending.summary.raw_bytes)
        {
            self.imported_history.pending = None;
            self.set_imported_read_error(
                is_response,
                "归档消息身份、摘要或分片范围不匹配，未替换已读取内容。".into(),
                false,
            );
            if let Some(row) = self.imported_history.row.clone() {
                self.refresh_imported_history(&row, window, cx);
            }
            cx.notify();
            return true;
        }
        let pending = self.imported_history.pending.as_mut().unwrap();
        if let Err(error) = pending.bytes.try_reserve(fragment.bytes.len()) {
            self.imported_history.pending = None;
            self.set_imported_read_error(
                is_response,
                format!("无法分配归档消息缓冲：{error}"),
                false,
            );
            if let Some(row) = self.imported_history.row.clone() {
                self.refresh_imported_history(&row, window, cx);
            }
            cx.notify();
            return true;
        }
        pending.bytes.extend_from_slice(&fragment.bytes);
        if pending.bytes.len() as u64 == pending.summary.raw_bytes {
            let pending = self.imported_history.pending.take().unwrap();
            if pending.response {
                self.history_response.load(&pending.bytes, window, cx);
                self.imported_history.response_ready = true;
            } else {
                self.history_request.load(&pending.bytes, window, cx);
                self.imported_history.request_ready = true;
            }
            if let Some(row) = self.imported_history.row.clone() {
                self.refresh_imported_history(&row, window, cx);
            }
        } else {
            pending.offset = pending.bytes.len() as u64;
            pending.limit =
                (pending.summary.raw_bytes - pending.offset).min(FRAGMENT_BYTES as u64) as usize;
            self.send_imported_fragment(cx);
        }
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests;
