use super::*;

#[tokio::test]
async fn rejected_conversion_consumes_matching_inspection_but_not_somebody_elses() {
    let directory = tempfile::tempdir().unwrap();
    let (path, _) = fixture(directory.path(), 72);
    let (events, mut receiver) = mpsc::channel(32);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let inspection = import::inspect(path, &CancellationToken::new(), &mut |_, _, _| {}).unwrap();
    let id = inspection.info.id;
    state.ext_archive.inspection = Some((state.project, inspection));
    state
        .convert_ext(
            state.project,
            Uuid::new_v4(),
            Uuid::new_v4(),
            directory.path().join("wrong.mio"),
            CancellationToken::new(),
        )
        .await;
    assert!(state.ext_archive.inspection.is_some());
    assert!(matches!(
        outcomes(&mut receiver).as_slice(),
        [(_, ExtJobOutcome::Failed { .. })]
    ));
    state.quiesced = true;
    state
        .convert_ext(
            state.project,
            Uuid::new_v4(),
            id,
            directory.path().join("blocked.mio"),
            CancellationToken::new(),
        )
        .await;
    finish(&mut state).await;
    assert!(state.ext_archive.inspection.is_none());
    assert!(matches!(
        outcomes(&mut receiver).as_slice(),
        [(_, ExtJobOutcome::Failed { .. })]
    ));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn replacing_project_cancels_accepted_job_and_reclaims_its_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let (path, _) = fixture(directory.path(), 72);
    let (events, mut receiver) = mpsc::channel(32);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let previous = state.project;
    let cancel = CancellationToken::new();
    state
        .inspect_ext(previous, Uuid::new_v4(), path, cancel.clone())
        .await;
    state
        .command(Command::NewProject { path: None })
        .await
        .unwrap();
    assert!(cancel.is_cancelled());
    assert!(state.ext_archive.active.is_none());
    assert!(state.ext_archive.inspection.is_none());
    assert_ne!(state.project, previous);
    assert!(matches!(
        outcomes(&mut receiver).as_slice(),
        [(_, ExtJobOutcome::Cancelled)]
    ));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn old_project_queries_always_return_their_own_failure_envelope() {
    let (events, mut receiver) = mpsc::channel(8);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let old = ProjectKey {
        generation: 0,
        ..state.project
    };
    let id = Uuid::new_v4();
    state
        .command(Command::QueryExtReport {
            project: old,
            query_revision: 5,
        })
        .await
        .unwrap();
    assert!(
        matches!(receiver.try_recv().unwrap(), Event::ExtReport { project, query_revision: 5, result: Err(_)} if project == old)
    );
    state
        .command(Command::QueryExtMessages {
            project: old,
            query_revision: 6,
            source_id: id,
            collection: Some("root.f1.f0".into()),
            offset: 10,
            limit: 25,
        })
        .await
        .unwrap();
    assert!(
        matches!(receiver.try_recv().unwrap(), Event::ExtMessages { project, query_revision: 6, source_id, offset: 10, result: Err(_), .. } if project == old && source_id == id)
    );
    state
        .command(Command::ReadExtMessage {
            project: old,
            selection_revision: 7,
            message_id: id,
            offset: 9,
            limit: 30,
        })
        .await
        .unwrap();
    assert!(
        matches!(receiver.try_recv().unwrap(), Event::ExtMessage { project, selection_revision: 7, message_id, offset: 9, result: Err(_), .. } if project == old && message_id == id)
    );
    state
        .command(Command::QueryExtItems {
            project: old,
            query_revision: 8,
            source_id: id,
            collection: Some("root.f2.f1".into()),
            offset: 2,
            limit: 10,
        })
        .await
        .unwrap();
    assert!(matches!(
        receiver.try_recv().unwrap(),
        Event::ExtItems { project, query_revision: 8, source_id, collection: Some(collection), offset: 2, result: Err(_) }
            if project == old && source_id == id && collection == "root.f2.f1"
    ));
    state
        .command(Command::ReadExtItem {
            project: old,
            selection_revision: 9,
            source_id: id,
            item_id: id,
            message_offset: 3,
            message_limit: 10,
        })
        .await
        .unwrap();
    assert!(matches!(
        receiver.try_recv().unwrap(),
        Event::ExtItem { project, selection_revision: 9, source_id, item_id, message_offset: 3, result: Err(_) }
            if project == old && source_id == id && item_id == id
    ));
}

#[tokio::test]
async fn item_query_commands_keep_empty_slots_metadata_and_current_draft_separate() {
    let (events, mut receiver) = mpsc::channel(8);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let project = state.project;
    let source_id = Uuid::new_v4();
    let raw = b"GET /current HTTP/1.1\r\n\r\n\0\xff";
    let collection = "root.f2.f1";
    let empty = ExtItem {
        id: Uuid::new_v4(),
        source_id,
        collection: collection.into(),
        index: 0,
        object_address: None,
        state: "null".into(),
        metadata_json: br#"{"slot":{"state":"null"}}"#.to_vec(),
    };
    let current = ExtItem {
        id: Uuid::new_v4(),
        source_id,
        collection: collection.into(),
        index: 1,
        object_address: Some(72),
        state: "mapped".into(),
        metadata_json: br#"{"current":{"state":"value"},"history":{"state":"failed"}}"#.to_vec(),
    };
    let message = ExtMessageSummary {
        id: Uuid::new_v4(),
        source_id,
        item_id: current.id,
        collection: collection.into(),
        item_index: current.index,
        role: "current_request".into(),
        field_id: 4,
        raw_offset: 0,
        raw_bytes: raw.len() as u64,
        body_offset: Some(raw.len() as u64 - 2),
        declared_version: Some(HttpVersion::Http11),
        method: Some("GET".into()),
        request_target: Some("/current".into()),
        status: None,
        diagnostic: None,
    };
    let mut report = ExtImportReport {
        repeater_active_tab: None,
        schema_floor: 228,
        schema_revision: 228,
        source_bytes: raw.len() as u64,
        indexed_items: 0,
        indexed_messages: 0,
        collections: vec![],
        gaps: vec!["Synthetic service fixture; target and history are unresolved".into()],
        complete_archive: false,
    };
    state.store.begin_source(source_id, &report).unwrap();
    state.store.append_source_chunk(source_id, 0, raw).unwrap();
    state
        .store
        .put_ext_batch(
            &[empty.clone(), current.clone()],
            std::slice::from_ref(&message),
        )
        .unwrap();
    report.indexed_items = 2;
    report.indexed_messages = 1;
    report.collections.push(ExtCollectionReport {
        path: collection.into(),
        logical_items: Some(2),
        indexed_items: 2,
        message_count: 1,
        state: "available".into(),
    });
    report.complete_archive = true;
    state.store.finish_source(source_id, &report).unwrap();

    for (offset, item) in [empty, current].iter().enumerate() {
        state
            .command(Command::QueryExtItems {
                project,
                query_revision: 10 + offset as u64,
                source_id,
                collection: Some(collection.into()),
                offset,
                limit: 1,
            })
            .await
            .unwrap();
        let Event::ExtItems {
            project: actual_project,
            query_revision,
            source_id: actual_source,
            collection: actual_collection,
            offset: actual_offset,
            result,
        } = receiver.try_recv().unwrap()
        else {
            panic!("item query must return its own result envelope");
        };
        assert_eq!((actual_project, actual_source), (project, source_id));
        assert_eq!(
            (query_revision, actual_offset),
            (10 + offset as u64, offset)
        );
        assert_eq!(actual_collection.as_deref(), Some(collection));
        let page = result.unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].id, item.id);
        assert_eq!(page.rows[0].state, item.state);
        assert_eq!(page.rows[0].message_count, offset);
        assert_eq!(page.next_offset, (offset == 0).then_some(1));

        state
            .command(Command::ReadExtItem {
                project,
                selection_revision: 20 + offset as u64,
                source_id,
                item_id: item.id,
                message_offset: 0,
                message_limit: 1,
            })
            .await
            .unwrap();
        let Event::ExtItem {
            project: actual_project,
            selection_revision,
            source_id: actual_source,
            item_id,
            message_offset: 0,
            result,
        } = receiver.try_recv().unwrap()
        else {
            panic!("item selection must return its own result envelope");
        };
        assert_eq!((actual_project, actual_source), (project, source_id));
        assert_eq!((selection_revision, item_id), (20 + offset as u64, item.id));
        let detail = result.unwrap();
        assert_eq!(detail.item, *item);
        assert_eq!(detail.messages.total, offset);
        assert_eq!(detail.messages.next_offset, None);
        if offset == 0 {
            assert!(
                detail.messages.rows.is_empty(),
                "null cannot inherit a neighbor's draft"
            );
        } else {
            assert_eq!(
                detail.messages.rows.as_slice(),
                std::slice::from_ref(&message)
            );
        }
    }

    state
        .command(Command::ReadExtMessage {
            project,
            selection_revision: 30,
            message_id: message.id,
            offset: 0,
            limit: raw.len(),
        })
        .await
        .unwrap();
    let Event::ExtMessage { result, .. } = receiver.try_recv().unwrap() else {
        panic!("raw fragments keep the existing lazy message command");
    };
    let fragment = result.unwrap();
    assert_eq!(fragment.summary, message);
    assert_eq!(fragment.bytes, raw);
    assert!(state.sends.is_empty());
    assert!(state.proxy.is_none());
}
