use super::*;
use crate::transport::{
    CancellableConnector, MioProxyConnectorLayer, ResponseTiming, apply_hostname_overrides,
};
use rama::{
    extensions::Extensions,
    http::client::EasyHttpWebClient,
    io::Io,
    net::{
        Protocol,
        address::{Host, HostWithPort},
        client::ConnectRequest,
        transport::TransportProtocol,
    },
};

pub(super) fn should_pass_through(state: &State, host: &str, port: u16) -> bool {
    let normalized = host.trim_matches(['[', ']']);
    state
        .ssl_pass_through
        .borrow()
        .as_ref()
        .is_some_and(|matcher| matcher(normalized, port))
}

pub(super) fn spawn_passthrough(
    request: &Request,
    host: String,
    port: u16,
    state: Arc<State>,
    permit: Arc<tokio::sync::OwnedSemaphorePermit>,
    failed_request: HttpRequest,
) -> Result<Response, ProxyError> {
    let parsed_host: Host = host
        .trim_matches(['[', ']'])
        .parse()
        .map_err(|_| ProxyError::invalid("CONNECT 透传目标主机无效"))?;
    let upgrade = handle_upgrade(request);
    let task_state = state.clone();
    state.tasks.spawn(async move {
        let _permit = permit;
        let run = relay_tunnel(upgrade, parsed_host, port, &task_state);
        tokio::select! {
            biased;
            _ = task_state.cancel.cancelled() => {},
            result = run => {
                if let Err(error) = result
                    && !matches!(error, ProxyError::Cancelled)
                {
                    task_state.failure(failed_request, &error).await;
                }
            }
        }
    });
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::OK;
    Ok(response)
}

async fn relay_tunnel(
    upgrade: impl Future<Output = Result<rama::http::io::upgrade::Upgraded, rama::error::BoxError>>,
    host: Host,
    port: u16,
    state: &State,
) -> Result<(), ProxyError> {
    let cancel = state.cancel.child_token();
    let _cancel_guard = cancel.clone().drop_guard();
    let host_text = host.to_string();
    let route = state.resolve_upstream_route_for_host(&host_text);
    let extensions = Extensions::new();
    extensions.insert(route.rama()?);
    apply_hostname_overrides(&extensions, &state.config.hostname_overrides)?;
    let connect_req =
        ConnectRequest::new_with_extensions(HostWithPort::new(host, port), extensions)
            .with_application_protocol(Protocol::HTTPS)
            .with_transport_protocol(TransportProtocol::Tcp);
    let proxy_tls = route.tls_config_with_options(
        &state.config.client_certificates,
        &state.config.upstream_tls_options,
    )?;
    let timing = Arc::new(Mutex::new(ResponseTiming::default()));
    let connector = EasyHttpWebClient::connector_builder()
        .with_default_transport_connector()
        .map_connector(|inner| CancellableConnector {
            inner,
            cancel: cancel.clone(),
            timing: timing.clone(),
        })
        .with_default_dns_connector()
        .with_tls_proxy_support_using_rustls_config(proxy_tls)
        .with_custom_proxy_connector(MioProxyConnectorLayer {
            overrides: state.config.hostname_overrides.clone(),
            timing,
        })
        .build_connector();
    let deadline = tokio::time::Instant::now() + state.config.request_timeout;
    let (mut ingress, mut egress) = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(ProxyError::Cancelled),
        joined = tokio::time::timeout_at(deadline, async {
            let ingress_fut = async { upgrade.await.map_err(ProxyError::network) };
            let egress_fut = async {
                Box::pin(connector.serve(connect_req))
                    .await
                    .map(|established| established.conn)
                    .map_err(ProxyError::network)
            };
            tokio::try_join!(ingress_fut, egress_fut)
        }) => joined.map_err(|_| ProxyError::Timeout)??,
    };
    copy_tunnel(&mut ingress, &mut egress, &cancel).await
}

pub(super) async fn relay_stream(
    mut ingress: tokio::net::TcpStream,
    host: &str,
    port: u16,
    state: &State,
) -> Result<(), ProxyError> {
    let parsed_host: Host = host
        .trim_matches(['[', ']'])
        .parse()
        .map_err(|_| ProxyError::invalid("SOCKS5 透传目标主机无效"))?;
    let cancel = state.cancel.child_token();
    let _cancel_guard = cancel.clone().drop_guard();
    let route = state.resolve_upstream_route_for_host(host);
    let extensions = Extensions::new();
    extensions.insert(route.rama()?);
    apply_hostname_overrides(&extensions, &state.config.hostname_overrides)?;
    let connect_req =
        ConnectRequest::new_with_extensions(HostWithPort::new(parsed_host, port), extensions)
            .with_application_protocol(Protocol::HTTPS)
            .with_transport_protocol(TransportProtocol::Tcp);
    let proxy_tls = route.tls_config_with_options(
        &state.config.client_certificates,
        &state.config.upstream_tls_options,
    )?;
    let timing = Arc::new(Mutex::new(ResponseTiming::default()));
    let connector = EasyHttpWebClient::connector_builder()
        .with_default_transport_connector()
        .map_connector(|inner| CancellableConnector {
            inner,
            cancel: cancel.clone(),
            timing: timing.clone(),
        })
        .with_default_dns_connector()
        .with_tls_proxy_support_using_rustls_config(proxy_tls)
        .with_custom_proxy_connector(MioProxyConnectorLayer {
            overrides: state.config.hostname_overrides.clone(),
            timing,
        })
        .build_connector();
    let deadline = tokio::time::Instant::now() + state.config.request_timeout;
    let mut egress = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(ProxyError::Cancelled),
        connected = tokio::time::timeout_at(deadline, Box::pin(connector.serve(connect_req))) => {
            connected
                .map_err(|_| ProxyError::Timeout)?
                .map(|established| established.conn)
                .map_err(ProxyError::network)?
        }
    };
    copy_tunnel(&mut ingress, &mut egress, &cancel).await
}

async fn copy_tunnel<I, E>(
    ingress: &mut I,
    egress: &mut E,
    cancel: &CancellationToken,
) -> Result<(), ProxyError>
where
    I: Io + Unpin,
    E: Io + Unpin,
{
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(ProxyError::Cancelled),
        result = tokio::io::copy_bidirectional(ingress, egress) => {
            match result {
                Ok(_) => Ok(()),
                Err(_) if cancel.is_cancelled() => Err(ProxyError::Cancelled),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::UnexpectedEof
                            | std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::BrokenPipe
                    ) =>
                {
                    Ok(())
                }
                Err(error) => Err(ProxyError::network(error)),
            }
        }
    }
}
