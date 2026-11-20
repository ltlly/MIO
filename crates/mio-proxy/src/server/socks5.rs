use super::*;
use rama::http::header::{HOST, HeaderValue};
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
        res = tokio::time::timeout(state.config.request_timeout, accept_connect(&mut stream)) => res,
    };
    let (host, port, authority) = match handshake {
        Ok(Ok(target)) => target,
        _ => return,
    };

    let failed_request = HttpRequest {
        method: "CONNECT".into(),
        url: authority.clone(),
        version: mio_plugin_api::HttpVersion::Http11,
        headers: Vec::new(),
        body: Vec::new(),
        trailers: Vec::new(),
    };

    let mut sniff = [0u8; 1];
    let sniffed = tokio::select! {
        biased;
        _ = state.cancel.cancelled() => return,
        res = tokio::time::timeout(state.config.request_timeout, stream.peek(&mut sniff)) => res,
    };
    let is_tls = match sniffed {
        Ok(Ok(1)) => sniff[0] == 0x16,
        Ok(Ok(_)) => return,
        Ok(Err(_)) | Err(_) => return,
    };

    if is_tls {
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

        let tls_options = state.listener_tls_options();
        let config = match tokio::time::timeout(
            state.config.request_timeout,
            state
                .config
                .ca
                .server_config_with_options(&host, &tls_options),
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
        let service = service_fn(move |request: Request| {
            served_flag.store(true, Ordering::Relaxed);
            let state = handler_state.clone();
            let authority = authority.clone();
            async move {
                Ok::<_, Infallible>(
                    listener::handle_http_with_ingress(request, state, Some(authority), true).await,
                )
            }
        });
        let failure_state = state.clone();
        let failure_host = host.clone();
        let run = async {
            let serve_result = if state.config.enable_http2 {
                let mut server = HttpServer::auto(state.exec.clone());
                server
                    .http1_mut()
                    .set_keep_alive(state.config.http1_keep_alive);
                server.h2_mut().set_max_concurrent_streams(
                    state.config.max_connections.min(u32::MAX as usize) as u32,
                );
                TlsAcceptorService::new(config, server.service(service), false)
                    .serve(io)
                    .await
            } else {
                let mut server = HttpServer::new_http1(state.exec.clone());
                server
                    .http1_mut()
                    .set_keep_alive(state.config.http1_keep_alive);
                TlsAcceptorService::new(config, server.service(service), false)
                    .serve(io)
                    .await
            };
            serve_result
                .or_else(|err| {
                    if served_any.load(Ordering::Relaxed)
                        || listener::is_benign_tunnel_disconnect(err.as_ref())
                    {
                        Ok(())
                    } else {
                        failure_state.notify_client_tls_failure(failure_host, port);
                        Err(err)
                    }
                })
                .map_err(ProxyError::network)
        };
        tokio::select! {
            biased;
            _ = state.cancel.cancelled() => {},
            result = run => {
                if let Err(error) = result {
                    state.failure(failed_request, &error).await;
                }
            }
        }
    } else {
        let io = GracefulIo::new(
            Box::pin(state.cancel.clone().cancelled_owned()),
            TcpStream::new(stream),
        );
        let handler_state = state.clone();
        let fallback_authority = authority.clone();
        let service = service_fn(move |mut request: Request| {
            let state = handler_state.clone();
            let fallback_authority = fallback_authority.clone();
            async move {
                if request.uri().scheme().is_none()
                    && !request.headers().contains_key(HOST)
                    && let Ok(hv) = HeaderValue::from_str(&fallback_authority)
                {
                    request.headers_mut().insert(HOST, hv);
                }
                Ok::<_, Infallible>(
                    listener::handle_http_with_ingress(request, state, None, true).await,
                )
            }
        });
        if state.config.enable_http2 {
            let mut server = HttpServer::auto(state.exec.clone());
            server
                .http1_mut()
                .set_keep_alive(state.config.http1_keep_alive);
            server.h2_mut().set_max_concurrent_streams(
                state.config.max_connections.min(u32::MAX as usize) as u32,
            );
            tokio::select! {
                biased;
                _ = state.cancel.cancelled() => {},
                _ = server.serve(io, service) => {},
            }
        } else {
            let mut server = HttpServer::new_http1(state.exec.clone());
            server
                .http1_mut()
                .set_keep_alive(state.config.http1_keep_alive);
            tokio::select! {
                biased;
                _ = state.cancel.cancelled() => {},
                _ = server.serve(io, service) => {},
            }
        }
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
