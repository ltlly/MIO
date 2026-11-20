use std::time::Duration;

use mio_core::{
    Client, Command, Event, FlowState, ProjectKey, Runtime, WebSocketDirection,
    WebSocketHistoryFilter, WebSocketKind, WebSocketMessage, WebSocketSummary,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

async fn event(client: &mut Client, matches: impl Fn(&Event) -> bool) -> Event {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let event = client.events.recv().await.expect("service connected");
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
    .expect("bounded causal event")
}

async fn opened(client: &mut Client) -> ProjectKey {
    let Event::ProjectOpened { project, .. } =
        event(client, |event| matches!(event, Event::ProjectOpened { .. })).await
    else {
        unreachable!()
    };
    project
}

async fn head(stream: &mut TcpStream) -> Vec<u8> {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        assert!(head.len() < 8192, "bounded fixture header");
        head.push(stream.read_u8().await.unwrap());
    }
    head
}

async fn write_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8], masked: bool) {
    assert!(payload.len() < 4096);
    let mut frame = vec![0x80 | opcode];
    let mask_bit = if masked { 0x80 } else { 0 };
    if payload.len() < 126 {
        frame.push(mask_bit | payload.len() as u8);
    } else {
        frame.push(mask_bit | 126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    }
    let mask = [0x12, 0x34, 0x56, 0x78];
    if masked {
        frame.extend_from_slice(&mask);
    }
    frame.extend(payload.iter().enumerate().map(|(index, byte)| {
        if masked {
            byte ^ mask[index % 4]
        } else {
            *byte
        }
    }));
    stream.write_all(&frame).await.unwrap();
}

async fn read_frame(stream: &mut TcpStream, masked: bool) -> (u8, Vec<u8>) {
    let opcode = stream.read_u8().await.unwrap();
    assert_eq!(
        opcode & 0xf0,
        0x80,
        "fixture expects one uncompressed FIN frame"
    );
    let length = stream.read_u8().await.unwrap();
    assert_eq!(length & 0x80 != 0, masked);
    let length = match length & 0x7f {
        126 => stream.read_u16().await.unwrap() as usize,
        127 => panic!("fixture exceeds its bounded size"),
        length => length as usize,
    };
    assert!(length < 4096);
    let mut mask = [0; 4];
    if masked {
        stream.read_exact(&mut mask).await.unwrap();
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await.unwrap();
    if masked {
        for (index, byte) in body.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    (opcode & 0x0f, body)
}

async fn page(client: &mut Client, project: ProjectKey) -> Vec<WebSocketSummary> {
    client
        .commands
        .send(Command::QueryWebSocketHistory {
            selected_message_id: None,
            filter: Default::default(),
            project,
            query_revision: 7,
            flow_id: None,
            offset: 0,
            limit: 200,
        })
        .await
        .unwrap();
    let Event::WebSocketHistoryPage {
        project: actual,
        query_revision: 7,
        filter,
        total,
        rows,
        selected_match,
        ..
    } = event(client, |event| {
        matches!(event, Event::WebSocketHistoryPage { .. })
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(actual, project);
    assert_eq!(filter, WebSocketHistoryFilter::default());
    assert_eq!(selected_match, None);
    assert_eq!(total, 2);
    assert_eq!(rows.len(), 2);
    rows
}

async fn details(
    client: &mut Client,
    project: ProjectKey,
    rows: &[WebSocketSummary],
) -> Vec<WebSocketMessage> {
    let mut records = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let selection_revision = index as u64 + 1;
        client
            .commands
            .send(Command::ReadWebSocket {
                project,
                selection_revision,
                message_id: row.id,
            })
            .await
            .unwrap();
        let Event::WebSocketDetail {
            project: actual,
            selection_revision: actual_selection,
            record,
            original_payload,
        } = event(client, |event| {
            matches!(event, Event::WebSocketDetail { .. })
        })
        .await
        else {
            unreachable!()
        };
        assert_eq!((actual, actual_selection), (project, selection_revision));
        assert_eq!(record.id, row.id);
        assert_eq!(record.payload.len() as u64, row.payload_bytes);
        assert!(original_payload.is_none());
        records.push(*record);
    }
    records
}

#[tokio::test]
async fn real_websocket_capture_stop_save_and_reopen_keeps_both_directions() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (received, request_received) = oneshot::channel();
        let origin = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = head(&mut stream).await;
            assert!(request.starts_with(b"GET /socket HTTP/1.1\r\n"));
            assert!(request.windows(24).any(|bytes| bytes == b"dGhlIHNhbXBsZSBub25jZQ=="));
            stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\nContent-Encoding: gzip\r\n\r\n").await.unwrap();
            write_frame(&mut stream, 2, &(0..=255).collect::<Vec<u8>>(), false).await;
            let (opcode, body) = read_frame(&mut stream, true).await;
            assert_eq!(opcode, 1);
            assert_eq!(body, "最新中文🙂".as_bytes());
            received.send(()).unwrap();
            assert_eq!(stream.read(&mut [0]).await.unwrap(), 0, "Stop must close the origin leg");
        });
        let (runtime, mut client) = Runtime::start().unwrap();
        let Event::ProjectOpened { project, workspace, .. } = event(&mut client, |event| matches!(event, Event::ProjectOpened { .. })).await else { unreachable!() };
        assert!(!workspace.proxy_messages.unpack_compressed_requests);
        assert!(workspace.proxy_messages.unpack_compressed_responses);
        client.commands.send(Command::StartProxy {
            project,
            listen: "127.0.0.1:0".parse().unwrap(),
        }).await.unwrap();
        let Event::ProxyState { address: Some(proxy), .. } = event(&mut client, |event| matches!(event, Event::ProxyState { address: Some(_), .. })).await else { unreachable!() };
        let mut stream = TcpStream::connect(proxy).await.unwrap();
        stream.write_all(format!("GET http://{address}/socket HTTP/1.1\r\nHost: {address}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").as_bytes()).await.unwrap();
        let response_head = head(&mut stream).await;
        assert!(response_head.starts_with(b"HTTP/1.1 101"));
        assert!(String::from_utf8(response_head).unwrap().to_ascii_lowercase().contains("\r\ncontent-encoding: gzip\r\n"));
        let (opcode, body) = read_frame(&mut stream, false).await;
        assert_eq!(opcode, 2);
        assert_eq!(body, (0..=255).collect::<Vec<u8>>());
        write_frame(&mut stream, 1, "最新中文🙂".as_bytes(), true).await;
        request_received.await.unwrap();
        client.commands.send(Command::StopProxy { project }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::ProxyState { address: None, .. })).await;
        assert_eq!(stream.read(&mut [0]).await.unwrap(), 0, "Stop must close the client leg");
        origin.await.unwrap();

        let rows = page(&mut client, project).await;
        assert_eq!(rows[0].flow_id, rows[1].flow_id);
        assert_eq!((rows[0].sequence, rows[1].sequence), (1, 2));
        assert_eq!((rows[0].ordinal, rows[1].ordinal), (1, 2));
        assert!(rows.iter().all(|row| row.connection_id == 1 && row.listener_port == Some(proxy.port())));
        assert!(rows.iter().all(|row| row.url == format!("http://{address}/socket")));
        let records = details(&mut client, project, &rows).await;
        assert_eq!((records[0].direction, records[0].kind), (WebSocketDirection::ServerToClient, WebSocketKind::Binary));
        assert_eq!(records[0].payload, (0..=255).collect::<Vec<u8>>());
        assert_eq!((records[1].direction, records[1].kind), (WebSocketDirection::ClientToServer, WebSocketKind::Text));
        assert_eq!(records[1].payload, "最新中文🙂".as_bytes());
        let filter = WebSocketHistoryFilter { hide_incoming: true, listener_port: Some(proxy.port()), term: r"\xE6\x9C\x80".into(), regex: true, ..Default::default() };
        client.commands.send(Command::QueryWebSocketHistory {
            selected_message_id: Some(rows[1].id),
            project, query_revision: 8, filter: filter.clone(), flow_id: None, offset: 0, limit: 200,
        }).await.unwrap();
        let filtered = event(&mut client, |event| matches!(event, Event::WebSocketHistoryPage { query_revision: 8, .. })).await;
        assert!(matches!(filtered, Event::WebSocketHistoryPage { filter: actual, total: 1, rows: selected, selected_match, .. }
            if actual == filter && selected == [rows[1].clone()] && selected_match == Some((rows[1].id, true))));
        client.commands.send(Command::ReadFlow { project, selection_revision: 9, flow_id: rows[0].flow_id }).await.unwrap();
        let Event::FlowDetail { record, original, .. } = event(&mut client, |event| matches!(event, Event::FlowDetail { .. })).await else { unreachable!() };
        assert_eq!(record.summary.status, Some(101));
        assert_eq!(record.summary.state, FlowState::Cancelled);
        let response = record.response.as_ref().unwrap();
        assert_eq!(response.status, 101);
        assert!(response.body.is_empty() && response.trailers.is_empty());
        assert_eq!(response.headers.iter().filter(|header| header.name.eq_ignore_ascii_case("content-encoding")).map(|header| header.value.as_slice()).collect::<Vec<_>>(), [b"gzip"]);
        assert_eq!(*original, mio_core::OriginalMessages::default(), "no-body 101 cannot fabricate a decompressed response version");

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("websocket.mio");
        client.commands.send(Command::SaveProject { websocket_notes: Vec::new(), ext_http_notes: Vec::new(), ext_http_highlights: Vec::new(), project, path: Some(path.clone()), workspace: None }).await.unwrap();
        event(&mut client, |event| matches!(event, Event::ProjectSaved { .. })).await;
        client.commands.send(Command::NewProject { path: None }).await.unwrap();
        assert_ne!(opened(&mut client).await, project);
        client.commands.send(Command::OpenProject { path }).await.unwrap();
        let reopened = opened(&mut client).await;
        assert_ne!(reopened, project);
        assert_eq!(page(&mut client, reopened).await, rows);
        assert_eq!(details(&mut client, reopened, &rows).await, records);
        client.commands.send(Command::ReadFlow { project: reopened, selection_revision: 10, flow_id: rows[0].flow_id }).await.unwrap();
        let Event::FlowDetail { record: restored, original: restored_original, .. } = event(&mut client, |event| matches!(event, Event::FlowDetail { selection_revision: 10, .. })).await else { unreachable!() };
        assert_eq!(restored, record);
        assert_eq!(restored_original, original);
        client.commands.send(Command::ReadWebSocket { project, selection_revision: 99, message_id: rows[0].id }).await.unwrap();
        let stale = event(&mut client, |event| matches!(event, Event::WebSocketDetailError { .. })).await;
        assert!(matches!(stale, Event::WebSocketDetailError { project: actual, selection_revision: 99, message_id, .. } if actual == project && message_id == rows[0].id));
        runtime.shutdown().unwrap();
    })
    .await
    .expect("bounded end-to-end WebSocket product workflow");
}
