use super::*;

#[tokio::test]
async fn interception_cpu_deadline_rejects_late_results_and_overflow_without_admission() {
    use mio_plugin_api::HttpVersion;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    tokio::time::timeout(Duration::from_secs(3), async {
        let ca = MitmAuthority::generate().unwrap();
        let mut failures = Vec::new();
        for (
            case,
            enabled,
            budget,
            hold,
            filter_error,
            expected_calls,
            transform_mode,
            expected_transform_calls,
        ) in [
            ("late_bypass", true, Duration::from_nanos(1), false, false, 1, None, 0),
            ("late_hold", true, Duration::from_nanos(1), true, false, 1, None, 0),
            ("late_error", true, Duration::from_nanos(1), false, true, 1, None, 0),
            ("overflow", true, Duration::MAX, false, false, 0, None, 0),
            ("disabled", false, Duration::MAX, false, false, 0, None, 0),
            (
                "transform_late_ok",
                true,
                Duration::from_nanos(1),
                true,
                false,
                0,
                Some(false),
                1,
            ),
            (
                "transform_late_err",
                false,
                Duration::from_nanos(1),
                false,
                false,
                0,
                Some(true),
                1,
            ),
            (
                "transform_overflow",
                false,
                Duration::MAX,
                false,
                false,
                0,
                Some(false),
                0,
            ),
        ] {
            let project = ProjectKey {
                id: Uuid::new_v4(),
                generation: 1,
            };
            let mut record = new_record(HttpRequest {
                method: "POST".into(),
                url: "http://127.0.0.1:1/deadline".into(),
                version: HttpVersion::Http11,
                headers: Vec::new(),
                body: vec![0, 0xfe, b'C'],
                trailers: Vec::new(),
            });
            let original = record.clone();
            let (events, receiver) = mpsc::channel(1);
            let cancel = CancellationToken::new();
            let shutdown = Shutdown::new(cancel.clone().cancelled_owned());
            let tasks = TaskTracker::new();
            let (_mode, intercept) = watch::channel(InterceptMode {
                requests: enabled,
                responses: false,
            });
            let calls = Arc::new(AtomicUsize::new(0));
            let counted_calls = calls.clone();
            let expired = Arc::new(AtomicBool::new(false));
            let observed_expiry = expired.clone();
            let filter: InterceptFilter = Arc::new(
                move |record, _, _, _, _, ctx, callback_cancel, (deadline, max_body)| {
                    assert_eq!(ctx.listener_port, 49153);
                    assert_eq!(ctx.target_ip, Some("127.0.0.1"));
                    assert!(!ctx.was_annotated);
                    assert_eq!(max_body, 128);
                    assert!(!callback_cancel.is_cancelled());
                    counted_calls.fetch_add(1, Ordering::SeqCst);
                    let checksum = std::hint::black_box(&record.request.body)
                        .iter()
                        .fold(0_u8, |total, byte| total.wrapping_add(*byte));
                    assert_eq!(std::hint::black_box(checksum), 65);
                    observed_expiry.store(Instant::now() >= deadline, Ordering::SeqCst);
                    if filter_error {
                        Err(ProxyError::InvalidMessage("late condition error".into()))
                    } else {
                        Ok(hold)
                    }
                },
            );
            let transform_calls = Arc::new(AtomicUsize::new(0));
            let counted_transform = transform_calls.clone();
            let transform_expired = Arc::new(AtomicBool::new(false));
            let observed_transform_expiry = transform_expired.clone();
            let transformer = transform_mode.map(|transform_error| -> FlowTransform {
                Arc::new(
                    move |record, response_stage, response_head_only, callback_cancel, (deadline, max_body)| {
                        assert!(!response_stage && !response_head_only);
                        assert_eq!(max_body, 128);
                        assert!(!callback_cancel.is_cancelled());
                        counted_transform.fetch_add(1, Ordering::SeqCst);
                        record.request.body = b"expired mutation".to_vec();
                        let checksum = std::hint::black_box(&record.request.body)
                            .iter()
                            .fold(0_u8, |total, byte| total.wrapping_add(*byte));
                        assert_ne!(std::hint::black_box(checksum), 0);
                        observed_transform_expiry.store(Instant::now() >= deadline, Ordering::SeqCst);
                        if transform_error {
                            Err(ProxyError::InvalidMessage("late transform error".into()))
                        } else {
                            Ok(true)
                        }
                    },
                )
            });
            let (_processing_tx, processing) = watch::channel((transformer, Some(filter), None));
            let (_ws_tx, websocket_transform) = watch::channel(None);
            let (_ssl_tx, ssl_pass_through) = watch::channel(None);
            let (_plugin_hooks, plugin_hooks) = watch::channel((None, None));
            let (_drop_oos_tx, drop_out_of_scope) = watch::channel(None);
            let mut config = ProxyConfig::new(ca.clone());
            config.max_body_bytes = 128;
            config.request_timeout = budget;
            config.intercept_timeout = Duration::from_millis(10);
            let state = Arc::new(State {
                project,
                listener_port: 49153,
                config,
                events,
                cancel: cancel.clone(),
                intercept,
                processing,
                websocket_transform,
                ssl_pass_through,
                plugin_hooks,
                drop_out_of_scope,
                tasks: tasks.clone(),
                requests: Arc::new(Semaphore::new(1)),
                exec: Executor::graceful(shutdown.guard()),
                terminal_records: TerminalRecords::default(),
                injectors: websocket::WebSocketInjectorRegistry::new(),
            });
            let result = state
                .intercept(&mut record, false, None, &CapturedFlowMetrics::default(), false, false)
                .await;
            let result_valid = if enabled || transform_mode.is_some() {
                matches!(&result, Err(ProxyError::Timeout))
            } else {
                matches!(&result, Ok((false, false)))
            };
            let untouched = record == original;
            let queue_unchanged = receiver.is_empty();
            let calls_valid = calls.load(Ordering::SeqCst) == expected_calls;
            let expiry_valid = expired.load(Ordering::SeqCst) == (expected_calls != 0);
            let transform_calls_valid =
                transform_calls.load(Ordering::SeqCst) == expected_transform_calls;
            let transform_expiry_valid =
                transform_expired.load(Ordering::SeqCst) == (expected_transform_calls != 0);
            cancel.cancel();
            tasks.close();
            tasks.wait().await;
            drop(state);
            shutdown.shutdown().await;
            drop(receiver);
            if !(result_valid
                && untouched
                && queue_unchanged
                && calls_valid
                && expiry_valid
                && transform_calls_valid
                && transform_expiry_valid)
            {
                failures.push(format!(
                    "{case}: result={result:?}, untouched={untouched}, queue_unchanged={queue_unchanged}, calls_valid={calls_valid}, expiry_valid={expiry_valid}, transform_calls_valid={transform_calls_valid}, transform_expiry_valid={transform_expiry_valid}"
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    })
    .await
    .expect("real callback deadlines, overflow and disabled mode finish without admission or leaked tasks");
}

#[tokio::test]
async fn response_transform_and_filter_share_the_snapshot_before_capture_backpressure() {
    use mio_plugin_api::HttpVersion;
    use std::sync::atomic::{AtomicUsize, Ordering};

    tokio::time::timeout(Duration::from_secs(3), async {
        let project = ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        };
        let mut record = new_record(HttpRequest {
            method: "POST".into(),
            url: "http://127.0.0.1:1/processing-snapshot".into(),
            version: HttpVersion::Http11,
            headers: Vec::new(),
            body: vec![0, 0xff],
            trailers: Vec::new(),
        });
        record.response = Some(HttpResponse {
            status: 200,
            version: HttpVersion::Http11,
            reason_phrase: None,
            headers: Vec::new(),
            body: b"before".to_vec(),
            trailers: Vec::new(),
        });
        let original = record.clone();
        let (events, mut receiver) = mpsc::channel(1);
        events
            .try_send(ProxyEvent::Captured {
                project,
                record: original.clone(),
                response_stream: None,
                listener_port: None,
                capture_metrics: CapturedFlowMetrics::default(),
            })
            .unwrap();
        let cancel = CancellationToken::new();
        let shutdown = Shutdown::new(cancel.clone().cancelled_owned());
        let tasks = TaskTracker::new();
        let (_mode, intercept) = watch::channel(InterceptMode {
            requests: false,
            responses: true,
        });
        let old_calls = Arc::new(AtomicUsize::new(0));
        let counted_old_calls = old_calls.clone();
        let filter: InterceptFilter = Arc::new(move |_, _, _, _, _, ctx, _, _| {
            assert_eq!(ctx.listener_port, 49153);
            counted_old_calls.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        });
        let (entered, mut observed_transform) = mpsc::channel(1);
        let transformer: FlowTransform = Arc::new(move |record, response_stage, _, _, _| {
            assert!(response_stage);
            record.response.as_mut().unwrap().body = vec![0, 0xfe, b'X'];
            record.summary.response_bytes = 3;
            entered.try_send(()).unwrap();
            Ok(true)
        });
        let (processing_tx, processing) = watch::channel((Some(transformer), Some(filter), None));
        let (_ws_tx, websocket_transform) = watch::channel(None);
        let (_ssl_tx, ssl_pass_through) = watch::channel(None);
        let (_plugin_hooks, plugin_hooks) = watch::channel((None, None));
        let (_drop_oos_tx, drop_out_of_scope) = watch::channel(None);
        let mut config = ProxyConfig::new(MitmAuthority::generate().unwrap());
        config.max_body_bytes = 128;
        config.request_timeout = Duration::from_secs(2);
        config.intercept_timeout = Duration::from_secs(2);
        let state = Arc::new(State {
            project,
            listener_port: 49153,
            config,
            events,
            cancel: cancel.clone(),
            intercept,
            processing,
            websocket_transform,
            ssl_pass_through,
            plugin_hooks,
            drop_out_of_scope,
            tasks: tasks.clone(),
            requests: Arc::new(Semaphore::new(1)),
            exec: Executor::graceful(shutdown.guard()),
            terminal_records: TerminalRecords::default(),
            injectors: websocket::WebSocketInjectorRegistry::new(),
        });
        let running_state = state.clone();
        let running = tokio::spawn(async move {
            let result = running_state
                .intercept(&mut record, true, None, &CapturedFlowMetrics::default(), false, false)
                .await;
            (result, record)
        });
        observed_transform.recv().await.unwrap();
        assert!(!running.is_finished());
        assert_eq!(receiver.len(), 1);
        let new_calls = Arc::new(AtomicUsize::new(0));
        let counted_new_calls = new_calls.clone();
        processing_tx.send_modify(|processing| {
            processing.1 = Some(Arc::new(move |_, _, _, _, _, _, _, _| {
                counted_new_calls.fetch_add(1, Ordering::SeqCst);
                Ok(false)
            }));
        });
        let initial = receiver.recv().await.unwrap();
        assert!(matches!(initial, ProxyEvent::Captured { record, .. } if record == original));
        let received = receiver.recv().await.unwrap();
        assert!(matches!(received, ProxyEvent::Captured { record, .. }
            if record.response == original.response && record.summary.revision > original.summary.revision));
        let stage = receiver.recv().await.unwrap();
        let kept_snapshot = if let ProxyEvent::Intercepted { record, reply, .. } = stage {
            reply
                .send(InterceptDecision::ForwardResponse(record.response.unwrap()))
                .unwrap();
            true
        } else {
            false
        };
        if kept_snapshot {
            receiver.recv().await.unwrap();
        }
        let (result, latest) = running.await.unwrap();
        cancel.cancel();
        tasks.close();
        tasks.wait().await;
        drop(state);
        shutdown.shutdown().await;
        assert!(
            kept_snapshot,
            "backpressure must not combine the old transform with the new predicate"
        );
        assert_eq!(result.unwrap(), (true, false));
        assert_eq!(old_calls.load(Ordering::SeqCst), 1);
        assert_eq!(new_calls.load(Ordering::SeqCst), 0);
        assert_eq!(latest.request, original.request);
        assert_eq!(latest.response.unwrap().body, [0, 0xfe, b'X']);
    })
    .await
    .expect("a bounded raw capture and filter update retain the admitted stage snapshot and join");
}
