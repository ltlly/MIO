use super::*;
use std::collections::BTreeMap;

fn remove(
    store: &mut ProjectStore,
    pool: Uuid,
    allow: bool,
) -> Result<crate::IntruderPoolDeleteReceipt, crate::IntruderPoolDeleteError> {
    store.delete_intruder_resource_pool(
        project(store),
        pool,
        (
            store.intruder_resource_pools().unwrap().revision,
            store.workspace().unwrap().revision,
        ),
        allow,
        false,
        &CancellationToken::new(),
    )
}

#[test]
fn unused_delete_does_not_initialize_legacy_default_and_strict_set_stays_strict() {
    let mut store = ProjectStore::create(None).unwrap();
    let unused = pool();
    let used = pool();
    store
        .set_intruder_resource_pools(
            project(&store),
            0,
            vec![unused.clone(), used.clone()],
            &CancellationToken::new(),
        )
        .unwrap();
    let input = spec(Some(IntruderPoolSelection::Existing(used.id)), true);
    bind(&mut store, &input);
    let workspace = store.workspace().unwrap();
    let usage = store
        .intruder_pool_usage(&CancellationToken::new())
        .unwrap();
    assert_eq!(
        usage.task_counts,
        BTreeMap::from([(unused.id, 0), (used.id, 0)])
    );
    assert_eq!(usage.requires_confirmation, BTreeSet::from([used.id]));
    assert!(
        store
            .set_intruder_resource_pools(
                project(&store),
                1,
                vec![unused.clone()],
                &CancellationToken::new()
            )
            .is_err()
    );
    let receipt = remove(&mut store, unused.id, false).unwrap();
    assert!(receipt.catalog.default_pool_id.is_none());
    assert_eq!(
        receipt.catalog.pools.as_slice(),
        std::slice::from_ref(&used)
    );
    assert_eq!(receipt.workspace, workspace);
    assert_eq!((receipt.reassigned_runs, receipt.reassigned_tabs), (0, 0));
    let error = remove(&mut store, used.id, false).unwrap_err();
    assert!(error.message.contains("确认"));
    assert_eq!(error.reassigned_sessions, 0);
    assert!(!error.default_pool_created);
    assert_eq!(store.workspace().unwrap(), workspace);
}

#[test]
fn confirmed_delete_rebinds_project_and_session_open_and_closed_without_changing_frozen_inputs() {
    let mut store = ProjectStore::create(None).unwrap();
    let catalog = store
        .ensure_intruder_default_pool(project(&store), &CancellationToken::new())
        .unwrap();
    let default = catalog.default_pool_id.unwrap();
    let deleted = pool();
    let mut pools = catalog.pools;
    pools.push(deleted.clone());
    store
        .set_intruder_resource_pools(
            project(&store),
            catalog.revision,
            pools,
            &CancellationToken::new(),
        )
        .unwrap();
    let mut runs = Vec::new();
    for persistent in [false, true] {
        let input = spec(
            Some(IntruderPoolSelection::Existing(deleted.id)),
            persistent,
        );
        let tab = bind(&mut store, &input);
        let (run, _) = store
            .create_intruder_run_with_pool_catalog(
                project(&store),
                Uuid::new_v4(),
                tab,
                &input,
                1,
                &CancellationToken::new(),
            )
            .unwrap();
        let run = store
            .set_intruder_state(run.id, IntruderRunState::Stopped)
            .unwrap();
        runs.push((run, input));
    }
    let mut workspace = store.workspace().unwrap();
    workspace.revision += 1;
    workspace
        .intruder
        .closed
        .push(workspace.intruder.tabs.remove(0));
    store.save_workspace(&workspace).unwrap();
    let usage = store
        .intruder_pool_usage(&CancellationToken::new())
        .unwrap();
    assert_eq!(
        usage.task_counts[&deleted.id], 2,
        "terminal datasets remain tasks"
    );
    assert_eq!(usage.task_counts[&default], 0);
    let receipt = remove(&mut store, deleted.id, true).unwrap();
    assert_eq!((receipt.reassigned_runs, receipt.reassigned_tabs), (2, 2));
    assert!(receipt.catalog.find(deleted.id).is_none());
    assert_eq!(receipt.workspace.revision, workspace.revision + 1);
    for tab in receipt
        .workspace
        .intruder
        .tabs
        .iter()
        .chain(&receipt.workspace.intruder.closed)
    {
        let old = workspace
            .intruder
            .tabs
            .iter()
            .chain(&workspace.intruder.closed)
            .find(|old| old.id == tab.id)
            .unwrap();
        let mut expected = old.clone();
        expected.spec.resource_pool = Some(IntruderPoolSelection::Existing(default));
        expected.spec.revision += 1;
        assert_eq!(*tab, expected);
    }
    for (run, input) in runs {
        let mut expected = run.clone();
        expected.resource_pool = Some(default);
        expected.resource_pool_revision += 1;
        assert_eq!(store.read_intruder_run(run.id).unwrap(), expected);
        assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
    }
    let usage = store
        .intruder_pool_usage(&CancellationToken::new())
        .unwrap();
    assert_eq!(usage.task_counts, BTreeMap::from([(default, 2)]));
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    store
        .save(Some(&directory.path().join("deleted.mio")))
        .unwrap();
    assert!(
        remove(&mut store, default, true)
            .unwrap_err()
            .message
            .contains("默认")
    );
}

#[test]
fn legacy_used_delete_initializes_default_once_and_reports_the_actual_catalog() {
    let mut store = ProjectStore::create(None).unwrap();
    let deleted = pool();
    let input = spec(Some(IntruderPoolSelection::Create(deleted.clone())), true);
    let tab = bind(&mut store, &input);
    let (run, _) = store
        .create_intruder_run_with_pool_catalog(
            project(&store),
            Uuid::new_v4(),
            tab,
            &input,
            1,
            &CancellationToken::new(),
        )
        .unwrap();
    let receipt = remove(&mut store, deleted.id, true).unwrap();
    assert_eq!(receipt.catalog.revision, 3);
    let default = receipt.catalog.default_pool_id.unwrap();
    assert_eq!(receipt.catalog.pools, [default_pool(default)]);
    assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
    assert_eq!(
        store.read_intruder_run(run.id).unwrap().resource_pool,
        Some(default)
    );
}

#[test]
fn frozen_pool_history_neither_counts_as_current_usage_nor_requires_delete_confirmation() {
    let mut store = ProjectStore::create(None).unwrap();
    let catalog = store
        .ensure_intruder_default_pool(project(&store), &CancellationToken::new())
        .unwrap();
    let default = catalog.default_pool_id.unwrap();
    let old = pool();
    let input = spec(Some(IntruderPoolSelection::Create(old.clone())), true);
    let tab = bind(&mut store, &input);
    let (run, _) = store
        .create_intruder_run_with_pool_catalog(
            project(&store),
            Uuid::new_v4(),
            tab,
            &input,
            1,
            &CancellationToken::new(),
        )
        .unwrap();
    store
        .set_intruder_resource_pool(
            project(&store),
            run.id,
            run.resource_pool_revision,
            default,
            &CancellationToken::new(),
        )
        .unwrap();
    let mut workspace = store.workspace().unwrap();
    workspace.revision += 1;
    workspace.intruder.tabs[0].spec.revision += 1;
    workspace.intruder.tabs[0].spec.resource_pool = Some(IntruderPoolSelection::Existing(default));
    store.save_workspace(&workspace).unwrap();
    let usage = store
        .intruder_pool_usage(&CancellationToken::new())
        .unwrap();
    assert_eq!(usage.task_counts[&old.id], 0);
    assert!(!usage.requires_confirmation.contains(&old.id));
    let catalog = store.intruder_resource_pools().unwrap();
    assert!(
        store
            .set_intruder_resource_pools(
                project(&store),
                catalog.revision,
                catalog
                    .pools
                    .into_iter()
                    .filter(|pool| pool.id != old.id)
                    .collect(),
                &CancellationToken::new()
            )
            .is_err()
    );
    let receipt = remove(&mut store, old.id, false).unwrap();
    assert_eq!((receipt.reassigned_runs, receipt.reassigned_tabs), (0, 0));
    assert_eq!(receipt.workspace, workspace);
    assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
}

#[test]
fn delete_preflight_rejects_cas_cancel_revision_and_schema_before_any_session_reassignment() {
    for mode in 0..5 {
        let mut store = ProjectStore::create(None).unwrap();
        let catalog = store
            .ensure_intruder_default_pool(project(&store), &CancellationToken::new())
            .unwrap();
        let deleted = pool();
        let mut pools = catalog.pools;
        pools.push(deleted.clone());
        let catalog = store
            .set_intruder_resource_pools(
                project(&store),
                catalog.revision,
                pools,
                &CancellationToken::new(),
            )
            .unwrap();
        let input = spec(Some(IntruderPoolSelection::Existing(deleted.id)), false);
        let tab = bind(&mut store, &input);
        let (run, _) = store
            .create_intruder_run_with_pool_catalog(
                project(&store),
                Uuid::new_v4(),
                tab,
                &input,
                1,
                &CancellationToken::new(),
            )
            .unwrap();
        if mode == 3 {
            let mut workspace = store.workspace().unwrap();
            workspace.revision += 1;
            workspace.intruder.tabs[0].spec.revision = i64::MAX as u64;
            store.save_workspace(&workspace).unwrap();
        }
        if mode == 4 {
            store.connection.execute_batch("CREATE TABLE pool_extension(value BLOB REFERENCES metadata(value) ON UPDATE CASCADE)").unwrap();
        }
        let workspace = store.workspace().unwrap();
        let cancel = CancellationToken::new();
        if mode == 2 {
            cancel.cancel();
        }
        let error = store
            .delete_intruder_resource_pool(
                project(&store),
                deleted.id,
                (
                    catalog.revision - u64::from(mode == 0),
                    workspace.revision - u64::from(mode == 1),
                ),
                true,
                false,
                &cancel,
            )
            .unwrap_err();
        assert_eq!(error.reassigned_sessions, 0);
        assert!(!error.default_pool_created);
        assert_eq!(store.intruder_resource_pools().unwrap(), catalog);
        assert_eq!(store.workspace().unwrap(), workspace);
        assert_eq!(store.read_intruder_run(run.id).unwrap(), run);
        assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
    }
}
