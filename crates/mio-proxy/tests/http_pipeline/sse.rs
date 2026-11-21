use super::*;
use hyper::body::{Body as HttpBody, Frame as HttpFrame};
use std::{pin::Pin, task::Poll};

#[path = "sse/intercept.rs"]
mod intercept;
#[path = "sse/limits_and_stop.rs"]
mod limits_and_stop;
#[path = "sse/routes_and_client.rs"]
mod routes_and_client;
#[path = "sse/streaming.rs"]
mod streaming;

const FIRST: &[u8] = b": keep-alive\r\nid: 1\r\ndata: first\r\n\r\n";
const SECOND: &[u8] = "id: 2\ndata: 中文\n\n".as_bytes();

struct EventBody(mpsc::Receiver<Result<HttpFrame<Bytes>, std::io::Error>>);

impl HttpBody for EventBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Result<HttpFrame<Self::Data>, Self::Error>>> {
        self.0.poll_recv(cx)
    }
}

struct SseOrigin {
    address: SocketAddr,
    certificate: Option<Vec<u8>>,
    events: mpsc::Receiver<OriginEvent>,
    frames: Option<mpsc::Sender<Result<HttpFrame<Bytes>, std::io::Error>>>,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl SseOrigin {
    async fn start(tls_h2: Option<bool>, length: Option<usize>) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let (certificate, acceptor) = match tls_h2 {
            Some(h2) => {
                let (certificate, config) = origin_certificate(CertificateKind::Valid, h2);
                (Some(certificate), Some(TlsAcceptor::from(Arc::new(config))))
            }
            None => (None, None),
        };
        let (sender, events) = mpsc::channel(8);
        let (frames, receiver) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            tokio::select! {
                _ = task_cancel.cancelled() => {},
                _ = async {
                    let (stream, _) = listener.accept().await.unwrap();
                    if let Some(acceptor) = acceptor {
                        let stream = acceptor.accept(stream).await.unwrap();
                        let alpn = stream.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
                        serve_sse_origin(stream, alpn, sender.clone(), receiver, length).await;
                    } else {
                        serve_sse_origin(stream, None, sender.clone(), receiver, length).await;
                    }
                    let _ = sender.send(OriginEvent::Closed).await;
                } => {},
            }
        });
        Self {
            address,
            certificate,
            events,
            frames: Some(frames),
            cancel,
            task,
        }
    }

    fn url(&self) -> String {
        format!(
            "{}://localhost:{}/events",
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

    async fn send(&self, bytes: &'static [u8]) {
        self.frames
            .as_ref()
            .unwrap()
            .send(Ok(HttpFrame::data(Bytes::from_static(bytes))))
            .await
            .unwrap();
    }

    async fn next(&mut self) -> OriginEvent {
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

async fn serve_sse_origin<I: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    io: I,
    alpn: Option<Vec<u8>>,
    events: mpsc::Sender<OriginEvent>,
    receiver: mpsc::Receiver<Result<HttpFrame<Bytes>, std::io::Error>>,
    length: Option<usize>,
) {
    let receiver = Arc::new(std::sync::Mutex::new(Some(receiver)));
    let service = service_fn(move |request: Request<Incoming>| {
        let events = events.clone();
        let alpn = alpn.clone();
        let body = EventBody(
            receiver
                .lock()
                .unwrap()
                .take()
                .expect("one fixture request"),
        );
        async move {
            let version = request.version();
            let headers = request
                .headers()
                .iter()
                .map(|(name, value)| (name.to_string(), value.as_bytes().to_vec()))
                .collect();
            let request_body = request
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec();
            events
                .send(OriginEvent::Request {
                    version,
                    alpn,
                    headers,
                    body: request_body,
                })
                .await
                .unwrap();
            let mut response = Response::new(body);
            response.headers_mut().insert(
                "content-type",
                "Text/Event-Stream; charset=utf-8".parse().unwrap(),
            );
            if let Some(length) = length {
                response
                    .headers_mut()
                    .insert("content-length", length.into());
            } else {
                response
                    .headers_mut()
                    .insert("trailer", "x-stream-end".parse().unwrap());
            }
            Ok::<_, Infallible>(response)
        }
    });
    let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
    let _ = builder.serve_connection(TokioIo::new(io), service).await;
}

#[derive(Debug)]
enum StreamEvent {
    Head {
        status: u16,
        version: Version,
        alpn: Option<Vec<u8>>,
        headers: hyper::HeaderMap,
    },
    Data(Vec<u8>),
    Trailers(hyper::HeaderMap),
    End(Result<(), String>),
}

struct SseClient {
    events: mpsc::Receiver<StreamEvent>,
    task: JoinHandle<()>,
}

impl SseClient {
    fn start(address: SocketAddr, url: String, ca: Vec<u8>, tls: bool, h2: bool) -> Self {
        let (events, receiver) = mpsc::channel(16);
        let task = tokio::spawn(async move {
            if tls {
                let authority = url
                    .trim_start_matches("https://")
                    .split('/')
                    .next()
                    .unwrap();
                let stream = tls_stream(address, authority, ca, h2).await;
                let alpn = stream.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
                consume_sse(stream, url, h2, alpn, events).await;
            } else {
                let stream = TcpStream::connect(address).await.unwrap();
                consume_sse(stream, url, false, None, events).await;
            }
        });
        Self {
            events: receiver,
            task,
        }
    }

    async fn next(&mut self) -> StreamEvent {
        tokio::time::timeout(DEADLINE, self.events.recv())
            .await
            .expect("SSE must make progress before the origin releases EOF")
            .unwrap()
    }

    async fn bytes(&mut self, expected: &[u8]) {
        let mut bytes = Vec::new();
        while bytes.len() < expected.len() {
            match self.next().await {
                StreamEvent::Data(data) => bytes.extend(data),
                event => panic!("expected streamed DATA, got {event:?}"),
            }
        }
        assert_eq!(bytes, expected);
    }

    async fn finish(self) {
        tokio::time::timeout(DEADLINE, self.task)
            .await
            .expect("the actual ingress connection must be reclaimed")
            .unwrap();
    }
}

async fn consume_sse<I: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    io: I,
    url: String,
    h2: bool,
    alpn: Option<Vec<u8>>,
    events: mpsc::Sender<StreamEvent>,
) {
    let request = Request::builder()
        .uri(url)
        .header("accept", "text/event-stream")
        .header("te", "trailers")
        .body(Full::new(Bytes::new()))
        .unwrap();
    let (mut h1_sender, mut h2_sender) = (None, None);
    let (response, driver) = if h2 {
        let (mut sender, connection) =
            hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(io))
                .await
                .unwrap();
        let driver = tokio::spawn(connection);
        let response = sender.send_request(request).await.unwrap();
        h2_sender = Some(sender);
        (response, driver)
    } else {
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(io))
            .await
            .unwrap();
        let driver = tokio::spawn(connection);
        let response = sender.send_request(request).await.unwrap();
        h1_sender = Some(sender);
        (response, driver)
    };
    events
        .send(StreamEvent::Head {
            status: response.status().as_u16(),
            version: response.version(),
            alpn,
            headers: response.headers().clone(),
        })
        .await
        .unwrap();
    let mut body = response.into_body();
    let mut result = Ok(());
    while let Some(frame) = body.frame().await {
        let event = match frame {
            Ok(frame) => match frame.into_data() {
                // HTTP/2 can expose an empty DATA EOS; it adds no bytes to the SSE stream.
                Ok(data) if data.is_empty() => continue,
                Ok(data) => StreamEvent::Data(data.to_vec()),
                Err(frame) => StreamEvent::Trailers(frame.into_trailers().unwrap()),
            },
            Err(error) => {
                result = Err(error.to_string());
                break;
            }
        };
        events.send(event).await.unwrap();
    }
    events.send(StreamEvent::End(result)).await.unwrap();
    drop((body, h1_sender, h2_sender));
    let _ = tokio::time::timeout(DEADLINE, driver)
        .await
        .expect("HTTP driver must close after EOF/reset and releasing its sender")
        .unwrap();
}

async fn start_proxy(
    origin: &SseOrigin,
    change: impl FnOnce(&mut ProxyConfig),
    event_capacity: usize,
) -> (mio_proxy::ProxyHandle, Vec<u8>, mpsc::Receiver<ProxyEvent>) {
    let ca = MitmAuthority::generate().unwrap();
    let certificate = ca.certificate_der();
    let mut config = ProxyConfig::new(ca);
    config.tls.upstream.trust = origin.trust();
    config.limits.request_timeout = DEADLINE * 2;
    change(&mut config);
    let (events, receiver) = mpsc::channel(event_capacity);
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

async fn terminal(events: &mut mpsc::Receiver<ProxyEvent>) -> mio_plugin_api::FlowRecord {
    tokio::time::timeout(DEADLINE, async {
        loop {
            if let ProxyEvent::Captured {
                record,
                response_stream,
                ..
            } = events.recv().await.unwrap()
                && !matches!(
                    record.summary.state,
                    FlowState::Pending | FlowState::Intercepted
                )
            {
                if let Some(stream) = response_stream {
                    stream.validate().unwrap();
                    assert_eq!(stream.original_status, 200);
                    assert_eq!(
                        stream.body_complete,
                        record.summary.state == FlowState::Complete
                            && !matches!(record.summary.status, Some(204 | 304)),
                        "only the source EOS makes the original body complete"
                    );
                }
                return record;
            }
        }
    })
    .await
    .unwrap()
}
