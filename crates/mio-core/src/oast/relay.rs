use std::{
    collections::VecDeque,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, PoisonError, RwLock},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, Semaphore, mpsc},
    task::JoinHandle,
    time::{Instant, sleep_until, timeout_at},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use url::Url;

use super::{
    CONNECTION_TIMEOUT, DEFAULT_DNS_DOMAIN, LocalOastServer, MAX_HTTP_CONCURRENCY,
    OastInteraction, STOP_TIMEOUT, SelfHostedOastConfig,
};

const DEFAULT_MAX_RELAY_BUFFER: usize = 2048;
const MAX_RELAY_POLL_REQ_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OastRelayConfig {
    pub bind_address: IpAddr,
    pub http_port: u16,
    pub dns_port: u16,
    pub poll_port: u16,
    pub server_domain: String,
    pub dns_answer_ip: Option<Ipv4Addr>,
    pub auth_token: Option<String>,
    pub max_buffered_interactions: usize,
}

impl Default for OastRelayConfig {
    fn default() -> Self {
        Self {
            bind_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            http_port: 0,
            dns_port: 0,
            poll_port: 0,
            server_domain: DEFAULT_DNS_DOMAIN.to_owned(),
            dns_answer_ip: None,
            auth_token: None,
            max_buffered_interactions: DEFAULT_MAX_RELAY_BUFFER,
        }
    }
}

#[derive(Default)]
struct RelayBuffer {
    next_seq: u64,
    cap: usize,
    entries: VecDeque<(u64, OastInteraction)>,
}

impl RelayBuffer {
    fn new(cap: usize) -> Self {
        Self {
            next_seq: 0,
            cap: cap.clamp(16, 16_384),
            entries: VecDeque::new(),
        }
    }

    fn push(&mut self, item: OastInteraction) {
        self.next_seq = self.next_seq.saturating_add(1);
        let seq = self.next_seq;
        while self.entries.len() >= self.cap {
            self.entries.pop_front();
        }
        self.entries.push_back((seq, item));
    }

    fn query_since(&self, cursor: u64) -> (Vec<OastInteraction>, String) {
        let effective_cursor = if cursor > self.next_seq { 0 } else { cursor };
        let items = self
            .entries
            .iter()
            .filter(|(seq, _)| *seq > effective_cursor)
            .map(|(_, item)| item.clone())
            .collect();
        let next = self.next_seq;
        (items, next.to_string())
    }
}

pub struct OastRelayServer {
    local: Option<LocalOastServer>,
    poll_addr: SocketAddr,
    server_domain: String,
    cancel: CancellationToken,
    connections: TaskTracker,
    collector_task: Option<JoinHandle<()>>,
    poll_task: Option<JoinHandle<()>>,
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

async fn read_poll_request(
    stream: &mut TcpStream,
    cancel: &CancellationToken,
    deadline: Instant,
) -> Option<String> {
    let mut raw = Vec::with_capacity(1024);
    let mut buf = [0u8; 1024];
    let mut has_terminator = false;
    while raw.len() < MAX_RELAY_POLL_REQ_BYTES {
        let to_read = buf.len().min(MAX_RELAY_POLL_REQ_BYTES - raw.len());
        let read = stream.read(&mut buf[..to_read]);
        tokio::select! {
            biased;
            () = cancel.cancelled() => return None,
            _ = sleep_until(deadline) => break,
            res = read => match res {
                Ok(0) => break,
                Ok(n) => raw.extend_from_slice(&buf[..n]),
                Err(_) => return None,
            }
        }
        if raw.windows(4).any(|w| w == b"\r\n\r\n") || raw.windows(2).any(|w| w == b"\n\n") {
            has_terminator = true;
            break;
        }
    }
    (has_terminator && !raw.is_empty()).then(|| String::from_utf8_lossy(&raw).into_owned())
}

async fn write_poll_json_response(
    stream: &mut TcpStream,
    status: &str,
    body: &[u8],
    cancel: &CancellationToken,
    deadline: Instant,
) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        body.len()
    );
    let write = async {
        let _ = stream.write_all(head.as_bytes()).await;
        let _ = stream.write_all(body).await;
        let _ = stream.flush().await;
        let _ = stream.shutdown().await;
    };
    tokio::select! {
        biased;
        () = cancel.cancelled() => {}
        _ = sleep_until(deadline) => {}
        _ = write => {}
    }
}

async fn handle_poll_connection(
    mut stream: TcpStream,
    expected_token: Option<String>,
    server_domain: String,
    buffer: Arc<RwLock<RelayBuffer>>,
    cancel: CancellationToken,
    permit: OwnedSemaphorePermit,
) {
    let _permit = permit;
    let deadline = Instant::now() + CONNECTION_TIMEOUT;
    let Some(req_text) = read_poll_request(&mut stream, &cancel, deadline).await else {
        return;
    };
    let mut lines = req_text.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");

    let mut auth_header = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("authorization")
        {
            auth_header = Some(value.trim());
        }
    }

    if let Some(expected) = expected_token.as_deref().filter(|s| !s.is_empty()) {
        let provided = auth_header.unwrap_or("");
        let bearer = provided
            .strip_prefix("Bearer ")
            .or_else(|| provided.strip_prefix("bearer "))
            .unwrap_or(provided);
        if !constant_time_eq(bearer.trim().as_bytes(), expected.as_bytes()) {
            write_poll_json_response(
                &mut stream,
                "401 Unauthorized",
                br#"{"error":"unauthorized"}"#,
                &cancel,
                deadline,
            )
            .await;
            return;
        }
    }

    if !method.eq_ignore_ascii_case("GET") {
        write_poll_json_response(
            &mut stream,
            "405 Method Not Allowed",
            br#"{"error":"method_not_allowed"}"#,
            &cancel,
            deadline,
        )
        .await;
        return;
    }

    let parsed_target = Url::parse(&format!("http://relay.invalid{target}")).ok();
    let path = parsed_target
        .as_ref()
        .map_or("/", |url| url.path());
    if path == "/healthz" {
        let body = serde_json::to_vec(&serde_json::json!({
            "ok": true,
            "server_domain": server_domain,
        }))
        .unwrap_or_default();
        write_poll_json_response(&mut stream, "200 OK", &body, &cancel, deadline).await;
        return;
    }

    if path != "/poll" {
        write_poll_json_response(
            &mut stream,
            "404 Not Found",
            br#"{"error":"not_found"}"#,
            &cancel,
            deadline,
        )
        .await;
        return;
    }

    let cursor = parsed_target
        .as_ref()
        .and_then(|url| {
            url.query_pairs()
                .find(|(k, _)| k == "cursor")
                .and_then(|(_, v)| v.trim().parse::<u64>().ok())
        })
        .unwrap_or(0);
    let (interactions, next_cursor) = buffer
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .query_since(cursor);
    let body = serde_json::to_vec(&serde_json::json!({
        "interactions": interactions,
        "next_cursor": next_cursor,
        "server_domain": server_domain,
    }))
    .unwrap_or_default();
    write_poll_json_response(&mut stream, "200 OK", &body, &cancel, deadline).await;
}

impl OastRelayServer {
    pub async fn start(config: OastRelayConfig) -> Result<Self> {
        let domain = config.server_domain.trim().trim_end_matches('.').to_owned();
        ensure!(!domain.is_empty(), "OAST 中继服务域名不能为空");
        let oast_cfg = SelfHostedOastConfig {
            enabled: false,
            polling_url: None,
            server_domain: Some(domain.clone()),
            authorization_header: None,
            poll_interval_ms: super::DEFAULT_SELF_HOSTED_POLL_INTERVAL_MS,
            bind_address: Some(config.bind_address.to_string()),
            http_port: Some(config.http_port),
            dns_port: Some(config.dns_port),
            dns_answer_ip: config.dns_answer_ip.map(|ip| ip.to_string()),
            allow_public_cloud_domains: false,
        };
        let (events_tx, mut events_rx) = mpsc::channel(256);
        let local = LocalOastServer::start_with_config(events_tx, oast_cfg)
            .await
            .context("启动 OAST 中继捕获监听失败")?;
        let poll_listener = TcpListener::bind((config.bind_address, config.poll_port))
            .await
            .context("启动 OAST 中继轮询端口失败")?;
        let poll_addr = poll_listener.local_addr()?;

        let buffer = Arc::new(RwLock::new(RelayBuffer::new(
            config.max_buffered_interactions,
        )));
        let cancel = CancellationToken::new();
        let connections = TaskTracker::new();

        let buf_collector = Arc::clone(&buffer);
        let collector_cancel = cancel.clone();
        let collector_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    () = collector_cancel.cancelled() => break,
                    next = events_rx.recv() => match next {
                        Some(item) => {
                            buf_collector
                                .write()
                                .unwrap_or_else(PoisonError::into_inner)
                                .push(item);
                        }
                        None => break,
                    }
                }
            }
        });

        let poll_buf = Arc::clone(&buffer);
        let poll_cancel = cancel.clone();
        let poll_conns = connections.clone();
        let expected_token = config.auth_token.clone();
        let poll_domain = domain.clone();
        let poll_task = tokio::spawn(async move {
            let semaphore = Arc::new(Semaphore::new(MAX_HTTP_CONCURRENCY));
            loop {
                let permit = tokio::select! {
                    biased;
                    () = poll_cancel.cancelled() => break,
                    acq = semaphore.clone().acquire_owned() => match acq {
                        Ok(p) => p,
                        Err(_) => break,
                    }
                };
                let accepted = tokio::select! {
                    biased;
                    () = poll_cancel.cancelled() => break,
                    res = poll_listener.accept() => res,
                };
                match accepted {
                    Ok((stream, _peer)) => {
                        poll_conns.spawn(handle_poll_connection(
                            stream,
                            expected_token.clone(),
                            poll_domain.clone(),
                            Arc::clone(&poll_buf),
                            poll_cancel.clone(),
                            permit,
                        ));
                    }
                    Err(_) => {
                        drop(permit);
                        if poll_cancel.is_cancelled() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(Self {
            local: Some(local),
            poll_addr,
            server_domain: domain,
            cancel,
            connections,
            collector_task: Some(collector_task),
            poll_task: Some(poll_task),
        })
    }

    pub fn http_addr(&self) -> SocketAddr {
        self.local.as_ref().expect("relay running").http_addr()
    }

    pub fn dns_addr(&self) -> SocketAddr {
        self.local.as_ref().expect("relay running").dns_addr()
    }

    pub fn poll_addr(&self) -> SocketAddr {
        self.poll_addr
    }

    pub fn polling_url(&self) -> String {
        format!("http://{}/poll", self.poll_addr)
    }

    pub fn server_domain(&self) -> &str {
        &self.server_domain
    }

    pub async fn stop(mut self) -> Result<()> {
        self.cancel.cancel();
        self.connections.close();
        let deadline = Instant::now() + STOP_TIMEOUT;
        if timeout_at(deadline, self.connections.wait()).await.is_err() {
            if let Some(task) = self.collector_task.take() {
                task.abort();
            }
            if let Some(task) = self.poll_task.take() {
                task.abort();
            }
            if let Some(local) = self.local.take() {
                let _ = local.stop().await;
            }
            bail!("OAST relay stop deadline exceeded");
        }
        let mut res = Ok(());
        if let Some(mut task) = self.poll_task.take()
            && timeout_at(deadline, &mut task).await.is_err()
        {
            task.abort();
            res = Err(anyhow!("OAST relay poll task stop timeout"));
        }
        if let Some(mut task) = self.collector_task.take()
            && timeout_at(deadline, &mut task).await.is_err()
        {
            task.abort();
            res = Err(anyhow!("OAST relay collector stop timeout"));
        }
        if let Some(local) = self.local.take() {
            let local_res = local.stop().await;
            res = res.and(local_res);
        }
        res
    }
}

impl Drop for OastRelayServer {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.connections.close();
        if let Some(task) = self.collector_task.take() {
            task.abort();
        }
        if let Some(task) = self.poll_task.take() {
            task.abort();
        }
    }
}
