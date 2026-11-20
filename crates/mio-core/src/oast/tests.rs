use super::*;
use tokio::time::timeout;

const TEST_EVENT_TIMEOUT: Duration = Duration::from_secs(2);

fn dns_query(labels: &[&str], id: u16) -> Vec<u8> {
    let mut packet = Vec::new();
    packet.extend_from_slice(&id.to_be_bytes());
    packet.extend_from_slice(&0x0100_u16.to_be_bytes());
    packet.extend_from_slice(&1_u16.to_be_bytes());
    packet.extend_from_slice(&0_u16.to_be_bytes());
    packet.extend_from_slice(&0_u16.to_be_bytes());
    packet.extend_from_slice(&0_u16.to_be_bytes());
    for label in labels {
        packet.push(u8::try_from(label.len()).expect("dns label length"));
        packet.extend_from_slice(label.as_bytes());
    }
    packet.push(0);
    packet.extend_from_slice(&DNS_QTYPE_A.to_be_bytes());
    packet.extend_from_slice(&DNS_QCLASS_IN.to_be_bytes());
    packet
}

async fn next_interaction(events_rx: &mut mpsc::Receiver<OastInteraction>) -> OastInteraction {
    timeout(TEST_EVENT_TIMEOUT, events_rx.recv())
        .await
        .expect("interaction deadline")
        .expect("interaction channel")
}

#[tokio::test]
async fn captures_issued_http_and_dns_callbacks() {
    let (events_tx, mut events_rx) = mpsc::channel(16);
    let mut server = LocalOastServer::start(events_tx).await.unwrap();
    assert_eq!(server.http_addr().ip(), Ipv4Addr::LOCALHOST);
    assert_eq!(server.dns_addr().ip(), Ipv4Addr::LOCALHOST);
    assert!(!server.self_hosted_config().enabled);

    let http_payload = server.issue_payload("Repeater Tab 1");
    assert_eq!(http_payload.token.len(), TOKEN_HEX_LEN);
    assert!(
        http_payload
            .token
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
    );
    assert_eq!(
        http_payload.http_callback_url,
        format!("http://{}/oast/{}", server.http_addr(), http_payload.token)
    );
    assert_eq!(
        http_payload.dns_qname,
        format!("{}.oast.local", http_payload.token)
    );
    assert!(server.issued_tokens().contains(&http_payload));

    let request = format!(
        "GET /oast/{}?probe=1 HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        http_payload.token,
        server.http_addr()
    );
    let mut http_stream = TcpStream::connect(server.http_addr()).await.unwrap();
    http_stream.write_all(request.as_bytes()).await.unwrap();
    http_stream.flush().await.unwrap();
    let mut http_response = Vec::new();
    http_stream.read_to_end(&mut http_response).await.unwrap();
    let http_response = String::from_utf8_lossy(&http_response);
    assert!(http_response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(http_response.ends_with("ok"));

    let http_interaction = next_interaction(&mut events_rx).await;
    assert_eq!(http_interaction.protocol, OastProtocol::Http);
    assert_eq!(
        http_interaction.token.as_deref(),
        Some(http_payload.token.as_str())
    );
    assert_eq!(
        http_interaction.summary,
        format!("GET /oast/{}?probe=1", http_payload.token)
    );
    assert!(!http_interaction.raw_request.is_empty());
    assert!(http_interaction.raw_request.len() <= MAX_RAW_REQUEST_BYTES);

    let dns_payload = server.issue_payload("DNS callback");
    assert_ne!(dns_payload.token, http_payload.token);
    let dns_labels = ["sub", dns_payload.token.as_str(), "oast", "local"];
    let query = dns_query(&dns_labels, 0x4d49);
    let dns_socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    dns_socket.send_to(&query, server.dns_addr()).await.unwrap();
    let mut response = [0_u8; 512];
    let (response_len, _) = dns_socket.recv_from(&mut response).await.unwrap();
    let response = &response[..response_len];
    assert_eq!(u16::from_be_bytes([response[0], response[1]]), 0x4d49);
    assert_eq!(response[2] & 0x80, 0x80);
    assert_eq!(response[2] & 0x04, 0x04);
    assert_eq!(response[3] & 0x0f, DNS_RCODE_NOERROR);
    assert_eq!(u16::from_be_bytes([response[4], response[5]]), 1);
    assert_eq!(u16::from_be_bytes([response[6], response[7]]), 1);
    assert_eq!(&response[response_len - 4..], &[127, 0, 0, 1]);

    let dns_interaction = next_interaction(&mut events_rx).await;
    assert_eq!(dns_interaction.protocol, OastProtocol::Dns);
    assert_eq!(
        dns_interaction.token.as_deref(),
        Some(dns_payload.token.as_str())
    );
    assert_eq!(
        dns_interaction.summary,
        format!("A sub.{}.oast.local", dns_payload.token)
    );
    assert_eq!(dns_interaction.raw_request, query);

    server.stop().await.unwrap();
}

#[tokio::test]
async fn correlates_host_and_query_and_rejects_blocked_cloud() {
    let (events_tx, mut events_rx) = mpsc::channel(16);
    let mut server = LocalOastServer::start(events_tx).await.unwrap();
    let payload = server.issue_payload("Callback probe");
    let token = payload.token.as_str();
    let request = format!(
        "GET /callback?callback={token}&probe=1 HTTP/1.1\r\nHost: {token}.oast.local\r\nConnection: close\r\n\r\n"
    );
    let mut http_stream = TcpStream::connect(server.http_addr()).await.unwrap();
    http_stream.write_all(request.as_bytes()).await.unwrap();
    http_stream.flush().await.unwrap();
    let mut http_response = Vec::new();
    http_stream.read_to_end(&mut http_response).await.unwrap();
    assert!(http_response.starts_with(b"HTTP/1.1 200 OK"));

    let interaction = next_interaction(&mut events_rx).await;
    assert_eq!(interaction.protocol, OastProtocol::Http);
    assert_eq!(interaction.token.as_deref(), Some(token));
    assert_eq!(
        interaction.summary,
        format!("GET /callback?callback={token}&probe=1")
    );

    let query = dns_query(&["unknown", "oast", "local"], 0x1234);
    let dns_socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    dns_socket.send_to(&query, server.dns_addr()).await.unwrap();
    let mut response = [0_u8; 512];
    let (response_len, _) = dns_socket.recv_from(&mut response).await.unwrap();
    let response = &response[..response_len];
    assert_eq!(response[3] & 0x0f, DNS_RCODE_NXDOMAIN);
    assert_eq!(u16::from_be_bytes([response[6], response[7]]), 0);
    let interaction = next_interaction(&mut events_rx).await;
    assert_eq!(interaction.protocol, OastProtocol::Dns);
    assert_eq!(interaction.token, None);
    assert_eq!(interaction.summary, "A unknown.oast.local");

    let oversized_body = vec![b'a'; MAX_RAW_REQUEST_BYTES + 4096];
    let oversized_headers = format!(
        "POST /oast/{token} HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        server.http_addr(),
        oversized_body.len()
    );
    let mut oversized_stream = TcpStream::connect(server.http_addr()).await.unwrap();
    let (mut read_half, mut write_half) = oversized_stream.split();
    let send_task = async {
        let _ = write_half.write_all(oversized_headers.as_bytes()).await;
        let _ = write_half.write_all(&oversized_body).await;
        let _ = write_half.flush().await;
    };
    let recv_task = async {
        let mut buf = Vec::new();
        let _ = read_half.read_to_end(&mut buf).await;
        buf
    };
    let (_, oversized_resp) = tokio::join!(send_task, recv_task);
    assert!(oversized_resp.starts_with(b"HTTP/1.1 200 OK"));
    let capped_interaction = next_interaction(&mut events_rx).await;
    assert_eq!(capped_interaction.protocol, OastProtocol::Http);
    assert_eq!(capped_interaction.token.as_deref(), Some(token));
    assert_eq!(capped_interaction.raw_request.len(), MAX_RAW_REQUEST_BYTES);

    let blocked_config = SelfHostedOastConfig {
        enabled: true,
        polling_url: Some("https://collector.interact.sh/poll".to_owned()),
        ..SelfHostedOastConfig::default()
    };
    assert!(blocked_config.validate().is_err());
    assert_eq!(SelfHostedOastConfig::default().validate().unwrap(), None);
    server.stop().await.unwrap();
}

#[tokio::test]
async fn captures_partial_slow_tcp_probe_and_uses_self_hosted_domain_url() {
    let (events_tx, mut events_rx) = mpsc::channel(16);
    let config = SelfHostedOastConfig {
        enabled: true,
        polling_url: Some("https://oast.internal:8443/poll".to_owned()),
        server_domain: Some("oast.internal".to_owned()),
        authorization_header: None,
        poll_interval_ms: 1_000,
    };
    let mut server = LocalOastServer::start_with_config(events_tx, config)
        .await
        .unwrap();
    let payload = server.issue_payload("SSRF probe");
    assert_eq!(
        payload.http_callback_url,
        format!("https://oast.internal/oast/{}", payload.token)
    );
    assert_eq!(
        payload.dns_qname,
        format!("{}.oast.internal", payload.token)
    );

    // Send an incomplete HTTP request without CRLFCRLF and keep socket open past CONNECTION_TIMEOUT.
    let mut slow_stream = TcpStream::connect(server.http_addr()).await.unwrap();
    slow_stream
        .write_all(format!("GET /oast/{} HTTP/1.1\r\nHost: slow", payload.token).as_bytes())
        .await
        .unwrap();
    slow_stream.flush().await.unwrap();
    let interaction = timeout(
        CONNECTION_TIMEOUT + Duration::from_secs(2),
        events_rx.recv(),
    )
    .await
    .expect("slow probe must still emit interaction after read deadline")
    .expect("interaction channel open");
    assert_eq!(interaction.token.as_deref(), Some(payload.token.as_str()));
    drop(slow_stream);
    server.stop().await.unwrap();
}
