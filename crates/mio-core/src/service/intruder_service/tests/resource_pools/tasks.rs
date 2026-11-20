use super::*;

#[tokio::test]
async fn deleting_an_inflight_pool_preserves_old_ownership_and_default_clock_until_join() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen_tx, seen) = tokio::sync::oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut stream).await, 255);
            seen_tx.send(()).unwrap();
            assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0);
            listener
        });
        let (mut state, _events) = state();
        let catalog = state
            .ensure_intruder_default_pool(state.project, Uuid::new_v4(), &CancellationToken::new())
            .unwrap();
        let default = catalog.default_pool_id.unwrap();
        let deleted = pool("deleted", Some(1));
        let mut pools = catalog.pools;
        pools.push(deleted.clone());
        let catalog = install(&mut state, pools);
        let mut input = spec(format!("http://{address}"), 2);
        input.resource_pool = Some(IntruderPoolSelection::Existing(deleted.id));
        let run = admit(&mut state, input.clone());
        let cancel = state.intruder.runs[&run].cancel.clone();
        state.advance_intruder(run).await.unwrap();
        seen.await.unwrap();
        let clock = PoolTiming {
            last_send: Some(PoolInstant::now()),
            sent_count: 7,
            not_before: None,
        };
        state.intruder.pool_timings.insert(default, clock);
        let usage = state.intruder_pool_usage(state.project).unwrap();
        assert_eq!(usage.task_counts[&deleted.id], 1);
        assert!(usage.requires_confirmation.contains(&deleted.id));
        let workspace = state.store.workspace().unwrap();
        let rejected = state
            .delete_intruder_resource_pool(
                state.project,
                Uuid::new_v4(),
                deleted.id,
                (catalog.revision, workspace.revision),
                false,
                &CancellationToken::new(),
            )
            .unwrap_err();
        assert_eq!(rejected.reassigned_sessions, 0);
        assert_eq!(state.intruder.resource_pools, catalog);
        let receipt = state
            .delete_intruder_resource_pool(
                state.project,
                Uuid::new_v4(),
                deleted.id,
                (catalog.revision, workspace.revision),
                true,
                &CancellationToken::new(),
            )
            .unwrap();
        assert_eq!((receipt.reassigned_runs, receipt.reassigned_tabs), (1, 1));
        assert!(!cancel.is_cancelled());
        assert_eq!(state.intruder.pool_timings[&default], clock);
        assert!(!state.intruder.pool_timings.contains_key(&deleted.id));
        assert_eq!(state.intruder.requests.len(), 1);
        assert_eq!(
            state
                .intruder
                .requests
                .values()
                .next()
                .unwrap()
                .resource_pool,
            Some(deleted.id)
        );
        assert_eq!(
            state.intruder.runs[&run].summary.resource_pool,
            Some(default)
        );
        assert_eq!(state.intruder.runs[&run].summary.resource_pool_revision, 2);
        assert_eq!(state.store.read_intruder_spec(run).unwrap(), input);
        state.request_intruder_stop(state.project, run).unwrap();
        assert_eq!(state.intruder.reclaim_candidate(), Some(run));
        // Stop first persists Stopping; only a later pass after join retires the run.
        state.advance_intruder(run).await.unwrap();
        assert_eq!(
            state.intruder_run(run).unwrap().state,
            IntruderRunState::Stopping
        );
        assert_eq!(state.intruder.runs[&run].active, 1);
        finish_next(&mut state).await;
        assert_eq!(state.intruder.runs[&run].active, 0);
        assert!(state.intruder.requests.is_empty());
        assert_eq!(state.intruder.reclaim_candidate(), Some(run));
        state.advance_intruder(run).await.unwrap();
        let summary = state.intruder_run(run).unwrap();
        assert_eq!(summary.state, IntruderRunState::Stopped);
        assert_eq!(summary.resource_pool, Some(default));
        assert_eq!(summary.resource_pool_revision, 2);
        assert!(!state.intruder.has_resources());
        assert_eq!(
            state
                .intruder_pool_usage(state.project)
                .unwrap()
                .task_counts[&default],
            1
        );
        no_extra(origin.await.unwrap());
    })
    .await
    .expect("one held loopback request must retain ownership and join without replay");
}

#[tokio::test]
async fn tasks_usage_and_delete_commands_keep_project_operation_and_failure_receipts() {
    let (mut state, mut events) = state();
    let project = state.project;
    let unused = pool("unused", None);
    let catalog = install(&mut state, vec![unused.clone()]);
    state
        .command(Command::QueryIntruderPoolUsage {
            project,
            query_revision: 7,
        })
        .await
        .unwrap();
    let Event::IntruderPoolUsage {
        project: owner,
        query_revision: 7,
        result: Ok(usage),
    } = events.try_recv().unwrap()
    else {
        panic!("usage query must have a correlated receipt")
    };
    assert_eq!(owner, project);
    assert_eq!(usage.catalog_revision, catalog.revision);
    assert_eq!(usage.task_counts[&unused.id], 0);
    let operation = Uuid::new_v4();
    let workspace = state.store.workspace().unwrap();
    state
        .command(Command::DeleteIntruderResourcePool {
            project,
            operation_id: operation,
            pool_id: unused.id,
            expected_catalog_revision: 0,
            expected_workspace_revision: workspace.revision,
            allow_reassign: false,
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    let Event::IntruderResourcePoolDeleted {
        project: owner,
        operation_id,
        pool_id,
        result: Err(error),
    } = events.try_recv().unwrap()
    else {
        panic!("stale delete must return a structured failure")
    };
    assert_eq!(
        (owner, operation_id, pool_id),
        (project, operation, unused.id)
    );
    assert_eq!(error.catalog, Some(catalog.clone()));
    assert_eq!(error.workspace.as_deref(), Some(&workspace));
    assert_eq!(error.reassigned_sessions, 0);
    state
        .command(Command::DeleteIntruderResourcePool {
            project,
            operation_id: operation,
            pool_id: unused.id,
            expected_catalog_revision: catalog.revision,
            expected_workspace_revision: workspace.revision,
            allow_reassign: false,
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    let Event::IntruderResourcePoolDeleted {
        result: Ok(receipt),
        ..
    } = events.try_recv().unwrap()
    else {
        panic!("unused pool must delete immediately")
    };
    assert!(receipt.catalog.pools.is_empty());
    assert!(receipt.catalog.default_pool_id.is_none());
    assert_eq!(receipt.workspace, workspace);
}
