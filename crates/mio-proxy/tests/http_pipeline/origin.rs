use super::*;

#[derive(Debug)]
pub(super) enum OriginEvent {
    Request {
        version: Version,
        alpn: Option<Vec<u8>>,
        headers: Vec<(String, Vec<u8>)>,
        body: Vec<u8>,
    },
    TlsRejected,
    Closed,
}

pub(super) struct Origin {
    pub(crate) address: SocketAddr,
    pub(crate) certificate: Option<Vec<u8>>,
    pub(crate) events: mpsc::Receiver<OriginEvent>,
    pub(crate) received: Arc<AtomicUsize>,
    pub(crate) cancel: CancellationToken,
    pub(crate) task: JoinHandle<()>,
}

#[derive(Clone, Copy)]
pub(super) enum CertificateKind {
    Valid,
    WrongName,
    Expired,
}

pub(super) fn origin_certificate(
    kind: CertificateKind,
    h2: bool,
) -> (Vec<u8>, rustls::ServerConfig) {
    let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let ca_key = KeyPair::generate().unwrap();
    let certificate = ca.self_signed(&ca_key).unwrap().der().to_vec();
    let issuer = Issuer::new(ca, ca_key);
    let names = if matches!(kind, CertificateKind::WrongName) {
        vec!["wrong.invalid".into()]
    } else {
        vec!["localhost".into(), "127.0.0.1".into()]
    };
    let mut leaf = CertificateParams::new(names).unwrap();
    if matches!(kind, CertificateKind::Expired) {
        leaf.not_before = rcgen::date_time_ymd(2000, 1, 1);
        leaf.not_after = rcgen::date_time_ymd(2001, 1, 1);
    }
    let leaf_key = KeyPair::generate().unwrap();
    let leaf = leaf.signed_by(&leaf_key, &issuer).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![leaf.der().clone()],
            PrivateKeyDer::try_from(leaf_key.serialize_der()).unwrap(),
        )
        .unwrap();
    config.alpn_protocols = vec![if h2 {
        b"h2".to_vec()
    } else {
        b"http/1.1".to_vec()
    }];
    (certificate, config)
}

impl Origin {
    pub(super) async fn start(tls: Option<(CertificateKind, bool)>, silent: bool) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let (certificate, acceptor) = match tls {
            Some((kind, h2)) => {
                let (ca, config) = origin_certificate(kind, h2);
                (Some(ca), Some(TlsAcceptor::from(Arc::new(config))))
            }
            None => (None, None),
        };
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let (event_tx, events) = mpsc::channel(32);
        let received = Arc::new(AtomicUsize::new(0));
        let task_received = received.clone();
        let task = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    _ = task_cancel.cancelled() => break,
                    Some(result) = tasks.join_next(), if !tasks.is_empty() => { result.unwrap(); },
                    result = listener.accept() => {
                        let (stream, _) = result.unwrap();
                        let acceptor = acceptor.clone();
                        let events = event_tx.clone();
                        let received = task_received.clone();
                        let cancel = task_cancel.clone();
                        tasks.spawn(async move {
                            tokio::select! {
                                _ = cancel.cancelled() => {},
                                _ = async {
                                    if let Some(acceptor) = acceptor {
                                        match acceptor.accept(stream).await {
                                            Ok(stream) => {
                                                let alpn = stream.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
                                                serve_origin(stream, alpn, events.clone(), received, silent).await;
                                            }
                                            Err(_) => { events.send(OriginEvent::TlsRejected).await.unwrap(); },
                                        }
                                    } else {
                                        serve_origin(stream, None, events.clone(), received, silent).await;
                                    }
                                    events.send(OriginEvent::Closed).await.unwrap();
                                } => {},
                            }
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
            certificate,
            events,
            received,
            cancel,
            task,
        }
    }

    pub(super) fn url(&self) -> String {
        format!(
            "{}://localhost:{}/test",
            if self.certificate.is_some() {
                "https"
            } else {
                "http"
            },
            self.address.port()
        )
    }

    pub(super) fn trust(&self) -> UpstreamTls {
        match &self.certificate {
            Some(certificate) => UpstreamTls::CustomRoots {
                certificates_der: vec![certificate.clone()],
                include_system: false,
            },
            None => UpstreamTls::System,
        }
    }

    pub(super) async fn next(&mut self) -> OriginEvent {
        tokio::time::timeout(DEADLINE, self.events.recv())
            .await
            .unwrap()
            .unwrap()
    }

    pub(super) async fn stop(self) {
        self.cancel.cancel();
        tokio::time::timeout(DEADLINE, self.task)
            .await
            .unwrap()
            .unwrap();
    }
}

async fn serve_origin<I: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    io: I,
    alpn: Option<Vec<u8>>,
    events: mpsc::Sender<OriginEvent>,
    received: Arc<AtomicUsize>,
    silent: bool,
) {
    let service = service_fn(move |request: Request<Incoming>| {
        let events = events.clone();
        let received = received.clone();
        let alpn = alpn.clone();
        async move {
            let wait_forever = silent && request.uri().path() != "/quick";
            let version = request.version();
            let headers = request
                .headers()
                .iter()
                .map(|(name, value)| (name.to_string(), value.as_bytes().to_vec()))
                .collect();
            let body = request
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec();
            received.fetch_add(1, Ordering::SeqCst);
            events
                .send(OriginEvent::Request {
                    version,
                    alpn,
                    headers,
                    body,
                })
                .await
                .unwrap();
            if wait_forever {
                return std::future::pending::<Result<Response<Full<Bytes>>, Infallible>>().await;
            }
            let mut response = Response::new(Full::new(Bytes::from_static(&[0, 0xff, b'O'])));
            response
                .headers_mut()
                .append("x-origin", "one".parse().unwrap());
            response
                .headers_mut()
                .append("x-origin", "two".parse().unwrap());
            Ok::<_, Infallible>(response)
        }
    });
    let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
    let _ = builder.serve_connection(TokioIo::new(io), service).await;
}
