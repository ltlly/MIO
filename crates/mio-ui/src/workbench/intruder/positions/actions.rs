use std::ops::Range;

use super::*;

impl Workbench {
    pub(in crate::workbench) fn intruder_non_request_input_focused(
        &self,
        id: TabId,
        window: &Window,
        cx: &App,
    ) -> bool {
        self.intruder
            .rename
            .as_ref()
            .is_some_and(|rename| rename.input.focus_handle(cx).is_focused(window))
            || self.intruder.editors.get(&id).is_some_and(|editors| {
                editors.has_non_request_focus(window, cx)
                    || editors.search.focus_handle(cx).is_focused(window)
            })
    }

    fn save_intruder_position_edit(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.intruder_input_changed(id, window, cx);
        match self.capture_drafts(window, cx) {
            Ok(changed) => {
                self.workspace_dirty |= changed;
                self.sync_intruder_editors(window, cx);
                self.save_workspace(cx);
            }
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn add_intruder_position(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() {
            return;
        }
        let Some(editors) = self.intruder.editors.get_mut(&id) else {
            return;
        };
        if editors.has_composition(window, cx) {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        match editors.request.add(window, cx) {
            Ok(()) => self.save_intruder_position_edit(id, window, cx),
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn clear_intruder_positions(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || self.backend_disconnected {
            return;
        }
        let Some(editors) = self.intruder.editors.get_mut(&id) else {
            return;
        };
        if editors.has_composition(window, cx) {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        if editors.request.unplaced.is_some() {
            // Invalid persisted ranges have no text markers to restore with text Undo.
            if self.change_intruder_spec(id, |spec| spec.positions.clear(), window, cx) {
                self.status =
                    "已明确清除无效位置；Raw 字节未改变。原无效范围不属于文本 Undo 历史。".into();
            }
            return;
        }
        match editors.request.clear(window, cx) {
            Ok(()) => self.save_intruder_position_edit(id, window, cx),
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn auto_intruder_positions(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || self.backend_disconnected
            || self.intruder.positions_request.is_some()
        {
            return;
        }
        let Some(project) = self.project else { return };
        let Some(tab) = self.workspace.intruder.tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        let (spec_revision, draft_revision) = (tab.spec.revision, tab.spec.draft.revision);
        let Some(editors) = self.intruder.editors.get_mut(&id) else {
            return;
        };
        if editors.has_composition(window, cx) {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        let content = match editors.request.content(window, cx) {
            Ok(content) => content,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let view = editors.request.view;
        let editor_id = editors.request.state().entity_id();
        let selection = editors.request.state().read(cx).selected_range();
        let repair = editors.request.unplaced.is_some();
        let prepared = (|| {
            let (text, _) = without_markers(&content);
            let range: Option<Range<usize>> = if selection.is_empty() {
                None
            } else {
                let offsets = byte_offsets(
                    &text,
                    &[
                        unmarked_offset(&content, selection.start),
                        unmarked_offset(&content, selection.end),
                    ],
                    view,
                )?;
                Some(offsets[0]..offsets[1])
            };
            let retained = if let Some(range) = &range {
                if repair {
                    return Err(
                        "原 payload 范围无效；请取消选区后自动识别整个请求，或先清除 §。"
                            .to_owned(),
                    );
                }
                snapshot_content(&content, view)?
                    .1
                    .into_iter()
                    .map(|mark| mark.position)
                    .filter(|position| position.end <= range.start || position.start >= range.end)
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            Ok((decode_text(text, view)?, range, retained))
        })();
        let (bytes, range, retained) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let operation_id = Uuid::new_v4();
        let pending = PendingPositions {
            project,
            operation_id,
            tab_id: id,
            editor_id,
            view,
            content,
            selection,
            target: editors.target.read(cx).value(),
            document_revision: editors.request.state().read(cx).document_revision(),
            target_revision: editors.target.read(cx).document_revision(),
            spec_revision,
            draft_revision,
            retained,
            repair,
        };
        let placement = if self.intruder_user_misc().intruder_auto_payload_append {
            mio_core::intruder::AutomaticPlacement::Append
        } else {
            mio_core::intruder::AutomaticPlacement::Replace
        };
        if self.enqueue(
            Command::AutoIntruderPositions {
                project,
                operation_id,
                raw: bytes,
                selection: range,
                placement,
            },
            cx,
        ) {
            self.intruder.positions_request = Some(pending);
            self.status = "正在自动识别 payload 位置…".into();
        }
        cx.notify();
    }

    pub(in crate::workbench) fn on_intruder_positions_event(
        &mut self,
        event: Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Event::IntruderPositions {
            project,
            operation_id,
            positions: result,
        } = event
        else {
            return;
        };
        if self.project != Some(project)
            || !self
                .intruder
                .positions_request
                .as_ref()
                .is_some_and(|pending| {
                    pending.project == project && pending.operation_id == operation_id
                })
        {
            return;
        }
        let pending = self
            .intruder
            .positions_request
            .take()
            .expect("matched automatic placement request");
        let id = pending.tab_id;
        let Some(editors) = self.intruder.editors.get_mut(&id) else {
            cx.notify();
            return;
        };
        if editors.request.state().entity_id() != pending.editor_id {
            cx.notify();
            return;
        }
        if self.transition.is_some()
            || self.backend_disconnected
            || !self.workspace.intruder.tabs.iter().any(|tab| {
                tab.id == id
                    && tab.spec.revision == pending.spec_revision
                    && tab.spec.draft.revision == pending.draft_revision
            })
            || editors.has_composition(window, cx)
            || editors.request.state().read(cx).document_revision() != pending.document_revision
            || editors.target.read(cx).document_revision() != pending.target_revision
            || editors.target.read(cx).value() != pending.target
            || editors.request.view != pending.view
            || editors.request.state().read(cx).content() != pending.content
            || editors.request.state().read(cx).selected_range() != pending.selection
        {
            self.fail(
                "自动识别期间请求、选区或项目状态已改变；标记未应用，请明确重试。",
                cx,
            );
            return;
        }
        let mut positions = match result {
            Ok(positions) => positions,
            Err(error) => {
                self.fail(format!("未自动设置 payload 位置：{error}"), cx);
                return;
            }
        };
        positions.extend(pending.retained);
        positions.sort_by_key(|position| (position.start, position.end));
        let count = positions.len();
        match editors.request.set_positions(&positions, window, cx) {
            Ok(()) => {
                if self.tool == Tool::Intruder && self.workspace.intruder.active == Some(id) {
                    editors
                        .request
                        .state()
                        .update(cx, |state, cx| state.focus(window, cx));
                }
                self.save_intruder_position_edit(id, window, cx);
                self.status = if pending.repair {
                    format!(
                        "已自动设置 {count} 个位置；Raw 字节未改变。原无效范围不属于文本 Undo 历史。"
                    )
                } else {
                    format!("已自动设置 {count} 个 payload 位置；Raw 字节未改变。")
                };
                cx.notify();
            }
            Err(error) => self.fail(error, cx),
        }
    }

    pub(in crate::workbench) fn switch_intruder_view(
        &mut self,
        id: TabId,
        view: MessageView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() {
            return;
        }
        let Some(editors) = self.intruder.editors.get_mut(&id) else {
            return;
        };
        if editors.has_composition(window, cx) {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        match editors.request.switch(view, window, cx) {
            Ok(()) => {
                editors.hex_length = editors.request.byte_length(cx);
                self.search_intruder(id, None, window, cx);
            }
            Err(error) => self.fail(error, cx),
        }
        cx.notify();
    }
}
