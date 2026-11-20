use super::*;

use std::sync::atomic::Ordering;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

pub(super) async fn wait_event(client: &mut crate::Client, pred: &dyn Fn(&Event) -> bool) -> Event {
    loop {
        let ev = client.events.recv().await.unwrap();
        if pred(&ev) {
            return ev;
        }
    }
}

pub(super) fn compute_ws_accept(key: &str) -> String {
    let mut msg = key.trim().as_bytes().to_vec();
    msg.extend_from_slice(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let bit_len = (msg.len() as u64) * 8;
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    let (mut h0, mut h1, mut h2, mut h3, mut h4) = (
        0x67452301u32,
        0xEFCDAB89u32,
        0x98BADCFEu32,
        0x10325476u32,
        0xC3D2E1F0u32,
    );
    for chunk in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for (i, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }
    let mut digest = [0u8; 20];
    for (dst, val) in digest
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip([h0, h1, h2, h3, h4])
    {
        *dst = val.to_be_bytes();
    }
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(28);
    for chunk in digest.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[((n >> 18) & 63) as usize] as char);
        out.push(B64[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

pub(super) async fn read_http_head(stream: &mut tokio::net::TcpStream) -> String {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(stream.read_u8().await.unwrap());
    }
    String::from_utf8(head).unwrap()
}

pub(super) async fn read_ws_frame(
    stream: &mut tokio::net::TcpStream,
    masked: bool,
) -> (u8, Vec<u8>) {
    let first = stream.read_u8().await.unwrap();
    let second = stream.read_u8().await.unwrap();
    assert_eq!(second & 0x80 != 0, masked);
    let len = (second & 0x7f) as usize;
    let mask = if masked {
        let mut m = [0u8; 4];
        stream.read_exact(&mut m).await.unwrap();
        m
    } else {
        [0u8; 4]
    };
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await.unwrap();
    if masked {
        for (idx, b) in payload.iter_mut().enumerate() {
            *b ^= mask[idx % 4];
        }
    }
    (first & 0x0f, payload)
}

pub(super) async fn write_ws_frame(
    stream: &mut tokio::net::TcpStream,
    opcode: u8,
    payload: &[u8],
    masked: bool,
) {
    let mut frame = vec![0x80 | opcode];
    if masked {
        frame.push(0x80 | (payload.len() as u8));
        let mask = [0x12, 0x34, 0x56, 0x78];
        frame.extend_from_slice(&mask);
        for (idx, b) in payload.iter().enumerate() {
            frame.push(b ^ mask[idx % 4]);
        }
    } else {
        frame.push(payload.len() as u8);
        frame.extend_from_slice(payload);
    }
    stream.write_all(&frame).await.unwrap();
}

pub(super) fn spawn_origin_server(
    listener: TcpListener,
    accepted_connections: &std::sync::Arc<std::sync::atomic::AtomicUsize>,
    c2s_tx: tokio::sync::oneshot::Sender<()>,
) -> JoinHandle<()> {
    let accepted_counter = accepted_connections.clone();
    tokio::spawn(async move {
        // Connection 1: live Proxy WS connection
        let (mut stream1, _) = listener.accept().await.unwrap();
        accepted_counter.fetch_add(1, Ordering::SeqCst);
        let head1 = read_http_head(&mut stream1).await;
        let key1 = head1
            .lines()
            .find_map(|line| {
                let (k, v) = line.split_once(':')?;
                k.eq_ignore_ascii_case("sec-websocket-key")
                    .then_some(v.trim())
            })
            .unwrap();
        let accept1 = compute_ws_accept(key1);
        stream1
            .write_all(
                format!(
                    "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {accept1}\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let (op, first_msg) = read_ws_frame(&mut stream1, true).await;
        assert_eq!((op, first_msg.as_slice()), (1, b"seed-c2s".as_slice()));
        let (op, injected_c2s) = read_ws_frame(&mut stream1, true).await;
        assert_eq!(
            (op, injected_c2s.as_slice()),
            (1, b"injected-c2s".as_slice())
        );
        c2s_tx.send(()).unwrap();
        let mut rest = [0u8; 32];
        let _ = stream1.read(&mut rest).await;
        drop(stream1);

        // Connection 2: standalone reconnect from WebSocket Repeater
        let (mut stream2, _) = listener.accept().await.unwrap();
        accepted_counter.fetch_add(1, Ordering::SeqCst);
        let head2 = read_http_head(&mut stream2).await;
        assert!(
            head2
                .lines()
                .any(|l| l.eq_ignore_ascii_case("cookie: sid=live-cookie"))
        );
        assert!(
            head2
                .lines()
                .any(|l| l.eq_ignore_ascii_case("origin: http://app.local"))
        );
        let key2 = head2
            .lines()
            .find_map(|line| {
                let (k, v) = line.split_once(':')?;
                k.eq_ignore_ascii_case("sec-websocket-key")
                    .then_some(v.trim())
            })
            .unwrap();
        assert_ne!(key2, "dGhlIHNhbXBsZSBub25jZQ==");
        let accept2 = compute_ws_accept(key2);
        stream2
            .write_all(
                format!(
                    "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {accept2}\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let (op, standalone_msg) = read_ws_frame(&mut stream2, true).await;
        assert_eq!(
            (op, standalone_msg.as_slice()),
            (1, b"standalone-ping".as_slice())
        );
        write_ws_frame(&mut stream2, 1, b"standalone-pong", false).await;
        let _ = stream2.read(&mut rest).await;
    })
}

pub(super) async fn assert_quiesce_and_resume_retains_tab(
    client: &mut crate::Client,
    project: ProjectKey,
    tab_id: TabId,
) {
    // Quiesce must emit SessionClosed before Quiesced and retain tab metadata on Resume.
    client
        .commands
        .send(crate::Command::Quiesce { project })
        .await
        .unwrap();
    let mut saw_session_closed = false;
    loop {
        let ev = wait_event(client, &|e| {
            matches!(
                e,
                Event::WebSocketRepeater(WebSocketRepeaterEvent::SessionClosed { .. })
                    | Event::Quiesced { .. }
            )
        })
        .await;
        match ev {
            Event::WebSocketRepeater(WebSocketRepeaterEvent::SessionClosed {
                tab_id: closed_id,
                mode: WebSocketRepeaterConnectionMode::Disconnected,
                ..
            }) if closed_id == tab_id => {
                saw_session_closed = true;
            }
            Event::Quiesced { project: p } if p == project => {
                assert!(
                    saw_session_closed,
                    "Quiesce must flush SessionClosed before Quiesced"
                );
                break;
            }
            _ => {}
        }
    }
    client
        .commands
        .send(crate::Command::Resume { project })
        .await
        .unwrap();
    client
        .commands
        .send(crate::Command::WebSocketRepeater(
            crate::api::WebSocketRepeaterCommand::Send {
                project,
                operation_id: Uuid::new_v4(),
                tab_id,
                url: String::new(),
                source_flow_id: None,
                direction: WebSocketDirection::ClientToServer,
                kind: WebSocketKind::Text,
                payload: b"after-resume".to_vec(),
                draft_revision: 5,
                cancel: CancellationToken::new(),
            },
        ))
        .await
        .unwrap();
    let Event::WebSocketRepeater(WebSocketRepeaterEvent::Sent {
        draft_revision: 5,
        result: Err(resume_err),
        ..
    }) = wait_event(client, &|e| {
        matches!(
            e,
            Event::WebSocketRepeater(WebSocketRepeaterEvent::Sent {
                draft_revision: 5,
                result: Err(_),
                ..
            })
        )
    })
    .await
    else {
        unreachable!()
    };
    assert!(
        !resume_err.contains("标签不存在"),
        "Quiesce + Resume must retain WS Repeater tab state, got: {resume_err}"
    );
}

pub(super) async fn assert_offline_reopen_retains_tab(
    client: &mut crate::Client,
    project: ProjectKey,
    tab_id: TabId,
    workspace: Workspace,
    opened_tab: WebSocketRepeaterTab,
    live_flow_id: FlowId,
    accepted_connections: &std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    // Save workspace + project, then reopen: tab is restored offline (no extra connection).
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let saved_path = dir.path().canonicalize().unwrap().join("ws-repeater.mio");
    let mut next_ws = workspace;
    next_ws.revision += 1;
    let mut persisted_tab = opened_tab;
    persisted_tab.draft_revision = 5;
    persisted_tab.draft_payload = b"standalone-ping".to_vec();
    next_ws.websocket_tabs = vec![persisted_tab];
    next_ws.active_websocket_tab = Some(tab_id);
    client
        .commands
        .send(crate::Command::SaveProject {
            project,
            path: Some(saved_path.clone()),
            workspace: Some(next_ws),
            websocket_notes: Vec::new(),
            ext_http_notes: Vec::new(),
            ext_http_highlights: Vec::new(),
        })
        .await
        .unwrap();
    let _ = wait_event(client, &|e| matches!(e, Event::ProjectSaved { .. })).await;

    client
        .commands
        .send(crate::Command::NewProject { path: None })
        .await
        .unwrap();
    let _ = wait_event(
        client,
        &|e| matches!(e, Event::ProjectOpened { project: p, .. } if *p != project),
    )
    .await;

    client
        .commands
        .send(crate::Command::OpenProject { path: saved_path })
        .await
        .unwrap();
    let Event::ProjectOpened {
        project: reopened_project,
        workspace: reopened_ws,
        ..
    } = wait_event(client, &|e| {
        matches!(
            e,
            Event::ProjectOpened { workspace, .. } if workspace.websocket_tabs.len() == 1
        )
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(reopened_ws.active_websocket_tab, Some(tab_id));
    assert_eq!(accepted_connections.load(Ordering::SeqCst), 2);

    // Sending on reopened tab without explicit Connect must fail offline.
    client
        .commands
        .send(crate::Command::WebSocketRepeater(
            crate::api::WebSocketRepeaterCommand::Send {
                project: reopened_project,
                operation_id: Uuid::new_v4(),
                tab_id,
                url: String::new(),
                source_flow_id: Some(live_flow_id),
                direction: WebSocketDirection::ClientToServer,
                kind: WebSocketKind::Text,
                payload: b"offline-check".to_vec(),
                draft_revision: 6,
                cancel: CancellationToken::new(),
            },
        ))
        .await
        .unwrap();
    let _ = wait_event(client, &|e| {
        matches!(
            e,
            Event::WebSocketRepeater(WebSocketRepeaterEvent::Sent {
                draft_revision: 6,
                mode: WebSocketRepeaterConnectionMode::Disconnected,
                result: Err(_),
                ..
            })
        )
    })
    .await;
    assert_eq!(accepted_connections.load(Ordering::SeqCst), 2);
}
