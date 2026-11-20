use super::*;

#[test]
fn ids_revisions_sequences_and_orphans_cannot_silently_replace_messages() {
    let mut store = ProjectStore::create(None).unwrap();
    let flow = flow_record();
    store.put(&flow).unwrap();
    let original = message(flow.summary.id, 1);
    store.put_websocket(&original).unwrap();
    for revision in [0, 2, u64::MAX] {
        let mut conflict = original.clone();
        conflict.revision = revision;
        assert!(store.put_websocket(&conflict).is_err());
        conflict.id = Uuid::new_v4();
        assert!(store.put_websocket(&conflict).is_err());
    }
    for sequence in [0, u64::MAX] {
        let mut invalid = message(flow.summary.id, sequence);
        assert!(store.put_websocket(&invalid).is_err());
        invalid.id = original.id;
        assert!(store.put_websocket(&invalid).is_err());
    }
    let mut conflict = original.clone();
    conflict.payload.push(1);
    assert!(store.put_websocket(&conflict).is_err());
    conflict = original.clone();
    conflict.observed_unix_ms += 1;
    assert!(store.put_websocket(&conflict).is_err());
    conflict = original.clone();
    conflict.direction = WebSocketDirection::ServerToClient;
    assert!(store.put_websocket(&conflict).is_err());
    conflict = original.clone();
    conflict.kind = WebSocketKind::Ping;
    assert!(store.put_websocket(&conflict).is_err());
    assert!(store.put_websocket(&message(flow.summary.id, 1)).is_err());
    assert!(store.put_websocket(&message(Uuid::new_v4(), 2)).is_err());
    assert_eq!(store.read_websocket(original.id).unwrap(), original);
    assert_eq!(store.websocket_page(None, 0, 500, &[]).unwrap().0, 1);
    let payloads: usize = store
        .connection
        .query_row("SELECT count(*) FROM websocket_payloads", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(payloads, 1);
    let largest_sequence = message(flow.summary.id, i64::MAX as u64);
    store.put_websocket(&largest_sequence).unwrap();
    assert_eq!(
        store.read_websocket(largest_sequence.id).unwrap(),
        largest_sequence
    );
}

#[test]
fn pagination_and_body_limits_are_bounded_and_never_truncate_corrupt_payloads() {
    let mut store = ProjectStore::create(None).unwrap();
    let flow = flow_record();
    store.put(&flow).unwrap();
    assert!(store.websocket_page(None, 0, 0, &[]).is_err());
    assert!(store.websocket_page(None, 0, 100_001, &[]).is_err());
    assert!(store.websocket_page(None, usize::MAX, 1, &[]).is_err());
    assert!(
        store
            .websocket_page(None, i64::MAX as usize, 1, &[])
            .is_err()
    );
    assert!(
        store
            .websocket_page(None, 0, 1, &[Uuid::nil(); MAX_EXCLUDED_IDS + 1])
            .is_err()
    );
    assert_eq!(
        store
            .websocket_page(None, 0, 1, &[Uuid::nil(); MAX_EXCLUDED_IDS])
            .unwrap()
            .0,
        0
    );
    assert!(store.flow_url(Uuid::new_v4()).is_err());
    assert!(store.read_websocket(Uuid::new_v4()).is_err());
    let record = message(flow.summary.id, 1);
    {
        let mut oversized = record.clone();
        oversized.payload.resize(MAX_BODY + 1, 0);
        assert!(store.put_websocket(&oversized).is_err());
    }
    assert_eq!(store.websocket_page(None, 0, 1, &[]).unwrap().0, 0);
    store.put_websocket(&record).unwrap();
    store
        .connection
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    store
        .connection
        .execute(
            "UPDATE websocket_payloads SET body=zeroblob(?1)",
            [MAX_BODY as i64 + 1],
        )
        .unwrap();
    assert!(
        store
            .read_websocket(record.id)
            .unwrap_err()
            .to_string()
            .contains("16 MiB")
    );
    assert_eq!(
        store.websocket_page(None, 0, 1, &[]).unwrap().1[0].payload_bytes,
        2
    );
    store
        .connection
        .execute("UPDATE websocket_payloads SET body=x'0001ff'", [])
        .unwrap();
    assert!(
        store
            .read_websocket(record.id)
            .unwrap_err()
            .to_string()
            .contains("长度")
    );
    store
        .connection
        .execute("UPDATE websocket_payloads SET body='xx'", [])
        .unwrap();
    assert!(
        store
            .read_websocket(record.id)
            .unwrap_err()
            .to_string()
            .contains("二进制")
    );
    store
        .connection
        .execute("UPDATE websocket_payloads SET body=?1", ["\0text"])
        .unwrap();
    assert!(
        store
            .read_websocket(record.id)
            .unwrap_err()
            .to_string()
            .contains("二进制"),
        "TEXT must be rejected before interpreting its character count as a byte length"
    );
    store
        .connection
        .execute("DELETE FROM websocket_payloads", [])
        .unwrap();
    assert!(
        store
            .read_websocket(record.id)
            .unwrap_err()
            .to_string()
            .contains("缺失")
    );
}

#[test]
fn recovery_and_save_as_commit_http_websocket_and_workspace_as_one_retryable_batch() {
    for save_as in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.mio");
        let target = directory.path().join("target.mio");
        let mut store = ProjectStore::create(Some(&source)).unwrap();
        let original_flow = flow_record();
        store.put(&original_flow).unwrap();
        let original = message(original_flow.summary.id, 1);
        store.put_websocket(&original).unwrap();
        let before = store.workspace().unwrap();
        let mut workspace = before.clone();
        workspace.open_request(None).unwrap();
        let mut added_flow = flow_record();
        added_flow.summary.state = FlowState::Pending;
        let added = message(added_flow.summary.id, 1);
        let mut conflicting = original.clone();
        conflicting.payload.push(42);
        let destination = save_as.then_some(target.as_path());
        assert!(
            store
                .save_with(
                    destination,
                    Some(&workspace),
                    &[added_flow.clone().into()],
                    &[added.clone().into(), conflicting.into()],
                    &[],
                    &CancellationToken::new(),
                )
                .is_err()
        );
        assert_eq!(store.path.as_deref(), Some(source.as_path()));
        assert_eq!(store.workspace().unwrap(), before);
        assert_eq!(store.read(original_flow.summary.id).unwrap(), original_flow);
        assert_eq!(store.read_websocket(original.id).unwrap(), original);
        assert!(store.read(added_flow.summary.id).is_err());
        assert!(store.read_websocket(added.id).is_err());
        assert!(!target.exists());

        store
            .save_with(
                destination,
                Some(&workspace),
                &[added_flow.clone().into()],
                &[added.clone().into(), original.clone().into()],
                &[],
                &CancellationToken::new(),
            )
            .unwrap();
        added_flow.summary.revision += 1;
        added_flow.summary.state = FlowState::Cancelled;
        assert_eq!(store.workspace().unwrap(), workspace);
        assert_eq!(store.read(added_flow.summary.id).unwrap(), added_flow);
        assert_eq!(store.read_websocket(added.id).unwrap(), added);
        let saved_path = store.path.clone().unwrap();
        assert_eq!(saved_path, if save_as { target } else { source.clone() });
        drop(store);
        let reopened = ProjectStore::open(&saved_path).unwrap();
        assert_eq!(reopened.workspace().unwrap(), workspace);
        assert_eq!(reopened.read_websocket(added.id).unwrap(), added);
        assert_eq!(reopened.read_websocket(original.id).unwrap(), original);
        if save_as {
            let source = ProjectStore::open(&source).unwrap();
            assert_eq!(source.workspace().unwrap(), before);
            assert!(source.read(added_flow.summary.id).is_err());
            assert!(source.read_websocket(added.id).is_err());
            assert_eq!(source.read_websocket(original.id).unwrap(), original);
        }
    }
}
