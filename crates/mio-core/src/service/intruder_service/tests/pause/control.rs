use super::*;

#[tokio::test]
async fn pause_is_durable_allows_inflight_completion_and_dispatches_only_after_explicit_resume() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen, received) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut stream).await, b'A');
            seen.send(()).unwrap();
            released.await.unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n\0\xff",
                )
                .await
                .unwrap();
            listener
        });
        let (mut state, mut events) = state();
        let project = state.project;
        let directory = tempfile::tempdir().unwrap();
        save(&mut state, Some(directory.path().join("paused.mio")))
            .await
            .unwrap();
        let mut input = spec(format!("http://{address}"), 3);
        input.payload_sets = vec![vec![vec![b'A'], vec![b'B'], vec![b'C']]];
        let id = admit(&mut state, input);
        state.advance_intruder(id).await.unwrap();
        received.await.unwrap();
        let paused = control(&mut state, &mut events, project, id, true)
            .await
            .unwrap();
        assert_eq!(paused.state, IntruderRunState::Paused);
        assert_eq!(
            state.store.read_intruder_run(id).unwrap(),
            paused,
            "success ACK follows commit"
        );
        assert_eq!(
            state.intruder.advance_candidate(),
            None,
            "spare concurrency cannot bypass pause"
        );
        state.advance_intruder(id).await.unwrap();
        assert_eq!(state.intruder.jobs.len(), 1);
        release.send(()).unwrap();
        let completion = state.intruder.jobs.join_next_with_id().await.unwrap();
        state.complete_intruder(completion).await.unwrap();
        let listener = origin.await.unwrap().into_std().unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
        let page = state.intruder_result_page(id, None, 3).unwrap();
        assert!(page.persisted);
        assert_eq!(
            (page.run.state, page.run.dispatched, page.run.completed),
            (IntruderRunState::Paused, 1, 1)
        );
        let IntruderResultOutcome::Flow(first) = &page.rows[0].outcome else {
            panic!("real origin response must be retained")
        };
        assert_eq!(
            state.read_flow(first.id).unwrap().response.unwrap().body,
            [0, 255]
        );
        state.command(Command::Resume { project }).await.unwrap();
        save(&mut state, None).await.unwrap();
        assert_eq!(
            state.intruder.advance_candidate(),
            None,
            "project resume and save do not resume attacks"
        );
        state.advance_intruder(id).await.unwrap();
        assert!(state.intruder.jobs.is_empty());
        assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);

        let resumed = control(&mut state, &mut events, project, id, false)
            .await
            .unwrap();
        assert_eq!(
            (resumed.state.clone(), resumed.dispatched),
            (IntruderRunState::Running, 1)
        );
        assert_eq!(state.store.read_intruder_run(id).unwrap(), resumed);
        let listener = TcpListener::from_std(listener).unwrap();
        let origin = tokio::spawn(async move {
            let mut values = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                values.push(request(&mut stream).await);
                stream
                    .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                    .await
                    .unwrap();
            }
            (listener, values)
        });
        state.advance_intruder(id).await.unwrap();
        state.advance_intruder(id).await.unwrap();
        while let Some(completion) = state.intruder.jobs.join_next_with_id().await {
            state.complete_intruder(completion).await.unwrap();
        }
        state.advance_intruder(id).await.unwrap();
        let (listener, mut values) = origin.await.unwrap();
        values.sort_unstable();
        assert_eq!(values, b"BC", "resume must never resend the first POST");
        assert_eq!(
            listener.into_std().unwrap().accept().unwrap_err().kind(),
            ErrorKind::WouldBlock
        );
        let page = state.intruder_result_page(id, None, 3).unwrap();
        assert_eq!(
            (
                page.run.state,
                page.run.total,
                page.run.dispatched,
                page.run.completed
            ),
            (IntruderRunState::Completed, 3, 3, 3)
        );
        for (ordinal, row) in page.rows.iter().enumerate() {
            assert_eq!(row.ordinal, ordinal as u64);
            let IntruderResultOutcome::Flow(flow) = &row.outcome else {
                panic!("all three ordinals were sent")
            };
            assert_eq!(
                state.read_flow(flow.id).unwrap().request.body,
                [b'A' + ordinal as u8]
            );
        }
        assert!(
            control(&mut state, &mut events, project, id, false)
                .await
                .is_err()
        );
        assert!(
            control(&mut state, &mut events, project, id, true)
                .await
                .is_err()
        );
        assert!(!state.intruder.has_resources());
    })
    .await
    .expect("three causally released loopback requests and cleanup are bounded");
}
