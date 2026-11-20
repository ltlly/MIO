use super::*;

#[tokio::test]
async fn paused_stop_quiesce_and_shutdown_cancel_and_join_the_real_socket() {
    tokio::time::timeout(Duration::from_secs(5), async {
        for action in 0..3 {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let (seen, received) = oneshot::channel();
            let origin = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                assert_eq!(request(&mut stream).await, 0xff);
                seen.send(()).unwrap();
                assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0);
                listener
            });
            let (mut state, mut events) = state();
            let project = state.project;
            let id = admit(&mut state, spec(format!("http://{address}"), 2));
            state.advance_intruder(id).await.unwrap();
            received.await.unwrap();
            control(&mut state, &mut events, project, id, true)
                .await
                .unwrap();
            match action {
                0 => {
                    state
                        .command(Command::StopIntruder {
                            project,
                            run_id: id,
                        })
                        .await
                        .unwrap();
                    assert_eq!(state.intruder.advance_candidate(), Some(id));
                    state.advance_intruder(id).await.unwrap();
                    let completion = state.intruder.jobs.join_next_with_id().await.unwrap();
                    state.complete_intruder(completion).await.unwrap();
                    state.advance_intruder(id).await.unwrap();
                }
                1 => state.command(Command::Quiesce { project }).await.unwrap(),
                _ => state.finish_intruder_shutdown().await.unwrap(),
            }
            let listener = origin.await.unwrap();
            assert!(!state.intruder.has_resources());
            assert!(state.intruder.requests.is_empty());
            let page = state.intruder_result_page(id, None, 1).unwrap();
            assert!(page.persisted);
            assert_eq!(
                (
                    page.run.state,
                    page.run.total,
                    page.run.dispatched,
                    page.run.completed
                ),
                (IntruderRunState::Stopped, 2, 1, 1)
            );
            let IntruderResultOutcome::Flow(flow) = &page.rows[0].outcome else {
                panic!("cancelled request keeps its ordinal and flow")
            };
            assert_eq!(flow.state, FlowState::Cancelled);
            assert_eq!(
                listener.into_std().unwrap().accept().unwrap_err().kind(),
                ErrorKind::WouldBlock
            );
        }
    })
    .await
    .expect("all three pause cleanup entry points join their actual sender with no timed sleeps");
}

#[tokio::test]
async fn reopening_a_paused_snapshot_interrupts_without_dispatch_or_implicit_resume() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("paused-reopen.mio");
        let mut store = ProjectStore::create(Some(&path)).unwrap();
        let input = spec(format!("http://{}", listener.local_addr().unwrap()), 2);
        let tab = bind(&mut store, &input);
        let id = Uuid::new_v4();
        store.create_intruder_run(id, tab, &input, 1).unwrap();
        store
            .set_intruder_state(id, IntruderRunState::Paused)
            .unwrap();
        store.close().unwrap();
        let (mut state, mut events) = state();
        state.command(Command::OpenProject { path }).await.unwrap();
        let project = state.project;
        let restored = state.intruder_result_page(id, None, 1).unwrap();
        assert_eq!(
            (
                restored.run.state,
                restored.run.dispatched,
                restored.run.completed
            ),
            (IntruderRunState::Interrupted, 0, 0)
        );
        assert!(
            control(&mut state, &mut events, project, id, false)
                .await
                .is_err()
        );
        assert_eq!(state.store.read_intruder_spec(id).unwrap(), input);
        assert!(!state.intruder.has_resources());
        assert_eq!(
            listener.into_std().unwrap().accept().unwrap_err().kind(),
            ErrorKind::WouldBlock
        );
    })
    .await
    .expect("opening a paused snapshot only performs local interrupted-state recovery");
}
