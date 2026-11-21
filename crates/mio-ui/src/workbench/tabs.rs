use gpui_kit::component::{
    menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
    resizable::{HandleEdge, resize_handle},
};
use gpui_kit::{Axis, DragMoveEvent, Empty, MouseButton, WeakEntity};
use mio_core::{TabGroup, Uuid};

use super::*;

pub(in crate::workbench) mod actions;
pub(in crate::workbench) mod menus;
mod render_pane;
mod render_strip;
pub(in crate::workbench) use render_pane::RepeaterCombinedPane;

use menus::{repeater_group_menu, repeater_tab_menu, ws_repeater_tab_menu};

#[derive(Clone, Copy)]
pub(super) enum RenameTarget {
    Tab(TabId),
    WebSocketTab(TabId),
}

pub(super) struct Rename {
    pub(super) target: RenameTarget,
    pub(super) input: Entity<InputState>,
}

#[derive(Clone)]
struct TabDrag {
    pub(super) id: TabId,
    pub(super) title: String,
}

impl Render for TabDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(12.))
            .py(px(5.))
            .bg(theme::background())
            .border_1()
            .border_color(theme::accent())
            .child(self.title.clone())
    }
}

#[derive(Clone)]
struct ResizeTabBar;

impl Render for ResizeTabBar {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

fn ordered_tabs(tabs: &[WorkTab]) -> Vec<&WorkTab> {
    let mut result = Vec::with_capacity(tabs.len());
    let mut groups = HashSet::new();
    for tab in tabs {
        match tab.group {
            Some(group) if groups.insert(group) => {
                // Collapsed members retain their place for navigation and directional close.
                result.extend(tabs.iter().filter(|tab| tab.group == Some(group)));
            }
            Some(_) => {}
            None => result.push(tab),
        }
    }
    result
}

const MAX_RESIDENT_TAB_EDITORS: usize = 12;

impl Workbench {
    pub(super) fn prompt_close_unpersisted_tab(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self.project;
        let answer = window.prompt(
            PromptLevel::Warning,
            "此标签草稿尚未落盘保存",
            Some("当前项目存在存储错误，关闭后未持久化的更改可能丢失。"),
            &["仍然关闭", "保留标签"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = answer.await {
                let _ = this.update_in(cx, |view, window, cx| {
                    if view.project == project {
                        view.finish_close_repeater_tab(id, window, cx);
                    }
                });
            }
        })
        .detach();
    }

    fn touch_tab_editor_lru(&mut self, id: TabId) {
        self.tab_editor_lru.retain(|&existing| existing != id);
        self.tab_editor_lru.push(id);
    }

    fn evict_idle_tab_editors(&mut self) {
        while self.tab_editors.len() > MAX_RESIDENT_TAB_EDITORS {
            let Some(victim) = self.tab_editor_lru.iter().copied().find(|&cand| {
                self.workspace.active != Some(cand)
                    && !self.sending.contains_key(&cand)
                    && !self.pending_sends.contains_key(&cand)
                    && !self.repeater_reads.values().any(|&tid| tid == cand)
                    && !self.extensions.active_messages.contains_key(&cand)
                    && self.tab_editors.get(&cand).is_some_and(|e| !e.dirty)
            }) else {
                break;
            };
            self.tab_editors.remove(&victim);
            self.tab_editor_lru.retain(|&existing| existing != victim);
        }
    }

    pub(in crate::workbench) fn ensure_tab_editor(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab) = self.workspace.tabs.iter().find(|tab| tab.id == id).cloned() else {
            return false;
        };
        self.touch_tab_editor_lru(id);
        if let Some(editors) = self.tab_editors.get_mut(&id) {
            if !editors.dirty && editors.loaded_revision < tab.draft.revision {
                editors.request.load(&tab.draft.raw, window, cx);
                editors.target.update(cx, |state, cx| {
                    state.set_value(tab.draft.target.clone(), window, cx)
                });
                editors.loaded_revision = tab.draft.revision;
            }
            return true;
        }
        let mut request = MessageEditor::new(window, cx);
        request.load(&tab.draft.raw, window, cx);
        let response = MessageEditor::new(window, cx);
        let target = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(tab.draft.target.clone())
                .placeholder("https://localhost:8443")
        });
        let mut subscriptions = Vec::new();
        for input in [&request.raw, &request.hex] {
            // IME preedit emits notify, not Change; only mark transitions refresh the footer.
            let mut composing = false;
            subscriptions.push(cx.observe_in(input, window, move |_, input, window, cx| {
                let marked = input.update(cx, |state, cx| {
                    state.marked_text_range(window, cx).is_some()
                });
                if marked != composing {
                    composing = marked;
                    cx.notify();
                }
            }));
            subscriptions.push(cx.subscribe(input, move |this, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    let mut needs_notify = false;
                    if let Some(editors) = this.tab_editors.get_mut(&id) {
                        if !editors.dirty {
                            editors.dirty = true;
                            needs_notify = true;
                        }
                        if editors.request.pending_replace_all.is_some() {
                            needs_notify = true;
                        }
                    }
                    let live_sidebar = this.tool == Tool::Repeater
                        && this.workspace.active == Some(id)
                        && (this.inspector.open || this.repeater_explanations_open);
                    if needs_notify || live_sidebar {
                        cx.notify();
                    }
                }
            }));
        }
        let mut composing = false;
        subscriptions.push(cx.observe_in(&target, window, move |_, input, window, cx| {
            let marked = input.update(cx, |state, cx| {
                state.marked_text_range(window, cx).is_some()
            });
            if marked != composing {
                composing = marked;
                cx.notify();
            }
        }));
        subscriptions.push(cx.subscribe(&target, move |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                let mut needs_notify = false;
                if let Some(editors) = this.tab_editors.get_mut(&id)
                    && !editors.dirty
                {
                    editors.dirty = true;
                    needs_notify = true;
                }
                let live_sidebar = this.tool == Tool::Repeater
                    && this.workspace.active == Some(id)
                    && this.repeater_explanations_open;
                if needs_notify || live_sidebar {
                    cx.notify();
                }
            }
        }));
        self.tab_editors.insert(
            id,
            TabEditors {
                request,
                response,
                target,
                combined_pane: std::cell::Cell::new(RepeaterCombinedPane::Request),
                dirty: false,
                loaded_revision: tab.draft.revision,
                response_revision: None,
                loaded_flow: None,
                loaded_original: Box::default(),
                result: None,
                _subscriptions: subscriptions,
            },
        );
        if let (Some(project), Some(flow_id)) = (self.project, tab.last_flow) {
            self.selection_revision += 1;
            let revision = self.selection_revision;
            if self.enqueue(
                Command::ReadFlow {
                    project,
                    selection_revision: revision,
                    flow_id,
                },
                cx,
            ) {
                self.repeater_reads.insert(revision, id);
            }
        }
        self.evict_idle_tab_editors();
        true
    }

    pub(in crate::workbench) fn sync_tab_editors(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids: HashSet<_> = self.workspace.tabs.iter().map(|tab| tab.id).collect();
        self.tab_editors.retain(|id, _| ids.contains(id));
        self.tab_editor_lru.retain(|id| ids.contains(id));
        for tab in &self.workspace.tabs {
            if let Some(editors) = self.tab_editors.get_mut(&tab.id)
                && !editors.dirty
                && editors.loaded_revision < tab.draft.revision
            {
                editors.request.load(&tab.draft.raw, window, cx);
                editors.target.update(cx, |state, cx| {
                    state.set_value(tab.draft.target.clone(), window, cx)
                });
                editors.loaded_revision = tab.draft.revision;
            }
        }
        if let Some(active_id) = self
            .workspace
            .active
            .filter(|id| ids.contains(id))
            .or_else(|| self.workspace.tabs.first().map(|tab| tab.id))
        {
            self.ensure_tab_editor(active_id, window, cx);
        }
        self.sync_intruder_editors(window, cx);
        self.sync_websocket_repeater_editors(window, cx);
        self.refresh_archive_response(window, cx);
        self.refresh_repeater_history(window, cx);
    }

    pub(in crate::workbench) fn capture_drafts(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        if self.transition == Some(super::TransitionPhase::Switching) {
            return Err("项目正在切换，当前工作区暂不可编辑。".into());
        }
        self.validate_websocket_notes_composition(window, cx)?;
        self.validate_audit_composition(window, cx)
            .map_err(str::to_owned)?;
        self.collaborator_pane
            .validate_composition(window, cx)
            .map_err(str::to_owned)?;
        self.validate_http_notes_composition(window, cx)?;
        self.validate_extra_tools_composition(window, cx)?;
        self.validate_sequencer_composition(window, cx)?;
        self.validate_organiser_composition(window, cx)
            .map_err(str::to_owned)?;
        self.validate_inspector_composition(window, cx)
            .map_err(str::to_owned)?;
        self.validate_repeater_target_dialog_composition(cx)
            .map_err(str::to_owned)?;
        let rename = self.snapshot_intruder_rename(window, cx)?;
        let intruder = self.snapshot_intruder_drafts(window, cx)?;
        let ws_drafts = self.snapshot_websocket_repeater_drafts(window, cx)?;
        let mut snapshots = Vec::new();
        for tab in &self.workspace.tabs {
            let Some(editors) = self.tab_editors.get(&tab.id) else {
                continue;
            };
            if !editors.dirty && self.workspace.active != Some(tab.id) {
                continue;
            }
            // Read at the action boundary, including IME changes whose Change event has not arrived.
            let raw = editors.request.bytes(window, cx)?;
            let target = editing::input_snapshot(&editors.target, window, cx)
                .map_err(str::to_owned)?
                .to_string();
            let version =
                actions::sync_captured_draft_version(&tab.draft.raw, &raw, tab.draft.version);
            if (tab.draft.raw != raw || tab.draft.target != target || tab.draft.version != version)
                && tab.draft.revision == u64::MAX
            {
                return Err("草稿 revision 已耗尽；未采集任何部分输入。".into());
            }
            snapshots.push((tab.id, raw, target, version));
        }
        // An unconfirmed composition in any editor must not partially harvest other drafts.
        let mut changed = false;
        for (id, raw, target, version) in snapshots {
            let tab = self
                .workspace
                .tabs
                .iter_mut()
                .find(|tab| tab.id == id)
                .expect("snapshotted tab exists");
            if tab.draft.raw != raw || tab.draft.target != target || tab.draft.version != version {
                tab.draft.raw = raw;
                tab.draft.target = target;
                tab.draft.version = version;
                tab.draft.revision += 1;
                changed = true;
            }
            let editors = self
                .tab_editors
                .get_mut(&id)
                .expect("snapshotted editor exists");
            editors.loaded_revision = tab.draft.revision;
            editors.dirty = false;
        }
        let renamed = self.apply_intruder_rename_snapshot(rename);
        let ws_changed = self.apply_websocket_repeater_drafts(ws_drafts);
        Ok(self.apply_intruder_drafts(intruder) || ws_changed || renamed || changed)
    }

    pub(in crate::workbench) fn save_workspace(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(project) = self.project else {
            return false;
        };
        let group_ids: HashSet<_> = self.workspace.groups.iter().map(|group| group.id).collect();
        for ws_tab in &mut self.workspace.websocket_tabs {
            if ws_tab
                .group
                .is_some_and(|group| !group_ids.contains(&group))
            {
                ws_tab.group = None;
            }
        }
        if self.workspace.active.is_some()
            || self.workspace.active_websocket_tab.is_some_and(|id| {
                !self
                    .workspace
                    .websocket_tabs
                    .iter()
                    .any(|tab| !tab.closed && tab.id == id)
            })
        {
            self.workspace.active_websocket_tab = None;
        }
        self.workspace_dirty = true;
        self.workspace.tab_sidebar_width = self.tab_width as u16;
        if self.pending_workspace.is_some()
            || self.choosing_save_as
            || self
                .transition
                .as_ref()
                .is_some_and(|phase| *phase != super::TransitionPhase::Ready)
            || self
                .pending_project_save
                .as_ref()
                .is_some_and(Option::is_some)
        {
            return true;
        }
        let Some(revision) = self.workspace.revision.checked_add(1) else {
            self.fail("工作区 revision 已耗尽，无法继续保存。", cx);
            return false;
        };
        let mut workspace = (*self.workspace).clone();
        workspace.revision = revision;
        if self.enqueue(Command::SaveWorkspace { project, workspace }, cx) {
            self.pending_workspace = Some(revision);
            self.workspace_dirty = false;
            true
        } else {
            false
        }
    }

    pub(in crate::workbench) fn flush_workspace(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.intruder.resource_pools.management.delete_pending() {
            self.fail("资源池删除尚未确认；当前编辑保留，确认后可保存。", cx);
            return false;
        }
        if !self.backend_available() {
            return false;
        }
        match self.capture_drafts(window, cx) {
            Ok(true) => self.save_workspace(cx),
            Ok(false) => {
                if self.workspace_dirty {
                    self.save_workspace(cx)
                } else {
                    true
                }
            }
            Err(error) => {
                self.fail(error, cx);
                false
            }
        }
    }
}

#[cfg(test)]
mod layout_tests;
#[cfg(test)]
mod order_tests;
