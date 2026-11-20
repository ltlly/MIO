use super::*;
use crate::{IntruderPoolDelayMode, IntruderPoolInputs};

fn retained_pool(mode: IntruderPoolDelayMode) -> IntruderResourcePool {
    let mut pool = pool();
    pool.max_concurrent_requests = None;
    pool.retained_inputs = Some(IntruderPoolInputs {
        cap: " 关闭时的输入 ".into(),
        delay: "00500".into(),
        increment: "0009".into(),
        delay_mode: mode,
    });
    pool.automatic_throttling.other_codes = " kept while disabled ".into();
    pool
}

#[test]
fn retained_pool_inputs_save_as_and_reopen_preserve_catalog_open_closed_and_frozen_values() {
    let mut store = ProjectStore::create(None).unwrap();
    let mut snapshots = Vec::new();
    for mode in [
        IntruderPoolDelayMode::Fixed,
        IntruderPoolDelayMode::Random,
        IntruderPoolDelayMode::Increment,
    ] {
        let configured = retained_pool(mode);
        let input = spec(Some(IntruderPoolSelection::Create(configured)), true);
        let tab = bind(&mut store, &input);
        let run = store
            .create_intruder_run(Uuid::new_v4(), tab, &input, 1)
            .unwrap();
        snapshots.push((run, input));
    }
    let mut workspace = store.workspace().unwrap();
    workspace.revision += 1;
    workspace
        .intruder
        .closed
        .push(workspace.intruder.tabs.pop().unwrap());
    workspace.intruder.active = Some(workspace.intruder.tabs[0].id);
    store.save_workspace(&workspace).unwrap();
    let catalog = store.intruder_resource_pools().unwrap();
    let mut changed = catalog.pools;
    changed[0].retained_inputs.as_mut().unwrap().delay = "unparsed while off".into();
    let catalog = store
        .set_intruder_resource_pools(
            project(&store),
            catalog.revision,
            changed,
            &CancellationToken::new(),
        )
        .unwrap();
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let original = directory.path().join("inputs.mio");
    let copy = directory.path().join("inputs-copy.mio");
    store.save(Some(&original)).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    for path in [&original, &copy] {
        let restored = ProjectStore::open(path).unwrap();
        assert_eq!(
            restored
                .connection
                .pragma_query_value::<i32, _>(None, "user_version", |row| row.get(0))
                .unwrap(),
            crate::store::SCHEMA
        );
        assert_eq!(restored.intruder_resource_pools().unwrap(), catalog);
        assert_eq!(restored.workspace().unwrap(), workspace);
        for (run, input) in &snapshots {
            assert_eq!(restored.read_intruder_run(run.id).unwrap(), *run);
            assert_eq!(restored.read_intruder_spec(run.id).unwrap(), *input);
        }
        assert!(
            catalog
                .pools
                .iter()
                .all(|pool| pool.delay.is_none() && pool.max_concurrent_requests.is_none())
        );
        restored.close().unwrap();
    }
}

#[test]
fn inconsistent_retained_input_rejects_catalog_and_start_without_changing_saved_data() {
    let mut store = ProjectStore::create(None).unwrap();
    let configured = retained_pool(IntruderPoolDelayMode::Fixed);
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
    let mut invalid = configured;
    invalid.delay = Some(IntruderPoolDelay::Fixed { milliseconds: 501 });
    assert!(
        store
            .set_intruder_resource_pools(
                project(&store),
                catalog.revision,
                vec![invalid.clone()],
                &CancellationToken::new()
            )
            .unwrap_err()
            .to_string()
            .contains("不一致")
    );
    invalid.id = Uuid::new_v4();
    let invalid = spec(Some(IntruderPoolSelection::Create(invalid)), true);
    assert!(
        store
            .create_intruder_run_with_pool_catalog(
                project(&store),
                Uuid::new_v4(),
                tab,
                &invalid,
                2,
                &CancellationToken::new()
            )
            .unwrap_err()
            .to_string()
            .contains("不一致")
    );
    assert_eq!(store.intruder_resource_pools().unwrap(), catalog);
    assert_eq!(store.workspace().unwrap(), workspace);
    assert_eq!(store.read_intruder_run(run.id).unwrap(), run);
    assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
    assert_eq!(store.intruder_runs(None, 10).unwrap().rows.len(), 1);
}
