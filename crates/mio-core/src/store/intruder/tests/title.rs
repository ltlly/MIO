use super::*;

fn project(store: &ProjectStore) -> crate::ProjectKey {
    crate::ProjectKey {
        id: store.id,
        generation: 1,
    }
}

fn rename(
    store: &mut ProjectStore,
    run: &IntruderRunSummary,
    title: &str,
) -> Result<IntruderRunSummary> {
    store.rename_intruder_run(
        project(store),
        run.id,
        run.title_revision,
        title,
        &CancellationToken::new(),
    )
}

#[test]
fn task_titles_isolate_runs_from_the_same_tab_and_survive_results_save_and_save_as() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.mio");
    let copy = directory.path().join("copy.mio");
    let mut store = ProjectStore::create(Some(&source)).unwrap();
    let input = spec(1);
    let tab = bind(&mut store, &input);
    let first = store
        .create_intruder_run(Uuid::new_v4(), tab, &input, 1)
        .unwrap();
    let second = store
        .create_intruder_run(Uuid::new_v4(), tab, &input, 2)
        .unwrap();
    let workspace = store.workspace().unwrap();
    let renamed = rename(&mut store, &first, "任务甲🙂\0保留").unwrap();
    assert_eq!(renamed.revision, first.revision);
    assert_eq!(renamed.title_revision, 1);
    assert_eq!(renamed.title, "任务甲🙂\0保留");
    assert_eq!(store.read_intruder_run(second.id).unwrap(), second);
    assert_eq!(store.workspace().unwrap(), workspace);
    assert_eq!(store.read_intruder_spec(first.id).unwrap(), input);
    let response = complete(&pending());
    let completed = store
        .put_intruder_result(first.id, 0, Ok(&response))
        .unwrap();
    assert_eq!(
        (completed.title.as_str(), completed.title_revision),
        (renamed.title.as_str(), 1)
    );
    let finished = store
        .set_intruder_state(first.id, IntruderRunState::Completed)
        .unwrap();
    store.save(None).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    for path in [source, copy] {
        let reopened = ProjectStore::open(&path).unwrap();
        assert_eq!(reopened.read_intruder_run(first.id).unwrap(), finished);
        assert_eq!(reopened.read_intruder_run(second.id).unwrap(), second);
        assert_eq!(reopened.read(response.summary.id).unwrap(), response);
        assert_eq!(reopened.workspace().unwrap(), workspace);
        reopened.close().unwrap();
    }
}

#[test]
fn task_title_cas_rejects_stale_invalid_cancelled_and_failed_writes_without_retry() {
    let mut store = store();
    let original = run(&mut store, 1);
    let first = rename(&mut store, &original, "first").unwrap();
    assert!(rename(&mut store, &original, "stale").is_err());
    assert!(rename(&mut store, &first, &"中".repeat(1366)).is_err());
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(
        store
            .rename_intruder_run(project(&store), first.id, 1, "cancelled", &cancelled)
            .is_err()
    );
    assert!(
        store
            .rename_intruder_run(
                crate::ProjectKey {
                    id: Uuid::new_v4(),
                    generation: 1
                },
                first.id,
                1,
                "foreign",
                &CancellationToken::new()
            )
            .is_err()
    );
    store
        .connection
        .authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Update {
                table_name: "intruder_runs",
                column_name: "title" | "title_revision",
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }));
    assert!(rename(&mut store, &first, "denied").is_err());
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    let directory = tempfile::tempdir().unwrap();
    store
        .save(Some(&directory.path().join("saved.mio")))
        .unwrap();
    assert_eq!(store.read_intruder_run(first.id).unwrap(), first);
    let automatic = rename(&mut store, &first, "").unwrap();
    assert_eq!(
        (
            automatic.title.as_str(),
            automatic.title_revision,
            automatic.revision
        ),
        ("", 2, first.revision)
    );
    store
        .connection
        .execute(
            "UPDATE intruder_runs SET title_revision=?1 WHERE id=?2",
            params![i64::MAX, first.id.to_string()],
        )
        .unwrap();
    let exhausted = store.read_intruder_run(first.id).unwrap();
    assert!(rename(&mut store, &exhausted, "overflow").is_err());
    assert_eq!(store.read_intruder_run(first.id).unwrap(), exhausted);
}

#[test]
fn task_title_schema_one_persists_across_reopen_and_legacy_version_stays_readonly() {
    for legacy_conflict in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy-nine.mio");
        let mut store = ProjectStore::create(Some(&path)).unwrap();
        let attack = run(&mut store, 1);
        let workspace = store.workspace().unwrap();
        store.save(None).unwrap();
        store.close().unwrap();

        let reopened = ProjectStore::open(&path).unwrap();
        assert_eq!(reopened.read_intruder_run(attack.id).unwrap(), attack);
        assert_eq!(reopened.workspace().unwrap(), workspace);
        assert_eq!(reopened.read_intruder_spec(attack.id).unwrap(), spec(1));
        assert_eq!(
            reopened
                .connection
                .pragma_query_value::<i32, _>(None, "user_version", |row| row.get(0))
                .unwrap(),
            super::super::super::SCHEMA
        );
        reopened.close().unwrap();

        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 9).unwrap();
        if legacy_conflict {
            conn.execute_batch(
                "ALTER TABLE intruder_runs DROP COLUMN title;
                 ALTER TABLE intruder_runs ADD COLUMN title BLOB DEFAULT X'FF00';",
            )
            .unwrap();
        }
        conn.close().unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(ProjectStore::open(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[test]
fn task_title_storage_types_and_byte_limit_are_checked_before_summary_delivery() {
    for (column, value) in [
        ("title", rusqlite::types::Value::Blob(vec![255])),
        ("title", rusqlite::types::Value::Text("中".repeat(1366))),
        ("title_revision", rusqlite::types::Value::Integer(-1)),
        ("title_revision", rusqlite::types::Value::Real(1.5)),
    ] {
        let mut store = store();
        let attack = run(&mut store, 1);
        store
            .connection
            .execute_batch("PRAGMA ignore_check_constraints=ON;")
            .unwrap();
        store
            .connection
            .execute(
                &format!("UPDATE intruder_runs SET {column}=?1 WHERE id=?2"),
                params![value, attack.id.to_string()],
            )
            .unwrap();
        assert!(store.read_intruder_run(attack.id).is_err());
        assert!(store.intruder_runs(None, 10).is_err());
    }
}
