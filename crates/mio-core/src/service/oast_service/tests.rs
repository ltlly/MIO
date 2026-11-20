use std::{net::Ipv4Addr, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::mpsc,
    time::timeout,
};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::oast::OastProtocol;

const TEST_TIMEOUT: Duration = Duration::from_secs(3);

fn build_dns_a_query(qname: &str, query_id: u16) -> Vec<u8> {
    let mut packet = Vec::with_capacity(64);
    packet.extend_from_slice(&query_id.to_be_bytes());
    packet.extend_from_slice(&0x0100_u16.to_be_bytes());
    packet.extend_from_slice(&1_u16.to_be_bytes());
    packet.extend_from_slice(&0_u16.to_be_bytes());
    packet.extend_from_slice(&0_u16.to_be_bytes());
    packet.extend_from_slice(&0_u16.to_be_bytes());
    for label in qname.trim_end_matches('.').split('.') {
        packet.push(u8::try_from(label.len()).expect("valid dns label length"));
        packet.extend_from_slice(label.as_bytes());
    }
    packet.push(0);
    packet.extend_from_slice(&1_u16.to_be_bytes());
    packet.extend_from_slice(&1_u16.to_be_bytes());
    packet
}

#[tokio::test]
async fn generate_payloads_captures_http_and_dns_interactions_and_stops_cleanly() {
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let mut state = State::new(events_tx, CancellationToken::new()).unwrap();
    let project = state.project;
    assert!(!state.collaborator.has_resources());

    state
        .collaborator_command(CollaboratorCommand::GeneratePayloads {
            project,
            count: 2,
            label: "Scanner SSRF Probe".to_owned(),
        })
        .await
        .unwrap();

    let Event::Collaborator(CollaboratorEvent::Status {
        project: status_project,
        status,
    }) = timeout(TEST_TIMEOUT, events_rx.recv())
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("expected CollaboratorEvent::Status");
    };
    assert_eq!(status_project, project);
    assert!(status.running);
    let http_addr = status.http_addr.expect("loopback http listener");
    let dns_addr = status.dns_addr.expect("loopback dns listener");
    assert_eq!(http_addr.ip(), Ipv4Addr::LOCALHOST);
    assert_eq!(dns_addr.ip(), Ipv4Addr::LOCALHOST);

    let Event::Collaborator(CollaboratorEvent::PayloadsGenerated {
        project: gen_project,
        payloads,
    }) = timeout(TEST_TIMEOUT, events_rx.recv())
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("expected CollaboratorEvent::PayloadsGenerated");
    };
    assert_eq!(gen_project, project);
    assert_eq!(payloads.len(), 2);
    let http_payload = &payloads[0];
    let dns_payload = &payloads[1];

    let raw_http = format!(
        "GET /oast/{}?src=ssrf HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        http_payload.token, http_addr
    );
    let mut tcp = TcpStream::connect(http_addr).await.unwrap();
    tcp.write_all(raw_http.as_bytes()).await.unwrap();
    tcp.flush().await.unwrap();
    let mut resp = Vec::new();
    tcp.read_to_end(&mut resp).await.unwrap();
    assert!(resp.starts_with(b"HTTP/1.1 200 OK"));

    let CollaboratorEvent::InteractionObserved {
        project: obs_project_1,
        interaction: http_obs,
    } = timeout(TEST_TIMEOUT, state.collaborator.next_signal())
        .await
        .unwrap()
    else {
        panic!("expected HTTP InteractionObserved");
    };
    assert_eq!(obs_project_1, project);
    assert_eq!(http_obs.protocol, OastProtocol::Http);
    assert_eq!(http_obs.token.as_deref(), Some(http_payload.token.as_str()));

    let query = build_dns_a_query(&format!("exfil.{}", dns_payload.dns_qname), 0x2048);
    let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    udp.send_to(&query, dns_addr).await.unwrap();
    let mut dns_reply = [0_u8; 512];
    let (reply_len, _) = timeout(TEST_TIMEOUT, udp.recv_from(&mut dns_reply))
        .await
        .unwrap()
        .unwrap();
    assert!(reply_len >= 12);

    let CollaboratorEvent::InteractionObserved {
        project: obs_project_2,
        interaction: dns_obs,
    } = timeout(TEST_TIMEOUT, state.collaborator.next_signal())
        .await
        .unwrap()
    else {
        panic!("expected DNS InteractionObserved");
    };
    assert_eq!(obs_project_2, project);
    assert_eq!(dns_obs.protocol, OastProtocol::Dns);
    assert_eq!(dns_obs.token.as_deref(), Some(dns_payload.token.as_str()));

    state
        .collaborator_command(CollaboratorCommand::Stop { project })
        .await
        .unwrap();
    let Event::Collaborator(CollaboratorEvent::Status {
        status: stopped, ..
    }) = timeout(TEST_TIMEOUT, events_rx.recv())
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("expected stopped status");
    };
    assert!(!stopped.running);
    assert_eq!(stopped.interactions.len(), 2);
    assert!(!state.collaborator.has_resources());

    let rebound_tcp = TcpListener::bind(http_addr).await.unwrap();
    let rebound_udp = UdpSocket::bind(dns_addr).await.unwrap();
    drop((rebound_tcp, rebound_udp));
}

#[tokio::test]
async fn rejects_blocked_cloud_domains_and_deduplicates_self_hosted_polling() {
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let mut state = State::new(events_tx, CancellationToken::new()).unwrap();
    let project = state.project;

    let blocked = SelfHostedOastConfig {
        enabled: true,
        polling_url: Some("https://poll.interact.sh/interactions".to_owned()),
        server_domain: Some("sub.interactsh.com".to_owned()),
        authorization_header: None,
        poll_interval_ms: 1_000,
    };
    assert!(
        state
            .collaborator_command(CollaboratorCommand::Start {
                project,
                config: blocked,
            })
            .await
            .is_err()
    );

    let id_one = Uuid::new_v4();
    let id_two = Uuid::new_v4();
    let first_item = OastInteraction {
        id: id_one,
        token: Some("0123456789abcdef".to_owned()),
        protocol: OastProtocol::Http,
        peer_addr: "127.0.0.1:50001".to_owned(),
        summary: "GET /oast/0123456789abcdef".to_owned(),
        raw_request: b"GET /oast/0123456789abcdef HTTP/1.1\r\n\r\n".to_vec(),
        observed_unix_ms: 1_700_000_001,
    };
    let second_item = OastInteraction {
        id: id_two,
        token: Some("fedcba9876543210".to_owned()),
        protocol: OastProtocol::Dns,
        peer_addr: "127.0.0.1:50002".to_owned(),
        summary: "A fedcba9876543210.self.local".to_owned(),
        raw_request: vec![0x12, 0x34, 0x01, 0x00],
        observed_unix_ms: 1_700_000_002,
    };

    let poll_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let poll_addr = poll_listener.local_addr().unwrap();
    let batch_1 = serde_json::to_vec(&vec![first_item.clone(), first_item.clone()]).unwrap();
    let batch_2 = serde_json::to_vec(&vec![first_item.clone(), second_item.clone()]).unwrap();
    let poll_server = tokio::spawn(async move {
        for body in [batch_1, batch_2] {
            let (mut stream, _) = poll_listener.accept().await.unwrap();
            let mut req_buf = Vec::new();
            let mut chunk = [0_u8; 1024];
            while !req_buf.windows(4).any(|w| w == b"\r\n\r\n") {
                let read_len = stream.read(&mut chunk).await.unwrap();
                if read_len == 0 {
                    break;
                }
                req_buf.extend_from_slice(&chunk[..read_len]);
            }
            let req_text = String::from_utf8_lossy(&req_buf).to_ascii_lowercase();
            assert!(req_text.contains("authorization: bearer test-secret-token"));
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(header.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
            stream.shutdown().await.unwrap();
        }
    });

    let self_hosted = SelfHostedOastConfig {
        enabled: true,
        polling_url: Some(format!("http://{poll_addr}/poll")),
        server_domain: Some("self.local".to_owned()),
        authorization_header: Some("Bearer test-secret-token".to_owned()),
        poll_interval_ms: 500,
    };
    state
        .collaborator_command(CollaboratorCommand::Start {
            project,
            config: self_hosted,
        })
        .await
        .unwrap();
    let _ = timeout(TEST_TIMEOUT, events_rx.recv())
        .await
        .unwrap()
        .unwrap();

    state
        .collaborator_command(CollaboratorCommand::PollNow { project })
        .await
        .unwrap();
    let first_event = timeout(TEST_TIMEOUT, state.collaborator.next_signal())
        .await
        .unwrap();
    assert!(matches!(
        first_event,
        CollaboratorEvent::InteractionObserved { ref interaction, .. } if interaction.id == id_one
    ));
    let first_status = timeout(TEST_TIMEOUT, state.collaborator.next_signal())
        .await
        .unwrap();
    let CollaboratorEvent::Status { status, .. } = first_status else {
        panic!("expected Status after first PollNow");
    };
    assert_eq!(status.interactions.len(), 1);

    state
        .collaborator_command(CollaboratorCommand::PollNow { project })
        .await
        .unwrap();
    let second_event = timeout(TEST_TIMEOUT, state.collaborator.next_signal())
        .await
        .unwrap();
    assert!(matches!(
        second_event,
        CollaboratorEvent::InteractionObserved { ref interaction, .. } if interaction.id == id_two
    ));
    let second_status = timeout(TEST_TIMEOUT, state.collaborator.next_signal())
        .await
        .unwrap();
    let CollaboratorEvent::Status { status, .. } = second_status else {
        panic!("expected Status after second PollNow");
    };
    assert_eq!(status.interactions.len(), 2);
    assert_eq!(status.interactions[0].id, id_two);
    assert_eq!(status.interactions[1].id, id_one);

    state.collaborator.shutdown().await.unwrap();
    poll_server.await.unwrap();
}

#[tokio::test]
async fn self_hosted_poll_interval_triggers_background_polling_and_cancels_on_stop() {
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let mut state = State::new(events_tx, CancellationToken::new()).unwrap();
    let project = state.project;

    let auto_id = Uuid::new_v4();
    let auto_item = OastInteraction {
        id: auto_id,
        token: Some("aaaa1111bbbb2222".to_owned()),
        protocol: OastProtocol::Http,
        peer_addr: "127.0.0.1:51000".to_owned(),
        summary: "GET /oast/aaaa1111bbbb2222".to_owned(),
        raw_request: b"GET /oast/aaaa1111bbbb2222 HTTP/1.1\r\n\r\n".to_vec(),
        observed_unix_ms: 1_700_000_100,
    };
    let poll_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let poll_addr = poll_listener.local_addr().unwrap();
    let body = serde_json::to_vec(&vec![auto_item]).unwrap();
    let (accepted_tx, mut accepted_rx) = mpsc::unbounded_channel::<()>();
    let poll_server = tokio::spawn(async move {
        while let Ok((mut stream, _)) = poll_listener.accept().await {
            let _ = accepted_tx.send(());
            let mut req_buf = [0_u8; 512];
            let _ = stream.read(&mut req_buf).await;
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes()).await;
            let _ = stream.write_all(&body).await;
            let _ = stream.shutdown().await;
        }
    });

    state
        .collaborator_command(CollaboratorCommand::Start {
            project,
            config: SelfHostedOastConfig {
                enabled: true,
                polling_url: Some(format!("http://{poll_addr}/auto")),
                server_domain: Some("auto.local".to_owned()),
                authorization_header: None,
                poll_interval_ms: 250,
            },
        })
        .await
        .unwrap();
    let _ = timeout(TEST_TIMEOUT, events_rx.recv()).await.unwrap();

    let auto_event = timeout(Duration::from_secs(2), state.collaborator.next_signal())
        .await
        .expect("auto-poll should fire without PollNow");
    assert!(matches!(
        auto_event,
        CollaboratorEvent::InteractionObserved { ref interaction, .. } if interaction.id == auto_id
    ));

    state
        .collaborator_command(CollaboratorCommand::Stop { project })
        .await
        .unwrap();
    while accepted_rx.try_recv().is_ok() {}
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(
        accepted_rx.try_recv().is_err(),
        "auto-poll must stop firing after CollaboratorCommand::Stop"
    );

    state.collaborator.shutdown().await.unwrap();
    poll_server.abort();
}
