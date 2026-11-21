use super::*;

#[path = "websocket/frames.rs"]
mod frames;
#[path = "websocket/handshake.rs"]
mod handshake;
#[path = "websocket/intercept.rs"]
mod intercept;
#[path = "websocket/limits.rs"]
mod limits;
#[path = "websocket/routes.rs"]
mod routes;

const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const ACCEPT: &str = "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";
const MAX_FRAME: usize = 4096;

async fn next_websocket_intercept(
    events: &mut mpsc::Receiver<ProxyEvent>,
) -> (
    mio_plugin_api::WebSocketMessage,
    tokio::sync::oneshot::Sender<mio_proxy::WebSocketInterceptDecision>,
) {
    tokio::time::timeout(DEADLINE, async {
        loop {
            if let ProxyEvent::WebSocketIntercepted { record, reply, .. } =
                events.recv().await.unwrap()
            {
                return (record, reply);
            }
        }
    })
    .await
    .unwrap()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Frame {
    fin: bool,
    opcode: u8,
    payload: Vec<u8>,
}

impl Frame {
    fn new(opcode: u8, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            fin: true,
            opcode,
            payload: payload.into(),
        }
    }

    fn encode(&self, masked: bool) -> Vec<u8> {
        assert!(self.payload.len() <= MAX_FRAME);
        let mut bytes = vec![u8::from(self.fin) << 7 | self.opcode];
        let mask_bit = if masked { 0x80 } else { 0 };
        if self.payload.len() < 126 {
            bytes.push(mask_bit | self.payload.len() as u8);
        } else {
            bytes.push(mask_bit | 126);
            bytes.extend_from_slice(&(self.payload.len() as u16).to_be_bytes());
        }
        let mask = [0x17, 0x29, 0x3b, 0x4d];
        if masked {
            bytes.extend_from_slice(&mask);
        }
        for (index, byte) in self.payload.iter().enumerate() {
            bytes.push(if masked {
                byte ^ mask[index % 4]
            } else {
                *byte
            });
        }
        bytes
    }
}

async fn read_head<I: AsyncRead + Unpin>(io: &mut I) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        assert!(bytes.len() < 8192, "fixture HTTP head must remain bounded");
        bytes.push(io.read_u8().await?);
    }
    Ok(bytes)
}

async fn read_frame<I: AsyncRead + Unpin>(io: &mut I, masked: bool) -> std::io::Result<Frame> {
    let first = io.read_u8().await?;
    let second = io.read_u8().await?;
    assert_eq!(first & 0x70, 0, "fixture negotiates no WS extensions");
    assert_eq!(second & 0x80 != 0, masked, "RFC 6455 mask direction");
    let length = match second & 0x7f {
        126 => u64::from(io.read_u16().await?),
        127 => io.read_u64().await?,
        length => u64::from(length),
    };
    assert!(length <= MAX_FRAME as u64, "fixture frame size is bounded");
    let mut mask = [0; 4];
    if masked {
        io.read_exact(&mut mask).await?;
    }
    let mut payload = vec![0; length as usize];
    io.read_exact(&mut payload).await?;
    if masked {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    Ok(Frame {
        fin: first & 0x80 != 0,
        opcode: first & 0x0f,
        payload,
    })
}

#[derive(Debug)]
enum WsEvent {
    Handshake {
        path: String,
        upgrade: bool,
        connection: bool,
        key: bool,
        alpn: Option<Vec<u8>>,
    },
    Frame(Frame),
    TlsRejected,
    Closed,
}

struct WsOrigin {
    address: SocketAddr,
    certificate: Option<Vec<u8>>,
    events: mpsc::Receiver<WsEvent>,
    commands: mpsc::Sender<Frame>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}
impl WsOrigin {
    async fn start(tls: Option<CertificateKind>) -> Self {
        Self::start_with_reply(tls, None).await
    }

    async fn start_with_reply(tls: Option<CertificateKind>, reply: Option<String>) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let (certificate, acceptor) = match tls {
            Some(kind) => {
                let (ca, config) = origin_certificate(kind, false);
                (Some(ca), Some(TlsAcceptor::from(Arc::new(config))))
            }
            None => (None, None),
        };
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let (events_tx, events) = mpsc::channel(16);
        let (commands, receiver) = mpsc::channel(8);
        let task = tokio::spawn(async move {
            tokio::select! {
                _ = task_cancel.cancelled() => {},
                _ = async {
                    let (stream, _) = listener.accept().await.unwrap();
                    if let Some(acceptor) = acceptor {
                        match acceptor.accept(stream).await {
                            Ok(stream) => {
                                let alpn = stream.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
                                serve_ws_origin(stream, alpn, &events_tx, receiver, reply.as_deref()).await;
                            }
                            Err(_) => { events_tx.send(WsEvent::TlsRejected).await.unwrap(); }
                        }
                    } else {
                        serve_ws_origin(stream, None, &events_tx, receiver, reply.as_deref()).await;
                    }
                    let _ = events_tx.send(WsEvent::Closed).await;
                } => {},
            }
        });
        Self {
            address,
            certificate,
            events,
            commands,
            cancel,
            task,
        }
    }

    fn url(&self) -> String {
        format!(
            "{}://localhost:{}/websocket",
            if self.certificate.is_some() {
                "https"
            } else {
                "http"
            },
            self.address.port()
        )
    }

    fn trust(&self) -> UpstreamTls {
        match &self.certificate {
            Some(certificate) => UpstreamTls::CustomRoots {
                certificates_der: vec![certificate.clone()],
                include_system: false,
            },
            None => UpstreamTls::System,
        }
    }

    async fn next(&mut self) -> WsEvent {
        tokio::time::timeout(DEADLINE, self.events.recv())
            .await
            .unwrap()
            .unwrap()
    }

    async fn stop(self) {
        self.cancel.cancel();
        tokio::time::timeout(DEADLINE, self.task)
            .await
            .unwrap()
            .unwrap();
    }
}

async fn serve_ws_origin<I: AsyncRead + AsyncWrite + Unpin>(
    mut io: I,
    alpn: Option<Vec<u8>>,
    events: &mpsc::Sender<WsEvent>,
    mut commands: mpsc::Receiver<Frame>,
    reply: Option<&str>,
) {
    let Ok(head) = read_head(&mut io).await else {
        return;
    };
    let head = std::str::from_utf8(&head).unwrap();
    let values = |name: &str| {
        head.split("\r\n")
            .filter_map(move |line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case(name).then_some(value.trim())
            })
            .collect::<Vec<_>>()
    };
    let upgrade = values("upgrade")
        .iter()
        .any(|value| value.eq_ignore_ascii_case("websocket"));
    let connection = values("connection").iter().any(|value| {
        value
            .split(',')
            .any(|part| part.trim().eq_ignore_ascii_case("upgrade"))
    });
    let key = values("sec-websocket-key") == [KEY];
    assert!(
        values("proxy-authorization").is_empty(),
        "upstream credentials must not reach the WS origin"
    );
    events
        .send(WsEvent::Handshake {
            path: head
                .lines()
                .next()
                .unwrap()
                .split_ascii_whitespace()
                .nth(1)
                .unwrap()
                .to_owned(),
            upgrade,
            connection,
            key,
            alpn,
        })
        .await
        .unwrap();
    if !upgrade || !connection || !key {
        io.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        return;
    }
    let mut response = reply.map(str::to_owned).unwrap_or_else(|| format!("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {ACCEPT}\r\n\r\n")).into_bytes();
    // A real protocol engine may already have buffered bytes beyond the 101 head.
    response.extend(Frame::new(1, b"server-first".to_vec()).encode(false));
    io.write_all(&response).await.unwrap();
    let (mut reader, mut writer) = tokio::io::split(io);
    let incoming = async {
        while let Ok(frame) = read_frame(&mut reader, true).await {
            events.send(WsEvent::Frame(frame)).await.unwrap();
        }
    };
    let outgoing = async {
        while let Some(frame) = commands.recv().await {
            if writer.write_all(&frame.encode(false)).await.is_err() {
                break;
            }
            if frame.opcode == 8 {
                writer.shutdown().await.unwrap();
            }
        }
    };
    tokio::select! { _ = incoming => {}, _ = outgoing => {} }
}

async fn ws_proxy(
    origin: &WsOrigin,
) -> (mio_proxy::ProxyHandle, Vec<u8>, mpsc::Receiver<ProxyEvent>) {
    ws_proxy_with(origin, 32, |_| {}).await
}

async fn ws_proxy_with(
    origin: &WsOrigin,
    capacity: usize,
    configure: impl FnOnce(&mut ProxyConfig),
) -> (mio_proxy::ProxyHandle, Vec<u8>, mpsc::Receiver<ProxyEvent>) {
    let ca = MitmAuthority::generate().unwrap();
    let certificate = ca.certificate_der();
    let mut config = ProxyConfig::new(ca);
    config.tls.upstream.trust = origin.trust();
    config.limits.request_timeout = DEADLINE;
    configure(&mut config);
    let (events, receiver) = mpsc::channel(capacity);
    let proxy = mio_proxy::start(
        config,
        ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
        events,
    )
    .await
    .unwrap();
    (proxy, certificate, receiver)
}

async fn ws_handshake<I: AsyncRead + AsyncWrite + Unpin>(io: &mut I, url: &str) -> Vec<u8> {
    io.write_all(format!("GET {url} HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive, Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {KEY}\r\n\r\n").as_bytes()).await.unwrap();
    tokio::time::timeout(DEADLINE, read_head(io))
        .await
        .unwrap()
        .unwrap()
}

async fn next_frame<I: AsyncRead + Unpin>(io: &mut I) -> Frame {
    tokio::time::timeout(DEADLINE, read_frame(io, false))
        .await
        .unwrap()
        .unwrap()
}

async fn assert_origin_frame(origin: &mut WsOrigin, expected: &Frame) {
    match origin.next().await {
        WsEvent::Frame(frame) => assert_eq!(&frame, expected),
        event => panic!("expected origin frame, got {event:?}"),
    }
}

async fn terminal_record(events: &mut mpsc::Receiver<ProxyEvent>) -> mio_plugin_api::FlowRecord {
    tokio::time::timeout(DEADLINE, async {
        loop {
            if let ProxyEvent::Captured { record, .. } = events.recv().await.unwrap()
                && !matches!(
                    record.summary.state,
                    FlowState::Pending | FlowState::Intercepted
                )
            {
                return record;
            }
        }
    })
    .await
    .unwrap()
}
