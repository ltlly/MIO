use super::*;
use crate::intruder::MAX_POSITIONS;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

#[test]
fn payload_page_budget_returns_complete_rows_and_remainder_without_gaps_for_both_backings() {
    for session in [false, true] {
        let spec = IntruderSpec {
            revision: 1,
            draft: Draft {
                revision: 1,
                target: "http://127.0.0.1:9".into(),
                raw: vec![b'x'; MAX_PAYLOAD_SETS],
                version: HttpVersion::Http11,
            },
            attack_type: AttackType::Pitchfork,
            positions: (0..MAX_PAYLOAD_SETS)
                .map(|start| Position {
                    start,
                    end: start + 1,
                })
                .collect(),
            payload_sets: vec![
                vec!["🙂".repeat(PREVIEW_CHARACTERS).into_bytes(); 100];
                MAX_PAYLOAD_SETS
            ],
            max_concurrent_requests: 1,
            request_timeout_ms: 1000,
            update_host_header: true,
            save_to_project: !session,
            baseline_request: true,
            resource_pool: None,
        };
        let (store, run) = fixture_spec(spec.clone());
        let connection = store.intruder_connection(run.id).unwrap().0;
        connection.authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Read {
                table_name: "messages",
                ..
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }));
        let ordinals = (0..run.total).rev().collect::<Vec<_>>();
        let mut collected = Vec::new();
        let mut pages = 0;
        while collected.len() < ordinals.len() {
            let rows = store
                .intruder_payload_summaries(
                    run.id,
                    &ordinals[collected.len()..],
                    &CancellationToken::new(),
                )
                .unwrap();
            assert!(
                !rows.is_empty(),
                "a bounded query must make progress or return an explicit error"
            );
            if pages == 0 {
                assert!(
                    rows.len() < ordinals.len(),
                    "this fixture must exceed one page"
                );
            }
            for (index, row) in rows.iter().enumerate() {
                assert_eq!(row.ordinal, ordinals[collected.len() + index]);
                if row.ordinal == 0 {
                    assert!(row.payloads.is_empty());
                } else {
                    assert_eq!(
                        row.payloads.len(),
                        MAX_PAYLOAD_SETS,
                        "distinct sets stay distinct even with identical values"
                    );
                    for (set, payload) in row.payloads.iter().enumerate() {
                        assert_eq!(
                            (
                                payload.selected.position,
                                payload.selected.set_index,
                                payload.selected.payload_index
                            ),
                            (set, set, row.ordinal as usize - 1)
                        );
                        assert_eq!(payload.raw_bytes, 1024);
                        assert_eq!(
                            payload.preview_bytes,
                            spec.payload_sets[set][row.ordinal as usize - 1]
                        );
                    }
                }
            }
            collected.extend(rows.into_iter().map(|row| row.ordinal));
            pages += 1;
            assert!(
                pages <= 4,
                "small previews must not degenerate into one-result pages"
            );
        }
        assert_eq!(collected, ordinals);
        assert!(pages > 1);
        connection.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        assert_eq!(store.read_intruder_spec(run.id).unwrap(), spec);
        assert_eq!(
            store.read_intruder_run(run.id).unwrap().retention,
            if session {
                IntruderRetention::Session
            } else {
                IntruderRetention::Project
            }
        );
        store.close().unwrap();
    }
}

#[test]
fn connection_payload_query_allows_one_off_page_selection_without_relaxing_public_page_limit() {
    let (store, run) = fixture(false, vec![b"x".to_vec(); 501], 1, false);
    let ordinals = (0..run.total).rev().collect::<Vec<_>>();
    assert!(
        store
            .intruder_payload_summaries(run.id, &ordinals, &CancellationToken::new())
            .is_err()
    );
    let connection = store.intruder_connection(run.id).unwrap().0;
    let transaction = connection.unchecked_transaction().unwrap();
    let rows = payload_summaries(&transaction, &run, &ordinals, &|| Ok(())).unwrap();
    assert_eq!(rows.len(), 501);
    assert_eq!(
        rows.iter().map(|row| row.ordinal).collect::<Vec<_>>(),
        ordinals
    );
    assert!(
        rows.iter()
            .all(|row| row.payloads.len() == 1 && row.payloads[0].preview_bytes == b"x")
    );
    assert!(
        !transaction.is_autocommit(),
        "the helper must not finish its caller's snapshot"
    );
    transaction.commit().unwrap();
    store.close().unwrap();
}

#[test]
fn preview_keeps_incomplete_binary_input_but_does_not_split_a_truncated_unicode_character() {
    assert_eq!(preview(vec![0xf0, 0x9f], true), [0xf0, 0x9f]);
    let bytes = [vec![b'a'; PREVIEW_CHARACTERS - 1], vec![0xf0, 0x9f]].concat();
    assert_eq!(preview(bytes, false), vec![b'a'; PREVIEW_CHARACTERS - 1]);
    assert_eq!(
        preview("🙂".repeat(257).into_bytes(), true),
        "🙂".repeat(256).into_bytes()
    );
}

#[test]
fn payload_summaries_reject_a_moved_initial_baseline_instead_of_relabelling_payloads() {
    let (mut store, run) = fixture(false, vec![b"A".to_vec(), b"B".to_vec()], 1, true);
    store
        .put_intruder_result(run.id, 0, Err("baseline fixture"))
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE intruder_runs SET baseline_ordinal=1 WHERE id=?1",
            [run.id.to_string()],
        )
        .unwrap();
    assert!(
        store.read_intruder_run(run.id).is_ok(),
        "the lifecycle itself permits a late baseline"
    );
    assert!(store.read_intruder_spec(run.id).is_err());
    assert!(
        store
            .intruder_payload_summaries(run.id, &[0], &CancellationToken::new())
            .unwrap_err()
            .to_string()
            .contains("组合数量与运行不一致")
    );
    let baseline: u64 = store
        .connection
        .query_row(
            "SELECT baseline_ordinal FROM intruder_runs WHERE id=?1",
            [run.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        baseline, 1,
        "a failed read must not repair or rewrite source metadata"
    );
    store.close().unwrap();
}

#[test]
fn payload_order_compares_full_blobs_beyond_preview_and_handles_all_attack_types() {
    let mut prefix_b = "中🙂".repeat(300).into_bytes();
    let mut prefix_a = prefix_b.clone();
    prefix_a.push(b'a');
    prefix_b.push(b'b');
    assert_eq!(
        preview(prefix_a.clone(), false),
        preview(prefix_b.clone(), false)
    );
    let (store, run) = fixture(
        false,
        vec![
            prefix_b,
            vec![0xff, 0x00],
            prefix_a,
            Vec::new(),
            vec![0x00, 0xff],
        ],
        MAX_POSITIONS,
        true,
    );
    let connection = store.intruder_connection(run.id).unwrap().0;
    let transaction = connection.unchecked_transaction().unwrap();
    let order = payload_order(&transaction, &run, &|| Ok(())).unwrap();
    let mut asc = (0..run.total).collect::<Vec<_>>();
    asc.sort_by(|&l, &r| order.compare(l, r, true).unwrap().then_with(|| l.cmp(&r)));
    assert_eq!(asc, [0, 4, 5, 3, 1, 2]);
    let mut desc = (0..run.total).collect::<Vec<_>>();
    desc.sort_by(|&l, &r| order.compare(l, r, false).unwrap().then_with(|| l.cmp(&r)));
    assert_eq!(desc, [2, 1, 3, 5, 4, 0]);
    transaction.commit().unwrap();
    store.close().unwrap();

    let (cluster_store, cluster_run) = fixture_spec(IntruderSpec {
        revision: 1,
        draft: Draft {
            revision: 1,
            target: "http://127.0.0.1:9".into(),
            raw: b"ab".to_vec(),
            version: HttpVersion::Http11,
        },
        attack_type: AttackType::ClusterBomb,
        positions: vec![Position { start: 0, end: 1 }, Position { start: 1, end: 2 }],
        payload_sets: vec![
            vec![b"b".to_vec(), b"a".to_vec()],
            vec![b"2".to_vec(), b"1".to_vec()],
        ],
        max_concurrent_requests: 1,
        request_timeout_ms: 1000,
        update_host_header: true,
        save_to_project: true,
        baseline_request: true,
        resource_pool: None,
    });
    let connection = cluster_store.intruder_connection(cluster_run.id).unwrap().0;
    let transaction = connection.unchecked_transaction().unwrap();
    let order = payload_order(&transaction, &cluster_run, &|| Ok(())).unwrap();
    let mut asc = (0..cluster_run.total).collect::<Vec<_>>();
    asc.sort_by(|&l, &r| order.compare(l, r, true).unwrap().then_with(|| l.cmp(&r)));
    assert_eq!(asc, [0, 4, 3, 2, 1]);
    assert!(payload_order(&transaction, &cluster_run, &|| bail!("cancelled")).is_err());
    transaction.commit().unwrap();
    cluster_store.close().unwrap();

    let (pitchfork_store, pitchfork_run) = fixture_spec(IntruderSpec {
        revision: 1,
        draft: Draft {
            revision: 1,
            target: "http://127.0.0.1:9".into(),
            raw: b"ab".to_vec(),
            version: HttpVersion::Http11,
        },
        attack_type: AttackType::Pitchfork,
        positions: vec![Position { start: 0, end: 1 }, Position { start: 1, end: 2 }],
        payload_sets: vec![
            vec![b"x".to_vec(), b"a".to_vec(), b"x".to_vec()],
            vec![b"2".to_vec(), b"9".to_vec(), b"1".to_vec()],
        ],
        max_concurrent_requests: 1,
        request_timeout_ms: 1000,
        update_host_header: true,
        save_to_project: true,
        baseline_request: false,
        resource_pool: None,
    });
    let connection = pitchfork_store
        .intruder_connection(pitchfork_run.id)
        .unwrap()
        .0;
    let transaction = connection.unchecked_transaction().unwrap();
    let order = payload_order(&transaction, &pitchfork_run, &|| Ok(())).unwrap();
    let mut asc = (0..pitchfork_run.total).collect::<Vec<_>>();
    asc.sort_by(|&l, &r| order.compare(l, r, true).unwrap().then_with(|| l.cmp(&r)));
    assert_eq!(asc, [1, 2, 0]);
    transaction.commit().unwrap();
    pitchfork_store.close().unwrap();
}
