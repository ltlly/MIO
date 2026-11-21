use super::*;

#[test]
fn missing_or_mismatched_columns_are_rejected_readonly_without_changing_source_data() {
    for (mutation, expected_column) in [
        ("ALTER TABLE intruder_runs DROP COLUMN paused;", "intruder_runs"),
        (
            "ALTER TABLE intruder_runs DROP COLUMN baseline_ordinal;",
            "intruder_runs",
        ),
        ("ALTER TABLE flows DROP COLUMN listener_port;", "flows"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("column-conflict.mio");
        fixture(&path);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(mutation).unwrap();
        connection.close().unwrap();
        let error = assert_readonly_rejection(&path);
        assert!(error.contains(expected_column), "unexpected error: {error}");
    }
}

#[test]
fn schema_one_open_save_save_as_slim_save_and_reopen_preserve_unknown_ordinary_tables() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("schema-one.mio");
    let (id, flow, mut workspace) = fixture(&path);
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

    let mut store = ProjectStore::open(&path).unwrap();
    assert_eq!(store.id, id);
    assert_eq!(store.workspace().unwrap(), workspace);
    assert_eq!(store.read(flow.summary.id).unwrap(), flow);
    assert!(store.ext_sources().unwrap().is_empty());
    assert_eq!(store.websocket_page(None, 0, 1, &[]).unwrap().0, 0);
    let version: i32 = store
        .connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA);
    let retained: Vec<u8> = store
        .connection
        .query_row(
            "SELECT data FROM additional_notes WHERE id='retained'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained, note);

    workspace.revision += 1;
    workspace.repeater_settings.unpack_compressed_responses = false;
    store.save_workspace(&workspace).unwrap();
    store.save(None).unwrap();

    let save_as_path = directory.path().join("saved-as.mio");
    store.save(Some(&save_as_path)).unwrap();

    let slim_path = directory.path().join("slim.mio");
    store
        .slim_save_as(
            &slim_path,
            &crate::SlimSaveOptions::default(),
            &[],
            &[],
            &[],
            &[],
            &tokio_util::sync::CancellationToken::new(),
        )
        .unwrap();
    store.close().unwrap();

    for check_path in [&path, &save_as_path, &slim_path] {
        let reopened = ProjectStore::open(check_path).unwrap();
        assert_eq!(reopened.workspace().unwrap(), workspace);
        assert_eq!(reopened.read(flow.summary.id).unwrap(), flow);
        reopened.close().unwrap();
    }
}

#[test]
fn occupied_discard_namespace_is_rejected_readonly() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("occupied-discard-namespace.mio");
    fixture(&path);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO metadata VALUES(?1,?2)",
            params![
                format!("intruder_discarded:{}", Uuid::new_v4()),
                [1_u8; 3].as_slice()
            ],
        )
        .unwrap();
    connection.close().unwrap();
    assert!(assert_readonly_rejection(&path).contains("丢弃记录"));
}
