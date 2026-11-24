use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Response, Version, body::Incoming, service::service_fn};
use hyper_util::rt::{TokioExecutor, TokioIo};
use mio_plugin_api::{FlowState, Header, HttpRequest, HttpVersion, ProjectKey, Uuid};
use mio_proxy::{
    ClientCertificateFormat, ClientCertificateRule, InterceptDecision, InterceptFilter,
    InterceptFilterOutcome, MitmAuthority, ParsedClientIdentity, ProxyConfig, ProxyCredentials,
    ProxyEndpoint, ProxyError, ProxyEvent, SendConfig, UpstreamRoute, UpstreamTls,
};
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use std::{
    convert::Infallible,
    net::{Ipv4Addr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
    task::{JoinHandle, JoinSet},
};
use tokio_rustls::{
    TlsAcceptor, TlsConnector,
    rustls::{
        self, RootCertStore,
        pki_types::{CertificateDer, PrivateKeyDer, ServerName},
    },
};
use tokio_util::sync::CancellationToken;

const DEADLINE: Duration = Duration::from_secs(5);

#[path = "http_pipeline/client.rs"]
mod client;
#[path = "http_pipeline/filter.rs"]
mod filter;
#[path = "http_pipeline/filter_errors.rs"]
mod filter_errors;
#[path = "http_pipeline/origin.rs"]
mod origin;
#[path = "http_pipeline/request_target.rs"]
mod request_target;
#[path = "http_pipeline/response_flags.rs"]
mod response_flags;
#[path = "http_pipeline/roundtrip.rs"]
mod roundtrip;
#[path = "http_pipeline/sse.rs"]
mod sse;
#[path = "http_pipeline/ssl_passthrough.rs"]
mod ssl_passthrough;
#[path = "http_pipeline/stop.rs"]
mod stop;
#[path = "http_pipeline/stop_reclaim.rs"]
mod stop_reclaim;
#[path = "http_pipeline/terminal.rs"]
mod terminal;
#[path = "http_pipeline/transparent_connect.rs"]
mod transparent_connect;
#[path = "http_pipeline/upstream.rs"]
mod upstream;
#[path = "http_pipeline/upstream_mtls.rs"]
mod upstream_mtls;
#[path = "http_pipeline/upstream_tests.rs"]
mod upstream_tests;
#[path = "http_pipeline/websocket.rs"]
mod websocket;

use client::{
    client, client_with_fragmented_hello, connect_tunnel, next_intercept, proxy, proxy_with,
    proxy_with_route, tls_connector, tls_stream,
};
use origin::{CertificateKind, Origin, OriginEvent, origin_certificate};
use upstream::{Upstream, UpstreamEvent, UpstreamKind, proxy_credentials, upstream_request};
