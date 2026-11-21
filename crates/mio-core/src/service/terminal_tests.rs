use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

mod reap_and_stop;
#[tokio::test]
async fn full_ui_queue_cannot_interrupt_durable_proxy_stop_drain() {
    let (events, mut received) = mpsc::channel(1);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    state.events.try_send(Event::Stopped).unwrap();
    let mut first = tests::completed_flow();
    first.summary.source = TrafficSource::Proxy;
    let mut second = first.clone();
    second.summary.id = Uuid::new_v4();
    let mut decisions = Vec::new();
    for record in [&first, &second] {
        let (reply, decision) = oneshot::channel();
        decisions.push(decision);
        state.pending.insert(
            record.summary.id,
            Pending {
                revision: 1,
                stage: InterceptStage::Response,
                response_head_only: false,
                intercepted_request: record.request.clone(),
                intercepted_response: record.response.clone(),
                reply,
            },
        );
        state
            .proxy_sender
            .try_send(ProxyEvent::Captured {
                project: state.project,
                record: record.clone(),
                response_stream: None,
                listener_port: None,
                capture_metrics: mio_proxy::CapturedFlowMetrics::default(),
            })
            .unwrap();
    }

    let stopped = tokio::time::timeout(Duration::from_millis(200), state.stop_proxy()).await;
    let final_record = state.store.read_optional(second.summary.id).unwrap();
    // Release all output backpressure before asserting the failure; no task or socket is leaked.
    if stopped.is_err() {
        state.cancel.cancel();
        state.stop_proxy().await.unwrap();
    }
    assert!(matches!(received.try_recv().unwrap(), Event::Stopped));
    assert_eq!(state.store.read(first.summary.id).unwrap(), first);
    assert_eq!(state.store.read(second.summary.id).unwrap(), second);
    assert!(
        stopped.is_ok_and(|result| result.is_ok()),
        "Stop must drain final captures without waiting for UI event capacity"
    );
    assert_eq!(final_record, Some(second.clone()));
    assert!(state.pending.is_empty());
    for mut decision in decisions {
        assert!(matches!(
            decision.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        ));
    }
    let notifications = tokio::time::timeout(Duration::from_secs(1), async {
        let (_, notifications) = tokio::join!(state.flush_proxy_notifications(), async {
            let mut notifications = Vec::new();
            for _ in 0..4 {
                notifications.push(received.recv().await.unwrap());
            }
            notifications
        });
        notifications
    })
    .await
    .unwrap();
    assert!(
        matches!(notifications[0], Event::InterceptResolved { flow_id, .. } if flow_id == first.summary.id)
    );
    assert!(
        matches!(notifications[1], Event::HistoryChanged { project } if project == state.project)
    );
    assert!(
        matches!(notifications[2], Event::InterceptResolved { flow_id, .. } if flow_id == second.summary.id)
    );
    assert!(
        matches!(notifications[3], Event::ProxyState { project, address: None, .. } if project == state.project)
    );
    assert!(state.proxy_notifications.is_empty());
    assert!(received.try_recv().is_err());
}

#[tokio::test]
async fn proxy_stop_outbox_preserves_each_storage_error_and_blocks_new_command_admission() {
    use std::future::Future;

    for quiesce in [false, true] {
        let (events, mut received) = mpsc::channel(2);
        let cancel = CancellationToken::new();
        let state = State::new(events, cancel.clone()).unwrap();
        let project = state.project;
        let mut first = tests::completed_flow();
        first.summary.source = TrafficSource::Proxy;
        let mut second = first.clone();
        second.summary.id = Uuid::new_v4();
        for record in [&first, &second] {
            state
                .proxy_sender
                .try_send(ProxyEvent::Captured {
                    project,
                    record: record.clone(),
                    response_stream: None,
                    listener_port: None,
                    capture_metrics: mio_proxy::CapturedFlowMetrics::default(),
                })
                .unwrap();
        }
        let (commands, receiver) = mpsc::channel(2);
        commands
            .try_send(if quiesce {
                Command::Quiesce { project }
            } else {
                Command::StopProxy { project }
            })
            .unwrap();
        commands
            .try_send(Command::QueryHistory {
                project,
                query_revision: 9,
                offset: 0,
                limit: 2,
                filter: String::new(),
            })
            .unwrap();
        let run = state.run(receiver);
        tokio::pin!(run);
        std::future::poll_fn(|cx| {
            assert!(run.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(
            received.len(),
            2,
            "initial project notifications fill the UI queue"
        );
        assert_eq!(
            commands.capacity(),
            1,
            "Stop was admitted, the next command was not"
        );
        let mut notifications = Vec::new();
        for _ in 0..8 {
            while let Ok(event) = received.try_recv() {
                notifications.push(event);
            }
            if notifications.iter().any(|event| {
                matches!(
                    event,
                    Event::HistoryPage {
                        query_revision: 9,
                        ..
                    }
                )
            }) {
                break;
            }
            std::future::poll_fn(|cx| {
                assert!(run.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
        }
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), run)
            .await
            .unwrap()
            .unwrap();
        let stop_state = notifications
            .iter()
            .enumerate()
            .filter_map(|(index, event)| {
                matches!(event, Event::ProxyState { address: None, .. }).then_some(index)
            })
            .nth(1)
            .unwrap();
        let (history_index, total, rows) = notifications
            .iter()
            .enumerate()
            .find_map(|(index, event)| match event {
                Event::HistoryPage {
                    query_revision: 9,
                    total,
                    rows,
                    ..
                } => Some((index, *total, rows)),
                _ => None,
            })
            .unwrap();
        assert_eq!(total, 2);
        assert!(
            rows.iter()
                .any(|row| row.id() == HistoryEntryId::Live(first.summary.id))
        );
        assert!(
            rows.iter()
                .any(|row| row.id() == HistoryEntryId::Live(second.summary.id))
        );
        assert!(stop_state < history_index);
        if quiesce {
            let quiesced = notifications.iter().position(|event| matches!(event, Event::Quiesced { project: actual } if *actual == project)).unwrap();
            assert!(stop_state < quiesced && quiesced < history_index);
        }
        assert!(
            !notifications
                .iter()
                .any(|event| matches!(event, Event::Error { .. }))
        );
    }

    let (events, mut received) = mpsc::channel(1);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    state.events.try_send(Event::Stopped).unwrap();
    state.store.reject_writes(true);
    for _ in 0..2 {
        state
            .proxy_sender
            .try_send(ProxyEvent::Captured {
                project: state.project,
                record: tests::completed_flow(),
                response_stream: None,
                listener_port: None,
                capture_metrics: mio_proxy::CapturedFlowMetrics::default(),
            })
            .unwrap();
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(200), state.stop_proxy())
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(state.recovery.len(), 2);
    assert!(matches!(received.try_recv().unwrap(), Event::Stopped));
    let notifications = tokio::time::timeout(Duration::from_secs(1), async {
        let (_, notifications) = tokio::join!(state.flush_proxy_notifications(), async {
            let mut notifications = Vec::new();
            for _ in 0..4 {
                notifications.push(received.recv().await.unwrap());
            }
            notifications
        });
        notifications
    })
    .await
    .unwrap();
    assert_eq!(notifications.iter().filter(|event| matches!(event, Event::Error { operation: "capture", message, .. } if !message.is_empty())).count(), 2);
    assert!(matches!(notifications[2], Event::HistoryChanged { .. }));
    assert!(matches!(
        notifications[3],
        Event::ProxyState { address: None, .. }
    ));
    assert!(state.proxy_notifications.is_empty());
}

#[tokio::test]
async fn sse_head_only_rejections_keep_pending_and_allow_one_valid_forward() {
    let (events, mut received) = mpsc::channel(8);
    let mut state = State::new(events, CancellationToken::new()).unwrap();
    let id = Uuid::new_v4();
    let (reply, mut decision) = oneshot::channel();
    let sample = tests::completed_flow();
    state.pending.insert(
        id,
        Pending {
            revision: 7,
            stage: InterceptStage::Response,
            response_head_only: true,
            intercepted_request: sample.request,
            intercepted_response: sample.response,
            reply,
        },
    );
    let head = HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![Header {
            name: "Content-Type".into(),
            value: b"text/event-stream".to_vec(),
        }],
        body: Vec::new(),
        trailers: Vec::new(),
    };
    for kind in 0..3 {
        let mut invalid = head.clone();
        match kind {
            0 => invalid.body = vec![0, 255],
            1 => invalid.trailers.push(Header {
                name: "X-End".into(),
                value: b"changed".to_vec(),
            }),
            2 => invalid.status = 101,
            _ => unreachable!(),
        }
        let result = state
            .command(Command::Decide {
                project: state.project,
                flow_id: id,
                revision: 7,
                decision: Decision::ForwardResponse(invalid),
            })
            .await;
        assert!(
            result.is_err(),
            "unsupported SSE replacement {kind} must be rejected"
        );
        assert!(state.pending.contains_key(&id));
        assert!(matches!(
            decision.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert!(
            received.try_recv().is_err(),
            "rejection must not resolve the intercept"
        );
    }
    state
        .command(Command::Decide {
            project: state.project,
            flow_id: id,
            revision: 7,
            decision: Decision::ForwardResponse(head.clone()),
        })
        .await
        .unwrap();
    assert!(!state.pending.contains_key(&id));
    assert!(
        matches!(decision.await.unwrap(), InterceptDecision::ForwardResponse(actual) if actual == head)
    );
    assert!(
        matches!(received.try_recv().unwrap(), Event::InterceptResolved { flow_id, .. } if flow_id == id)
    );
    assert!(received.try_recv().is_err());
}

#[tokio::test]
async fn full_proxy_queue_and_storage_failure_still_preserve_the_terminal_record() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let origin = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let target = format!("http://{}/terminal", origin.local_addr().unwrap());
        let (seen, received) = oneshot::channel();
        let source = tokio::spawn(async move {
            let (mut stream, _) = origin.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                assert!(head.len() < 8192);
                head.push(stream.read_u8().await.unwrap());
            }
            let mut body = [0; 3];
            stream.read_exact(&mut body).await.unwrap();
            seen.send(body).unwrap();
            assert!(matches!(stream.read(&mut [0; 1]).await, Ok(0) | Err(_)));
        });
        let (events, _receiver) = mpsc::channel(32);
        let mut state = State::new(events, CancellationToken::new()).unwrap();
        let (sender, proxy_events) = mpsc::channel(1);
        state.proxy_events = proxy_events;
        state.proxy_sender = sender.clone();
        let config = ProxyConfig::new(MitmAuthority::generate().unwrap());
        state.proxy = Some(
            mio_proxy::start(config, state.project, sender)
                .await
                .unwrap(),
        );
        let mut client = TcpStream::connect(state.proxy.as_ref().unwrap().address())
            .await
            .unwrap();
        client
            .write_all(
                format!("POST {target} HTTP/1.1\r\nHost: localhost\r\nContent-Length: 3\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        client.write_all(&[0, 0xff, b'T']).await.unwrap();
        assert_eq!(received.await.unwrap(), [0, 0xff, b'T']);
        assert_eq!(state.proxy_events.len(), 1);
        state.store.reject_writes(true);
        let stopped = state.stop_proxy().await;
        assert!(matches!(client.read(&mut [0; 1]).await, Ok(0) | Err(_)));
        source.await.unwrap();
        assert!(stopped.is_err(), "storage failure must be visible");
        let record = &state
            .recovery
            .back()
            .expect("unsaved traffic remains recoverable")
            .record;
        assert_eq!(
            record.summary.state,
            FlowState::Cancelled,
            "Stop must drain the terminal mailbox before marking or saving pending traffic"
        );
        assert_eq!(record.request.body, [0, 0xff, b'T']);
        assert_eq!(state.read_flow(record.summary.id).unwrap(), *record);
        assert!(state.proxy.is_none());
    })
    .await
    .expect("bounded real socket Stop and fixture cleanup");
}
