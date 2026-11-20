use super::*;

mod recovery;

fn query(project: ProjectKey, revision: u64) -> HttpHistoryQuery {
    HttpHistoryQuery {
        project,
        query_revision: revision,
        offset: 0,
        limit: 20,
        url_filter: "fixture".into(),
        filter: HttpHistoryFilter::default(),
        sort: HttpHistorySort::default(),
        selected_ids: Vec::new(),
    }
}

fn empty_result() -> HttpHistoryResult {
    HttpHistoryResult {
        total: 0,
        rows: Vec::new(),
        selected_matches: Vec::new(),
        selected_ranks: Vec::new(),
        selected_rows: Vec::new(),
        live_annotations: Vec::new(),
        live_numbers: Vec::new(),
        row_metadata: Vec::new(),
    }
}

fn parked(state: &mut State, revision: u64) -> oneshot::Sender<()> {
    let (release, wait) = oneshot::channel();
    state.http_query.active = Some(Active {
        query: query(state.project, revision),
        cancel: state.cancel.child_token(),
        worker: tokio::spawn(async move {
            wait.await.context("HTTP query fixture release dropped")?;
            Ok(empty_result())
        }),
    });
    release
}

async fn complete_next(state: &mut State) {
    let signal = tokio::time::timeout(
        Duration::from_secs(3),
        state.http_query.next_signal(&state.events, true),
    )
    .await
    .expect("HTTP query worker must join within the fixture budget");
    assert!(matches!(signal, Signal::Finished(_)));
    state.http_query_signal(signal);
}

#[tokio::test]
async fn latest_http_query_starts_only_after_the_same_cancelled_worker_joins() {
    let (events, _receiver) = mpsc::channel(8);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let release = parked(&mut state, 1);
    let first_id = state.http_query.active.as_ref().unwrap().worker.id();
    for revision in [2, 3] {
        state
            .command(Command::QueryHttpHistory {
                query: query(state.project, revision),
            })
            .await
            .unwrap();
    }
    let retained_id = state.http_query.active.as_ref().unwrap().worker.id();
    let cancelled = state
        .http_query
        .active
        .as_ref()
        .unwrap()
        .cancel
        .is_cancelled();
    let latest = state.http_query.latest.as_ref().unwrap().query_revision;
    let no_premature_result = state.http_query.result.is_none();
    release.send(()).unwrap();
    complete_next(&mut state).await;
    let started_id = state.http_query.active.as_ref().unwrap().worker.id();
    let started_revision = state
        .http_query
        .active
        .as_ref()
        .unwrap()
        .query
        .query_revision;
    complete_next(&mut state).await;

    assert_eq!(retained_id, first_id);
    assert!(cancelled && no_premature_result);
    assert_ne!(started_id, first_id);
    assert_eq!((latest, started_revision), (3, 3));
    assert!(!state.http_query.has_worker());
    assert!(state.http_query.latest.is_none());
    let Some(Event::HttpHistoryPage {
        query: actual,
        result: Ok(result),
    }) = &state.http_query.result
    else {
        panic!("the latest query must own the completed result")
    };
    assert_eq!(actual, &query(state.project, 3));
    assert_eq!(result, &empty_result());
}

#[tokio::test]
async fn foreign_http_query_cannot_steal_current_delivery_behind_a_full_event_queue() {
    let (events, mut receiver) = mpsc::channel(1);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let release = parked(&mut state, 1);
    state.events.try_send(Event::Stopped).unwrap();
    let foreign = ProjectKey {
        generation: state.project.generation - 1,
        ..state.project
    };
    state
        .command(Command::QueryHttpHistory {
            query: query(foreign, 99),
        })
        .await
        .unwrap();
    let active_not_cancelled = !state
        .http_query
        .active
        .as_ref()
        .unwrap()
        .cancel
        .is_cancelled();
    release.send(()).unwrap();
    complete_next(&mut state).await;
    assert!(matches!(receiver.try_recv(), Ok(Event::Stopped)));
    let signal = tokio::time::timeout(
        Duration::from_secs(3),
        state.http_query.next_signal(&state.events, false),
    )
    .await
    .unwrap();
    assert!(matches!(signal, Signal::Ready(_)));
    state.http_query_signal(signal);
    let Event::HttpHistoryPage {
        query: actual,
        result: Ok(result),
    } = receiver.try_recv().unwrap()
    else {
        panic!("the current query must keep the single delivery slot")
    };
    assert!(active_not_cancelled);
    assert!(!state.http_query.has_worker());
    assert_eq!(actual, query(state.project, 1));
    assert_eq!(result, empty_result());
    assert!(receiver.try_recv().is_err());
}

#[tokio::test(start_paused = true)]
async fn http_query_deadline_and_save_as_retain_the_reader_without_publishing_files() {
    let (events, _receiver) = mpsc::channel(8);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let release = parked(&mut state, 1);
    let id = state.http_query.active.as_ref().unwrap().worker.id();
    let source = state.store.test_actual_path();
    let original = state.store.workspace().unwrap();
    let project = state.project;
    let deadline = state
        .stop_http_query_until(tokio::time::Instant::now())
        .await;
    let retained_after_deadline = state.http_query.active.as_ref().unwrap().worker.id() == id;
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("not-published.mio");
    let save = state
        .command(Command::SaveProject {
            project,
            path: Some(destination.clone()),
            workspace: None,
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
        })
        .await;
    let retained_after_save = state.http_query.active.as_ref().unwrap().worker.id() == id;
    let cancelled = state
        .http_query
        .active
        .as_ref()
        .unwrap()
        .cancel
        .is_cancelled();
    let unpublished =
        !destination.exists() && std::fs::read_dir(directory.path()).unwrap().count() == 0;
    release.send(()).unwrap();
    state.finish_http_query_shutdown().await.unwrap();

    assert!(deadline.is_err() && save.is_err());
    assert!(retained_after_deadline && retained_after_save && cancelled);
    assert!(
        unpublished,
        "Save As must wait before creating staging or destination"
    );
    assert_eq!(state.project, project);
    assert_eq!(state.store.test_actual_path(), source);
    assert_eq!(state.store.workspace().unwrap(), original);
    assert!(source.exists());
    assert!(!state.http_query.has_worker());
}

#[tokio::test]
async fn full_event_queue_proxy_stop_cancels_but_does_not_wait_for_http_reader() {
    let (events, mut receiver) = mpsc::channel(1);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    state.proxy = Some(
        mio_proxy::start(
            ProxyConfig::new(MitmAuthority::generate().unwrap()),
            state.project,
            state.proxy_sender.clone(),
        )
        .await
        .unwrap(),
    );
    let release = parked(&mut state, 1);
    let id = state.http_query.active.as_ref().unwrap().worker.id();
    state.http_query.result = Some(Event::HttpHistoryPage {
        query: query(state.project, 1),
        result: Ok(empty_result()),
    });
    state.events.try_send(Event::Stopped).unwrap();
    let stopped = tokio::time::timeout(
        Duration::from_secs(2),
        state.command(Command::StopProxy {
            project: state.project,
        }),
    )
    .await;
    let proxy_joined = state.proxy.is_none();
    let reader_retained = state
        .http_query
        .active
        .as_ref()
        .is_some_and(|active| active.worker.id() == id && active.cancel.is_cancelled());
    release.send(()).unwrap();
    state.finish_http_query_shutdown().await.unwrap();
    if state.proxy.is_some() {
        state.finish_proxy_shutdown().await.unwrap();
    }

    assert!(stopped.is_ok_and(|result| result.is_ok()));
    assert!(proxy_joined && reader_retained);
    assert!(matches!(receiver.try_recv(), Ok(Event::Stopped)));
    assert!(
        receiver.try_recv().is_err(),
        "query delivery cannot bypass backpressure"
    );
    assert!(!state.http_query.has_worker());
}

#[tokio::test]
async fn http_query_keeps_original_scalars_and_latest_recovery_state_without_changing_records() {
    for persisted in [false, true] {
        let (events, _receiver) = mpsc::channel(8);
        let mut state = State::new(events, CancellationToken::new()).unwrap();
        let mut original = super::super::tests::completed_flow();
        original.summary.source = TrafficSource::Proxy;
        original.summary.url = "http://127.0.0.1:9001/fixture/original-only".into();
        original.request.url = original.summary.url.clone();
        original.summary.status = Some(201);
        original.response.as_mut().unwrap().status = 201;
        if persisted {
            state.store.put(&original).unwrap();
        }
        let mut intermediate = original.clone();
        intermediate.summary.revision += 1;
        if persisted {
            intermediate.summary.status = Some(202);
            intermediate.response.as_mut().unwrap().status = 202;
        }
        let mut latest = intermediate.clone();
        latest.summary.revision += 1;
        latest.summary.status = Some(404);
        latest.response.as_mut().unwrap().status = 404;
        latest.summary.method = "PUT".into();
        latest.request.method = "PUT".into();
        latest.summary.url = "http://127.0.0.1:9001/fixture/edited-only".into();
        latest.request.url = latest.summary.url.clone();
        latest.response.as_mut().unwrap().body = vec![0, 254, b'B'];
        latest.summary.response_bytes = 3;
        let mut foreign = latest.clone();
        foreign.summary.id = Uuid::new_v4();
        foreign.summary.source = TrafficSource::Repeater;
        state.recovery.extend([
            intermediate.into(),
            latest.clone().into(),
            foreign.clone().into(),
        ]);
        let source_path = state.store.test_actual_path();
        let original_workspace = state.store.workspace().unwrap();
        let original_recovery = state
            .recovery
            .iter()
            .map(|capture| capture.record.clone())
            .collect::<Vec<_>>();
        let mut requested = query(state.project, 1);
        requested.filter.status_classes = [true, false, false, false];
        requested.url_filter = "original-only".into();
        requested.selected_ids = vec![HistoryEntryId::Live(original.summary.id)];
        state
            .command(Command::QueryHttpHistory {
                query: requested.clone(),
            })
            .await
            .unwrap();
        complete_next(&mut state).await;
        let Some(Event::HttpHistoryPage {
            query: actual,
            result: Ok(result),
        }) = &state.http_query.result
        else {
            panic!("valid recovered flow must produce a query result")
        };
        let mut displayed = latest.summary.clone();
        displayed.method = original.request.method.clone();
        displayed.url = original.request.url.clone();
        displayed.status = Some(201);
        assert_eq!(actual, &requested);
        assert_eq!(result.total, 1);
        assert_eq!(result.rows, [HistoryRow::Live(displayed)]);
        assert_eq!(result.selected_matches, [(requested.selected_ids[0], true)]);
        assert_eq!(result.selected_ranks, [(requested.selected_ids[0], 0)]);
        requested.query_revision += 1;
        requested.filter.search_term = r"\x00\xFEB".into();
        requested.filter.regex = true;
        state
            .command(Command::QueryHttpHistory {
                query: requested.clone(),
            })
            .await
            .unwrap();
        complete_next(&mut state).await;
        assert!(
            matches!(&state.http_query.result, Some(Event::HttpHistoryPage { result: Ok(result), .. })
            if result.total == 1 && result.rows.len() == 1
            && result.selected_matches == [(requested.selected_ids[0], true)]
            && result.selected_ranks == [(requested.selected_ids[0], 0)])
        );
        requested.query_revision += 1;
        requested.filter.search_term.clear();
        requested.filter.regex = false;
        requested.filter.status_classes = [false, false, true, false];
        requested.url_filter.clear();
        state
            .command(Command::QueryHttpHistory {
                query: requested.clone(),
            })
            .await
            .unwrap();
        complete_next(&mut state).await;
        assert!(
            matches!(&state.http_query.result, Some(Event::HttpHistoryPage { result: Ok(result), .. })
            if result.total == 0 && result.rows.is_empty()
            && result.selected_matches == [(requested.selected_ids[0], false)]
            && result.selected_ranks.is_empty())
        );
        if persisted {
            assert_eq!(state.store.read(original.summary.id).unwrap(), original);
        } else {
            assert!(
                state
                    .store
                    .read_optional(original.summary.id)
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(state.read_flow(latest.summary.id).unwrap(), latest);
        assert_eq!(state.read_flow(foreign.summary.id).unwrap(), foreign);
        assert_eq!(state.store.test_actual_path(), source_path);
        assert_eq!(state.store.workspace().unwrap(), original_workspace);
        assert_eq!(
            state
                .recovery
                .iter()
                .map(|capture| capture.record.clone())
                .collect::<Vec<_>>(),
            original_recovery
        );
        assert!(!state.http_query.has_worker());
    }
}
