use super::*;

#[tokio::test]
async fn interception_cancellation_prevents_callbacks_admission_and_preserves_terminal_bytes() {
    use mio_plugin_api::{Header, HttpVersion};
    use std::sync::atomic::{AtomicUsize, Ordering};

    tokio::time::timeout(Duration::from_secs(5), async {
        let ca = MitmAuthority::generate().unwrap();
        let mut failures = Vec::new();
        for (case, initially_cancelled, should_hold, filter_error) in [
            ("entry_cancelled", true, false, false),
            ("filter_bypass_cancelled", false, false, false),
            ("filter_hold_cancelled", false, true, false),
            ("filter_error_cancelled", false, false, true),
        ] {
            for saturated in [false, true] {
                let project = ProjectKey {
                    id: Uuid::new_v4(),
                    generation: 1,
                };
                let mut original = new_record(HttpRequest {
                    method: "POST".into(),
                    url: "http://127.0.0.1:1/cancelled".into(),
                    version: HttpVersion::Http11,
                    headers: vec![
                        Header {
                            name: "x-original".into(),
                            value: vec![0xff, b'A'],
                        },
                        Header {
                            name: "x-original".into(),
                            value: b"second".to_vec(),
                        },
                    ],
                    body: vec![0, 0xfe, b'C'],
                    trailers: Vec::new(),
                });
                original.summary.request_bytes = original.request.body.len() as u64;
                let (events, mut receiver) = mpsc::channel(1);
                if saturated {
                    events
                        .try_send(ProxyEvent::Captured {
                            project,
                            record: original.clone(),
                            response_stream: None,
                            listener_port: None,
                            capture_metrics: CapturedFlowMetrics::default(),
                        })
                        .unwrap();
                }
                let cancel = CancellationToken::new();
                let shutdown = Shutdown::new(cancel.clone().cancelled_owned());
                let tasks = TaskTracker::new();
                let filter_calls = Arc::new(AtomicUsize::new(0));
                let counted_filter = filter_calls.clone();
                let filter_cancel = cancel.clone();
                let filter: InterceptFilter = Arc::new(
                    move |_, _, _, _, _, ctx, callback_cancel, (deadline, max_body)| {
                        assert_eq!(ctx.listener_port, 1);
                        assert_eq!(ctx.target_ip, Some("127.0.0.1"));
                        assert!(!ctx.was_annotated);
                        assert_eq!(max_body, 128);
                        assert!(!callback_cancel.is_cancelled());
                        let now = Instant::now();
                        assert!(deadline > now);
                        assert!(deadline <= now + Duration::from_secs(2));
                        counted_filter.fetch_add(1, Ordering::SeqCst);
                        callback_cancel.cancel();
                        assert!(filter_cancel.is_cancelled());
                        if filter_error {
                            Err(ProxyError::InvalidMessage("cancelled filter error".into()))
                        } else if should_hold {
                            Ok(InterceptFilterOutcome::Intercept)
                        } else {
                            Ok(InterceptFilterOutcome::Bypass)
                        }
                    },
                );
                let transform_calls = Arc::new(AtomicUsize::new(0));
                let counted_transform = transform_calls.clone();
                let transformer: FlowTransform = Arc::new(move |_, _, _, cancel, (deadline, max)| {
                    assert!(!cancel.is_cancelled());
                    assert!(deadline > Instant::now());
                    assert_eq!(max, 128);
                    counted_transform.fetch_add(1, Ordering::SeqCst);
                    Ok(false)
                });
                let mut config = ProxyConfig::new(ca.clone());
                config.limits.max_connections = 1;
                config.limits.max_body_bytes = 128;
                config.limits.request_timeout = Duration::from_secs(2);
                config.limits.intercept_timeout = Duration::from_millis(100);
                config.hooks.intercept_requests = true;
                config.hooks.intercept_filter = Some(filter);
                config.hooks.flow_transform = Some(transformer);
                let (static_config, dynamic_init, _) = config.into_parts();
                let (_dynamic_tx, dynamic) = watch::channel(dynamic_init);
                let requests = Arc::new(Semaphore::new(1));
                let terminal_records = TerminalRecords::default();
                let state = Arc::new(State {
                    project,
                    listener_port: 1,
                    config: static_config,
                    events,
                    cancel: cancel.clone(),
                    dynamic,
                    tasks: tasks.clone(),
                    requests: requests.clone(),
                    exec: Executor::graceful(shutdown.guard()),
                    terminal_records: terminal_records.clone(),
                    injectors: websocket::WebSocketInjectorRegistry::new(),
                });
                let mut active = ActiveFlow {
                    state: state.clone(),
                    record: Some(original.clone()),
                    response_stream: None,
                    capture_metrics: CapturedFlowMetrics::default(),
                    request_was_intercepted: false,
                    request_was_modified: false,
                    started: Instant::now(),
                    permit: Arc::new(requests.clone().acquire_owned().await.unwrap()),
                };
                if initially_cancelled {
                    cancel.cancel();
                }
                let result = state
                    .intercept(
                        active.record.as_mut().unwrap(),
                        false,
                        None,
                        &active.capture_metrics,
                        false,
                        false,
                    )
                    .await;
                let cancelled_result = matches!(&result, Err(ProxyError::Cancelled));
                let untouched = active.record.as_ref() == Some(&original);
                let queue_unchanged = receiver.len() == usize::from(saturated);
                let expected_calls = usize::from(!initially_cancelled);
                let callbacks_valid = filter_calls.load(Ordering::SeqCst) == expected_calls
                    && transform_calls.load(Ordering::SeqCst) == expected_calls;
                if let Err(error) = &result {
                    active.finish(Err(error)).await;
                } else {
                    active.finish(Ok(())).await;
                }
                drop(active);
                tasks.close();
                tasks.wait().await;
                drop(state);
                shutdown.shutdown().await;
                let mut queued = Vec::new();
                while let Ok(event) = receiver.try_recv() {
                    queued.push(event);
                }
                let retained = std::mem::take(&mut *terminal_records.lock().unwrap());
                let queue_valid = matches!(
                    queued.as_slice(),
                    [ProxyEvent::Captured {
                        project: owner,
                        record,
                        response_stream: None,
                        ..
                    }] if *owner == project && (!saturated || record == &original)
                );
                let retained_valid = if saturated {
                    retained.len() == 1 && retained[0].capture.response_stream.is_none()
                } else {
                    retained.is_empty()
                };
                let terminal = if saturated {
                    retained.first().map(|terminal| &terminal.capture.record)
                } else {
                    queued.first().and_then(|event| match event {
                        ProxyEvent::Captured { record, .. } => Some(record),
                        _ => None,
                    })
                };
                let terminal_valid = terminal.is_some_and(|record| {
                    record.summary.id == original.summary.id
                        && record.summary.revision == original.summary.revision + 1
                        && record.summary.state == FlowState::Cancelled
                        && record.summary.request_bytes == original.summary.request_bytes
                        && record.request == original.request
                        && record.response.is_none()
                });
                drop(queued);
                drop(retained);
                let reclaimed = requests.available_permits() == 1;
                if !(cancelled_result
                    && untouched
                    && queue_unchanged
                    && callbacks_valid
                    && queue_valid
                    && retained_valid
                    && terminal_valid
                    && reclaimed)
                {
                    failures.push(format!(
                        "{case}, saturated={saturated}: result={result:?}, untouched={untouched}, queue_unchanged={queue_unchanged}, callbacks_valid={callbacks_valid}, queue_valid={queue_valid}, retained_valid={retained_valid}, terminal_valid={terminal_valid}, reclaimed={reclaimed}"
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    })
    .await
    .expect("cancelled interception and both terminal delivery paths finish and join within budget");
}
