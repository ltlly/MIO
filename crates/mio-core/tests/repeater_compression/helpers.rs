pub(crate) use std::{
    io::{ErrorKind, Write},
    net::SocketAddr,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

pub(crate) use anyhow::{Context, Result, ensure};
pub(crate) use flate2::{
    Compression,
    write::{GzEncoder, ZlibEncoder},
};
pub(crate) use mio_core::{
    Client, Command, Event, FlowRecord, FlowState, Header, HttpResponse, HttpVersion,
    OriginalMessages, ProjectKey, RepeaterRedirectMode, RepeaterSettings, Runtime, TrafficSource,
    UpstreamProxyProtocol, UpstreamProxySettings, WorkTab, Workspace,
};
pub(crate) use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
    task::JoinHandle,
    time::{Instant, timeout_at},
};

pub(crate) const BODY: &[u8] = b"\0\xff\x80first\r\nsecond\0\xfe";
// Node v22.23.2 zlib.brotliCompressSync(BODY), independently decoded back to BODY.
pub(crate) const BROTLI_BODY: &[u8] = &[
    0x8b, 0x08, 0x80, 0x00, 0xff, 0x80, 0x66, 0x69, 0x72, 0x73, 0x74, 0x0d, 0x0a, 0x73, 0x65, 0x63,
    0x6f, 0x6e, 0x64, 0x00, 0xfe, 0x03,
];
pub(crate) const REQUEST_BODY: &[u8] = b"\0\xffQ";
pub(crate) const EXCHANGE_BUDGET: Duration = Duration::from_secs(3);
pub(crate) const FIXTURE_BUDGET: Duration = Duration::from_secs(8);
pub(crate) type ResponseGate = (oneshot::Sender<()>, oneshot::Receiver<()>);

pub(crate) async fn event(
    client: &mut Client,
    predicate: impl Fn(&Event) -> bool,
) -> Result<Event> {
    loop {
        let event = client
            .events
            .recv()
            .await
            .context("Runtime events closed")?;
        ensure!(
            !matches!(
                event,
                Event::Error { .. }
                    | Event::WorkspaceRejected { .. }
                    | Event::SendRejected { .. }
                    | Event::FlowDetailError { .. }
            ),
            "unexpected Runtime failure: {event:?}"
        );
        if predicate(&event) {
            return Ok(event);
        }
    }
}

pub(crate) fn compressed_body(encoding: &str) -> Result<Vec<u8>> {
    match encoding {
        "gzip" => {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
            encoder.write_all(BODY)?;
            Ok(encoder.finish()?)
        }
        "deflate" => {
            // HTTP deflate uses a zlib wrapper, not a raw DEFLATE stream.
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
            encoder.write_all(BODY)?;
            Ok(encoder.finish()?)
        }
        "br" => Ok(BROTLI_BODY.to_vec()),
        _ => unreachable!(),
    }
}

pub(crate) fn response_bytes(encoding: Option<&str>, body: &[u8]) -> Vec<u8> {
    let coding = encoding
        .map(|encoding| format!("Content-Encoding: {encoding}\r\n"))
        .unwrap_or_default();
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n{coding}Content-Length: {}\r\nX-Fixture: one\r\nX-Fixture: two\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

pub(crate) fn check_response(
    response: &HttpResponse,
    encoding: Option<&str>,
    body: &[u8],
) -> Result<()> {
    ensure!(response.status == 200, "response status changed");
    ensure!(
        response.version == HttpVersion::Http11,
        "response version changed"
    );
    ensure!(response.body == body, "response binary body changed");
    ensure!(response.trailers.is_empty(), "unexpected response trailers");
    let values = |name: &str| {
        response
            .headers
            .iter()
            .filter(|header| header.name.eq_ignore_ascii_case(name))
            .map(|header| header.value.as_slice())
            .collect::<Vec<_>>()
    };
    ensure!(
        values("content-type") == [b"application/octet-stream".as_slice()],
        "response Content-Type changed"
    );
    ensure!(
        values("content-encoding") == encoding.map(str::as_bytes).into_iter().collect::<Vec<_>>(),
        "response Content-Encoding does not match its body"
    );
    ensure!(
        values("content-length") == [body.len().to_string().as_bytes()],
        "response Content-Length does not match its body"
    );
    ensure!(
        values("x-fixture") == [b"one".as_slice(), b"two".as_slice()],
        "duplicate response headers changed"
    );
    ensure!(
        response.headers.len() == 4 + usize::from(encoding.is_some()),
        "response gained or lost headers"
    );
    Ok(())
}
