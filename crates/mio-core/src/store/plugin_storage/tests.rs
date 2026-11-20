use tokio_util::sync::CancellationToken;

use super::*;
use crate::{
    AuditFinding, AuditPhase, AuditStateSnapshot, Draft, FlowId, FlowRecord, FlowState,
    HttpResponse, HttpVersion, IntruderRetention, IntruderRunState, IntruderSpec, IntruderTab,
    IssueConfidence, ProjectKey, TrafficSource, Uuid,
    intruder::{AttackType, Position},
    store::{ProjectStore, schema_guard},
};

fn project(store: &ProjectStore) -> ProjectKey {
    ProjectKey {
        id: store.id,
        generation: 1,
    }
}

fn sample_finding(flow_id: Option<FlowId>) -> AuditFinding {
    AuditFinding {
        id: Uuid::new_v4(),
        issue_id: "0x00200300".to_owned(),
        title: "反射告警".to_owned(),
        severity_label: "高".to_owned(),
        severity_rank: 3,
        confidence: IssueConfidence::Firm,
        phase: AuditPhase::Active,
        host: "127.0.0.1".to_owned(),
        url: "http://127.0.0.1/test".to_owned(),
        parameter: Some("q".to_owned()),
        flow_id,
        summary: "摘要".to_owned(),
        evidence: "证据".to_owned(),
        request_highlights: Vec::new(),
        response_highlights: Vec::new(),
        remediation: "修复".to_owned(),
        detected_unix_ms: 1_700_000_000_000,
    }
}

#[test]
fn get_set_overwrite_reopen_and_plugin_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plugin-kv.mio");
    let cancel = CancellationToken::new();
    {
        let mut store = ProjectStore::create(Some(&path)).unwrap();
        let key = project(&store);
        assert_eq!(
            store.plugin_storage_get(key, "plugin.a", "k1").unwrap(),
            None
        );
        store
            .plugin_storage_set(key, "plugin.a", "k1", &[0, 255, 1], &cancel)
            .unwrap();
        store
            .plugin_storage_set(key, "plugin.b", "k1", b"other", &cancel)
            .unwrap();
        assert_eq!(
            store
                .plugin_storage_get(key, "plugin.a", "k1")
                .unwrap()
                .as_deref(),
            Some(&[0, 255, 1][..])
        );
        assert_eq!(
            store.plugin_storage_get(key, "plugin.a", "K1").unwrap(),
            None
        );
        assert_eq!(
            store
                .plugin_storage_get(key, "plugin.b", "k1")
                .unwrap()
                .as_deref(),
            Some(&b"other"[..])
        );
        store
            .plugin_storage_set(key, "plugin.a", "k1", b"updated", &cancel)
            .unwrap();
        store.save(None).unwrap();
        store.close().unwrap();
    }
    let reopened = ProjectStore::open(&path).unwrap();
    let key = project(&reopened);
    assert_eq!(
        reopened
            .plugin_storage_get(key, "plugin.a", "k1")
            .unwrap()
            .as_deref(),
        Some(&b"updated"[..])
    );
    assert_eq!(
        reopened
            .plugin_storage_get(key, "plugin.b", "k1")
            .unwrap()
            .as_deref(),
        Some(&b"other"[..])
    );
    reopened.close().unwrap();
}

#[test]
fn limit_rejections_leave_no_partial_writes() {
    let mut store = ProjectStore::create(None).unwrap();
    let key = project(&store);
    let cancel = CancellationToken::new();

    assert!(
        store
            .plugin_storage_set(key, "", "k", b"v", &cancel)
            .is_err()
    );
    assert!(
        store
            .plugin_storage_set(key, &"p".repeat(129), "k", b"v", &cancel)
            .is_err()
    );
    assert!(
        store
            .plugin_storage_set(key, "bad:id", "k", b"v", &cancel)
            .is_err()
    );
    assert!(
        store
            .plugin_storage_set(key, "p", "", b"v", &cancel)
            .is_err()
    );
    assert!(
        store
            .plugin_storage_set(
                key,
                "p",
                &"k".repeat(MAX_STORAGE_KEY_BYTES + 1),
                b"v",
                &cancel
            )
            .is_err()
    );
    assert!(
        store
            .plugin_storage_set(
                key,
                "p",
                "k",
                &vec![1; MAX_STORAGE_VALUE_BYTES + 1],
                &cancel
            )
            .is_err()
    );

    for idx in 0..MAX_ENTRIES {
        store
            .plugin_storage_set(key, "plugin.cap", &format!("k{idx}"), &[1], &cancel)
            .unwrap();
    }
    let before: Vec<u8> = store
        .connection
        .query_row(
            "SELECT value FROM main.metadata WHERE key='plugin_storage:plugin.cap'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        store
            .plugin_storage_set(key, "plugin.cap", "extra", &[2], &cancel)
            .is_err()
    );
    let after: Vec<u8> = store
        .connection
        .query_row(
            "SELECT value FROM main.metadata WHERE key='plugin_storage:plugin.cap'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(after, before);
    store
        .plugin_storage_set(key, "plugin.cap", "k0", &[9], &cancel)
        .unwrap();

    for idx in 1..MAX_NAMESPACES {
        store
            .plugin_storage_set(key, &format!("ns.{idx}"), "k", &[1], &cancel)
            .unwrap();
    }
    assert!(
        store
            .plugin_storage_set(key, "ns.overflow", "k", &[1], &cancel)
            .is_err()
    );
    assert_eq!(
        store.plugin_storage_get(key, "ns.overflow", "k").unwrap(),
        None
    );
}

fn finished_intruder_two_flows(
    store: &mut ProjectStore,
    run_id: Uuid,
) -> (crate::IntruderRunSummary, FlowRecord, FlowRecord) {
    let spec = IntruderSpec {
        resource_pool: None,
        baseline_request: false,
        revision: 1,
        draft: Draft {
            revision: 1,
            target: "http://127.0.0.1:1".into(),
            raw: b"POST / HTTP/1.1\r\n\r\nX".to_vec(),
            version: HttpVersion::Http11,
        },
        attack_type: AttackType::Sniper,
        positions: vec![Position { start: 19, end: 20 }],
        payload_sets: vec![vec![b"1".to_vec(), b"2".to_vec()]],
        max_concurrent_requests: 1,
        request_timeout_ms: 1000,
        update_host_header: true,
        save_to_project: true,
    };
    let mut ws = store.workspace().unwrap();
    let tab = Uuid::new_v4();
    ws.intruder.tabs.push(IntruderTab {
        id: tab,
        title: "kv-audit-test".into(),
        group: None,
        spec: spec.clone(),
        last_run: None,
    });
    ws.intruder.active = Some(tab);
    ws.revision += 1;
    store.save_workspace(&ws).unwrap();
    store.create_intruder_run(run_id, tab, &spec, 100).unwrap();
    let mut f1 = super::super::tests::record();
    f1.summary.source = TrafficSource::Intruder;
    f1.summary.state = FlowState::Complete;
    f1.summary.status = Some(200);
    f1.summary.response_bytes = 2;
    f1.response = Some(HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![],
        body: b"ok".to_vec(),
        trailers: vec![],
    });
    let mut f2 = f1.clone();
    f2.summary.id = Uuid::new_v4();
    store.put_intruder_result(run_id, 0, Ok(&f1)).unwrap();
    store.put_intruder_result(run_id, 1, Ok(&f2)).unwrap();
    let run = store
        .set_intruder_state(run_id, IntruderRunState::Completed)
        .unwrap();
    (run, f1, f2)
}

#[test]
fn plugin_storage_and_audit_state_coexist_with_intruder_discard_and_pin_flows() {
    let mut store = ProjectStore::create(None).unwrap();
    let key = project(&store);
    let cancel = CancellationToken::new();
    store
        .plugin_storage_set(key, "scanner.helper", "token", b"secret", &cancel)
        .unwrap();

    let (run, pinned_flow, unpinned_flow) = finished_intruder_two_flows(&mut store, Uuid::new_v4());
    store
        .write_audit_state(
            &AuditStateSnapshot {
                revision: 1,
                live_passive_enabled: true,
                live_passive_crawl_enabled: true,
                tasks: Vec::new(),
                findings: vec![sample_finding(Some(pinned_flow.summary.id))],
            },
            &cancel,
        )
        .unwrap();

    let mut next_ws = store.workspace().unwrap();
    next_ws.revision += 1;
    let receipt = store
        .discard_intruder_run(run.id, run.revision, next_ws, &cancel)
        .unwrap();
    assert_eq!((receipt.deleted_flows, receipt.retained_flows), (1, 1));
    assert_eq!(store.read(pinned_flow.summary.id).unwrap(), pinned_flow);
    assert!(
        store
            .read_optional(unpinned_flow.summary.id)
            .unwrap()
            .is_none()
    );

    let (run2, session_pinned, session_unpinned) =
        finished_intruder_two_flows(&mut store, Uuid::new_v4());
    store
        .write_audit_state(
            &AuditStateSnapshot {
                revision: 2,
                live_passive_enabled: true,
                live_passive_crawl_enabled: true,
                tasks: Vec::new(),
                findings: vec![
                    sample_finding(Some(pinned_flow.summary.id)),
                    sample_finding(Some(session_pinned.summary.id)),
                ],
            },
            &cancel,
        )
        .unwrap();
    let mut ws2 = store.workspace().unwrap();
    ws2.revision += 1;
    let moved = store
        .set_intruder_retention(
            key,
            run2.id,
            run2.revision,
            IntruderRetention::Session,
            ws2,
            &cancel,
        )
        .unwrap();
    let mut ws3 = store.workspace().unwrap();
    ws3.revision += 1;
    let receipt2 = store
        .discard_intruder_run(run2.id, moved.run.revision, ws3, &cancel)
        .unwrap();
    assert_eq!((receipt2.deleted_flows, receipt2.retained_flows), (1, 1));
    assert_eq!(
        store.read(session_pinned.summary.id).unwrap(),
        session_pinned
    );
    assert!(
        store
            .read_optional(session_unpinned.summary.id)
            .unwrap()
            .is_none()
    );

    store
        .connection
        .execute(
            "INSERT INTO main.metadata VALUES('plugin_storage:bad/id', X'7b7d')",
            [],
        )
        .unwrap();
    let tx = store.connection.unchecked_transaction().unwrap();
    assert!(schema_guard::validate_intruder_discard(&tx).is_err());
    tx.rollback().unwrap();
    store
        .connection
        .execute(
            "DELETE FROM main.metadata WHERE key='plugin_storage:bad/id'",
            [],
        )
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE main.metadata SET value='corrupt' WHERE key='audit_state'",
            [],
        )
        .unwrap();
    let tx = store.connection.unchecked_transaction().unwrap();
    assert!(schema_guard::validate_intruder_discard(&tx).is_err());
    tx.rollback().unwrap();
}
