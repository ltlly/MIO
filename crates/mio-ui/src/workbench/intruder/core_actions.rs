use super::*;

#[derive(Clone, Copy, PartialEq, Eq, gpui_kit::Action)]
#[action(namespace = mio, no_json)]
pub(in crate::workbench) struct AddIntruderPosition;

impl Workbench {
    pub(in crate::workbench) fn intruder_input_changed(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let attack_type = self
            .workspace
            .intruder
            .tabs
            .iter()
            .find(|tab| tab.id == id)
            .map(|tab| tab.spec.attack_type);
        if let Some(editors) = self.intruder.editors.get_mut(&id) {
            editors.dirty = true;
            let positions = editors.request.position_count(cx).unwrap_or(0);
            let sets = match attack_type {
                Some(
                    mio_core::intruder::AttackType::Sniper
                    | mio_core::intruder::AttackType::BatteringRam,
                ) => 1,
                _ => positions,
            };
            if positions == 0 || editors.payload_position.is_some_and(|index| index >= sets) {
                editors.payload_position = None;
                editors.payload_selection.clear();
                if editors.payload_input.focus_handle(cx).is_focused(window) {
                    self.focus.focus(window, cx);
                }
            }
        }
        if let Some(pending) = self.intruder.starts.get(&id)
            && pending.phase == StartPhase::Saving
            && self.snapshot_intruder_spec(id, window, cx).as_ref() != Ok(&pending.spec)
        {
            self.cancel_intruder_save_wait(id, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn cancel_intruder_save_wait(
        &mut self,
        id: TabId,
        cx: &mut Context<Self>,
    ) {
        if self
            .intruder
            .starts
            .get(&id)
            .is_some_and(|pending| pending.phase == StartPhase::Saving)
        {
            let pending = self
                .intruder
                .starts
                .remove(&id)
                .expect("checked pending start");
            pending.cancel.cancel();
            self.status =
                "等待保存时 Intruder 输入已变更或关闭；未开始，请再次点击「开始攻击」。".into();
            cx.notify();
        }
    }

    pub(in crate::workbench) fn intruder_save_failed(&mut self, cx: &mut Context<Self>) {
        self.cancel_intruder_default_pool(
            "项目正在切换、退出或保存未确认；待导入请求保留，未创建标签。",
            cx,
        );
        let waiting: Vec<_> = self
            .intruder
            .starts
            .iter()
            .filter_map(|(id, pending)| (pending.phase == StartPhase::Saving).then_some(*id))
            .collect();
        if !waiting.is_empty() {
            self.status = "Intruder 开始已取消：配置保存未确认，未自动发送或重试。".into();
        }
        for id in waiting {
            if let Some(pending) = self.intruder.starts.remove(&id) {
                pending.cancel.cancel();
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn intruder_backend_disconnected(&mut self, cx: &mut Context<Self>) {
        self.intruder.resource_pools.disconnect();
        self.intruder_pool_disconnected(cx);
        self.cancel_intruder_default_pool("后台已断开；待导入请求保留，默认资源池未确认。", cx);
        for pending in self.intruder.starts.values() {
            pending.cancel.cancel();
        }
        for run in self.intruder.runs.values() {
            run.cancel.cancel();
        }
        self.intruder.starts.clear();
        self.intruder_baseline_disconnected(cx);
        self.intruder.transfer_reads.clear();
        self.intruder.positions_request = None;
        self.intruder_results_backend_disconnected(cx);
        if !self.intruder.runs.is_empty() {
            self.status = "后台服务已断开；Intruder 停止结果未确认，未自动重发。".into();
        }
        cx.notify();
    }

    pub(in crate::workbench) fn intruder_project_changed(&mut self) {
        self.intruder.reset();
    }

    pub(in crate::workbench) fn change_intruder_spec(
        &mut self,
        id: TabId,
        update: impl FnOnce(&mut IntruderSpec),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.try_change_intruder_spec(
            id,
            |spec| {
                update(spec);
                Ok(())
            },
            window,
            cx,
        )
    }

    pub(in crate::workbench) fn try_change_intruder_spec(
        &mut self,
        id: TabId,
        update: impl FnOnce(&mut IntruderSpec) -> Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.transition.is_some() {
            return false;
        }
        match self.capture_drafts(window, cx) {
            Ok(changed) => self.workspace_dirty |= changed,
            Err(error) => {
                self.fail(error, cx);
                return false;
            }
        }
        let Some(tab) = self.workspace.intruder.tabs.iter().find(|tab| tab.id == id) else {
            return false;
        };
        let mut next = tab.spec.clone();
        if let Err(error) = update(&mut next) {
            self.fail(error, cx);
            return false;
        }
        let next = match revised_spec(&tab.spec, next) {
            Ok(next) => next,
            Err(error) => {
                self.fail(error, cx);
                return false;
            }
        };
        if next != tab.spec {
            self.cancel_intruder_save_wait(id, cx);
            let tab = self
                .workspace
                .intruder
                .tabs
                .iter_mut()
                .find(|tab| tab.id == id)
                .expect("checked open Intruder tab");
            let previous = std::mem::replace(&mut tab.spec, next);
            if let Err(error) = self.workspace.validate_structure() {
                self.workspace
                    .intruder
                    .tabs
                    .iter_mut()
                    .find(|tab| tab.id == id)
                    .expect("checked tab")
                    .spec = previous;
                self.fail(
                    format!("未应用 Intruder 配置：{error}；当前输入仍保留。"),
                    cx,
                );
                return false;
            }
            self.sync_intruder_editors(window, cx);
            self.save_workspace(cx);
            cx.notify();
            return true;
        }
        true
    }

    pub(in crate::workbench) fn close_intruder_tab(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_intruder_save_wait(id, cx);
        if !self.flush_workspace(window, cx) {
            return;
        }
        let Some(index) = self
            .workspace
            .intruder
            .tabs
            .iter()
            .position(|tab| tab.id == id)
        else {
            return;
        };
        self.focus.focus(window, cx);
        if let Some(pending) = self.intruder.starts.get(&id) {
            // Admission may have started; retain its identity until the matching result arrives.
            pending.cancel.cancel();
        }
        let tab = self.workspace.intruder.tabs.remove(index);
        self.workspace.intruder.closed.push(tab);
        if self.workspace.intruder.active == Some(id) {
            self.workspace.intruder.active = self
                .workspace
                .intruder
                .tabs
                .get(index.min(self.workspace.intruder.tabs.len().saturating_sub(1)))
                .map(|tab| tab.id);
        }
        self.intruder.editors.remove(&id);
        if self
            .intruder
            .rename
            .as_ref()
            .is_some_and(|rename| !rename.is_group && rename.id == id)
        {
            self.intruder.rename = None;
        }
        self.save_workspace(cx);
        self.status = "Intruder 工作标签已关闭；配置和输入保留在当前项目。".into();
        cx.notify();
    }

    pub(in crate::workbench::intruder) fn intruder_user_misc(&self) -> &mio_core::UserMiscSettings {
        if self.user_preferences.busy() {
            &self.user_preferences.misc
        } else if let Some(confirmed) = &self.user_preferences.confirmed {
            if self.user_preferences.misc != mio_core::UserMiscSettings::default()
                && confirmed.misc == mio_core::UserMiscSettings::default()
            {
                &self.user_preferences.misc
            } else {
                &confirmed.misc
            }
        } else {
            &self.user_preferences.misc
        }
    }

    pub(in crate::workbench::intruder) fn intruder_onboarding_dismissed(&self) -> bool {
        self.intruder.onboarding_dismissed
            || self.intruder_user_misc().intruder_onboarding_dismissed
    }

    pub(in crate::workbench::intruder) fn set_intruder_onboarding_dismissed(
        &mut self,
        dismissed: bool,
        cx: &mut Context<Self>,
    ) {
        self.intruder.onboarding_dismissed = dismissed;
        let mut misc = self.intruder_user_misc().clone();
        let changed = misc.intruder_onboarding_dismissed != dismissed;
        misc.intruder_onboarding_dismissed = dismissed;
        self.user_preferences.misc = misc.clone();
        if changed && self.user_preferences.can_save() {
            let display = self
                .user_preferences
                .confirmed
                .as_ref()
                .map(|p| p.display.clone())
                .unwrap_or_else(|| self.user_preferences.display.clone());
            let message_editor = self
                .user_preferences
                .confirmed
                .as_ref()
                .map(|p| p.message_editor.clone())
                .unwrap_or_else(|| self.user_preferences.message_editor.clone());
            let _ = self.set_user_workbench_preferences(display, message_editor, misc, cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn intruder_positions_context_menu(
        &self,
        input: gpui_kit::Entity<gpui_kit::component::input::TextareaState>,
        readonly: bool,
    ) -> impl Fn(
        gpui_kit::component::native_menu::NativeMenu,
        &mut Window,
        &mut App,
    ) -> gpui_kit::component::native_menu::NativeMenu
    + 'static {
        let unavailable =
            self.project.is_none() || self.transition.is_some() || self.backend_disconnected;
        let has_markers = self
            .workspace
            .intruder
            .active
            .and_then(|id| self.intruder.editors.get(&id))
            .map(|ed| ed.request.has_unplaced());
        let base_menu = self.positions_context_menu(input.clone(), readonly);
        move |menu, window, cx| {
            let markers_present = has_markers.unwrap_or(false)
                || input
                    .read(cx)
                    .tokens()
                    .iter()
                    .any(|span| span.token().id().starts_with("intruder-position:"));
            let menu = menu
                .menu_with_disabled(
                    "添加 § 标记",
                    unavailable || readonly,
                    Box::new(AddIntruderPosition),
                )
                .menu_with_disabled(
                    "清除 § 标记",
                    unavailable || readonly || !markers_present,
                    Box::new(crate::ClearIntruderPositions),
                )
                .menu_with_disabled(
                    "自动识别 § 标记",
                    unavailable || readonly,
                    Box::new(crate::AutoMarkIntruderPositions),
                )
                .separator();
            base_menu(menu, window, cx)
        }
    }

    pub(in crate::workbench) fn duplicate_intruder_tab(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || !self.backend_available()
            || !self.flush_workspace(window, cx)
        {
            return;
        }
        let Some(index) = self
            .workspace
            .intruder
            .tabs
            .iter()
            .position(|tab| tab.id == id)
        else {
            return;
        };
        let source = self.workspace.intruder.tabs[index].clone();
        let new_id = Uuid::new_v4();
        let mut spec = source.spec.clone();
        spec.revision = 1;
        spec.draft.revision = 1;
        if let Some(mio_core::IntruderPoolSelection::Create(pool)) = &mut spec.resource_pool {
            pool.id = Uuid::new_v4();
        }
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
        let title = if source.title.parse::<u32>().is_ok() {
            number.to_string()
        } else {
            let candidate = format!("{} (副本)", source.title);
            if candidate.len() <= 512 {
                candidate
            } else {
                number.to_string()
            }
        };
        let prev_active = self.workspace.intruder.active;
        let prev_grep = self.workspace.intruder.grep_configs.get(&id).cloned();
        self.workspace.intruder.tabs.insert(
            index + 1,
            mio_core::IntruderTab {
                id: new_id,
                title,
                group: source.group,
                spec,
                last_run: None,
            },
        );
        if let Some(grep) = prev_grep {
            self.workspace.intruder.grep_configs.insert(new_id, grep);
        }
        self.workspace.intruder.active = Some(new_id);
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.intruder.tabs.remove(index + 1);
            self.workspace.intruder.grep_configs.remove(&new_id);
            self.workspace.intruder.active = prev_active;
            self.fail(format!("未复制 Intruder 工作标签：{error}"), cx);
            return;
        }
        self.sync_intruder_editors(window, cx);
        self.save_workspace(cx);
        if let Some(editors) = self.intruder.editors.get(&new_id) {
            editors
                .request
                .state()
                .update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();
    }

    pub(in crate::workbench) fn new_intruder_group(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || !self.backend_available()
            || !self.flush_workspace(window, cx)
        {
            return;
        }
        let Some(tab_idx) = self
            .workspace
            .intruder
            .tabs
            .iter()
            .position(|tab| tab.id == id)
        else {
            return;
        };
        let group_id = Uuid::new_v4();
        let next_num = self.workspace.intruder.groups.len().saturating_add(1);
        let title = format!("分组 {next_num}");
        let prev_group = self.workspace.intruder.tabs[tab_idx].group;
        self.workspace.intruder.groups.push(mio_core::TabGroup {
            id: group_id,
            title,
            collapsed: false,
            color: None,
        });
        self.workspace.intruder.tabs[tab_idx].group = Some(group_id);
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace.intruder.groups.pop();
            self.workspace.intruder.tabs[tab_idx].group = prev_group;
            self.fail(format!("未创建 Intruder 分组：{error}"), cx);
            return;
        }
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn move_intruder_to_group(
        &mut self,
        id: TabId,
        group_id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || !self.backend_available()
            || !self.flush_workspace(window, cx)
        {
            return;
        }
        if let Some(target_gid) = group_id
            && !self
                .workspace
                .intruder
                .groups
                .iter()
                .any(|group| group.id == target_gid)
        {
            return;
        }
        let Some(tab_idx) = self
            .workspace
            .intruder
            .tabs
            .iter()
            .position(|tab| tab.id == id)
        else {
            return;
        };
        let prev_group = self.workspace.intruder.tabs[tab_idx].group;
        if prev_group == group_id {
            return;
        }
        self.workspace.intruder.tabs[tab_idx].group = group_id;
        if let Some(target_gid) = group_id
            && let Some(group) = self
                .workspace
                .intruder
                .groups
                .iter_mut()
                .find(|group| group.id == target_gid)
        {
            group.collapsed = false;
        }
        let mut removed_empty_groups = Vec::new();
        if let Some(old_gid) = prev_group
            && !self
                .workspace
                .intruder
                .tabs
                .iter()
                .chain(&self.workspace.intruder.closed)
                .any(|tab| tab.group == Some(old_gid))
            && let Some(pos) = self
                .workspace
                .intruder
                .groups
                .iter()
                .position(|group| group.id == old_gid)
        {
            removed_empty_groups.push((pos, self.workspace.intruder.groups.remove(pos)));
        }
        if let Err(error) = self.workspace.validate_structure() {
            for (pos, group) in removed_empty_groups.into_iter().rev() {
                self.workspace.intruder.groups.insert(pos, group);
            }
            self.workspace.intruder.tabs[tab_idx].group = prev_group;
            self.fail(format!("未移动 Intruder 标签分组：{error}"), cx);
            return;
        }
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn set_intruder_group_color(
        &mut self,
        group_id: Uuid,
        color: Option<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || !self.backend_available()
            || !self.flush_workspace(window, cx)
        {
            return;
        }
        if color.is_some_and(|c| c > 14) {
            return;
        }
        let Some(group) = self
            .workspace
            .intruder
            .groups
            .iter_mut()
            .find(|group| group.id == group_id)
        else {
            return;
        };
        if group.color == color {
            return;
        }
        let previous = group.color;
        group.color = color;
        if let Err(error) = self.workspace.validate_structure() {
            if let Some(group) = self
                .workspace
                .intruder
                .groups
                .iter_mut()
                .find(|group| group.id == group_id)
            {
                group.color = previous;
            }
            self.fail(format!("未更新 Intruder 分组颜色：{error}"), cx);
            return;
        }
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn ungroup_intruder_tabs(
        &mut self,
        group_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some()
            || !self.backend_available()
            || !self.flush_workspace(window, cx)
        {
            return;
        }
        let Some(group_pos) = self
            .workspace
            .intruder
            .groups
            .iter()
            .position(|group| group.id == group_id)
        else {
            return;
        };
        let removed_group = self.workspace.intruder.groups.remove(group_pos);
        let prev_open: Vec<TabId> = self
            .workspace
            .intruder
            .tabs
            .iter_mut()
            .filter_map(|tab| {
                if tab.group == Some(group_id) {
                    tab.group = None;
                    Some(tab.id)
                } else {
                    None
                }
            })
            .collect();
        let prev_closed: Vec<TabId> = self
            .workspace
            .intruder
            .closed
            .iter_mut()
            .filter_map(|tab| {
                if tab.group == Some(group_id) {
                    tab.group = None;
                    Some(tab.id)
                } else {
                    None
                }
            })
            .collect();
        if let Err(error) = self.workspace.validate_structure() {
            self.workspace
                .intruder
                .groups
                .insert(group_pos, removed_group);
            for tab in &mut self.workspace.intruder.tabs {
                if prev_open.contains(&tab.id) {
                    tab.group = Some(group_id);
                }
            }
            for tab in &mut self.workspace.intruder.closed {
                if prev_closed.contains(&tab.id) {
                    tab.group = Some(group_id);
                }
            }
            self.fail(format!("未取消 Intruder 分组：{error}"), cx);
            return;
        }
        if self
            .intruder
            .rename
            .as_ref()
            .is_some_and(|rename| rename.is_group && rename.id == group_id)
        {
            self.intruder.rename = None;
        }
        self.save_workspace(cx);
        cx.notify();
    }

    pub(in crate::workbench) fn close_intruder_group_tabs(
        &mut self,
        group_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transition.is_some() || !self.backend_available() {
            return;
        }
        let ids: Vec<TabId> = self
            .workspace
            .intruder
            .tabs
            .iter()
            .filter(|tab| tab.group == Some(group_id))
            .map(|tab| tab.id)
            .collect();
        self.close_intruder_tabs(&ids, window, cx);
    }

    pub(in crate::workbench) fn submit_result_window_new_pool(
        &mut self,
        project: ProjectKey,
        operation: Uuid,
        pool: mio_core::IntruderResourcePool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.project != Some(project) {
            return Err("当前项目已切换；未创建资源池。".into());
        }
        let Some(before) = self.intruder.resource_pools.catalog.clone() else {
            return Err("资源池目录尚未读取完成，请刷新后重试。".into());
        };
        pool.validate().map_err(|error| error.to_string())?;
        if before.find(pool.id).is_some() {
            return Err("新资源池身份已存在；未覆盖。".into());
        }
        let mut pools = before.pools.clone();
        pools.push(pool);
        let mut proposed = before.clone();
        proposed.pools = pools.clone();
        proposed.validate().map_err(|error| error.to_string())?;
        self.start_pool_mutation(
            operation,
            crate::workbench::settings::resource_pools::MutationKind::Update {
                catalog: before.clone(),
                pools: pools.clone(),
            },
            cx,
        )?;
        let cancel = self.shutdown.child_token();
        if !self.enqueue(
            Command::SetIntruderResourcePools {
                project,
                operation_id: operation,
                expected_revision: before.revision,
                pools,
                cancel,
            },
            cx,
        ) {
            self.intruder.resource_pools.management.cancel_pending();
            return Err(self
                .error
                .clone()
                .unwrap_or_else(|| "资源池创建命令未入队。".into()));
        }
        cx.notify();
        Ok(())
    }
}
