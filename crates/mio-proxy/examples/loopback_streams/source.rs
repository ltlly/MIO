use super::*;
use rama::{
    futures::{SinkExt, StreamExt},
    http::ws::{
        AsyncWebSocket, Message, ProtocolError,
        protocol::{Role, WebSocketConfig},
    },
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

async fn chunk(stream: &mut TcpStream, bytes: &[u8]) -> Result<()> {
    stream
        .write_all(format!("{:x}\r\n", bytes.len()).as_bytes())
        .await?;
    stream.write_all(bytes).await?;
    stream.write_all(b"\r\n").await?;
    Ok(())
}

async fn source_sse(mut stream: TcpStream, source: &Source) -> Result<()> {
    let (steps, mut receiver) = mpsc::channel(2);
    let accepted = {
        let mut active = source.active_sse.lock().unwrap();
        if active.as_ref().is_some_and(|sender| !sender.is_closed()) {
            false
        } else {
            *active = Some(steps);
            true
        }
    };
    if !accepted {
        stream
            .write_all(b"HTTP/1.1 409 Conflict\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await?;
        return Ok(());
    }
    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await?;
    chunk(&mut stream, FIRST).await?;
    println!("SSE_SOURCE bytes={} first=true", FIRST.len());
    let mut second = false;
    let mut probe = [0; 1];
    let complete = loop {
        tokio::select! {
            biased;
            closed = stream.read(&mut probe) => {
                if matches!(closed, Ok(1..)) { return Err(invalid("fixture does not accept pipelining").into()); }
                break false;
            },
            step = receiver.recv() => match step {
                Some(Step::Next) if !second => {
                    chunk(&mut stream, SECOND).await?;
                    second = true;
                    println!("SSE_SOURCE bytes={} second=true", FIRST.len() + SECOND.len());
                },
                Some(Step::Next) => {},
                Some(Step::Eof) => { stream.write_all(b"0\r\n\r\n").await?; break true; },
                None => break false,
            },
        }
    };
    println!(
        "SSE_SOURCE closed=true complete={complete} bytes={}",
        FIRST.len() + usize::from(second) * SECOND.len()
    );
    Ok(())
}

pub(super) fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .with_max_message_size(4096)
        .with_max_frame_size(4096)
}

pub(super) fn ws_messages() -> [Message; 2] {
    [
        Message::Text(WS_TEXT.into()),
        Message::Binary(WS_BINARY.to_vec().into()),
    ]
}

pub(super) fn disconnected(message: &Option<std::result::Result<Message, ProtocolError>>) -> bool {
    match message {
        None | Some(Err(ProtocolError::ResetWithoutClosingHandshake)) => true,
        Some(Err(error)) => error.is_connection_error(),
        _ => false,
    }
}

async fn source_ws(mut stream: TcpStream, request: &str) -> Result<()> {
    if header(request, "sec-websocket-key") != Some(KEY)
        || header(request, "sec-websocket-version") != Some("13")
    {
        return Err(invalid("fixture requires its synthetic WS handshake").into());
    }
    stream.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {ACCEPT}\r\n\r\n").as_bytes()).await?;
    let mut socket = AsyncWebSocket::from_raw_socket(
        rama::tcp::TcpStream::new(stream),
        Role::Server,
        Some(ws_config()),
    )
    .await;
    for expected in ws_messages() {
        let received = socket
            .next()
            .await
            .ok_or_else(|| invalid("WS source closed before its message"))??;
        if received != expected {
            return Err(invalid("WS source bytes mismatch").into());
        }
        socket.send(received).await?;
    }
    println!(
        "WS_SOURCE messages=2 bytes={} matches=true",
        WS_TEXT.len() + WS_BINARY.len()
    );
    let ending = socket.next().await;
    let complete = matches!(ending, Some(Ok(Message::Close(_))));
    if complete {
        socket.flush().await?;
    } else if !disconnected(&ending) {
        return Err(invalid("unexpected WS source terminal").into());
    }
    println!("WS_SOURCE closed=true complete={complete}");
    Ok(())
}

async fn read_request_body(stream: &mut TcpStream, request: &str) -> Result<Vec<u8>> {
    if header(request, "transfer-encoding").is_some() {
        return Err(invalid("fixture does not accept chunked request bodies").into());
    }
    let length = match header(request, "content-length") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| invalid("invalid content-length"))?,
        None => 0,
    };
    if length > 64 * 1024 {
        return Err(invalid("fixture request body exceeds 64 KiB").into());
    }
    let mut body = vec![0; length];
    if length > 0 {
        tokio::time::timeout(IO_DEADLINE, stream.read_exact(&mut body)).await??;
    }
    Ok(body)
}

async fn serve_bench(
    stream: &mut TcpStream,
    method: &str,
    target: &str,
    request: &str,
    source: &Source,
) -> Result<bool> {
    let body = read_request_body(stream, request).await?;
    let count = source.bench_requests.fetch_add(1, Ordering::SeqCst) + 1;
    let keep_alive =
        !header(request, "connection").is_some_and(|value| value.eq_ignore_ascii_case("close"));
    let (status, content_type, marker) = if target.contains("kind=script") {
        (
            "200 OK",
            "application/javascript; charset=utf-8",
            "alpha-script",
        )
    } else if target.contains("kind=miss") {
        (
            "404 Not Found",
            "text/html; charset=utf-8",
            "missing-needle",
        )
    } else {
        ("200 OK", "text/html; charset=utf-8", "mio-r5-canary")
    };
    let pad = "x".repeat(count % 64);
    let payload = format!(
        "/* {marker} */ seq={count} method={method} target={target} req_bytes={} pad={pad}\n",
        body.len()
    );
    let connection = if keep_alive { "keep-alive" } else { "close" };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nX-Fixture: one\r\nX-Fixture: two\r\nContent-Length: {}\r\nConnection: {connection}\r\n\r\n{payload}",
        payload.len()
    );
    stream.write_all(response.as_bytes()).await?;
    Ok(keep_alive)
}

async fn serve_hold(mut stream: TcpStream, request: &str, source: &Source) -> Result<()> {
    let _body = read_request_body(&mut stream, request).await?;
    source.hold_requests.fetch_add(1, Ordering::SeqCst);
    source.hold_active.fetch_add(1, Ordering::SeqCst);
    struct ActiveGuard<'a>(&'a Source);
    impl Drop for ActiveGuard<'_> {
        fn drop(&mut self) {
            self.0.hold_active.fetch_sub(1, Ordering::SeqCst);
            self.0.hold_closed.fetch_add(1, Ordering::SeqCst);
        }
    }
    let _guard = ActiveGuard(source);
    let mut probe = [0; 1];
    tokio::select! {
        biased;
        _ = source.cancel.cancelled() => {}
        read = stream.read(&mut probe) => {
            if matches!(read, Ok(1..)) {
                return Err(invalid("hold endpoint expects client/proxy EOF without pipelining").into());
            }
        }
    }
    Ok(())
}

async fn serve(mut stream: TcpStream, source: Arc<Source>) -> Result<()> {
    let mut reused = false;
    loop {
        let request = match head(&mut stream).await {
            Ok(request) => request,
            Err(_) if reused => return Ok(()),
            Err(error) => return Err(error),
        };
        let line = request
            .lines()
            .next()
            .ok_or_else(|| invalid("missing request"))?;
        let mut parts = line.split_whitespace();
        let method = parts.next().ok_or_else(|| invalid("missing method"))?;
        let target = parts
            .next()
            .ok_or_else(|| invalid("missing fixture path"))?;
        let path = target.split('?').next().unwrap_or(target);
        if (path == "/bench" || path.starts_with("/bench/")) && matches!(method, "GET" | "POST") {
            if !serve_bench(&mut stream, method, target, &request, &source).await? {
                return Ok(());
            }
            reused = true;
            continue;
        }
        if path == "/hold" && matches!(method, "GET" | "POST") {
            return serve_hold(stream, &request, &source).await;
        }
        if method != "GET"
            || header(&request, "transfer-encoding").is_some()
            || header(&request, "content-length").is_some_and(|value| value != "0")
        {
            return Err(invalid("fixture accepts only empty GET requests").into());
        }
        if path == "/stats" {
            let body = format!(
                "{{\"bench_requests\":{},\"hold_requests\":{},\"hold_active\":{},\"hold_closed\":{}}}\n",
                source.bench_requests.load(Ordering::SeqCst),
                source.hold_requests.load(Ordering::SeqCst),
                source.hold_active.load(Ordering::SeqCst),
                source.hold_closed.load(Ordering::SeqCst),
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await?;
            return Ok(());
        }
        match path {
            "/events" => return source_sse(stream, &source).await,
            "/socket" => return source_ws(stream, &request).await,
            "/next" | "/eof" => {
                let accepted = source
                    .active_sse
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|sender| {
                        sender
                            .try_send(if path == "/next" {
                                Step::Next
                            } else {
                                Step::Eof
                            })
                            .is_ok()
                    });
                if !accepted {
                    stream
                        .write_all(
                            b"HTTP/1.1 409 Conflict\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await?;
                    return Ok(());
                }
            }
            "/shutdown" => {}
            _ => return Err(invalid("unknown fixture endpoint").into()),
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await?;
        if path == "/shutdown" {
            source.cancel.cancel();
        }
        return Ok(());
    }
}

pub(super) async fn start_source() -> Result<(SocketAddr, CancellationToken, JoinHandle<Result<()>>)>
{
    start_source_with_limit(16, true).await
}

pub(super) async fn start_source_with_limit(
    max_connections: usize,
    verbose: bool,
) -> Result<(SocketAddr, CancellationToken, JoinHandle<Result<()>>)> {
    if !(1..=256).contains(&max_connections) {
        return Err(invalid("max_connections must be between 1 and 256").into());
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let address = listener.local_addr()?;
    let source = Arc::new(Source::default());
    let cancel = source.cancel.clone();
    let task = tokio::spawn(async move {
        let mut tasks = JoinSet::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
        let result = async {
            loop {
                tokio::select! {
                    biased;
                    _ = source.cancel.cancelled() => break,
                    _ = tokio::time::sleep_until(deadline) => break,
                    Some(result) = tasks.join_next(), if !tasks.is_empty() => { result?; },
                    accepted = listener.accept(), if tasks.len() < max_connections => {
                        let (stream, _) = accepted?;
                        stream.set_nodelay(true)?;
                        let source = source.clone();
                        tasks.spawn(async move {
                            tokio::select! {
                                _ = source.cancel.cancelled() => {},
                                result = tokio::time::timeout(FLOW_DEADLINE, serve(stream, source.clone())) => {
                                    if verbose {
                                        println!("SOURCE_CONNECTION success={}", matches!(result, Ok(Ok(()))));
                                    }
                                },
                            }
                        });
                    },
                }
            }
            Ok::<(), Box<dyn Error + Send + Sync>>(())
        }
        .await;
        drop(listener);
        source.cancel.cancel();
        let joined = tokio::time::timeout(IO_DEADLINE, async {
            while let Some(result) = tasks.join_next().await {
                result?;
            }
            Ok::<_, tokio::task::JoinError>(())
        })
        .await;
        if joined.is_err() {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        }
        joined??;
        result?;
        println!("SOURCE_STOPPED joined=true");
        Ok(())
    });
    Ok((address, cancel, task))
}

pub(super) async fn control(origin: SocketAddr, command: &str) -> Result<()> {
    let mut stream = connect(origin).await?;
    stream
        .write_all(
            format!("GET /{command} HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await?;
    if !head(&mut stream).await?.starts_with("HTTP/1.1 200 ") {
        return Err(invalid("source rejected the control command").into());
    }
    println!("CONTROL accepted=true");
    Ok(())
}

pub(super) async fn read_stats(origin: SocketAddr) -> Result<(String, usize)> {
    let mut stream = connect(origin).await?;
    stream
        .write_all(
            format!("GET /stats HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await?;
    let resp_head = head(&mut stream).await?;
    if !resp_head.starts_with("HTTP/1.1 200 ") {
        return Err(invalid("source rejected stats request").into());
    }
    let len = header(&resp_head, "content-length")
        .ok_or_else(|| invalid("missing stats content-length"))?
        .parse::<usize>()
        .map_err(|_| invalid("invalid stats content-length"))?;
    if len > 4096 {
        return Err(invalid("stats body exceeds limit").into());
    }
    let mut body = vec![0; len];
    stream.read_exact(&mut body).await?;
    let json = String::from_utf8(body)?;
    let bench_requests = json
        .split("\"bench_requests\":")
        .nth(1)
        .and_then(|rest| rest.split([',', '}']).next())
        .ok_or_else(|| invalid("missing bench_requests in stats"))?
        .trim()
        .parse::<usize>()
        .map_err(|_| invalid("invalid bench_requests count"))?;
    Ok((json, bench_requests))
}
