use super::*;

#[tokio::test]
async fn failed_dispatch_is_unsent_and_saving_recovery_never_starts_it() {
    tokio::time::timeout(Duration::from_secs(4), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (mut state, _events) = state();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("unsent.mio");
        save(&mut state, Some(path)).await.unwrap();
        let id = admit(&mut state, spec(format!("http://{}", listener.local_addr().unwrap()), 1));
        state.store.reject_writes(true);
        assert!(state.advance_intruder(id).await.is_err());
        assert!(state.intruder.jobs.is_empty());
        assert!(state.stop_intruder().await.is_err());
        assert!(!state.intruder.has_resources());
        let page = state.intruder_result_page(id, None, 1).unwrap();
        assert!(!page.persisted);
        assert!(matches!(&page.run.state, IntruderRunState::Failed(_)));
        assert_eq!((page.run.total, page.run.dispatched, page.run.completed), (1, 1, 1));
        let IntruderResultOutcome::Flow(summary) = &page.rows[0].outcome else { panic!("unsent input remains inspectable") };
        let record = state.read_flow(summary.id).unwrap();
        assert_eq!(record.request.body, [0xff]);
        assert!(matches!(&record.summary.state, FlowState::Failed(message) if message.contains("未发送")));
        assert!(save(&mut state, None).await.is_err());
        assert_eq!(state.intruder_result_page(id, None, 1).unwrap(), page);
        state.store.reject_writes(false);
        save(&mut state, None).await.unwrap();
        let saved = state.intruder_result_page(id, None, 1).unwrap();
        assert!(saved.persisted);
        assert_eq!(saved.rows, page.rows);
        assert_eq!(saved.run, page.run);
        assert_eq!(state.store.read(summary.id).unwrap(), record);
        assert_eq!(state.unsaved_count(), 0);
        assert_eq!(listener.into_std().unwrap().accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    }).await.expect("unsent admission recovery and cleanup are bounded");
}

#[tokio::test]
async fn response_write_fault_preserves_real_response_and_cancels_other_admitted_posts() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen, received) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut first).await, 0xff);
            let (mut second, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut second).await, 0xff);
            seen.send(()).unwrap();
            released.await.unwrap();
            first
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\n\0\xffR",
                )
                .await
                .unwrap();
            drop(first);
            assert_eq!(second.read(&mut [0; 1]).await.unwrap(), 0);
            listener
        });
        let (mut state, _events) = state();
        let id = admit(&mut state, spec(format!("http://{address}"), 3));
        state.advance_intruder(id).await.unwrap();
        state.advance_intruder(id).await.unwrap();
        received.await.unwrap();
        assert_eq!(state.intruder.runs[&id].active, 2);
        assert_eq!(
            state.intruder.advance_candidate(),
            None,
            "per-run concurrency must prevent the third POST"
        );
        state.store.reject_writes(true);
        release.send(()).unwrap();
        let completion = state.intruder.jobs.join_next_with_id().await.unwrap();
        assert!(state.complete_intruder(completion).await.is_err());
        assert!(state.stop_intruder().await.is_err());
        let listener = origin.await.unwrap();
        assert!(!state.intruder.has_resources());
        let page = state.intruder_result_page(id, None, 2).unwrap();
        assert!(!page.persisted);
        assert!(matches!(page.run.state, IntruderRunState::Failed(_)));
        assert_eq!(
            (page.run.total, page.run.dispatched, page.run.completed),
            (3, 2, 2)
        );
        let summaries: Vec<_> = page
            .rows
            .iter()
            .map(|row| {
                let IntruderResultOutcome::Flow(summary) = &row.outcome else {
                    panic!("admitted POST must retain its flow")
                };
                summary
            })
            .collect();
        assert_eq!(
            summaries
                .iter()
                .filter(|summary| summary.state == FlowState::Complete)
                .count(),
            1
        );
        assert_eq!(
            summaries
                .iter()
                .filter(|summary| summary.state == FlowState::Cancelled)
                .count(),
            1
        );
        for summary in summaries {
            let record = state.read_flow(summary.id).unwrap();
            assert_eq!(record.request.body, [0xff]);
            if summary.state == FlowState::Complete {
                assert_eq!(record.response.unwrap().body, [0, 0xff, b'R']);
            }
        }
        let directory = tempfile::tempdir().unwrap();
        state.store.reject_writes(false);
        save(&mut state, Some(directory.path().join("recovered.mio")))
            .await
            .unwrap();
        let saved = state.intruder_result_page(id, None, 2).unwrap();
        assert!(saved.persisted);
        assert_eq!((saved.run, saved.rows), (page.run, page.rows));
        assert_eq!(state.unsaved_count(), 0);
        assert_eq!(
            listener.into_std().unwrap().accept().unwrap_err().kind(),
            ErrorKind::WouldBlock
        );
    })
    .await
    .expect("storage-fault cancellation and real origin cleanup are bounded");
}

#[tokio::test]
async fn stop_deadline_keeps_the_same_sender_owned_until_its_actual_join() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let (mut state, _events) = state();
        let id = admit(&mut state, spec("http://127.0.0.1:1".into(), 1));
        let mut record = super::super::tests::completed_flow();
        record.summary.source = TrafficSource::Intruder;
        record.summary.revision = 1;
        record.summary.state = FlowState::Pending;
        record.summary.status = None;
        record.summary.response_bytes = 0;
        record.response = None;
        state
            .persist_intruder_result(id, 0, Ok(Box::new(record.clone())))
            .unwrap();
        let (release, released) = oneshot::channel();
        let project = state.project;
        let flow_id = record.summary.id;
        let job = state.intruder.jobs.spawn(async move {
            released.await.unwrap();
            record.summary.revision += 1;
            record.summary.state = FlowState::Cancelled;
            Completion {
                project,
                record,
                reason_phrase: None,
            }
        });
        let task_id = job.id();
        state.intruder.requests.insert(
            task_id,
            ActiveRequest {
                run_id: id,
                ordinal: 0,
                flow_id,
                resource_pool: None,
            },
        );
        state.intruder.runs.get_mut(&id).unwrap().active = 1;
        let error = state
            .stop_intruder_until(tokio::time::Instant::now())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("保留任务句柄"));
        assert!(state.intruder.requests.contains_key(&task_id));
        assert_eq!(state.intruder.jobs.len(), 1);
        assert_eq!(
            state.intruder_run(id).unwrap().state,
            IntruderRunState::Stopping
        );
        assert_eq!(
            state.store.read(flow_id).unwrap().summary.state,
            FlowState::Pending
        );
        release.send(()).unwrap();
        state.stop_intruder().await.unwrap();
        assert!(!state.intruder.has_resources());
        assert!(state.intruder.requests.is_empty());
        assert_eq!(
            state.intruder_run(id).unwrap().state,
            IntruderRunState::Stopped
        );
        assert_eq!(
            state.store.read(flow_id).unwrap().summary.state,
            FlowState::Cancelled
        );
    })
    .await
    .expect("deadline regression uses a causal release, not a timed sleep");
}

#[tokio::test]
async fn quiesce_rejects_unjoined_intruder_even_when_storage_recovery_is_pending() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (mut state, mut events) = state();
        let directory = tempfile::tempdir().unwrap();
        save(&mut state, Some(directory.path().join("quiesce.mio")))
            .await
            .unwrap();
        let id = admit(&mut state, spec("http://127.0.0.1:1".into(), 1));
        let mut record = super::super::tests::completed_flow();
        record.summary.source = TrafficSource::Intruder;
        record.summary.revision = 1;
        record.summary.state = FlowState::Pending;
        record.summary.status = None;
        record.summary.response_bytes = 0;
        record.response = None;
        state
            .persist_intruder_result(id, 0, Ok(Box::new(record.clone())))
            .unwrap();
        let (release, released) = oneshot::channel();
        let project = state.project;
        let flow_id = record.summary.id;
        let job = state.intruder.jobs.spawn(async move {
            released.await.unwrap();
            record.summary.revision += 1;
            record.summary.state = FlowState::Cancelled;
            Completion {
                project,
                record,
                reason_phrase: None,
            }
        });
        let task_id = job.id();
        state.intruder.requests.insert(
            task_id,
            ActiveRequest {
                run_id: id,
                ordinal: 0,
                flow_id,
                resource_pool: None,
            },
        );
        state.intruder.runs.get_mut(&id).unwrap().active = 1;
        state.store.reject_writes(true);

        let blocked = state.command(Command::Quiesce { project }).await;
        let still_owned =
            state.intruder.requests.contains_key(&task_id) && state.intruder.jobs.len() == 1;
        let pending_recovery = state.unsaved_count();
        let mut premature_ack = false;
        while let Ok(event) = events.try_recv() {
            premature_ack |= matches!(event, Event::Quiesced { .. });
        }

        // Always release and join before asserting the failed deadline's observations.
        release.send(()).unwrap();
        let retried = state.command(Command::Quiesce { project }).await;
        let mut joined_ack = false;
        while let Ok(event) = events.try_recv() {
            joined_ack |= matches!(event, Event::Quiesced { project: key } if key == project);
        }
        state.store.reject_writes(false);
        let saved = save(&mut state, None).await;

        assert!(still_owned, "deadline must retain the actual sender handle");
        assert!(pending_recovery > 0);
        assert!(
            blocked.is_err(),
            "unjoined Intruder cannot acknowledge Quiesce"
        );
        assert!(!premature_ack);
        assert!(
            retried.is_ok(),
            "joined storage recovery must remain saveable"
        );
        assert!(joined_ack);
        assert!(!state.intruder.has_resources());
        saved.unwrap();
        assert_eq!(state.unsaved_count(), 0);
        assert_eq!(
            state.store.read(flow_id).unwrap().summary.state,
            FlowState::Cancelled
        );
    })
    .await
    .expect("Quiesce deadline and causal sender release must finish within five seconds");
}

#[tokio::test]
async fn opening_an_interrupted_snapshot_never_resumes_its_unsent_payloads() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("interrupted.mio");
        let input = spec(format!("http://{}", listener.local_addr().unwrap()), 2);
        let mut store = ProjectStore::create(Some(&path)).unwrap();
        let id = Uuid::new_v4();
        let tab = bind(&mut store, &input);
        store.create_intruder_run(id, tab, &input, 1).unwrap();
        let mut record = super::super::tests::completed_flow();
        record.summary.source = TrafficSource::Intruder;
        record.summary.revision = 1;
        record.summary.state = FlowState::Pending;
        record.summary.status = None;
        record.summary.response_bytes = 0;
        record.response = None;
        store.put_intruder_result(id, 0, Ok(&record)).unwrap();
        store.close().unwrap();
        let (mut state, _events) = state();
        let original = state.project;
        state.command(Command::OpenProject { path }).await.unwrap();
        assert_ne!(state.project.id, original.id);
        assert!(!state.intruder.has_resources());
        let page = state.intruder_result_page(id, None, 1).unwrap();
        assert!(page.persisted);
        assert_eq!(page.run.state, IntruderRunState::Interrupted);
        assert_eq!((page.run.total, page.run.dispatched, page.run.completed), (2, 1, 1));
        let restored = state.read_flow(record.summary.id).unwrap();
        assert_eq!(restored.request, record.request);
        assert!(matches!(&restored.summary.state, FlowState::Failed(message) if message.contains("结果未知") && message.contains("未自动重发")));
        assert_eq!(state.store.read_intruder_spec(id).unwrap(), input);
        assert!(state.admit_intruder(original, Uuid::new_v4(), Uuid::new_v4(), input.clone(), CancellationToken::new()).is_err());
        assert!(state.admit_intruder(state.project, id, Uuid::new_v4(), input, CancellationToken::new()).is_err());
        assert_eq!(listener.into_std().unwrap().accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    }).await.expect("opening a snapshot only performs bounded local recovery, never a send");
}

#[test]
fn recovered_run_summaries_stay_within_the_encoded_page_budget() {
    let (mut state, _events) = state();
    let ids: Vec<_> = (0..4)
        .map(|_| admit(&mut state, spec("http://127.0.0.1:1".into(), 1)))
        .collect();
    for id in ids {
        let mut run = state.intruder.runs.remove(&id).unwrap().summary;
        run.revision += 1;
        run.state = IntruderRunState::Failed("\0".repeat(64 * 1024));
        state
            .intruder
            .recovery
            .push(IntruderMutation::State { run });
    }
    let first = state.intruder_run_page(None, 500).unwrap();
    assert!(serde_json::to_vec(&first.rows).unwrap().len() <= RESULT_PAGE_BYTES);
    assert_eq!(first.rows.len(), 2);
    assert_eq!(first.next_after, first.rows.last().map(|row| row.id));
    let last = state.intruder_run_page(first.next_after, 500).unwrap();
    assert_eq!(last.rows.len(), 2);
    assert_eq!(last.next_after, None);
    assert!(serde_json::to_vec(&last.rows).unwrap().len() <= RESULT_PAGE_BYTES);
}
