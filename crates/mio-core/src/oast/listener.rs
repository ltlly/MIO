use super::dns::run_dns_server;
use super::payload::{generate_unique_token, now_unix_ms, truncate_lossy, truncate_str};
use super::*;

struct ParsedHttpRequest {
    method: String,
    target: String,
    body_start: Option<usize>,
    content_length: Option<usize>,
    chunked: bool,
    candidates: Vec<String>,
}

fn parse_bounded_http_request(raw: &[u8]) -> ParsedHttpRequest {
    let first_line_end = raw
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(raw.len());
    let first_line = truncate_lossy(&raw[..first_line_end], SUMMARY_LIMIT);
    let first_line = first_line.trim_end_matches('\r');
    let mut request_line = first_line.split_whitespace();
    let method = request_line.next().map_or_else(
        || "HTTP".to_owned(),
        |part| truncate_str(part, 32).to_owned(),
    );
    let target = request_line.next().map_or_else(
        || "/".to_owned(),
        |part| truncate_str(part, SUMMARY_LIMIT).to_owned(),
    );

    let mut host = None;
    let mut content_length = None;
    let mut chunked = false;
    let mut body_start = None;
    let mut cursor = first_line_end.saturating_add(1);
    while cursor < raw.len() {
        let newline = raw[cursor..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(raw.len(), |offset| cursor + offset);
        let line = &raw[cursor..newline];
        let next_cursor = newline.saturating_add(1).min(raw.len());
        if line == b"\r" || line.is_empty() {
            body_start = Some(next_cursor);
            break;
        }

        let line_text = String::from_utf8_lossy(line);
        if let Some((name, value)) = line_text.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim();
            match name.as_str() {
                "host" => host = Some(value.to_owned()),
                "content-length" => {
                    content_length = value
                        .split(',')
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .parse::<usize>()
                        .ok();
                }
                "transfer-encoding" => {
                    chunked = value.to_ascii_lowercase().contains("chunked");
                }
                _ => {}
            }
        }
        cursor = next_cursor;
    }

    let absolute_url = if target.contains("://") {
        Url::parse(&target).ok()
    } else {
        None
    };
    let mut candidates = vec![target.clone()];
    if let Some(url) = absolute_url.as_ref() {
        let path = if url.path().is_empty() {
            "/"
        } else {
            url.path()
        };
        candidates.push(path.to_owned());
        if let Some(query) = url.query() {
            candidates.push(query.to_owned());
        }
        if let Some(url_host) = url.host_str() {
            candidates.push(url_host.to_owned());
        }
    } else if let Some((path, query)) = target.split_once('?') {
        candidates.push(path.to_owned());
        candidates.push(query.to_owned());
    }
    if let Some(host) = host.as_ref() {
        candidates.push(host.clone());
    }

    ParsedHttpRequest {
        method,
        target,
        body_start,
        content_length,
        chunked,
        candidates,
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn chunked_complete(body: &[u8]) -> bool {
    body.starts_with(b"0\r\n\r\n")
        || body.starts_with(b"0\n\n")
        || body.ends_with(b"\r\n0\r\n\r\n")
        || body.ends_with(b"\n0\n\n")
        || contains_bytes(body, b"\r\n0\r\n\r\n")
}

fn http_complete(raw: &[u8], parsed: &ParsedHttpRequest) -> bool {
    let Some(body_start) = parsed.body_start else {
        return false;
    };
    if parsed.chunked {
        return chunked_complete(&raw[body_start..]);
    }
    if let Some(content_length) = parsed.content_length {
        return raw.len() >= body_start.saturating_add(content_length);
    }
    true
}

async fn read_bounded_http_request(
    stream: &mut TcpStream,
    cancel: &CancellationToken,
    deadline: Instant,
) -> Vec<u8> {
    let mut raw = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    while raw.len() < MAX_RAW_REQUEST_BYTES {
        let to_read = chunk.len().min(MAX_RAW_REQUEST_BYTES - raw.len());
        let read = stream.read(&mut chunk[..to_read]);
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            _ = sleep_until(deadline) => break,
            read_result = read => match read_result {
                Ok(0) => break,
                Ok(read_len) => raw.extend_from_slice(&chunk[..read_len]),
                Err(_) => break,
            },
        }
        let parsed = parse_bounded_http_request(&raw);
        if http_complete(&raw, &parsed) {
            break;
        }
    }
    raw
}

pub(super) async fn send_oast_interaction(
    events_tx: &EventSender,
    cancel: &CancellationToken,
    deadline: Instant,
    interaction: OastInteraction,
) {
    let send = events_tx.send(interaction);
    tokio::select! {
        biased;
        () = cancel.cancelled() => {}
        _ = sleep_until(deadline) => {}
        _ = send => {}
    }
}

async fn write_http_response(
    stream: &mut TcpStream,
    cancel: &CancellationToken,
    deadline: Instant,
) {
    let write = async {
        let _ = stream.write_all(HTTP_RESPONSE).await;
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

async fn handle_http_connection(
    mut stream: TcpStream,
    peer_addr: SocketAddr,
    events_tx: EventSender,
    cancel: CancellationToken,
    issued_tokens: IssuedTokens,
    permit: OwnedSemaphorePermit,
) {
    let _permit = permit;
    let read_deadline = Instant::now() + CONNECTION_TIMEOUT;
    let raw_request = read_bounded_http_request(&mut stream, &cancel, read_deadline).await;
    if raw_request.is_empty() {
        return;
    }

    let parsed = parse_bounded_http_request(&raw_request);
    let token = {
        let book = issued_tokens.read().unwrap_or_else(PoisonError::into_inner);
        book.match_candidates(&parsed.candidates)
    };
    let interaction = OastInteraction {
        id: Uuid::new_v4(),
        token,
        protocol: OastProtocol::Http,
        peer_addr: peer_addr.to_string(),
        summary: format!(
            "{} {}",
            parsed.method,
            truncate_str(&parsed.target, SUMMARY_LIMIT)
        ),
        raw_request,
        observed_unix_ms: now_unix_ms(),
    };
    let io_deadline = Instant::now() + CONNECTION_TIMEOUT;
    send_oast_interaction(&events_tx, &cancel, io_deadline, interaction).await;
    write_http_response(&mut stream, &cancel, io_deadline).await;
}

async fn run_http_server(
    listener: TcpListener,
    events_tx: EventSender,
    cancel: CancellationToken,
    issued_tokens: IssuedTokens,
    connections: TaskTracker,
) {
    let semaphore = Arc::new(Semaphore::new(MAX_HTTP_CONCURRENCY));
    loop {
        let permit = tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            acquired = semaphore.clone().acquire_owned() => match acquired {
                Ok(permit) => permit,
                Err(_) => break,
            },
        };
        let accepted = tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            accepted = listener.accept() => accepted,
        };

        match accepted {
            Ok((stream, peer_addr)) => {
                connections.spawn(handle_http_connection(
                    stream,
                    peer_addr,
                    events_tx.clone(),
                    cancel.clone(),
                    issued_tokens.clone(),
                    permit,
                ));
            }
            Err(_) => {
                drop(permit);
                if cancel.is_cancelled() {
                    break;
                }
            }
        }
    }
}

impl LocalOastServer {
    pub async fn start(events_tx: EventSender) -> Result<Self> {
        Self::start_with_config(events_tx, SelfHostedOastConfig::default()).await
    }

    pub async fn start_with_config(
        events_tx: EventSender,
        config: SelfHostedOastConfig,
    ) -> Result<Self> {
        let _validated_polling_url = config.validate()?;
        let bind_ip = config.effective_bind_ip()?;
        let http_listener = TcpListener::bind((bind_ip, config.http_port.unwrap_or(0))).await?;
        let dns_socket = UdpSocket::bind((bind_ip, config.dns_port.unwrap_or(0))).await?;
        let http_addr = http_listener.local_addr()?;
        let dns_addr = dns_socket.local_addr()?;
        let dns_answer_ip = config.effective_dns_answer_ip(http_addr.ip())?;
        let cancel = CancellationToken::new();
        let connections = TaskTracker::new();
        let issued_tokens = Arc::new(RwLock::new(IssuedTokenBook::default()));
        let http_task = tokio::spawn(run_http_server(
            http_listener,
            events_tx.clone(),
            cancel.clone(),
            issued_tokens.clone(),
            connections.clone(),
        ));
        let dns_task = tokio::spawn(run_dns_server(
            dns_socket,
            events_tx,
            cancel.clone(),
            issued_tokens.clone(),
            dns_answer_ip,
        ));

        Ok(Self {
            http_addr,
            dns_addr,
            dns_answer_ip,
            config,
            issued_tokens,
            cancel,
            connections,
            http_task: Some(http_task),
            dns_task: Some(dns_task),
        })
    }

    pub fn http_addr(&self) -> SocketAddr {
        self.http_addr
    }

    pub fn dns_addr(&self) -> SocketAddr {
        self.dns_addr
    }

    pub fn issue_payload(&mut self, label: impl Into<String>) -> OastPayloadToken {
        let label = label.into();
        let mut book = self
            .issued_tokens
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let token = generate_unique_token(&book.tokens);
        let configured_domain = self
            .config
            .server_domain
            .as_deref()
            .map(str::trim)
            .filter(|domain| !domain.is_empty())
            .map(|domain| domain.trim_end_matches('.'))
            .filter(|domain| !domain.is_empty())
            .map(str::to_ascii_lowercase);
        let dns_domain = configured_domain
            .clone()
            .unwrap_or_else(|| DEFAULT_DNS_DOMAIN.to_owned());
        let http_callback_url = if self.config.enabled
            && let Some(domain) = configured_domain.as_deref()
        {
            let scheme = self
                .config
                .polling_url
                .as_deref()
                .and_then(|raw| Url::parse(raw.trim()).ok())
                .map(|url| {
                    if url.scheme() == "https" {
                        "https"
                    } else {
                        "http"
                    }
                })
                .unwrap_or("http");
            format!("{scheme}://{domain}/oast/{token}")
        } else {
            let callback_host = if self.http_addr.ip().is_unspecified() {
                match self.http_addr {
                    SocketAddr::V4(v4) => SocketAddr::from((self.dns_answer_ip, v4.port())),
                    SocketAddr::V6(v6) => SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, v6.port())),
                }
            } else {
                self.http_addr
            };
            format!("http://{callback_host}/oast/{token}")
        };
        let payload = OastPayloadToken {
            token: token.clone(),
            label,
            http_callback_url,
            dns_qname: format!("{token}.{dns_domain}"),
            created_unix_ms: now_unix_ms(),
        };
        book.insert(payload.clone());
        payload
    }

    pub fn issued_tokens(&self) -> Vec<OastPayloadToken> {
        self.issued_tokens
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .payloads()
    }

    pub fn self_hosted_config(&self) -> &SelfHostedOastConfig {
        &self.config
    }

    pub async fn stop(mut self) -> Result<()> {
        self.cancel.cancel();
        self.connections.close();
        let deadline = Instant::now() + STOP_TIMEOUT;
        match timeout_at(deadline, self.connections.wait()).await {
            Ok(()) => {}
            Err(_) => {
                if let Some(task) = self.http_task.take() {
                    task.abort();
                }
                if let Some(task) = self.dns_task.take() {
                    task.abort();
                }
                bail!("OAST stop deadline exceeded");
            }
        }

        let mut result = Ok(());
        if let Some(mut task) = self.http_task.take() {
            match timeout_at(deadline, &mut task).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => result = Err(anyhow!("OAST HTTP task failed: {error}")),
                Err(_) => {
                    task.abort();
                    result = Err(anyhow!("OAST stop deadline exceeded"));
                }
            }
        }
        if let Some(mut task) = self.dns_task.take() {
            match timeout_at(deadline, &mut task).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => result = Err(anyhow!("OAST DNS task failed: {error}")),
                Err(_) => {
                    task.abort();
                    result = Err(anyhow!("OAST stop deadline exceeded"));
                }
            }
        }
        result
    }
}

impl Drop for LocalOastServer {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.connections.close();
        if let Some(task) = self.http_task.take() {
            task.abort();
        }
        if let Some(task) = self.dns_task.take() {
            task.abort();
        }
    }
}
