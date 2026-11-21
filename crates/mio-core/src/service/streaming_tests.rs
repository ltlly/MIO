use super::*;

#[tokio::test]
async fn repeater_progress_preserves_unread_update_until_capacity_recovers() {
    let (events, mut receiver) = mpsc::channel(8);
    let mut state = State::new(events.clone(), CancellationToken::new()).unwrap();
    let record = tests::completed_flow();
    let tab_id = Uuid::new_v4();
    let (progress_tx, progress_rx) = watch::channel(None);
    let task = state.repeater.jobs.spawn(std::future::pending());
    state.repeater.sends.insert(
        tab_id,
        ActiveSend {
            cancel: CancellationToken::new(),
            task_id: task.id(),
            flow_id: record.summary.id,
            draft_revision: 7,
            progress_rx: Some(progress_rx),
        },
    );
    for _ in 0..4 {
        events
            .try_send(Event::HistoryChanged {
                project: state.project,
            })
            .unwrap();
    }
    let response = record.response.unwrap();
    progress_tx.send(Some((response.clone(), 123))).unwrap();
    tokio::select! {
        biased;
        _ = next_repeater_progress(&mut state.repeater.sends, &events, true) => {
            panic!("progress must not starve commands while only four slots are free");
        }
        _ = std::future::ready(()) => {}
    }
    assert_eq!(events.capacity(), 4);
    assert!(
        state.repeater.sends[&tab_id]
            .progress_rx
            .as_ref()
            .unwrap()
            .has_changed()
            .unwrap()
    );
    receiver.try_recv().unwrap();
    tokio::select! {
        biased;
        selected = next_repeater_progress(&mut state.repeater.sends, &events, true) => assert_eq!(selected, tab_id),
        _ = std::future::ready(()) => panic!("freeing a slot must release the pending progress"),
    }
    state.emit_repeater_progress(tab_id);
    assert!(
        !state.repeater.sends[&tab_id]
            .progress_rx
            .as_ref()
            .unwrap()
            .has_changed()
            .unwrap()
    );
    for _ in 0..3 {
        receiver.try_recv().unwrap();
    }
    assert!(matches!(receiver.try_recv().unwrap(), Event::SendProgress {
        tab_id: actual_tab,
        run_id,
        draft_revision: 7,
        response: actual_response,
        elapsed_ms: 123,
        ..
    } if actual_tab == tab_id && run_id == record.summary.id && *actual_response == response));
    tokio::select! {
        biased;
        _ = next_repeater_progress(&mut state.repeater.sends, &events, true) => panic!("consumed progress must park"),
        _ = std::future::ready(()) => {}
    }
    task.abort();
    while state.repeater.jobs.join_next().await.is_some() {}
}

#[tokio::test]
async fn unsaved_stream_headers_and_partial_body_agree_with_save_as_originals() {
    let (events, _receiver) = mpsc::channel(32);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let mut record = tests::completed_flow();
    record.summary.state = FlowState::Pending;
    record.summary.response_bytes = 0;
    let response = record.response.as_mut().unwrap();
    response.body.clear();
    let stream = StreamedResponse {
        original_status: response.status,
        original_version: response.version,
        original_headers: response.headers.clone(),
        body_complete: false,
    };
    state.store.reject_writes(true);
    assert!(state.persist_capture(&record, Some(&stream), None).is_err());
    assert!(
        state
            .original_flow(record.summary.id)
            .unwrap()
            .response
            .is_none()
    );
    record.summary.revision += 1;
    record.response.as_mut().unwrap().headers.push(Header {
        name: "x-edited".into(),
        value: b"yes".to_vec(),
    });
    assert!(state.persist_capture(&record, Some(&stream), None).is_err());
    record.summary.revision += 1;
    record.summary.state = FlowState::Cancelled;
    record.summary.response_bytes = 3;
    record.response.as_mut().unwrap().body = vec![0, 255, 42];
    assert!(state.persist_capture(&record, Some(&stream), None).is_err());
    let original = state.original_flow(record.summary.id).unwrap();
    assert_eq!(original.response_body_complete, Some(false));
    let response = original.response.as_ref().unwrap();
    assert_eq!(response.headers, stream.original_headers);
    assert_eq!(response.body, [0, 255, 42]);
    let directory = tempfile::tempdir().unwrap();
    state
        .command(Command::SaveProject {
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project: state.project,
            path: Some(directory.path().join("partial.mio")),
            workspace: None,
        })
        .await
        .unwrap();
    assert_eq!(state.unsaved_count(), 0);
    assert_eq!(state.store.original(record.summary.id).unwrap(), original);
    assert_eq!(state.store.read(record.summary.id).unwrap(), record);
}
