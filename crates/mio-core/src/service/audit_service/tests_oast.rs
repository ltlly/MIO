use std::{
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};

use mio_plugin_api::HttpVersion;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::*;
use crate::{
    AuditPhase, CollaboratorEvent, Command, IssueConfidence, OastInteraction, OastProtocol,
    target_issues::IssueSeverity,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

fn ssrf_draft(addr: SocketAddr, path_and_query: &str) -> Draft {
    let raw = format!("GET {path_and_query} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    Draft {
        revision: 1,
        target: format!("http://{addr}"),
        raw: raw.into_bytes(),
        version: HttpVersion::Http11,
    }
}

async fn read_request_line(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 512];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    String::from_utf8_lossy(&buf)
        .lines()
        .next()
        .map(str::to_owned)
}

async fn perform_loopback_get(callback_url: &str) {
    let Ok(parsed) = Url::parse(callback_url) else {
        return;
    };
    let Some(port) = parsed.port() else {
        return;
    };
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(Ok(mut conn)) = timeout(Duration::from_secs(2), TcpStream::connect(addr)).await else {
        return;
    };
    let path = match parsed.query() {
        Some(q) => format!("{}?{q}", parsed.path()),
        None => parsed.path().to_owned(),
    };
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    let _ = conn.write_all(req.as_bytes()).await;
    let mut resp = [0_u8; 256];
    let _ = timeout(Duration::from_secs(2), conn.read(&mut resp)).await;
}

async fn ssrf_origin(listener: TcpListener) {
    while let Ok((mut stream, _)) = listener.accept().await {
        tokio::spawn(async move {
            let Some(request_line) = read_request_line(&mut stream).await else {
                return;
            };
            let target = request_line.split_whitespace().nth(1).unwrap_or("/");
            let parsed = Url::parse(&format!("http://127.0.0.1{target}"))
                .unwrap_or_else(|_| Url::parse("http://127.0.0.1/").unwrap());
            for (_, value) in parsed.query_pairs() {
                if value.starts_with("http://127.0.0.1:") {
                    perform_loopback_get(&value).await;
                }
            }
            let body = "fetched";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
    }
}

#[tokio::test]
async fn active_scan_auto_starts_local_oast_and_emits_high_ssrf_finding_on_callback() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let origin = tokio::spawn(ssrf_origin(listener));

    let (runtime, client) = crate::Runtime::start().unwrap();
    let cmd_tx = client.commands;
    let mut events_rx = client.events;

    let opened = timeout(TEST_TIMEOUT, events_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let Event::ProjectOpened { project, .. } = opened else {
        panic!("expected ProjectOpened first");
    };

    let task_id = Uuid::new_v4();
    cmd_tx
        .send(Command::Audit(AuditCommand::StartScan {
            project,
            task_id,
            title: "SSRF OAST Auto Scan".to_owned(),
            draft: Box::new(ssrf_draft(addr, "/proxy?url=http://127.0.0.1:9/seed")),
            mode: AuditTaskMode::PassiveAndActive,
            crawl_same_origin: false,
            cancel: CancellationToken::new(),
        }))
        .await
        .unwrap();

    let mut saw_collaborator_callback = false;
    let mut task_completed = false;
    let mut latest_snapshot: Option<AuditStateSnapshot> = None;

    while !(saw_collaborator_callback
        && task_completed
        && latest_snapshot
            .as_ref()
            .is_some_and(|s| s.findings.iter().any(|f| f.issue_id == "0x00100a00")))
    {
        let event = timeout(TEST_TIMEOUT, events_rx.recv())
            .await
            .expect("timed out waiting for OAST SSRF finding and scan completion")
            .expect("service event stream closed unexpectedly");
        match event {
            Event::Collaborator(CollaboratorEvent::InteractionObserved { interaction, .. }) => {
                if interaction.protocol == OastProtocol::Http {
                    saw_collaborator_callback = true;
                }
            }
            Event::Audit(AuditEvent::TaskUpdated { task, .. })
                if task.id == task_id && task.state == AuditTaskState::Completed =>
            {
                task_completed = true;
            }
            Event::Audit(AuditEvent::State {
                result: Ok(snapshot),
                ..
            }) => {
                latest_snapshot = Some(*snapshot);
            }
            _ => {}
        }
    }

    let snapshot = latest_snapshot.expect("AuditEvent::State snapshot pushed on OAST callback");
    let finding = snapshot
        .findings
        .iter()
        .find(|f| f.issue_id == "0x00100a00")
        .expect("0x00100a00 SSRF finding must be present");
    assert_eq!(
        finding.title,
        "服务端请求伪造 / OAST 带外交互 (SSRF via Collaborator)"
    );
    assert_eq!(finding.severity(), IssueSeverity::High);
    assert_eq!(finding.confidence, IssueConfidence::Certain);
    assert_eq!(finding.phase, AuditPhase::Active);
    assert_eq!(finding.parameter.as_deref(), Some("OAST (Collaborator)"));
    assert!(finding.url.contains("/proxy?url="));
    assert!(finding.evidence.contains("HTTP 回调自"));
    assert!(finding.flow_id.is_some());

    drop(cmd_tx);
    drop(events_rx);
    timeout(
        TEST_TIMEOUT,
        tokio::task::spawn_blocking(move || runtime.join()),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    origin.abort();
}

#[test]
fn oast_probe_registry_is_bounded_and_matches_dns_or_summary_tokens() {
    let mut service = AuditService::default();
    let project = ProjectKey {
        id: Uuid::new_v4(),
        generation: 1,
    };
    for idx in 0..(MAX_OAST_PROBES + 10) {
        service.register_oast_probe(OastProbeRecord {
            project,
            token: format!("{idx:016x}"),
            callback_url: format!("http://127.0.0.1:8080/oast/{idx:016x}"),
            task_id: Uuid::new_v4(),
            seed_flow_id: None,
            target_url: format!("http://127.0.0.1/item/{idx}"),
        });
    }
    assert_eq!(service.oast_probes.len(), MAX_OAST_PROBES);
    assert_eq!(service.oast_probes[0].token, format!("{:016x}", 10));

    let latest_token = format!("{:016x}", MAX_OAST_PROBES + 9);
    let dns_interaction = OastInteraction {
        id: Uuid::new_v4(),
        token: None,
        protocol: OastProtocol::Dns,
        peer_addr: "127.0.0.1:55300".to_owned(),
        summary: format!("DNS A {latest_token}.oast.local"),
        raw_request: Vec::new(),
        observed_unix_ms: 1_700_000_000_000,
    };
    let matched = service.matching_oast_probes(project, &dns_interaction);
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].token, latest_token);
}
