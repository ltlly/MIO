use super::*;
use crate::transport::{apply_hostname_overrides, build_passthrough_connector};
use rama::{
    extensions::Extensions,
    io::Io,
    net::{
        Protocol,
        address::{Host, HostWithPort},
        client::{ConnectRequest, ConnectorService},
        transport::TransportProtocol,
    },
};

pub(super) fn should_pass_through(state: &State, host: &str, port: u16) -> bool {
    let normalized = crate::route::strip_host_brackets(host);
    state
        .dynamic
        .borrow()
        .ssl_pass_through
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
    let parsed_host: Host = crate::route::strip_host_brackets(&host)
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

async fn connect_passthrough_egress(
    host: Host,
    port: u16,
    state: &State,
    cancel: &CancellationToken,
) -> Result<impl Io + Unpin, ProxyError> {
    let host_text = host.to_string();
    let route = state.resolve_upstream_route_for_host(&host_text);
    let extensions = Extensions::new();
    extensions.insert(route.rama()?);
    apply_hostname_overrides(&extensions, &state.config.network.hostname_overrides)?;
    let connect_req =
        ConnectRequest::new_with_extensions(HostWithPort::new(host, port), extensions)
            .with_application_protocol(Protocol::HTTPS)
            .with_transport_protocol(TransportProtocol::Tcp);
    let proxy_tls = route.tls_config_with_options(
        &state.config.tls.upstream.client_certificates,
        &state.config.tls.upstream.options,
    )?;
    let connector = build_passthrough_connector(
        proxy_tls,
        &state.config.network.hostname_overrides,
        cancel.clone(),
    )?;
    let deadline = tokio::time::Instant::now() + state.config.limits.request_timeout;
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(ProxyError::Cancelled),
        connected = tokio::time::timeout_at(deadline, Box::pin(connector.connect(connect_req))) => {
            connected
                .map_err(|_| ProxyError::Timeout)?
                .map(|established| established.conn)
                .map_err(ProxyError::network)
        }
    }
}

async fn relay_tunnel(
    upgrade: impl Future<Output = Result<rama::http::io::upgrade::Upgraded, rama::error::BoxError>>,
    host: Host,
    port: u16,
    state: &State,
) -> Result<(), ProxyError> {
    let cancel = state.cancel.child_token();
    let _cancel_guard = cancel.clone().drop_guard();
    let deadline = tokio::time::Instant::now() + state.config.limits.request_timeout;
    let (mut ingress, mut egress) = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(ProxyError::Cancelled),
        joined = tokio::time::timeout_at(deadline, async {
            let ingress_fut = async { upgrade.await.map_err(ProxyError::network) };
            let egress_fut = connect_passthrough_egress(host, port, state, &cancel);
            tokio::try_join!(ingress_fut, egress_fut)
        }) => joined.map_err(|_| ProxyError::Timeout)??,
    };
    copy_tunnel(&mut ingress, &mut egress, &cancel).await
}

pub(super) async fn relay_stream<I>(
    mut ingress: I,
    host: &str,
    port: u16,
    state: &State,
) -> Result<(), ProxyError>
where
    I: Io + Unpin,
{
    let parsed_host: Host = crate::route::strip_host_brackets(host)
        .parse()
        .map_err(|_| ProxyError::invalid("SOCKS5 透传目标主机无效"))?;
    let cancel = state.cancel.child_token();
    let _cancel_guard = cancel.clone().drop_guard();
    let mut egress = connect_passthrough_egress(parsed_host, port, state, &cancel).await?;
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
