use super::*;

#[tokio::test]
async fn original_message_failure_does_not_publish_a_partial_detail() {
    let (events, mut receiver) = mpsc::channel(8);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let original = tests::completed_flow();
    state.store.put(&original).unwrap();
    let mut changed = original.clone();
    changed.summary.revision += 1;
    changed.request.body = b"changed".to_vec();
    state.store.put(&changed).unwrap();
    let connection = rusqlite::Connection::open(state.store.test_actual_path()).unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE messages SET head=x'ff' WHERE flow=?1 AND side=2",
                [original.summary.id.to_string()],
            )
            .unwrap(),
        1,
    );
    drop(connection);
    assert_eq!(state.read_flow(original.summary.id).unwrap(), changed);
    assert!(state.original_flow(original.summary.id).is_err());

    state
        .command(Command::ReadFlow {
            project: state.project,
            selection_revision: 7,
            flow_id: original.summary.id,
        })
        .await
        .expect("original-message failures must retain the same read identity");
    assert!(
        matches!(receiver.try_recv().unwrap(), Event::FlowDetailError {
        project,
        selection_revision: 7,
        flow_id,
        message,
    } if project == state.project && flow_id == original.summary.id && !message.is_empty())
    );
    assert!(
        receiver.try_recv().is_err(),
        "no partial success before the error"
    );
}

#[tokio::test]
async fn read_flow_errors_keep_the_requested_project_selection_and_flow() {
    let (events, mut receiver) = mpsc::channel(8);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let record = tests::completed_flow();
    state.store.put(&record).unwrap();
    let stale_project = ProjectKey {
        generation: state.project.generation - 1,
        ..state.project
    };
    for (project, selection_revision, flow_id) in [
        (state.project, 41, Uuid::new_v4()),
        (stale_project, 42, record.summary.id),
        (state.project, 43, Uuid::new_v4()),
    ] {
        state
            .command(Command::ReadFlow {
                project,
                selection_revision,
                flow_id,
            })
            .await
            .expect("detail failures must be correlated events, not generic command errors");
        let Event::FlowDetailError {
            project: actual_project,
            selection_revision: actual_selection,
            flow_id: actual_flow,
            message,
        } = receiver.try_recv().unwrap()
        else {
            panic!("missing correlated flow detail error");
        };
        assert_eq!(actual_project, project);
        assert_eq!(actual_selection, selection_revision);
        assert_eq!(actual_flow, flow_id);
        assert!(!message.is_empty());
        assert!(receiver.try_recv().is_err(), "one terminal event per read");
    }

    state
        .command(Command::ReadFlow {
            project: state.project,
            selection_revision: 44,
            flow_id: record.summary.id,
        })
        .await
        .unwrap();
    let Event::FlowDetail {
        project,
        selection_revision,
        record: actual,
        original,
    } = receiver.try_recv().unwrap()
    else {
        panic!("a failed selection must not prevent the next successful read");
    };
    assert_eq!(project, state.project);
    assert_eq!(selection_revision, 44);
    assert_eq!(*actual, record);
    assert_eq!(*original, OriginalMessages::default());
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn read_flow_preserves_original_and_edited_response_reason_phrases_and_wire_bytes() {
    let (events, mut receiver) = mpsc::channel(8);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let mut first = tests::completed_flow();
    first.summary.revision = 1;
    first.response = Some(
        crate::parse_response(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nok",
            HttpVersion::Http11,
        )
        .unwrap(),
    );
    state.store.put(&first).unwrap();

    let mut edited = first.clone();
    edited.summary.revision = 2;
    edited.response.as_mut().unwrap().reason_phrase = Some(b"Custom Reason".to_vec());
    state.store.put(&edited).unwrap();

    state
        .command(Command::ReadFlow {
            project: state.project,
            selection_revision: 88,
            flow_id: first.summary.id,
        })
        .await
        .unwrap();
    let Event::FlowDetail {
        record: actual,
        original,
        ..
    } = receiver.try_recv().unwrap()
    else {
        panic!("expected FlowDetail");
    };
    assert_eq!(
        actual.response.as_ref().unwrap().reason_phrase.as_deref(),
        Some(b"Custom Reason".as_slice())
    );
    assert_eq!(
        original.response.as_ref().unwrap().reason_phrase.as_deref(),
        Some(b"OK".as_slice())
    );
    assert!(
        crate::response_to_raw(original.response.as_ref().unwrap())
            .starts_with(b"HTTP/1.1 200 OK\r\n")
    );
    assert!(
        crate::response_to_raw(actual.response.as_ref().unwrap())
            .starts_with(b"HTTP/1.1 200 Custom Reason\r\n")
    );

    let mut phrasing_absent = tests::completed_flow();
    phrasing_absent.response = Some(
        crate::parse_response(
            b"HTTP/1.1 200\r\nContent-Length: 2\r\n\r\nok",
            HttpVersion::Http11,
        )
        .unwrap(),
    );
    state.store.put(&phrasing_absent).unwrap();
    let loaded_absent = state.store.read(phrasing_absent.summary.id).unwrap();
    assert_eq!(loaded_absent.response.as_ref().unwrap().reason_phrase, None);
    assert!(
        crate::response_to_raw(loaded_absent.response.as_ref().unwrap())
            .starts_with(b"HTTP/1.1 200\r\n")
    );
}
