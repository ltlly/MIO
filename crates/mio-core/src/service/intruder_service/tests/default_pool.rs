use super::*;

async fn ensure_default(
    state: &mut State,
    events: &mut mpsc::Receiver<Event>,
) -> IntruderResourcePools {
    let operation = Uuid::new_v4();
    state
        .command(Command::EnsureIntruderDefaultPool {
            project: state.project,
            operation_id: operation,
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();
    let Event::IntruderDefaultPoolReady {
        project,
        operation_id,
        result,
    } = events.try_recv().unwrap()
    else {
        panic!("explicit default initialization must return a correlated receipt")
    };
    assert_eq!((project, operation_id), (state.project, operation));
    result.unwrap()
}

#[tokio::test]
async fn query_is_read_only_and_explicit_default_is_idempotent_without_rebinding_legacy_runs() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (mut state, mut events) = state();
        state
            .command(Command::QueryIntruderResourcePools {
                project: state.project,
                query_revision: 10,
            })
            .await
            .unwrap();
        let Event::IntruderResourcePools {
            query_revision: 10,
            result,
            ..
        } = events.try_recv().unwrap()
        else {
            panic!("query must return the original empty catalog")
        };
        assert_eq!(result.unwrap(), IntruderResourcePools::default());
        assert_eq!(
            state.store.intruder_resource_pools().unwrap(),
            IntruderResourcePools::default()
        );
        let mut custom = pool("existing custom", Some(2));
        custom.delay = Some(IntruderPoolDelay::Fixed { milliseconds: 100 });
        install(&mut state, vec![custom.clone()]);
        let custom_timing = PoolTiming::default()
            .after_send(PoolInstant::now(), custom.delay)
            .unwrap();
        state.intruder.pool_timings.insert(custom.id, custom_timing);
        let mut input = spec("http://127.0.0.1:1".into(), 1);
        input.max_concurrent_requests = 1;
        let legacy = admit(&mut state, input.clone());
        let before = state.intruder_run(legacy).unwrap();
        let catalog = ensure_default(&mut state, &mut events).await;
        let default_id = catalog
            .default_pool_id
            .expect("explicit initialization returns its stable default identity");
        let default = catalog.find(default_id).unwrap();
        assert_eq!(default.name, "默认资源池");
        assert_eq!(default.max_concurrent_requests, Some(10));
        assert_eq!(default.delay, None);
        assert!(default.automatic_throttling.enabled && default.automatic_throttling.status_429);
        assert!(
            !default.automatic_throttling.status_503 && !default.automatic_throttling.other_enabled
        );
        assert_eq!(
            default.automatic_throttling.response_codes().unwrap(),
            [429]
        );
        assert_eq!(catalog.pools.len(), 2);
        assert_eq!(state.intruder.pool_timings[&custom.id], custom_timing);
        assert_eq!(
            state.intruder.pool_timings[&default_id],
            PoolTiming::default()
        );
        assert_eq!(state.intruder_run(legacy).unwrap(), before);
        assert_eq!(state.store.read_intruder_spec(legacy).unwrap(), input);
        assert_eq!(state.intruder.runs[&legacy].max_concurrent_requests, 1);
        assert_eq!(
            (before.resource_pool, before.resource_pool_revision),
            (None, 0)
        );
        assert_eq!(ensure_default(&mut state, &mut events).await, catalog);
        let mut edited = catalog.pools.clone();
        let default = edited
            .iter_mut()
            .find(|pool| pool.id == default_id)
            .unwrap();
        default.name = "retained user name".into();
        default.max_concurrent_requests = Some(2);
        default.delay = Some(IntruderPoolDelay::Fixed { milliseconds: 200 });
        default.automatic_throttling.enabled = false;
        let edited = install(&mut state, edited);
        let default_timing = PoolTiming::default()
            .after_send(PoolInstant::now(), edited.find(default_id).unwrap().delay)
            .unwrap();
        state
            .intruder
            .pool_timings
            .insert(default_id, default_timing);
        assert_eq!(
            ensure_default(&mut state, &mut events).await,
            edited,
            "repeat ensure does not restore factory values over edited settings"
        );
        assert_eq!(state.intruder.pool_timings[&default_id], default_timing);
        assert_eq!(state.intruder.pool_timings[&custom.id], custom_timing);
        state
            .command(Command::QueryIntruderResourcePools {
                project: state.project,
                query_revision: 11,
            })
            .await
            .unwrap();
        let Event::IntruderResourcePools {
            query_revision: 11,
            result,
            ..
        } = events.try_recv().unwrap()
        else {
            panic!("query must return the confirmed catalog")
        };
        assert_eq!(result.unwrap(), edited);
        assert_eq!(state.store.intruder_resource_pools().unwrap(), edited);
        state.stop_intruder().await.unwrap();
        assert!(!state.intruder.has_resources());
    })
    .await
    .expect(
        "default initialization, repeated confirmation and legacy-run preservation are bounded",
    );
}

#[tokio::test]
async fn default_pool_failures_and_cancellation_preserve_catalog_and_all_existing_clocks() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

    tokio::time::timeout(Duration::from_secs(5), async {
        for failure in [
            "nil",
            "project",
            "root_cancel",
            "cancel",
            "quiesced",
            "write",
            "during_write",
            "recovery",
        ] {
            let (mut state, mut events) = state();
            let custom = pool("kept", Some(1));
            let catalog = install(&mut state, vec![custom.clone()]);
            let timing = PoolTiming::default()
                .after_send(PoolInstant::now(), None)
                .unwrap();
            state.intruder.pool_timings.insert(custom.id, timing);
            let legacy = admit(&mut state, spec("http://127.0.0.1:1".into(), 1));
            let mut project = state.project;
            let operation_id = if failure == "nil" {
                Uuid::nil()
            } else {
                Uuid::new_v4()
            };
            let cancel = CancellationToken::new();
            match failure {
                "project" => project.generation += 1,
                "root_cancel" => state.cancel.cancel(),
                "cancel" => cancel.cancel(),
                "quiesced" => state.quiesced = true,
                "write" => state.store.reject_writes(true),
                "during_write" => {
                    let cancel = cancel.clone();
                    state
                        .store
                        .test_intruder_connection(legacy)
                        .unwrap()
                        .authorizer(Some(move |context: AuthContext<'_>| {
                            if matches!(
                                context.action,
                                AuthAction::Update {
                                    table_name: "metadata",
                                    column_name: "value",
                                    ..
                                }
                            ) {
                                cancel.cancel();
                            }
                            Authorization::Allow
                        }));
                }
                "recovery" => {
                    state.store.reject_writes(true);
                    assert!(
                        state
                            .persist_intruder_state(legacy, IntruderRunState::Stopping)
                            .is_err()
                    );
                    state.store.reject_writes(false);
                }
                _ => {}
            }
            state
                .command(Command::EnsureIntruderDefaultPool {
                    project,
                    operation_id,
                    cancel,
                })
                .await
                .unwrap();
            let Event::IntruderDefaultPoolReady {
                project: owner,
                operation_id: actual,
                result,
            } = events.try_recv().unwrap()
            else {
                panic!("default initialization refusal stays correlated")
            };
            assert_eq!((owner, actual), (project, operation_id));
            assert!(result.is_err(), "{failure}");
            state
                .store
                .test_intruder_connection(legacy)
                .unwrap()
                .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
            state.store.cancel_queries_with(CancellationToken::new());
            state.store.reject_writes(false);
            state.quiesced = false;
            assert_eq!(state.intruder.resource_pools, catalog, "{failure}");
            assert_eq!(
                state.store.intruder_resource_pools().unwrap(),
                catalog,
                "{failure}"
            );
            assert_eq!(state.intruder.pool_timings.len(), 1);
            assert_eq!(state.intruder.pool_timings[&custom.id], timing);
            let stopped = state.stop_intruder().await;
            assert_eq!(stopped.is_err(), failure == "recovery");
            assert!(!state.intruder.has_resources());
        }
    })
    .await
    .expect("failed default initialization never replaces confirmed catalog or timing state");
}

#[tokio::test]
async fn two_runs_share_the_explicit_default_pool_and_live_cap_changes_keep_owned_sockets() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen_tx, seen) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let origin = tokio::spawn(async move {
            let mut first = None;
            let mut other = None;
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                match request(&mut stream).await {
                    b'A' => assert!(first.replace(stream).is_none()),
                    b'B' => assert!(other.replace(stream).is_none()),
                    value => panic!("unexpected default-pool fixture byte {value}"),
                }
            }
            seen_tx.send(()).unwrap();
            released.await.unwrap();
            respond(&mut first.unwrap()).await;
            assert_eq!(other.unwrap().read(&mut [0; 1]).await.unwrap(), 0);
            listener
        });
        let (mut state, mut events) = state();
        let catalog = ensure_default(&mut state, &mut events).await;
        let default_id = catalog.default_pool_id.unwrap();
        assert_eq!(
            catalog.find(default_id).unwrap().max_concurrent_requests,
            Some(10)
        );
        let mut input = spec(format!("http://{address}"), 2);
        input.resource_pool = Some(IntruderPoolSelection::Existing(default_id));
        input.max_concurrent_requests = 1;
        input.payload_sets = vec![vec![b"A".to_vec(), b"A".to_vec()]];
        let first = admit(&mut state, input.clone());
        input.payload_sets = vec![vec![b"B".to_vec()]];
        let other = admit(&mut state, input);
        state.advance_intruder(first).await.unwrap();
        state.advance_intruder(other).await.unwrap();
        seen.await.unwrap();
        assert_eq!(state.intruder.requests.len(), 2);
        assert!(
            state
                .intruder
                .requests
                .values()
                .all(|request| request.resource_pool == Some(default_id))
        );
        let mut pools = catalog.pools.clone();
        pools
            .iter_mut()
            .find(|pool| pool.id == default_id)
            .unwrap()
            .max_concurrent_requests = Some(1);
        install(&mut state, pools);
        release.send(()).unwrap();
        finish_next(&mut state).await;
        assert_eq!(state.intruder.runs[&other].active, 1);
        state.advance_intruder(first).await.unwrap();
        assert_eq!(
            state.intruder_run(first).unwrap().dispatched,
            1,
            "the second run still occupies the same default pool after the live cap edit"
        );
        state.stop_intruder().await.unwrap();
        assert!(!state.intruder.has_resources());
        no_extra(origin.await.unwrap());
    })
    .await
    .expect("two tiny requests prove shared default-pool ownership without a load fixture");
}

#[tokio::test]
async fn runtime_default_pool_ack_backpressure_keeps_a_queued_stop_ahead_of_new_dispatch() {
    async fn event(client: &mut Client, matches: impl Fn(&Event) -> bool) -> Event {
        loop {
            let event = client
                .events
                .recv()
                .await
                .expect("Runtime remains connected");
            assert!(
                !matches!(event, Event::Error { .. }),
                "unexpected Runtime error: {event:?}"
            );
            if matches(&event) {
                return event;
            }
        }
    }

    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen_tx, seen) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut stream).await, 255);
            seen_tx.send(()).unwrap();
            released.await.unwrap();
            respond(&mut stream).await;
            assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0);
            listener
        });
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let path = directory.path().join("default-ack.mio");
        let (runtime, mut client) = Runtime::start().unwrap();
        let Event::ProjectOpened { project, mut workspace, .. } = event(&mut client, |event| matches!(event, Event::ProjectOpened { .. })).await else { unreachable!() };
        client.commands.send(Command::SaveProject { websocket_notes: Vec::new(), ext_http_notes: Vec::new(), ext_http_highlights: Vec::new(), project, path: Some(path.clone()), workspace: None }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::ProjectSaved { project: owner, .. } if *owner == project)).await;
        let mut input = spec(format!("http://{address}"), 2);
        input.max_concurrent_requests = 1;
        let tab = Uuid::new_v4();
        workspace.revision += 1;
        workspace.intruder.tabs.push(crate::IntruderTab { id: tab, title: "legacy independent".into(), group: None, spec: input.clone(), last_run: None });
        workspace.intruder.active = Some(tab);
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::WorkspaceChanged { project: owner, workspace: saved } if *owner == project && saved == &workspace)).await;
        let run_id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        client.commands.send(Command::StartIntruder { project, run_id, tab_id: tab, spec: Box::new(input), cancel: cancel.clone() }).await.unwrap();
        let Event::IntruderStarted { result, .. } = event(&mut client, |event| matches!(event, Event::IntruderStarted { run_id: id, .. } if *id == run_id)).await else { unreachable!() };
        assert_eq!(result.unwrap().resource_pool, None);
        seen.await.unwrap();
        client.commands.send(Command::QueryIntruderResults { project, run_id, query_revision: 50, after: None, limit: 2 }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::IntruderResults { query_revision: 50, .. })).await;
        // Consume the pending run invalidation released by the result query before owning every slot.
        event(&mut client, |event| matches!(event, Event::IntruderChanged { project: owner } if *owner == project)).await;
        // Do not query History here: its already-published invalidation remains coalesced while
        // the held response completes behind the one controlled initialization ACK.
        while let Ok(event) = client.events.try_recv() { assert!(!matches!(event, Event::Error { .. })); }
        let capacity = client.events.max_capacity();
        for query_revision in 0..capacity as u64 {
            client.commands.send(Command::QueryIntruderRuns { project, query_revision, after: None, limit: 1 }).await.unwrap();
        }
        while client.events.len() != capacity { tokio::task::yield_now().await; }
        let operation_id = Uuid::new_v4();
        client.commands.send(Command::EnsureIntruderDefaultPool { project, operation_id, cancel: CancellationToken::new() }).await.unwrap();
        let disk = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let initialized = || disk.query_row::<bool, _, _>("SELECT EXISTS(SELECT 1 FROM metadata WHERE key='intruder_resource_pools')", [], |row| row.get(0)).unwrap();
        while !initialized() { tokio::task::yield_now().await; }
        assert_eq!(client.events.len(), capacity);
        client.commands.send(Command::StopIntruder { project, run_id }).await.unwrap();
        release.send(()).unwrap();
        let phase = || disk.query_row::<(u64, u64, i64), _, _>("SELECT dispatched,completed,state FROM intruder_runs WHERE id=?1", [run_id.to_string()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
        while phase().1 != 1 { tokio::task::yield_now().await; }
        assert_eq!(phase(), (1, 1, 0), "completed first response cannot dispatch the next payload before the retained ACK");
        for query_revision in 0..capacity as u64 {
            let Event::IntruderRuns { project: owner, query_revision: actual, result } = client.events.recv().await.unwrap() else { panic!("controlled queries precede the default-pool ACK") };
            assert_eq!((owner, actual), (project, query_revision));
            assert_eq!(result.unwrap().rows.len(), 1);
            if query_revision == 0 {
                while client.events.len() != capacity || phase().2 != 2 { tokio::task::yield_now().await; }
            }
        }
        let Event::IntruderDefaultPoolReady { result, .. } = event(&mut client, |event| matches!(event, Event::IntruderDefaultPoolReady { operation_id: actual, .. } if *actual == operation_id)).await else { unreachable!() };
        assert!(result.unwrap().default_pool_id.is_some());
        assert!(cancel.is_cancelled());
        assert_eq!(phase(), (1, 1, 2));
        let binding: Option<String> = disk.query_row("SELECT pool_id FROM intruder_runs WHERE id=?1", [run_id.to_string()], |row| row.get(0)).unwrap();
        assert_eq!(binding, None);
        let listener = origin.await.unwrap();
        tokio::task::spawn_blocking(move || runtime.shutdown()).await.unwrap().unwrap();
        no_extra(listener);
    }).await.expect("default initialization ACK, queued Stop and one real response preserve dispatch ordering");
}
