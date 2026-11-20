use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use rusqlite::{
    Connection, OpenFlags,
    hooks::{AuthAction, AuthContext, Authorization},
    params,
    types::Value,
};

use crate::{Uuid, Workspace, store::ProjectStore};

use super::super::{APPLICATION_ID, SCHEMA};
use super::{WORKSPACE_QUERY, project_id, queries::workspace_with_limit};

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

fn stored_workspace(store: &ProjectStore) -> Vec<u8> {
    store
        .connection
        .query_row(
            "SELECT value FROM main.metadata WHERE key='workspace'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn put_invalid_workspace(store: &ProjectStore) -> Vec<u8> {
    let invalid = Workspace {
        active: Some(Uuid::new_v4()),
        ..Workspace::default()
    };
    assert!(invalid.validate().is_err());
    let bytes = serde_json::to_vec(&invalid).unwrap();
    store
        .connection
        .execute(
            "UPDATE main.metadata SET value=?1 WHERE key='workspace'",
            params![&bytes],
        )
        .unwrap();
    bytes
}

#[test]
fn metadata_view_is_rejected_without_changing_source() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("metadata-view.mio");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(&format!(
            "PRAGMA application_id={APPLICATION_ID};
             PRAGMA user_version={SCHEMA};
             CREATE TABLE backing_metadata(key TEXT PRIMARY KEY, value BLOB NOT NULL);
             CREATE VIEW metadata AS SELECT key,value FROM backing_metadata;"
        ))
        .unwrap();
    connection
        .execute(
            "INSERT INTO backing_metadata VALUES ('id',?1),('workspace',?2)",
            params![
                Uuid::new_v4().to_string(),
                serde_json::to_vec(&Workspace::default()).unwrap()
            ],
        )
        .unwrap();
    drop(connection);
    let before = source_files(directory.path());

    let rejected = ProjectStore::open(&path).is_err();
    let source_unchanged = source_files(directory.path()) == before;

    assert!(
        rejected && source_unchanged,
        "metadata view must be rejected read-only: rejected={rejected}, source_unchanged={source_unchanged}"
    );
}

#[test]
fn unknown_workspace_fields_are_readable_but_cannot_be_rewritten() {
    for pointer in [
        "",
        "/repeater_settings",
        "/upstream_tls",
        "/intruder",
        "/groups/0",
        "/intruder/groups/0",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("unknown-workspace.mio");
        let mut store = ProjectStore::create(None).unwrap();
        store.save(Some(&path)).unwrap();
        store.close().unwrap();
        let connection = Connection::open(&path).unwrap();
        let mut workspace = serde_json::to_value(Workspace::default()).unwrap();
        if pointer == "/upstream_tls" {
            workspace["upstream_tls"] =
                serde_json::to_value(crate::UpstreamTlsSettings::default()).unwrap();
        }
        if let Some(groups) = pointer.strip_suffix("/0") {
            *workspace.pointer_mut(groups).unwrap() = serde_json::json!([{
                "id": Uuid::new_v4(),
                "title": "旧写入器不能丢弃分组扩展",
                "collapsed": false,
            }]);
        }
        workspace
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(
                if pointer.ends_with("/groups/0") {
                    "futureColor"
                } else {
                    "unmapped-field"
                }
                .into(),
                serde_json::json!("preserve this"),
            );
        let original = serde_json::to_vec(&workspace).unwrap();
        connection
            .execute(
                "UPDATE metadata SET value=?1 WHERE key='workspace'",
                [&original],
            )
            .unwrap();
        drop(connection);
        let before = source_files(directory.path());
        let mut store = ProjectStore::open(&path).unwrap();
        let mut proposal = store.workspace().unwrap();
        proposal.revision += 1;
        let result = store.save_workspace(&proposal);
        assert!(
            result.is_err(),
            "unknown workspace field was lost at {pointer}"
        );
        assert!(result.unwrap_err().to_string().contains("未识别字段"));
        assert_eq!(stored_workspace(&store), original);
        let copy_directory = tempfile::tempdir().unwrap();
        let copy = copy_directory.path().join("retained.mio");
        let rejected = store.save_with(
            Some(&copy),
            Some(&proposal),
            &[],
            &[],
            &[],
            &tokio_util::sync::CancellationToken::new(),
        );
        assert!(rejected.unwrap_err().to_string().contains("未识别字段"));
        assert!(!copy.exists(), "unrecognized fields must block publication");
        assert!(source_files(copy_directory.path()).is_empty());
        assert_eq!(stored_workspace(&store), original);
        store.save(Some(&copy)).unwrap();
        assert_eq!(stored_workspace(&store), original);
        store.close().unwrap();
        assert_eq!(source_files(directory.path()), before);
        let reopened = ProjectStore::open(&copy).unwrap();
        assert_eq!(stored_workspace(&reopened), original);
        reopened.close().unwrap();
    }
}

#[test]
fn workspace_read_rejects_invalid_structure_without_changing_metadata() {
    let store = ProjectStore::create(None).unwrap();
    let before = put_invalid_workspace(&store);

    let rejected = store.workspace().is_err();
    let unchanged = stored_workspace(&store) == before;

    assert!(
        rejected && unchanged,
        "invalid workspace read must fail without writes: rejected={rejected}, unchanged={unchanged}"
    );
}

#[test]
fn workspace_write_rejects_invalid_previous_structure_without_changing_metadata() {
    let mut store = ProjectStore::create(None).unwrap();
    let before = put_invalid_workspace(&store);
    let replacement = Workspace {
        revision: 1,
        ..Workspace::default()
    };
    replacement.validate().unwrap();

    let rejected = store.save_workspace(&replacement).is_err();
    let unchanged = stored_workspace(&store) == before;

    assert!(
        rejected && unchanged,
        "invalid previous workspace must not be overwritten: rejected={rejected}, unchanged={unchanged}"
    );
}

#[test]
fn workspace_byte_gate_accepts_exact_limit_and_does_not_return_oversize_payload() {
    let store = ProjectStore::create(None).unwrap();
    let encoded = stored_workspace(&store);
    assert!(encoded.len() < 1024);
    let transaction = store.connection.unchecked_transaction().unwrap();

    assert_eq!(
        workspace_with_limit(&transaction, encoded.len()).unwrap(),
        Workspace::default()
    );
    let limit = encoded.len() - 1;
    let projected: Option<Vec<u8>> = transaction
        .query_row(WORKSPACE_QUERY, [limit as i64], |row| row.get(2))
        .unwrap();
    assert_eq!(projected, None, "SQL must withhold the out-of-budget blob");
    assert!(workspace_with_limit(&transaction, limit).is_err());
    transaction.commit().unwrap();
    assert_eq!(stored_workspace(&store), encoded);
}

#[test]
fn workspace_type_gate_rejects_text_json_and_numeric_values_without_writes() {
    let mut store = ProjectStore::create(None).unwrap();
    for invalid in [
        Value::Text(serde_json::to_string(&Workspace::default()).unwrap()),
        Value::Integer(1),
        Value::Real(1.5),
    ] {
        store
            .connection
            .execute(
                "UPDATE main.metadata SET value=?1 WHERE key='workspace'",
                params![&invalid],
            )
            .unwrap();
        assert!(store.workspace().is_err());
        assert!(
            store
                .save_workspace(&Workspace {
                    revision: 1,
                    ..Workspace::default()
                })
                .is_err()
        );
        let after: Value = store
            .connection
            .query_row(
                "SELECT value FROM main.metadata WHERE key='workspace'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(after, invalid);
    }
}

#[test]
fn id_requires_exactly_36_text_bytes_before_uuid_parsing() {
    let store = ProjectStore::create(None).unwrap();
    let transaction = store.connection.unchecked_transaction().unwrap();
    assert_eq!(project_id(&transaction).unwrap(), store.id);
    for invalid in [
        Value::Text(store.id.simple().to_string()),
        Value::Text(format!("{}\0", store.id)),
        Value::Text("é".repeat(36)),
        Value::Blob(store.id.to_string().into_bytes()),
        Value::Integer(36),
    ] {
        transaction
            .execute(
                "UPDATE main.metadata SET value=?1 WHERE key='id'",
                params![invalid],
            )
            .unwrap();
        assert!(project_id(&transaction).is_err());
    }
}

#[test]
fn computed_or_virtual_metadata_is_rejected_before_reading_its_values() {
    let directory = tempfile::tempdir().unwrap();
    for (index, schema) in [
        "CREATE VIEW metadata AS SELECT 'workspace' AS key,zeroblob(16) AS value;",
        "CREATE VIRTUAL TABLE metadata USING fts5(key,value);",
        "CREATE TABLE metadata(key TEXT,value BLOB GENERATED ALWAYS AS (zeroblob(16)) VIRTUAL);",
        "CREATE TABLE metadata(source_key TEXT,key TEXT GENERATED ALWAYS AS (source_key) VIRTUAL,value BLOB);",
    ]
    .into_iter()
    .enumerate()
    {
        let path = directory.path().join(format!("case-{index}.mio"));
        let setup = Connection::open(&path).unwrap();
        setup.execute_batch(schema).unwrap();
        drop(setup);
        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        connection
            .pragma_update(None, "trusted_schema", false)
            .unwrap();
        let value_reads = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&value_reads);
        connection.authorizer(Some(move |context: AuthContext<'_>| {
            if let AuthAction::Read {
                table_name: "metadata",
                ..
            } = context.action
            {
                observed.fetch_add(1, Ordering::SeqCst);
            }
            Authorization::Allow
        }));
        let transaction = connection.unchecked_transaction().unwrap();
        assert!(project_id(&transaction).is_err(), "{schema}");
        assert!(
            workspace_with_limit(&transaction, 1024).is_err(),
            "{schema}"
        );
        assert_eq!(value_reads.load(Ordering::SeqCst), 0, "{schema}");
    }
}

#[test]
fn workspace_reads_and_writes_bind_to_main_metadata() {
    let mut store = ProjectStore::create(None).unwrap();
    let temporary = Workspace {
        revision: 99,
        ..Workspace::default()
    };
    let temporary_bytes = serde_json::to_vec(&temporary).unwrap();
    store
        .connection
        .execute_batch("CREATE TEMP TABLE metadata(key TEXT,value BLOB);")
        .unwrap();
    store
        .connection
        .execute(
            "INSERT INTO temp.metadata VALUES ('workspace',?1)",
            params![&temporary_bytes],
        )
        .unwrap();
    assert_eq!(store.workspace().unwrap(), Workspace::default());
    let replacement = Workspace {
        revision: 1,
        ..Workspace::default()
    };
    store.save_workspace(&replacement).unwrap();
    assert_eq!(store.workspace().unwrap(), replacement);
    let unchanged: Vec<u8> = store
        .connection
        .query_row(
            "SELECT value FROM temp.metadata WHERE key='workspace'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unchanged, temporary_bytes);
}

#[test]
fn migration_revalidates_reopened_metadata_before_schema_writes() {
    for invalid_id in [false, true] {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(&format!(
                "PRAGMA application_id={APPLICATION_ID}; PRAGMA user_version=1;
                 CREATE TABLE metadata(key TEXT PRIMARY KEY,value BLOB NOT NULL);
                 CREATE TABLE flows(id TEXT PRIMARY KEY,revision INTEGER NOT NULL,
                     started INTEGER NOT NULL,url TEXT NOT NULL,method TEXT NOT NULL,
                     state TEXT NOT NULL,summary BLOB NOT NULL);
                 CREATE TABLE messages(flow TEXT NOT NULL REFERENCES flows(id),
                     side INTEGER NOT NULL,head BLOB NOT NULL,body BLOB NOT NULL,
                     PRIMARY KEY(flow,side));"
            ))
            .unwrap();
        let id = Uuid::new_v4();
        let stored_id = if invalid_id { Uuid::new_v4() } else { id };
        let workspace = Workspace {
            active: (!invalid_id).then(Uuid::new_v4),
            ..Workspace::default()
        };
        let bytes = serde_json::to_vec(&workspace).unwrap();
        connection
            .execute(
                "INSERT INTO metadata VALUES ('id',?1),('workspace',?2)",
                params![stored_id.to_string(), &bytes],
            )
            .unwrap();
        let mut store = ProjectStore {
            connection,
            id,
            path: None,
            temporary: None,
            intruder_sessions: super::super::intruder::IntruderSessions::default(),
            cached_workspace: std::sync::Mutex::new(None),
        };

        let error = store.migrate().unwrap_err().to_string();
        assert_eq!(
            error,
            if invalid_id {
                "项目 ID 在重新打开后已变化；未迁移或保存"
            } else {
                "当前工作标签不存在"
            }
        );

        let schema: i32 = store
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(schema, 1);
        let table_count: i64 = store
            .connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type='table'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 3, "no migration tables may be created");
        assert_eq!(stored_workspace(&store), bytes);
    }
}
