use super::*;

#[tokio::test]
async fn actual_origin_observes_host_setting_binary_payload_and_duplicate_headers() {
    tokio::time::timeout(DEADLINE, async {
        let (runtime, mut client) = Runtime::start().unwrap();
        let project = opened(&mut client).await;
        let mut workspace = Workspace::default();
        for update_host_header in [true, false] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let origin = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let received = receive_request(&mut stream).await;
                echo_response(&mut stream, &received).await;
                (listener, received)
            });
            let payload = vec![0, 0xff, b'\r', b'\n', b'Z'];
            let mut spec = spec(format!("http://{address}"), vec![payload.clone()]);
            spec.update_host_header = update_host_header;
            let run_id = Uuid::new_v4();
            let tab_id = bind(&mut client, project, &mut workspace, &spec).await;
            start(
                &mut client,
                project,
                run_id,
                tab_id,
                &spec,
                CancellationToken::new(),
            )
            .await
            .unwrap();
            let page = terminal(&mut client, project, run_id).await;
            assert_eq!(page.run.state, IntruderRunState::Completed);
            assert_eq!(
                (page.run.total, page.run.dispatched, page.run.completed),
                (1, 1, 1)
            );
            assert_eq!(page.rows.len(), 1);
            let IntruderResultOutcome::Flow(flow) = &page.rows[0].outcome else {
                panic!("valid binary payload must produce a real flow")
            };
            assert_eq!(flow.state, FlowState::Complete);
            let (listener, received) = origin.await.unwrap();
            assert!(received.starts_with(b"POST /intruder HTTP/1.1\r\n"));
            assert_eq!(body(&received), payload);
            assert_eq!(header_values(&received, "X-Repeat"), ["one", "two"]);
            assert_eq!(header_values(&received, "Content-Length"), ["5"]);
            let expected_host = if update_host_header {
                address.to_string()
            } else {
                "original.invalid".into()
            };
            assert_eq!(header_values(&received, "Host"), [expected_host.as_str()]);
            no_extra_connection(listener);
        }
        shutdown(runtime).await;
    })
    .await
    .expect("real Host and binary Intruder workflow is bounded");
}

#[tokio::test]
async fn silent_post_stop_and_out_of_band_cancel_close_origin_without_retrying() {
    tokio::time::timeout(DEADLINE, async {
        for out_of_band in [true, false] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let (seen, received) = oneshot::channel();
            let origin = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = receive_request(&mut stream).await;
                seen.send(request).unwrap();
                assert_eq!(
                    stream.read(&mut [0; 1]).await.unwrap(),
                    0,
                    "Stop must close the actual origin socket"
                );
                listener
            });
            let (runtime, mut client) = Runtime::start().unwrap();
            let project = opened(&mut client).await;
            let spec = spec(
                format!("http://{address}"),
                vec![vec![0, 0xff], b"second".to_vec(), b"third".to_vec()],
            );
            let cancel = CancellationToken::new();
            let run_id = Uuid::new_v4();
            let tab_id = bind(&mut client, project, &mut Workspace::default(), &spec).await;
            start(&mut client, project, run_id, tab_id, &spec, cancel.clone())
                .await
                .unwrap();
            assert_eq!(body(&received.await.unwrap()), [0, 0xff]);
            if out_of_band {
                cancel.cancel();
            } else {
                client
                    .commands
                    .send(Command::StopIntruder { project, run_id })
                    .await
                    .unwrap();
            }
            let page = terminal(&mut client, project, run_id).await;
            assert_eq!(page.run.state, IntruderRunState::Stopped);
            assert_eq!(
                (page.run.total, page.run.dispatched, page.run.completed),
                (3, 1, 1)
            );
            assert_eq!(page.rows.len(), 1);
            let IntruderResultOutcome::Flow(flow) = &page.rows[0].outcome else {
                panic!("the received POST must retain its terminal flow")
            };
            assert_eq!(flow.state, FlowState::Cancelled);
            let listener = origin.await.unwrap();
            shutdown(runtime).await;
            no_extra_connection(listener);
        }
    })
    .await
    .expect("Stop, cancellation and origin cleanup have one finite upper bound");
}

#[tokio::test]
async fn silent_post_timeout_closes_origin_and_reopens_without_retrying() {
    tokio::time::timeout(DEADLINE, async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen, received) = oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            seen.send(receive_request(&mut stream).await).unwrap();
            assert_eq!(
                stream.read(&mut [0; 1]).await.unwrap(),
                0,
                "the request timeout must close the actual silent origin socket"
            );
            listener
        });
        let (runtime, mut client) = Runtime::start().unwrap();
        let project = opened(&mut client).await;
        let payload = vec![0, 0xff, b'T'];
        let mut spec = spec(format!("http://{address}"), vec![payload.clone()]);
        spec.request_timeout_ms = 300;
        let run_id = Uuid::new_v4();
        let tab_id = bind(&mut client, project, &mut Workspace::default(), &spec).await;
        start(
            &mut client,
            project,
            run_id,
            tab_id,
            &spec,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let request = received.await.unwrap();
        assert!(request.starts_with(b"POST /intruder HTTP/1.1\r\n"));
        assert_eq!(body(&request), payload);
        let page = terminal(&mut client, project, run_id).await;
        let listener = origin.await.unwrap();
        assert_eq!(page.run.state, IntruderRunState::Completed);
        assert_eq!(
            (page.run.total, page.run.dispatched, page.run.completed),
            (1, 1, 1)
        );
        assert_eq!(page.rows.len(), 1);
        let IntruderResultOutcome::Flow(flow) = &page.rows[0].outcome else {
            panic!("a timed-out POST must retain its actual terminal flow")
        };
        assert_eq!(
            flow.state,
            FlowState::Failed(mio_proxy::ProxyError::Timeout.to_string())
        );
        assert!(flow.elapsed_ms >= spec.request_timeout_ms);

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("intruder-timeout.mio");
        client
            .commands
            .send(Command::SaveProject {
                websocket_notes: Vec::new(),
                ext_http_notes: Vec::new(),
                ext_http_highlights: Vec::new(),
                project,
                path: Some(path.clone()),
                workspace: None,
            })
            .await
            .unwrap();
        event(&mut client, |event| {
            matches!(event, Event::ProjectSaved { project: key, path: saved, .. }
                if *key == project && *saved == path)
        })
        .await;
        client
            .commands
            .send(Command::NewProject { path: None })
            .await
            .unwrap();
        assert_ne!(opened(&mut client).await.id, project.id);
        client
            .commands
            .send(Command::OpenProject { path })
            .await
            .unwrap();
        let reopened = opened(&mut client).await;
        assert_eq!(reopened.id, project.id);
        assert!(reopened.generation > project.generation);
        assert_eq!(
            results(&mut client, reopened, run_id, 90, None, 1)
                .await
                .unwrap(),
            page
        );
        client
            .commands
            .send(Command::ReadFlow {
                project: reopened,
                selection_revision: 91,
                flow_id: flow.id,
            })
            .await
            .unwrap();
        match event(&mut client, |event| {
            matches!(event, Event::FlowDetail { project: key, selection_revision: 91, .. }
                if *key == reopened)
        })
        .await
        {
            Event::FlowDetail { record, .. } => {
                assert_eq!(record.summary, *flow);
                assert_eq!(record.request.body, payload);
                assert!(record.response.is_none());
            }
            _ => unreachable!(),
        }
        shutdown(runtime).await;
        no_extra_connection(listener);
    })
    .await
    .expect("request timeout, EOF, save, reopen and cleanup have one finite upper bound");
}

#[tokio::test]
async fn stopping_one_run_does_not_cancel_another_runs_in_flight_post() {
    tokio::time::timeout(DEADLINE, async {
        let stopped_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stopped_address = stopped_listener.local_addr().unwrap();
        let (stopped_seen, stopped_received) = oneshot::channel();
        let stopped_origin = tokio::spawn(async move {
            let (mut stream, _) = stopped_listener.accept().await.unwrap();
            stopped_seen
                .send(receive_request(&mut stream).await)
                .unwrap();
            assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0);
            stopped_listener
        });
        let kept_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let kept_address = kept_listener.local_addr().unwrap();
        let (kept_seen, kept_received) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let kept_origin = tokio::spawn(async move {
            let (mut stream, _) = kept_listener.accept().await.unwrap();
            let request = receive_request(&mut stream).await;
            kept_seen.send(request.clone()).unwrap();
            released.await.unwrap();
            echo_response(&mut stream, &request).await;
            kept_listener
        });
        let (runtime, mut client) = Runtime::start().unwrap();
        let project = opened(&mut client).await;
        let stopped_id = Uuid::new_v4();
        let kept_id = Uuid::new_v4();
        let mut workspace = Workspace::default();
        for (run_id, input) in [
            (
                stopped_id,
                spec(
                    format!("http://{stopped_address}"),
                    vec![b"stop".to_vec(), b"must-not-send".to_vec()],
                ),
            ),
            (
                kept_id,
                spec(format!("http://{kept_address}"), vec![b"keep".to_vec()]),
            ),
        ] {
            let tab_id = bind(&mut client, project, &mut workspace, &input).await;
            start(
                &mut client,
                project,
                run_id,
                tab_id,
                &input,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        }
        // Both non-idempotent requests are on the wire before either run is stopped.
        assert_eq!(body(&stopped_received.await.unwrap()), b"stop");
        assert_eq!(body(&kept_received.await.unwrap()), b"keep");
        client
            .commands
            .send(Command::StopIntruder {
                project,
                run_id: stopped_id,
            })
            .await
            .unwrap();
        let stopped = terminal(&mut client, project, stopped_id).await;
        assert_eq!(stopped.run.state, IntruderRunState::Stopped);
        assert_eq!(
            (
                stopped.run.total,
                stopped.run.dispatched,
                stopped.run.completed
            ),
            (2, 1, 1)
        );
        assert!(matches!(
            stopped.rows.as_slice(),
            [row] if matches!(&row.outcome,
                IntruderResultOutcome::Flow(flow) if flow.state == FlowState::Cancelled)
        ));
        let stopped_listener = stopped_origin.await.unwrap();

        // The second origin cannot answer until the first run has fully stopped.
        let pending = results(&mut client, project, kept_id, 70, None, 1)
            .await
            .unwrap();
        assert!(pending.persisted);
        assert_eq!(pending.run.state, IntruderRunState::Running);
        assert_eq!(
            (
                pending.run.total,
                pending.run.dispatched,
                pending.run.completed
            ),
            (1, 1, 0)
        );
        assert!(matches!(
            pending.rows.as_slice(),
            [row] if matches!(&row.outcome,
                IntruderResultOutcome::Flow(flow) if flow.state == FlowState::Pending)
        ));
        release.send(()).unwrap();
        let completed = terminal(&mut client, project, kept_id).await;
        assert_eq!(completed.run.state, IntruderRunState::Completed);
        assert_eq!(
            (
                completed.run.total,
                completed.run.dispatched,
                completed.run.completed
            ),
            (1, 1, 1)
        );
        assert!(matches!(
            completed.rows.as_slice(),
            [row] if matches!(&row.outcome,
                IntruderResultOutcome::Flow(flow) if flow.state == FlowState::Complete)
        ));
        let kept_listener = kept_origin.await.unwrap();
        shutdown(runtime).await;
        no_extra_connection(stopped_listener);
        no_extra_connection(kept_listener);
    })
    .await
    .expect("stopping one run must preserve the other run and bound socket cleanup");
}
