use gpui_kit::Empty;
use mio_core::{IntruderTab, intruder::AttackType};

use super::*;

mod bar;
mod workspace_view;

#[cfg(test)]
#[path = "tabs_tests.rs"]
mod tests;

pub(in crate::workbench) struct Rename {
    pub(super) id: TabId,
    pub(super) is_group: bool,
    pub(super) input: Entity<InputState>,
    original_title: String,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone)]
struct ResizeIntruderTabBar;

impl Render for ResizeIntruderTabBar {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

#[derive(Clone)]
struct IntruderTabDrag {
    id: TabId,
    title: String,
}

impl Render for IntruderTabDrag {
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

impl Workbench {
    pub(in crate::workbench) fn begin_intruder_rename(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.flush_workspace(window, cx) {
            return;
        }
        let Some(tab) = self.workspace.intruder.tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        let title = tab.title.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title.clone()));
        let project = self.project;
        let subscription =
            cx.subscribe_in(&input, window, move |this, input, event, window, cx| {
                if this.project != project
                    || !this.intruder.rename.as_ref().is_some_and(|rename| {
                        rename.id == id && rename.input.entity_id() == input.entity_id()
                    })
                {
                    return;
                }
                match event {
                    InputEvent::PressEnter { .. } | InputEvent::Blur => {
                        this.apply_intruder_rename(window, cx)
                    }
                    InputEvent::Change => cx.notify(),
                    _ => {}
                }
            });
        let mut composing = false;
        let observer = cx.observe_in(&input, window, move |this, input, window, cx| {
            if this.project != project
                || !this.intruder.rename.as_ref().is_some_and(|rename| {
                    rename.id == id && rename.input.entity_id() == input.entity_id()
                })
            {
                return;
            }
            let marked = input.update(cx, |state, cx| {
                state.marked_text_range(window, cx).is_some()
            });
            if marked != composing {
                composing = marked;
                cx.notify();
            }
        });
        self.intruder.rename = Some(Rename {
            id,
            is_group: false,
            input: input.clone(),
            original_title: title,
            _subscriptions: vec![subscription, observer],
        });
        input.update(cx, |state, cx| {
            state.select_all(window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    pub(in crate::workbench) fn begin_intruder_group_rename(
        &mut self,
        group_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.flush_workspace(window, cx) {
            return;
        }
        let Some(group) = self
            .workspace
            .intruder
            .groups
            .iter()
            .find(|group| group.id == group_id)
        else {
            return;
        };
        let title = group.title.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title.clone()));
        let project = self.project;
        let subscription =
            cx.subscribe_in(&input, window, move |this, input, event, window, cx| {
                if this.project != project
                    || !this.intruder.rename.as_ref().is_some_and(|rename| {
                        rename.is_group
                            && rename.id == group_id
                            && rename.input.entity_id() == input.entity_id()
                    })
                {
                    return;
                }
                match event {
                    InputEvent::PressEnter { .. } | InputEvent::Blur => {
                        this.apply_intruder_rename(window, cx)
                    }
                    InputEvent::Change => cx.notify(),
                    _ => {}
                }
            });
        let mut composing = false;
        let observer = cx.observe_in(&input, window, move |this, input, window, cx| {
            if this.project != project
                || !this.intruder.rename.as_ref().is_some_and(|rename| {
                    rename.is_group
                        && rename.id == group_id
                        && rename.input.entity_id() == input.entity_id()
                })
            {
                return;
            }
            let marked = input.update(cx, |state, cx| {
                state.marked_text_range(window, cx).is_some()
            });
            if marked != composing {
                composing = marked;
                cx.notify();
            }
        });
        self.intruder.rename = Some(Rename {
            id: group_id,
            is_group: true,
            input: input.clone(),
            original_title: title,
            _subscriptions: vec![subscription, observer],
        });
        input.update(cx, |state, cx| {
            state.select_all(window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    pub(in crate::workbench) fn snapshot_intruder_rename(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Option<(TabId, String)>, String> {
        let Some(rename) = &self.intruder.rename else {
            return Ok(None);
        };
        let title = editing::input_snapshot(&rename.input, window, cx)
            .map_err(str::to_owned)?
            .trim()
            .to_owned();
        if title.is_empty() || title.len() > 512 {
            return Err("名称须为 1–512 字节的非空文本。".into());
        }
        Ok(Some((rename.id, title)))
    }

    pub(in crate::workbench) fn apply_intruder_rename_snapshot(
        &mut self,
        rename: Option<(TabId, String)>,
    ) -> bool {
        let Some((id, title)) = rename else {
            return false;
        };
        if let Some(tab) = self
            .workspace
            .intruder
            .tabs
            .iter_mut()
            .find(|tab| tab.id == id)
        {
            let changed = tab.title != title;
            tab.title = title;
            return changed;
        }
        if let Some(group) = self
            .workspace
            .intruder
            .groups
            .iter_mut()
            .find(|group| group.id == id)
        {
            let changed = group.title != title;
            group.title = title;
            return changed;
        }
        false
    }

    pub(in crate::workbench) fn apply_intruder_rename(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let Some(rename) = &self.intruder.rename else {
            return;
        };
        let focused = rename.input.focus_handle(cx).is_focused(window);
        if !self.flush_workspace(window, cx) {
            return;
        }
        self.intruder.rename = None;
        if focused && let Some(focus) = &self.intruder.tabs_focus {
            focus.focus(window, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn cancel_intruder_rename(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rename) = &self.intruder.rename else {
            return;
        };
        if let Err(error) = editing::input_snapshot(&rename.input, window, cx) {
            self.fail(error, cx);
            return;
        }
        let original = (rename.id, rename.original_title.clone());
        self.intruder.rename = None;
        if self.apply_intruder_rename_snapshot(Some(original)) {
            self.save_workspace(cx);
        }
        if let Some(focus) = &self.intruder.tabs_focus {
            focus.focus(window, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn close_intruder_tabs(
        &mut self,
        ids: &[TabId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // An active IME composition must block the entire batch before any tab is removed.
        if ids.is_empty() || self.transition.is_some() || !self.flush_workspace(window, cx) {
            return;
        }
        for id in ids {
            self.close_intruder_tab(*id, window, cx);
        }
    }

    pub(in crate::workbench) fn set_intruder_orientation(
        &mut self,
        desired: Orientation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.intruder.orientation == desired
            || self.transition.is_some()
            || !self.flush_workspace(window, cx)
        {
            return;
        }
        self.workspace.intruder.orientation = desired;
        self.intruder
            .tabs_scroll
            .set_offset(gpui_kit::Point::default());
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn new_intruder_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(pending) = self.pending_intruder_default_draft() {
            self.open_intruder_draft(pending, window, cx);
            return;
        }
        let mode = self.intruder_user_misc().intruder_new_tab_config_mode;
        if mode == 1
            && let Some(active_id) = self.workspace.intruder.active
            && self
                .workspace
                .intruder
                .tabs
                .iter()
                .any(|tab| tab.id == active_id)
        {
            if self.transition.is_some()
                || !self.backend_available()
                || !self.flush_workspace(window, cx)
            {
                return;
            }
            if let Some(source) = self
                .workspace
                .intruder
                .tabs
                .iter()
                .find(|tab| tab.id == active_id)
                .cloned()
            {
                if source.spec.resource_pool.is_some() {
                    let id = Uuid::new_v4();
                    let previous_active = self.workspace.intruder.active;
                    let number = self
                        .workspace
                        .intruder
                        .tabs
                        .iter()
                        .chain(&self.workspace.intruder.closed)
                        .filter_map(|tab| tab.title.parse::<u32>().ok())
                        .max()
                        .unwrap_or(0)
                        .saturating_add(1);
                    let mut spec = source.spec;
                    spec.revision = 1;
                    spec.draft.revision = 1;
                    if let Some(mio_core::IntruderPoolSelection::Create(pool)) =
                        &mut spec.resource_pool
                    {
                        pool.id = Uuid::new_v4();
                    }
                    let copied_grep = self
                        .workspace
                        .intruder
                        .grep_configs
                        .get(&active_id)
                        .cloned();
                    self.workspace.intruder.tabs.push(IntruderTab {
                        id,
                        title: number.to_string(),
                        group: None,
                        spec,
                        last_run: None,
                    });
                    if let Some(grep) = copied_grep {
                        self.workspace.intruder.grep_configs.insert(id, grep);
                    }
                    self.workspace.intruder.active = Some(id);
                    if let Err(error) = self.workspace.validate_structure() {
                        self.workspace.intruder.tabs.pop();
                        self.workspace.intruder.grep_configs.remove(&id);
                        self.workspace.intruder.active = previous_active;
                        self.fail(format!("未新建 Intruder 工作标签：{error}"), cx);
                        return;
                    }
                    self.cancel_plugin_interactions(cx);
                    self.tool = Tool::Intruder;
                    self.sync_intruder_editors(window, cx);
                    if let Some(editors) = self.intruder.editors.get(&id) {
                        editors
                            .request
                            .state()
                            .update(cx, |state, cx| state.focus(window, cx));
                    }
                    self.save_workspace(cx);
                    cx.notify();
                    return;
                }
                self.open_intruder_draft(source.spec.draft, window, cx);
                return;
            }
        }
        self.open_intruder_draft(
            Draft {
                revision: 1,
                target: String::new(),
                raw: Vec::new(),
                version: HttpVersion::Http11,
            },
            window,
            cx,
        );
    }

    pub(in crate::workbench) fn append_intruder_default_draft(
        &mut self,
        draft: Draft,
        pool_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let id = Uuid::new_v4();
        let previous_active = self.workspace.intruder.active;
        let first =
            self.workspace.intruder.tabs.is_empty() && self.workspace.intruder.closed.is_empty();
        let blank_mode = self.intruder_user_misc().intruder_new_tab_config_mode == 2
            && draft.raw.is_empty()
            && draft.target.is_empty();
        let number = self
            .workspace
            .intruder
            .tabs
            .iter()
            .chain(&self.workspace.intruder.closed)
            .filter_map(|tab| tab.title.parse::<u32>().ok())
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        self.workspace.intruder.tabs.push(IntruderTab {
            id,
            title: number.to_string(),
            group: None,
            spec: IntruderSpec {
                resource_pool: Some(mio_core::IntruderPoolSelection::Existing(pool_id)),
                baseline_request: !blank_mode,
                revision: 1,
                draft,
                attack_type: AttackType::Sniper,
                positions: Vec::new(),
                payload_sets: vec![Vec::new()],
                max_concurrent_requests: 1,
                request_timeout_ms: 30_000,
                update_host_header: !blank_mode,
                save_to_project: false,
            },
            last_run: None,
        });
        self.workspace.intruder.active = Some(id);
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.intruder.tabs.pop();
            self.workspace.intruder.active = previous_active;
            self.fail(format!("未新建 Intruder 工作标签：{error}"), cx);
            return false;
        }
        if first {
            self.workspace.intruder.orientation = Orientation::Vertical;
        }
        self.cancel_plugin_interactions(cx);
        self.tool = Tool::Intruder;
        self.sync_intruder_editors(window, cx);
        self.intruder.editors[&id]
            .request
            .state()
            .update(cx, |state, cx| state.focus(window, cx));
        self.save_workspace(cx);
        cx.notify();
        true
    }

    pub(in crate::workbench) fn select_intruder_tab(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.flush_workspace(window, cx) {
            return;
        }
        let Some(tab) = self.workspace.intruder.tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        if let Some(group_id) = tab.group
            && let Some(group) = self
                .workspace
                .intruder
                .groups
                .iter_mut()
                .find(|group| group.id == group_id)
        {
            group.collapsed = false;
        }
        self.cancel_plugin_interactions(cx);
        let switching = self.workspace.intruder.active != Some(id);
        let auto_expand = self.intruder_user_misc().intruder_side_panel_auto_expand;
        self.workspace.intruder.active = Some(id);
        if switching && let Some(editors) = self.intruder.editors.get_mut(&id) {
            if auto_expand {
                if !editors.payloads_open
                    && !editors.resource_pool_open
                    && !editors.settings_open
                    && !self.inspector.open
                {
                    editors.payloads_open = true;
                }
            } else {
                editors.payloads_open = false;
                editors.resource_pool_open = false;
                editors.settings_open = false;
            }
        }
        self.save_workspace(cx);
        if let Some(editors) = self.intruder.editors.get(&id) {
            editors
                .request
                .state()
                .update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();
    }

    pub(in crate::workbench) fn restore_intruder_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.flush_workspace(window, cx) {
            return;
        }
        if let Some(tab) = self.workspace.intruder.closed.pop() {
            let id = tab.id;
            if let Some(group_id) = tab.group
                && let Some(group) = self
                    .workspace
                    .intruder
                    .groups
                    .iter_mut()
                    .find(|group| group.id == group_id)
            {
                group.collapsed = false;
            }
            self.workspace.intruder.active = Some(id);
            self.workspace.intruder.tabs.push(tab);
            self.sync_intruder_editors(window, cx);
            self.save_workspace(cx);
            self.tool = Tool::Intruder;
            self.intruder.editors[&id]
                .request
                .state()
                .update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();
    }

    pub(in crate::workbench) fn step_intruder_tab(
        &mut self,
        direction: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tabs = &self.workspace.intruder.tabs;
        if tabs.is_empty() {
            return;
        }
        let index = tabs
            .iter()
            .position(|tab| Some(tab.id) == self.workspace.intruder.active)
            .unwrap_or(0);
        let next = (index as isize + direction).rem_euclid(tabs.len() as isize) as usize;
        self.select_intruder_tab(tabs[next].id, window, cx);
    }

    pub(in crate::workbench) fn move_intruder_tab_by(
        &mut self,
        direction: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.flush_workspace(window, cx) {
            return;
        }
        let tabs = &mut self.workspace.intruder.tabs;
        let Some(index) = tabs
            .iter()
            .position(|tab| Some(tab.id) == self.workspace.intruder.active)
        else {
            return;
        };
        let Some(next) = index
            .checked_add_signed(direction)
            .filter(|next| *next < tabs.len())
        else {
            return;
        };
        tabs[index].group = tabs[next].group;
        tabs.swap(index, next);
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn move_intruder_tab(
        &mut self,
        id: TabId,
        before: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let after = self.drop_after;
        self.drop_target = None;
        self.drop_after = false;
        if id == before || self.transition.is_some() || !self.flush_workspace(window, cx) {
            return;
        }
        let tabs = &mut self.workspace.intruder.tabs;
        let (Some(from), Some(to)) = (
            tabs.iter().position(|tab| tab.id == id),
            tabs.iter().position(|tab| tab.id == before),
        ) else {
            return;
        };
        let group = tabs[to].group;
        let mut tab = tabs.remove(from);
        tab.group = group;
        tabs.insert(
            (if from < to { to - 1 } else { to }) + usize::from(after),
            tab,
        );
        self.save_workspace(cx);
        cx.notify();
    }
}
