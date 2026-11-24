use super::*;

#[test]
fn delete_and_clear_live_proxy_history_preserve_archive_repeater_and_pending_flows() {
    let (mut store, _source, archive_items) = fixture(0, false, false);
    let mut ws = store.workspace().unwrap();
    let effective = ws.effective_proxy_listeners();
    assert_eq!(effective.len(), 1);
    assert_eq!(effective[0].bind_address, "127.0.0.1:8080");
    ws.proxy_listeners.push(crate::ProxyListenerConfig {
        bind_address: "127.0.0.1:8088".into(),
        ..Default::default()
    });
    ws.validate().unwrap();
    assert_eq!(ws.effective_proxy_listeners()[0].bind_address, "127.0.0.1:8088");
    ws.proxy_listeners[0].bind_address = "0.0.0.0:8088".into();
    ws.validate().unwrap();
    ws.proxy_listeners[0].bind_address = "invalid-host:8088".into();
    assert!(ws.validate().is_err());

    let mut proxy_a = crate::store::tests::record();
    proxy_a.summary.source = TrafficSource::Proxy;
    store.put(&proxy_a).unwrap();

    let mut proxy_b = crate::store::tests::record();
    proxy_b.summary.source = TrafficSource::Proxy;
    store.put(&proxy_b).unwrap();

    let mut proxy_pending = crate::store::tests::record();
    proxy_pending.summary.source = TrafficSource::Proxy;
    proxy_pending.summary.state = FlowState::Pending;
    proxy_pending.summary.status = None;
    proxy_pending.response = None;
    store.put(&proxy_pending).unwrap();

    let mut repeater_flow = crate::store::tests::record();
    repeater_flow.summary.source = TrafficSource::Repeater;
    store.put(&repeater_flow).unwrap();

    let mut audit_pinned = crate::store::tests::record();
    audit_pinned.summary.source = TrafficSource::Proxy;
    store.put(&audit_pinned).unwrap();
    store
        .write_audit_state(
            &crate::AuditStateSnapshot {
                revision: 1,
                live_passive_enabled: true,
                live_passive_crawl_enabled: true,
                live_passive_suite_scope_only: false,
                tasks: Vec::new(),
                findings: vec![crate::AuditFinding {
                    id: Uuid::new_v4(),
                    issue_id: "0x00200300".into(),
                    title: "审计引用保护".into(),
                    severity_label: "信息".into(),
                    severity_rank: 0,
                    confidence: crate::IssueConfidence::Firm,
                    phase: crate::AuditPhase::Passive,
                    host: "127.0.0.1".into(),
                    url: audit_pinned.summary.url.clone(),
                    parameter: None,
                    flow_id: Some(audit_pinned.summary.id),
                    summary: "摘要".into(),
                    evidence: "证据".into(),
                    request_highlights: Vec::new(),
                    response_highlights: Vec::new(),
                    remediation: "修复".into(),
                    detected_unix_ms: 1_700_000_000_000,
                }],
            },
            &CancellationToken::new(),
        )
        .unwrap();
    assert!(
        store
            .delete_live_proxy_flows(&[proxy_a.summary.id, repeater_flow.summary.id])
            .is_err()
    );
    assert!(
        store
            .delete_live_proxy_flows(&[proxy_a.summary.id, proxy_pending.summary.id])
            .is_err()
    );
    assert!(store.read_optional(proxy_a.summary.id).unwrap().is_some());

    let deleted = store
        .delete_live_proxy_flows(&[proxy_a.summary.id, proxy_a.summary.id])
        .unwrap();
    assert_eq!(deleted, vec![proxy_a.summary.id]);
    assert!(store.read_optional(proxy_a.summary.id).unwrap().is_none());
    assert!(store.read_optional(proxy_b.summary.id).unwrap().is_some());
    assert!(
        store
            .read_optional(repeater_flow.summary.id)
            .unwrap()
            .is_some()
    );

    let cleared = store.clear_live_proxy_history().unwrap();
    assert!(cleared.contains(&proxy_b.summary.id));
    assert!(cleared.contains(&audit_pinned.summary.id));
    assert_eq!(cleared.len(), 2);
    assert!(store.read_optional(proxy_b.summary.id).unwrap().is_none());
    assert!(store.read_optional(audit_pinned.summary.id).unwrap().is_none());
    let audit_after = store.read_audit_state().unwrap();
    assert_eq!(audit_after.findings.len(), 1);
    assert_eq!(audit_after.findings[0].flow_id, None);
    assert!(
        store
            .read_optional(proxy_pending.summary.id)
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .read_optional(repeater_flow.summary.id)
            .unwrap()
            .is_some()
    );
    assert!(!archive_items.is_empty());
    let (_total, rows) = store.history_page(0, 50, "").unwrap();
    assert!(
        rows.iter()
            .any(|row| matches!(row, HistoryRow::ExtProxy(_)))
    );
}

#[test]
fn delete_live_proxy_history_rejects_invalid_summaries_atomically() {
    for mode in [
        "malformed-json",
        "id-mismatch",
        "unknown-field",
        "summary-pending",
    ] {
        let mut store = ProjectStore::create(None).unwrap();
        let healthy = crate::store::tests::record();
        store.put(&healthy).unwrap();
        let mut corrupt = crate::store::tests::record();
        store.put(&corrupt).unwrap();
        let original_request = corrupt.request.clone();
        corrupt.summary.revision += 1;
        corrupt.request.body = vec![255, 0, 42];
        store.put(&corrupt).unwrap();
        let mut summary = serde_json::to_value(&corrupt.summary).unwrap();
        match mode {
            "id-mismatch" => summary["id"] = json!(Uuid::new_v4()),
            "unknown-field" => summary["future_field"] = json!(true),
            "summary-pending" => summary["state"] = json!(FlowState::Pending),
            _ => {}
        }
        let corrupt_summary = if mode == "malformed-json" {
            b"{".to_vec()
        } else {
            serde_json::to_vec(&summary).unwrap()
        };
        store
            .connection
            .execute(
                "UPDATE flows SET summary=?2 WHERE id=?1",
                params![corrupt.summary.id.to_string(), corrupt_summary],
            )
            .unwrap();

        let error = store
            .delete_live_proxy_flows(&[healthy.summary.id, corrupt.summary.id])
            .unwrap_err();
        let expected_error = match mode {
            "id-mismatch" => "不是可删除的 Proxy",
            "summary-pending" => "仍在传输或拦截中",
            _ => "摘要 JSON 损坏",
        };
        assert!(
            error.to_string().contains(expected_error),
            "{mode}: {error}"
        );
        assert!(store.connection.is_autocommit(), "{mode}");
        assert_eq!(store.read(healthy.summary.id).unwrap(), healthy, "{mode}");
        let saved_summary: Vec<u8> = store
            .connection
            .query_row(
                "SELECT summary FROM flows WHERE id=?1",
                [corrupt.summary.id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(saved_summary, corrupt_summary, "{mode}");
        assert_eq!(
            store.original(corrupt.summary.id).unwrap().request,
            Some(original_request),
            "{mode}"
        );
        store
            .connection
            .execute(
                "UPDATE flows SET summary=?2 WHERE id=?1",
                params![
                    corrupt.summary.id.to_string(),
                    serde_json::to_vec(&corrupt.summary).unwrap()
                ],
            )
            .unwrap();
        assert_eq!(store.read(corrupt.summary.id).unwrap(), corrupt, "{mode}");
        assert_eq!(
            store
                .delete_live_proxy_flows(&[healthy.summary.id, corrupt.summary.id])
                .unwrap(),
            [healthy.summary.id, corrupt.summary.id],
            "{mode}"
        );
        assert_eq!(
            store
                .connection
                .query_row::<usize, _, _>("SELECT count(*) FROM messages", [], |row| row.get(0))
                .unwrap(),
            0,
            "{mode}"
        );
        store.close().unwrap();
    }
}

#[test]
fn clear_live_proxy_history_rejects_invalid_candidates_atomically() {
    for mode in [
        "missing-field",
        "id-mismatch",
        "unknown-field",
        "summary-intercepted",
    ] {
        let mut store = ProjectStore::create(None).unwrap();
        let mut healthy = crate::store::tests::record();
        healthy.summary.started_unix_ms = 20;
        store.put(&healthy).unwrap();
        let corrupt = crate::store::tests::record();
        store.put(&corrupt).unwrap();
        let mut summary = serde_json::to_value(&corrupt.summary).unwrap();
        match mode {
            "missing-field" => {
                summary.as_object_mut().unwrap().remove("request_bytes");
            }
            "id-mismatch" => summary["id"] = json!(Uuid::new_v4()),
            "unknown-field" => summary["future_field"] = json!(true),
            "summary-intercepted" => summary["state"] = json!(FlowState::Intercepted),
            _ => unreachable!(),
        }
        let corrupt_summary = serde_json::to_vec(&summary).unwrap();
        store
            .connection
            .execute(
                "UPDATE flows SET summary=?2 WHERE id=?1",
                params![corrupt.summary.id.to_string(), corrupt_summary],
            )
            .unwrap();

        let error = store.clear_live_proxy_history().unwrap_err();
        let expected_error = match mode {
            "id-mismatch" | "summary-intercepted" => "摘要状态或归属不一致",
            _ => "摘要 JSON 损坏",
        };
        assert!(
            error.to_string().contains(expected_error),
            "{mode}: {error}"
        );
        assert!(store.connection.is_autocommit(), "{mode}");
        assert_eq!(store.read(healthy.summary.id).unwrap(), healthy, "{mode}");
        let saved_summary: Vec<u8> = store
            .connection
            .query_row(
                "SELECT summary FROM flows WHERE id=?1",
                [corrupt.summary.id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(saved_summary, corrupt_summary, "{mode}");
        store
            .connection
            .execute(
                "UPDATE flows SET summary=?2 WHERE id=?1",
                params![
                    corrupt.summary.id.to_string(),
                    serde_json::to_vec(&corrupt.summary).unwrap()
                ],
            )
            .unwrap();
        assert_eq!(store.read(corrupt.summary.id).unwrap(), corrupt, "{mode}");
        assert_eq!(
            store.clear_live_proxy_history().unwrap(),
            [healthy.summary.id, corrupt.summary.id],
            "{mode}"
        );
        assert_eq!(
            store
                .connection
                .query_row::<usize, _, _>("SELECT count(*) FROM messages", [], |row| row.get(0))
                .unwrap(),
            0,
            "{mode}"
        );
        store.close().unwrap();
    }
}

#[test]
fn delete_and_clear_live_proxy_history_reject_session_owned_identity_collisions() {
    let mut store = ProjectStore::create(None).unwrap();
    let spec = crate::IntruderSpec {
        resource_pool: None,
        baseline_request: false,
        revision: 1,
        draft: crate::Draft {
            revision: 1,
            target: "http://127.0.0.1:1".into(),
            raw: b"POST / HTTP/1.1\r\n\r\nX".to_vec(),
            version: HttpVersion::Http11,
        },
        attack_type: crate::intruder::AttackType::Sniper,
        positions: vec![crate::intruder::Position { start: 19, end: 20 }],
        payload_sets: vec![vec![vec![0, 255]]],
        max_concurrent_requests: 1,
        request_timeout_ms: 1000,
        update_host_header: true,
        save_to_project: false,
    };
    let tab_id = Uuid::new_v4();
    let mut workspace = store.workspace().unwrap();
    workspace.revision += 1;
    workspace.intruder.tabs.push(crate::IntruderTab {
        id: tab_id,
        title: "Session deletion guard".into(),
        group: None,
        spec: spec.clone(),
        last_run: None,
    });
    workspace.intruder.active = Some(tab_id);
    store.save_workspace(&workspace).unwrap();
    let run = store
        .create_intruder_run_with_retention(
            crate::ProjectKey {
                id: store.id,
                generation: 1,
            },
            Uuid::new_v4(),
            tab_id,
            &spec,
            50,
        )
        .unwrap();
    let mut session_flow = crate::store::tests::record();
    session_flow.summary.source = TrafficSource::Intruder;
    session_flow.summary.state = FlowState::Pending;
    session_flow.summary.elapsed_ms = 0;
    assert!(
        store
            .preflight_intruder_result(run.id, 0, &session_flow)
            .unwrap()
    );
    store
        .put_intruder_result(run.id, 0, Ok(&session_flow))
        .unwrap();
    session_flow.summary.revision += 1;
    session_flow.summary.state = FlowState::Complete;
    session_flow.summary.status = Some(200);
    session_flow.summary.response_bytes = 3;
    session_flow.response = Some(crate::HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: Vec::new(),
        body: vec![0, 255, 42],
        trailers: Vec::new(),
    });
    store
        .put_intruder_result(run.id, 0, Ok(&session_flow))
        .unwrap();
    let terminal = store
        .set_intruder_state(run.id, crate::IntruderRunState::Completed)
        .unwrap();
    let session_results = store.intruder_results(run.id, None, 10).unwrap();
    let session_runs = store.read_intruder_run(run.id).unwrap();
    assert!(store.is_session_intruder_run(run.id));
    assert_eq!(store.read(session_flow.summary.id).unwrap(), session_flow);

    let mut healthy = crate::store::tests::record();
    healthy.summary.started_unix_ms = 20;
    store.put(&healthy).unwrap();
    let mut collision = healthy.clone();
    collision.summary.id = session_flow.summary.id;
    collision.summary.started_unix_ms = 10;
    assert!(store.put(&collision).is_err());
    let transaction = store.connection.transaction().unwrap();
    crate::store::messages::write_record_with_reason_phrase(&transaction, &collision, None)
        .unwrap();
    transaction.commit().unwrap();

    for selected_delete in [true, false] {
        let error = if selected_delete {
            store
                .delete_live_proxy_flows(&[healthy.summary.id, collision.summary.id])
                .unwrap_err()
        } else {
            store.clear_live_proxy_history().unwrap_err()
        };
        assert!(
            error.to_string().contains("Intruder Session 流量不能"),
            "{error}"
        );
        assert!(store.connection.is_autocommit());
        assert_eq!(store.read(healthy.summary.id).unwrap(), healthy);
        assert_eq!(
            crate::store::read_record(&store.connection, collision.summary.id).unwrap(),
            Some(collision.clone())
        );
        assert_eq!(store.read_intruder_run(run.id).unwrap(), session_runs);
        assert_eq!(
            store
                .test_read_session_flow_raw(run.id, session_flow.summary.id)
                .unwrap(),
            Some(session_flow.clone())
        );
    }
    store
        .connection
        .execute(
            "DELETE FROM flows WHERE id=?1",
            [collision.summary.id.to_string()],
        )
        .unwrap();
    store
        .connection
        .execute(
            "DELETE FROM metadata WHERE key=?1",
            [format!(
                "{}{}",
                crate::store::messages::HTTP_HISTORY_META_PREFIX,
                collision.summary.id
            )],
        )
        .unwrap();
    assert_eq!(store.read(session_flow.summary.id).unwrap(), session_flow);
    assert_eq!(
        store.clear_live_proxy_history().unwrap(),
        [healthy.summary.id]
    );
    assert_eq!(store.read_intruder_run(run.id).unwrap(), terminal);
    assert_eq!(
        store.intruder_results(run.id, None, 10).unwrap(),
        session_results
    );
    store.close().unwrap();
}
