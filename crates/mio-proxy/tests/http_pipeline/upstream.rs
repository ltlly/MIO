use super::*;

#[derive(Clone, Copy)]
pub(super) enum UpstreamKind {
    Http,
    Https(CertificateKind),
    Socks5,
}

#[derive(Debug)]
pub(super) enum UpstreamEvent {
    Route { target: String, authenticated: bool },
    AuthRejected,
    TlsRejected,
    Closed,
}

pub(super) struct Upstream {
    pub(crate) address: SocketAddr,
    pub(crate) kind: UpstreamKind,
    pub(crate) certificate: Option<Vec<u8>>,
    pub(crate) events: mpsc::Receiver<UpstreamEvent>,
    pub(crate) cancel: CancellationToken,
    pub(crate) task: JoinHandle<()>,
}

impl Upstream {
    pub(crate) async fn start(
        origin: &Origin,
        kind: UpstreamKind,
        authentication: bool,
        stall: bool,
    ) -> Self {
        Self::start_at_origin(origin.address, kind, authentication, stall).await
    }

    pub(crate) async fn start_at_origin(
        origin: SocketAddr,
        kind: UpstreamKind,
        authentication: bool,
        stall: bool,
    ) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let (certificate, acceptor) = match kind {
            UpstreamKind::Https(kind) => {
                let (ca, config) = origin_certificate(kind, false);
                (Some(ca), Some(TlsAcceptor::from(Arc::new(config))))
            }
            _ => (None, None),
        };
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let (sender, events) = mpsc::channel(32);
        let task = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    _ = task_cancel.cancelled() => break,
                    Some(result) = tasks.join_next(), if !tasks.is_empty() => { result.unwrap(); },
                    accepted = listener.accept() => {
                        let (stream, _) = accepted.unwrap();
                        let acceptor = acceptor.clone();
                        let events = sender.clone();
                        let cancel = task_cancel.clone();
                        tasks.spawn(async move {
                            tokio::select! {
                                _ = cancel.cancelled() => {},
                                _ = async {
                                    if let Some(acceptor) = acceptor {
                                        match acceptor.accept(stream).await {
                                            Ok(stream) => relay_http_upstream(stream, origin, authentication, stall, &events).await,
                                            Err(_) => events.send(UpstreamEvent::TlsRejected).await.unwrap(),
                                        }
                                    } else if matches!(kind, UpstreamKind::Socks5) {
                                        relay_socks_upstream(stream, origin, authentication, stall, &events).await;
                                    } else {
                                        relay_http_upstream(stream, origin, authentication, stall, &events).await;
                                    }
                                } => {},
                            }
                            let _ = events.send(UpstreamEvent::Closed).await;
                        });
                    }
                }
            }
            while let Some(result) = tasks.join_next().await {
                result.unwrap();
            }
        });
        Self {
            address,
            kind,
            certificate,
            events,
            cancel,
            task,
        }
    }

    pub(crate) fn route(&self, credentials: Option<ProxyCredentials>) -> UpstreamRoute {
        let endpoint = ProxyEndpoint {
            host: "localhost".into(),
            port: self.address.port(),
            credentials,
        };
        match self.kind {
            UpstreamKind::Http => UpstreamRoute::Http(endpoint),
            UpstreamKind::Https(_) => UpstreamRoute::Https {
                endpoint,
                tls: UpstreamTls::CustomRoots {
                    certificates_der: vec![self.certificate.clone().unwrap()],
                    include_system: false,
                },
            },
            UpstreamKind::Socks5 => UpstreamRoute::Socks5(endpoint),
        }
    }

    pub(crate) async fn next(&mut self) -> UpstreamEvent {
        tokio::time::timeout(DEADLINE, self.events.recv())
            .await
            .unwrap()
            .unwrap()
    }

    pub(crate) async fn stop(self) {
        self.cancel.cancel();
        tokio::time::timeout(DEADLINE, self.task)
            .await
            .unwrap()
            .unwrap();
    }
}

pub(super) fn proxy_credentials() -> Option<ProxyCredentials> {
    Some(ProxyCredentials {
        username: "user".into(),
        password: "pass".into(),
    })
}

async fn until_eof<I: AsyncRead + Unpin>(stream: &mut I) {
    let mut bytes = [0; 1024];
    while matches!(stream.read(&mut bytes).await, Ok(count) if count > 0) {}
}

async fn relay_http_upstream<I: AsyncRead + AsyncWrite + Unpin>(
    mut stream: I,
    origin: SocketAddr,
    authentication: bool,
    stall: bool,
    events: &mpsc::Sender<UpstreamEvent>,
) {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        assert!(head.len() < 8192);
        let Ok(byte) = stream.read_u8().await else {
            return;
        };
        head.push(byte);
    }
    let mut lines = head[..head.len() - 4]
        .split(|byte| *byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line));
    let request_line = std::str::from_utf8(lines.next().unwrap()).unwrap();
    let parts = request_line.split_ascii_whitespace().collect::<Vec<_>>();
    assert_eq!(parts.len(), 3);
    let headers = lines.collect::<Vec<_>>();
    let is_proxy_auth = |line: &[u8]| {
        line.split(|byte| *byte == b':')
            .next()
            .unwrap()
            .eq_ignore_ascii_case(b"proxy-authorization")
    };
    let credentials = headers
        .iter()
        .filter(|line| is_proxy_auth(line))
        .collect::<Vec<_>>();
    let authenticated = credentials.len() == 1
        && credentials[0]
            .splitn(2, |byte| *byte == b':')
            .nth(1)
            .unwrap()
            .trim_ascii()
            == b"Basic dXNlcjpwYXNz";
    if authentication && !authenticated {
        stream.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"mio-test\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        events.send(UpstreamEvent::AuthRejected).await.unwrap();
        return;
    }
    events
        .send(UpstreamEvent::Route {
            target: parts[1].into(),
            authenticated,
        })
        .await
        .unwrap();
    if stall {
        until_eof(&mut stream).await;
        return;
    }
    let mut destination = TcpStream::connect(origin).await.unwrap();
    if parts[0] == "CONNECT" {
        assert_eq!(parts[1], format!("localhost:{}", origin.port()));
        stream
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .unwrap();
    } else {
        let uri: hyper::Uri = parts[1].parse().unwrap();
        assert_eq!(uri.scheme_str(), Some("http"));
        assert_eq!(uri.port_u16(), Some(origin.port()));
        destination
            .write_all(
                format!(
                    "{} {} {}\r\n",
                    parts[0],
                    uri.path_and_query().unwrap(),
                    parts[2]
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        for line in headers {
            if !is_proxy_auth(line) {
                destination.write_all(line).await.unwrap();
                destination.write_all(b"\r\n").await.unwrap();
            }
        }
        destination.write_all(b"\r\n").await.unwrap();
    }
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut destination).await;
}

async fn relay_socks_upstream(
    mut stream: TcpStream,
    origin: SocketAddr,
    authentication: bool,
    stall: bool,
    events: &mpsc::Sender<UpstreamEvent>,
) {
    assert_eq!(stream.read_u8().await.unwrap(), 5);
    let count = stream.read_u8().await.unwrap() as usize;
    let mut methods = vec![0; count];
    stream.read_exact(&mut methods).await.unwrap();
    let selected = if authentication { 2 } else { 0 };
    if !methods.contains(&selected) {
        stream.write_all(&[5, 255]).await.unwrap();
        events.send(UpstreamEvent::AuthRejected).await.unwrap();
        return;
    }
    stream.write_all(&[5, selected]).await.unwrap();
    if authentication {
        assert_eq!(stream.read_u8().await.unwrap(), 1);
        let count = stream.read_u8().await.unwrap() as usize;
        let mut username = vec![0; count];
        stream.read_exact(&mut username).await.unwrap();
        let count = stream.read_u8().await.unwrap() as usize;
        let mut password = vec![0; count];
        stream.read_exact(&mut password).await.unwrap();
        if username != b"user" || password != b"pass" {
            stream.write_all(&[1, 1]).await.unwrap();
            events.send(UpstreamEvent::AuthRejected).await.unwrap();
            return;
        }
        stream.write_all(&[1, 0]).await.unwrap();
    }
    let mut prefix = [0; 4];
    stream.read_exact(&mut prefix).await.unwrap();
    assert_eq!(&prefix[..3], &[5, 1, 0]);
    assert_eq!(
        prefix[3], 3,
        "destination DNS must be delegated to the SOCKS proxy"
    );
    let count = stream.read_u8().await.unwrap() as usize;
    let mut host = vec![0; count];
    stream.read_exact(&mut host).await.unwrap();
    let port = stream.read_u16().await.unwrap();
    assert_eq!(port, origin.port());
    events
        .send(UpstreamEvent::Route {
            target: format!("{}:{port}", std::str::from_utf8(&host).unwrap()),
            authenticated: authentication,
        })
        .await
        .unwrap();
    if stall {
        until_eof(&mut stream).await;
        return;
    }
    let mut destination = TcpStream::connect(origin).await.unwrap();
    stream
        .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0])
        .await
        .unwrap();
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut destination).await;
}

pub(super) fn upstream_request(origin: &Origin) -> HttpRequest {
    HttpRequest {
        method: "POST".into(),
        url: origin.url(),
        version: HttpVersion::Http11,
        headers: vec![Header {
            name: "proxy-authorization".into(),
            value: b"must-not-reach-origin".to_vec(),
        }],
        body: vec![0, 255, b'P'],
        trailers: Vec::new(),
    }
}
