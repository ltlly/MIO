mod migrations;

use std::{collections::BTreeMap, fs, path::Path};

use rusqlite::{Connection, OpenFlags, params};

use crate::{FlowRecord, Uuid, Workspace, store::ProjectStore};

use super::super::{SCHEMA, tests::record as flow_record};

fn fixture(path: &Path) -> (Uuid, FlowRecord, Workspace) {
    let mut store = ProjectStore::create(Some(path)).unwrap();
    let flow = flow_record();
    store.put(&flow).unwrap();
    let mut workspace = Workspace::default();
    workspace.open_request(Some(&flow.request)).unwrap();
    store.save_workspace(&workspace).unwrap();
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
        fixture(&path);
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
    fixture(&path);
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
        let (_, flow, _) = fixture(&path);
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
    let (_, flow, _) = fixture(&path);
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
        fixture(&path);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(mutation).unwrap();
        connection.close().unwrap();

        assert_eq!(assert_readonly_rejection(&path), expected_error);
    }
}

#[test]
fn unsupported_user_version_is_rejected_readonly_without_changing_data() {
    for schema in [0, SCHEMA + 1, 6, 27, i32::MAX] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("unsupported-version.mio");
        fixture(&path);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("CREATE TABLE retained_table(retained BLOB NOT NULL);")
            .unwrap();
        let original = [0u8, 255, 42];
        connection
            .execute(
                "INSERT INTO retained_table VALUES (?1)",
                params![original.as_slice()],
            )
            .unwrap();
        connection
            .pragma_update(None, "user_version", schema)
            .unwrap();
        connection.close().unwrap();

        assert_eq!(
            assert_readonly_rejection(&path),
            "不是受支持的 MIO 项目；未修改文件"
        );

        let read = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let retained: Vec<u8> = read
            .query_row("SELECT retained FROM retained_table", [], |row| row.get(0))
            .unwrap();
        assert_eq!(retained, original);
        let unchanged_version: i32 = read
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(unchanged_version, schema);
    }
}
