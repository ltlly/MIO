use super::*;

#[test]
fn random_delay_save_as_preserves_catalog_open_closed_and_frozen_create() {
    let mut store = ProjectStore::create(None).unwrap();
    let mut snapshots = Vec::new();
    for milliseconds in [0, 300_000] {
        let mut configured = pool();
        configured.delay = Some(IntruderPoolDelay::Random { milliseconds });
        let input = spec(Some(IntruderPoolSelection::Create(configured)), true);
        let tab = bind(&mut store, &input);
        let run = store
            .create_intruder_run(Uuid::new_v4(), tab, &input, 1)
            .unwrap();
        snapshots.push((run, input));
    }
    let mut workspace = store.workspace().unwrap();
    workspace.revision += 1;
    let closed = workspace.intruder.tabs.pop().unwrap();
    workspace.intruder.closed.push(closed);
    workspace.intruder.active = Some(workspace.intruder.tabs[0].id);
    store.save_workspace(&workspace).unwrap();
    let catalog = store.intruder_resource_pools().unwrap();
    let mut changed = catalog.pools;
    changed[0].delay = Some(IntruderPoolDelay::Random {
        milliseconds: 123_456,
    });
    let catalog = store
        .set_intruder_resource_pools(
            project(&store),
            catalog.revision,
            changed,
            &CancellationToken::new(),
        )
        .unwrap();

    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let original = directory.path().join("random.mio");
    let copy = directory.path().join("random-copy.mio");
    store.save(Some(&original)).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    for path in [&original, &copy] {
        let restored = ProjectStore::open(path).unwrap();
        assert_eq!(restored.intruder_resource_pools().unwrap(), catalog);
        assert_eq!(restored.workspace().unwrap(), workspace);
        for (run, input) in &snapshots {
            assert_eq!(restored.read_intruder_run(run.id).unwrap(), *run);
            assert_eq!(restored.read_intruder_spec(run.id).unwrap(), *input);
        }
        restored.close().unwrap();
    }
}

#[test]
fn out_of_range_random_delay_rejects_catalog_and_start_without_mutation() {
    let mut store = ProjectStore::create(None).unwrap();
    let configured = pool();
    let input = spec(
        Some(IntruderPoolSelection::Create(configured.clone())),
        true,
    );
    let tab = bind(&mut store, &input);
    let run = store
        .create_intruder_run(Uuid::new_v4(), tab, &input, 1)
        .unwrap();
    let catalog = store.intruder_resource_pools().unwrap();
    let workspace = store.workspace().unwrap();
    let original_bytes: Vec<u8> = store
        .connection
        .query_row("SELECT value FROM metadata WHERE key=?1", [KEY], |row| {
            row.get(0)
        })
        .unwrap();
    let mut invalid = configured;
    invalid.delay = Some(IntruderPoolDelay::Random {
        milliseconds: 300_001,
    });
    assert!(
        store
            .set_intruder_resource_pools(
                project(&store),
                catalog.revision,
                vec![invalid.clone()],
                &CancellationToken::new(),
            )
            .unwrap_err()
            .to_string()
            .contains("300000")
    );
    invalid.id = Uuid::new_v4();
    let invalid_input = spec(Some(IntruderPoolSelection::Create(invalid)), true);
    assert!(
        store
            .create_intruder_run_with_pool_catalog(
                project(&store),
                Uuid::new_v4(),
                tab,
                &invalid_input,
                2,
                &CancellationToken::new(),
            )
            .unwrap_err()
            .to_string()
            .contains("300000")
    );
    let actual_bytes: Vec<u8> = store
        .connection
        .query_row("SELECT value FROM metadata WHERE key=?1", [KEY], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(actual_bytes, original_bytes);
    assert_eq!(store.intruder_resource_pools().unwrap(), catalog);
    assert_eq!(store.workspace().unwrap(), workspace);
    assert_eq!(store.read_intruder_run(run.id).unwrap(), run);
    assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
    assert_eq!(store.intruder_runs(None, 10).unwrap().rows.len(), 1);
    store.close().unwrap();
}
