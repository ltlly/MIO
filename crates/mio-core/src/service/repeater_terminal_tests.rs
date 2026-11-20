use super::*;
use std::{
    io::{ErrorKind, Write},
    net::TcpListener as StdTcpListener,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::AbortHandle,
};

mod recovery_and_quiesce;
mod sse_streaming;
mod tls_trust;
const PAYLOAD: &[u8] = &[0, 255, b'P'];
const RESPONSE: &[u8] = &[0, 254, b'R'];

struct HeldSend {
    tab_id: TabId,
    task: AbortHandle,
    expected: FlowRecord,
    release: oneshot::Sender<()>,
    listener: StdTcpListener,
}

async fn held_send(state: &mut State) -> HeldSend {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let origin = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).await.unwrap();
            head.push(byte[0]);
            assert!(head.len() <= 1024, "bounded synthetic request head");
        }
        let mut body = [0; 3];
        stream.read_exact(&mut body).await.unwrap();
        stream
            .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 3\r\n\r\n")
            .await
            .unwrap();
        stream.write_all(RESPONSE).await.unwrap();
        let eof = stream.read(&mut [0]).await.unwrap();
        (listener.into_std().unwrap(), head, body, eof)
    });
    let mut record = tests::completed_flow();
    record.summary.url = format!("http://{address}/owned-repeater");
    record.summary.revision = 1;
    record.summary.state = FlowState::Pending;
    record.summary.status = None;
    record.summary.response_bytes = 0;
    record.request.url = record.summary.url.clone();
    record.request.body = PAYLOAD.to_vec();
    record.response = None;
    state.store.put(&record).unwrap();
    let flow_id = record.summary.id;
    let project = state.project;
    let tab_id = Uuid::new_v4();
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let (release, released) = oneshot::channel();
    let (ready, response_ready) = oneshot::channel();
    let task = state.jobs.spawn(async move {
        let response = mio_proxy::send(
            record.request.clone(),
            SendConfig {
                max_body_bytes: 1024,
                timeout: Duration::from_secs(2),
                http_version: Some(HttpVersion::Http11),
                ..SendConfig::default()
            },
            task_cancel,
        )
        .await
        .unwrap();
        record.summary.revision += 1;
        record.summary.state = FlowState::Complete;
        record.summary.status = Some(response.status);
        record.summary.response_bytes = response.body.len() as u64;
        record.response = Some(response);
        ready.send(record.clone()).unwrap();
        // Hold the caller's completion tail; client tests separately hold the actual SDK guard.
        let _ = released.await;
        Completion {
            project,
            tab_id,
            draft_revision: 7,
            received: None,
            record,
        }
    });
    state.sends.insert(
        tab_id,
        ActiveSend {
            cancel,
            task_id: task.id(),
            flow_id,
            draft_revision: 7,
            progress_rx: None,
        },
    );
    let expected = response_ready.await.unwrap();
    let (listener, head, body, eof) = origin.await.unwrap();
    assert!(head.starts_with(b"POST /owned-repeater HTTP/1.1\r\n"));
    assert!(
        String::from_utf8(head)
            .unwrap()
            .to_ascii_lowercase()
            .contains("\r\ncontent-length: 3\r\n")
    );
    assert_eq!(body, PAYLOAD);
    assert_eq!(
        eof, 0,
        "the real POST socket has closed before the held tail"
    );
    HeldSend {
        tab_id,
        task,
        expected,
        release,
        listener,
    }
}

fn gzip_response() -> HttpResponse {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(RESPONSE).unwrap();
    let body = encoder.finish().unwrap();
    HttpResponse {
        status: 201,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![
            Header {
                name: "Content-Encoding".into(),
                value: b"gzip".to_vec(),
            },
            Header {
                name: "Content-Length".into(),
                value: body.len().to_string().into_bytes(),
            },
            Header {
                name: "X-Duplicate".into(),
                value: b"one".to_vec(),
            },
            Header {
                name: "X-Duplicate".into(),
                value: b"two".to_vec(),
            },
        ],
        body,
        trailers: vec![],
    }
}

fn received_record(response: HttpResponse) -> FlowRecord {
    let mut record = tests::completed_flow();
    record.summary.status = Some(response.status);
    record.summary.response_bytes = response.body.len() as u64;
    record.response = Some(response);
    record
}

fn pending_record(received: &FlowRecord) -> FlowRecord {
    let mut pending = received.clone();
    pending.summary.revision -= 1;
    pending.summary.state = FlowState::Pending;
    pending.summary.status = None;
    pending.summary.response_bytes = 0;
    pending.summary.elapsed_ms = 0;
    pending.response = None;
    pending
}

async fn controlled_response_send(
    state: &mut State,
    response: &HttpResponse,
    unpack: bool,
    send_unpack: bool,
) -> (
    TabId,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<StdTcpListener>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let bytes = response_to_raw(response);
    let (release, released) = oneshot::channel();
    let (ready, request_arrived) = oneshot::channel();
    let origin = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(3), async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                assert!(head.len() < 4096);
                head.push(stream.read_u8().await.unwrap());
            }
            assert!(head.starts_with(b"POST /owned-repeater HTTP/1.1\r\n"));
            let mut body = [0; 3];
            stream.read_exact(&mut body).await.unwrap();
            assert_eq!(body, PAYLOAD);
            ready.send(()).unwrap();
            released.await.unwrap();
            stream.write_all(&bytes).await.unwrap();
            assert_eq!(stream.read(&mut [0]).await.unwrap(), 0);
            listener.into_std().unwrap()
        })
        .await
        .expect("controlled origin, response and EOF stay bounded")
    });
    let mut request = tests::completed_flow().request;
    request.url = format!("http://{address}/owned-repeater");
    request.body = PAYLOAD.to_vec();
    let mut workspace = state.store.workspace().unwrap();
    let tab_id = workspace.open_request(Some(&request)).unwrap();
    workspace.repeater_settings.unpack_compressed_responses = unpack;
    let draft = workspace
        .tabs
        .iter()
        .find(|tab| tab.id == tab_id)
        .unwrap()
        .draft
        .clone();
    state
        .command(Command::SaveWorkspace {
            project: state.project,
            workspace,
        })
        .await
        .unwrap();
    state
        .command(Command::Send {
            run_id: Uuid::new_v4(),
            cancel: tokio_util::sync::CancellationToken::new(),
            project: state.project,
            tab_id,
            draft,
            settings: RepeaterSettings {
                unpack_compressed_responses: send_unpack,
                ..RepeaterSettings::default()
            },
        })
        .await
        .unwrap();
    request_arrived.await.unwrap();
    (tab_id, release, origin)
}

#[tokio::test]
async fn repeater_freezes_saved_unpack_setting_and_keeps_rev_two_without_transform() {
    tokio::time::timeout(Duration::from_secs(8), async {
        for case in 0..4 {
            let (events, mut events_rx) = mpsc::channel(64);
            let mut state = State::new(events, CancellationToken::new()).unwrap();
            let mut response = gzip_response();
            if case == 2 {
                response.body = RESPONSE.to_vec();
                response.headers[1].value = response.body.len().to_string().into_bytes();
                response.headers.remove(0);
            } else if case == 3 {
                response.body = b"synthetic-invalid-gzip".to_vec();
                response.headers[1].value = response.body.len().to_string().into_bytes();
            }
            let enabled = case != 1;
            let (tab_id, release, origin) = controlled_response_send(&mut state, &response, enabled, enabled).await;
            let before = drain_events(&mut events_rx);
            assert!(before.iter().any(|event| matches!(event, Event::WorkspaceChanged { workspace, .. } if workspace.repeater_settings.unpack_compressed_responses == enabled)));
            let mut changed = state.store.workspace().unwrap();
            changed.revision += 1;
            changed.repeater_settings.unpack_compressed_responses = !enabled;
            state.command(Command::SaveWorkspace { project: state.project, workspace: changed.clone() }).await.unwrap();
            assert!(matches!(events_rx.try_recv().unwrap(), Event::WorkspaceChanged { workspace, .. } if workspace == changed));
            release.send(()).unwrap();
            let completion = state.jobs.join_next().await.unwrap().unwrap();
            let listener = origin.await.unwrap();
            let decoded = case == 0;
            assert_eq!(completion.received.is_some(), decoded);
            let original_response = completion.received.as_ref().and_then(|record| record.response.clone());
            let record = completion.record.clone();
            state.complete(completion).await.unwrap();
            let finished = drain_events(&mut events_rx);
            assert_eq!(finished.iter().filter(|event| matches!(event, Event::SendFinished { tab_id: id, record: actual, persisted: true, .. } if *id == tab_id && **actual == record)).count(), 1);
            assert_eq!(record.summary.revision, if decoded { 3 } else { 2 });
            assert_eq!(record.summary.status, Some(response.status));
            assert_eq!(record.response.as_ref().unwrap().body, if decoded { RESPONSE.to_vec() } else { response.body.clone() });
            if case == 3 {
                assert!(matches!(&record.summary.state, FlowState::Failed(message) if message.contains("解压") && !message.contains("synthetic-invalid-gzip")));
            } else {
                assert_eq!(record.summary.state, FlowState::Complete);
            }
            state.command(Command::ReadFlow { project: state.project, selection_revision: 7, flow_id: record.summary.id }).await.unwrap();
            let Event::FlowDetail { record: stored, original, .. } = events_rx.try_recv().unwrap() else { panic!("ReadFlow returns final and Original"); };
            assert_eq!(*stored, record);
            assert_eq!(original.request, None);
            assert_eq!(original.response, original_response);
            if let Some(original) = &original.response {
                assert_eq!(original.body, response.body);
            }
            assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
        }
    }).await.expect("four single-request fixtures and their joins stay bounded");
}

#[tokio::test]
async fn repeater_decode_failure_cancellation_deadline_and_worker_panic_keep_received_bytes() {
    tokio::time::timeout(Duration::from_secs(4), async {
        for case in 0..6 {
            let (events, mut events_rx) = mpsc::channel(16);
            let mut state = State::new(events, CancellationToken::new()).unwrap();
            let mut response = gzip_response();
            if case == 2 {
                response.body = b"synthetic-codec-canary".to_vec();
            }
            response.trailers.push(Header { name: "X-End".into(), value: vec![0, 255] });
            let mut record = received_record(response.clone());
            let expected = record.clone();
            let pending = pending_record(&record);
            state.store.put(&pending).unwrap();
            let cancel = CancellationToken::new();
            if case == 0 { cancel.cancel(); }
            let deadline = if case == 1 { Instant::now() - Duration::from_millis(1) } else { Instant::now() + Duration::from_secs(2) };
            let active_cancel = cancel.clone();
            let worker = tokio::task::spawn_blocking(move || {
                if case == 3 { return Err(http::UnpackError::Failed("synthetic-codec-canary".into())); }
                assert_ne!(case, 4, "synthetic-codec-canary");
                http::unpack_response(&response, "POST", if case == 5 { 1 } else { 1024 }, &cancel, deadline)
            });
            let task_id = worker.id();
            let received = finish_repeater_unpack(&mut record, worker).await;
            assert!(received.is_none());
            assert_eq!(record.summary.revision, 2);
            assert_eq!(record.response, expected.response);
            if case == 0 { assert_eq!(record.summary.state, FlowState::Cancelled); }
            else { assert!(matches!(&record.summary.state, FlowState::Failed(message) if !message.contains("synthetic-codec-canary"))); }
            let tab_id = Uuid::new_v4();
            state.sends.insert(tab_id, ActiveSend { cancel: active_cancel, task_id, flow_id: record.summary.id, draft_revision: 1, progress_rx: None });
            state.complete(Completion { project: state.project, tab_id, draft_revision: 1, received, record: record.clone() }).await.unwrap();
            assert!(drain_events(&mut events_rx).iter().any(|event| matches!(event, Event::SendFinished { record: stored, persisted: true, .. } if **stored == record)));
            assert_eq!(state.store.read(record.summary.id).unwrap(), record);
            assert_eq!(state.store.original(record.summary.id).unwrap(), OriginalMessages::default());
        }
    }).await.expect("small codec inputs and every blocking worker join stay bounded");
}

#[tokio::test]
async fn repeater_stop_retains_codec_handle_and_determined_success_wins_late_cancel() {
    tokio::time::timeout(Duration::from_secs(4), async {
        let (events, mut events_rx) = mpsc::channel(16);
        let mut state = State::new(events, CancellationToken::new()).unwrap();
        let response = gzip_response();
        let expected = received_record(response.clone());
        let pending = pending_record(&expected);
        state.store.put(&pending).unwrap();
        let mut record = expected.clone();
        let tab_id = Uuid::new_v4();
        let project = state.project;
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let (release, released) = std::sync::mpsc::sync_channel(1);
        let (ready, decoded) = oneshot::channel();
        let deadline = Instant::now() + Duration::from_secs(2);
        let task = state.jobs.spawn(async move {
            let worker = tokio::task::spawn_blocking(move || {
                let result = http::unpack_response(&response, "POST", 1024, &task_cancel, deadline);
                let _ = ready.send(());
                released.recv_timeout(Duration::from_secs(2)).unwrap();
                result
            });
            let received = finish_repeater_unpack(&mut record, worker).await;
            Completion { project, tab_id, draft_revision: 1, received, record }
        });
        state.sends.insert(tab_id, ActiveSend { cancel: cancel.clone(), task_id: task.id(), flow_id: expected.summary.id, draft_revision: 1, progress_rx: None });
        decoded.await.unwrap();
        let first_stop = state.stop_repeater_until(tokio::time::Instant::now()).await;
        let retained = !task.is_finished() && state.jobs.len() == 1 && state.sends.contains_key(&tab_id);
        let before = state.store.read(expected.summary.id).unwrap();
        let early = drain_events(&mut events_rx);
        let released = release.send(());
        let second_stop = state.stop_repeater_until(tokio::time::Instant::now() + Duration::from_secs(2)).await;
        let terminal = state.store.read(expected.summary.id).unwrap();
        assert!(first_stop.is_err());
        assert!(retained && cancel.is_cancelled());
        assert_eq!(before, pending);
        assert!(!early.iter().any(|event| matches!(event, Event::SendFinished { .. })));
        released.unwrap();
        second_stop.unwrap();
        assert!(state.jobs.is_empty() && state.sends.is_empty() && task.is_finished());
        assert_eq!(terminal.summary.state, FlowState::Complete);
        assert_eq!(terminal.summary.revision, 3);
        assert_eq!(terminal.response.as_ref().unwrap().body, RESPONSE);
        assert_eq!(state.store.original(terminal.summary.id).unwrap().response, expected.response);
        assert_eq!(drain_events(&mut events_rx).iter().filter(|event| matches!(event, Event::SendFinished { persisted: true, record, .. } if **record == terminal)).count(), 1);
    }).await.expect("Stop deadline retains and then joins the actual codec worker");
}

fn drain_events(events: &mut mpsc::Receiver<Event>) -> Vec<Event> {
    let mut received = Vec::new();
    while let Ok(event) = events.try_recv() {
        received.push(event);
    }
    received
}
