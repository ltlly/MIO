use super::*;

fn stored_phase(store: &ProjectStore, id: Uuid) -> (i64, i64) {
    store
        .connection
        .query_row(
            "SELECT state,paused FROM intruder_runs WHERE id=?1",
            [id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

#[test]
fn pause_blocks_new_ordinals_but_inflight_completion_and_explicit_resume_keep_identity() {
    let mut store = store();
    let run = run(&mut store, 2);
    let request = pending();
    store.put_intruder_result(run.id, 0, Ok(&request)).unwrap();
    let paused = store
        .set_intruder_state(run.id, IntruderRunState::Paused)
        .unwrap();
    assert_eq!(stored_phase(&store, run.id), (0, 1));
    assert_eq!(paused.state, IntruderRunState::Paused);
    assert_eq!(paused.revision, run.revision + 2);
    assert_eq!(
        store
            .set_intruder_state(run.id, IntruderRunState::Paused)
            .unwrap(),
        paused
    );
    assert!(
        store
            .put_intruder_result(run.id, 1, Err("not dispatched"))
            .is_err()
    );
    let response = complete(&request);
    let finished = store.put_intruder_result(run.id, 0, Ok(&response)).unwrap();
    assert_eq!(
        (finished.state, finished.dispatched, finished.completed),
        (IntruderRunState::Paused, 1, 1)
    );
    assert_eq!(store.read(request.summary.id).unwrap(), response);
    let resumed = store
        .set_intruder_state(run.id, IntruderRunState::Running)
        .unwrap();
    assert_eq!(resumed.revision, paused.revision + 2);
    assert_eq!(stored_phase(&store, run.id), (0, 0));
    assert_eq!(
        store
            .set_intruder_state(run.id, IntruderRunState::Running)
            .unwrap(),
        resumed
    );
    store
        .put_intruder_result(run.id, 1, Err("invalid request, not sent"))
        .unwrap();
    assert!(
        store
            .set_intruder_state(run.id, IntruderRunState::Paused)
            .is_err()
    );
    let completed = store
        .set_intruder_state(run.id, IntruderRunState::Completed)
        .unwrap();
    assert_eq!(
        (completed.total, completed.dispatched, completed.completed),
        (2, 2, 2)
    );
    assert_eq!(stored_phase(&store, run.id), (3, 0));
    for state in [IntruderRunState::Paused, IntruderRunState::Running] {
        assert!(store.set_intruder_state(run.id, state).is_err());
    }
}

#[test]
fn stopping_and_terminal_transitions_clear_the_pause_flag() {
    for terminal in [
        IntruderRunState::Stopped,
        IntruderRunState::Failed("write fault".into()),
    ] {
        let mut store = store();
        let run = run(&mut store, 1);
        store
            .set_intruder_state(run.id, IntruderRunState::Paused)
            .unwrap();
        store
            .set_intruder_state(run.id, IntruderRunState::Stopping)
            .unwrap();
        assert_eq!(stored_phase(&store, run.id), (1, 0));
        assert!(
            store
                .set_intruder_state(run.id, IntruderRunState::Paused)
                .is_err()
        );
        assert!(
            store
                .set_intruder_state(run.id, IntruderRunState::Running)
                .is_err()
        );
        let stopped = store.set_intruder_state(run.id, terminal.clone()).unwrap();
        assert_eq!(stopped.state, terminal);
        assert_eq!(stored_phase(&store, run.id).1, 0);
    }
    let mut store = store();
    let run = run(&mut store, 1);
    let request = pending();
    store.put_intruder_result(run.id, 0, Ok(&request)).unwrap();
    store
        .set_intruder_state(run.id, IntruderRunState::Paused)
        .unwrap();
    store
        .put_intruder_result(run.id, 0, Ok(&complete(&request)))
        .unwrap();
    assert!(
        store
            .set_intruder_state(run.id, IntruderRunState::Running)
            .is_err()
    );
    store
        .set_intruder_state(run.id, IntruderRunState::Completed)
        .unwrap();
    assert_eq!(stored_phase(&store, run.id), (3, 0));
}

#[test]
fn paused_save_as_reopen_and_atomic_recovery_preserve_results_without_replay() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("paused.mio");
    let copy = directory.path().join("paused-copy.mio");
    let mut store = store();
    let run = run(&mut store, 3);
    let request = pending();
    let observed = complete(&pending());
    store.put_intruder_result(run.id, 0, Ok(&request)).unwrap();
    store.put_intruder_result(run.id, 1, Ok(&observed)).unwrap();
    let paused = store
        .set_intruder_state(run.id, IntruderRunState::Paused)
        .unwrap();
    let input = store.read_intruder_spec(run.id).unwrap();
    store.save(Some(&path)).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();

    for path in [path, copy] {
        let mut restored = ProjectStore::open(&path).unwrap();
        assert_eq!(restored.read_intruder_run(run.id).unwrap(), paused);
        assert_eq!(stored_phase(&restored, run.id), (0, 1));
        restored.connection.authorizer(Some(deny_run_update));
        assert!(restored.interrupt_intruder_runs().is_err());
        restored
            .connection
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        assert_eq!(restored.read_intruder_run(run.id).unwrap(), paused);
        assert_eq!(restored.read(request.summary.id).unwrap(), request);
        restored.connection.authorizer(Some(deny_large_data_reads));
        assert_eq!(restored.interrupt_intruder_runs().unwrap(), 1);
        restored
            .connection
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        assert_eq!(stored_phase(&restored, run.id), (4, 0));
        let interrupted = restored.read_intruder_run(run.id).unwrap();
        assert_eq!(
            (
                interrupted.state,
                interrupted.total,
                interrupted.dispatched,
                interrupted.completed
            ),
            (IntruderRunState::Interrupted, 3, 2, 2)
        );
        assert_eq!(restored.read_intruder_spec(run.id).unwrap(), input);
        assert_eq!(restored.read(observed.summary.id).unwrap(), observed);
        let failed = restored.read(request.summary.id).unwrap();
        assert_eq!(failed.request, request.request);
        assert!(
            matches!(failed.summary.state, FlowState::Failed(ref error) if error.contains("结果未知") && error.contains("未自动重发"))
        );
        assert!(
            restored
                .set_intruder_state(run.id, IntruderRunState::Running)
                .is_err()
        );
        assert_eq!(restored.interrupt_intruder_runs().unwrap(), 0);
        restored.close().unwrap();
    }
}

fn schema_six(store: &mut ProjectStore, id: Uuid) {
    store.connection.execute_batch(
        "ALTER TABLE intruder_runs DROP COLUMN paused;
         ALTER TABLE intruder_runs DROP COLUMN title;
         ALTER TABLE intruder_runs DROP COLUMN title_revision;
         ALTER TABLE intruder_runs DROP COLUMN baseline_ordinal;
         ALTER TABLE intruder_runs DROP COLUMN pool_revision;
         ALTER TABLE intruder_runs DROP COLUMN pool_id;
         ALTER TABLE intruder_runs ADD COLUMN extension BLOB;
         CREATE INDEX intruder_extension ON intruder_runs(extension,id);
         CREATE TABLE retained_notes(run TEXT PRIMARY KEY REFERENCES intruder_runs(id),data BLOB NOT NULL);
         DROP INDEX flows_source_started; DROP INDEX flows_source_status; DROP INDEX flows_source_capture; ALTER TABLE flows DROP COLUMN response_length; ALTER TABLE flows DROP COLUMN capture_number; ALTER TABLE flows DROP COLUMN elapsed_ms; ALTER TABLE flows DROP COLUMN response_bytes; ALTER TABLE flows DROP COLUMN status; ALTER TABLE flows DROP COLUMN source; ALTER TABLE flows DROP COLUMN listener_port;
         ALTER TABLE intruder_results DROP COLUMN highlight_revision;
         ALTER TABLE intruder_results DROP COLUMN highlight;
         ALTER TABLE intruder_results DROP COLUMN notes_revision;
         ALTER TABLE intruder_results DROP COLUMN notes;
         DROP TABLE site_map_entries; DROP TABLE ext_http_highlights; DROP TABLE websocket_originals; DROP TABLE ext_http_notes; DROP TABLE repeater_history; DROP TABLE websocket_notes; DROP TABLE websocket_connections; PRAGMA user_version=6;"
    ).unwrap();
    store
        .connection
        .execute(
            "UPDATE intruder_runs SET extension=?1 WHERE id=?2",
            params![[0u8, 255, 10].as_slice(), id.to_string()],
        )
        .unwrap();
    store
        .connection
        .execute(
            "INSERT INTO retained_notes VALUES(?1,?2)",
            params![id.to_string(), [255u8, 0, 7].as_slice()],
        )
        .unwrap();
}

fn assert_extensions(store: &ProjectStore, id: Uuid) {
    let values: (Vec<u8>, Vec<u8>) = store
        .connection
        .query_row(
            "SELECT extension,data FROM intruder_runs JOIN retained_notes ON run=id WHERE id=?1",
            [id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(values, (vec![0, 255, 10], vec![255, 0, 7]));
    let index: String = store
        .connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='intruder_extension'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        index,
        "CREATE INDEX intruder_extension ON intruder_runs(extension,id)"
    );
    let violation = store
        .connection
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .exists([])
        .unwrap();
    assert!(!violation);
}

#[test]
fn schema_six_upgrade_preserves_unknown_columns_indices_foreign_keys_payloads_and_bodies() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy-six.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let run = run(&mut store, 3);
    let request = pending();
    let observed = complete(&pending());
    store.put_intruder_result(run.id, 0, Ok(&request)).unwrap();
    let prior = store.put_intruder_result(run.id, 1, Ok(&observed)).unwrap();
    let input = store.read_intruder_spec(run.id).unwrap();
    let workspace = store.workspace().unwrap();
    schema_six(&mut store, run.id);
    store.checkpoint().unwrap();
    store.close().unwrap();
    let legacy =
        Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        legacy
            .pragma_query_value::<i32, _>(None, "user_version", |row| row.get(0))
            .unwrap(),
        6
    );
    legacy.close().unwrap();
    let mut restored = ProjectStore::open(&path).unwrap();
    assert_eq!(
        restored
            .connection
            .pragma_query_value::<i32, _>(None, "user_version", |row| row.get(0))
            .unwrap(),
        super::super::super::SCHEMA
    );
    assert_eq!(restored.read_intruder_run(run.id).unwrap(), prior);
    assert_eq!(restored.read_intruder_spec(run.id).unwrap(), input);
    assert_eq!(restored.workspace().unwrap(), workspace);
    assert_eq!(restored.read(request.summary.id).unwrap(), request);
    assert_eq!(restored.read(observed.summary.id).unwrap(), observed);
    assert_eq!(stored_phase(&restored, run.id), (0, 0));
    assert_extensions(&restored, run.id);
    restored
        .set_intruder_state(run.id, IntruderRunState::Paused)
        .unwrap();
    assert_eq!(stored_phase(&restored, run.id), (0, 1));
    assert_extensions(&restored, run.id);
    restored.close().unwrap();
}

fn deny_version_write(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Pragma {
            pragma_name: "user_version",
            pragma_value: Some(_),
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}

#[test]
fn failed_upgrade_rolls_back_the_added_column_version_and_existing_data() {
    let mut store = store();
    let run = run(&mut store, 1);
    let input = store.read_intruder_spec(run.id).unwrap();
    schema_six(&mut store, run.id);
    store.connection.authorizer(Some(deny_version_write));
    assert!(store.migrate().is_err());
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert_eq!(
        store
            .connection
            .pragma_query_value::<i32, _>(None, "user_version", |row| row.get(0))
            .unwrap(),
        6
    );
    let paused_exists: bool = store
        .connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('intruder_runs') WHERE name='paused')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        !paused_exists,
        "ALTER TABLE and version promotion must roll back together"
    );
    assert_extensions(&store, run.id);
    store.migrate().unwrap();
    assert_eq!(store.read_intruder_run(run.id).unwrap(), run);
    assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
    assert_extensions(&store, run.id);
}

#[test]
fn schema_checks_and_summary_decoding_reject_invalid_pause_flags() {
    use rusqlite::types::Value;
    let mut store = store();
    let run = run(&mut store, 1);
    for value in [
        Value::Integer(-1),
        Value::Integer(2),
        Value::Real(0.5),
        Value::Blob(vec![1]),
    ] {
        assert!(
            store
                .connection
                .execute(
                    "UPDATE intruder_runs SET paused=?1 WHERE id=?2",
                    params![value, run.id.to_string()]
                )
                .is_err()
        );
        store
            .connection
            .pragma_update(None, "ignore_check_constraints", true)
            .unwrap();
        store
            .connection
            .execute(
                "UPDATE intruder_runs SET paused=?1 WHERE id=?2",
                params![value, run.id.to_string()],
            )
            .unwrap();
        assert!(store.read_intruder_run(run.id).is_err());
        assert!(store.intruder_runs(None, 1).is_err());
        store
            .connection
            .execute(
                "UPDATE intruder_runs SET paused=0 WHERE id=?1",
                [run.id.to_string()],
            )
            .unwrap();
        store
            .connection
            .pragma_update(None, "ignore_check_constraints", false)
            .unwrap();
    }
    assert!(
        store
            .connection
            .execute(
                "UPDATE intruder_runs SET paused=1,state=1 WHERE id=?1",
                [run.id.to_string()]
            )
            .is_err()
    );
    assert_eq!(store.read_intruder_run(run.id).unwrap(), run);
}
