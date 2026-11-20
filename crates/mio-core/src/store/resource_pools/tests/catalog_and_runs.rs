use super::*;

#[test]
fn project_pool_catalog_cas_and_save_as_preserve_ids_and_independent_legacy_caps() {
    let mut store = ProjectStore::create(None).unwrap();
    assert_eq!(
        store.intruder_resource_pools().unwrap(),
        IntruderResourcePools::default()
    );
    let first = pool();
    let mut second = pool();
    second.max_concurrent_requests = None;
    let catalog = store
        .set_intruder_resource_pools(
            project(&store),
            0,
            vec![first.clone(), second.clone()],
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(catalog.revision, 1);
    assert!(
        store
            .set_intruder_resource_pools(project(&store), 0, vec![], &CancellationToken::new())
            .is_err()
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(
        store
            .set_intruder_resource_pools(project(&store), 1, vec![], &cancelled)
            .is_err()
    );
    assert_eq!(store.intruder_resource_pools().unwrap(), catalog);
    for cap in [1, 2, 16] {
        let mut input = spec(None, true);
        input.max_concurrent_requests = cap;
        let tab = bind(&mut store, &input);
        let run = store
            .create_intruder_run(Uuid::new_v4(), tab, &input, 1)
            .unwrap();
        assert_eq!((run.resource_pool, run.resource_pool_revision), (None, 0));
        assert_eq!(
            store
                .read_intruder_spec(run.id)
                .unwrap()
                .max_concurrent_requests,
            cap
        );
    }
    let catalog = store
        .set_intruder_resource_pools(
            project(&store),
            catalog.revision,
            vec![first],
            &CancellationToken::new(),
        )
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("pool.mio");
    let copy = directory.path().join("pool-copy.mio");
    store.save(Some(&original)).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    for path in [&original, &copy] {
        let reopened = ProjectStore::open(path).unwrap();
        assert_eq!(reopened.intruder_resource_pools().unwrap(), catalog);
        assert_eq!(reopened.intruder_runs(None, 10).unwrap().rows.len(), 3);
        reopened.close().unwrap();
    }
}

#[test]
fn project_and_session_start_publish_create_pool_once_without_persisting_session_run() {
    for persistent in [false, true] {
        let mut store = ProjectStore::create(None).unwrap();
        let created = pool();
        let input = spec(
            Some(IntruderPoolSelection::Create(created.clone())),
            persistent,
        );
        let tab = bind(&mut store, &input);
        assert_eq!(
            store.intruder_resource_pools().unwrap(),
            IntruderResourcePools::default()
        );
        let (run, catalog) = store
            .create_intruder_run_with_pool_catalog(
                project(&store),
                Uuid::new_v4(),
                tab,
                &input,
                1,
                &CancellationToken::new(),
            )
            .unwrap();
        assert_eq!(
            (run.resource_pool, run.resource_pool_revision),
            (Some(created.id), 1)
        );
        assert_eq!(catalog, store.intruder_resource_pools().unwrap());
        assert_eq!(catalog.pools, [created]);
        assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
        assert!(
            store
                .create_intruder_run_with_pool_catalog(
                    project(&store),
                    Uuid::new_v4(),
                    tab,
                    &input,
                    2,
                    &CancellationToken::new()
                )
                .is_err()
        );
        assert_eq!(store.intruder_resource_pools().unwrap(), catalog);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pool-start.mio");
        store.save(Some(&path)).unwrap();
        let disk = ProjectStore::open(&path).unwrap();
        assert_eq!(disk.intruder_resource_pools().unwrap(), catalog);
        assert_eq!(
            disk.find_intruder_run(run.id).unwrap().is_some(),
            persistent
        );
        disk.close().unwrap();
        if !persistent {
            assert_eq!(store.read_intruder_run(run.id).unwrap(), run);
            let mut workspace = store.workspace().unwrap();
            workspace.revision += 1;
            let promoted = store
                .set_intruder_retention(
                    project(&store),
                    run.id,
                    run.revision,
                    IntruderRetention::Project,
                    workspace,
                    &CancellationToken::new(),
                )
                .unwrap();
            assert_eq!(
                (
                    promoted.run.resource_pool,
                    promoted.run.resource_pool_revision
                ),
                (run.resource_pool, 1)
            );
            assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
        }
    }
}

#[test]
fn pool_create_and_start_failures_roll_back_catalog_run_and_session_publication() {
    for persistent in [false, true] {
        for failure in ["commit", "cancel", "during_catalog", "invalid"] {
            let mut store = ProjectStore::create(None).unwrap();
            let input = spec(Some(IntruderPoolSelection::Create(pool())), persistent);
            let tab = bind(&mut store, &input);
            let token = CancellationToken::new();
            let mut sent = input.clone();
            match failure {
                "commit" => {
                    store.connection.commit_hook(Some(|| true));
                }
                "cancel" => token.cancel(),
                "during_catalog" => {
                    let cancel = token.clone();
                    store
                        .connection
                        .authorizer(Some(move |context: AuthContext<'_>| {
                            if matches!(
                                context.action,
                                AuthAction::Insert {
                                    table_name: "metadata",
                                    ..
                                }
                            ) {
                                cancel.cancel();
                            }
                            Authorization::Allow
                        }));
                }
                "invalid" => sent.draft.target = "invalid target".into(),
                _ => unreachable!(),
            }
            let id = Uuid::new_v4();
            assert!(
                store
                    .create_intruder_run_with_pool_catalog(
                        project(&store),
                        id,
                        tab,
                        &sent,
                        1,
                        &token
                    )
                    .is_err(),
                "{persistent}/{failure}"
            );
            store.connection.commit_hook(None::<fn() -> bool>);
            store
                .connection
                .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
            assert_eq!(
                store.intruder_resource_pools().unwrap(),
                IntruderResourcePools::default()
            );
            assert!(store.find_intruder_run(id).unwrap().is_none());
            assert!(!store.is_session_intruder_run(id));
            assert_eq!(store.workspace().unwrap().intruder.tabs[0].spec, input);
        }
    }
    let mut store = ProjectStore::create(None).unwrap();
    store.set_intruder_session_quota(1);
    let input = spec(Some(IntruderPoolSelection::Create(pool())), false);
    let tab = bind(&mut store, &input);
    let id = Uuid::new_v4();
    assert!(
        store
            .create_intruder_run_with_pool_catalog(
                project(&store),
                id,
                tab,
                &input,
                1,
                &CancellationToken::new()
            )
            .is_err()
    );
    assert_eq!(
        store.intruder_resource_pools().unwrap(),
        IntruderResourcePools::default()
    );
    assert!(store.find_intruder_run(id).unwrap().is_none());
}

#[test]
fn run_pool_binding_has_an_independent_revision_and_never_rewrites_frozen_create() {
    for persistent in [false, true] {
        let mut store = ProjectStore::create(None).unwrap();
        let first = pool();
        let second = pool();
        let input = spec(
            Some(IntruderPoolSelection::Create(first.clone())),
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
        store
            .set_intruder_resource_pools(
                project(&store),
                1,
                vec![first, second.clone()],
                &CancellationToken::new(),
            )
            .unwrap();
        if persistent {
            store.connection.commit_hook(Some(|| true));
        } else {
            store.reject_session_intruder_writes(run.id, true);
        }
        assert!(
            store
                .set_intruder_resource_pool(
                    project(&store),
                    run.id,
                    1,
                    second.id,
                    &CancellationToken::new()
                )
                .is_err()
        );
        store.connection.commit_hook(None::<fn() -> bool>);
        if !persistent {
            store.reject_session_intruder_writes(run.id, false);
        }
        assert_eq!(store.read_intruder_run(run.id).unwrap(), run);
        let rebound = store
            .set_intruder_resource_pool(
                project(&store),
                run.id,
                1,
                second.id,
                &CancellationToken::new(),
            )
            .unwrap();
        let mut expected = run.clone();
        expected.resource_pool = Some(second.id);
        expected.resource_pool_revision = 2;
        assert_eq!(rebound, expected);
        assert!(
            store
                .set_intruder_resource_pool(
                    project(&store),
                    run.id,
                    2,
                    second.id,
                    &CancellationToken::new()
                )
                .is_err()
        );
        assert!(
            store
                .set_intruder_resource_pool(
                    project(&store),
                    run.id,
                    1,
                    input.resource_pool.as_ref().unwrap().id(),
                    &CancellationToken::new()
                )
                .is_err()
        );
        let changed = store
            .put_intruder_result(run.id, 0, Err("payload result"))
            .unwrap();
        assert_eq!(
            (changed.resource_pool, changed.resource_pool_revision),
            (Some(second.id), 2)
        );
        assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
        store
            .put_intruder_result(run.id, 1, Err("other payload"))
            .unwrap();
        let finished = store
            .set_intruder_state(run.id, IntruderRunState::Completed)
            .unwrap();
        let catalog = store.intruder_resource_pools().unwrap();
        let baseline = store
            .request_intruder_baseline(
                project(&store),
                run.id,
                finished.revision,
                &CancellationToken::new(),
            )
            .unwrap();
        assert_eq!(
            (baseline.resource_pool, baseline.resource_pool_revision),
            (Some(second.id), 2)
        );
        assert_eq!(store.intruder_resource_pools().unwrap(), catalog);
        assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
    }
}
