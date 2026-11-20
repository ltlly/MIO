use mio_core::{ExtMessageSummary, ExtResponseRef};

use super::*;

const FRAGMENT_BYTES: usize = 64 * 1024;
const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024 + 128 * 1024;

#[derive(Default)]
pub(super) struct ArchiveResponses {
    revision: u64,
    active: Option<(TabId, ExtResponseRef)>,
    pending: Option<Pending>,
    loaded: bool,
    error: Option<(String, bool)>,
}

struct Pending {
    project: ProjectKey,
    revision: u64,
    tab_id: TabId,
    reference: ExtResponseRef,
    summary: Option<ExtMessageSummary>,
    offset: u64,
    limit: usize,
    bytes: Vec<u8>,
}

impl Workbench {
    fn archive_response_target(&self) -> Option<(TabId, ExtResponseRef)> {
        if self.tool != Tool::Repeater {
            return None;
        }
        self.workspace
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == self.workspace.active && tab.last_flow.is_none())
            .and_then(|tab| {
                tab.archive_response
                    .clone()
                    .map(|reference| (tab.id, reference))
            })
    }

    pub(in crate::workbench) fn refresh_archive_response(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.archive_response_target();
        if self.archive_responses.active != target {
            if let Some((id, _)) = self.archive_responses.active.take()
                && self
                    .workspace
                    .tabs
                    .iter()
                    .any(|tab| tab.id == id && tab.last_flow.is_none())
                && let Some(editors) = self.tab_editors.get_mut(&id)
            {
                if editors.response.state().focus_handle(cx).is_focused(window) {
                    self.focus.focus(window, cx);
                }
                editors.response.load(&[], window, cx);
                editors.response_revision = None;
                editors.result = None;
            }
            self.archive_responses.pending = None;
            self.archive_responses.loaded = false;
            self.archive_responses.error = None;
            self.archive_responses.active = target;
        }
        let Some((tab_id, reference)) = self.archive_responses.active.clone() else {
            return;
        };
        if self.archive_responses.pending.is_some()
            || self.archive_responses.loaded
            || self.archive_responses.error.is_some()
            || !self.backend_available()
            || self.transition.is_some()
        {
            return;
        }
        let Some(project) = self.project else { return };
        let Some(revision) = self.archive_responses.revision.checked_add(1) else {
            self.archive_response_failed(
                "归档响应读取 revision 已耗尽，请重新打开项目。".into(),
                false,
                cx,
            );
            return;
        };
        self.archive_responses.revision = revision;
        self.archive_responses.pending = Some(Pending {
            project,
            revision,
            tab_id,
            reference,
            summary: None,
            offset: 0,
            limit: 0,
            bytes: Vec::new(),
        });
        if let Some(editors) = self.tab_editors.get_mut(&tab_id) {
            editors.result = Some("正在读取归档响应…".into());
        }
        self.enqueue_archive_response(cx);
    }

    fn enqueue_archive_response(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = &self.archive_responses.pending else {
            return;
        };
        let command = Command::ReadExtResponse {
            project: pending.project,
            selection_revision: pending.revision,
            reference: pending.reference.clone(),
            offset: pending.offset,
            limit: pending.limit,
        };
        if !self.enqueue(command, cx) {
            let error = self
                .error
                .clone()
                .unwrap_or_else(|| "后台未接受归档响应读取。".into());
            self.archive_response_failed(error, true, cx);
        }
        cx.notify();
    }

    fn archive_response_failed(&mut self, message: String, retry: bool, cx: &mut Context<Self>) {
        self.archive_responses.pending = None;
        self.archive_responses.error = Some((message.clone(), retry));
        if let Some((id, _)) = &self.archive_responses.active
            && let Some(editors) = self.tab_editors.get_mut(id)
        {
            editors.result = Some(format!("归档响应：{message}"));
        }
        cx.notify();
    }

    pub(in crate::workbench) fn retry_archive_response(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .archive_responses
            .error
            .as_ref()
            .is_some_and(|(_, retry)| *retry)
        {
            self.archive_responses.error = None;
            self.archive_responses.loaded = false;
            self.refresh_archive_response(window, cx);
        }
    }

    pub(in crate::workbench) fn invalidate_archive_response(
        &mut self,
        tab_id: TabId,
        cx: &mut Context<Self>,
    ) {
        if self
            .archive_responses
            .active
            .as_ref()
            .is_some_and(|(id, _)| *id == tab_id)
        {
            self.archive_responses.pending = None;
            self.archive_responses.error = None;
            cx.notify();
        }
    }

    pub(in crate::workbench) fn archive_response_disconnected(&mut self, cx: &mut Context<Self>) {
        if self.archive_responses.pending.is_some() {
            self.archive_response_failed("后台服务已断开，归档响应读取未确认。".into(), true, cx);
        }
    }

    pub(in crate::workbench) fn on_archive_response(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Event::ExtResponse {
            project,
            selection_revision,
            reference,
            offset,
            result,
        } = event
        else {
            return;
        };
        let Some(pending) = &self.archive_responses.pending else {
            return;
        };
        if pending.project != project
            || self.project != Some(project)
            || pending.revision != selection_revision
            || pending.reference != reference
            || pending.offset != offset
        {
            return;
        }
        if self.archive_response_target() != Some((pending.tab_id, reference.clone())) {
            self.archive_responses.pending = None;
            return;
        }
        let fragment = match result {
            Ok(fragment) => fragment,
            Err(error) => {
                self.archive_response_failed(error, true, cx);
                return;
            }
        };
        let valid = fragment.summary.id == reference.message_id
            && fragment.summary.source_id == reference.source_id
            && fragment.summary.item_id == reference.item_id
            && fragment.summary.role == "response"
            && fragment.offset == offset
            && fragment.bytes.len() == pending.limit
            && pending
                .summary
                .as_ref()
                .is_none_or(|summary| *summary == fragment.summary)
            && offset
                .checked_add(fragment.bytes.len() as u64)
                .is_some_and(|end| end <= fragment.summary.raw_bytes);
        if !valid {
            self.archive_response_failed(
                "归档响应身份、摘要或分片范围不匹配；未替换已有响应。".into(),
                true,
                cx,
            );
            return;
        }
        if fragment.summary.raw_bytes > MAX_RESPONSE_BYTES {
            self.archive_response_failed("响应超过当前编辑器的 16 MiB + 128 KiB 上限；引用已保留，可在 外部导入记录中完整分片查看。".into(), false, cx);
            return;
        }
        let pending = self.archive_responses.pending.as_mut().unwrap();
        if pending.summary.is_none() {
            if let Err(error) = pending
                .bytes
                .try_reserve_exact(fragment.summary.raw_bytes as usize)
            {
                self.archive_response_failed(format!("无法分配归档响应缓冲：{error}"), true, cx);
                return;
            }
            pending.summary = Some(fragment.summary);
        }
        pending.bytes.extend_from_slice(&fragment.bytes);
        let total = pending.summary.as_ref().unwrap().raw_bytes;
        if pending.bytes.len() as u64 == total {
            let pending = self.archive_responses.pending.take().unwrap();
            if let Some(editors) = self.tab_editors.get_mut(&pending.tab_id) {
                editors.response.load(&pending.bytes, window, cx);
                editors.response_revision = Some(reference.draft_revision);
                editors.result = Some(format!("归档响应 · {total} 字节；未发送"));
            }
            self.archive_responses.loaded = true;
            self.archive_responses.error = None;
            cx.notify();
        } else {
            pending.offset = pending.bytes.len() as u64;
            pending.limit = (total - pending.offset).min(FRAGMENT_BYTES as u64) as usize;
            self.enqueue_archive_response(cx);
        }
    }

    pub(in crate::workbench) fn render_archive_response_error(
        &self,
        tab_id: TabId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self
            .archive_responses
            .active
            .as_ref()
            .is_some_and(|(id, _)| *id == tab_id)
        {
            return None;
        }
        let (error, retry) = self.archive_responses.error.as_ref()?;
        Some(
            div()
                .px(px(8.))
                .py(px(4.))
                .text_color(theme::danger_fg())
                .child(error.clone())
                .when(*retry, |this| {
                    this.child(
                        button("archive-response-retry", "重试读取归档响应")
                            .icon(gpui_kit::assets::IconName::RefreshCw)
                            .tooltip("重新从项目归档存储按分片加载关联的 HTTP 响应报文")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.retry_archive_response(window, cx)
                            })),
                    )
                })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests;
