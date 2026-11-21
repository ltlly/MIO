use super::*;

#[tokio::test]
async fn completed_proxy_is_reaped_once_without_overtaking_the_waiting_notification() {
    use std::future::Future;

    tokio::time::timeout(Duration::from_secs(8), async {
        let (events, mut received) = mpsc::channel(1);
        let mut state = State::new(events, CancellationToken::new()).unwrap();
        let mut proxy = mio_proxy::start(
            ProxyConfig::new(MitmAuthority::generate().unwrap()),
            state.project,
            state.proxy_sender.clone(),
        )
        .await
        .unwrap();
        proxy.cancellation_token().cancel();
        proxy.join().await.unwrap();
        assert!(proxy.is_joined());
        state.proxy = Some(proxy);
        let flow_id = Uuid::new_v4();
        state.queue_proxy_notification(Event::InterceptResolved {
            project: state.project,
            flow_id,
        });
        state.events.try_send(Event::Stopped).unwrap();
        {
            let mut flush = Box::pin(state.flush_proxy_notifications());
            std::future::poll_fn(|cx| {
                assert!(flush.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
        }
        assert!(state.proxy.is_none());
        assert_eq!(received.len(), 1);
        assert_eq!(state.proxy_notifications.len(), 3);
        {
            let mut waiting = Box::pin(proxy_completion(&mut state.proxy));
            std::future::poll_fn(|cx| {
                assert!(waiting.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
        }
        assert_eq!(state.proxy_notifications.len(), 3);
        assert!(matches!(received.try_recv().unwrap(), Event::Stopped));
        let (_, notifications) = tokio::join!(state.flush_proxy_notifications(), async {
            let mut notifications = Vec::new();
            for _ in 0..3 {
                notifications.push(received.recv().await.unwrap());
            }
            notifications
        });
        assert!(matches!(notifications[0], Event::InterceptResolved { flow_id: id, .. } if id == flow_id));
        assert!(matches!(notifications[1], Event::HistoryChanged { .. }));
        assert!(matches!(notifications[2], Event::ProxyState { address: None, cancel: None, .. }));
        assert!(state.proxy_notifications.is_empty());
    })
    .await
    .expect("completed-runner reclamation and FIFO delivery are bounded");
}

#[tokio::test]
async fn joined_proxy_failure_keeps_its_root_cause_when_terminal_storage_also_fails() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let (events, received) = mpsc::channel(1);
        let mut state = State::new(events, CancellationToken::new()).unwrap();
        let mut proxy = mio_proxy::start(
            ProxyConfig::new(MitmAuthority::generate().unwrap()),
            state.project,
            state.proxy_sender.clone(),
        )
        .await
        .unwrap();
        proxy.stop().await.unwrap();
        assert!(proxy.is_joined());
        state.proxy = Some(proxy);
        state.events.try_send(Event::Stopped).unwrap();
        state.store.reject_writes(true);
        let mut record = tests::completed_flow();
        record.summary.source = TrafficSource::Proxy;
        state.proxy_sender.try_send(ProxyEvent::Captured {
            project: state.project,
            record: record.clone(),
            response_stream: None,
            listener_port: None,
            capture_metrics: mio_proxy::CapturedFlowMetrics::default(),
        }).unwrap();
        let error = mio_proxy::ProxyError::Network("runner failure root cause".into());
        let message = error.to_string();
        state.proxy_finished(Err(error));
        assert!(state.proxy.is_none());
        assert_eq!(received.len(), 1);
        assert_eq!(state.read_flow(record.summary.id).unwrap(), record);
        assert_eq!(state.recovery.len(), 1);
        let notifications = &state.proxy_notifications;
        assert_eq!(notifications.len(), 4);
        assert!(matches!(notifications.front(), Some(Event::Error { operation: "capture", .. })));
        assert!(matches!(notifications[2], Event::ProxyState { address: None, cancel: None, .. }));
        assert!(matches!(notifications.back(), Some(Event::Error { operation: "stop_proxy", message: actual, .. }) if *actual == message));
    })
    .await
    .expect("joined failure and unsaved terminal retention are bounded");
}

#[tokio::test]
async fn stop_deadline_retains_real_proxy_until_event_permit_or_shutdown_reaps_it() {
    use std::{future::Future, task::Poll};

    const BODY: [u8; 3] = [0, 0xff, b'D'];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    for shutdown in [false, true] {
        let (events, mut received) = mpsc::channel(1);
        let mut state = State::new(events, CancellationToken::new()).unwrap();
        state.events.try_send(Event::Stopped).unwrap();
        let mut config = ProxyConfig::new(MitmAuthority::generate().unwrap());
        config.limits.stop_timeout = Duration::from_millis(2);
        let project = state.project;
        let sender = state.proxy_sender.clone();
        let (started, proxy) = oneshot::channel();
        let (pause, pause_requested) = oneshot::channel();
        let (paused, pause_confirmed) = oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let (finish, finished) = oneshot::channel();
        let runtime_thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            if started
                .send(runtime.block_on(mio_proxy::start(config, project, sender)))
                .is_err()
            {
                return;
            }
            let _ = runtime.block_on(pause_requested);
            let _ = paused.send(());
            // Outside block_on, neither the real runner nor its socket tasks can advance.
            let _ = released.recv();
            let _ = runtime.block_on(finished);
        });
        let outcome = tokio::time::timeout_at(deadline - Duration::from_secs(1), async {
            state.proxy = Some(proxy.await??);
            let proxy = state.proxy.as_ref().context("missing real Proxy handle")?;
            let original_handle = std::ptr::from_ref(proxy);
            let address = proxy.address();
            let cancel = proxy.cancellation_token();
            let origin = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
            let target = format!("http://{}/deadline", origin.local_addr()?);
            let mut downstream = TcpStream::connect(address).await?;
            downstream
                .write_all(
                    format!(
                        "POST {target} HTTP/1.1\r\nHost: localhost\r\nContent-Length: 3\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await?;
            downstream.write_all(&BODY).await?;
            let (mut upstream, _) = origin.accept().await?;
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                ensure!(head.len() < 8192, "unbounded origin request head");
                head.push(upstream.read_u8().await?);
            }
            ensure!(head.starts_with(b"POST /deadline HTTP/1.1\r\n"));
            let mut body = [0; BODY.len()];
            upstream.read_exact(&mut body).await?;
            ensure!(body == BODY);
            pause
                .send(())
                .map_err(|_| anyhow::anyhow!("Proxy runtime exited before pause"))?;
            pause_confirmed.await?;
            ensure!(!cancel.is_cancelled());

            let error = state
                .stop_proxy()
                .await
                .err()
                .context("a parked real runner must exceed the Stop deadline")?;
            ensure!(matches!(
                error.downcast_ref::<mio_proxy::ProxyError>(),
                Some(mio_proxy::ProxyError::StopDeadline)
            ));
            let proxy = state
                .proxy
                .as_ref()
                .context("deadline discarded the join")?;
            ensure!(std::ptr::eq(proxy, original_handle));
            ensure!(proxy.address() == address && !proxy.is_joined());
            ensure!(cancel.is_cancelled() && proxy.cancellation_token().is_cancelled());
            ensure!(
                std::net::TcpListener::bind(address)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::AddrInUse),
                "the parked listener is still owned after the deadline"
            );
            let pending = {
                let mut completion = std::pin::pin!(proxy_completion(&mut state.proxy));
                std::future::poll_fn(|cx| Poll::Ready(completion.as_mut().poll(cx).is_pending()))
                    .await
            };
            ensure!(pending, "StopDeadline must leave the same unfinished join");
            let (total, rows) = state.store.page(0, 2, "")?;
            ensure!(total == 1 && rows.len() == 1);
            let initial = state.store.read(rows[0].id)?;
            ensure!(initial.summary.state == FlowState::Pending);
            ensure!(initial.request.body == BODY && initial.request.url == target);
            ensure!(received.len() == 1);
            ensure!(state.proxy_notifications.len() == 2);
            ensure!(matches!(received.try_recv()?, Event::Stopped));
            let (_, notices) = tokio::join!(state.error("stop_proxy", &error), async {
                let mut notices = Vec::new();
                for _ in 0..3 {
                    notices.push(received.recv().await.context("UI event channel closed")?);
                }
                Ok::<_, anyhow::Error>(notices)
            });
            let notices = notices?;
            ensure!(
                matches!(notices[0], Event::HistoryChanged { project } if project == state.project)
            );
            ensure!(matches!(&notices[1], Event::ProxyState {
                project, address: Some(actual), cancel: Some(token), ..
            } if *project == state.project && *actual == address && token.is_cancelled()));
            ensure!(matches!(&notices[2], Event::Error {
                project: Some(project), operation: "stop_proxy", message,
            } if *project == state.project && *message == error.to_string()));
            ensure!(
                state.proxy.is_some(),
                "no stopped ACK before releasing the runner"
            );
            ensure!(received.is_empty() && state.proxy_notifications.is_empty());

            state.events.try_send(Event::Stopped)?;
            release.send(())?;
            if shutdown {
                state.finish_proxy_shutdown().await?;
            } else {
                std::future::poll_fn(|cx| {
                    let pending = {
                        let mut permit = std::pin::pin!(state.event_permit());
                        permit.as_mut().poll(cx).is_pending()
                    };
                    if !pending {
                        return Poll::Ready(Err(anyhow::anyhow!(
                            "reaping must not require capacity in the full UI queue"
                        )));
                    }
                    if state.proxy.is_none() {
                        Poll::Ready(Ok(()))
                    } else {
                        Poll::Pending
                    }
                })
                .await?;
            }
            ensure!(state.proxy.is_none());
            ensure!(received.len() == 1, "reaping must leave the UI queue full");
            ensure!(upstream.read(&mut [0; 1]).await? == 0);
            ensure!(downstream.read(&mut [0; 1]).await? == 0);
            ensure!(TcpStream::connect(address).await.is_err());
            ensure!(
                origin
                    .into_std()?
                    .accept()
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::WouldBlock),
                "the non-idempotent POST must not be retried"
            );
            let terminal = state.store.read(initial.summary.id)?;
            ensure!(terminal.summary.state == FlowState::Cancelled);
            ensure!(terminal.summary.revision > initial.summary.revision);
            ensure!(terminal.request == initial.request && terminal.response.is_none());
            ensure!(state.store.page(0, 2, "")?.0 == 1);
            ensure!(state.unsaved_count() == 0 && state.pending.is_empty());
            let reopened = ProjectStore::open(&state.store.test_actual_path())?;
            ensure!(reopened.read(initial.summary.id)? == terminal);
            ensure!(state.proxy_events.is_empty());
            ensure!(state.proxy_notifications.len() == 1);
            ensure!(matches!(received.try_recv()?, Event::Stopped));
            state.flush_proxy_notifications().await;
            ensure!(matches!(received.try_recv()?, Event::ProxyState {
                project, address: None, cancel: None, ..
            } if project == state.project));
            state.finish_proxy_shutdown().await?;
            ensure!(state.proxy_notifications.is_empty());
            ensure!(received.is_empty());
            Ok::<_, anyhow::Error>(())
        })
        .await;

        // Reserve the last second for release and joins even after a failed observation.
        if let Some(proxy) = &state.proxy {
            proxy.request_stop();
        }
        let _ = release.send(());
        let cleanup = tokio::time::timeout_at(
            deadline - Duration::from_millis(500),
            state.finish_proxy_shutdown(),
        )
        .await;
        let _ = finish.send(());
        tokio::time::timeout_at(
            deadline,
            tokio::task::spawn_blocking(move || runtime_thread.join()),
        )
        .await
        .expect("dedicated Proxy runtime cleanup fits the total 8s budget")
        .unwrap()
        .unwrap();
        cleanup.expect("remaining Proxy join is bounded").unwrap();
        outcome
            .expect("both deadline recovery paths fit the shared 8s budget")
            .unwrap_or_else(|error| panic!("finish_proxy_shutdown={shutdown}: {error:#}"));
    }
}
