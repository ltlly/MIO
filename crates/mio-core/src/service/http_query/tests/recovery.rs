use super::*;

#[tokio::test]
async fn http_query_parameterized_recovery_uses_original_requests_through_delivery_and_save_as() {
    for persisted in [false, true] {
        let (events, mut receiver) = mpsc::channel(32);
        let mut state = State::new(events, CancellationToken::new()).unwrap();
        let originals: Vec<_> = [true, false]
            .into_iter()
            .map(|parameterized| {
                let mut record = crate::service::tests::completed_flow();
                record.summary.source = TrafficSource::Proxy;
                record.request.url = format!(
                    "http://127.0.0.1:9001/fixture/original-{}",
                    if parameterized { "form" } else { "binary" }
                );
                record.summary.url = record.request.url.clone();
                record.request.headers = vec![Header {
                    name: "Content-Type".into(),
                    value: if parameterized {
                        b"application/x-www-form-urlencoded".to_vec()
                    } else {
                        b"application/octet-stream".to_vec()
                    },
                }];
                record.request.body = if parameterized {
                    b"name=value".to_vec()
                } else {
                    b"&&&".to_vec()
                };
                record.summary.request_bytes = record.request.body.len() as u64;
                record
            })
            .collect();
        let edited: Vec<_> = originals
            .iter()
            .enumerate()
            .map(|(index, original)| {
                let mut record = original.clone();
                record.summary.revision += 1;
                record.request.headers = originals[1 - index].request.headers.clone();
                record.request.body = originals[1 - index].request.body.clone();
                record.summary.request_bytes = record.request.body.len() as u64;
                record
            })
            .collect();
        if persisted {
            for (original, changed) in originals.iter().zip(&edited) {
                state.persist(original).unwrap();
                state.persist(changed).unwrap();
                assert_eq!(
                    state
                        .store
                        .original(original.summary.id)
                        .unwrap()
                        .request
                        .as_ref(),
                    Some(&original.request)
                );
            }
        }
        state.store.reject_writes(true);
        let mut latest = Vec::new();
        for (original, changed) in originals.iter().zip(&edited) {
            if !persisted {
                assert!(state.persist(original).is_err());
            }
            let mut pending = changed.clone();
            if persisted {
                pending.summary.revision += 1;
            }
            assert!(state.persist(&pending).is_err());
            pending.summary.revision += 1;
            pending.response.as_mut().unwrap().body = vec![0, 254, b'R'];
            pending.summary.response_bytes = 3;
            assert!(state.persist(&pending).is_err());
            latest.push(pending);
        }
        let recovery_before = state.storage.recovery.clone();
        let workspace_before = state.store.workspace().unwrap();
        let source_path = state.store.test_actual_path();
        let original_project = state.project;
        let directory = tempfile::tempdir().unwrap();
        let saved_path = directory.path().join("parameterized-recovery.mio");

        for phase in 0..2 {
            let mut requested = query(state.project, phase + 1);
            requested.filter.parameterized_only = true;
            requested.limit = 1;
            requested.selected_ids = originals
                .iter()
                .map(|record| HistoryEntryId::Live(record.summary.id))
                .collect();
            state
                .command(Command::QueryHttpHistory {
                    query: requested.clone(),
                })
                .await
                .unwrap();
            complete_next(&mut state).await;
            let signal = tokio::time::timeout(
                Duration::from_secs(3),
                state.queries.http.next_signal(&state.events, false),
            )
            .await
            .expect("completed HTTP history query must obtain its delivery slot");
            assert!(matches!(signal, Signal::Ready(_)));
            state.http_query_signal(signal);
            let Event::HttpHistoryPage {
                query: actual,
                result: Ok(result),
            } = receiver.try_recv().unwrap()
            else {
                panic!("the query must deliver its original-request result through the event queue")
            };
            assert_eq!(actual, requested);
            assert_eq!(result.total, 1);
            assert_eq!(result.rows, [HistoryRow::Live(latest[0].summary.clone())]);
            assert_eq!(
                result.selected_matches,
                [
                    (requested.selected_ids[0], true),
                    (requested.selected_ids[1], false)
                ]
            );
            assert_eq!(result.selected_ranks, [(requested.selected_ids[0], 0)]);
            assert!(result.selected_rows.is_empty());
            assert!(!state.queries.http.has_worker());
            assert!(state.queries.http.result.is_none());
            assert!(receiver.try_recv().is_err());
            for record in &latest {
                assert_eq!(state.read_flow(record.summary.id).unwrap(), *record);
            }
            assert_eq!(state.store.workspace().unwrap(), workspace_before);

            if phase == 0 {
                assert_eq!(state.project, original_project);
                assert_eq!(state.storage.recovery, recovery_before);
                assert_eq!(state.store.test_actual_path(), source_path);
                for (original, changed) in originals.iter().zip(&edited) {
                    assert_eq!(
                        state
                            .store
                            .read_optional(original.summary.id)
                            .unwrap()
                            .as_ref(),
                        persisted.then_some(changed)
                    );
                }
                state
                    .command(Command::SaveProject {
                        project: state.project,
                        path: Some(saved_path.clone()),
                        workspace: None,
                        websocket_notes: Vec::new(),
                        ext_http_notes: Vec::new(),
                        ext_http_highlights: Vec::new(),
                    })
                    .await
                    .unwrap();
                let mut saved = false;
                while let Ok(event) = receiver.try_recv() {
                    if let Event::ProjectSaved { project, path, .. } = event {
                        assert_eq!(project, original_project);
                        assert_eq!(path, saved_path);
                        saved = true;
                    }
                }
                assert!(saved && state.storage.recovery.is_empty());
                state
                    .command(Command::OpenProject {
                        path: saved_path.clone(),
                    })
                    .await
                    .unwrap();
                let mut opened = false;
                while let Ok(event) = receiver.try_recv() {
                    if let Event::ProjectOpened { project, path, .. } = event {
                        assert_eq!(project.id, original_project.id);
                        assert_eq!(project.generation, original_project.generation + 1);
                        assert_eq!(path, Some(saved_path.clone()));
                        opened = true;
                    }
                }
                assert!(opened);
                assert_eq!(state.store.test_actual_path(), saved_path);
                for (original, record) in originals.iter().zip(&latest) {
                    assert_eq!(state.store.read(record.summary.id).unwrap(), *record);
                    assert_eq!(
                        state
                            .store
                            .original(record.summary.id)
                            .unwrap()
                            .request
                            .as_ref(),
                        Some(&original.request)
                    );
                }
            }
        }
        state.store.close().unwrap();
    }
}

#[test]
fn http_query_parameterized_recovery_rejects_inconsistent_original_identity_only_when_needed() {
    let (events, _receiver) = mpsc::channel(8);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let mut first = crate::service::tests::completed_flow();
    first.summary.source = TrafficSource::Proxy;
    first.request.url = "http://127.0.0.1:9001/fixture/original".into();
    first.summary.url = first.request.url.clone();
    first.request.body = b"a=1".to_vec();
    state.storage.recovery.push_back(first.clone().into());
    let recovery = state.http_recovery_snapshot().unwrap();
    let mut requested = query(state.project, 1);
    let path = state.store.test_actual_path();
    let cancel = CancellationToken::new();
    for different_method in [false, true] {
        let mut pending = recovery.clone();
        let request = pending[0].first_request.as_mut().unwrap();
        if different_method {
            request.method = "PUT".into();
        } else {
            request.url = "http://127.0.0.1:9001/fixture/foreign".into();
        }
        requested.filter.parameterized_only = false;
        assert_eq!(
            ProjectStore::http_history_snapshot(&path, &requested, &pending, &cancel)
                .unwrap()
                .total,
            1
        );
        requested.filter.parameterized_only = true;
        assert!(
            ProjectStore::http_history_snapshot(&path, &requested, &pending, &cancel)
                .unwrap_err()
                .to_string()
                .contains("原始请求与摘要不一致")
        );
        pending[0].first_request_url = "http://127.0.0.1:9001/fixture/query?original=1".into();
        assert_eq!(
            ProjectStore::http_history_snapshot(&path, &requested, &pending, &cancel)
                .unwrap()
                .total,
            1,
            "original URL parameters require no RAM body fallback"
        );
    }
    state.store.put(&first).unwrap();
    let mut edited = first.clone();
    edited.summary.revision += 1;
    edited.request.body = vec![0, 255, b'x'];
    state.store.put(&edited).unwrap();
    let mut pending = recovery;
    pending[0].summary.revision = edited.summary.revision + 1;
    pending[0].first_request.as_mut().unwrap().url = "http://127.0.0.1:9001/fixture/foreign".into();
    assert_eq!(
        ProjectStore::http_history_snapshot(&path, &requested, &pending, &cancel)
            .unwrap()
            .total,
        1,
        "the persisted original request takes precedence over inconsistent RAM identity"
    );
    state.store.close().unwrap();
}

#[test]
fn http_recovery_snapshot_preserves_stream_original_status_and_waits_for_first_response() {
    let (events, _receiver) = mpsc::channel(8);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let mut request = crate::service::tests::completed_flow();
    request.summary.source = TrafficSource::Proxy;
    request.summary.status = None;
    request.summary.response_bytes = 0;
    request.response = None;
    state.storage.recovery.push_back(request.clone().into());
    let mut received = crate::service::tests::completed_flow();
    received.summary.id = request.summary.id;
    received.summary.source = TrafficSource::Proxy;
    received.summary.revision = request.summary.revision + 1;
    received.summary.status = Some(404);
    received.response.as_mut().unwrap().status = 404;
    received.request.method = "PUT".into();
    received.request.url = "http://127.0.0.1:9001/changed".into();
    received.summary.method = received.request.method.clone();
    received.summary.url = received.request.url.clone();
    state.storage.recovery.push_back(CapturedFlow {
        record: received.clone(),
        response_stream: Some(StreamedResponse {
            original_status: 201,
            original_version: HttpVersion::Http11,
            original_headers: vec![],
            body_complete: false,
        }),
        listener_port: None,
        capture_metrics: mio_proxy::CapturedFlowMetrics::default(),
    });
    let rows = state.http_recovery_snapshot().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].summary, received.summary);
    assert_eq!(rows[0].first_request_method, request.request.method);
    assert_eq!(rows[0].first_request_url, request.request.url);
    assert_eq!(rows[0].first_response_status, Some(201));
    assert_eq!(rows[0].first_request.as_ref(), Some(&request.request));
    assert_eq!(
        rows[0]
            .first_response
            .as_ref()
            .map(|response| response.status),
        Some(201)
    );
    assert_eq!(rows[0].request.as_ref(), Some(&received.request));
    assert_eq!(rows[0].response.as_ref(), received.response.as_ref());
    state
        .storage.recovery
        .back_mut()
        .unwrap()
        .response_stream
        .as_mut()
        .unwrap()
        .original_status = 0;
    assert!(state.http_recovery_snapshot().is_err());
    assert_eq!(state.storage.recovery.back().unwrap().record, received);
}

#[test]
fn oversized_http_recovery_is_rejected_instead_of_truncating_unsaved_rows() {
    let (events, _receiver) = mpsc::channel(8);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let mut template = crate::service::tests::completed_flow();
    template.summary.source = TrafficSource::Proxy;
    for _ in 0..512 {
        template.summary.id = Uuid::new_v4();
        state.storage.recovery.push_back(template.clone().into());
    }
    assert_eq!(state.http_recovery_snapshot().unwrap().len(), 512);
    template.summary.id = Uuid::new_v4();
    state.storage.recovery.push_back(template.into());
    assert!(state.http_recovery_snapshot().is_err());
    assert_eq!(state.storage.recovery.len(), 513);
}
