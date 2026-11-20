use super::source::{disconnected, ws_config, ws_messages};
use super::*;
use mio_plugin_api::{ProjectKey, Uuid};
use mio_proxy::{MitmAuthority, ProxyConfig, ProxyEvent};
use rama::{
    futures::{SinkExt, StreamExt},
    http::ws::{AsyncWebSocket, Message, protocol::Role},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::{mpsc, oneshot},
    task::JoinSet,
};

async fn read_chunk(stream: &mut TcpStream) -> io::Result<Option<Vec<u8>>> {
    let line = read_until(stream, b"\r\n", 64).await?;
    let text = std::str::from_utf8(&line).map_err(|_| invalid("invalid chunk length"))?;
    let length = usize::from_str_radix(text.trim().split(';').next().unwrap_or(""), 16)
        .map_err(|_| invalid("invalid chunk length"))?;
    if length > 4096 {
        return Err(invalid("fixture chunk too large"));
    }
    if length == 0 {
        if read_until(stream, b"\r\n", 8192).await? != b"\r\n" {
            return Err(invalid("fixture does not emit trailers"));
        }
        return Ok(None);
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await?;
    let mut ending = [0; 2];
    stream.read_exact(&mut ending).await?;
    if ending != *b"\r\n" {
        return Err(invalid("invalid chunk delimiter"));
    }
    Ok(Some(bytes))
}

pub(super) async fn client_sse(
    proxy: SocketAddr,
    origin: SocketAddr,
    expect_stop: bool,
    mut first: Option<oneshot::Sender<()>>,
) -> Result<()> {
    let mut stream = connect(proxy).await?;
    stream
        .write_all(
            format!(
                "GET http://{origin}/events HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await?;
    let response = head(&mut stream).await?;
    if !response.starts_with("HTTP/1.1 200 ")
        || header(&response, "transfer-encoding") != Some("chunked")
    {
        return Err(invalid("expected the synthetic streaming response").into());
    }
    println!(
        "SSE_CLIENT head_ok=true edited={}",
        header(&response, "x-fixture-edit").is_some()
    );
    let expected = [FIRST, SECOND].concat();
    let mut bytes = Vec::new();
    let mut first_seen = false;
    let interrupted = loop {
        match read_chunk(&mut stream).await {
            Ok(Some(chunk)) => {
                bytes.extend(chunk);
                if !expected.starts_with(&bytes) {
                    return Err(invalid("SSE bytes mismatch").into());
                }
                if !first_seen && bytes.len() >= FIRST.len() {
                    first_seen = true;
                    println!("SSE_CLIENT first=true bytes={}", FIRST.len());
                    if let Some(first) = first.take() {
                        let _ = first.send(());
                    }
                }
                if bytes.len() == expected.len() {
                    println!("SSE_CLIENT second=true bytes={}", bytes.len());
                }
            }
            Ok(None) => break false,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionAborted
                ) =>
            {
                break true;
            }
            Err(error) => return Err(error.into()),
        }
    };
    if interrupted != expect_stop || bytes.as_slice() != if expect_stop { FIRST } else { &expected }
    {
        return Err(invalid("SSE terminal or complete bytes mismatch").into());
    }
    println!(
        "SSE_CLIENT complete={} interrupted={interrupted} bytes={} matches=true",
        !interrupted,
        bytes.len()
    );
    Ok(())
}

pub(super) async fn client_ws(
    proxy: SocketAddr,
    origin: SocketAddr,
    expect_stop: bool,
    ready: Option<oneshot::Sender<()>>,
) -> Result<()> {
    let mut stream = connect(proxy).await?;
    stream.write_all(format!("GET http://{origin}/socket HTTP/1.1\r\nHost: {origin}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: {KEY}\r\nSec-WebSocket-Version: 13\r\n\r\n").as_bytes()).await?;
    let response = head(&mut stream).await?;
    if !response.starts_with("HTTP/1.1 101 ")
        || header(&response, "sec-websocket-accept") != Some(ACCEPT)
    {
        return Err(invalid("WS response handshake mismatch").into());
    }
    let mut socket = AsyncWebSocket::from_raw_socket(
        rama::tcp::TcpStream::new(stream),
        Role::Client,
        Some(ws_config()),
    )
    .await;
    for expected in ws_messages() {
        socket.send(expected.clone()).await?;
        let received = socket
            .next()
            .await
            .ok_or_else(|| invalid("WS echo missing"))??;
        if received != expected {
            return Err(invalid("WS echo bytes mismatch").into());
        }
    }
    println!(
        "WS_CLIENT messages=2 bytes={} matches=true",
        WS_TEXT.len() + WS_BINARY.len()
    );
    if let Some(ready) = ready {
        let _ = ready.send(());
    }
    if !expect_stop {
        socket.close(None).await?;
    }
    let ending = socket.next().await;
    if if expect_stop {
        !disconnected(&ending)
    } else {
        !matches!(ending, Some(Ok(Message::Close(_))))
    } {
        return Err(invalid("WS terminal mismatch").into());
    }
    println!("WS_CLIENT closed=true complete={}", !expect_stop);
    Ok(())
}

pub(super) async fn self_test() -> Result<()> {
    let (events, mut captures) = mpsc::channel(64);
    let mut proxy = mio_proxy::start(
        ProxyConfig::new(MitmAuthority::generate()?),
        ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
        events,
    )
    .await?;
    let (origin, source_cancel, source) = match start_source().await {
        Ok(source) => source,
        Err(error) => {
            proxy.stop().await?;
            return Err(error);
        }
    };
    let _source_cancel_on_drop = source_cancel.clone().drop_guard();
    let address = proxy.address();
    let mut clients = JoinSet::new();
    let checked = tokio::time::timeout(Duration::from_secs(10), async {
        let (first, received) = oneshot::channel();
        clients.spawn(client_sse(address, origin, false, Some(first)));
        received.await?;
        control(origin, "next").await?;
        control(origin, "eof").await?;
        clients.join_next().await.unwrap()??;
        client_ws(address, origin, false, None).await?;
        let (ready, received) = oneshot::channel();
        clients.spawn(client_ws(address, origin, true, Some(ready)));
        received.await?;
        let (first, received) = oneshot::channel();
        clients.spawn(client_sse(address, origin, true, Some(first)));
        received.await?;
        proxy.stop().await?;
        while let Some(client) = clients.join_next().await {
            client??;
        }
        Ok::<(), Box<dyn Error + Send + Sync>>(())
    })
    .await;
    let stopped = proxy.stop().await;
    clients.abort_all();
    while clients.join_next().await.is_some() {}
    let mut sse_complete = false;
    let mut sse_partial = false;
    while let Ok(event) = captures.try_recv() {
        if let ProxyEvent::Captured {
            record,
            response_stream: Some(stream),
            ..
        } = event
            && let Some(response) = record.response
        {
            sse_complete |= stream.body_complete && response.body == [FIRST, SECOND].concat();
            sse_partial |= !stream.body_complete && response.body == FIRST;
        }
    }
    let shutdown = tokio::time::timeout(IO_DEADLINE, control(origin, "shutdown")).await;
    source_cancel.cancel();
    let joined = source.await;
    checked??;
    stopped?;
    shutdown??;
    joined??;
    assert!(
        sse_complete && sse_partial,
        "fixture checks complete and partial proxy capture bytes"
    );
    assert!(
        TcpStream::connect(origin).await.is_err(),
        "source listener must be reclaimed"
    );
    assert!(
        TcpStream::connect(address).await.is_err(),
        "proxy listener must be reclaimed"
    );
    println!(
        "SELF_TEST sse_complete=true sse_partial=true ws_complete=true ws_stop=true joined=true"
    );
    Ok(())
}
