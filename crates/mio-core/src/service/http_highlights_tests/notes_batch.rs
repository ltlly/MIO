use super::*;

#[tokio::test]
async fn notes_batch_commands_keep_shared_text_atomic_revisions_and_cancel_identity() {
    let (mut state, mut receiver, source, items) = fixture();
    let owner = state.project;
    let ids: Vec<_> = items.iter().map(|item| (source, item.id)).collect();
    let operation = Uuid::new_v4();
    state
        .command(Command::ReadExtHttpNotesBatch {
            project: owner,
            operation_id: operation,
            ids: ids.clone(),
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    let events = drain(&mut receiver);
    let [
        Event::ExtHttpNotesBatch {
            project,
            operation_id,
            result: Ok(values),
        },
    ] = events.as_slice()
    else {
        panic!("{events:?}")
    };
    assert_eq!((*project, *operation_id), (owner, operation));
    assert_eq!(
        &values.states,
        &ids.iter()
            .map(|&(source, item)| (source, item, 0, false))
            .collect::<Vec<_>>()
    );
    assert_eq!(values.common_text.as_deref(), Some(""));
    let initial: Vec<_> = values
        .states
        .iter()
        .map(|&(source, item, revision, _)| (source, item, revision))
        .collect();
    let text = "# 公共备注\n**原文🙂**";
    let operation = Uuid::new_v4();
    state
        .command(Command::SetExtHttpNotesBatch {
            project: owner,
            operation_id: operation,
            changes: initial.clone(),
            text: text.into(),
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    let events = drain(&mut receiver);
    let [
        Event::ExtHttpNotesBatchChanged {
            project,
            operation_id,
            result: Ok(saved),
        },
        Event::HistoryChanged { .. },
    ] = events.as_slice()
    else {
        panic!("{events:?}")
    };
    assert_eq!((*project, *operation_id), (owner, operation));
    assert_eq!(
        saved,
        &ids.iter()
            .map(|&(source, item)| (source, item, 1))
            .collect::<Vec<_>>()
    );
    for item in &items {
        assert_eq!(
            state
                .store
                .ext_http_notes(owner, source, item.id)
                .unwrap()
                .text,
            text
        );
    }
    state.storage.history_dirty = false;
    for cancelled in [false, true] {
        let token = CancellationToken::new();
        if cancelled {
            token.cancel();
        }
        let operation = Uuid::new_v4();
        state
            .command(Command::SetExtHttpNotesBatch {
                project: owner,
                operation_id: operation,
                changes: initial.clone(),
                text: String::new(),
                cancel: token,
            })
            .await
            .unwrap();
        let events = drain(&mut receiver);
        let [
            Event::ExtHttpNotesBatchChanged {
                project,
                operation_id,
                result: Err(error),
            },
        ] = events.as_slice()
        else {
            panic!("{events:?}")
        };
        assert_eq!((*project, *operation_id), (owner, operation));
        if cancelled {
            assert!(error.stored.is_empty());
        } else {
            assert_eq!(
                error.stored,
                ids.iter()
                    .map(|&(source, item)| (source, item, 1, true))
                    .collect::<Vec<_>>()
            );
        }
        for item in &items {
            assert_eq!(
                state
                    .store
                    .ext_http_notes(owner, source, item.id)
                    .unwrap()
                    .text,
                text
            );
        }
        assert!(!state.storage.history_dirty);
    }
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let operation = Uuid::new_v4();
    state
        .command(Command::ReadExtHttpNotesBatch {
            project: owner,
            operation_id: operation,
            ids: ids.clone(),
            cancel: cancelled,
        })
        .await
        .unwrap();
    assert!(
        matches!(drain(&mut receiver).as_slice(),[Event::ExtHttpNotesBatch{project,operation_id,result:Err(_)}] if *project==owner && *operation_id==operation)
    );
    let stale = ProjectKey {
        generation: owner.generation + 1,
        ..owner
    };
    let operation = Uuid::new_v4();
    state
        .command(Command::SetExtHttpNotesBatch {
            project: stale,
            operation_id: operation,
            changes: saved.clone(),
            text: String::new(),
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    assert!(
        matches!(drain(&mut receiver).as_slice(),[Event::ExtHttpNotesBatchChanged{project,operation_id,result:Err(error)}] if *project==stale && *operation_id==operation && error.stored.is_empty())
    );
    let directory = tempfile::tempdir().unwrap();
    let copy = directory.path().join("batch-retry.mio");
    state.store.reject_writes(true);
    let operation = Uuid::new_v4();
    state
        .command(Command::SetExtHttpNotesBatch {
            project: owner,
            operation_id: operation,
            changes: saved.clone(),
            text: String::new(),
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    assert!(matches!(
        drain(&mut receiver).as_slice(),
        [Event::ExtHttpNotesBatchChanged { result: Err(_), .. }]
    ));
    state
        .command(Command::SaveProject {
            project: owner,
            path: Some(copy.clone()),
            workspace: None,
            websocket_notes: vec![],
            ext_http_notes: vec![],
            ext_http_highlights: vec![],
        })
        .await
        .unwrap();
    drain(&mut receiver);
    let operation = Uuid::new_v4();
    state
        .command(Command::SetExtHttpNotesBatch {
            project: owner,
            operation_id: operation,
            changes: saved.clone(),
            text: String::new(),
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    assert!(drain(&mut receiver).iter().any(|event| matches!(event,Event::ExtHttpNotesBatchChanged{project,operation_id,result:Ok(values)} if *project==owner && *operation_id==operation && values==&ids.iter().map(|&(source,item)|(source,item,2)).collect::<Vec<_>>())));
    state.store.close().unwrap();
    let reopened = ProjectStore::open(&copy).unwrap();
    assert_eq!(
        reopened
            .ext_http_notes_batch(owner, &ids, &CancellationToken::new())
            .unwrap()
            .states,
        ids.iter()
            .map(|&(source, item)| (source, item, 2, false))
            .collect::<Vec<_>>()
    );
    reopened.close().unwrap();
}
