use super::*;
use serde_json::json;

mod notes_batch;

fn fixture() -> (State, mpsc::Receiver<Event>, Uuid, Vec<ExtItem>) {
    let (events, receiver) = mpsc::channel(64);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let source = Uuid::new_v4();
    let mut report = ExtImportReport {
        schema_floor: 229,
        schema_revision: 229,
        source_bytes: 1024,
        indexed_items: 0,
        indexed_messages: 0,
        collections: vec![],
        repeater_active_tab: None,
        gaps: vec![],
        complete_archive: false,
    };
    state.store.begin_source(source, &report).unwrap();
    state
        .store
        .append_source_chunk(source, 0, &[0; 1024])
        .unwrap();
    let items: Vec<_> = (0..2)
        .map(|index| {
            let id = Uuid::new_v4();
            let address = 100 + index;
            let binding = json!({"source_id":source,"item_id":id});
            let metadata = json!({"version":1,"context":"http_history_229","slot":{"state":"value","value":address},
                "record":{"object":{"address":address,"object_type":1,"subtype":0},"mapping":{"state":"value","value":{
                    "highlight":{"id":13,"binding":binding,"observation":{"state":"value","value":3}},
                    "annotations":{"id":27,"binding":binding,"observation":{"state":"null"}}
                }}}});
            ExtItem {
                id,
                source_id: source,
                collection: "root.f1.f0".into(),
                index,
                object_address: Some(address),
                state: "mapped".into(),
                metadata_json: serde_json::to_vec(&metadata).unwrap(),
            }
        })
        .collect();
    state.store.put_ext_batch(&items, &[]).unwrap();
    report.indexed_items = 2;
    report.collections.push(ExtCollectionReport {
        path: "root.f1.f0".into(),
        logical_items: Some(2),
        indexed_items: 2,
        message_count: 0,
        state: "available".into(),
    });
    report.complete_archive = true;
    state.store.finish_source(source, &report).unwrap();
    (state, receiver, source, items)
}

fn changes(
    source: Uuid,
    items: &[ExtItem],
    revision: u64,
    color: u8,
) -> Vec<ExtHttpHighlightChange> {
    items
        .iter()
        .map(|item| ExtHttpHighlightChange {
            source_id: source,
            item_id: item.id,
            expected_highlight_revision: revision,
            color,
        })
        .collect()
}

fn drain(receiver: &mut mpsc::Receiver<Event>) -> Vec<Event> {
    let mut events = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        if !matches!(event, Event::ProjectDiskUsage { .. }) {
            events.push(event);
        }
    }
    events
}

#[tokio::test]
async fn highlight_commands_correlate_batches_and_reject_stale_cancelled_or_invalid_operations() {
    let (mut state, mut receiver, source, items) = fixture();
    let owner = state.project;
    let mut live = super::tests::completed_flow();
    live.summary.source = TrafficSource::Proxy;
    state.store.put(&live).unwrap();
    let live_id = live.summary.id;
    let operation = Uuid::new_v4();
    state
        .command(Command::SetExtHttpHighlights {
            project: owner,
            operation_id: operation,
            changes: changes(source, &items, 0, 1),
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    let events = drain(&mut receiver);
    let [
        Event::ExtHttpHighlightsChanged {
            project,
            operation_id,
            result: Ok(saved),
        },
        Event::HistoryChanged {
            project: history_owner,
        },
    ] = events.as_slice()
    else {
        panic!("{events:?}")
    };
    assert_eq!(
        (*project, *operation_id, *history_owner),
        (owner, operation, owner)
    );
    assert_eq!(
        saved,
        &items
            .iter()
            .map(|item| (
                source,
                item.id,
                ExtHttpHighlight {
                    revision: 1,
                    color: 1
                }
            ))
            .collect::<Vec<_>>()
    );
    // Verify Live Proxy flow notes + highlights via source_id == Uuid::nil()
    let live_note_op = Uuid::new_v4();
    state
        .command(Command::SetExtHttpNotes {
            project: owner,
            operation_id: live_note_op,
            change: ExtHttpNotesChange {
                source_id: Uuid::nil(),
                item_id: live_id,
                expected_notes_revision: 0,
                text: "**Live** 备注".into(),
            },
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    drain(&mut receiver);
    assert_eq!(
        state
            .store
            .ext_http_notes(owner, Uuid::nil(), live_id)
            .unwrap(),
        ExtHttpNotes {
            revision: 1,
            text: "**Live** 备注".into()
        }
    );
    let live_hl_op = Uuid::new_v4();
    state
        .command(Command::SetExtHttpHighlights {
            project: owner,
            operation_id: live_hl_op,
            changes: vec![ExtHttpHighlightChange {
                source_id: Uuid::nil(),
                item_id: live_id,
                expected_highlight_revision: 0,
                color: 4,
            }],
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    drain(&mut receiver);
    assert_eq!(
        state
            .store
            .ext_http_highlights(
                owner,
                &[ExtHttpHighlightChange {
                    source_id: Uuid::nil(),
                    item_id: live_id,
                    expected_highlight_revision: 1,
                    color: 0,
                }]
            )
            .unwrap(),
        [(
            Uuid::nil(),
            live_id,
            ExtHttpHighlight {
                revision: 1,
                color: 4
            }
        )]
    );
    state.storage.history_dirty = false;
    for scenario in 0..5 {
        let cancel = CancellationToken::new();
        if scenario == 1 {
            cancel.cancel();
        }
        let project = if scenario == 2 {
            ProjectKey {
                generation: owner.generation + 1,
                ..owner
            }
        } else {
            owner
        };
        let operation_id = if scenario == 3 {
            Uuid::nil()
        } else {
            Uuid::new_v4()
        };
        state
            .command(Command::SetExtHttpHighlights {
                project,
                operation_id,
                changes: changes(
                    source,
                    &items,
                    if scenario == 0 { 0 } else { 1 },
                    if scenario == 4 { 255 } else { 0 },
                ),
                cancel,
            })
            .await
            .unwrap();
        let events = drain(&mut receiver);
        let [
            Event::ExtHttpHighlightsChanged {
                project: actual_project,
                operation_id: actual_operation,
                result: Err(error),
            },
        ] = events.as_slice()
        else {
            panic!("{events:?}")
        };
        assert_eq!(
            (*actual_project, *actual_operation),
            (project, operation_id)
        );
        assert!(!error.message.is_empty());
        if scenario == 1 || scenario == 2 {
            assert!(error.stored.is_empty());
        } else {
            assert_eq!(&error.stored, saved);
        }
        assert_eq!(
            state
                .store
                .ext_http_highlights(owner, &changes(source, &items, 1, 0))
                .unwrap(),
            *saved
        );
        assert!(!state.storage.history_dirty);
    }
    let operation = Uuid::new_v4();
    state
        .command(Command::SetExtHttpHighlights {
            project: owner,
            operation_id: operation,
            changes: changes(source, &items, 1, 1),
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    let events = drain(&mut receiver);
    assert!(
        matches!(events.as_slice(), [Event::ExtHttpHighlightsChanged { operation_id, result: Ok(values), .. }] if *operation_id == operation && values == saved)
    );
    for item in items {
        assert_eq!(state.store.ext_item(item.id).unwrap(), item);
    }
    state.store.close().unwrap();
}

#[tokio::test]
async fn project_save_failure_retains_recovery_then_save_as_returns_all_annotation_receipts() {
    let (mut state, mut receiver, source, items) = fixture();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.mio");
    let copy = directory.path().join("recovered.mio");
    state.store.save(Some(&path)).unwrap();
    let owner = state.project;
    let cancel = CancellationToken::new();
    state
        .store
        .set_ext_http_highlights(owner, &changes(source, &items, 0, 1), &cancel)
        .unwrap();
    let recovery = super::tests::completed_flow();
    let message = WebSocketMessage {
        id: Uuid::new_v4(),
        revision: 1,
        flow_id: recovery.summary.id,
        sequence: 1,
        direction: WebSocketDirection::ClientToServer,
        kind: WebSocketKind::Binary,
        payload: vec![0, 255, 42],
        observed_unix_ms: 5,
    };
    state.storage.recovery.push_back(recovery.clone().into());
    state.storage.websocket_recovery.push_back(message.clone().into());
    let before = state.store.workspace().unwrap();
    let mut workspace = before.clone();
    workspace.open_request(None).unwrap();
    let save = |revision, path| Command::SaveProject {
        project: owner,
        path,
        workspace: Some(workspace.clone()),
        websocket_notes: vec![WebSocketNotesChange {
            message_id: message.id,
            expected_notes_revision: 0,
            text: "WS🙂".into(),
        }],
        ext_http_notes: vec![ExtHttpNotesChange {
            source_id: source,
            item_id: items[0].id,
            expected_notes_revision: 0,
            text: "HTTP\n**原文**".into(),
        }],
        ext_http_highlights: changes(source, &items, revision, 0),
    };
    assert!(state.command(save(0, None)).await.is_err());
    assert_eq!(
        (state.storage.recovery.len(), state.storage.websocket_recovery.len()),
        (1, 1)
    );
    assert!(
        !drain(&mut receiver)
            .iter()
            .any(|event| matches!(event, Event::ProjectSaved { .. }))
    );
    assert_eq!(state.store.workspace().unwrap(), before);
    assert!(state.store.read(recovery.summary.id).is_err());
    state.store.reject_writes(true);
    state.command(save(1, Some(copy.clone()))).await.unwrap();
    assert_eq!(state.unsaved_count(), 0);
    let events = drain(&mut receiver);
    let receipts: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::ProjectSaved {
                project,
                path,
                websocket_notes,
                ext_http_notes,
                ext_http_highlights,
                ..
            } => Some((
                project,
                path,
                websocket_notes,
                ext_http_notes,
                ext_http_highlights,
            )),
            _ => None,
        })
        .collect();
    assert_eq!(receipts.len(), 1);
    let (project, saved, ws, http, highlights) = receipts[0];
    assert_eq!((*project, saved), (owner, &copy));
    assert_eq!(ws, &[(message.id, 1)]);
    assert_eq!(http, &[(source, items[0].id, 1)]);
    assert_eq!(
        highlights,
        &items
            .iter()
            .map(|item| (source, item.id, 2))
            .collect::<Vec<_>>()
    );
    assert_eq!(state.store.workspace().unwrap(), workspace);
    assert_eq!(state.store.read(recovery.summary.id).unwrap(), recovery);
    assert_eq!(state.store.read_websocket(message.id).unwrap(), message);
    for item in &items {
        assert_eq!(state.store.ext_item(item.id).unwrap(), *item);
    }
    state.store.close().unwrap();
    let original = ProjectStore::open(&path).unwrap();
    assert_eq!(original.workspace().unwrap(), before);
    assert!(original.read(recovery.summary.id).is_err());
    assert!(original.read_websocket(message.id).is_err());
    assert_eq!(
        original
            .ext_http_highlights(owner, &changes(source, &items, 1, 0))
            .unwrap()
            .iter()
            .map(|(_, _, value)| value.color)
            .collect::<Vec<_>>(),
        [1, 1]
    );
    original.close().unwrap();
}
