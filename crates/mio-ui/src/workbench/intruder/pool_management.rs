use mio_core::{IntruderPoolSelection, IntruderResourcePools};

use super::*;

#[cfg(test)]
#[path = "pool_management_tests.rs"]
mod legacy_create_tests;

fn references(selection: Option<&IntruderPoolSelection>, id: Uuid) -> bool {
    match selection {
        Some(IntruderPoolSelection::Existing(pool)) => *pool == id,
        Some(IntruderPoolSelection::Create(pool)) => pool.id == id,
        None => false,
    }
}

impl Workbench {
    pub(in crate::workbench) fn intruder_pool_management_available(&self) -> bool {
        self.backend_available()
            && self.transition.is_none()
            && self.quit_attempt.is_none()
            && !self.file_picker_pending()
            && self.pending_project_save.is_none()
            && self.project_saves_in_flight == 0
            && self.intruder.starts.is_empty()
            && self.intruder.default_pool.is_none()
            && self.intruder.pool_changes.is_empty()
            && self.intruder.baseline_requests.is_empty()
            && self.intruder_result_close.is_none()
            && self.pending_records == 0
            && self.storage_error.is_none()
    }

    pub(in crate::workbench) fn apply_deleted_pool_state(
        &mut self,
        project: ProjectKey,
        pool_id: Uuid,
        catalog: &IntruderResourcePools,
        workspaces: (&Workspace, &Workspace),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.project != Some(project) {
            return Err("资源池删除回执属于其它项目。".into());
        }
        let (before, stored) = workspaces;
        catalog.validate().map_err(|error| error.to_string())?;
        stored
            .validate_structure()
            .map_err(|error| error.to_string())?;
        if !self.accept_intruder_resource_pools(project, catalog, cx) {
            return Err("资源池回执目录未被采用；请重新读取实际状态。".into());
        }
        if catalog.find(pool_id).is_none() {
            let default = catalog.default_pool_id;
            let mut workspace = self.workspace.clone();
            for tab in workspace
                .intruder
                .tabs
                .iter_mut()
                .chain(&mut workspace.intruder.closed)
            {
                let mut spec = tab.spec.clone();
                if references(spec.resource_pool.as_ref(), pool_id) {
                    spec.resource_pool = Some(IntruderPoolSelection::Existing(
                        default.ok_or("删除回执缺少默认资源池；当前编辑保留。")?,
                    ));
                }
                spec = revised_spec(&tab.spec, spec)?;
                if let Some(saved) = stored
                    .intruder
                    .tabs
                    .iter()
                    .chain(&stored.intruder.closed)
                    .find(|saved| saved.id == tab.id)
                {
                    spec = rebased_spec(&spec, &saved.spec)?;
                }
                tab.spec = spec;
            }
            workspace.revision = workspace.revision.max(stored.revision);
            workspace
                .validate_structure()
                .map_err(|error| error.to_string())?;
            self.workspace_dirty |= workspace != *stored;
            self.workspace = workspace;
            // Keep uncommitted text and IME; only replace references to the deleted identity.
            for (id, editors) in &mut self.intruder.editors {
                if editors.pool_form.existing == Some(pool_id) {
                    editors.pool_form.existing = default;
                }
                if editors.pool_form.id == pool_id {
                    let previous = before
                        .intruder
                        .tabs
                        .iter()
                        .find(|tab| tab.id == *id)
                        .and_then(|tab| match &tab.spec.resource_pool {
                            Some(IntruderPoolSelection::Create(pool)) if pool.id == pool_id => {
                                Some(pool)
                            }
                            _ => None,
                        });
                    if editors.pool_form.creating
                        && !editors.pool_form.has_composition(window, cx)
                        && previous.is_some_and(|pool| {
                            editors.pool_form.snapshot(window, cx).as_ref() == Ok(pool)
                        })
                    {
                        editors.pool_form.creating = false;
                        editors.pool_form.existing = default;
                    }
                    editors.pool_form.id = Uuid::new_v4();
                    editors.dirty |= editors.pool_form.creating;
                }
            }
        }
        let ids: HashSet<_> = self
            .intruder
            .runs
            .keys()
            .copied()
            .chain(self.intruder.results.views.values().map(|view| view.run_id))
            .collect();
        for id in ids {
            self.query_intruder_run_status(id, cx);
        }
        if self.intruder.results.runs.loaded && self.intruder.results.runs.pending.is_none() {
            self.query_intruder_runs(self.intruder.results.runs.after, cx);
        }
        cx.notify();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::TestAppContext;
    use gpui_kit::test::TestWindowExt;

    use super::*;
    use crate::workbench::intruder::tests::{attack, default_pool_catalog, intruder_fixture};
    use crate::workbench::tests::drain;

    #[gpui_kit::test]
    fn delete_receipt_preserves_uncommitted_target_while_rebasing_open_and_closed_bindings(
        cx: &mut TestAppContext,
    ) {
        let mut catalog = default_pool_catalog();
        let default = catalog.default_pool_id.unwrap();
        let mut pool = catalog.pools[0].clone();
        pool.id = Uuid::new_v4();
        pool.name = "删除池".into();
        catalog.pools.push(pool.clone());
        let mut tab = attack();
        tab.spec.resource_pool = Some(IntruderPoolSelection::Existing(pool.id));
        let id = tab.id;
        let mut fixture = intruder_fixture(cx, vec![tab.clone()], 32);
        let stored = cx
            .update_window(fixture.window, |_, window, cx| {
                let (stored, target) = fixture.view.update(cx, |view, cx| {
                    view.accept_intruder_resource_pools(fixture.project, &catalog, cx);
                    let mut closed = tab.clone();
                    closed.id = Uuid::new_v4();
                    view.workspace.intruder.closed.push(closed);
                    view.tool = Tool::Intruder;
                    let stored = view.workspace.clone();
                    let target = view.intruder.editors[&id].target.clone();
                    cx.notify();
                    (stored, target)
                });
                window.render_frame(cx);
                window.click(("input", target.entity_id()), cx);
                window.press(crate::kbd!("a"), cx);
                window.input("http://127.0.0.1:9123/new-target", cx);
                stored
            })
            .unwrap();
        cx.run_until_parked();
        assert!(drain(&mut fixture.commands).is_empty());
        let mut committed = stored.clone();
        committed.revision += 1;
        for tab in committed
            .intruder
            .tabs
            .iter_mut()
            .chain(&mut committed.intruder.closed)
        {
            tab.spec.resource_pool = Some(IntruderPoolSelection::Existing(default));
            tab.spec.revision += 1;
        }
        catalog.pools.retain(|p| p.id != pool.id);
        catalog.revision += 1;
        cx.update_window(fixture.window, |_, window, cx| {
            fixture.view.update(cx, |view, cx| {
                view.apply_deleted_pool_state(
                    fixture.project,
                    pool.id,
                    &catalog,
                    (&stored, &committed),
                    window,
                    cx,
                )
                .unwrap();
                assert_eq!(view.workspace, committed);
                let snapshot = view.snapshot_intruder_spec(id, window, cx).unwrap();
                assert_eq!(snapshot.draft.target, "http://127.0.0.1:9123/new-target");
                assert_eq!(
                    snapshot.resource_pool,
                    Some(IntruderPoolSelection::Existing(default))
                );
                assert!(snapshot.revision > committed.intruder.tabs[0].spec.revision);
                assert!(view.intruder.editors[&id].dirty);
                assert_eq!(view.intruder.editors[&id].pool_form.existing, Some(default));
            });
        })
        .unwrap();
        assert!(
            drain(&mut fixture.commands).is_empty(),
            "a delete receipt must not save or replay local edits"
        );
    }
}
