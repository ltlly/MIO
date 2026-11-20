use super::*;

#[test]
fn baseline_admission_keeps_the_total_limit_without_materializing_payload_combinations() {
    let mut store = store();
    let mut input = spec(0);
    input.attack_type = AttackType::ClusterBomb;
    input.positions = vec![Position { start: 0, end: 0 }; 6];
    input.payload_sets = vec![(0..10).map(|value| vec![value]).collect(); 6];
    let tab = bind(&mut store, &input);
    let run = store
        .create_intruder_run(Uuid::new_v4(), tab, &input, 1)
        .unwrap();
    assert_eq!(run.total, MAX_REQUESTS);
    assert!(
        store
            .request_intruder_baseline(
                project(&store),
                run.id,
                run.revision,
                &CancellationToken::new()
            )
            .is_err()
    );
    assert_eq!(store.read_intruder_run(run.id).unwrap(), run);
    assert_eq!(
        store
            .connection
            .query_row::<usize, _, _>("SELECT count(*) FROM intruder_results", [], |row| row
                .get(0))
            .unwrap(),
        0
    );
}

#[test]
fn baseline_admission_rejects_unknown_execution_metadata_without_rewriting_its_bytes() {
    let mut store = store();
    let (run, input) = baseline_run(&mut store, false);
    store.put_intruder_result(run.id, 0, Err("old A")).unwrap();
    store.put_intruder_result(run.id, 1, Err("old B")).unwrap();
    let before = store
        .set_intruder_state(run.id, IntruderRunState::Completed)
        .unwrap();
    let meta: Vec<u8> = store
        .connection
        .query_row(
            "SELECT spec_meta FROM intruder_runs WHERE id=?1",
            [run.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&meta).unwrap();
    value["future_send_policy"] = serde_json::json!({"requires_explicit_route": true});
    let unknown = serde_json::to_vec(&value).unwrap();
    store
        .connection
        .execute(
            "UPDATE intruder_runs SET spec_meta=?1 WHERE id=?2",
            params![unknown, run.id.to_string()],
        )
        .unwrap();
    // Unknown execution fields remain readable for inspection, but cannot authorize another send.
    assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
    let result = store.request_intruder_baseline(
        project(&store),
        run.id,
        before.revision,
        &CancellationToken::new(),
    );
    let unchanged: Vec<u8> = store
        .connection
        .query_row(
            "SELECT spec_meta FROM intruder_runs WHERE id=?1",
            [run.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unchanged, unknown);
    assert!(
        result.is_err(),
        "unknown execution metadata must reject baseline admission before reactivation"
    );
    assert_eq!(store.read_intruder_run(run.id).unwrap(), before);
    assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
}

#[test]
fn baseline_admission_rejects_corrupt_summary_even_when_index_counts_match() {
    for corrupt_revision in [false, true] {
        let mut store = store();
        let (run, _) = baseline_run(&mut store, false);
        let pending = pending();
        store.put_intruder_result(run.id, 0, Ok(&pending)).unwrap();
        let completed = complete(&pending);
        store
            .put_intruder_result(run.id, 0, Ok(&completed))
            .unwrap();
        store
            .put_intruder_result(run.id, 1, Err("old rejection"))
            .unwrap();
        let before = store
            .set_intruder_state(run.id, IntruderRunState::Completed)
            .unwrap();
        let mut damaged = completed.summary.clone();
        if corrupt_revision {
            damaged.revision += 1;
        } else {
            damaged.source = TrafficSource::Repeater;
        }
        let bytes = serde_json::to_vec(&damaged).unwrap();
        store
            .connection
            .execute(
                "UPDATE flows SET summary=?1 WHERE id=?2",
                params![bytes, damaged.id.to_string()],
            )
            .unwrap();
        let index: (u64, u64, u64, u64) = store.connection.query_row(
            "SELECT count(*),min(r.ordinal),max(r.ordinal),sum(CASE WHEN r.error IS NOT NULL OR f.state IN ('complete','dropped','cancelled','failed') THEN 1 ELSE 0 END)
             FROM intruder_results r LEFT JOIN flows f ON f.id=r.flow WHERE r.run=?1",
            [run.id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).unwrap();
        assert_eq!(
            index,
            (2, 0, 1, 2),
            "summary corruption leaves every aggregate index check satisfied"
        );
        let mut prepared = false;
        assert!(
            store
                .request_intruder_baseline_with(
                    project(&store),
                    run.id,
                    before.revision,
                    &CancellationToken::new(),
                    |_| {
                        prepared = true;
                        Ok(())
                    }
                )
                .is_err()
        );
        assert!(
            !prepared,
            "corrupt summaries must be rejected before sender preparation"
        );
        assert_eq!(store.read_intruder_run(run.id).unwrap(), before);
        assert_eq!(
            store
                .connection
                .query_row::<Vec<u8>, _, _>(
                    "SELECT summary FROM flows WHERE id=?1",
                    [damaged.id.to_string()],
                    |row| row.get(0)
                )
                .unwrap(),
            bytes
        );
    }
}

#[test]
fn baseline_project_preparation_failure_or_cancellation_preserves_the_original_run() {
    for failure in ["prepare", "cancel", "write"] {
        let mut store = store();
        let (run, input) = baseline_run(&mut store, false);
        store.put_intruder_result(run.id, 0, Err("old A")).unwrap();
        store.put_intruder_result(run.id, 1, Err("old B")).unwrap();
        let before = store
            .set_intruder_state(run.id, IntruderRunState::Completed)
            .unwrap();
        let rows = store.intruder_results(run.id, None, 10).unwrap().rows;
        let workspace = store.workspace().unwrap();
        if failure == "write" {
            store
                .connection
                .authorizer(Some(|context: AuthContext<'_>| match context.action {
                    AuthAction::Update {
                        table_name: "intruder_runs",
                        ..
                    } => Authorization::Deny,
                    _ => Authorization::Allow,
                }));
        }
        let cancel = CancellationToken::new();
        let mut prepared = 0;
        let result = store.request_intruder_baseline_with(
            project(&store),
            run.id,
            before.revision,
            &cancel,
            |spec| {
                prepared += 1;
                assert_eq!(spec, input);
                if failure == "prepare" {
                    bail!("fixture sender preparation refused")
                }
                if failure == "cancel" {
                    cancel.cancel();
                }
                Ok(())
            },
        );
        store
            .connection
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        assert!(result.is_err(), "{failure}");
        assert_eq!(
            prepared, 1,
            "validation must hand the frozen spec to preparation exactly once"
        );
        assert_eq!(store.read_intruder_run(run.id).unwrap(), before);
        assert_eq!(store.intruder_results(run.id, None, 10).unwrap().rows, rows);
        assert_eq!(store.read_intruder_spec(run.id).unwrap(), input);
        assert_eq!(store.workspace().unwrap(), workspace);
        assert!(
            store
                .request_intruder_baseline(
                    project(&store),
                    run.id,
                    before.revision,
                    &CancellationToken::new()
                )
                .is_ok(),
            "failed preparation cannot consume the one baseline slot"
        );
    }
}

#[test]
fn baseline_admission_preserves_extension_references_and_allows_unrelated_extensions() {
    let mut independent = store();
    let (run, _) = baseline_run(&mut independent, false);
    independent
        .connection
        .execute_batch(
            "CREATE TABLE extension_notes(id INTEGER PRIMARY KEY, payload BLOB NOT NULL);
         INSERT INTO extension_notes VALUES(1,X'FF002A');
         ALTER TABLE intruder_runs ADD COLUMN extension_payload BLOB;
         UPDATE intruder_runs SET extension_payload=X'FF002A';",
        )
        .unwrap();
    let accepted = independent
        .request_intruder_baseline(
            project(&independent),
            run.id,
            run.revision,
            &CancellationToken::new(),
        )
        .unwrap();
    assert!(accepted.baseline.is_some());
    for query in [
        "SELECT payload FROM extension_notes WHERE id=1",
        "SELECT extension_payload FROM intruder_runs",
    ] {
        assert_eq!(
            independent
                .connection
                .query_row::<Vec<u8>, _, _>(query, [], |row| row.get(0))
                .unwrap(),
            [255, 0, 42]
        );
    }

    let mut observations = Vec::new();
    for action in ["CASCADE", "SET NULL"] {
        let mut store = store();
        let (run, _) = baseline_run(&mut store, false);
        store
            .connection
            .execute_batch(&format!(
                "CREATE UNIQUE INDEX extension_run_total ON intruder_runs(total);
             CREATE TABLE extension_totals(id INTEGER PRIMARY KEY, total INTEGER
                 REFERENCES intruder_runs(total) ON UPDATE {action});
             INSERT INTO extension_totals VALUES(1,2);"
            ))
            .unwrap();
        let before: Option<u64> = store
            .connection
            .query_row("SELECT total FROM extension_totals WHERE id=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(before, Some(2));
        let result = store.request_intruder_baseline(
            project(&store),
            run.id,
            run.revision,
            &CancellationToken::new(),
        );
        let after: Option<u64> = store
            .connection
            .query_row("SELECT total FROM extension_totals WHERE id=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        let unchanged_run = store.read_intruder_run(run.id).unwrap() == run;
        let foreign_keys: bool = store
            .connection
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        let violations: usize = store
            .connection
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        observations.push((
            action,
            result.is_ok(),
            before,
            after,
            unchanged_run,
            foreign_keys,
            violations,
        ));
    }
    assert!(
        observations.iter().all(
            |(_, accepted, before, after, unchanged_run, foreign_keys, violations)| {
                before == after
                    && (*accepted || *unchanged_run)
                    && *foreign_keys
                    && *violations == 0
            }
        ),
        "baseline admission must preserve dependent extension values without disabling FK checks; observations={observations:?}"
    );
}
