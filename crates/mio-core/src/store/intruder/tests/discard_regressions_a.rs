use super::*;

#[test]
fn regression_discard_preserves_nocase_parent_websocket_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nocase-parent.mio");
    ProjectStore::create(Some(&path)).unwrap().close().unwrap();
    let schema = Connection::open(&path).unwrap();
    schema
        .execute_batch(&format!(
            "PRAGMA foreign_keys=ON;
         DROP TABLE flows;
         CREATE TABLE flows (
             id TEXT PRIMARY KEY COLLATE NOCASE,revision INTEGER NOT NULL,started INTEGER NOT NULL,
             url TEXT NOT NULL,method TEXT NOT NULL,state TEXT NOT NULL,summary BLOB NOT NULL,
             {},{},{},{},{},{},{});
         CREATE INDEX flows_started ON flows(started DESC,id);
         CREATE INDEX flows_state ON flows(state);
         CREATE INDEX flows_source_started ON flows(source,started DESC,id);
         CREATE INDEX flows_source_status ON flows(source,status);
         CREATE INDEX flows_source_capture ON flows(source,capture_number);",
            crate::store::FLOW_LISTENER_PORT_COLUMN,
            crate::store::FLOW_SOURCE_COLUMN,
            crate::store::FLOW_STATUS_COLUMN,
            crate::store::FLOW_RESPONSE_BYTES_COLUMN,
            crate::store::FLOW_ELAPSED_MS_COLUMN,
            crate::store::FLOW_CAPTURE_NUMBER_COLUMN,
            crate::store::FLOW_RESPONSE_LENGTH_COLUMN,
        ))
        .unwrap();
    schema.close().unwrap();
    let mut store = ProjectStore::open(&path).expect("ordinary NOCASE parent schema is readable");
    let attack = run(&mut store, 1);
    let mut flow = complete(&pending());
    flow.summary.id = Uuid::from_u128(0xabcdef);
    store.put_intruder_result(attack.id, 0, Ok(&flow)).unwrap();
    let attack = store
        .set_intruder_state(attack.id, IntruderRunState::Completed)
        .unwrap();
    let message = crate::WebSocketMessage {
        id: Uuid::new_v4(),
        revision: 1,
        flow_id: flow.summary.id,
        sequence: 1,
        direction: crate::WebSocketDirection::ServerToClient,
        kind: crate::WebSocketKind::Binary,
        payload: vec![0, 255, b'W'],
        observed_unix_ms: 1,
    };
    store.put_websocket(&message).unwrap();
    let uppercase = flow.summary.id.to_string().to_ascii_uppercase();
    assert_ne!(uppercase, flow.summary.id.to_string());
    assert_eq!(
        store
            .connection
            .execute(
                "UPDATE websocket_messages SET flow=?1 WHERE id=?2",
                params![uppercase, message.id.to_string()],
            )
            .unwrap(),
        1
    );
    assert!(
        !store
            .connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
    store.close().unwrap();

    let mut store =
        ProjectStore::open(&path).expect("FK-valid mixed-case reference must reach discard");
    assert_eq!(store.read(flow.summary.id).unwrap(), flow);
    assert_eq!(store.read_websocket(message.id).unwrap(), message);
    let before_page = store.websocket_page(None, 0, 1, &[]).unwrap();
    assert_eq!(before_page.0, 1);
    assert_eq!(before_page.1[0].flow_id, flow.summary.id);
    assert_eq!(before_page.1[0].connection_id, 1);
    assert_eq!(
        store
            .websocket_page(Some(flow.summary.id), 0, 1, &[])
            .unwrap(),
        before_page
    );
    let child_match: bool = store
        .connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM websocket_messages WHERE flow=?1)",
            [flow.summary.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        !child_match,
        "the fixture must expose child-side BINARY comparison"
    );
    let before_workspace = discard_workspace_bytes(&store.connection);
    let proposal = discard_proposal(&store);
    let result = store.discard_intruder_run(
        attack.id,
        attack.revision,
        proposal,
        &CancellationToken::new(),
    );
    let after_flow = store.read_optional(flow.summary.id).unwrap();
    let after_message = store.read_websocket(message.id).ok();
    let after_payload: Option<Vec<u8>> = store
        .connection
        .query_row(
            "SELECT body FROM websocket_payloads WHERE message=?1",
            [message.id.to_string()],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    let after_page = store.websocket_page(None, 0, 1, &[]).ok();
    let after_run = store.find_intruder_run(attack.id).unwrap();
    let after_workspace = discard_workspace_bytes(&store.connection);
    let marker = retired_run(&store.connection, attack.id).unwrap();
    store.close().unwrap();

    assert_eq!(
        after_flow,
        Some(flow),
        "discard must not cascade-delete a NOCASE-referenced flow"
    );
    assert_eq!(after_payload.as_deref(), Some(message.payload.as_slice()));
    assert_eq!(after_message, Some(message));
    assert_eq!(after_page, Some(before_page));
    match result {
        Ok(receipt) => {
            assert_eq!((receipt.deleted_flows, receipt.retained_flows), (0, 1));
            assert!(after_run.is_none());
        }
        Err(_) => {
            assert_eq!(after_run, Some(attack));
            assert_eq!(after_workspace, before_workspace);
            assert!(marker.is_none());
        }
    }
}

#[test]
fn regression_discard_refuses_unknown_workspace_scalar_without_rewriting_blob() {
    let mut store = store();
    let attack = run(&mut store, 1);
    let flow = complete(&pending());
    store.put_intruder_result(attack.id, 0, Ok(&flow)).unwrap();
    let attack = store
        .set_intruder_state(attack.id, IntruderRunState::Completed)
        .unwrap();
    let mut json: serde_json::Value =
        serde_json::from_slice(&discard_workspace_bytes(&store.connection)).unwrap();
    json.as_object_mut().unwrap().insert(
        "unrelated_scalar_sentinel".into(),
        serde_json::json!("retain exactly"),
    );
    let original = serde_json::to_vec(&json).unwrap();
    store
        .connection
        .execute(
            "UPDATE main.metadata SET value=?1 WHERE key='workspace'",
            [&original],
        )
        .unwrap();
    assert_eq!(discard_workspace_bytes(&store.connection), original);
    store
        .workspace()
        .expect("unknown scalar does not invalidate known workspace fields")
        .validate()
        .unwrap();
    let proposal = discard_proposal(&store);
    let result = store.discard_intruder_run(
        attack.id,
        attack.revision,
        proposal,
        &CancellationToken::new(),
    );
    let after_workspace = discard_workspace_bytes(&store.connection);
    let after_run = store.find_intruder_run(attack.id).unwrap();
    let after_flow = store.read_optional(flow.summary.id).unwrap();
    let marker = retired_run(&store.connection, attack.id).unwrap();
    store.close().unwrap();

    assert!(
        result.is_err(),
        "discard must refuse a workspace rewrite that loses an unrelated unknown scalar"
    );
    assert_eq!(
        after_workspace, original,
        "rejection must preserve the exact workspace BLOB"
    );
    assert_eq!(after_run, Some(attack));
    assert_eq!(after_flow, Some(flow));
    assert!(marker.is_none());
}
