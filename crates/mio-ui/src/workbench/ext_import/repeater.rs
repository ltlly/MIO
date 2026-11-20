use gpui_kit::assets::IconName;
use mio_core::{ExtMessageFragment, HttpVersion, WorkTab, validate_target_origin};

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Selection {
    revision: u64,
    source: Uuid,
    item: Uuid,
    message: Uuid,
    bytes: u64,
    version: HttpVersion,
    history: bool,
}

struct Pending {
    project: ProjectKey,
    operation: Uuid,
    selection: Selection,
    workspace_revision: Option<u64>,
    intruder: bool,
}

#[derive(Default)]
pub(super) struct OpenState {
    pending: Option<Pending>,
    error: Option<String>,
}

impl Workbench {
    fn ext_repeater_selection(&self) -> Option<Selection> {
        if !self.ext_import.visible || !self.ext_import.browser_visible {
            if self.tool != Tool::Proxy || self.proxy_page != ProxyPage::History {
                return None;
            }
            let message = self.imported_history_request()?;
            return Some(Selection {
                revision: self.history_detail_revision,
                source: message.source_id,
                item: message.item_id,
                message: message.id,
                bytes: message.raw_bytes,
                version: message.declared_version?,
                history: true,
            });
        }
        let (revision, fragment) = self.ext_import.messages.as_ref()?.selected_fragment()?;
        let message = &fragment.summary;
        if !(matches!(message.role.as_str(), "request" | "current_request")
            || (message.role == "edited_request"
                && message.field_id == 17
                && matches!(message.collection.as_str(), "root.f1.f0" | "root.f1.f1")))
            || message.raw_bytes == 0
        {
            return None;
        }
        Some(Selection {
            revision,
            source: message.source_id,
            item: message.item_id,
            message: message.id,
            bytes: message.raw_bytes,
            version: message.declared_version?,
            history: false,
        })
    }

    pub(in crate::workbench) fn cancel_ext_repeater(&mut self) {
        self.ext_import.repeater = OpenState::default();
    }

    pub(in crate::workbench) fn ext_repeater_save_failed(
        &mut self,
        message: &str,
        cx: &mut Context<Self>,
    ) {
        if self.ext_import.repeater.pending.take().is_some() {
            self.ext_import.repeater.error = Some(message.into());
            cx.notify();
        }
    }

    fn fail_ext_repeater(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        let message = message.into();
        self.ext_import.repeater.pending = None;
        self.ext_import.repeater.error = Some(message.clone());
        self.fail(message, cx);
    }

    fn open_ext_repeater(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_ext_history_transfer(false, window, cx);
    }

    pub(in crate::workbench) fn open_ext_history_transfer(
        &mut self,
        intruder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.ext_import.repeater.pending.is_some() {
            return;
        }
        let (Some(project), Some(selection)) = (self.project, self.ext_repeater_selection())
        else {
            if matches!(self.selected_history, Some(HistoryEntryId::External { .. })) {
                self.fail(
                    "归档请求缺失或 HTTP 声明版本未识别；原文保留，未创建工作标签。",
                    cx,
                );
            }
            return;
        };
        if selection.history && !self.imported_history_request_ready() {
            self.fail(
                "归档请求尚未完整读入；请等待读取或在导入记录中查看诊断。",
                cx,
            );
            return;
        }
        self.ext_import.repeater = OpenState {
            pending: Some(Pending {
                project,
                operation: Uuid::new_v4(),
                selection,
                workspace_revision: None,
                intruder,
            }),
            error: None,
        };
        self.flush_ext_repeater(window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn flush_ext_repeater(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = &self.ext_import.repeater.pending else {
            return;
        };
        if self.project != Some(pending.project)
            || self.ext_repeater_selection() != Some(pending.selection)
            || self.transition.is_some()
            || self.choosing_save_as
            || self.pending_project_save.is_some()
        {
            self.fail_ext_repeater("选择或项目状态已变化，未载入归档请求；请重新选择。", cx);
            return;
        }
        if pending.workspace_revision.is_some() {
            return;
        }
        if !self.flush_workspace(window, cx) {
            let error = self
                .error
                .clone()
                .unwrap_or_else(|| "当前草稿尚未保存。".into());
            self.fail_ext_repeater(format!("未载入归档请求：{error}"), cx);
            return;
        }
        if self.pending_workspace.is_some() || self.workspace_dirty {
            return;
        }
        let pending = self
            .ext_import
            .repeater
            .pending
            .as_ref()
            .expect("pending selection");
        let revision = self.workspace.revision;
        let command = Command::OpenExtRepeater {
            project: pending.project,
            operation_id: pending.operation,
            source_id: pending.selection.source,
            item_id: pending.selection.item,
            message_id: pending.selection.message,
            workspace_revision: revision,
        };
        if self.enqueue(command, cx) {
            self.ext_import
                .repeater
                .pending
                .as_mut()
                .expect("pending selection")
                .workspace_revision = Some(revision);
        } else {
            let error = self
                .error
                .clone()
                .unwrap_or_else(|| "读取请求未送达后台。".into());
            self.fail_ext_repeater(error, cx);
        }
    }

    pub(in crate::workbench) fn on_ext_repeater_event(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Event::ExtRepeaterOpened {
            project,
            operation_id,
            source_id,
            item_id,
            message_id,
            workspace_revision,
            result,
        } = event
        else {
            return;
        };
        let Some(pending) = &self.ext_import.repeater.pending else {
            return;
        };
        if self.project != Some(project)
            || pending.project != project
            || pending.operation != operation_id
            || pending.selection.source != source_id
            || pending.selection.item != item_id
            || pending.selection.message != message_id
            || pending.workspace_revision != Some(workspace_revision)
        {
            return;
        }
        let selection = pending.selection;
        let intruder = pending.intruder;
        self.ext_import.repeater.pending = None;
        if self.ext_repeater_selection() != Some(selection)
            || self.transition.is_some()
            || self.choosing_save_as
            || self.pending_project_save.is_some()
        {
            self.fail_ext_repeater("选择或项目状态已变化，未应用旧的归档读取结果。", cx);
            return;
        }
        let tab = match result {
            Ok(tab) => tab,
            Err(error) => {
                self.fail_ext_repeater(error, cx);
                return;
            }
        };
        let fragment = self
            .ext_import
            .messages
            .as_ref()
            .and_then(|messages| messages.selected_fragment())
            .map(|(_, fragment)| fragment);
        if tab.draft.revision != 1
            || (!tab.draft.target.is_empty() && validate_target_origin(&tab.draft.target).is_err())
            || tab.draft.version != selection.version
            || tab.draft.raw.len() as u64 != selection.bytes
            || tab.group.is_some()
            || tab.last_flow.is_some()
            || tab.archive_response.as_ref().is_some_and(|reference| {
                reference.source_id != selection.source
                    || reference.item_id != selection.item
                    || reference.message_id == selection.message
                    || reference.message_id.is_nil()
                    || reference.draft_revision != 1
            })
            || if selection.history {
                !self
                    .history_request
                    .bytes(window, cx)
                    .is_ok_and(|raw| raw == tab.draft.raw)
                    || tab.archive_response.is_some()
            } else {
                !fragment.is_some_and(|fragment| proposal_contains_fragment(&tab, fragment))
            }
        {
            self.fail_ext_repeater("归档草稿与所选消息不一致，未创建工作标签。", cx);
            return;
        }
        match self.capture_drafts(window, cx) {
            Ok(changed) => self.workspace_dirty |= changed,
            Err(error) => {
                self.fail_ext_repeater(error, cx);
                return;
            }
        }
        if intruder {
            self.open_intruder_draft(tab.draft, window, cx);
            return;
        }
        let mut candidate = self.workspace.clone();
        let has_target = !tab.draft.target.is_empty();
        candidate.active = Some(tab.id);
        candidate.tabs.push(tab);
        if let Err(error) = candidate.validate_structure() {
            self.fail_ext_repeater(format!("未载入归档请求：{error}"), cx);
            return;
        }
        self.workspace = candidate;
        self.sync_tab_editors(window, cx);
        self.ext_import.visible = false;
        self.ext_import.browser_visible = false;
        self.tool = Tool::Repeater;
        self.refresh_archive_response(window, cx);
        self.focus.focus(window, cx);
        self.status = if has_target {
            "已载入归档请求及连接目标，未发送；请核对后再发送。"
        } else {
            "已载入归档请求，未发送；连接目标尚未映射，请先填写并核对。"
        }
        .into();
        if !self.save_workspace(cx) {
            let error = self
                .error
                .clone()
                .unwrap_or_else(|| "保存尚未确认。".into());
            self.fail(format!("归档草稿已保留在工作台，但未保存：{error}"), cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn render_ext_repeater(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.ext_import.repeater;
        let pending = state.pending.as_ref();
        let label = match pending {
            Some(Pending {
                workspace_revision: None,
                ..
            }) => "等待当前草稿保存…",
            Some(_) => "正在读取归档请求…",
            None => "载入 Repeater",
        };
        div()
            .flex()
            .flex_col()
            .child(
                toolbar()
                    .child(
                        button("ext-open-repeater", label)
                            .icon(IconName::Repeat)
                            .tooltip("将当前选中的只读归档请求复制到 Repeater 新标签页（不自动发送请求）")
                            .disabled(
                                pending.is_some()
                                    || self.ext_repeater_selection().is_none()
                                    || self.transition.is_some()
                                    || self.choosing_save_as
                                    || self.pending_project_save.is_some(),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_ext_repeater(window, cx)
                            })),
                    )
                    .child(
                        div()
                            .text_color(theme::muted_foreground())
                            .child("仅载入已识别的请求及已验证的连接目标；未映射的目标留空，不自动发送。"),
                    ),
            )
            .when_some(state.error.as_ref(), |this, error| {
                this.child(
                    div()
                        .px(px(8.))
                        .text_color(theme::danger_fg())
                        .child(error.clone()),
                )
            })
            .into_any_element()
    }
}

fn proposal_contains_fragment(tab: &WorkTab, fragment: &ExtMessageFragment) -> bool {
    let Ok(start) = usize::try_from(fragment.offset) else {
        return false;
    };
    let Some(end) = start.checked_add(fragment.bytes.len()) else {
        return false;
    };
    tab.draft.raw.get(start..end) == Some(fragment.bytes.as_slice())
}

#[cfg(test)]
mod tests;
