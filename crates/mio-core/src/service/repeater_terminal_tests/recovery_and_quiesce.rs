use super::*;
use std::future::Future;

#[tokio::test]
async fn repeater_recovery_exposes_ordered_received_and_final_before_save_without_resending() {
    tokio::time::timeout(Duration::from_secs(8), async {
        for save_as in [false, true] {
            let (events, mut events_rx) = mpsc::channel(64);
            let mut state = State::new(events, CancellationToken::new()).unwrap();
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("repeater-recovery.mio");
            let copy = directory.path().join("repeater-copy.mio");
            state.command(Command::SaveProject { websocket_notes: Vec::new(), ext_http_notes: Vec::new(), ext_http_highlights: Vec::new(), project: state.project, path: Some(path.clone()), workspace: None }).await.unwrap();
            let response = gzip_response();
            let (tab_id, release, origin) = controlled_response_send(&mut state, &response, true, true).await;
            drain_events(&mut events_rx);
            state.store.reject_writes(true);
            release.send(()).unwrap();
            let completion = state.repeater.jobs.join_next().await.unwrap().unwrap();
            let listener = origin.await.unwrap();
            let received = completion.received.as_ref().unwrap().as_ref().clone();
            let final_record = completion.record.clone();
            let pending = state.store.read(final_record.summary.id).unwrap();
            assert!(state.complete(completion).await.is_err());
            assert_eq!(state.storage.recovery.iter().map(|capture| &capture.record).collect::<Vec<_>>(), [&received, &final_record]);
            assert!(state.persist_repeater_completion(Some(&received), &final_record).is_err());
            assert_eq!(state.storage.recovery.len(), 2, "repeated recovery completion cannot duplicate the pair");
            assert_eq!(state.store.read(final_record.summary.id).unwrap(), pending);
            let finished = drain_events(&mut events_rx);
            assert_eq!(finished.iter().filter(|event| matches!(event, Event::SendFinished { tab_id: id, record, persisted: false, .. } if *id == tab_id && **record == final_record)).count(), 1);
            state.command(Command::ReadFlow { project: state.project, selection_revision: 9, flow_id: final_record.summary.id }).await.unwrap();
            let Event::FlowDetail { record, original, .. } = events_rx.try_recv().unwrap() else { panic!("recovery ReadFlow returns a complete detail"); };
            assert_eq!(*record, final_record);
            assert_eq!(original.response, received.response);
            assert_eq!(original.request, None);
            assert!(state.command(Command::SaveProject { websocket_notes: Vec::new(), ext_http_notes: Vec::new(), ext_http_highlights: Vec::new(), project: state.project, path: None, workspace: None }).await.is_err());
            assert_eq!(state.storage.recovery.len(), 2);
            if !save_as { state.store.reject_writes(false); }
            state.command(Command::SaveProject { websocket_notes: Vec::new(), ext_http_notes: Vec::new(), ext_http_highlights: Vec::new(), project: state.project, path: save_as.then(|| copy.clone()), workspace: None }).await.unwrap();
            assert_eq!(state.unsaved_count(), 0);
            if !save_as {
                state.command(Command::SaveProject { websocket_notes: Vec::new(), ext_http_notes: Vec::new(), ext_http_highlights: Vec::new(), project: state.project, path: Some(copy.clone()), workspace: None }).await.unwrap();
            }
            assert!(state.repeater.sends.is_empty() && state.repeater.jobs.is_empty());
            state.store.close().unwrap();
            for saved in [&path, &copy] {
                let reopened = ProjectStore::open(saved).unwrap();
                if save_as && saved == &path {
                    assert_eq!(reopened.read(pending.summary.id).unwrap(), pending);
                } else {
                    assert_eq!(reopened.read(final_record.summary.id).unwrap(), final_record);
                    assert_eq!(reopened.original(final_record.summary.id).unwrap(), *original);
                }
                reopened.close().unwrap();
            }
            assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
        }
    }).await.expect("recovery, both save routes, reopens and origin joins stay bounded");
}

#[tokio::test]
async fn repeater_stop_deadline_retains_the_same_task_and_response_for_retry() {
    tokio::time::timeout(Duration::from_secs(7), async {
        let (events, mut received) = mpsc::channel(64);
        let mut state = State::new(events, CancellationToken::new()).unwrap();
        let held = held_send(&mut state).await;
        let stopped = state.stop_resources().await;
        let retained = state.repeater.jobs.len() == 1
            && state.repeater.sends.get(&held.tab_id).is_some_and(|active| {
                active.task_id == held.task.id() && active.flow_id == held.expected.summary.id
            })
            && !held.task.is_finished();
        let before_release = state.store.read(held.expected.summary.id).unwrap();
        let early_events = drain_events(&mut received);

        // Release and join even on the old aborting path before checking the observations.
        let released = held.release.send(());
        let retried = state.stop_resources().await;
        let final_record = state.store.read(held.expected.summary.id).unwrap();
        let final_events = drain_events(&mut received);
        let extra_request = held.listener.accept();

        assert!(retained, "a Stop deadline must retain the same sender handle");
        assert!(stopped.unwrap_err().to_string().contains("保留任务句柄"));
        assert_eq!(before_release.summary.state, FlowState::Pending);
        assert!(!early_events.iter().any(|event| matches!(event, Event::SendFinished { .. })));
        assert!(released.is_ok(), "the completion tail was not aborted");
        retried.unwrap();
        assert!(state.repeater.jobs.is_empty() && state.repeater.sends.is_empty());
        assert_eq!(final_record, held.expected);
        assert_eq!(final_events.iter().filter(|event| matches!(event, Event::SendFinished { record, persisted: true, .. } if **record == held.expected)).count(), 1);
        assert_eq!(extra_request.unwrap_err().kind(), ErrorKind::WouldBlock);
    })
    .await
    .expect("Stop deadline, causal release and every fixture join are bounded");
}

#[tokio::test]
async fn unjoined_repeater_blocks_quiesce_even_with_pending_storage_recovery() {
    tokio::time::timeout(Duration::from_secs(7), async {
        let (events, mut received) = mpsc::channel(64);
        let mut state = State::new(events, CancellationToken::new()).unwrap();
        let project = state.project;
        let directory = tempfile::tempdir().unwrap();
        state.command(Command::SaveProject {
            websocket_notes: Vec::new(), ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project,
            path: Some(directory.path().join("repeater-recovery.mio")),
            workspace: None,
        }).await.unwrap();
        drain_events(&mut received);
        let held = held_send(&mut state).await;
        state.store.reject_writes(true);
        state.persist(&tests::completed_flow()).unwrap_err();

        let blocked = state.command(Command::Quiesce { project }).await;
        let retained = state.repeater.jobs.len() == 1 && state.repeater.sends.contains_key(&held.tab_id);
        let pending_recovery = state.unsaved_count();
        let early_events = drain_events(&mut received);
        let released = held.release.send(());
        let retried = state.command(Command::Quiesce { project }).await;
        let joined_events = drain_events(&mut received);
        state.store.reject_writes(false);
        let saved = state.command(Command::SaveProject {
            websocket_notes: Vec::new(), ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project,
            path: None,
            workspace: None,
        }).await;
        let final_record = state.store.read(held.expected.summary.id).unwrap();
        let extra_request = held.listener.accept();

        assert!(retained, "Quiesce must retain a sender that has not joined");
        assert!(pending_recovery > 0);
        assert!(blocked.is_err(), "storage recovery cannot hide a live sender");
        assert!(!early_events.iter().any(|event| matches!(event, Event::Quiesced { .. })));
        assert!(released.is_ok());
        retried.unwrap();
        assert_eq!(joined_events.iter().filter(|event| matches!(event, Event::Quiesced { project: actual } if *actual == project)).count(), 1);
        saved.unwrap();
        assert!(state.repeater.jobs.is_empty() && state.repeater.sends.is_empty());
        assert_eq!(state.unsaved_count(), 0);
        assert_eq!(final_record, held.expected);
        assert_eq!(extra_request.unwrap_err().kind(), ErrorKind::WouldBlock);
    })
    .await
    .expect("Quiesce deadline and causal sender release finish without sleeps");
}

#[tokio::test]
async fn service_shutdown_keeps_repeater_ownership_after_its_stop_deadline() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let (events, mut received) = mpsc::channel(64);
        let mut state = State::new(events, CancellationToken::new()).unwrap();
        let project = state.project;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("repeater-shutdown.mio");
        state
            .command(Command::SaveProject {
                websocket_notes: Vec::new(),
                ext_http_notes: Vec::new(),
                ext_http_highlights: Vec::new(),
                project,
                path: Some(path.clone()),
                workspace: None,
            })
            .await
            .unwrap();
        drain_events(&mut received);
        let held = held_send(&mut state).await;
        let cancel = state.cancel.clone();
        let (_commands, incoming) = mpsc::channel(1);
        let run = state.run(incoming);
        tokio::pin!(run);
        let initialized = std::future::poll_fn(|cx| {
            std::task::Poll::Ready(match run.as_mut().poll(cx) {
                std::task::Poll::Pending => None,
                std::task::Poll::Ready(result) => Some(result),
            })
        })
        .await;
        let started_running = initialized.is_none();
        cancel.cancel();
        let early = match initialized {
            Some(result) => Some(result),
            None => tokio::time::timeout(Duration::from_secs(4), &mut run)
                .await
                .ok(),
        };
        let returned_early = early.is_some();
        let finished_early = held.task.is_finished();
        let early_events = drain_events(&mut received);
        let released = held.release.send(());
        let result = match early {
            Some(result) => result,
            None => run.await,
        };
        let final_events = drain_events(&mut received);
        let reopened = ProjectStore::open(&path).unwrap();
        let final_record = reopened.read(held.expected.summary.id).unwrap();
        reopened.close().unwrap();
        let extra_request = held.listener.accept();

        assert!(started_running, "service entered its loop before shutdown");
        assert!(
            !returned_early,
            "service shutdown cannot drop an unjoined sender"
        );
        assert!(!finished_early);
        assert!(
            !early_events
                .iter()
                .any(|event| matches!(event, Event::Stopped))
        );
        assert!(released.is_ok());
        assert!(held.task.is_finished());
        assert!(result.unwrap_err().to_string().contains("保留任务句柄"));
        assert_eq!(
            final_events
                .iter()
                .filter(|event| matches!(event, Event::Stopped))
                .count(),
            1
        );
        assert_eq!(final_record, held.expected);
        assert_eq!(extra_request.unwrap_err().kind(), ErrorKind::WouldBlock);
    })
    .await
    .expect("final shutdown retains ownership until the causal release, then joins");
}

#[tokio::test]
async fn repeater_send_uses_activation_settings_despite_newer_saved_defaults() {
    tokio::time::timeout(Duration::from_secs(8), async {
        for captured in [false, true] {
            let (events, mut receiver) = mpsc::channel(64);
            let mut state = State::new(events, CancellationToken::new()).unwrap();
            let response = gzip_response();
            let (tab_id, release, origin) =
                controlled_response_send(&mut state, &response, !captured, captured).await;
            for next in [captured, !captured] {
                let mut workspace = state.store.workspace().unwrap();
                workspace.revision += 1;
                workspace.repeater_settings.unpack_compressed_responses = next;
                state.command(Command::SaveWorkspace { project: state.project, workspace }).await.unwrap();
            }
            release.send(()).unwrap();
            let completion = state.repeater.jobs.join_next().await.unwrap().unwrap();
            assert_eq!(completion.tab_id, tab_id);
            assert_eq!(completion.received.is_some(), captured);
            let record = completion.record.clone();
            assert_eq!(record.summary.state, FlowState::Complete);
            assert_eq!(record.summary.revision, if captured { 3 } else { 2 });
            assert_eq!(record.response.as_ref().unwrap().body, if captured { RESPONSE.to_vec() } else { response.body.clone() });
            state.complete(completion).await.unwrap();
            assert_eq!(state.store.read(record.summary.id).unwrap(), record);
            assert_eq!(state.store.original(record.summary.id).unwrap().response, captured.then_some(response));
            assert_eq!(drain_events(&mut receiver).iter().filter(|event| matches!(event, Event::SendFinished { tab_id: id, persisted: true, .. } if *id == tab_id)).count(), 1);
            let listener = origin.await.unwrap();
            assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
        }
    }).await.expect("two controlled responses, setting changes, persistence and joins stay bounded");
}
