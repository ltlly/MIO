use std::{
    ffi::OsString,
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use mio_core::{Client, Event};
use serde::Serialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub(crate) struct WorkflowConfig {
    pub out: PathBuf,
    pub static_assets: usize,
    pub ipc_iters: usize,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WorkflowReport {
    pub cold_startup_ms: f64,
    pub proxy_capture_ms: f64,
    pub provenance_steps_verified: usize,
    pub repeater_send_ms: f64,
    pub sse_stream_ms: f64,
    pub scanner_audit_ms: f64,
    pub scanner_findings: usize,
    pub ipc_roundtrip_p50_us: u64,
    pub ipc_roundtrip_p95_us: u64,
    pub full_save_ms: f64,
    pub slim_save_ms: f64,
    pub full_project_bytes: u64,
    pub slim_project_bytes: u64,
    pub slim_reduction_percent: f64,
    pub slim_removed_flows: usize,
    pub warm_reopen_ms: f64,
    pub process_memory_bytes: u64,
    pub runtime_join_confirmed: bool,
    pub total_wall_ms: f64,
}

pub(crate) fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<WorkflowConfig> {
    let raw: Vec<String> = args
        .into_iter()
        .map(|a| {
            a.into_string()
                .map_err(|_| anyhow::anyhow!("non-UTF-8 arg"))
        })
        .collect::<Result<_>>()?;
    let mut out = None;
    let mut static_assets = 24usize;
    let mut ipc_iters = 40usize;
    let mut i = 0;
    while i < raw.len() {
        let val = raw.get(i + 1).context("missing flag value")?;
        match raw[i].as_str() {
            "--out" => out = Some(PathBuf::from(val)),
            "--static-assets" => static_assets = val.parse()?,
            "--ipc-iters" => ipc_iters = val.parse()?,
            other => bail!("unknown flag {other}"),
        }
        i += 2;
    }
    Ok(WorkflowConfig {
        out: out.context("missing --out <report.json>")?,
        static_assets: static_assets.clamp(2, 500),
        ipc_iters: ipc_iters.clamp(4, 500),
    })
}

pub(crate) async fn spawn_workflow_origin(cancel: CancellationToken) -> Result<SocketAddr> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        loop {
            let accept = tokio::select! {
                _ = cancel.cancelled() => break,
                res = listener.accept() => res,
            };
            let Ok((mut stream, _)) = accept else {
                continue;
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let Ok(n) = stream.read(&mut buf).await else {
                    return;
                };
                let req = String::from_utf8_lossy(&buf[..n]);
                let first = req.lines().next().unwrap_or("");
                let path = first.split_whitespace().nth(1).unwrap_or("/");
                let resp = if path.starts_with("/sse") {
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"tick\":1}\n\ndata: {\"tick\":2}\n\n".to_vec()
                } else if path.starts_with("/static/") {
                    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
                    png.resize(65_536, 0x42);
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        png.len()
                    );
                    let mut out = head.into_bytes();
                    out.extend_from_slice(&png);
                    out
                } else if path.starts_with("/vuln") {
                    let body = format!(
                        "<html><body><h1>Search</h1><div>query={path}</div><div>SQL syntax error near MySQL</div></body></html>"
                    );
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nServer: Apache/2.4.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .into_bytes()
                } else {
                    let body = r#"{"status":"ok","token":"eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhZG1pbiJ9.sig"}"#;
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .into_bytes()
                };
                let _ = stream.write_all(&resp).await;
            });
        }
    });
    Ok(addr)
}

pub(crate) async fn recv_ev<T>(
    client: &mut Client,
    mut f: impl FnMut(Event) -> Option<T>,
) -> Result<T> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let ev = client.events.recv().await.context("event stream closed")?;
            if let Event::Error {
                operation, message, ..
            } = &ev
            {
                bail!("runtime error in {operation}: {message}");
            }
            if let Event::SendRejected { message, .. } = &ev {
                bail!("repeater send rejected: {message}");
            }
            if let Some(v) = f(ev) {
                return Ok(v);
            }
        }
    })
    .await
    .context("event timeout")?
}

pub(crate) async fn send_via_proxy(
    proxy: SocketAddr,
    origin: SocketAddr,
    path: &str,
) -> Result<()> {
    let mut stream = TcpStream::connect(proxy).await?;
    let req = format!(
        "GET http://{origin}{path} HTTP/1.1\r\nHost: {origin}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await?;
    let mut sink = Vec::new();
    stream.read_to_end(&mut sink).await?;
    ensure!(sink.starts_with(b"HTTP/1.1 200"), "unexpected proxy reply");
    Ok(())
}
