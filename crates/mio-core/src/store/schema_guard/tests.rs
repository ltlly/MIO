mod migrations;

use std::{collections::BTreeMap, fs, path::Path};

use rusqlite::{Connection, OpenFlags, params};

use crate::{FlowRecord, Uuid, Workspace, store::ProjectStore};

use super::super::{SCHEMA, tests::record as flow_record};

const REPEATER_HISTORY_DDL: &str = "CREATE TABLE repeater_history (tab TEXT NOT NULL, archive_boundary INTEGER NOT NULL CHECK(archive_boundary>=0), ordinal INTEGER NOT NULL CHECK(ordinal>=0), flow TEXT NOT NULL UNIQUE REFERENCES flows(id) ON DELETE RESTRICT, draft_meta BLOB NOT NULL CHECK(typeof(draft_meta)='blob' AND length(draft_meta)<=524288), raw BLOB NOT NULL CHECK(typeof(raw)='blob' AND length(raw)<=17039360), PRIMARY KEY(tab,archive_boundary,ordinal)) WITHOUT ROWID;";

fn fixture(path: &Path, schema: i32) -> (Uuid, FlowRecord, Workspace) {
    assert!((1..=SCHEMA).contains(&schema));
    let mut store = ProjectStore::create(Some(path)).unwrap();
    if schema < 26 {
        store.connection.execute_batch("DROP TABLE site_map_entries;").unwrap();
    }
    let flow = flow_record();
    store.put(&flow).unwrap();
    let mut workspace = Workspace::default();
    workspace.open_request(Some(&flow.request)).unwrap();
    store.save_workspace(&workspace).unwrap();
    if schema < 27 {
        store
            .connection
            .execute_batch(
                "DROP INDEX flows_source_started;
                 DROP INDEX flows_source_status;
                 DROP INDEX flows_source_capture;
                 ALTER TABLE flows DROP COLUMN response_length;
                 ALTER TABLE flows DROP COLUMN capture_number;
                 ALTER TABLE flows DROP COLUMN elapsed_ms;
                 ALTER TABLE flows DROP COLUMN response_bytes;
                 ALTER TABLE flows DROP COLUMN status;
                 ALTER TABLE flows DROP COLUMN source;",
            )
            .unwrap();
    }
    if schema < 25 {
        store
            .connection
            .execute_batch("ALTER TABLE flows DROP COLUMN listener_port;")
            .unwrap();
    }
    if schema < 23 {
        store
            .connection
            .execute_batch("DROP TABLE ext_http_highlights;")
            .unwrap();
    }
    if schema < 24 {
        store
            .connection
            .execute_batch("DROP TABLE websocket_originals;")
            .unwrap();
    }
    if schema < 22 {
        store
            .connection
            .execute_batch("DROP TABLE ext_http_notes;")
            .unwrap();
    }
    if schema < 21 {
        store
            .connection
            .execute_batch("DROP TABLE repeater_history;")
            .unwrap();
    }
    if schema < 8 {
        let mut legacy = serde_json::to_value(&workspace).unwrap();
        legacy.as_object_mut().unwrap().remove("repeater_settings");
        if schema < 6 {
            legacy.as_object_mut().unwrap().remove("intruder");
        }
        store
            .connection
            .execute(
                "UPDATE metadata SET value=?1 WHERE key='workspace'",
                [serde_json::to_vec(&legacy).unwrap()],
            )
            .unwrap();
    }
    if schema < 4 {
        store
            .connection
            .execute_batch(
                "DROP TABLE intruder_results; DROP TABLE intruder_payloads;
                 DROP TABLE intruder_runs;",
            )
            .unwrap();
    } else if schema < 7 {
        store
            .connection
            .execute_batch("ALTER TABLE intruder_runs DROP COLUMN paused;")
            .unwrap();
    }
    if (4..10).contains(&schema) {
        store
            .connection
            .execute_batch(
                "ALTER TABLE intruder_runs DROP COLUMN title;
             ALTER TABLE intruder_runs DROP COLUMN title_revision;",
            )
            .unwrap();
    }
    if (4..12).contains(&schema) {
        store
            .connection
            .execute_batch("ALTER TABLE intruder_runs DROP COLUMN baseline_ordinal;")
            .unwrap();
    }
    if (4..13).contains(&schema) {
        store.connection.execute_batch("ALTER TABLE intruder_runs DROP COLUMN pool_revision; ALTER TABLE intruder_runs DROP COLUMN pool_id;").unwrap();
    }
    if (4..25).contains(&schema) {
        store
            .connection
            .execute_batch(
                "ALTER TABLE intruder_results DROP COLUMN highlight_revision;
                 ALTER TABLE intruder_results DROP COLUMN highlight;
                 ALTER TABLE intruder_results DROP COLUMN notes_revision;
                 ALTER TABLE intruder_results DROP COLUMN notes;",
            )
            .unwrap();
    }
    if schema < 3 {
        store
            .connection
            .execute_batch(
                "DROP TABLE ext_messages; DROP TABLE ext_items;
                 DROP TABLE ext_source_chunks; DROP TABLE ext_sources;",
            )
            .unwrap();
    }
    if schema < 20 {
        store
            .connection
            .execute_batch("DROP TABLE websocket_notes;")
            .unwrap();
    }
    if schema < 2 {
        store
            .connection
            .execute_batch("DROP TABLE websocket_payloads; DROP TABLE websocket_messages;")
            .unwrap();
    }
    if schema < 19 {
        store
            .connection
            .execute_batch("DROP TABLE websocket_connections;")
            .unwrap();
    }
    store
        .connection
        .pragma_update(None, "user_version", schema)
        .unwrap();
    store.checkpoint().unwrap();
    store
        .connection
        .execute_batch("PRAGMA journal_mode=DELETE;")
        .unwrap();
    let id = store.id;
    store.close().unwrap();
    (id, flow, workspace)
}

fn source_files(directory: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

fn assert_readonly_rejection(path: &Path) -> String {
    let directory = path.parent().unwrap();
    let before = source_files(directory);
    let error = ProjectStore::open(path)
        .err()
        .map(|error| error.to_string());
    let rejected = error.is_some();
    let source_unchanged = source_files(directory) == before;
    assert!(
        rejected && source_unchanged,
        "untrusted project schema must be rejected read-only: rejected={rejected}, source_unchanged={source_unchanged}"
    );
    error.unwrap()
}

#[test]
fn known_data_views_are_rejected_readonly_before_product_queries() {
    for table in ["flows", "ext_source_chunks"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("known-view.mio");
        fixture(&path, SCHEMA);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(&format!(
                "ALTER TABLE {table} RENAME TO stored_{table};
                 CREATE VIEW {table} AS SELECT * FROM stored_{table};"
            ))
            .unwrap();
        connection.close().unwrap();

        assert_readonly_rejection(&path);
    }
}

#[test]
fn known_virtual_table_is_rejected_readonly() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("known-virtual.mio");
    fixture(&path, SCHEMA);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "ALTER TABLE flows RENAME TO stored_flows;
             CREATE VIRTUAL TABLE flows USING fts5(id,revision,started,url,method,state,summary);",
        )
        .unwrap();
    connection.close().unwrap();

    assert_readonly_rejection(&path);
}

#[test]
fn generated_product_column_is_rejected_even_when_its_current_value_is_valid() {
    for storage in ["VIRTUAL", "STORED"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("generated-summary.mio");
        let (_, flow, _) = fixture(&path, SCHEMA);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(&format!(
                "ALTER TABLE flows RENAME TO stored_flows;
                 CREATE TABLE flows (
                     id TEXT PRIMARY KEY,revision INTEGER NOT NULL,started INTEGER NOT NULL,
                     url TEXT NOT NULL,method TEXT NOT NULL,state TEXT NOT NULL,
                     summary BLOB GENERATED ALWAYS AS (stored_summary) {storage},
                     stored_summary BLOB NOT NULL);
                 INSERT INTO flows(id,revision,started,url,method,state,stored_summary)
                     SELECT id,revision,started,url,method,state,summary FROM stored_flows;"
            ))
            .unwrap();
        let generated: Vec<u8> = connection
            .query_row(
                "SELECT summary FROM flows WHERE id=?1",
                [flow.summary.id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generated, serde_json::to_vec(&flow.summary).unwrap());
        connection.close().unwrap();

        assert_readonly_rejection(&path);
    }
}

#[test]
fn workspace_save_trigger_that_can_tamper_with_flows_is_rejected_readonly() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workspace-trigger.mio");
    let (_, flow, _) = fixture(&path, SCHEMA);
    let connection = Connection::open(&path).unwrap();
    connection
        .pragma_update(None, "trusted_schema", false)
        .unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER tamper_with_flow AFTER UPDATE OF value ON metadata
             WHEN NEW.key='workspace'
             BEGIN UPDATE flows SET method='TRIGGERED'; END;",
        )
        .unwrap();
    // Prove this is an active data-changing trigger, not merely an unused schema name.
    // Roll back the demonstration before taking the complete on-disk source snapshot.
    let transaction = connection.unchecked_transaction().unwrap();
    transaction
        .execute("UPDATE metadata SET value=value WHERE key='workspace'", [])
        .unwrap();
    let tampered: String = transaction
        .query_row(
            "SELECT method FROM flows WHERE id=?1",
            [flow.summary.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tampered, "TRIGGERED");
    transaction.rollback().unwrap();
    let original: String = connection
        .query_row(
            "SELECT method FROM flows WHERE id=?1",
            [flow.summary.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(original, flow.summary.method);
    connection.close().unwrap();

    assert_readonly_rejection(&path);
}

#[test]
fn missing_required_table_or_stored_column_is_rejected_readonly() {
    for (mutation, expected_error) in [
        (
            "ALTER TABLE ext_source_chunks RENAME TO retained_chunks;",
            "项目缺少必需数据表 ext_source_chunks",
        ),
        (
            "ALTER TABLE flows RENAME COLUMN summary TO retained_summary;",
            "项目数据表 flows 缺少必需列",
        ),
        (
            "ALTER TABLE ext_http_highlights RENAME TO retained_highlights;",
            "项目缺少必需数据表 ext_http_highlights",
        ),
        (
            "ALTER TABLE ext_http_highlights RENAME COLUMN color TO retained_color;",
            "项目数据表 ext_http_highlights 缺少必需列",
        ),
        (
            "ALTER TABLE websocket_originals RENAME TO retained_originals;",
            "项目缺少必需数据表 websocket_originals",
        ),
        (
            "ALTER TABLE websocket_originals RENAME COLUMN body TO retained_body;",
            "项目数据表 websocket_originals 缺少必需列",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing-required-schema.mio");
        fixture(&path, SCHEMA);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(mutation).unwrap();
        connection.close().unwrap();

        assert_eq!(assert_readonly_rejection(&path), expected_error);
    }
}

#[test]
fn future_migration_target_tables_are_rejected_without_overwriting_their_data() {
    for (schema, table) in [
        (1, "websocket_payloads"),
        (2, "ext_items"),
        (3, "intruder_runs"),
        (23, "websocket_originals"),
        (22, "ext_http_highlights"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("occupied-migration-target.mio");
        fixture(&path, schema);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(&format!("CREATE TABLE {table}(retained BLOB NOT NULL);"))
            .unwrap();
        let original = [0u8, 255, 42];
        connection
            .execute(
                &format!("INSERT INTO {table} VALUES (?1)"),
                params![original.as_slice()],
            )
            .unwrap();
        connection.close().unwrap();

        assert_eq!(
            assert_readonly_rejection(&path),
            format!("项目迁移目标表 {table} 已存在；不会覆盖已有数据")
        );

        let read = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let retained: Vec<u8> = read
            .query_row(&format!("SELECT retained FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(retained, original);
        let unchanged_version: i32 = read
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(unchanged_version, schema);
    }
}

#[test]
fn highlight_table_unknown_columns_or_foreign_keys_block_discard_before_writes() {
    for (mutation, expected_error) in [
        ("", None),
        (
            "ALTER TABLE ext_http_highlights ADD COLUMN retained BLOB;",
            Some("项目包含未知或计算列；不能确认 Intruder 数据归属"),
        ),
        (
            "DROP TABLE ext_http_highlights;
             CREATE TABLE ext_http_highlights (
                item TEXT PRIMARY KEY NOT NULL REFERENCES flows(id) ON DELETE CASCADE,
                revision INTEGER NOT NULL CHECK(typeof(revision)='integer' AND revision>0),
                color INTEGER NOT NULL CHECK(typeof(color)='integer' AND color BETWEEN 0 AND 9)
             ) WITHOUT ROWID;",
            Some("项目包含未知外键；不能确认 Intruder 数据归属"),
        ),
    ] {
        let mut store = ProjectStore::create(None).unwrap();
        store.connection.execute_batch(mutation).unwrap();
        let changes = store
            .connection
            .query_row::<u64, _, _>("SELECT total_changes()", [], |row| row.get(0))
            .unwrap();
        let transaction = store.connection.transaction().unwrap();
        let result = crate::store::schema_guard::validate_intruder_discard(&transaction);
        match expected_error {
            Some(message) => assert_eq!(result.unwrap_err().to_string(), message),
            None => result.unwrap(),
        }
        transaction.rollback().unwrap();
        assert_eq!(
            store
                .connection
                .query_row::<u64, _, _>("SELECT total_changes()", [], |row| row.get(0))
                .unwrap(),
            changes
        );
    }
}
