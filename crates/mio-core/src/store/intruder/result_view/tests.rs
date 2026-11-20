use super::*;
use crate::intruder::MAX_POSITIONS;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

mod order_and_budget;

fn fixture(
    session: bool,
    payloads: Vec<Vec<u8>>,
    positions: usize,
    baseline: bool,
) -> (ProjectStore, IntruderRunSummary) {
    fixture_spec(IntruderSpec {
        revision: 1,
        draft: Draft {
            revision: 1,
            target: "http://127.0.0.1:9".into(),
            raw: vec![b'x'; positions],
            version: HttpVersion::Http11,
        },
        attack_type: if positions == 1 {
            AttackType::Sniper
        } else {
            AttackType::BatteringRam
        },
        positions: (0..positions)
            .map(|start| Position {
                start,
                end: start + 1,
            })
            .collect(),
        payload_sets: vec![payloads],
        max_concurrent_requests: 1,
        request_timeout_ms: 1000,
        update_host_header: true,
        save_to_project: !session,
        baseline_request: baseline,
        resource_pool: None,
    })
}

fn fixture_spec(spec: IntruderSpec) -> (ProjectStore, IntruderRunSummary) {
    let mut store = ProjectStore::create(None).unwrap();
    let tab = Uuid::new_v4();
    let mut workspace = store.workspace().unwrap();
    workspace.revision += 1;
    workspace.intruder.active = Some(tab);
    workspace.intruder.tabs.push(crate::IntruderTab {
        id: tab,
        title: "payload fixture".into(),
        group: None,
        spec: spec.clone(),
        last_run: None,
    });
    store.save_workspace(&workspace).unwrap();
    let project = ProjectKey {
        id: store.id,
        generation: 1,
    };
    let run = store
        .create_intruder_run_with_retention(project, Uuid::new_v4(), tab, &spec, 10)
        .unwrap();
    (store, run)
}

#[test]
fn payload_summaries_are_bounded_binary_source_values_and_keep_baseline_identity() {
    let long = "中🙂".repeat(400).into_bytes();
    let binary = vec![0xff, 0, b'a'];
    for session in [false, true] {
        let (mut store, run) = fixture(
            session,
            vec![long.clone(), binary.clone(), Vec::new()],
            1,
            true,
        );
        let original = store.read_intruder_spec(run.id).unwrap();
        for ordinal in 0..run.total {
            store
                .put_intruder_result(run.id, ordinal, Err("not sent fixture"))
                .unwrap();
        }
        store
            .set_intruder_state(run.id, IntruderRunState::Completed)
            .unwrap();
        let connection = store.intruder_connection(run.id).unwrap().0;
        connection.authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Read {
                table_name: "messages",
                ..
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }));
        let summaries = store
            .intruder_payload_summaries(run.id, &[3, 0, 1, 2], &CancellationToken::new())
            .unwrap();
        connection.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        assert_eq!(
            summaries.iter().map(|row| row.ordinal).collect::<Vec<_>>(),
            [3, 0, 1, 2]
        );
        assert!(summaries[1].payloads.is_empty());
        assert_eq!(summaries[0].payloads[0].raw_bytes, 0);
        assert!(summaries[0].payloads[0].preview_bytes.is_empty());
        let long_summary = &summaries[2].payloads[0];
        assert_eq!(long_summary.raw_bytes, long.len() as u64);
        assert_eq!(
            std::str::from_utf8(&long_summary.preview_bytes)
                .unwrap()
                .chars()
                .count(),
            PREVIEW_CHARACTERS
        );
        assert!(long.starts_with(&long_summary.preview_bytes));
        assert_eq!(summaries[3].payloads[0].preview_bytes, binary);
        assert_eq!(summaries[3].payloads[0].raw_bytes, 3);
        assert_eq!(
            summaries[3].payloads[0].selected,
            PayloadPosition {
                position: 0,
                set_index: 0,
                payload_index: 1
            }
        );
        assert_eq!(store.read_intruder_spec(run.id).unwrap(), original);
        assert_eq!(
            store.read_intruder_run(run.id).unwrap().retention,
            if session {
                IntruderRetention::Session
            } else {
                IntruderRetention::Project
            }
        );
        if !session {
            let directory = tempfile::tempdir().unwrap();
            let saved = directory.path().join("payload-results.mio");
            store.save(Some(&saved)).unwrap();
            let reopened = ProjectStore::open(&saved).unwrap();
            assert_eq!(
                reopened
                    .intruder_payload_summaries(run.id, &[3, 0, 1, 2], &CancellationToken::new())
                    .unwrap(),
                summaries
            );
            assert_eq!(reopened.read_intruder_spec(run.id).unwrap(), original);
            reopened.close().unwrap();
        }
        store.close().unwrap();
    }
}

#[test]
fn payload_summary_cancel_and_corruption_never_return_partial_inputs() {
    let (store, run) = fixture(false, vec![b"first".to_vec(), b"second".to_vec()], 1, false);
    let before = store.read_intruder_spec(run.id).unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    store.connection.authorizer(Some({
        let reads = reads.clone();
        move |context: AuthContext<'_>| {
            if matches!(context.action, AuthAction::Read { .. }) {
                reads.fetch_add(1, Ordering::SeqCst);
            }
            Authorization::Allow
        }
    }));
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        store
            .intruder_payload_summaries(run.id, &[0], &cancel)
            .is_err()
    );
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    store
        .connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    assert!(
        store
            .intruder_payload_summaries(run.id, &[0, 0], &CancellationToken::new())
            .is_err()
    );
    assert!(
        store
            .intruder_payload_summaries(run.id, &[2], &CancellationToken::new())
            .is_err()
    );
    assert!(
        store
            .intruder_payload_summaries(run.id, &[], &CancellationToken::new())
            .is_err()
    );
    assert_eq!(store.read_intruder_spec(run.id).unwrap(), before);
    store
        .connection
        .execute(
            "DELETE FROM intruder_payloads WHERE run=?1 AND ordinal=0",
            [run.id.to_string()],
        )
        .unwrap();
    assert!(
        store
            .intruder_payload_summaries(run.id, &[1], &CancellationToken::new())
            .unwrap_err()
            .to_string()
            .contains("位置缺失")
    );
    store.close().unwrap();
}

#[test]
fn battering_ram_payload_preview_deduplicates_input_identity_without_changing_request_mapping() {
    for session in [false, true] {
        let (store, run) = fixture(
            session,
            vec![vec![0xff, 0, b'a'], Vec::new()],
            MAX_POSITIONS,
            true,
        );
        let before = store.read_intruder_spec(run.id).unwrap();
        let plan = crate::intruder::AttackPlan::new(
            before.attack_type,
            before.draft.raw.clone(),
            before.positions.clone(),
            before.payload_sets.clone(),
        )
        .unwrap();
        assert_eq!(
            plan.payload_positions(1, Some(0)).unwrap().len(),
            MAX_POSITIONS
        );
        let rows = store
            .intruder_payload_summaries(run.id, &[0, 1, 2], &CancellationToken::new())
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert!(
            rows[0].payloads.is_empty(),
            "the baseline has no selected payload"
        );
        assert_eq!(
            rows[1].payloads.len(),
            1,
            "one payload set is shown once regardless of position count"
        );
        assert_eq!(
            rows[1].payloads[0].selected,
            PayloadPosition {
                position: 0,
                set_index: 0,
                payload_index: 0
            }
        );
        assert_eq!(rows[1].payloads[0].preview_bytes, [0xff, 0, b'a']);
        assert_eq!(rows[1].payloads[0].raw_bytes, 3);
        assert_eq!(
            rows[2].payloads.len(),
            1,
            "an empty selected payload is not a baseline"
        );
        assert_eq!(rows[2].payloads[0].raw_bytes, 0);
        assert!(rows[2].payloads[0].preview_bytes.is_empty());
        assert_eq!(
            plan.request_with_baseline(1, Some(0)).unwrap(),
            [0xff, 0, b'a'].repeat(MAX_POSITIONS)
        );
        assert_eq!(store.read_intruder_spec(run.id).unwrap(), before);
        store.close().unwrap();
    }
}
