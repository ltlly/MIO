use super::*;

#[test]
fn pool_catalog_edits_allow_referenced_pools_but_deletion_checks_closed_and_frozen_references() {
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
        let mut edited = first.clone();
        edited.name = "已改名".into();
        edited.max_concurrent_requests = Some(1);
        let catalog = store
            .set_intruder_resource_pools(
                project(&store),
                1,
                vec![edited, second.clone()],
                &CancellationToken::new(),
            )
            .unwrap();
        let mut workspace = store.workspace().unwrap();
        workspace.revision += 1;
        workspace
            .intruder
            .closed
            .push(workspace.intruder.tabs.remove(0));
        workspace.intruder.active = None;
        store.save_workspace(&workspace).unwrap();
        assert!(
            store
                .set_intruder_resource_pools(
                    project(&store),
                    catalog.revision,
                    vec![second.clone()],
                    &CancellationToken::new()
                )
                .is_err()
        );
        workspace.revision += 1;
        workspace.intruder.closed.clear();
        store.save_workspace(&workspace).unwrap();
        store
            .set_intruder_resource_pool(
                project(&store),
                run.id,
                1,
                second.id,
                &CancellationToken::new(),
            )
            .unwrap();
        assert!(
            store
                .set_intruder_resource_pools(
                    project(&store),
                    catalog.revision,
                    vec![second],
                    &CancellationToken::new()
                )
                .is_err(),
            "frozen Create must retain the original pool even after rebinding"
        );
        assert_eq!(store.intruder_resource_pools().unwrap(), catalog);
        assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
    }
}

#[test]
fn existing_pool_start_uses_a_confirmed_catalog_without_recreating_or_defaulting() {
    let mut store = ProjectStore::create(None).unwrap();
    let pool = pool();
    let catalog = store
        .set_intruder_resource_pools(
            project(&store),
            0,
            vec![pool.clone()],
            &CancellationToken::new(),
        )
        .unwrap();
    for id in [pool.id, Uuid::new_v4()] {
        let input = spec(Some(IntruderPoolSelection::Existing(id)), true);
        let tab = bind(&mut store, &input);
        let run_id = Uuid::new_v4();
        let result = store.create_intruder_run_with_pool_catalog(
            project(&store),
            run_id,
            tab,
            &input,
            1,
            &CancellationToken::new(),
        );
        if id == pool.id {
            let (run, returned_catalog) = result.unwrap();
            assert_eq!(
                (run.resource_pool, run.resource_pool_revision),
                (Some(id), 1)
            );
            assert_eq!(returned_catalog, catalog);
        } else {
            assert!(result.is_err());
            assert!(store.find_intruder_run(run_id).unwrap().is_none());
        }
        assert_eq!(store.intruder_resource_pools().unwrap(), catalog);
    }
}

#[test]
fn pool_catalog_unknown_fields_type_and_case_aliases_are_rejected_without_replacement() {
    for mutation in ["unknown", "type", "oversized"] {
        let mut store = ProjectStore::create(None).unwrap();
        let catalog = store
            .set_intruder_resource_pools(
                project(&store),
                0,
                vec![pool()],
                &CancellationToken::new(),
            )
            .unwrap();
        match mutation {
            "unknown" => {
                let mut json = serde_json::to_value(&catalog).unwrap();
                json["future_behavior"] = serde_json::json!(true);
                store
                    .connection
                    .execute(
                        "UPDATE metadata SET value=?1 WHERE key=?2",
                        params![serde_json::to_vec(&json).unwrap(), KEY],
                    )
                    .unwrap();
            }
            "type" => {
                store
                    .connection
                    .execute("UPDATE metadata SET value='invalid' WHERE key=?1", [KEY])
                    .unwrap();
            }
            "oversized" => {
                store
                    .connection
                    .execute(
                        "UPDATE metadata SET value=zeroblob(?1) WHERE key=?2",
                        params![MAX_INTRUDER_RESOURCE_POOLS_BYTES + 1, KEY],
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let before: (String, String, Vec<u8>) = store.connection.query_row("SELECT key,typeof(value),CAST(value AS BLOB) FROM metadata WHERE key=?1", [KEY], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
        assert!(store.intruder_resource_pools().is_err(), "{mutation}");
        assert!(
            store
                .set_intruder_resource_pools(project(&store), 1, vec![], &CancellationToken::new())
                .is_err(),
            "{mutation}"
        );
        let after: (String, String, Vec<u8>) = store.connection.query_row("SELECT key,typeof(value),CAST(value AS BLOB) FROM metadata WHERE key=?1", [KEY], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
        assert_eq!(before, after);
    }
}

#[test]
fn schema_one_preserves_pools_and_rejects_legacy_user_version_twelve_readonly() {
    for legacy_version in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy-pool.mio");
        let mut store = ProjectStore::create(Some(&path)).unwrap();
        let mut input = spec(None, true);
        input.max_concurrent_requests = 16;
        let tab = bind(&mut store, &input);
        let run = store
            .create_intruder_run(Uuid::new_v4(), tab, &input, 1)
            .unwrap();
        store.save(None).unwrap();
        if legacy_version {
            store
                .connection
                .pragma_update(None, "user_version", 12)
                .unwrap();
            store.checkpoint().unwrap();
            store
                .connection
                .execute_batch("PRAGMA journal_mode=DELETE;")
                .unwrap();
        }
        store.close().unwrap();
        let before = std::fs::read(&path).unwrap();
        let opened = ProjectStore::open(&path);
        if legacy_version {
            assert!(opened.is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
        } else {
            let restored = opened.unwrap();
            assert_eq!(restored.read_intruder_run(run.id).unwrap(), run);
            assert_eq!(restored.read_intruder_spec(run.id).unwrap(), input);
            assert_eq!(
                restored.intruder_resource_pools().unwrap(),
                IntruderResourcePools::default()
            );
            restored.close().unwrap();
        }
    }
}

#[test]
fn pool_catalog_update_preserves_inbound_metadata_values_and_allows_unrelated_extensions() {
    let mut independent = ProjectStore::create(None).unwrap();
    let mut input = pool();
    let initial = independent
        .set_intruder_resource_pools(
            project(&independent),
            0,
            vec![input.clone()],
            &CancellationToken::new(),
        )
        .unwrap();
    independent
        .connection
        .execute_batch(
            "CREATE TABLE extension_notes(id INTEGER PRIMARY KEY, payload BLOB NOT NULL);
         INSERT INTO extension_notes VALUES(1,X'FF002A');
         CREATE TABLE extension_keys(key TEXT REFERENCES metadata(key) ON UPDATE CASCADE);
         INSERT INTO extension_keys VALUES('intruder_resource_pools');",
        )
        .unwrap();
    input.name = "已编辑的池".into();
    let changed = independent
        .set_intruder_resource_pools(
            project(&independent),
            initial.revision,
            vec![input],
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(changed.revision, initial.revision + 1);
    assert_eq!(
        independent
            .connection
            .query_row::<Vec<u8>, _, _>(
                "SELECT payload FROM extension_notes WHERE id=1",
                [],
                |row| row.get(0)
            )
            .unwrap(),
        [255, 0, 42]
    );
    assert_eq!(
        independent
            .connection
            .query_row::<String, _, _>("SELECT key FROM extension_keys", [], |row| row.get(0))
            .unwrap(),
        KEY
    );
}
