use super::*;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SOCKS5_VERSION: u8 = 0x05;
const AUTH_NO_AUTH: u8 = 0x00;
const AUTH_NO_ACCEPTABLE: u8 = 0xff;
const CMD_CONNECT: u8 = 0x01;
const ATYP_IPV4: u8 = 0x01;
const ATYP_DOMAIN: u8 = 0x03;
const ATYP_IPV6: u8 = 0x04;
const REP_SUCCEEDED: u8 = 0x00;
const REP_GENERAL_FAILURE: u8 = 0x01;
const REP_COMMAND_NOT_SUPPORTED: u8 = 0x07;
const REP_ADDRESS_TYPE_NOT_SUPPORTED: u8 = 0x08;

fn is_benign_pre_handshake_io(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::NotConnected
    )
}

pub(super) async fn peek_first_byte(
    stream: &tokio::net::TcpStream,
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<Option<u8>, ProxyError> {
    let mut first = [0u8; 1];
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(ProxyError::Cancelled),
        res = tokio::time::timeout(timeout, stream.peek(&mut first)) => {
            match res {
                Ok(Ok(1)) => Ok(Some(first[0])),
                Ok(Ok(_)) => Ok(None),
                Ok(Err(err)) if is_benign_pre_handshake_io(&err) => Ok(None),
                Ok(Err(err)) => Err(ProxyError::network(err)),
                Err(_) => Err(ProxyError::Timeout),
            }
        }
    }
}

pub(super) const SOCKS5_LEAD_BYTE: u8 = SOCKS5_VERSION;
pub(super) const TLS_CLIENT_HELLO_LEAD_BYTE: u8 = 0x16;

pub(super) async fn serve_socks5(
    mut stream: tokio::net::TcpStream,
    state: Arc<State>,
    permit: Arc<tokio::sync::OwnedSemaphorePermit>,
) {
    let _permit = permit;
    let handshake = tokio::select! {
        biased;
        _ = state.cancel.cancelled() => return,
        res = tokio::time::timeout(state.config.limits.request_timeout, accept_connect(&mut stream)) => res,
    };
    let (host, port, authority) = match handshake {
        Ok(Ok(target)) => target,
        Ok(Err(error)) => {
            let listen_host = crate::route::format_host_for_authority(
                &state.config.network.listen.ip().to_string(),
            );
            let failed = HttpRequest {
                method: "CONNECT".into(),
                url: format!("socks5://{listen_host}:{}", state.listener_port),
                version: HttpVersion::Http11,
                headers: Vec::new(),
                body: Vec::new(),
                trailers: Vec::new(),
            };
            state.failure(failed, &error).await;
            return;
        }
        Err(_) => {
            let listen_host = crate::route::format_host_for_authority(
                &state.config.network.listen.ip().to_string(),
            );
            let failed = HttpRequest {
                method: "CONNECT".into(),
                url: format!("socks5://{listen_host}:{}", state.listener_port),
                version: HttpVersion::Http11,
                headers: Vec::new(),
                body: Vec::new(),
                trailers: Vec::new(),
            };
            state.failure(failed, &ProxyError::Timeout).await;
            return;
        }
    };

    let mut failed_request = HttpRequest {
        method: "CONNECT".into(),
        url: authority.clone(),
        version: mio_plugin_api::HttpVersion::Http11,
        headers: Vec::new(),
        body: Vec::new(),
        trailers: Vec::new(),
    };

    let is_tls = match peek_first_byte(
        &stream,
        state.config.limits.request_timeout,
        &state.cancel,
    )
    .await
    {
        Ok(Some(byte)) => byte == TLS_CLIENT_HELLO_LEAD_BYTE,
        Ok(None) | Err(ProxyError::Cancelled) => return,
        Err(error) => {
            state.failure(failed_request, &error).await;
            return;
        }
    };

    if is_tls {
        let target_is_ip = crate::route::normalize_host(&host)
            .parse::<IpAddr>()
            .is_ok();
        if ssl_passthrough::should_pass_through(&state, &host, port) {
            let (pt_host, pt_port) = state.effective_passthrough_target(&host, port);
            if let Err(error) =
                ssl_passthrough::relay_stream(stream, &pt_host, pt_port, &state).await
                && !matches!(error, ProxyError::Cancelled)
            {
                state.failure(failed_request, &error).await;
            }
            return;
        }
        if target_is_ip && state.dynamic.borrow().ssl_pass_through.is_some() {
            let mut peek_buf = [0u8; 4096];
            let peeked = tokio::select! {
                biased;
                _ = state.cancel.cancelled() => return,
                res = tokio::time::timeout(
                    state.config.limits.request_timeout,
                    stream.peek(&mut peek_buf),
                ) => res,
            };
            let peeked_len = match peeked {
                Ok(Ok(n)) if n > 0 => n,
                Ok(Ok(_)) => return,
                Ok(Err(err)) if is_benign_pre_handshake_io(&err) => return,
                Ok(Err(err)) => {
                    state.failure(failed_request, &ProxyError::network(err)).await;
                    return;
                }
                Err(_) => {
                    state.failure(failed_request, &ProxyError::Timeout).await;
                    return;
                }
            };
            if let Some(sni) = listener::parse_client_hello_sni(&peek_buf[..peeked_len])
                && ssl_passthrough::should_pass_through(&state, &sni, port)
            {
                let (pt_host, pt_port) = state.effective_passthrough_target(&sni, port);
                failed_request.url = format!(
                    "{}:{pt_port}",
                    crate::route::format_host_for_authority(&pt_host)
                );
                if let Err(error) =
                    ssl_passthrough::relay_stream(stream, &pt_host, pt_port, &state).await
                    && !matches!(error, ProxyError::Cancelled)
                {
                    state.failure(failed_request, &error).await;
                }
                return;
            }
        }

        let ca = state.dynamic.borrow().ca.clone();
        let tls_options = state.listener_tls_options();
        let observed_sni = Arc::new(Mutex::new(None::<String>));
        let config = match tokio::time::timeout(
            state.config.limits.request_timeout,
            ca.connect_server_config(&host, &tls_options, observed_sni.clone()),
        )
        .await
        {
            Ok(Ok(cfg)) => cfg,
            Ok(Err(error)) => {
                state.failure(failed_request, &error).await;
                return;
            }
            Err(_) => {
                state.failure(failed_request, &ProxyError::Timeout).await;
                return;
            }
        };

        let io = GracefulIo::new(
            Box::pin(state.cancel.clone().cancelled_owned()),
            TcpStream::new(stream),
        );
        let served_any = Arc::new(AtomicBool::new(false));
        let served_flag = served_any.clone();
        let handler_state = state.clone();
        let sni_for_req = observed_sni.clone();
        let fallback_authority = authority.clone();
        let service = service_fn(move |request: Request| {
            served_flag.store(true, Ordering::Relaxed);
            let state = handler_state.clone();
            let effective_authority = if target_is_ip {
                if let Some(sni) = crate::lock_unpoison(&sni_for_req).clone() {
                    listener::format_tunnel_authority(&sni, port, 443)
                } else {
                    fallback_authority.clone()
                }
            } else {
                fallback_authority.clone()
            };
            async move {
                Ok::<_, Infallible>(
                    listener::handle_http_with_ingress(
                        request,
                        state,
                        Some(message::TunnelTarget::Tls(effective_authority)),
                        true,
                    )
                    .await,
                )
            }
        });
        let failure_state = state.clone();
        let failure_host = host.clone();
        let sni_for_err = observed_sni.clone();
        let run = async {
            let outcome = state
                .serve_tls_http_connection(io, config, service, false)
                .await
                .or_else(|err| {
                    if served_any.load(Ordering::Relaxed)
                        || listener::is_benign_tunnel_disconnect(err.as_ref())
                    {
                        Ok(())
                    } else {
                        let observed = crate::lock_unpoison(&sni_for_err).clone();
                        let effective_host = if let Some(ref sni) = observed {
                            failed_request.url = format!(
                                "{}:{port}",
                                crate::route::format_host_for_authority(sni)
                            );
                            sni.clone()
                        } else {
                            failure_host
                        };
                        failure_state.notify_client_tls_failure(effective_host, port);
                        Err(err)
                    }
                })
                .map_err(ProxyError::network);
            if let Err(error) = outcome
                && !matches!(error, ProxyError::Cancelled)
                && !state.cancel.is_cancelled()
            {
                state.failure(failed_request, &error).await;
            }
        };
        tokio::select! {
            biased;
            _ = state.cancel.cancelled() => {},
            _ = run => {},
        }
    } else {
        let io = GracefulIo::new(
            Box::pin(state.cancel.clone().cancelled_owned()),
            TcpStream::new(stream),
        );
        let handler_state = state.clone();
        let plain_authority = listener::format_tunnel_authority(&host, port, 80);
        let service = service_fn(move |request: Request| {
            let state = handler_state.clone();
            let plain_authority = plain_authority.clone();
            async move {
                Ok::<_, Infallible>(
                    listener::handle_http_with_ingress(
                        request,
                        state,
                        Some(message::TunnelTarget::PlainHttp(plain_authority)),
                        true,
                    )
                    .await,
                )
            }
        });
        state.serve_http_connection(io, service).await;
    }
}

async fn accept_connect(
    stream: &mut tokio::net::TcpStream,
) -> Result<(String, u16, String), ProxyError> {
    let mut greeting = [0u8; 2];
    stream
        .read_exact(&mut greeting)
        .await
        .map_err(ProxyError::network)?;
    if greeting[0] != SOCKS5_VERSION {
        return Err(ProxyError::invalid("SOCKS5 版本不匹配"));
    }
    let nmethods = greeting[1] as usize;
    if nmethods == 0 {
        let _ = stream
            .write_all(&[SOCKS5_VERSION, AUTH_NO_ACCEPTABLE])
            .await;
        return Err(ProxyError::invalid("SOCKS5 客户端未提供认证方法"));
    }
    let mut methods = vec![0u8; nmethods];
    stream
        .read_exact(&mut methods)
        .await
        .map_err(ProxyError::network)?;
    if !methods.contains(&AUTH_NO_AUTH) {
        let _ = stream
            .write_all(&[SOCKS5_VERSION, AUTH_NO_ACCEPTABLE])
            .await;
        return Err(ProxyError::invalid("SOCKS5 仅支持本地无认证连接"));
    }
    stream
        .write_all(&[SOCKS5_VERSION, AUTH_NO_AUTH])
        .await
        .map_err(ProxyError::network)?;

    let mut req_head = [0u8; 4];
    stream
        .read_exact(&mut req_head)
        .await
        .map_err(ProxyError::network)?;
    if req_head[0] != SOCKS5_VERSION || req_head[2] != 0x00 {
        let _ = send_socks5_reply(stream, REP_GENERAL_FAILURE).await;
        return Err(ProxyError::invalid("SOCKS5 请求头无效"));
    }
    if req_head[1] != CMD_CONNECT {
        let _ = send_socks5_reply(stream, REP_COMMAND_NOT_SUPPORTED).await;
        return Err(ProxyError::invalid("SOCKS5 仅支持 CONNECT 命令"));
    }

    let (host, bracketed) = match req_head[3] {
        ATYP_IPV4 => {
            let mut octets = [0u8; 4];
            stream
                .read_exact(&mut octets)
                .await
                .map_err(ProxyError::network)?;
            let ip = Ipv4Addr::from(octets).to_string();
            (ip.clone(), ip)
        }
        ATYP_DOMAIN => {
            let mut len_buf = [0u8; 1];
            stream
                .read_exact(&mut len_buf)
                .await
                .map_err(ProxyError::network)?;
            let len = len_buf[0] as usize;
            if len == 0 {
                let _ = send_socks5_reply(stream, REP_GENERAL_FAILURE).await;
                return Err(ProxyError::invalid("SOCKS5 域名为空"));
            }
            let mut domain_bytes = vec![0u8; len];
            stream
                .read_exact(&mut domain_bytes)
                .await
                .map_err(ProxyError::network)?;
            let domain = String::from_utf8(domain_bytes)
                .map_err(|_| ProxyError::invalid("SOCKS5 域名不是有效 UTF-8"))?;
            (domain.clone(), domain)
        }
        ATYP_IPV6 => {
            let mut octets = [0u8; 16];
            stream
                .read_exact(&mut octets)
                .await
                .map_err(ProxyError::network)?;
            let ip = Ipv6Addr::from(octets).to_string();
            (ip.clone(), format!("[{ip}]"))
        }
        _ => {
            let _ = send_socks5_reply(stream, REP_ADDRESS_TYPE_NOT_SUPPORTED).await;
            return Err(ProxyError::invalid("SOCKS5 地址类型不支持"));
        }
    };

    let mut port_buf = [0u8; 2];
    stream
        .read_exact(&mut port_buf)
        .await
        .map_err(ProxyError::network)?;
    let port = u16::from_be_bytes(port_buf);
    if port == 0 {
        let _ = send_socks5_reply(stream, REP_GENERAL_FAILURE).await;
        return Err(ProxyError::invalid("SOCKS5 目标端口无效"));
    }

    send_socks5_reply(stream, REP_SUCCEEDED)
        .await
        .map_err(ProxyError::network)?;
    let authority = format!("{bracketed}:{port}");
    Ok((host, port, authority))
}

async fn send_socks5_reply(stream: &mut tokio::net::TcpStream, rep: u8) -> std::io::Result<()> {
    stream
        .write_all(&[SOCKS5_VERSION, rep, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0])
        .await
}
