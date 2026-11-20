use super::*;

#[test]
fn pause_column_version_mismatches_are_rejected_without_changing_source_data() {
    for schema in [6, 7] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pause-column-conflict.mio");
        fixture(&path, schema);
        let connection = Connection::open(&path).unwrap();
        if schema == 6 {
            connection
                .execute_batch("ALTER TABLE intruder_runs ADD COLUMN paused BLOB;")
                .unwrap();
        } else {
            connection
                .execute_batch("ALTER TABLE intruder_runs DROP COLUMN paused;")
                .unwrap();
        }
        connection.close().unwrap();
        let error = assert_readonly_rejection(&path);
        assert!(error.contains("paused"));
    }
}

#[test]
fn baseline_column_version_mismatches_are_rejected_without_changing_source_data() {
    for schema in [11, 12] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("baseline-column-conflict.mio");
        fixture(&path, schema);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(if schema == 11 {
                "ALTER TABLE intruder_runs ADD COLUMN baseline_ordinal BLOB;"
            } else {
                "ALTER TABLE intruder_runs DROP COLUMN baseline_ordinal;"
            })
            .unwrap();
        connection.close().unwrap();
        assert!(assert_readonly_rejection(&path).contains("baseline_ordinal"));
    }
}

#[test]
fn version_seven_repeater_settings_migration_only_adds_known_intruder_columns() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("repeater-settings-v7.mio");
    let (id, flow, workspace) = fixture(&path, 7);
    let read = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let schema_before: Vec<(String, String, Option<String>)> = read
        .prepare("SELECT type,name,sql FROM sqlite_schema ORDER BY type,name")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let legacy: Vec<u8> = read
        .query_row(
            "SELECT value FROM metadata WHERE key='workspace'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        serde_json::from_slice::<serde_json::Value>(&legacy)
            .unwrap()
            .get("repeater_settings")
            .is_none()
    );
    read.close().unwrap();

    let mut expected_schema = schema_before;
    let run_table = expected_schema
        .iter_mut()
        .find(|(kind, name, _)| kind == "table" && name == "intruder_runs")
        .unwrap();
    let expected = Connection::open_in_memory().unwrap();
    expected
        .execute_batch(run_table.2.as_ref().unwrap())
        .unwrap();
    expected.execute_batch(
        "ALTER TABLE intruder_runs ADD COLUMN title TEXT NOT NULL DEFAULT ''\n    CHECK(typeof(title)='text' AND length(CAST(title AS BLOB))<=4096);
         ALTER TABLE intruder_runs ADD COLUMN title_revision INTEGER NOT NULL DEFAULT 0\n    CHECK(typeof(title_revision)='integer' AND title_revision>=0);"
    ).unwrap();
    crate::store::intruder::add_baseline_column(&expected).unwrap();
    crate::store::intruder::add_resource_pool_columns(&expected).unwrap();
    run_table.2 = expected
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name='intruder_runs'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let flows_table = expected_schema
        .iter_mut()
        .find(|(kind, name, _)| kind == "table" && name == "flows")
        .unwrap();
    expected
        .execute_batch(flows_table.2.as_ref().unwrap())
        .unwrap();
    expected
        .execute_batch(&format!(
            "ALTER TABLE flows ADD COLUMN {};",
            crate::store::FLOW_LISTENER_PORT_COLUMN
        ))
        .unwrap();
    crate::store::add_flow_denorm_columns(&expected).unwrap();
    flows_table.2 = expected
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name='flows'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    expected_schema.extend(
        expected
            .prepare("SELECT type,name,sql FROM sqlite_schema WHERE name IN ('flows_source_started','flows_source_status','flows_source_capture') ORDER BY type,name")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<(String, String, Option<String>)>>>()
            .unwrap(),
    );
    let results_table = expected_schema
        .iter_mut()
        .find(|(kind, name, _)| kind == "table" && name == "intruder_results")
        .unwrap();
    expected
        .execute_batch(results_table.2.as_ref().unwrap())
        .unwrap();
    crate::store::intruder::add_result_annotation_columns(&expected).unwrap();
    results_table.2 = expected
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name='intruder_results'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    expected
        .execute_batch(
            "CREATE TABLE websocket_connections (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            flow TEXT NOT NULL UNIQUE REFERENCES flows(id) ON DELETE CASCADE,
            listener_port INTEGER CHECK(listener_port BETWEEN 1 AND 65535)
        );",
        )
        .unwrap();
    let connections_schema = expected
        .prepare("SELECT type,name,sql FROM sqlite_schema WHERE name IN ('websocket_connections','sqlite_autoindex_websocket_connections_1') ORDER BY type,name")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<(String, String, Option<String>)>>>()
        .unwrap();
    assert_eq!(connections_schema.len(), 2);
    expected_schema.extend(connections_schema);
    expected
        .execute_batch(
            "CREATE TABLE websocket_notes (
            message TEXT PRIMARY KEY NOT NULL REFERENCES websocket_messages(id) ON DELETE CASCADE,
            revision INTEGER NOT NULL CHECK(typeof(revision)='integer' AND revision>0),
            text TEXT NOT NULL CHECK(typeof(text)='text' AND length(CAST(text AS BLOB))<=65536)
        ) WITHOUT ROWID;",
        )
        .unwrap();
    let notes_schema = expected
        .prepare("SELECT type,name,sql FROM sqlite_schema WHERE name='websocket_notes' ORDER BY type,name")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<(String, String, Option<String>)>>>()
        .unwrap();
    assert_eq!(notes_schema.len(), 1);
    expected_schema.extend(notes_schema);
    expected
        .execute_batch(
            "CREATE TABLE ext_http_notes (
            item TEXT PRIMARY KEY NOT NULL REFERENCES ext_items(id) ON DELETE CASCADE,
            revision INTEGER NOT NULL CHECK(typeof(revision)='integer' AND revision>0),
            text TEXT NOT NULL CHECK(typeof(text)='text' AND length(CAST(text AS BLOB))<=65536)
        ) WITHOUT ROWID;",
        )
        .unwrap();
    expected_schema.push(
        expected
            .query_row(
                "SELECT type,name,sql FROM sqlite_schema WHERE name='ext_http_notes'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap(),
    );
    expected
        .execute_batch(
            "CREATE TABLE ext_http_highlights (
            item TEXT PRIMARY KEY NOT NULL REFERENCES ext_items(id) ON DELETE CASCADE,
            revision INTEGER NOT NULL CHECK(typeof(revision)='integer' AND revision>0),
            color INTEGER NOT NULL CHECK(typeof(color)='integer' AND color BETWEEN 0 AND 9)
        ) WITHOUT ROWID;",
        )
        .unwrap();
    expected_schema.push(
        expected
            .query_row(
                "SELECT type,name,sql FROM sqlite_schema WHERE name='ext_http_highlights'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap(),
    );
    crate::store::websocket::create_originals(&expected).unwrap();
    expected_schema.push(
        expected
            .query_row(
                "SELECT type,name,sql FROM sqlite_schema WHERE name='websocket_originals'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap(),
    );
    expected.execute_batch(REPEATER_HISTORY_DDL).unwrap();
    let history_schema = expected
        .prepare("SELECT type,name,sql FROM sqlite_schema WHERE name IN ('repeater_history','sqlite_autoindex_repeater_history_1') ORDER BY type,name")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<(String, String, Option<String>)>>>()
        .unwrap();
    assert_eq!(history_schema.len(), 2);
    expected_schema.extend(history_schema);
    crate::store::target_site_map::create_schema(&expected).unwrap();
    expected_schema.extend(expected.prepare("SELECT type,name,sql FROM sqlite_schema WHERE name IN ('site_map_entries','sqlite_autoindex_site_map_entries_2') ORDER BY type,name").unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap()
        .collect::<rusqlite::Result<Vec<(String, String, Option<String>)>>>().unwrap());
    expected_schema.sort();
    expected.close().unwrap();

    let mut store = ProjectStore::open(&path).unwrap();
    assert_eq!(store.id, id);
    assert_eq!(store.read(flow.summary.id).unwrap(), flow);
    assert_eq!(store.workspace().unwrap(), workspace);
    assert!(
        store
            .workspace()
            .unwrap()
            .repeater_settings
            .unpack_compressed_responses
    );
    let schema_after: Vec<(String, String, Option<String>)> = store
        .connection
        .prepare("SELECT type,name,sql FROM sqlite_schema ORDER BY type,name")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        schema_after, expected_schema,
        "migration may add only constrained task-name, baseline and resource-pool columns plus WebSocket connections, notes, 外部归档 HTTP highlights and Repeater history tables and their unique indexes; all other schema objects remain identical"
    );
    let unchanged: Vec<u8> = store
        .connection
        .query_row(
            "SELECT value FROM metadata WHERE key='workspace'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unchanged, legacy);
    assert_eq!(
        store
            .connection
            .pragma_query_value::<i32, _>(None, "user_version", |row| row.get(0))
            .unwrap(),
        SCHEMA
    );
    let mut disabled = workspace;
    disabled.revision += 1;
    disabled.repeater_settings.unpack_compressed_responses = false;
    store.save_workspace(&disabled).unwrap();
    store.close().unwrap();
    let reopened = ProjectStore::open(&path).unwrap();
    assert_eq!(reopened.workspace().unwrap(), disabled);
    reopened.close().unwrap();
}

#[test]
fn legitimate_versions_migrate_and_preserve_unknown_ordinary_tables() {
    for schema in 1..=SCHEMA {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legitimate.mio");
        let (id, flow, workspace) = fixture(&path, schema);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("CREATE TABLE additional_notes(id TEXT PRIMARY KEY,data BLOB NOT NULL);")
            .unwrap();
        let note = [0u8, 255, 10];
        connection
            .execute(
                "INSERT INTO additional_notes VALUES ('retained',?1)",
                params![note.as_slice()],
            )
            .unwrap();
        connection.close().unwrap();

        let store = ProjectStore::open(&path).unwrap();

        assert_eq!(store.id, id);
        assert_eq!(store.workspace().unwrap(), workspace);
        assert_eq!(store.read(flow.summary.id).unwrap(), flow);
        assert!(store.ext_sources().unwrap().is_empty());
        assert_eq!(store.websocket_page(None, 0, 1, &[]).unwrap().0, 0);
        let upgraded: i32 = store
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(upgraded, SCHEMA);
        let retained: Vec<u8> = store
            .connection
            .query_row(
                "SELECT data FROM additional_notes WHERE id='retained'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, note);
        store.close().unwrap();
    }
}

#[test]
fn intruder_discard_accepts_history_but_rejects_changed_flow_deletion_and_order_keys() {
    for (from, to) in [
        ("ON DELETE RESTRICT", "ON DELETE CASCADE"),
        ("ON DELETE RESTRICT", "ON DELETE RESTRICT ON UPDATE CASCADE"),
        ("tab TEXT NOT NULL", "tab TEXT COLLATE NOCASE NOT NULL"),
        (
            "PRIMARY KEY(tab,archive_boundary,ordinal)",
            "PRIMARY KEY(tab,ordinal,archive_boundary)",
        ),
    ] {
        let store = ProjectStore::create(None).unwrap();
        let transaction = store.connection.unchecked_transaction().unwrap();
        crate::store::schema_guard::validate_intruder_discard(&transaction).unwrap();
        transaction.commit().unwrap();
        store
            .connection
            .execute_batch("DROP TABLE repeater_history;")
            .unwrap();
        store
            .connection
            .execute_batch(&REPEATER_HISTORY_DDL.replace(from, to))
            .unwrap();
        let before: String = store
            .connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name='repeater_history'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let transaction = store.connection.unchecked_transaction().unwrap();
        let rejected = crate::store::schema_guard::validate_intruder_discard(&transaction).is_err();
        transaction.rollback().unwrap();
        let after: String = store
            .connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name='repeater_history'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            after, before,
            "rejecting a history ownership mismatch must not repair source DDL"
        );
        store.close().unwrap();
        assert!(
            rejected,
            "unsafe Repeater history schema must block destructive cleanup: {to}"
        );
    }
}

#[test]
fn version_eight_migration_preserves_data_and_reserves_discard_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("intruder-v8.mio");
    let (id, flow, workspace) = fixture(&path, 8);
    let store = ProjectStore::open(&path).unwrap();
    assert_eq!(store.id, id);
    assert_eq!(store.workspace().unwrap(), workspace);
    assert_eq!(store.read(flow.summary.id).unwrap(), flow);
    assert_eq!(
        store
            .connection
            .pragma_query_value::<i32, _>(None, "user_version", |row| row.get(0))
            .unwrap(),
        SCHEMA
    );
    store.close().unwrap();

    let path = directory.path().join("occupied-discard-namespace.mio");
    fixture(&path, 8);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO metadata VALUES(?1,?2)",
            params![
                format!("intruder_discarded:{}", Uuid::new_v4()),
                [1_u8; 16].as_slice()
            ],
        )
        .unwrap();
    connection.close().unwrap();
    assert!(assert_readonly_rejection(&path).contains("丢弃记录"));
}
