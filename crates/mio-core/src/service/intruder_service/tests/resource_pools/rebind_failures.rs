use super::*;

#[tokio::test]
async fn completed_baseline_uses_current_pool_without_recreating_its_frozen_create_selection() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (blocked_tx, blocked) = tokio::sync::oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut first).await, 255);
            respond(&mut first).await;
            drop(first);
            let (mut blocker, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut blocker).await, 255);
            blocked_tx.send(()).unwrap();
            assert_eq!(blocker.read(&mut [0; 1]).await.unwrap(), 0);
            let (mut baseline, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut baseline).await, b'X');
            respond(&mut baseline).await;
            listener
        });
        let (mut state, _events) = state();
        let created = pool("created on Start", None);
        let current = pool("current", Some(1));
        install(&mut state, vec![current.clone()]);
        let mut input = spec(format!("http://{address}"), 1);
        input.resource_pool = Some(IntruderPoolSelection::Create(created.clone()));
        let run = admit(&mut state, input.clone());
        assert_eq!(
            state.intruder_run(run).unwrap().resource_pool,
            Some(created.id)
        );
        assert_eq!(state.intruder_run(run).unwrap().resource_pool_revision, 1);
        let catalog = state.intruder.resource_pools.clone();
        assert_eq!(catalog.pools.len(), 2);
        state.advance_intruder(run).await.unwrap();
        finish_next(&mut state).await;
        state.advance_intruder(run).await.unwrap();
        let before = rebind(&mut state, run, current.id);
        assert_eq!(before.state, IntruderRunState::Completed);
        let mut blocker_input = spec(format!("http://{address}"), 1);
        blocker_input.resource_pool = Some(IntruderPoolSelection::Existing(current.id));
        let blocker = admit(&mut state, blocker_input);
        state.advance_intruder(blocker).await.unwrap();
        blocked.await.unwrap();
        let receipt = state
            .request_intruder_baseline(
                state.project,
                run,
                Uuid::new_v4(),
                before.revision,
                &CancellationToken::new(),
            )
            .unwrap();
        assert_eq!(receipt.run.resource_pool, Some(current.id));
        state.advance_intruder(run).await.unwrap();
        assert_eq!(
            state.intruder_run(run).unwrap().dispatched,
            1,
            "baseline shares the current pool's occupied slot"
        );
        assert_eq!(state.intruder.resource_pools, catalog);
        state.request_intruder_stop(state.project, blocker).unwrap();
        finish_next(&mut state).await;
        state.advance_intruder(blocker).await.unwrap();
        state.advance_intruder(blocker).await.unwrap();
        state.advance_intruder(run).await.unwrap();
        finish_next(&mut state).await;
        state.advance_intruder(run).await.unwrap();
        assert_eq!(
            state.intruder_run(run).unwrap().state,
            IntruderRunState::Completed
        );
        assert!(!receipt.cancel.is_cancelled());
        assert_eq!(state.store.read_intruder_spec(run).unwrap(), input);
        assert_eq!(state.intruder.resource_pools, catalog);
        assert!(!state.intruder.has_resources());
        no_extra(origin.await.unwrap());
    })
    .await
    .expect("completed Create-backed run reuses its current pool for one explicit baseline");
}
