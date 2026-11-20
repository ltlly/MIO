use super::*;

#[tokio::test]
async fn session_local_rejection_preserves_request_through_explicit_save_and_promotion() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (mut state, _events) = state();
        let mut shared = pool("Session local rejection", Some(1));
        shared.delay = Some(IntruderPoolDelay::Increment {
            initial_ms: 100,
            increment_ms: 100,
        });
        install(&mut state, vec![shared.clone()]);
        let mut input = spec(format!("http://{}", listener.local_addr().unwrap()), 1);
        input.save_to_project = false;
        input.draft.raw =
            b"CONNECT / HTTP/1.1\r\nHost: fixture.invalid\r\nContent-Length: 1\r\n\r\nX".to_vec();
        input.positions = vec![Position {
            start: input.draft.raw.len() - 1,
            end: input.draft.raw.len(),
        }];
        input.resource_pool = Some(IntruderPoolSelection::Existing(shared.id));
        let expected_request = parse_request(
            b"CONNECT / HTTP/1.1\r\nContent-Length: 1\r\n\r\n\xff",
            &input.draft.target,
            input.draft.version,
        )
        .unwrap();
        let id = admit(&mut state, input.clone());
        let timing = state.intruder.pool_timings[&shared.id];
        state.advance_intruder(id).await.unwrap();
        state.advance_intruder(id).await.unwrap();
        let before = state.intruder_result_page(id, None, 2).unwrap();
        assert_eq!(before.run.retention, IntruderRetention::Session);
        assert_eq!(before.run.state, IntruderRunState::Completed);
        assert_eq!(
            (
                before.run.dispatched,
                before.run.completed,
                before.rows.len()
            ),
            (1, 1, 1)
        );
        let IntruderResultOutcome::Flow(summary) = &before.rows[0].outcome else {
            panic!("Session local rejection must preserve its generated request");
        };
        assert!(matches!(&summary.state, FlowState::Failed(error) if error.contains("CONNECT")));
        let record = state.store.read(summary.id).unwrap();
        assert_eq!(record.request, expected_request);
        assert!(record.response.is_none());
        assert!(state.intruder.recovery.is_empty());
        assert!(!state.intruder.has_resources());
        assert_eq!(state.intruder.pool_timings[&shared.id], timing);
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let path = directory.path().join("local-rejection.mio");
        save(&mut state, Some(path.clone())).await.unwrap();
        let ordinary = ProjectStore::open(&path).unwrap();
        assert!(ordinary.find_intruder_run(id).unwrap().is_none());
        assert!(ordinary.read_optional(summary.id).unwrap().is_none());
        ordinary.close().unwrap();
        let mut workspace = state.store.workspace().unwrap();
        workspace.revision += 1;
        let promoted = state
            .set_intruder_retention(
                state.project,
                id,
                before.run.revision,
                IntruderRetention::Project,
                workspace,
            )
            .unwrap();
        save(&mut state, None).await.unwrap();
        let saved = ProjectStore::open(&path).unwrap();
        assert_eq!(saved.read_intruder_run(id).unwrap(), promoted.run);
        assert_eq!(saved.read(summary.id).unwrap(), record);
        assert_eq!(saved.read_intruder_spec(id).unwrap(), input);
        saved.close().unwrap();
        assert_eq!(state.intruder.pool_timings[&shared.id], timing);
        no_extra(listener);
    })
    .await
    .expect("a local Session rejection saves and promotes exact request bytes with zero sockets");
}
