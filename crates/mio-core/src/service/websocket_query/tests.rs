use super::*;

fn query(project: ProjectKey, revision: u64) -> WebSocketHistoryQuery {
    WebSocketHistoryQuery {
        project,
        query_revision: revision,
        filter: WebSocketHistoryFilter {
            term: "fixture".into(),
            ..Default::default()
        },
        flow_id: None,
        offset: 0,
        limit: 20,
        selected_message_id: None,
    }
}

fn empty_result() -> WebSocketHistoryResult {
    WebSocketHistoryResult {
        total: 0,
        rows: Vec::new(),
        selected_match: None,
    }
}

fn parked(state: &mut State, revision: u64) -> oneshot::Sender<()> {
    let (release, wait) = oneshot::channel();
    state.queries.websocket.active = Some(Active {
        query: query(state.project, revision),
        cancel: state.cancel.child_token(),
        worker: tokio::spawn(async move {
            wait.await.context("fixture release dropped")?;
            Ok(empty_result())
        }),
    });
    release
}

async fn complete_next(state: &mut State) {
    let signal = tokio::time::timeout(
        Duration::from_secs(3),
        state.queries.websocket.next_signal(&state.events, true),
    )
    .await
    .expect("query worker must join within fixture budget");
    assert!(matches!(signal, Signal::Finished(_)));
    state.websocket_query_signal(signal);
}

#[tokio::test]
async fn latest_request_waits_for_same_worker_then_starts_only_final_query() {
    let (events, _receiver) = mpsc::channel(8);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let release = parked(&mut state, 1);
    let first_id = state.queries.websocket.active.as_ref().unwrap().worker.id();
    state.query_websocket_history(query(state.project, 2));
    state.query_websocket_history(query(state.project, 3));
    let same_worker = state.queries.websocket.active.as_ref().unwrap().worker.id() == first_id;
    let first_cancelled = state
        .queries.websocket
        .active
        .as_ref()
        .unwrap()
        .cancel
        .is_cancelled();
    let latest = state
        .queries.websocket
        .latest
        .as_ref()
        .unwrap()
        .query_revision;
    release.send(()).unwrap();
    complete_next(&mut state).await;
    let last = state
        .queries.websocket
        .active
        .as_ref()
        .unwrap()
        .query
        .query_revision;
    let replaced = state.queries.websocket.active.as_ref().unwrap().worker.id() != first_id;
    complete_next(&mut state).await;
    assert!(same_worker && first_cancelled && replaced);
    assert_eq!((latest, last), (3, 3));
    assert!(!state.queries.websocket.has_worker());
    assert!(state.queries.websocket.latest.is_none());
    assert!(matches!(
        state.queries.websocket.result,
        Some(Event::WebSocketHistoryPage {
            query_revision: 3,
            total: 0,
            ..
        })
    ));
}

#[tokio::test]
async fn foreign_project_error_cannot_replace_current_completed_query_delivery() {
    let (events, mut receiver) = mpsc::channel(1);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let release = parked(&mut state, 1);
    state.events.try_send(Event::Stopped).unwrap();
    let old = ProjectKey {
        generation: state.project.generation - 1,
        ..state.project
    };
    state
        .command(Command::QueryWebSocketHistory {
            project: old,
            query_revision: 99,
            filter: WebSocketHistoryFilter::default(),
            flow_id: None,
            offset: 0,
            limit: 20,
            selected_message_id: None,
        })
        .await
        .unwrap();
    let active_not_cancelled = !state
        .queries.websocket
        .active
        .as_ref()
        .unwrap()
        .cancel
        .is_cancelled();
    release.send(()).unwrap();
    complete_next(&mut state).await;
    assert!(matches!(receiver.try_recv(), Ok(Event::Stopped)));
    let ready = tokio::time::timeout(
        Duration::from_secs(3),
        state.queries.websocket.next_signal(&state.events, false),
    )
    .await
    .unwrap();
    assert!(matches!(ready, Signal::Ready(_)));
    state.websocket_query_signal(ready);
    let delivered = receiver.try_recv().unwrap();
    assert!(active_not_cancelled);
    assert!(!state.queries.websocket.has_worker());
    assert!(
        matches!(delivered, Event::WebSocketHistoryPage {
        project, query_revision: 1, total: 0, rows, ..
    } if project == state.project && rows.is_empty()),
        "a foreign-project error must not consume the current query's only delivery slot"
    );
}

#[tokio::test]
async fn repeated_cancel_keeps_latest_query_error_until_worker_really_joins() {
    let (events, _receiver) = mpsc::channel(8);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let release = parked(&mut state, 1);
    state.query_websocket_history(query(state.project, 3));
    state.queries.websocket.cancel();
    let first_latest = matches!(
        state.queries.websocket.result,
        Some(Event::WebSocketHistoryError {
            query_revision: 3,
            ..
        })
    );
    state.queries.websocket.cancel();
    let second_latest = matches!(
        state.queries.websocket.result,
        Some(Event::WebSocketHistoryError {
            query_revision: 3,
            ..
        })
    );
    release.send(()).unwrap();
    state.finish_websocket_query_shutdown().await.unwrap();
    assert!(first_latest, "first cancellation belongs to newest query");
    assert!(
        second_latest,
        "repeated cancellation must not replace newest error with old worker revision"
    );
    assert!(matches!(
        state.queries.websocket.result,
        Some(Event::WebSocketHistoryError {
            query_revision: 3,
            ..
        })
    ));
    assert!(!state.queries.websocket.has_worker());
    assert!(state.queries.websocket.latest.is_none());
}

#[tokio::test(start_paused = true)]
async fn cancelled_current_worker_is_not_hidden_by_foreign_project_result() {
    let (events, _receiver) = mpsc::channel(8);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let release = parked(&mut state, 4);
    let old = ProjectKey {
        generation: state.project.generation - 1,
        ..state.project
    };
    state.queries.websocket.result = Some(query(old, 99).result(Err(anyhow::anyhow!("old query"))));
    state.queries.websocket.cancel();
    release.send(()).unwrap();
    state.finish_websocket_query_shutdown().await.unwrap();
    assert!(
        matches!(state.queries.websocket.result, Some(Event::WebSocketHistoryError {
        project, query_revision: 4, ..
    }) if project == state.project)
    );
}

#[tokio::test(start_paused = true)]
async fn deadline_and_save_as_keep_unjoined_reader_and_publish_nothing() {
    let (events, _receiver) = mpsc::channel(8);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let release = parked(&mut state, 1);
    let id = state.queries.websocket.active.as_ref().unwrap().worker.id();
    let source = state.store.test_actual_path();
    let original = state.store.workspace().unwrap();
    let project = state.project;
    let deadline = state
        .stop_websocket_query_until(tokio::time::Instant::now())
        .await;
    let same_after_deadline = state.queries.websocket.active.as_ref().unwrap().worker.id() == id;
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("not-published.mio");
    let saved = state
        .command(Command::SaveProject {
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project,
            path: Some(destination.clone()),
            workspace: None,
        })
        .await;
    let same_after_save = state.queries.websocket.active.as_ref().unwrap().worker.id() == id;
    let unpublished =
        !destination.exists() && std::fs::read_dir(directory.path()).unwrap().count() == 0;
    release.send(()).unwrap();
    state.finish_websocket_query_shutdown().await.unwrap();
    assert!(deadline.is_err());
    assert!(saved.is_err());
    assert!(same_after_deadline && same_after_save);
    assert!(
        unpublished,
        "Save As must not create staging or destination before reader join"
    );
    assert_eq!(state.project, project);
    assert_eq!(state.store.test_actual_path(), source);
    assert_eq!(state.store.workspace().unwrap(), original);
    assert!(source.exists());
    assert!(!state.queries.websocket.has_worker());
}

#[tokio::test]
async fn full_event_queue_does_not_make_proxy_stop_wait_for_search_worker() {
    let (events, mut receiver) = mpsc::channel(1);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let config = ProxyConfig::new(MitmAuthority::generate().unwrap());
    state.proxy.primary = Some(
        mio_proxy::start(config, state.project, state.proxy.sender.clone())
            .await
            .unwrap(),
    );
    let release = parked(&mut state, 1);
    let id = state.queries.websocket.active.as_ref().unwrap().worker.id();
    state.queries.websocket.result = Some(query(state.project, 1).result(Ok(empty_result())));
    state.events.try_send(Event::Stopped).unwrap();
    let stopped = tokio::time::timeout(
        Duration::from_secs(2),
        state.command(Command::StopProxy {
            project: state.project,
        }),
    )
    .await;
    let proxy_joined = state.proxy.primary.is_none();
    let retained = state
        .queries.websocket
        .active
        .as_ref()
        .is_some_and(|active| active.worker.id() == id && active.cancel.is_cancelled());
    release.send(()).unwrap();
    state.finish_websocket_query_shutdown().await.unwrap();
    if state.proxy.primary.is_some() {
        state.finish_proxy_shutdown().await.unwrap();
    }
    assert!(stopped.is_ok_and(|result| result.is_ok()));
    assert!(
        proxy_joined && retained,
        "Stop must join the proxy without waiting for parked search"
    );
    assert!(matches!(receiver.try_recv(), Ok(Event::Stopped)));
    assert!(
        receiver.try_recv().is_err(),
        "search result must not overtake a full event queue"
    );
    assert!(!state.queries.websocket.has_worker());
}

#[tokio::test]
async fn cancellation_after_precheck_still_joins_one_real_sixteen_mib_regex_match() {
    let (events, _receiver) = mpsc::channel(8);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let cancel = state.cancel.child_token();
    let worker_cancel = cancel.clone();
    let (entered, ready) = oneshot::channel();
    let (release, gate) = std::sync::mpsc::sync_channel(1);
    let (matched, observed) = oneshot::channel();
    let worker = tokio::task::spawn_blocking(move || {
        let body = vec![b'a'; 16 * 1024 * 1024];
        let matcher = regex::bytes::Regex::new(r"a+z$")?;
        ensure!(
            !worker_cancel.is_cancelled(),
            "cancelled before fixture checkpoint"
        );
        let _ = entered.send(());
        gate.recv_timeout(Duration::from_secs(3))
            .context("regex fixture gate timed out")?;
        let found = matcher.is_match(&body);
        let _ = matched.send((found, body.len(), worker_cancel.is_cancelled()));
        ensure!(
            !worker_cancel.is_cancelled(),
            "cancelled after indivisible match"
        );
        Ok(empty_result())
    });
    state.queries.websocket.active = Some(Active {
        query: query(state.project, 1),
        cancel,
        worker,
    });
    tokio::time::timeout(Duration::from_secs(3), ready)
        .await
        .unwrap()
        .unwrap();
    state.queries.websocket.cancel();
    release.send(()).unwrap();
    tokio::time::timeout(
        Duration::from_secs(3),
        state.finish_websocket_query_shutdown(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(observed.await.unwrap(), (false, 16 * 1024 * 1024, true));
    assert!(!state.queries.websocket.has_worker());
    assert!(matches!(
        state.queries.websocket.result,
        Some(Event::WebSocketHistoryError {
            query_revision: 1,
            ..
        })
    ));
}
