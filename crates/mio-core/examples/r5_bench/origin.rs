use std::{
    io::{self, ErrorKind},
    net::{Ipv4Addr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

const IO_DEADLINE: Duration = Duration::from_secs(5);
const FLOW_DEADLINE: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub(crate) struct OriginStats {
    pub bench_requests: usize,
    pub hold_requests: usize,
    pub hold_active: usize,
    pub hold_closed: usize,
}

#[derive(Default)]
struct OriginCounters {
    bench_requests: AtomicUsize,
    hold_requests: AtomicUsize,
    hold_active: AtomicUsize,
    hold_closed: AtomicUsize,
    cancel: CancellationToken,
}

pub(crate) struct BenchOrigin {
    pub address: SocketAddr,
    cancel: CancellationToken,
    task: JoinHandle<Result<()>>,
}

impl BenchOrigin {
    pub(crate) async fn start() -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let state = Arc::new(OriginCounters::default());
        let cancel = state.cancel.clone();
        let task = tokio::spawn(async move {
            let mut conns = JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    _ = state.cancel.cancelled() => break,
                    Some(res) = conns.join_next(), if !conns.is_empty() => { res??; },
                    accepted = listener.accept(), if conns.len() < 128 => {
                        let (stream, _) = accepted?;
                        stream.set_nodelay(true)?;
                        let state = state.clone();
                        conns.spawn(async move {
                            let _ = tokio::time::timeout(FLOW_DEADLINE, serve_origin_conn(stream, state)).await;
                            Ok::<(), anyhow::Error>(())
                        });
                    }
                }
            }
            drop(listener);
            state.cancel.cancel();
            while let Some(res) = conns.join_next().await {
                res??;
            }
            Ok(())
        });
        Ok(Self {
            address,
            cancel,
            task,
        })
    }

    pub(crate) async fn stop(self) -> Result<()> {
        self.cancel.cancel();
        tokio::time::timeout(IO_DEADLINE, self.task).await???;
        Ok(())
    }
}

pub(crate) async fn read_http_head(stream: &mut TcpStream) -> io::Result<String> {
    let mut bytes = Vec::with_capacity(256);
    while !bytes.ends_with(b"\r\n\r\n") {
        if bytes.len() >= 8192 {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "HTTP head too large",
            ));
        }
        bytes.push(stream.read_u8().await?);
    }
    String::from_utf8(bytes).map_err(|_| io::Error::new(ErrorKind::InvalidData, "non-utf8 head"))
}

pub(crate) fn header_val<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.split("\r\n")
        .filter_map(|line| line.split_once(':'))
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim())
}

async fn serve_origin_conn(mut stream: TcpStream, state: Arc<OriginCounters>) -> Result<()> {
    let mut reused = false;
    loop {
        let req = match read_http_head(&mut stream).await {
            Ok(req) => req,
            Err(_) if reused => return Ok(()),
            Err(err) => return Err(err.into()),
        };
        let mut parts = req.lines().next().unwrap_or("").split_whitespace();
        let method = parts.next().context("missing method")?;
        let target = parts.next().context("missing path")?;
        let path = target.split('?').next().unwrap_or(target);
        let content_len = header_val(&req, "content-length")
            .map(str::parse::<usize>)
            .transpose()?
            .unwrap_or(0);
        ensure!(content_len <= 64 * 1024, "origin body exceeds 64 KiB");
        if content_len > 0 {
            let mut discard = vec![0_u8; content_len];
            stream.read_exact(&mut discard).await?;
        }
        if (path == "/bench" || path.starts_with("/bench/")) && matches!(method, "GET" | "POST") {
            let seq = state.bench_requests.fetch_add(1, Ordering::SeqCst) + 1;
            let keep_alive =
                !header_val(&req, "connection").is_some_and(|v| v.eq_ignore_ascii_case("close"));
            let (status, ctype, marker) = if target.contains("kind=script") {
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
            let body = format!(
                "/* {marker} */ seq={seq} method={method} target={target} req_bytes={content_len} pad={}\n",
                "x".repeat(seq % 64)
            );
            let conn = if keep_alive { "keep-alive" } else { "close" };
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nX-Fixture: one\r\nX-Fixture: two\r\nContent-Length: {}\r\nConnection: {conn}\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(resp.as_bytes()).await?;
            if !keep_alive {
                return Ok(());
            }
            reused = true;
            continue;
        }
        if path == "/hold" {
            state.hold_requests.fetch_add(1, Ordering::SeqCst);
            state.hold_active.fetch_add(1, Ordering::SeqCst);
            struct HoldGuard<'a>(&'a OriginCounters);
            impl Drop for HoldGuard<'_> {
                fn drop(&mut self) {
                    self.0.hold_active.fetch_sub(1, Ordering::SeqCst);
                    self.0.hold_closed.fetch_add(1, Ordering::SeqCst);
                }
            }
            let _guard = HoldGuard(&state);
            let mut probe = [0_u8; 1];
            tokio::select! {
                biased;
                _ = state.cancel.cancelled() => {}
                _ = stream.read(&mut probe) => {}
            }
            return Ok(());
        }
        if path == "/stats" {
            let body = format!(
                "{{\"bench_requests\":{},\"hold_requests\":{},\"hold_active\":{},\"hold_closed\":{}}}\n",
                state.bench_requests.load(Ordering::SeqCst),
                state.hold_requests.load(Ordering::SeqCst),
                state.hold_active.load(Ordering::SeqCst),
                state.hold_closed.load(Ordering::SeqCst),
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(resp.as_bytes()).await?;
            return Ok(());
        }
        bail!("unknown benchmark origin path: {path}");
    }
}

pub(crate) async fn fetch_origin_stats(origin: SocketAddr) -> Result<OriginStats> {
    let mut stream = tokio::time::timeout(IO_DEADLINE, TcpStream::connect(origin)).await??;
    stream.set_nodelay(true)?;
    stream
        .write_all(
            format!("GET /stats HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await?;
    let head = tokio::time::timeout(IO_DEADLINE, read_http_head(&mut stream)).await??;
    ensure!(head.starts_with("HTTP/1.1 200 "), "origin /stats rejected");
    let len: usize = header_val(&head, "content-length")
        .context("missing stats content-length")?
        .parse()?;
    let mut body = vec![0_u8; len];
    tokio::time::timeout(IO_DEADLINE, stream.read_exact(&mut body)).await??;
    Ok(serde_json::from_slice(&body)?)
}
