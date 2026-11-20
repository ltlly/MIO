use super::*;

#[test]
fn schema_thirteen_delay_defaults_preserve_catalog_and_open_closed_frozen_create_bytes() {
    assert_legacy_pool_settings_migration(13, &["delay", "automatic_throttling"], [None; 3]);
}

#[test]
fn schema_fourteen_auto_defaults_preserve_catalog_and_open_closed_frozen_create_bytes() {
    assert_legacy_pool_settings_migration(
        14,
        &["automatic_throttling"],
        [
            None,
            Some(IntruderPoolDelay::Fixed { milliseconds: 500 }),
            Some(IntruderPoolDelay::Increment {
                initial_ms: 0,
                increment_ms: 300_000,
            }),
        ],
    );
}

#[test]
fn schema_sixteen_pool_settings_keep_default_identity_bytes_and_sql_objects() {
    assert_legacy_pool_settings_migration(
        16,
        &[],
        [
            None,
            Some(IntruderPoolDelay::Fixed { milliseconds: 500 }),
            Some(IntruderPoolDelay::Increment {
                initial_ms: 0,
                increment_ms: 300_000,
            }),
        ],
    );
}

#[test]
fn schema_seventeen_pool_inputs_migration_preserves_catalog_workspace_and_frozen_bytes() {
    assert_legacy_pool_settings_migration(
        17,
        &[],
        [
            None,
            Some(IntruderPoolDelay::Random { milliseconds: 500 }),
            Some(IntruderPoolDelay::Increment {
                initial_ms: 0,
                increment_ms: 300_000,
            }),
        ],
    );
}

fn assert_legacy_pool_settings_migration(
    version: i32,
    omitted: &[&str],
    delays: [Option<IntruderPoolDelay>; 3],
) {
    fn schema_objects(connection: &Connection) -> Vec<(String, String, Option<String>)> {
        connection
            .prepare("SELECT type,name,sql FROM sqlite_schema ORDER BY type,name")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let path = directory.path().join("legacy-delay.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let mut runs = Vec::new();
    for (cap, delay) in [None, Some(1), Some(16)].into_iter().zip(delays) {
        let mut original_pool = pool();
        original_pool.max_concurrent_requests = cap;
        original_pool.delay = delay;
        if version >= 15 {
            original_pool.automatic_throttling = IntruderAutoThrottle {
                enabled: true,
                status_429: false,
                status_503: true,
                other_enabled: true,
                other_codes: " 504,429, 504 ".into(),
            };
        }
        let input = spec(Some(IntruderPoolSelection::Create(original_pool)), true);
        let tab = bind(&mut store, &input);
        let run = store
            .create_intruder_run(Uuid::new_v4(), tab, &input, 1)
            .unwrap();
        runs.push((run, input));
    }
    let mut workspace = store.workspace().unwrap();
    workspace.revision += 1;
    let closed = workspace.intruder.tabs.pop().unwrap();
    workspace.intruder.closed.push(closed);
    workspace.intruder.active = Some(workspace.intruder.tabs[0].id);
    store.save_workspace(&workspace).unwrap();
    if version >= 16 {
        store
            .ensure_intruder_default_pool(project(&store), &CancellationToken::new())
            .unwrap();
    }
    let catalog = store.intruder_resource_pools().unwrap();

    let mut catalog_json = serde_json::to_value(&catalog).unwrap();
    if version < 16 {
        assert!(
            catalog_json
                .as_object_mut()
                .unwrap()
                .remove("default_pool_id")
                .is_some()
        );
    } else {
        assert!(catalog.default_pool_id.is_some());
    }
    for pool in catalog_json["pools"].as_array_mut().unwrap() {
        for field in omitted {
            assert!(pool.as_object_mut().unwrap().remove(*field).is_some());
        }
    }
    let mut workspace_json = serde_json::to_value(&workspace).unwrap();
    for list in ["tabs", "closed"] {
        for tab in workspace_json["intruder"][list].as_array_mut().unwrap() {
            for field in omitted {
                assert!(
                    tab["spec"]["resource_pool"]["Create"]
                        .as_object_mut()
                        .unwrap()
                        .remove(*field)
                        .is_some()
                );
            }
        }
    }
    let legacy_metadata = [
        (KEY, serde_json::to_vec(&catalog_json).unwrap()),
        ("workspace", serde_json::to_vec(&workspace_json).unwrap()),
    ];
    for (key, bytes) in &legacy_metadata {
        store
            .connection
            .execute(
                "UPDATE metadata SET value=?1 WHERE key=?2",
                params![bytes, key],
            )
            .unwrap();
    }
    let mut legacy_specs = Vec::new();
    for (run, _) in &runs {
        let bytes: Vec<u8> = store
            .connection
            .query_row(
                "SELECT spec_meta FROM intruder_runs WHERE id=?1",
                [run.id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        let mut json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for field in omitted {
            assert!(
                json["resource_pool"]["Create"]
                    .as_object_mut()
                    .unwrap()
                    .remove(*field)
                    .is_some()
            );
        }
        let legacy = serde_json::to_vec(&json).unwrap();
        store
            .connection
            .execute(
                "UPDATE intruder_runs SET spec_meta=?1 WHERE id=?2",
                params![legacy, run.id.to_string()],
            )
            .unwrap();
        legacy_specs.push((run.id, legacy));
    }
    let original_schema = schema_objects(&store.connection);
    store
        .connection
        .execute_batch("DROP TABLE site_map_entries; DROP INDEX flows_source_started; DROP INDEX flows_source_status; DROP INDEX flows_source_capture; ALTER TABLE flows DROP COLUMN response_length; ALTER TABLE flows DROP COLUMN capture_number; ALTER TABLE flows DROP COLUMN elapsed_ms; ALTER TABLE flows DROP COLUMN response_bytes; ALTER TABLE flows DROP COLUMN status; ALTER TABLE flows DROP COLUMN source; ALTER TABLE flows DROP COLUMN listener_port; ALTER TABLE intruder_results DROP COLUMN highlight_revision; ALTER TABLE intruder_results DROP COLUMN highlight; ALTER TABLE intruder_results DROP COLUMN notes_revision; ALTER TABLE intruder_results DROP COLUMN notes; DROP TABLE ext_http_highlights; DROP TABLE websocket_originals; DROP TABLE ext_http_notes; DROP TABLE repeater_history; DROP TABLE websocket_notes; DROP TABLE websocket_connections;")
        .unwrap();
    store
        .connection
        .pragma_update(None, "user_version", version)
        .unwrap();
    store.checkpoint().unwrap();
    store
        .connection
        .execute_batch("PRAGMA journal_mode=DELETE;")
        .unwrap();
    store.close().unwrap();

    let mut reopened = ProjectStore::open(&path).unwrap();
    assert_eq!(
        reopened
            .connection
            .pragma_query_value::<i32, _>(None, "user_version", |row| row.get(0))
            .unwrap(),
        crate::store::SCHEMA
    );
    assert_eq!(schema_objects(&reopened.connection), original_schema);
    assert_eq!(reopened.intruder_resource_pools().unwrap(), catalog);
    assert_eq!(
        reopened
            .intruder_resource_pools()
            .unwrap()
            .pools
            .iter()
            .filter(|pool| Some(pool.id) != catalog.default_pool_id)
            .map(|pool| pool.delay)
            .collect::<Vec<_>>(),
        delays
    );
    if version < 15 {
        for pool in &reopened.intruder_resource_pools().unwrap().pools {
            assert_eq!(pool.automatic_throttling, IntruderAutoThrottle::default());
            assert!(!pool.automatic_throttling.enabled);
            assert!(pool.automatic_throttling.status_429);
            assert!(
                !pool.automatic_throttling.status_503 && !pool.automatic_throttling.other_enabled
            );
            assert!(pool.automatic_throttling.other_codes.is_empty());
        }
    }
    assert_eq!(reopened.workspace().unwrap(), workspace);
    for (run, input) in &runs {
        assert_eq!(reopened.read_intruder_run(run.id).unwrap(), *run);
        assert_eq!(reopened.read_intruder_spec(run.id).unwrap(), *input);
    }
    reopened.save(None).unwrap();
    for (key, expected) in &legacy_metadata {
        let actual: Vec<u8> = reopened
            .connection
            .query_row("SELECT value FROM metadata WHERE key=?1", [key], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            &actual, expected,
            "opening and ordinary Save must not normalize legacy {key}"
        );
    }
    for (id, expected) in &legacy_specs {
        let actual: Vec<u8> = reopened
            .connection
            .query_row(
                "SELECT spec_meta FROM intruder_runs WHERE id=?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(&actual, expected);
    }
    reopened.close().unwrap();
}

#[test]
fn pool_fixed_and_increment_delay_save_as_preserves_current_and_frozen_settings() {
    let mut store = ProjectStore::create(None).unwrap();
    let mut snapshots = Vec::new();
    for delay in [
        IntruderPoolDelay::Fixed { milliseconds: 0 },
        IntruderPoolDelay::Increment {
            initial_ms: 0,
            increment_ms: 300_000,
        },
    ] {
        let mut configured = pool();
        configured.delay = Some(delay);
        let input = spec(Some(IntruderPoolSelection::Create(configured)), true);
        let tab = bind(&mut store, &input);
        let run = store
            .create_intruder_run(Uuid::new_v4(), tab, &input, 1)
            .unwrap();
        snapshots.push((run, input));
    }
    let catalog = store.intruder_resource_pools().unwrap();
    let mut invalid = catalog.pools.clone();
    invalid[1].delay = Some(IntruderPoolDelay::Increment {
        initial_ms: 0,
        increment_ms: 0,
    });
    assert!(
        store
            .set_intruder_resource_pools(
                project(&store),
                catalog.revision,
                invalid,
                &CancellationToken::new()
            )
            .is_err()
    );
    assert_eq!(store.intruder_resource_pools().unwrap(), catalog);
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("delay.mio");
    let copy = directory.path().join("delay-copy.mio");
    store.save(Some(&original)).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    for path in [&original, &copy] {
        let restored = ProjectStore::open(path).unwrap();
        assert_eq!(restored.intruder_resource_pools().unwrap(), catalog);
        for (run, input) in &snapshots {
            assert_eq!(restored.read_intruder_run(run.id).unwrap(), *run);
            assert_eq!(restored.read_intruder_spec(run.id).unwrap(), *input);
        }
        restored.close().unwrap();
    }
}

#[test]
fn automatic_throttle_fields_and_verbatim_csv_survive_save_as_and_rejected_edits() {
    let policies = [
        IntruderAutoThrottle {
            enabled: true,
            status_429: false,
            status_503: true,
            other_enabled: true,
            other_codes: " 504, 429,504 , 599, 100 ".into(),
        },
        IntruderAutoThrottle {
            enabled: false,
            status_429: true,
            status_503: true,
            other_enabled: true,
            other_codes: "not parsed while automatic throttle is off".into(),
        },
        IntruderAutoThrottle {
            enabled: true,
            status_429: true,
            status_503: false,
            other_enabled: false,
            other_codes: "malformed, retained while Other is off".into(),
        },
    ];
    let mut store = ProjectStore::create(None).unwrap();
    let mut snapshots = Vec::new();
    for policy in policies {
        assert!(policy.response_codes().is_ok());
        let mut configured = pool();
        configured.automatic_throttling = policy;
        let input = spec(Some(IntruderPoolSelection::Create(configured)), true);
        let tab = bind(&mut store, &input);
        let run = store
            .create_intruder_run(Uuid::new_v4(), tab, &input, 1)
            .unwrap();
        snapshots.push((run, input));
    }
    let catalog = store.intruder_resource_pools().unwrap();
    let active_codes: BTreeSet<_> = catalog.pools[0]
        .automatic_throttling
        .response_codes()
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(active_codes, BTreeSet::from([100, 429, 503, 504, 599]));
    let original_bytes: Vec<u8> = store
        .connection
        .query_row("SELECT value FROM metadata WHERE key=?1", [KEY], |row| {
            row.get(0)
        })
        .unwrap();
    for enabled in [true, false] {
        let mut invalid = catalog.pools.clone();
        invalid[0].automatic_throttling.enabled = enabled;
        invalid[0].automatic_throttling.other_enabled = true;
        invalid[0].automatic_throttling.other_codes = if enabled {
            "429,700".into()
        } else {
            "x".repeat(4097)
        };
        assert!(
            store
                .set_intruder_resource_pools(
                    project(&store),
                    catalog.revision,
                    invalid,
                    &CancellationToken::new()
                )
                .is_err()
        );
        assert_eq!(store.intruder_resource_pools().unwrap(), catalog);
        let after: Vec<u8> = store
            .connection
            .query_row("SELECT value FROM metadata WHERE key=?1", [KEY], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(after, original_bytes);
    }
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let original = directory.path().join("automatic-throttle.mio");
    let copy = directory.path().join("automatic-throttle-copy.mio");
    store.save(Some(&original)).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    for path in [&original, &copy] {
        let restored = ProjectStore::open(path).unwrap();
        assert_eq!(restored.intruder_resource_pools().unwrap(), catalog);
        let actual: Vec<u8> = restored
            .connection
            .query_row("SELECT value FROM metadata WHERE key=?1", [KEY], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(actual, original_bytes);
        for (run, input) in &snapshots {
            assert_eq!(restored.read_intruder_run(run.id).unwrap(), *run);
            assert_eq!(restored.read_intruder_spec(run.id).unwrap(), *input);
        }
        restored.close().unwrap();
    }
}
