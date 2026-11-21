use std::time::{Duration, Instant};

use mio_core::{
    IntruderPoolDelay, IntruderPoolSelection, IntruderResourcePool, IntruderResourcePools,
};

use super::*;

mod form;
mod render;
pub(in crate::workbench) use form::CreatePoolForm;
pub(in crate::workbench) use render::{PoolFlag, PoolFormAction, PoolFormChange, render_pool_form};
pub(in crate::workbench) use render::{pool_checkbox, pool_radio};

#[derive(Default)]
pub(in crate::workbench) struct ResourcePoolsUi {
    pub(in crate::workbench) management: crate::workbench::settings::resource_pools::PoolManagement,
    pub(in crate::workbench) catalog: Option<IntruderResourcePools>,
    pub(in crate::workbench) pending: Option<u64>,
    pub(in crate::workbench) error: Option<String>,
    query_revision: u64,
    deadline: Option<Instant>,
    timer: Option<Task<()>>,
}

impl ResourcePoolsUi {
    pub(in crate::workbench) fn disconnect(&mut self) {
        self.management.disconnect();
        self.pending = None;
        self.deadline = None;
        self.timer = None;
        self.error = Some("后台已断开；已读取的资源池目录保留，未确认新选择。".into());
    }
}

pub(in crate::workbench) fn pool_values(pool: &IntruderResourcePool) -> [String; 6] {
    let (delay, random, increment) = match pool.delay {
        None => (String::new(), String::new(), String::new()),
        Some(IntruderPoolDelay::Fixed { milliseconds }) => {
            (milliseconds.to_string(), String::new(), String::new())
        }
        Some(IntruderPoolDelay::Random { milliseconds }) => {
            (milliseconds.to_string(), "是".into(), String::new())
        }
        Some(IntruderPoolDelay::Increment {
            initial_ms,
            increment_ms,
        }) => (
            initial_ms.to_string(),
            String::new(),
            increment_ms.to_string(),
        ),
    };
    [
        pool.name.clone(),
        pool.max_concurrent_requests
            .map(|n| n.to_string())
            .unwrap_or_default(),
        delay,
        random,
        increment,
        if pool.automatic_throttling.enabled {
            "是"
        } else {
            "否"
        }
        .into(),
    ]
}

impl Workbench {
    fn expire_intruder_pool_query(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.intruder.resource_pools;
        if state.pending.is_some()
            && state
                .deadline
                .is_some_and(|deadline| cx.background_executor().now() >= deadline)
        {
            state.pending = None;
            state.deadline = None;
            state.timer = None;
            state.error =
                Some("资源池目录读取超时；已保留原选择，请重新打开 Resource pool 重试。".into());
            cx.notify();
        }
    }

    pub(in crate::workbench) fn query_intruder_resource_pools(&mut self, cx: &mut Context<Self>) {
        self.expire_intruder_pool_query(cx);
        let Some(project) = self.project else {
            return;
        };
        if self.intruder.resource_pools.pending.is_some() {
            return;
        }
        if !self.backend_available() || self.transition.is_some() || self.shutdown_requested {
            self.intruder.resource_pools.error =
                Some("当前项目或后台状态不允许读取资源池；原选择保留。".into());
            cx.notify();
            return;
        }
        self.intruder.resource_pools.query_revision += 1;
        let revision = self.intruder.resource_pools.query_revision;
        if !self.enqueue(
            Command::QueryIntruderResourcePools {
                project,
                query_revision: revision,
            },
            cx,
        ) {
            self.intruder.resource_pools.error = self.error.clone();
            return;
        }
        let state = &mut self.intruder.resource_pools;
        state.pending = Some(revision);
        state.deadline = Some(cx.background_executor().now() + Duration::from_secs(30));
        state.error = None;
        let timer = cx.background_executor().timer(Duration::from_secs(30));
        let owner = self.owner_window;
        let task = cx.spawn(async move |this, cx| {
            timer.await;
            let _ = owner.update(cx, |_, _, cx| {
                let _ = this.update(cx, |view, cx| {
                    if view.project == Some(project)
                        && view.intruder.resource_pools.pending == Some(revision)
                    {
                        view.expire_intruder_pool_query(cx);
                    }
                });
            });
        });
        self.intruder.resource_pools.timer = Some(task);
        cx.notify();
    }

    pub(in crate::workbench) fn accept_intruder_resource_pools(
        &mut self,
        project: ProjectKey,
        catalog: &IntruderResourcePools,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.project != Some(project) {
            return false;
        }
        if let Err(error) = catalog.validate() {
            self.intruder.resource_pools.error =
                Some(format!("资源池目录无效：{error}；原选择保留。"));
            cx.notify();
            return false;
        }
        if let Some(current) = &self.intruder.resource_pools.catalog {
            if catalog.revision < current.revision {
                return false;
            }
            if catalog.revision == current.revision && catalog != current {
                self.intruder.resource_pools.error =
                    Some("同 revision 的资源池目录不一致；已保留确认目录，请重新读取。".into());
                cx.notify();
                return false;
            }
        }
        self.intruder.resource_pools.catalog = Some(catalog.clone());
        self.intruder.resource_pools.error = None;
        cx.notify();
        true
    }

    pub(in crate::workbench) fn on_intruder_resource_pools(
        &mut self,
        project: ProjectKey,
        query_revision: u64,
        result: Result<IntruderResourcePools, String>,
        cx: &mut Context<Self>,
    ) {
        if self.project != Some(project)
            || self.intruder.resource_pools.pending != Some(query_revision)
        {
            return;
        }
        self.expire_intruder_pool_query(cx);
        if self.intruder.resource_pools.pending != Some(query_revision) {
            return;
        }
        self.intruder.resource_pools.pending = None;
        self.intruder.resource_pools.deadline = None;
        self.intruder.resource_pools.timer = None;
        if !self.backend_available() || self.transition.is_some() || self.shutdown_requested {
            self.intruder.resource_pools.error =
                Some("资源池回执到达时项目状态已改变；原选择保留，请重新读取。".into());
        } else {
            match result {
                Ok(catalog) => {
                    self.accept_intruder_resource_pools(project, &catalog, cx);
                }
                Err(error) => {
                    self.intruder.resource_pools.error = Some(format!(
                        "资源池目录读取失败：{error}；请重新打开 Resource pool 重试。"
                    ))
                }
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn intruder_pool_form_changed(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let create = self
            .intruder
            .editors
            .get(&id)
            .is_some_and(|editors| editors.pool_form.creating);
        if !create {
            cx.notify();
            return;
        }
        self.cancel_intruder_save_wait(id, cx);
        if let Some(editors) = self.intruder.editors.get_mut(&id) {
            editors.dirty = true;
        }
        if let Err(error) = self.snapshot_intruder_spec(id, window, cx) {
            if let Some(editors) = self.intruder.editors.get_mut(&id) {
                editors.pool_form.error = Some(error);
            }
            cx.notify();
            return;
        }
        if let Some(editors) = self.intruder.editors.get_mut(&id) {
            editors.pool_form.error = None;
        }
        self.flush_workspace(window, cx);
        cx.notify();
    }

    fn select_intruder_resource_pool(
        &mut self,
        id: TabId,
        pool_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.backend_available()
            || self.transition.is_some()
            || self.intruder.resource_pools.pending.is_some()
            || self.intruder.resource_pools.error.is_some()
            || !self
                .intruder
                .resource_pools
                .catalog
                .as_ref()
                .is_some_and(|catalog| catalog.find(pool_id).is_some())
        {
            return;
        }
        let Some(editors) = self.intruder.editors.get_mut(&id) else {
            return;
        };
        if editors.has_composition(window, cx) {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        if !editors.pool_form.creating && editors.pool_form.existing == Some(pool_id) {
            return;
        }
        editors.pool_form.creating = false;
        editors.pool_form.existing = Some(pool_id);
        editors.dirty = true;
        self.cancel_intruder_save_wait(id, cx);
        self.flush_workspace(window, cx);
        cx.notify();
    }

    fn use_intruder_existing_pool(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editors) = self.intruder.editors.get(&id) else {
            return;
        };
        let Some(pool_id) = editors.pool_form.existing else {
            self.fail("请在资源池表中选择已有池；当前创建输入仍保留。", cx);
            return;
        };
        self.select_intruder_resource_pool(id, pool_id, window, cx);
    }

    fn create_intruder_pool(&mut self, id: TabId, window: &mut Window, cx: &mut Context<Self>) {
        if !self.backend_available() || self.transition.is_some() {
            return;
        }
        let Some(editors) = self.intruder.editors.get_mut(&id) else {
            return;
        };
        if editors.has_composition(window, cx) {
            self.fail(editing::COMPOSITION_PENDING, cx);
            return;
        }
        if editors.pool_form.creating {
            return;
        }
        if self
            .intruder
            .resource_pools
            .catalog
            .as_ref()
            .is_some_and(|catalog| catalog.find(editors.pool_form.id).is_some())
            || editors.pool_form.existing == Some(editors.pool_form.id)
        {
            editors.pool_form.id = Uuid::new_v4();
        }
        editors.pool_form.creating = true;
        self.intruder_pool_form_changed(id, window, cx);
    }
}

#[cfg(test)]
mod tests;
