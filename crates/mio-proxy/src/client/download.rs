use std::time::Duration;

use mio_plugin_api::{Header, HttpRequest, HttpVersion};
use rama::{graceful::Shutdown, http::body::util::BodyExt, rt::Executor};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use super::{SendConfig, SendNetworkConfig, exchange_model_with_auth};
use crate::{ProxyError, UpstreamRoute, UpstreamTlsConfig, message};

const ERROR_SNIPPET_MAX: usize = 256;

#[derive(Clone, Debug)]
pub struct DownloadConfig {
    pub network: SendNetworkConfig,
    pub tls: UpstreamTlsConfig,
    pub max_body_bytes: usize,
    pub timeout: Duration,
    pub max_redirects: usize,
    pub headers: Vec<Header>,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            network: SendNetworkConfig::default(),
            tls: UpstreamTlsConfig::default(),
            max_body_bytes: 16 * 1024 * 1024,
            timeout: Duration::from_secs(60),
            max_redirects: 5,
            headers: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadOutcome {
    pub final_url: String,
    pub status: u16,
    pub version: HttpVersion,
    pub headers: Vec<Header>,
    pub content_length: Option<u64>,
    pub bytes_written: u64,
    pub error_snippet: Vec<u8>,
}

pub trait DownloadRouteResolver: Send {
    fn resolve_route(&mut self, target_url: &str) -> impl Future<Output = UpstreamRoute> + Send;
}

impl DownloadRouteResolver for UpstreamRoute {
    async fn resolve_route(&mut self, _target_url: &str) -> UpstreamRoute {
        self.clone()
    }
}

impl<T: DownloadRouteResolver + ?Sized> DownloadRouteResolver for &mut T {
    async fn resolve_route(&mut self, target_url: &str) -> UpstreamRoute {
        (**self).resolve_route(target_url).await
    }
}

pub trait DownloadSink: Send {
    fn on_response_head(
        &mut self,
        _status: u16,
        _headers: &[Header],
        _content_length: Option<u64>,
    ) -> impl Future<Output = Result<(), ProxyError>> + Send {
        async { Ok(()) }
    }

    fn write_chunk(
        &mut self,
        chunk: &[u8],
        total_received: u64,
        content_length: Option<u64>,
    ) -> impl Future<Output = Result<(), ProxyError>> + Send;

    fn flush(&mut self) -> impl Future<Output = Result<(), ProxyError>> + Send {
        async { Ok(()) }
    }
}

struct AsyncWriteSink<'a, W, F> {
    writer: &'a mut W,
    on_progress: F,
}

impl<W, F> DownloadSink for AsyncWriteSink<'_, W, F>
where
    W: AsyncWrite + Unpin + Send,
    F: FnMut(u64, Option<u64>) + Send,
{
    async fn write_chunk(
        &mut self,
        chunk: &[u8],
        total_received: u64,
        content_length: Option<u64>,
    ) -> Result<(), ProxyError> {
        self.writer
            .write_all(chunk)
            .await
            .map_err(ProxyError::network)?;
        (self.on_progress)(total_received, content_length);
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), ProxyError> {
        self.writer.flush().await.map_err(ProxyError::network)
    }
}

pub async fn download_to_writer<W, F>(
    initial_url: &str,
    config: DownloadConfig,
    writer: &mut W,
    cancel: CancellationToken,
    on_progress: F,
) -> Result<DownloadOutcome, ProxyError>
where
    W: AsyncWrite + Unpin + Send,
    F: FnMut(u64, Option<u64>) + Send,
{
    let route = config.network.upstream_route.clone();
    let mut sink = AsyncWriteSink {
        writer,
        on_progress,
    };
    download_to_sink(initial_url, config, route, &mut sink, cancel).await
}

fn build_download_request(
    parsed_url: &url::Url,
    base_headers: &[Header],
    strip_sensitive_auth: bool,
) -> Result<HttpRequest, ProxyError> {
    let host = parsed_url
        .host_str()
        .ok_or_else(|| ProxyError::invalid("下载 URL 缺少主机名"))?;
    let host_hdr = match parsed_url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    let mut headers = Vec::with_capacity(base_headers.len() + 1);
    for header in base_headers {
        if header.name.eq_ignore_ascii_case("host") {
            continue;
        }
        if strip_sensitive_auth
            && (header.name.eq_ignore_ascii_case("authorization")
                || header.name.eq_ignore_ascii_case("cookie"))
        {
            continue;
        }
        headers.push(header.clone());
    }
    headers.push(Header {
        name: "Host".into(),
        value: host_hdr.into_bytes(),
    });
    Ok(HttpRequest {
        method: "GET".into(),
        url: parsed_url.to_string(),
        version: HttpVersion::Http11,
        headers,
        body: Vec::new(),
        trailers: Vec::new(),
    })
}

pub async fn download_to_sink<R, S>(
    initial_url: &str,
    config: DownloadConfig,
    mut route_resolver: R,
    sink: &mut S,
    cancel: CancellationToken,
) -> Result<DownloadOutcome, ProxyError>
where
    R: DownloadRouteResolver,
    S: DownloadSink,
{
    let mut current_url = initial_url.to_owned();
    let initial_parsed = url::Url::parse(&current_url)
        .map_err(|err| ProxyError::invalid(format!("无效的下载 URL：{err}")))?;
    let initial_origin = initial_parsed.origin().ascii_serialization();
    let deadline = tokio::time::Instant::now() + config.timeout;

    for _hop in 0..=config.max_redirects {
        if cancel.is_cancelled() {
            return Err(ProxyError::Cancelled);
        }
        let now = tokio::time::Instant::now();
        let Some(remaining) = deadline.checked_duration_since(now) else {
            return Err(ProxyError::Timeout);
        };
        if remaining.is_zero() {
            return Err(ProxyError::Timeout);
        }

        let parsed_url = url::Url::parse(&current_url)
            .map_err(|err| ProxyError::invalid(format!("无效的下载 URL：{err}")))?;
        let strip_sensitive_auth = parsed_url.origin().ascii_serialization() != initial_origin;
        let request = build_download_request(&parsed_url, &config.headers, strip_sensitive_auth)?;
        let upstream_route = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ProxyError::Cancelled),
            resolved = tokio::time::timeout_at(deadline, route_resolver.resolve_route(&current_url)) => {
                resolved.map_err(|_| ProxyError::Timeout)?
            }
        };

        let mut send_cfg = SendConfig::default();
        send_cfg.network.upstream_route = upstream_route;
        send_cfg.network.hostname_overrides = config.network.hostname_overrides.clone();
        send_cfg.tls = config.tls.clone();
        send_cfg.limits.timeout = remaining;
        send_cfg.limits.max_body_bytes = config.max_body_bytes;
        send_cfg.http.use_connection_pool = false;

        let req_cancel = cancel.child_token();
        let shutdown = Shutdown::new(req_cancel.clone().cancelled_owned());
        let _cancel_on_drop = req_cancel.clone().drop_guard();
        let exec = Executor::graceful(shutdown.guard());

        let exchange_result = tokio::select! {
            biased;
            _ = req_cancel.cancelled() => Err(ProxyError::Cancelled),
            res = exchange_model_with_auth(request, &send_cfg, deadline, req_cancel.clone(), exec) => {
                res.map(|(resp, _)| resp)
            }
        };
        let response = match exchange_result {
            Ok(resp) => resp,
            Err(err) => {
                req_cancel.cancel();
                shutdown.shutdown().await;
                return Err(err);
            }
        };

        let mut header_budget = (
            crate::StreamedResponse::MAX_HEADERS,
            crate::StreamedResponse::MAX_HEAD_BYTES,
        );
        if let Err(err) = message::consume_header_budget(response.headers(), &mut header_budget) {
            drop(response);
            req_cancel.cancel();
            shutdown.shutdown().await;
            return Err(err);
        }

        let status = response.status().as_u16();
        let version = message::model_version(response.version());
        let response_headers = message::headers(response.headers());
        let content_length = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok());

        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let loc = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            drop(response);
            req_cancel.cancel();
            shutdown.shutdown().await;
            let loc = loc.ok_or_else(|| ProxyError::invalid("重定向响应缺少 Location 响应头"))?;
            let next = parsed_url
                .join(&loc)
                .map_err(|err| ProxyError::invalid(format!("无效的重定向地址：{err}")))?;
            if !matches!(next.scheme(), "http" | "https") {
                return Err(ProxyError::invalid("重定向目标仅支持 http 或 https"));
            }
            current_url = next.to_string();
            continue;
        }

        if !(200..300).contains(&status) {
            let mut body = response.into_body();
            let mut snippet = Vec::with_capacity(ERROR_SNIPPET_MAX);
            while snippet.len() < ERROR_SNIPPET_MAX {
                let frame = tokio::select! {
                    biased;
                    _ = req_cancel.cancelled() => break,
                    _ = tokio::time::sleep_until(deadline) => break,
                    frame = body.frame() => frame,
                };
                let Some(Ok(frame)) = frame else {
                    break;
                };
                if let Ok(data) = frame.into_data() {
                    let take = (ERROR_SNIPPET_MAX - snippet.len()).min(data.len());
                    snippet.extend_from_slice(&data[..take]);
                }
            }
            drop(body);
            req_cancel.cancel();
            shutdown.shutdown().await;
            return Ok(DownloadOutcome {
                final_url: current_url,
                status,
                version,
                headers: response_headers,
                content_length,
                bytes_written: 0,
                error_snippet: snippet,
            });
        }

        if let Some(len) = content_length
            && len > config.max_body_bytes as u64
        {
            drop(response);
            req_cancel.cancel();
            shutdown.shutdown().await;
            return Err(ProxyError::BodyLimit {
                limit: config.max_body_bytes,
            });
        }

        let head_res = tokio::select! {
            biased;
            _ = req_cancel.cancelled() => Err(ProxyError::Cancelled),
            _ = tokio::time::sleep_until(deadline) => Err(ProxyError::Timeout),
            res = sink.on_response_head(status, &response_headers, content_length) => res,
        };
        if let Err(err) = head_res {
            drop(response);
            req_cancel.cancel();
            shutdown.shutdown().await;
            return Err(err);
        }

        let mut received = 0u64;
        let mut body = response.into_body();
        let mut stream_result: Result<(), ProxyError> = loop {
            tokio::select! {
                biased;
                _ = req_cancel.cancelled() => break Err(ProxyError::Cancelled),
                _ = tokio::time::sleep_until(deadline) => break Err(ProxyError::Timeout),
                frame = body.frame() => match frame {
                    None => break Ok(()),
                    Some(Err(err)) => {
                        break Err(if req_cancel.is_cancelled() {
                            ProxyError::Cancelled
                        } else {
                            ProxyError::network(err)
                        });
                    }
                    Some(Ok(frame)) => match frame.into_data() {
                        Ok(data) if !data.is_empty() => {
                            let Some(next) = received
                                .checked_add(data.len() as u64)
                                .filter(|&n| n <= config.max_body_bytes as u64)
                            else {
                                break Err(ProxyError::BodyLimit {
                                    limit: config.max_body_bytes,
                                });
                            };
                            received = next;
                            let write_res = tokio::select! {
                                biased;
                                _ = req_cancel.cancelled() => Err(ProxyError::Cancelled),
                                _ = tokio::time::sleep_until(deadline) => Err(ProxyError::Timeout),
                                res = sink.write_chunk(&data, received, content_length) => res,
                            };
                            if let Err(err) = write_res {
                                break Err(err);
                            }
                        }
                        Ok(_) => {}
                        Err(frame) => {
                            if let Ok(trailers) = frame.into_trailers()
                                && let Err(err) =
                                    message::consume_header_budget(&trailers, &mut header_budget)
                            {
                                break Err(err);
                            }
                        }
                    }
                }
            }
        };

        if stream_result.is_ok() {
            stream_result = tokio::select! {
                biased;
                _ = req_cancel.cancelled() => Err(ProxyError::Cancelled),
                _ = tokio::time::sleep_until(deadline) => Err(ProxyError::Timeout),
                res = sink.flush() => res,
            };
        }

        drop(body);
        req_cancel.cancel();
        shutdown.shutdown().await;
        stream_result?;

        return Ok(DownloadOutcome {
            final_url: current_url,
            status,
            version,
            headers: response_headers,
            content_length,
            bytes_written: received,
            error_snippet: Vec::new(),
        });
    }

    Err(ProxyError::Network(format!(
        "重定向次数过多（超过 {} 次）",
        config.max_redirects
    )))
}
