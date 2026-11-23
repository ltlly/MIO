use std::{io::ErrorKind, time::Duration};

use mio_core::{
    Client, Command, Decision, Event, FlowId, FlowRecord, FlowState, InterceptStage,
    OriginalMessages, ProjectKey, ProxyInterceptOperator, ProxyInterceptRelation,
    ProxyInterceptRule, ProxyInterceptType, Runtime, Uuid, Workspace,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

const FIRST: &[u8] = b"data: first\n\n";
const SECOND: &[u8] = b"data: second\n\n";
const DEADLINE: Duration = Duration::from_secs(12);

#[derive(Clone, Copy, PartialEq, Eq)]
enum End {
    Complete,
    Stop,
    WriteFault,
    SuppressBody,
    EnableAfterUnintercepted,
}

async fn event(client: &mut Client, matches: impl Fn(&Event) -> bool) -> Event {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = client
                .events
                .recv()
                .await
                .expect("service remains connected");
            if matches(&event) {
                return event;
            }
            assert!(
                !matches!(event, Event::Error { .. }),
                "unexpected service error: {event:?}"
            );
        }
    })
    .await
    .expect("bounded causal service event")
}

async fn opened_with_workspace(client: &mut Client) -> (ProjectKey, Workspace) {
    let Event::ProjectOpened {
        project, workspace, ..
    } = event(client, |event| matches!(event, Event::ProjectOpened { .. })).await
    else {
        unreachable!()
    };
    (project, workspace)
}

async fn opened(client: &mut Client) -> ProjectKey {
    opened_with_workspace(client).await.0
}

async fn head(stream: &mut TcpStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        assert!(bytes.len() < 8192, "bounded fixture header");
        bytes.push(stream.read_u8().await.unwrap());
    }
    bytes
}

async fn detail(
    client: &mut Client,
    project: ProjectKey,
    flow_id: FlowId,
) -> (FlowRecord, OriginalMessages) {
    client
        .commands
        .send(Command::ReadFlow {
            project,
            selection_revision: 7,
            flow_id,
        })
        .await
        .unwrap();
    let Event::FlowDetail {
        record, original, ..
    } = event(client, |event| {
        matches!(
            event,
            Event::FlowDetail {
                selection_revision: 7,
                ..
            }
        )
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(record.summary.id, flow_id);
    (*record, *original)
}

async fn save(client: &mut Client, project: ProjectKey, path: Option<std::path::PathBuf>) {
    client
        .commands
        .send(Command::SaveProject {
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
            project,
            path,
            workspace: None,
        })
        .await
        .unwrap();
    event(client, |event| matches!(event, Event::ProjectSaved { .. })).await;
}

fn stream_header(response: &mio_core::HttpResponse) -> &[u8] {
    &response
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("x-stream"))
        .expect("fixture response header")
        .value
}

async fn observe(edited: bool, end: End) -> (FlowRecord, OriginalMessages) {
    tokio::time::timeout(DEADLINE, async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (release, released) = oneshot::channel();
        let origin = tokio::spawn(async move {
            if end == End::EnableAfterUnintercepted {
                let (mut stream, _) = listener.accept().await.unwrap();
                assert!(head(&mut stream).await.starts_with(b"GET /rules-off HTTP/1.1\r\n"));
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: rules-off\n\n").await.unwrap();
                stream.shutdown().await.unwrap();
            }
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = head(&mut stream).await;
            assert!(request.starts_with(b"GET /sse-original HTTP/1.1\r\n"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nX-Stream: upstream\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            stream.write_all(FIRST).await.unwrap();
            if matches!(end, End::Stop | End::SuppressBody) {
                assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0);
            } else {
                released.await.unwrap();
                stream.write_all(SECOND).await.unwrap();
                stream.shutdown().await.unwrap();
            }
            listener
        });
        let (runtime, mut client) = Runtime::start().unwrap();
        let (project, mut workspace) = opened_with_workspace(&mut client).await;
        assert!(!workspace.proxy_messages.unpack_compressed_requests);
        assert!(workspace.proxy_messages.unpack_compressed_responses);
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("sse-original.mio");
        let copy = directory.path().join("sse-original-copy.mio");
        save(&mut client, project, Some(source.clone())).await;
        client.commands.send(Command::SetIntercept { project, requests: false, responses: edited }).await.unwrap();
        client.commands.send(Command::StartProxy { project, listen: "127.0.0.1:0".parse().unwrap() }).await.unwrap();
        let Event::ProxyState { address: Some(proxy), .. } = event(&mut client, |event| matches!(event, Event::ProxyState { address: Some(_), .. })).await else { unreachable!() };
        if edited {
            if end == End::EnableAfterUnintercepted {
                assert!(!workspace.proxy_interception.response.enabled);
                let mut unheld = TcpStream::connect(proxy).await.unwrap();
                unheld.write_all(format!("GET http://{address}/rules-off HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
                let mut passed = Vec::new();
                unheld.read_to_end(&mut passed).await.unwrap();
                assert!(passed.starts_with(b"HTTP/1.1 200"));
                assert!(passed.windows(b"data: rules-off\n\n".len()).any(|bytes| bytes == b"data: rules-off\n\n"));
            }
            workspace.revision += 1;
            workspace.proxy_interception.response.enabled = true;
            if end == End::EnableAfterUnintercepted {
                workspace.proxy_interception.response.rules.push(ProxyInterceptRule {
                    id: Uuid::new_v4(),
                    enabled: true,
                    operator: ProxyInterceptOperator::And,
                    match_type: ProxyInterceptType::Body,
                    relationship: ProxyInterceptRelation::RequestMatches,
                    condition: "^$".into(),
                });
            }
            client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
            let Event::WorkspaceChanged { workspace: acknowledged, .. } = event(&mut client, |event| matches!(event,
                Event::WorkspaceChanged { project: owner, workspace: value } if *owner == project && value.revision == workspace.revision
            )).await else { unreachable!() };
            assert_eq!(acknowledged, workspace);
        }
        let mut downstream = TcpStream::connect(proxy).await.unwrap();
        downstream.write_all(format!("GET http://{address}/sse-original HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let intercepted_flow = if edited {
            let Event::InterceptPending { stage, response_head_only, record, .. } = event(&mut client, |event| matches!(event, Event::InterceptPending { .. })).await else { unreachable!() };
            assert_eq!(stage, InterceptStage::Response);
            assert!(response_head_only);
            let mut response = record.response.unwrap();
            assert!(response.body.is_empty());
            response.headers.iter_mut().find(|header| header.name.eq_ignore_ascii_case("x-stream")).unwrap().value = b"edited".to_vec();
            response.headers.iter_mut().find(|header| header.name.eq_ignore_ascii_case("content-type")).unwrap().value = b"application/octet-stream".to_vec();
            if end == End::SuppressBody { response.status = 204; }
            client.commands.send(Command::Decide { project, flow_id: record.summary.id, revision: record.summary.revision, decision: Decision::ForwardResponse(response) }).await.unwrap();
            Some(record.summary.id)
        } else {
            None
        };

        let mut wire = head(&mut downstream).await;
        let expected_header = if edited { "x-stream: edited" } else { "x-stream: upstream" };
        assert!(String::from_utf8_lossy(&wire).to_ascii_lowercase().contains(expected_header));
        while end != End::SuppressBody && !wire.windows(FIRST.len()).any(|bytes| bytes == FIRST) {
            let mut buffer = [0; 256];
            let count = downstream.read(&mut buffer).await.unwrap();
            assert!(count > 0, "first event arrives while the source still withholds EOF");
            wire.extend_from_slice(&buffer[..count]);
            assert!(wire.len() < 8192);
        }
        assert!(!wire.windows(SECOND.len()).any(|bytes| bytes == SECOND));

        // The ACK follows the durable edited head, so the write fault targets only terminal capture.
        if let Some(flow_id) = intercepted_flow {
            event(&mut client, |event| matches!(event, Event::InterceptResolved { flow_id: resolved, .. } if *resolved == flow_id)).await;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            let record = loop {
                let (record, _) = detail(&mut client, project, flow_id).await;
                if stream_header(record.response.as_ref().unwrap()) == b"edited"
                    || tokio::time::Instant::now() >= deadline
                {
                    break record;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            };
            assert_eq!(stream_header(record.response.as_ref().unwrap()), b"edited");
            if end != End::SuppressBody { assert_eq!(record.summary.state, FlowState::Pending); }
        }
        let writer = (end == End::WriteFault).then(|| {
            let connection = rusqlite::Connection::open(&source).unwrap();
            connection.execute_batch("BEGIN IMMEDIATE").unwrap();
            connection
        });
        if matches!(end, End::Stop | End::SuppressBody) {
            client.commands.send(Command::StopProxy { project }).await.unwrap();
        } else {
            release.send(()).unwrap();
            downstream.read_to_end(&mut wire).await.unwrap();
            assert!(wire.windows(SECOND.len()).any(|bytes| bytes == SECOND));
            if end == End::WriteFault {
                event(&mut client, |event| matches!(event, Event::Error { operation: "capture", .. })).await;
                event(&mut client, |event| matches!(event, Event::StorageState { pending_records, .. } if *pending_records > 0)).await;
            }
            client.commands.send(Command::StopProxy { project }).await.unwrap();
        }
        event(&mut client, |event| matches!(event, Event::ProxyState { address: None, .. })).await;
        if matches!(end, End::Stop | End::SuppressBody) {
            downstream.read_to_end(&mut wire).await.unwrap();
        }
        let listener = origin.await.unwrap();
        client.commands.send(Command::QueryHistory { project, query_revision: 1, offset: 0, limit: 1, filter: String::new() }).await.unwrap();
        let Event::HistoryPage { total, rows, .. } = event(&mut client, |event| matches!(event, Event::HistoryPage { query_revision: 1, .. })).await else { unreachable!() };
        assert_eq!(total, if end == End::EnableAfterUnintercepted { 2 } else { 1 });
        let [mio_core::HistoryRow::Live(summary)] = rows.as_slice() else {
            panic!("the latest captured SSE must be the single requested page row");
        };
        let flow_id = summary.id;
        if let Some(intercepted_flow) = intercepted_flow { assert_eq!(flow_id, intercepted_flow); }
        let observed = detail(&mut client, project, flow_id).await;
        if let Some(writer) = writer {
            client.commands.send(Command::SaveProject { websocket_notes: Vec::new(), ext_http_notes: Vec::new(), ext_http_highlights: Vec::new(), project, path: None, workspace: None }).await.unwrap();
            event(&mut client, |event| matches!(event, Event::Error { operation: "save_project", .. })).await;
            assert_eq!(detail(&mut client, project, flow_id).await, observed, "failed save cannot change the visible original or latest response");
            writer.execute_batch("ROLLBACK").unwrap();
        }
        save(&mut client, project, None).await;
        assert_eq!(detail(&mut client, project, flow_id).await, observed);
        save(&mut client, project, Some(copy.clone())).await;
        client.commands.send(Command::NewProject { path: None }).await.unwrap();
        opened(&mut client).await;
        for path in [source, copy] {
            client.commands.send(Command::OpenProject { path }).await.unwrap();
            let (reopened, reopened_workspace) = opened_with_workspace(&mut client).await;
            assert_eq!(reopened.id, project.id);
            assert!(reopened.generation > project.generation);
            assert_eq!(reopened_workspace, workspace, "confirmed response rules survive source and Save As reopen");
            assert_eq!(detail(&mut client, reopened, flow_id).await, observed, "save and save-as must preserve the same latest and original messages after reopen");
            client.commands.send(Command::QueryHistory { project: reopened, query_revision: 2, offset: 0, limit: 2, filter: String::new() }).await.unwrap();
            let Event::HistoryPage { total: reopened_total, .. } = event(&mut client, |event| matches!(event, Event::HistoryPage { query_revision: 2, .. })).await else { unreachable!() };
            assert_eq!(reopened_total, total);
        }
        tokio::task::spawn_blocking(move || runtime.shutdown()).await.unwrap().unwrap();
        assert_eq!(listener.into_std().unwrap().accept().unwrap_err().kind(), ErrorKind::WouldBlock, "saving and reopening never resend the request");
        let expected_body = match end { End::Stop => FIRST.to_vec(), End::SuppressBody => Vec::new(), _ => [FIRST, SECOND].concat() };
        assert_eq!(observed.0.response.as_ref().unwrap().body, expected_body);
        assert_eq!(observed.0.summary.response_bytes, expected_body.len() as u64);
        assert_eq!(observed.0.summary.state, if end == End::Stop { FlowState::Cancelled } else { FlowState::Complete });
        observed
    }).await.expect("first event, terminal capture, save, reopen and cleanup have one finite bound")
}

#[tokio::test]
async fn an_unedited_sse_does_not_acquire_a_fake_original_when_body_arrives() {
    let (_, original) = observe(false, End::Complete).await;
    assert!(original.request.is_none());
    assert_eq!(original.response_body_complete, None);
    assert!(
        original.response.is_none(),
        "streaming body arrival is not a user edit"
    );
}

#[tokio::test]
async fn sse_header_edits_preserve_the_original_header_and_complete_captured_body() {
    let (record, original) = observe(true, End::Complete).await;
    assert_eq!(original.response_body_complete, Some(true));
    let original = original.response.expect("the upstream response was edited");
    assert_eq!(stream_header(&original), b"upstream");
    assert_eq!(stream_header(record.response.as_ref().unwrap()), b"edited");
    assert_eq!(
        original.body,
        [FIRST, SECOND].concat(),
        "the original response cannot freeze at its empty head-only snapshot"
    );
}

#[tokio::test]
async fn stopping_an_edited_sse_preserves_the_observed_prefix_in_both_versions() {
    let (_, original) = observe(true, End::Stop).await;
    assert_eq!(original.response_body_complete, Some(false));
    let original = original.response.expect("the upstream response was edited");
    assert_eq!(stream_header(&original), b"upstream");
    assert_eq!(
        original.body, FIRST,
        "Stop preserves the same observed prefix in the original response"
    );
}

#[tokio::test]
async fn failed_sse_terminal_writes_keep_original_body_through_recovery_and_save_as() {
    let (_, original) = observe(true, End::WriteFault).await;
    assert_eq!(original.response_body_complete, Some(true));
    let original = original.response.expect("the upstream response was edited");
    assert_eq!(stream_header(&original), b"upstream");
    assert_eq!(
        original.body,
        [FIRST, SECOND].concat(),
        "recovery must distinguish body arrival from edits exactly as durable capture does"
    );
}

#[tokio::test]
async fn suppressing_sse_body_does_not_claim_the_unread_original_is_complete() {
    let (record, original) = observe(true, End::SuppressBody).await;
    assert_eq!(record.response.as_ref().unwrap().status, 204);
    assert_eq!(original.response_body_complete, Some(false));
    let original = original.response.expect("the upstream response was edited");
    assert_eq!(original.status, 200);
    assert!(original.body.is_empty());
}

#[tokio::test]
async fn sse_response_rules_saved_while_listening_hold_only_the_next_headers_and_survive_reopen() {
    let (record, original) = observe(true, End::EnableAfterUnintercepted).await;
    assert_eq!(record.summary.state, FlowState::Complete);
    assert_eq!(stream_header(record.response.as_ref().unwrap()), b"edited");
    assert_eq!(original.response_body_complete, Some(true));
    let original = original.response.unwrap();
    assert_eq!(stream_header(&original), b"upstream");
    assert_eq!(original.body, [FIRST, SECOND].concat());
}

#[tokio::test]
async fn sse_response_body_negative_match_fails_before_unread_body_and_preserves_source_head() {
    tokio::time::timeout(DEADLINE, async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (sent, source_head_sent) = oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, peer) = listener.accept().await.unwrap();
            assert!(peer.ip().is_loopback());
            assert!(head(&mut stream).await.starts_with(b"GET /sse-body-guard HTTP/1.1\r\n"));
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Encoding: gzip\r\nX-Stream: upstream\r\nX-Duplicate: one\r\nX-Duplicate: two\r\nConnection: close\r\n\r\n").await.unwrap();
            sent.send(()).unwrap();
            assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0, "the invalid body condition must close the source while its body is still withheld");
            listener
        });
        let (runtime, mut client) = Runtime::start().unwrap();
        let (project, mut workspace) = opened_with_workspace(&mut client).await;
        assert!(workspace.proxy_messages.unpack_compressed_responses);
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("sse-body-guard.mio");
        let copy = directory.path().join("sse-body-guard-copy.mio");
        workspace.revision += 1;
        workspace.proxy_interception.response.enabled = true;
        workspace.proxy_interception.response.rules = vec![ProxyInterceptRule {
            id: Uuid::new_v4(),
            enabled: true,
            operator: ProxyInterceptOperator::And,
            match_type: ProxyInterceptType::Body,
            relationship: ProxyInterceptRelation::ResponseDoesNotMatch,
            condition: "not-present".into(),
        }];
        client.commands.send(Command::SaveWorkspace { project, workspace: workspace.clone() }).await.unwrap();
        let Event::WorkspaceChanged { workspace: acknowledged, .. } = event(&mut client, |event| matches!(event,
            Event::WorkspaceChanged { project: owner, workspace: value } if *owner == project && value.revision == workspace.revision
        )).await else { unreachable!() };
        assert_eq!(acknowledged, workspace);
        save(&mut client, project, Some(source.clone())).await;
        client.commands.send(Command::SetIntercept { project, requests: false, responses: true }).await.unwrap();
        client.commands.send(Command::StartProxy { project, listen: "127.0.0.1:0".parse().unwrap() }).await.unwrap();
        let Event::ProxyState { address: Some(proxy), .. } = event(&mut client, |event| matches!(event, Event::ProxyState { address: Some(_), .. })).await else { unreachable!() };
        let mut downstream = TcpStream::connect(proxy).await.unwrap();
        downstream.write_all(format!("GET http://{address}/sse-body-guard HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        source_head_sent.await.unwrap();
        let mut wire = Vec::new();
        downstream.read_to_end(&mut wire).await.unwrap();
        assert!(wire.starts_with(b"HTTP/1.1 400"));
        assert!(String::from_utf8_lossy(&wire).contains("SSE 响应正文尚未完整接收"));
        let listener = origin.await.unwrap();
        client.commands.send(Command::StopProxy { project }).await.unwrap();
        event(&mut client, |event| {
            assert!(!matches!(event, Event::InterceptPending { .. }), "an unavailable response body cannot become a true negative match or a held empty message");
            matches!(event, Event::ProxyState { address: None, .. })
        }).await;
        assert!(TcpStream::connect(proxy).await.is_err());
        client.commands.send(Command::QueryHistory { project, query_revision: 31, offset: 0, limit: 1, filter: String::new() }).await.unwrap();
        let Event::HistoryPage { total, rows, .. } = event(&mut client, |event| matches!(event, Event::HistoryPage { query_revision: 31, .. })).await else { unreachable!() };
        assert_eq!(total, 1);
        let [mio_core::HistoryRow::Live(summary)] = rows.as_slice() else { panic!("the actual SSE must remain in history") };
        let observed = detail(&mut client, project, summary.id).await;
        let FlowState::Failed(message) = &observed.0.summary.state else { panic!("an unavailable SSE body cannot be passed successfully") };
        assert!(message.contains("SSE 响应正文尚未完整接收"));
        assert_eq!(observed.0.summary.status, Some(200));
        assert_eq!(observed.0.summary.response_bytes, 0);
        let response = observed.0.response.as_ref().unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(stream_header(response), b"upstream");
        assert!(response.body.is_empty() && response.trailers.is_empty());
        assert_eq!(response.headers.iter().filter(|header| header.name.eq_ignore_ascii_case("content-encoding")).map(|header| header.value.as_slice()).collect::<Vec<_>>(), [b"gzip"]);
        assert_eq!(response.headers.iter().filter(|header| header.name.eq_ignore_ascii_case("x-duplicate")).map(|header| header.value.as_slice()).collect::<Vec<_>>(), [b"one", b"two"]);
        assert_eq!(observed.1, OriginalMessages::default());
        save(&mut client, project, None).await;
        save(&mut client, project, Some(copy.clone())).await;
        client.commands.send(Command::NewProject { path: None }).await.unwrap();
        opened(&mut client).await;
        for path in [&source, &copy] {
            client.commands.send(Command::OpenProject { path: path.clone() }).await.unwrap();
            let (reopened, restored) = opened_with_workspace(&mut client).await;
            assert_eq!(reopened.id, project.id);
            assert!(reopened.generation > project.generation);
            assert_eq!(restored, workspace);
            assert_eq!(detail(&mut client, reopened, summary.id).await, observed);
        }
        tokio::task::spawn_blocking(move || runtime.shutdown()).await.unwrap().unwrap();
        for path in [source, copy] {
            let connection = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
            let saved_head: Vec<u8> = connection.query_row("SELECT head FROM messages WHERE flow=?1 AND side=1", [summary.id.to_string()], |row| row.get(0)).unwrap();
            let saved: serde_json::Value = serde_json::from_slice(&saved_head).unwrap();
            assert_eq!(saved["stream"]["body_complete"], false, "the source body EOS was never observed");
            assert_eq!(saved["stream"]["original_headers"], serde_json::to_value(&response.headers).unwrap());
        }
        assert_eq!(listener.into_std().unwrap().accept().unwrap_err().kind(), ErrorKind::WouldBlock, "failed rules, Save As and reopen never replay the unknown request");
    }).await.expect("SSE head-only negative-match error, both socket EOFs, raw head persistence and two reopens remain within eight seconds");
}
