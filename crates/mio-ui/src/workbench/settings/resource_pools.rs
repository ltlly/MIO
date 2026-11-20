use std::time::{Duration, Instant};

use gpui_kit::AnyWindowHandle;
use mio_core::{IntruderPoolUsage, IntruderResourcePool, IntruderResourcePools, Uuid};

use super::*;

mod actions;
mod editor;
mod table;
#[cfg(test)]
mod tests;

#[cfg(test)]
use editor::PoolEditor;
pub(super) use table::ScheduledTasksTable;
use table::PoolTable;

const DEADLINE: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(in crate::workbench) struct PoolManagement {
    usage: Option<IntruderPoolUsage>,
    query: Option<(ProjectKey, u64, Instant)>,
    query_revision: u64,
    usage_dirty: bool,
    query_timer: Option<Task<()>>,
    pending: Option<PoolMutation>,
    error: Option<String>,
    outcome: Option<(Uuid, Result<(), String>)>,
}

pub(in crate::workbench) enum MutationKind {
    Update {
        catalog: IntruderResourcePools,
        pools: Vec<IntruderResourcePool>,
    },
    Delete {
        pool_id: Uuid,
        catalog: IntruderResourcePools,
        workspace: Option<Box<Workspace>>,
        allow_reassign: bool,
        sent: bool,
    },
}

struct PoolMutation {
    project: ProjectKey,
    operation: Uuid,
    kind: MutationKind,
    cancel: CancellationToken,
    deadline: Instant,
    expired: bool,
    _timer: Option<Task<()>>,
}

impl Drop for PoolMutation {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl PoolManagement {
    pub(in crate::workbench) fn mutation_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub(in crate::workbench) fn delete_pending(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|p| matches!(p.kind, MutationKind::Delete { sent: true, .. }))
    }
    pub(in crate::workbench) fn delete_preparing(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|p| matches!(p.kind, MutationKind::Delete { sent: false, .. }))
    }
    pub(in crate::workbench) fn cancel_pending(&mut self) {
        if let Some(pending) = &self.pending {
            pending.cancel.cancel();
        }
    }
    pub(in crate::workbench) fn disconnect(&mut self) {
        self.cancel_pending();
        self.query = None;
        self.usage_dirty = false;
        self.query_timer = None;
        self.error = Some("后台已断开；资源池操作结果未确认，原输入保留。".into());
    }
}

#[derive(Clone)]
pub(super) struct PoolPageState {
    catalog: Option<IntruderResourcePools>,
    usage: Option<IntruderPoolUsage>,
    reading: bool,
    preparing: bool,
    busy: bool,
    enabled: bool,
    error: Option<String>,
    outcome: Option<(Uuid, Result<(), String>)>,
}

impl PoolPageState {
    pub(super) fn new(view: &Workbench) -> Self {
        let pools = &view.intruder.resource_pools;
        let management = &pools.management;
        Self {
            catalog: pools.catalog.clone(),
            usage: management.usage.clone(),
            reading: pools.pending.is_some() || management.query.is_some(),
            preparing: management.delete_preparing() && view.pending_workspace.is_none(),
            busy: management.mutation_pending(),
            enabled: view.backend_available()
                && view.transition.is_none()
                && view.quit_attempt.is_none()
                && !view.shutdown_requested
                && view.pending_records == 0
                && view.storage_error.is_none(),
            error: management.error.clone().or_else(|| pools.error.clone()),
            outcome: management.outcome.clone(),
        }
    }

    fn ready(&self) -> bool {
        self.enabled
            && !self.busy
            && !self.reading
            && self.error.is_none()
            && self
                .catalog
                .as_ref()
                .zip(self.usage.as_ref())
                .is_some_and(|(catalog, usage)| catalog.revision == usage.catalog_revision)
    }
}

pub(super) struct PoolPage {
    pub(super) selected: Option<Uuid>,
    table: Entity<TableState<PoolTable>>,
    dialog: Option<AnyWindowHandle>,
    opening: bool,
    confirming: bool,
}

impl PoolPage {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Settings>) -> Self {
        let owner = cx.weak_entity();
        let table = cx.new(|table_cx| {
            TableState::new(PoolTable { owner }, window, table_cx)
                .sortable(false)
                .col_selectable(false)
        });
        cx.subscribe_in(&table, window, |this, _, event, _, cx| {
            if let TableEvent::SelectRow(index) = event {
                this.pools.selected = this
                    .state
                    .pools
                    .catalog
                    .as_ref()
                    .and_then(|catalog| catalog.pools.get(*index))
                    .map(|pool| pool.id);
                cx.notify();
            }
        })
        .detach();
        Self {
            selected: None,
            table,
            dialog: None,
            opening: false,
            confirming: false,
        }
    }
    pub(super) fn refresh(&mut self, cx: &mut Context<Settings>) {
        self.table.update(cx, |table, cx| {
            table.refresh(cx);
            cx.notify();
        });
    }
    pub(super) fn close_dialog(&mut self, cx: &mut App) {
        if let Some(dialog) = self.dialog.take() {
            let _ = dialog.update(cx, |_, window, _| window.remove_window());
        }
    }
}
