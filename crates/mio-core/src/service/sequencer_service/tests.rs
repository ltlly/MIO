use std::{
    net::SocketAddr,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    time::{Instant, sleep},
};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::{Draft, sequencer::SampleEncoding};

const HEAD_LIMIT: usize = 16 * 1024;

#[tokio::test]
async fn captures_tokens_analyzes_samples_and_persists_no_flows() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let mut state = State::new(events_tx, CancellationToken::new()).unwrap();
    let origin = tokio::spawn(token_origin(listener, true));
    let project = state.project;
    let run_id = Uuid::new_v4();
    let config = capture_config(address, "/token", 20, 4);

    state
        .sequencer_command(SequencerCommand::StartLiveCapture {
            project,
            run_id,
            config: Box::new(config),
            cancel: CancellationToken::new(),
        })
        .await;
    let Event::Sequencer(SequencerEvent::CaptureStarted {
        project: started_project,
        run_id: started_run_id,
        target_samples,
    }) = events_rx.recv().await.unwrap()
    else {
        panic!("expected CaptureStarted");
    };
    assert_eq!(started_project, project);
    assert_eq!(started_run_id, run_id);
    assert_eq!(target_samples, 20);

    let mut finished = None;
    while let Some(event) = state.sequencer.next_signal().await {
        if matches!(event, SequencerEvent::CaptureFinished { .. }) {
            finished = Some(event);
            break;
        }
        if let SequencerEvent::CaptureProgress { latest_samples, .. } = &event {
            assert!(latest_samples.len() <= 32);
        }
    }
    let Some(SequencerEvent::CaptureFinished {
        requests_sent,
        tokens_collected,
        extraction_misses,
        request_errors,
        samples,
        stopped_early,
        error,
        ..
    }) = finished
    else {
        panic!("expected CaptureFinished");
    };
    assert_eq!(requests_sent, 20);
    assert_eq!(tokens_collected, 20);
    assert_eq!(extraction_misses, 0);
    assert_eq!(request_errors, 0);
    assert_eq!(samples.len(), 20);
    assert!(!stopped_early);
    assert!(error.is_none());
    assert!(!state.sequencer.has_resources());
    assert!(!state.sequencer.has_in_flight_jobs());
    let (total, rows) = state.store.history_page(0, 50, "").unwrap();
    assert_eq!(total, 0);
    assert!(rows.is_empty());

    let operation_id = Uuid::new_v4();
    state
        .sequencer_command(SequencerCommand::AnalyzeSamples {
            project,
            operation_id,
            samples,
            cancel: CancellationToken::new(),
        })
        .await;
    let SequencerEvent::AnalysisReady {
        project: analysis_project,
        operation_id: analysis_operation_id,
        result,
    } = state.sequencer.next_signal().await.unwrap()
    else {
        panic!("expected AnalysisReady");
    };
    assert_eq!(analysis_project, project);
    assert_eq!(analysis_operation_id, operation_id);
    let analysis = result.unwrap();
    assert_eq!(analysis.sample_count, 20);
    assert_eq!(analysis.encoding, SampleEncoding::Hex);
    assert!(analysis.effective_entropy_bits > 0.0);
    origin.abort();
}

#[tokio::test]
async fn stop_and_quiesce_cancel_capture_and_join_without_connection_leaks() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let disconnected = Arc::new(AtomicUsize::new(0));
    let origin = tokio::spawn(held_origin(
        listener,
        accepted.clone(),
        disconnected.clone(),
    ));
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let mut state = State::new(events_tx, CancellationToken::new()).unwrap();
    let project = state.project;

    let first_run_id = Uuid::new_v4();
    state
        .sequencer_command(SequencerCommand::StartLiveCapture {
            project,
            run_id: first_run_id,
            config: Box::new(capture_config(address, "/held", 100, 4)),
            cancel: CancellationToken::new(),
        })
        .await;
    wait_for_count(&accepted, 4).await;
    state
        .sequencer_command(SequencerCommand::StopLiveCapture {
            project,
            run_id: first_run_id,
        })
        .await;
    let first_finished = wait_capture_finished(&mut state).await;
    assert!(
        matches!(
            first_finished,
            SequencerEvent::CaptureFinished {
                stopped_early: true,
                error: None,
                ..
            }
        ),
        "unexpected first finish: {first_finished:?}"
    );
    assert!(!state.sequencer.has_resources());
    assert!(!state.sequencer.has_in_flight_jobs());
    wait_for_count(&disconnected, accepted.load(Ordering::SeqCst)).await;

    let second_run_id = Uuid::new_v4();
    state
        .sequencer_command(SequencerCommand::StartLiveCapture {
            project,
            run_id: second_run_id,
            config: Box::new(capture_config(address, "/held", 100, 3)),
            cancel: CancellationToken::new(),
        })
        .await;
    wait_for_count(&accepted, 7).await;
    state.sequencer.cancel_all();
    state.sequencer.quiesce_project(project).await.unwrap();
    state.flush_sequencer_events().await;
    let second_finished = wait_sequencer_finished(&mut events_rx).await;
    assert!(
        matches!(
            second_finished,
            SequencerEvent::CaptureFinished {
                stopped_early: true,
                ..
            }
        ),
        "unexpected second finish: {second_finished:?}"
    );
    assert!(!state.sequencer.has_resources());
    assert!(!state.sequencer.has_in_flight_jobs());
    wait_for_count(&disconnected, accepted.load(Ordering::SeqCst)).await;

    state.sequencer.drop_project(project);
    state.sequencer.shutdown().await.unwrap();
    origin.abort();
}

#[tokio::test]
async fn rejects_invalid_config_and_halts_missing_token_spin() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let origin = tokio::spawn(token_origin(listener, false));
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let mut state = State::new(events_tx, CancellationToken::new()).unwrap();
    let project = state.project;

    let mut invalid = capture_config(address, "/missing", 1, 4);
    state
        .sequencer_command(SequencerCommand::StartLiveCapture {
            project,
            run_id: Uuid::new_v4(),
            config: Box::new(invalid.clone()),
            cancel: CancellationToken::new(),
        })
        .await;
    assert!(matches!(
        events_rx.recv().await.unwrap(),
        Event::Sequencer(SequencerEvent::CaptureFinished {
            requests_sent: 0,
            stopped_early: true,
            error: Some(_),
            ..
        })
    ));

    invalid.target_samples = 2;
    state
        .sequencer_command(SequencerCommand::StartLiveCapture {
            project,
            run_id: Uuid::new_v4(),
            config: Box::new(invalid),
            cancel: CancellationToken::new(),
        })
        .await;
    assert!(matches!(
        events_rx.recv().await.unwrap(),
        Event::Sequencer(SequencerEvent::CaptureStarted {
            target_samples: 2,
            ..
        })
    ));
    let finished = wait_capture_finished(&mut state).await;
    let SequencerEvent::CaptureFinished {
        requests_sent,
        tokens_collected,
        extraction_misses,
        stopped_early,
        error,
        ..
    } = finished
    else {
        panic!("expected CaptureFinished");
    };
    assert_eq!(requests_sent, 6);
    assert_eq!(tokens_collected, 0);
    assert_eq!(extraction_misses, 6);
    assert!(stopped_early);
    assert!(error.is_some());
    origin.abort();
}

fn capture_config(
    address: SocketAddr,
    path: &str,
    target_samples: usize,
    concurrency: usize,
) -> SequencerCaptureConfig {
    let target = format!("http://{address}");
    let raw =
        format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n").into_bytes();
    SequencerCaptureConfig {
        draft: Draft {
            revision: 1,
            target,
            raw,
            version: crate::HttpVersion::Http11,
        },
        location: SequencerTokenLocation::Cookie {
            name: "sid".to_owned(),
        },
        target_samples,
        concurrency,
    }
}

async fn wait_for_count(count: &AtomicUsize, minimum: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while count.load(Ordering::SeqCst) < minimum && Instant::now() < deadline {
        sleep(Duration::from_millis(10)).await;
    }
    assert!(count.load(Ordering::SeqCst) >= minimum);
}

async fn wait_capture_finished(state: &mut State) -> SequencerEvent {
    while let Some(event) = state.sequencer.next_signal().await {
        if matches!(event, SequencerEvent::CaptureFinished { .. }) {
            return event;
        }
    }
    panic!("capture finished future ended without CaptureFinished");
}

async fn wait_sequencer_finished(events_rx: &mut mpsc::Receiver<Event>) -> SequencerEvent {
    loop {
        let Event::Sequencer(event) = events_rx.recv().await.unwrap() else {
            continue;
        };
        if matches!(event, SequencerEvent::CaptureFinished { .. }) {
            return event;
        }
    }
}

async fn token_origin(listener: TcpListener, include_cookie: bool) {
    let token_seed = Arc::new(AtomicU64::new(0x9e37_79b9_7f4a_7c15));
    while let Ok((mut stream, _)) = listener.accept().await {
        let token_seed = token_seed.clone();
        tokio::spawn(async move {
            let _head = read_head(&mut stream).await;
            let response = if include_cookie {
                let token = next_token(&token_seed);
                format!(
                    "HTTP/1.1 200 OK\r\nSet-Cookie: sid={token}; HttpOnly\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                )
            } else {
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_owned()
            };
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
    }
}

async fn held_origin(
    listener: TcpListener,
    accepted: Arc<AtomicUsize>,
    disconnected: Arc<AtomicUsize>,
) {
    while let Ok((mut stream, _)) = listener.accept().await {
        accepted.fetch_add(1, Ordering::SeqCst);
        let disconnected = disconnected.clone();
        tokio::spawn(async move {
            let _head = read_head(&mut stream).await;
            let mut ignored = [0_u8; 64];
            while stream.read(&mut ignored).await.unwrap_or(0) != 0 {}
            disconnected.fetch_add(1, Ordering::SeqCst);
        });
    }
}

async fn read_head(stream: &mut TcpStream) -> Vec<u8> {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let byte = match stream.read_u8().await {
            Ok(byte) => byte,
            Err(_) => break,
        };
        head.push(byte);
        if head.len() >= HEAD_LIMIT {
            break;
        }
    }
    head
}

fn next_token(seed: &AtomicU64) -> String {
    let state = seed.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::SeqCst);
    let mixed = state ^ 0x9e37_79b9_7f4a_7c15;
    let high = mixed.wrapping_mul(0x2545_f491_4f6c_dd1d);
    let low = high.rotate_left(21).wrapping_add(0x7f4a_7c15_9e37_79b9);
    format!("{high:016x}{low:016x}")
}
