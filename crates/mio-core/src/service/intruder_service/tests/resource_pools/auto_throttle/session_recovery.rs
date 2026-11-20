use super::*;

#[tokio::test]
async fn local_validation_failure_with_automatic_enabled_keeps_detail_without_retrying() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (mut state, _events) = state();
        let mut shared = pool("local rejection", Some(1));
        shared.automatic_throttling.enabled = true;
        install(&mut state, vec![shared.clone()]);
        let mut input = get_spec(
            format!("http://{}", listener.local_addr().unwrap()),
            shared.id,
            false,
        );
        input.draft.raw = b"CONNECT /auto?q=X HTTP/1.1\r\nHost: fixture.invalid\r\n\r\n".to_vec();
        let offset = input
            .draft
            .raw
            .iter()
            .position(|byte| *byte == b'X')
            .unwrap();
        input.positions = vec![Position {
            start: offset,
            end: offset + 1,
        }];
        let id = admit(&mut state, input);
        let timing = state.intruder.pool_timings[&shared.id];
        state.advance_intruder(id).await.unwrap();
        state.advance_intruder(id).await.unwrap();
        let page = state.intruder_result_page(id, None, 2).unwrap();
        let IntruderResultOutcome::Flow(summary) = &page.rows[0].outcome else {
            panic!("local rejection preserves the generated request detail")
        };
        assert!(matches!(&summary.state, FlowState::Failed(error) if error.contains("CONNECT")));
        assert_eq!(
            state.store.read(summary.id).unwrap().request.method,
            "CONNECT"
        );
        assert_eq!(state.intruder.pool_timings[&shared.id], timing);
        assert!(!state.intruder.has_resources());
        no_extra(listener);
    })
    .await
    .expect("local validation failure never enters automatic retry or charges a send");
}

#[tokio::test]
async fn session_final_retry_response_write_failure_saves_without_repeating_requests() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (final_seen_tx, final_seen) = tokio::sync::oneshot::channel();
        let (release_final, final_released) = tokio::sync::oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            read_get(&mut first).await;
            reply(&mut first, 429, b"retry").await;
            assert_eq!(first.read(&mut [0; 1]).await.unwrap(), 0);
            let (mut final_stream, _) = listener.accept().await.unwrap();
            read_get(&mut final_stream).await;
            final_seen_tx.send(()).unwrap();
            final_released.await.unwrap();
            reply(&mut final_stream, 200, &[0, 254, b'R']).await;
            assert_eq!(final_stream.read(&mut [0; 1]).await.unwrap(), 0);
            listener
        });
        let (mut state, _events) = state();
        let mut shared = pool("Session retry", Some(1));
        shared.automatic_throttling.enabled = true;
        install(&mut state, vec![shared.clone()]);
        let input = get_spec(format!("http://{address}"), shared.id, true);
        let id = admit(&mut state, input.clone());
        state.advance_intruder(id).await.unwrap();
        let timing = state.intruder.pool_timings[&shared.id];
        let flow = state.intruder.requests.values().next().unwrap().flow_id;
        final_seen.await.unwrap();
        assert_eq!(
            state.store.read(flow).unwrap().summary.state,
            FlowState::Pending
        );
        state.store.reject_session_intruder_writes(id, true);
        release_final.send(()).unwrap();
        let completion = state.intruder.jobs.join_next_with_id().await.unwrap();
        assert!(state.complete_intruder(completion).await.is_err());
        assert!(state.stop_intruder().await.is_err());
        let before = state.intruder_result_page(id, None, 2).unwrap();
        assert_eq!(before.run.retention, IntruderRetention::Session);
        assert_eq!(
            (
                before.run.dispatched,
                before.run.completed,
                before.rows.len()
            ),
            (1, 1, 1)
        );
        assert!(!before.persisted);
        let final_record = state.read_flow(flow).unwrap();
        assert_eq!(final_record.summary.state, FlowState::Complete);
        assert_eq!(final_record.summary.status, Some(200));
        assert_eq!(final_record.response.as_ref().unwrap().body, [0, 254, b'R']);
        assert!(!state.intruder.has_resources());
        assert!(!state.intruder.recovery.is_empty());
        state.store.reject_session_intruder_writes(id, false);
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let path = directory.path().join("automatic-session.mio");
        save(&mut state, Some(path.clone())).await.unwrap();
        assert!(state.intruder.recovery.is_empty());
        assert_eq!(state.intruder_result_page(id, None, 2).unwrap(), before);
        assert_eq!(state.store.read(flow).unwrap(), final_record);
        assert_eq!(state.store.read_intruder_spec(id).unwrap(), input);
        assert_eq!(state.intruder.pool_timings[&shared.id], timing);
        let disk = ProjectStore::open(&path).unwrap();
        assert!(disk.find_intruder_run(id).unwrap().is_none());
        assert!(disk.read_optional(flow).unwrap().is_none());
        disk.close().unwrap();
        no_extra(origin.await.unwrap());
    })
    .await
    .expect(
        "only the final Session response is recovered, and Save never reruns automatic requests",
    );
}
