use super::*;

pub(in crate::store::intruder) fn deny_large_data_reads(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Read {
            table_name: "messages" | "intruder_payloads",
            ..
        }
        | AuthAction::Read {
            table_name: "intruder_runs",
            column_name: "template" | "spec_meta",
            ..
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}

#[test]
fn run_pages_use_indexed_started_id_cursors_without_reading_attack_inputs() {
    let mut store = store();
    let empty = store.intruder_runs(None, 1).unwrap();
    assert!(empty.rows.is_empty());
    assert_eq!(empty.next_after, None);
    assert_eq!(store.find_intruder_run(Uuid::from_u128(99)).unwrap(), None);
    let input = spec(0);
    let tab = bind(&mut store, &input);
    for (id, started) in [(3, 10), (5, -4), (2, 10), (1, i64::MIN), (4, i64::MAX)] {
        store
            .create_intruder_run(Uuid::from_u128(id), tab, &input, started)
            .unwrap();
    }
    store.connection.authorizer(Some(deny_large_data_reads));
    let first = store.intruder_runs(None, 2).unwrap();
    assert_eq!(
        first
            .rows
            .iter()
            .map(|run| run.id.as_u128())
            .collect::<Vec<_>>(),
        [1, 5]
    );
    assert_eq!(first.next_after, Some(Uuid::from_u128(5)));
    let second = store.intruder_runs(first.next_after, 2).unwrap();
    assert_eq!(
        second
            .rows
            .iter()
            .map(|run| run.id.as_u128())
            .collect::<Vec<_>>(),
        [2, 3]
    );
    assert_eq!(second.next_after, Some(Uuid::from_u128(3)));
    let last = store.intruder_runs(second.next_after, 2).unwrap();
    assert_eq!(last.rows.len(), 1);
    assert_eq!(last.rows[0].id, Uuid::from_u128(4));
    assert_eq!(last.next_after, None);
    assert!(
        store
            .intruder_runs(Some(last.rows[0].id), 2)
            .unwrap()
            .rows
            .is_empty()
    );
    assert_eq!(
        store.find_intruder_run(first.rows[0].id).unwrap(),
        Some(first.rows[0].clone())
    );
    assert!(store.intruder_runs(Some(Uuid::from_u128(99)), 1).is_err());
    assert!(store.intruder_runs(None, 0).is_err());
    assert!(store.intruder_runs(None, 501).is_err());
    let mut query = store
        .connection
        .prepare(&format!("EXPLAIN QUERY PLAN {}", run_page_query(true)))
        .unwrap();
    let plan: Vec<String> = query
        .query_map(params![3, 10, Uuid::from_u128(2).to_string()], |row| {
            row.get(3)
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(
        plan.iter()
            .any(|line| line.contains("intruder_runs_started"))
    );
    assert!(!plan.iter().any(|line| line.contains("TEMP B-TREE")));
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
}

#[test]
fn run_page_budget_includes_json_escaping_and_returns_the_last_included_id() {
    let mut store = store();
    let message = "\0".repeat(MAX_ERROR_BYTES);
    for id in 1..=4 {
        let input = spec(0);
        let tab = bind(&mut store, &input);
        let run = store
            .create_intruder_run(Uuid::from_u128(id), tab, &input, 0)
            .unwrap();
        store
            .set_intruder_state(run.id, IntruderRunState::Failed(message.clone()))
            .unwrap();
    }
    let first = store.intruder_runs(None, 500).unwrap();
    assert_eq!(first.rows.len(), 2);
    assert_eq!(first.next_after, Some(Uuid::from_u128(2)));
    assert!(serde_json::to_vec(&first.rows).unwrap().len() <= PAGE_BYTES);
    let last = store.intruder_runs(first.next_after, 500).unwrap();
    assert_eq!(last.rows.len(), 2);
    assert_eq!(last.rows[0].id, Uuid::from_u128(3));
    assert_eq!(last.next_after, None);
    assert!(serde_json::to_vec(&last.rows).unwrap().len() <= PAGE_BYTES);
}

#[test]
fn optional_run_lookup_and_cursor_reads_reject_corrupt_metadata() {
    let mut store = store();
    let run = run(&mut store, 0);
    assert_eq!(store.find_intruder_run(run.id).unwrap(), Some(run.clone()));
    store
        .connection
        .execute_batch("PRAGMA ignore_check_constraints=ON")
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE intruder_runs SET tab='broken' WHERE id=?1",
            [run.id.to_string()],
        )
        .unwrap();
    assert!(store.find_intruder_run(run.id).is_err());
    assert!(store.intruder_runs(Some(run.id), 1).is_err());
    assert!(store.intruder_runs(None, 1).is_err());
    store
        .connection
        .execute(
            "UPDATE intruder_runs SET tab=?1,state=5,error=zeroblob(1) WHERE id=?2",
            params![run.tab_id.to_string(), run.id.to_string()],
        )
        .unwrap();
    assert!(store.find_intruder_run(run.id).is_err());
    assert!(store.intruder_runs(Some(run.id), 1).is_err());
    store
        .connection
        .execute(
            "UPDATE intruder_runs SET error=?1 WHERE id=?2",
            params!["x".repeat(MAX_ERROR_BYTES + 1), run.id.to_string()],
        )
        .unwrap();
    assert!(store.find_intruder_run(run.id).is_err());
    assert!(store.intruder_runs(Some(run.id), 1).is_err());
    assert_eq!(store.find_intruder_run(Uuid::new_v4()).unwrap(), None);
}

#[test]
fn result_pages_are_stable_summaries_without_reading_templates_payloads_or_bodies() {
    let mut store = store();
    let run = run(&mut store, 3);
    let request = pending();
    let finished = complete(&request);
    store.put_intruder_result(run.id, 0, Ok(&request)).unwrap();
    store
        .put_intruder_result(run.id, 1, Err("parse failure"))
        .unwrap();
    store
        .put_intruder_result(run.id, 2, Ok(&pending()))
        .unwrap();
    store.put_intruder_result(run.id, 0, Ok(&finished)).unwrap();
    store.connection.authorizer(Some(deny_large_data_reads));
    let first = store.intruder_results(run.id, None, 2).unwrap();
    assert!(first.persisted);
    assert_eq!(first.rows.len(), 2);
    assert_eq!(
        first.rows[0],
        IntruderResultSummary {
            ordinal: 0,
            outcome: IntruderResultOutcome::Flow(finished.summary)
        }
    );
    assert_eq!(first.next_after, Some(1));
    let second = store.intruder_results(run.id, first.next_after, 2).unwrap();
    assert!(second.persisted);
    assert_eq!(second.rows.len(), 1);
    assert_eq!(second.rows[0].ordinal, 2);
    assert_eq!(second.next_after, None);
    assert!(store.read_intruder_spec(run.id).is_err());
    assert!(store.intruder_results(run.id, None, 0).is_err());
    assert!(store.intruder_results(run.id, None, 501).is_err());
    assert!(store.intruder_results(run.id, Some(u64::MAX), 1).is_err());
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
}

#[test]
fn page_byte_budget_returns_a_cursor_instead_of_materializing_all_errors() {
    let mut store = store();
    let run = run(&mut store, 17);
    let error = "x".repeat(MAX_ERROR_BYTES);
    assert!(
        store
            .put_intruder_result(run.id, 0, Err(&(error.clone() + "x")))
            .is_err()
    );
    assert_eq!(store.read_intruder_run(run.id).unwrap(), run);
    for ordinal in 0..run.total {
        store
            .put_intruder_result(run.id, ordinal, Err(&error))
            .unwrap();
    }
    let first = store.intruder_results(run.id, None, 500).unwrap();
    assert_eq!((first.rows.len(), first.next_after), (16, Some(15)));
    let last = store
        .intruder_results(run.id, first.next_after, 500)
        .unwrap();
    assert_eq!((last.rows.len(), last.next_after), (1, None));
    assert_eq!(last.rows[0].ordinal, 16);
}

#[test]
fn sql_blob_gates_withhold_values_before_rust_allocation() {
    let mut store = store();
    let run = run(&mut store, 1);
    let (meta, template): (Vec<u8>, Vec<u8>) = store
        .connection
        .query_row(
            SPEC_QUERY,
            params![
                run.id.to_string(),
                MAX_SPEC_META as i64,
                MAX_REQUEST_BYTES as i64
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let (meta_over, template_over): (Option<Vec<u8>>, Option<Vec<u8>>) = store
        .connection
        .query_row(
            SPEC_QUERY,
            params![
                run.id.to_string(),
                meta.len() as i64 - 1,
                template.len() as i64 - 1
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((meta_over, template_over), (None, None));
    let withheld: Option<Vec<u8>> = store
        .connection
        .query_row(PAYLOAD_QUERY, params![run.id.to_string(), 1, 2], |row| {
            row.get(4)
        })
        .unwrap();
    assert_eq!(withheld, None);
    assert_eq!(store.read_intruder_spec(run.id).unwrap(), spec(1));
}

#[test]
fn corrupt_blob_types_lengths_and_payload_ordinals_are_rejected() {
    for column in ["spec_meta", "template"] {
        let mut store = store();
        let run = run(&mut store, 1);
        store
            .connection
            .execute_batch("PRAGMA ignore_check_constraints=ON")
            .unwrap();
        store
            .connection
            .execute(
                &format!("UPDATE intruder_runs SET {column}=CAST({column} AS TEXT) WHERE id=?1"),
                [run.id.to_string()],
            )
            .unwrap();
        assert!(store.read_intruder_spec(run.id).is_err());
    }
    let mut store = store();
    let run = run(&mut store, 1);
    store
        .connection
        .execute_batch("PRAGMA ignore_check_constraints=ON")
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE intruder_runs SET spec_meta=zeroblob(?1) WHERE id=?2",
            params![MAX_SPEC_META + 1, run.id.to_string()],
        )
        .unwrap();
    assert!(
        store
            .read_intruder_spec(run.id)
            .unwrap_err()
            .to_string()
            .contains("元数据类型或长度")
    );

    let mut store = self::store();
    let run = self::run(&mut store, 2);
    store
        .connection
        .execute_batch("PRAGMA ignore_check_constraints=ON")
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE intruder_payloads SET value='text' WHERE run=?1 AND ordinal=0",
            [run.id.to_string()],
        )
        .unwrap();
    assert!(store.read_intruder_spec(run.id).is_err());
    store
        .connection
        .execute(
            "UPDATE intruder_payloads SET value=X'00',ordinal=3 WHERE run=?1 AND ordinal=0",
            [run.id.to_string()],
        )
        .unwrap();
    assert!(store.read_intruder_spec(run.id).is_err());
}

#[test]
fn malformed_result_summaries_are_not_loaded_or_treated_as_other_tool_flows() {
    let mut store = store();
    let run = run(&mut store, 1);
    let request = pending();
    store.put_intruder_result(run.id, 0, Ok(&request)).unwrap();
    let original = serde_json::to_vec(&request.summary).unwrap();
    for value in [
        rusqlite::types::Value::Text("text".into()),
        rusqlite::types::Value::Blob(vec![b' '; MAX_SUMMARY_BYTES + 1]),
    ] {
        store
            .connection
            .execute(
                "UPDATE flows SET summary=?1 WHERE id=?2",
                params![value, request.summary.id.to_string()],
            )
            .unwrap();
        assert!(store.intruder_results(run.id, None, 1).is_err());
        assert!(store.interrupt_intruder_runs().is_err());
        assert_eq!(
            store.read_intruder_run(run.id).unwrap().state,
            IntruderRunState::Running
        );
    }
    store
        .connection
        .execute(
            "UPDATE flows SET summary=?1 WHERE id=?2",
            params![original, request.summary.id.to_string()],
        )
        .unwrap();
    let mut foreign = request.clone();
    foreign.summary.source = TrafficSource::Proxy;
    store
        .connection
        .execute(
            "UPDATE flows SET summary=?1 WHERE id=?2",
            params![
                serde_json::to_vec(&foreign.summary).unwrap(),
                request.summary.id.to_string()
            ],
        )
        .unwrap();
    assert!(store.interrupt_intruder_runs().is_err());
    assert_eq!(store.read(request.summary.id).unwrap(), foreign);
}
