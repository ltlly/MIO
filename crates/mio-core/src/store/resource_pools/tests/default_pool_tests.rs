use super::*;
use rusqlite::OptionalExtension;

fn catalog_bytes(store: &ProjectStore) -> Option<Vec<u8>> {
    store
        .connection
        .query_row("SELECT value FROM metadata WHERE key=?1", [KEY], |row| {
            row.get(0)
        })
        .optional()
        .unwrap()
}

#[test]
fn explicit_default_pool_is_stable_idempotent_and_does_not_rebind_legacy_runs() {
    let mut store = ProjectStore::create(None).unwrap();
    assert_eq!(
        store.intruder_resource_pools().unwrap(),
        IntruderResourcePools::default()
    );
    assert_eq!(catalog_bytes(&store), None);
    let mut custom = pool();
    custom.name = "默认资源池".into();
    custom.automatic_throttling.other_codes = "retained, while disabled".into();
    let original = store
        .set_intruder_resource_pools(
            project(&store),
            0,
            vec![custom.clone()],
            &CancellationToken::new(),
        )
        .unwrap();
    let mut input = spec(None, true);
    input.max_concurrent_requests = 16;
    let tab = bind(&mut store, &input);
    let run = store
        .create_intruder_run(Uuid::new_v4(), tab, &input, 1)
        .unwrap();
    let workspace = store.workspace().unwrap();
    let catalog = store
        .ensure_intruder_default_pool(project(&store), &CancellationToken::new())
        .unwrap();
    let id = catalog.default_pool_id.unwrap();
    assert!(!id.is_nil());
    assert_ne!(
        id, custom.id,
        "a same-named custom pool is never adopted as the default"
    );
    assert_eq!(catalog.revision, original.revision + 1);
    assert_eq!(&catalog.pools[1..], original.pools.as_slice());
    let default = &catalog.pools[0];
    assert_eq!(default.id, id);
    assert_eq!(default.name, "默认资源池");
    assert_eq!(default.max_concurrent_requests, Some(10));
    assert_eq!(default.delay, None);
    assert_eq!(
        default.automatic_throttling,
        IntruderAutoThrottle {
            enabled: true,
            status_429: true,
            status_503: false,
            other_enabled: false,
            other_codes: String::new(),
        }
    );
    let bytes = catalog_bytes(&store);
    store
        .connection
        .authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Insert { .. } | AuthAction::Update { .. } | AuthAction::Delete { .. } => {
                Authorization::Deny
            }
            _ => Authorization::Allow,
        }));
    assert_eq!(
        store
            .ensure_intruder_default_pool(project(&store), &CancellationToken::new())
            .unwrap(),
        catalog
    );
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert_eq!(catalog_bytes(&store), bytes);
    assert_eq!(store.workspace().unwrap(), workspace);
    assert_eq!(store.read_intruder_run(run.id).unwrap(), run);
    assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let first = directory.path().join("default.mio");
    let copy = directory.path().join("default-copy.mio");
    store.save(Some(&first)).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    for path in [&first, &copy] {
        let mut reopened = ProjectStore::open(path).unwrap();
        assert_eq!(reopened.intruder_resource_pools().unwrap(), catalog);
        assert_eq!(catalog_bytes(&reopened), bytes);
        assert_eq!(
            reopened
                .ensure_intruder_default_pool(project(&reopened), &CancellationToken::new())
                .unwrap(),
            catalog
        );
        assert_eq!(reopened.read_intruder_run(run.id).unwrap(), run);
        assert_eq!(reopened.read_intruder_spec(run.id).unwrap(), input);
        reopened.close().unwrap();
    }
}

#[test]
fn default_pool_can_be_renamed_but_cannot_be_deleted_replaced_or_recreated_without_references() {
    let mut store = ProjectStore::create(None).unwrap();
    let catalog = store
        .ensure_intruder_default_pool(project(&store), &CancellationToken::new())
        .unwrap();
    let id = catalog.default_pool_id.unwrap();
    assert!(store.workspace().unwrap().intruder.tabs.is_empty());
    assert!(store.intruder_runs(None, 10).unwrap().rows.is_empty());
    assert!(
        store
            .set_intruder_resource_pools(
                project(&store),
                catalog.revision,
                vec![],
                &CancellationToken::new()
            )
            .is_err()
    );
    let mut impostor = catalog.pools[0].clone();
    impostor.id = Uuid::new_v4();
    assert!(
        store
            .set_intruder_resource_pools(
                project(&store),
                catalog.revision,
                vec![impostor],
                &CancellationToken::new()
            )
            .is_err()
    );
    for replacement in [None, Some(Uuid::new_v4())] {
        let transaction = store.connection.unchecked_transaction().unwrap();
        assert!(
            super::super::write(
                &transaction,
                &catalog,
                catalog.pools.clone(),
                replacement,
                &CancellationToken::new()
            )
            .is_err()
        );
    }
    assert_eq!(store.intruder_resource_pools().unwrap(), catalog);
    let mut pools = catalog.pools.clone();
    pools[0].name = "重命名后的默认池".into();
    pools[0].max_concurrent_requests = Some(3);
    let renamed = store
        .set_intruder_resource_pools(
            project(&store),
            catalog.revision,
            pools,
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(renamed.default_pool_id, Some(id));
    assert_eq!(
        store
            .ensure_intruder_default_pool(project(&store), &CancellationToken::new())
            .unwrap(),
        renamed
    );
    assert!(
        store
            .set_intruder_resource_pools(
                project(&store),
                renamed.revision,
                vec![],
                &CancellationToken::new()
            )
            .is_err()
    );
    let input = spec(
        Some(IntruderPoolSelection::Create(renamed.pools[0].clone())),
        true,
    );
    let tab = bind(&mut store, &input);
    let run_id = Uuid::new_v4();
    assert!(store.create_intruder_run(run_id, tab, &input, 1).is_err());
    assert!(store.find_intruder_run(run_id).unwrap().is_none());
    assert_eq!(store.intruder_resource_pools().unwrap(), renamed);
}

#[test]
fn old_schema_fifteen_catalog_bytes_survive_open_and_the_default_counts_toward_sixty_four() {
    for count in [63, 64] {
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let path = directory.path().join("legacy-default.mio");
        let mut store = ProjectStore::create(Some(&path)).unwrap();
        let original = store
            .set_intruder_resource_pools(
                project(&store),
                0,
                (0..count).map(|_| pool()).collect(),
                &CancellationToken::new(),
            )
            .unwrap();
        let mut json = serde_json::to_value(&original).unwrap();
        assert!(
            json.as_object_mut()
                .unwrap()
                .remove("default_pool_id")
                .is_some()
        );
        let legacy = serde_json::to_vec(&json).unwrap();
        store
            .connection
            .execute(
                "UPDATE metadata SET value=?1 WHERE key=?2",
                params![legacy, KEY],
            )
            .unwrap();
        store.save(None).unwrap();
        store.close().unwrap();
        let mut reopened = ProjectStore::open(&path).unwrap();
        assert_eq!(
            reopened
                .connection
                .pragma_query_value::<i32, _>(None, "user_version", |row| row.get(0))
                .unwrap(),
            super::super::super::SCHEMA
        );
        assert_eq!(reopened.intruder_resource_pools().unwrap(), original);
        assert_eq!(catalog_bytes(&reopened), Some(legacy.clone()));
        assert!(
            reopened
                .intruder_resource_pools()
                .unwrap()
                .default_pool_id
                .is_none()
        );
        let result =
            reopened.ensure_intruder_default_pool(project(&reopened), &CancellationToken::new());
        if count == 64 {
            assert!(result.is_err());
            assert_eq!(reopened.intruder_resource_pools().unwrap(), original);
            assert_eq!(catalog_bytes(&reopened), Some(legacy));
        } else {
            let established = result.unwrap();
            assert_eq!(established.pools.len(), 64);
            assert_eq!(&established.pools[1..], original.pools.as_slice());
            assert!(established.default_pool_id.is_some());
            assert_eq!(established.revision, original.revision + 1);
        }
        reopened.close().unwrap();
    }
}

#[test]
fn default_pool_cancel_and_commit_failure_leave_no_catalog_or_identity_changes() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    for populated in [false, true] {
        for failure in ["cancel", "during_write", "commit", "project"] {
            let mut store = ProjectStore::create(None).unwrap();
            if populated {
                store
                    .set_intruder_resource_pools(
                        project(&store),
                        0,
                        vec![pool()],
                        &CancellationToken::new(),
                    )
                    .unwrap();
            }
            let before = store.intruder_resource_pools().unwrap();
            let bytes = catalog_bytes(&store);
            let cancel = CancellationToken::new();
            let observed = Arc::new(AtomicBool::new(false));
            let mut owner = project(&store);
            match failure {
                "cancel" => cancel.cancel(),
                "commit" => {
                    store.connection.commit_hook(Some(|| true));
                }
                "project" => owner.id = Uuid::new_v4(),
                "during_write" => {
                    let token = cancel.clone();
                    let wrote = Arc::clone(&observed);
                    store
                        .connection
                        .authorizer(Some(move |context: AuthContext<'_>| {
                            if matches!(
                                context.action,
                                AuthAction::Insert {
                                    table_name: "metadata",
                                    ..
                                } | AuthAction::Update {
                                    table_name: "metadata",
                                    column_name: "value",
                                    ..
                                }
                            ) {
                                wrote.store(true, Ordering::SeqCst);
                                token.cancel();
                            }
                            Authorization::Allow
                        }));
                }
                _ => unreachable!(),
            }
            assert!(
                store.ensure_intruder_default_pool(owner, &cancel).is_err(),
                "{populated}/{failure}"
            );
            store.connection.commit_hook(None::<fn() -> bool>);
            store
                .connection
                .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
            if failure == "during_write" {
                assert!(observed.load(Ordering::SeqCst));
            }
            assert_eq!(store.intruder_resource_pools().unwrap(), before);
            assert_eq!(catalog_bytes(&store), bytes);
        }
    }
}

#[test]
fn default_pool_creation_preserves_foreign_key_dependents_and_allows_unrelated_extensions() {
    let mut independent = ProjectStore::create(None).unwrap();
    independent
        .set_intruder_resource_pools(
            project(&independent),
            0,
            vec![pool()],
            &CancellationToken::new(),
        )
        .unwrap();
    independent.connection.execute_batch("CREATE TABLE extension_keys(key TEXT REFERENCES metadata(key) ON UPDATE CASCADE); INSERT INTO extension_keys VALUES('intruder_resource_pools');").unwrap();
    assert!(
        independent
            .ensure_intruder_default_pool(project(&independent), &CancellationToken::new())
            .unwrap()
            .default_pool_id
            .is_some()
    );
    assert_eq!(
        independent
            .connection
            .query_row::<String, _, _>("SELECT key FROM extension_keys", [], |row| row.get(0))
            .unwrap(),
        KEY
    );
}

#[test]
fn default_pool_encoding_limit_refuses_growth_without_discarding_existing_names() {
    let mut store = ProjectStore::create(None).unwrap();
    let mut pools: Vec<_> = (0..22)
        .map(|_| {
            let mut pool = pool();
            pool.name = "\u{1}".repeat(4096);
            pool
        })
        .collect();
    let (mut low, mut high) = (1, 4096);
    while low < high {
        let middle = low + (high - low) / 2 + 1;
        pools.last_mut().unwrap().name = "\u{1}".repeat(middle);
        let candidate = IntruderResourcePools {
            revision: 1,
            pools: pools.clone(),
            default_pool_id: None,
        };
        if serde_json::to_vec(&candidate).unwrap().len() <= MAX_INTRUDER_RESOURCE_POOLS_BYTES {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    pools.last_mut().unwrap().name = "\u{1}".repeat(low);
    let before = store
        .set_intruder_resource_pools(project(&store), 0, pools, &CancellationToken::new())
        .unwrap();
    let bytes = catalog_bytes(&store).unwrap();
    assert!(
        bytes.len() <= MAX_INTRUDER_RESOURCE_POOLS_BYTES
            && bytes.len() > MAX_INTRUDER_RESOURCE_POOLS_BYTES - 6
    );
    assert!(
        store
            .ensure_intruder_default_pool(project(&store), &CancellationToken::new())
            .is_err()
    );
    assert_eq!(store.intruder_resource_pools().unwrap(), before);
    assert_eq!(catalog_bytes(&store), Some(bytes));
}
