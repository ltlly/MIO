use super::*;

#[tokio::test]
async fn rebinding_keeps_the_old_pool_owned_until_join_without_cancelling_another_pool() {
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
        let other_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let other_address = other_listener.local_addr().unwrap();
        let (other_seen_tx, other_seen) = tokio::sync::oneshot::channel();
        let (release_other, other_released) = tokio::sync::oneshot::channel();
        let other_origin = tokio::spawn(async move {
            let (mut stream, _) = other_listener.accept().await.unwrap();
            assert_eq!(request(&mut stream).await, 255);
            other_seen_tx.send(()).unwrap();
            other_released.await.unwrap();
            respond(&mut stream).await;
            other_listener
        });
        let (mut state, _events) = state();
        let first_pool = pool("first", Some(1));
        let second_pool = pool("second", Some(1));
        let original_catalog = install(&mut state, vec![first_pool.clone(), second_pool.clone()]);
        // The frozen spec has no first-pool reference: only the in-flight ownership protects it.
        let input = spec(format!("http://{address}"), 2);
        let first = admit(&mut state, input.clone());
        assert_eq!(state.intruder_run(first).unwrap().resource_pool_revision, 0);
        assert_eq!(
            rebind(&mut state, first, first_pool.id).resource_pool_revision,
            1
        );
        let mut other_input = spec(format!("http://{other_address}"), 1);
        other_input.resource_pool = Some(IntruderPoolSelection::Existing(second_pool.id));
        let second = admit(&mut state, other_input);
        let other_cancel = state.intruder.runs[&second].cancel.clone();
        state.advance_intruder(first).await.unwrap();
        state.advance_intruder(second).await.unwrap();
        seen.await.unwrap();
        other_seen.await.unwrap();
        assert_eq!(
            state.intruder.requests.len(),
            2,
            "independent one-slot pools run concurrently"
        );
        state
            .set_intruder_paused(state.project, first, true)
            .unwrap();
        state
            .rename_intruder(state.project, first, Uuid::new_v4(), 0, "kept name")
            .unwrap();
        let rebound = rebind(&mut state, first, second_pool.id);
        assert_eq!(rebound.resource_pool_revision, 2);
        assert_eq!(rebound.state, IntruderRunState::Paused);
        assert_eq!(rebound.title, "kept name");
        assert_eq!(
            state
                .intruder
                .requests
                .values()
                .find(|request| request.run_id == first)
                .unwrap()
                .resource_pool,
            Some(first_pool.id)
        );
        let deletion = state
            .set_intruder_resource_pools(
                state.project,
                Uuid::new_v4(),
                original_catalog.revision,
                vec![second_pool.clone()],
                &CancellationToken::new(),
            )
            .unwrap_err();
        assert!(deletion.to_string().contains("在途请求"));
        assert_eq!(state.intruder.resource_pools, original_catalog);
        state.request_intruder_stop(state.project, first).unwrap();
        finish_next(&mut state).await;
        state.advance_intruder(first).await.unwrap();
        state.advance_intruder(first).await.unwrap();
        let stopped = state.intruder_run(first).unwrap();
        assert_eq!(stopped.state, IntruderRunState::Stopped);
        assert_eq!(
            (stopped.resource_pool, stopped.resource_pool_revision),
            (Some(second_pool.id), 2)
        );
        assert_eq!(
            (stopped.title.as_str(), stopped.title_revision),
            ("kept name", 1)
        );
        assert!(!other_cancel.is_cancelled());
        assert_eq!(state.intruder.requests.len(), 1);
        let deleted = install(&mut state, vec![second_pool]);
        assert!(
            deleted.find(first_pool.id).is_none(),
            "only the actual join releases the old pool reference"
        );
        assert_eq!(state.store.read_intruder_spec(first).unwrap(), input);
        release_other.send(()).unwrap();
        finish_next(&mut state).await;
        state.advance_intruder(second).await.unwrap();
        assert!(!state.intruder.has_resources());
        no_extra(origin.await.unwrap());
        no_extra(other_origin.await.unwrap());
    })
    .await
    .expect("two pools, live binding and isolated Stop preserve send-time ownership");
}

#[tokio::test]
async fn rebound_completion_write_failure_preserves_binding_title_and_old_pool_ownership_until_join()
 {
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
                    value => panic!("unexpected fixture payload: {value}"),
                }
            }
            seen_tx.send(()).unwrap();
            released.await.unwrap();
            let mut first = first.unwrap();
            first.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\n\x00\xffR").await.unwrap();
            drop(first);
            // The storage fault cancels the other socket, but its pool slot lasts until join.
            assert_eq!(other.unwrap().read(&mut [0; 1]).await.unwrap(), 0);
            listener
        });
        let (mut state, _events) = state();
        let mut original_pool = pool("original", Some(2));
        let current_pool = pool("current", Some(1));
        install(&mut state, vec![original_pool.clone(), current_pool.clone()]);
        let mut input = spec(format!("http://{address}"), 2);
        input.max_concurrent_requests = 1;
        input.resource_pool = Some(IntruderPoolSelection::Existing(original_pool.id));
        input.payload_sets = vec![vec![vec![b'A'], vec![b'A']]];
        let first = admit(&mut state, input.clone());
        let mut other_input = input.clone();
        other_input.payload_sets = vec![vec![vec![b'B'], vec![b'B']]];
        let other = admit(&mut state, other_input);
        state.advance_intruder(first).await.unwrap();
        state.advance_intruder(other).await.unwrap();
        seen.await.unwrap();
        let first_flow = state.intruder.requests.values().find(|request| request.run_id == first).unwrap().flow_id;
        let other_flow = state.intruder.requests.values().find(|request| request.run_id == other).unwrap().flow_id;
        state.rename_intruder(state.project, first, Uuid::new_v4(), 0, "kept after recovery").unwrap();
        let before = rebind(&mut state, first, current_pool.id);
        assert_eq!((before.resource_pool, before.resource_pool_revision), (Some(current_pool.id), 2));
        assert_eq!((before.title.as_str(), before.title_revision), ("kept after recovery", 1));
        assert!(state.intruder.requests.values().all(|request| request.resource_pool == Some(original_pool.id)));
        original_pool.max_concurrent_requests = Some(1);
        let catalog = install(&mut state, vec![original_pool.clone(), current_pool]);
        assert!(!state.intruder.pool_has_capacity(&state.intruder.runs[&other]));
        state.store.reject_writes(true);
        release.send(()).unwrap();
        let completion = state.intruder.jobs.join_next_with_id().await.unwrap();
        let Ok((_, completed)) = &completion else { panic!("the released original request must finish normally") };
        assert_eq!(completed.record.summary.id, first_flow);
        assert_eq!(state.intruder.requests.len(), 2, "only processing the joined completion releases a send-time pool slot");
        assert!(state.complete_intruder(completion).await.is_err());
        let recovering = state.intruder_run(first).unwrap();
        assert_eq!((recovering.resource_pool, recovering.resource_pool_revision), (before.resource_pool, 2));
        assert_eq!((recovering.title.as_str(), recovering.title_revision), (before.title.as_str(), 1));
        assert_eq!((recovering.revision, recovering.dispatched, recovering.completed), (before.revision + 1, 1, 1));
        assert_eq!(recovering.state, IntruderRunState::Running);
        let kept = state.read_flow(first_flow).unwrap();
        assert_eq!(kept.summary.state, FlowState::Complete);
        assert_eq!(kept.request.body, [b'A']);
        assert_eq!(kept.response.as_ref().unwrap().body, [0, 255, b'R']);
        assert_eq!(state.store.read(first_flow).unwrap().summary.state, FlowState::Pending);
        assert!(state.intruder.recovery.iter().any(|mutation| matches!(mutation,
            IntruderMutation::Result { run, ordinal: 0, result: Ok(record) }
                if run == &recovering && record.summary.id == first_flow && record.as_ref() == &kept)));
        assert_eq!(state.intruder.requests.len(), 1);
        let remaining = state.intruder.requests.values().next().unwrap();
        assert_eq!((remaining.run_id, remaining.flow_id, remaining.resource_pool), (other, other_flow, Some(original_pool.id)));
        assert_eq!(state.intruder.runs[&other].active, 1);
        assert!(state.intruder.runs[&other].cancel.is_cancelled());
        assert!(!state.intruder.pool_has_capacity(&state.intruder.runs[&other]), "the original pool remains occupied by its other unjoined request even after cancellation");
        assert!(state.stop_intruder().await.is_err());
        let listener = origin.await.unwrap();
        assert!(!state.intruder.has_resources());
        assert!(state.intruder.requests.is_empty());
        let unsaved = state.intruder_result_page(first, None, 2).unwrap();
        let other_unsaved = state.intruder_result_page(other, None, 2).unwrap();
        assert!(!unsaved.persisted);
        assert!(matches!(unsaved.run.state, IntruderRunState::Failed(_)));
        assert_eq!((unsaved.run.total, unsaved.run.dispatched, unsaved.run.completed), (2, 1, 1));
        assert_eq!((unsaved.run.resource_pool, unsaved.run.resource_pool_revision), (before.resource_pool, 2));
        assert_eq!((unsaved.run.title.as_str(), unsaved.run.title_revision), (before.title.as_str(), 1));
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        state.store.reject_writes(false);
        save(&mut state, Some(directory.path().join("rebound-recovery.mio"))).await.unwrap();
        let saved = state.intruder_result_page(first, None, 2).unwrap();
        let other_saved = state.intruder_result_page(other, None, 2).unwrap();
        assert!(saved.persisted && other_saved.persisted);
        assert_eq!((&saved.run, &saved.rows), (&unsaved.run, &unsaved.rows));
        assert_eq!((&other_saved.run, &other_saved.rows), (&other_unsaved.run, &other_unsaved.rows));
        assert_eq!(state.store.read(first_flow).unwrap(), kept);
        assert_eq!(state.store.read_intruder_spec(first).unwrap(), input);
        assert_eq!(state.intruder.resource_pools, catalog);
        assert_eq!(state.unsaved_count(), 0);
        no_extra(listener);
    }).await.expect("two tiny sockets preserve binding, title, recovery and send-time ownership across a write fault");
}
