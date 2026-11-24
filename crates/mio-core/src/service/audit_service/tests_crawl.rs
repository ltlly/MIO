use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use mio_plugin_api::HttpVersion;
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    time::{Instant, sleep, timeout},
};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::*;
use crate::{AuditPhase, Command};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

fn get_draft(addr: SocketAddr, path: &str) -> Draft {
    let raw = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    Draft {
        revision: 1,
        target: format!("http://{addr}"),
        raw: raw.into_bytes(),
        version: HttpVersion::Http11,
    }
}

fn json_post_draft(addr: SocketAddr, path: &str, json_body: &str) -> Draft {
    let raw = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json_body}",
        json_body.len()
    );
    Draft {
        revision: 1,
        target: format!("http://{addr}"),
        raw: raw.into_bytes(),
        version: HttpVersion::Http11,
    }
}

async fn read_http_request(stream: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0_u8; 512];
    let header_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() >= 32 * 1024 {
            return None;
        }
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let content_length = head
        .lines()
        .skip(1)
        .find_map(|line| {
            let (k, v) = line.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0)
        .min(64 * 1024);
    while buf.len() < header_end + content_length {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = buf[header_end..buf.len().min(header_end + content_length)].to_vec();
    Some((head, body))
}

async fn json_and_crawl_origin(listener: TcpListener) {
    while let Ok((mut stream, _)) = listener.accept().await {
        tokio::spawn(async move {
            let Some((head, body)) = read_http_request(&mut stream).await else {
                return;
            };
            let first_line = head.lines().next().unwrap_or_default();
            let mut parts = first_line.split_whitespace();
            let method = parts.next().unwrap_or("GET");
            let target = parts.next().unwrap_or("/");
            let parsed = Url::parse(&format!("http://127.0.0.1{target}"))
                .unwrap_or_else(|_| Url::parse("http://127.0.0.1/").unwrap());

            let resp = match (method, parsed.path()) {
                ("GET", "/") => {
                    let html = "<html><head><meta http-equiv=\"X-Frame-Options\" content=\"DENY\"></head><body><a href=\"/api/json-echo?msg=seed\">Echo</a><form action=\"/sub/order\" method=\"post\"><input name=\"sku\" value=\"A1\"></form></body></html>";
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nX-Frame-Options: DENY\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                        html.len()
                    )
                }
                ("POST", "/api/json-echo") => {
                    let echoed = serde_json::from_slice::<Value>(&body)
                        .ok()
                        .and_then(|v| {
                            v.get("user")
                                .and_then(|u| u.get("name"))
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                        .unwrap_or_default();
                    let out = serde_json::json!({ "echo": echoed }).to_string();
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                        out.len()
                    )
                }
                ("GET", "/api/json-echo") => {
                    let msg = parsed
                        .query_pairs()
                        .find_map(|(k, v)| (k == "msg").then(|| v.into_owned()))
                        .unwrap_or_default();
                    let out = serde_json::json!({ "echo": msg }).to_string();
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nSet-Cookie: sub_sid=plain_cookie; Path=/\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                        out.len()
                    )
                }
                ("POST", "/sub/order") => {
                    let raw_form = String::from_utf8_lossy(&body);
                    if raw_form.contains("%27") || raw_form.contains('\'') {
                        let err = "SQLSTATE[42000]: syntax error near '''";
                        format!(
                            "HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{err}",
                            err.len()
                        )
                    } else {
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_owned()
                    }
                }
                _ => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_owned(),
            };
            let _ = stream.write_all(resp.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
    }
}

#[tokio::test]
async fn active_scan_detects_canary_reflection_on_structured_json_post_parameters() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let origin = tokio::spawn(json_and_crawl_origin(listener));

    let (events_tx, mut events_rx) = mpsc::channel(128);
    let mut state = State::new(events_tx, CancellationToken::new()).unwrap();
    let project = state.project;
    let task_id = Uuid::new_v4();

    state
        .audit_command(AuditCommand::StartScan {
            project,
            task_id,
            title: "JSON Echo Active Scan".to_owned(),
            draft: Box::new(json_post_draft(
                addr,
                "/api/json-echo",
                r#"{"user":{"name":"alice"},"count":1}"#,
            )),
            mode: AuditTaskMode::PassiveAndActive,
            crawl_same_origin: false,
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();

    loop {
        let sig = timeout(TEST_TIMEOUT, state.audit.next_signal())
            .await
            .unwrap()
            .unwrap();
        let done = matches!(sig.kind, AuditSignalKind::Finished { .. });
        state.on_audit_signal(sig).await;
        while events_rx.try_recv().is_ok() {}
        if done {
            break;
        }
    }

    let snapshot = state.store.read_audit_state().unwrap();
    let task = snapshot.tasks.iter().find(|t| t.id == task_id).unwrap();
    assert_eq!(task.state, AuditTaskState::Completed);
    assert_eq!(task.requests_sent, 20);

    let canary_hit = snapshot
        .findings
        .iter()
        .find(|f| {
            f.issue_id == "0x00200300"
                && f.phase == AuditPhase::Active
                && f.parameter.as_deref() == Some("user.name")
        })
        .expect("JSON field user.name must trigger canary reflection finding");
    let flow_id = canary_hit
        .flow_id
        .expect("JSON probe finding bound to persisted flow");
    state
        .command(Command::ReadFlow {
            project,
            selection_revision: 1,
            flow_id,
        })
        .await
        .unwrap();
    let mut detail = None;
    while let Ok(ev) = events_rx.try_recv() {
        if let Event::FlowDetail { record, .. } = ev {
            detail = Some(record);
        }
    }
    let record = detail.expect("JSON probe flow persisted");
    assert!(
        String::from_utf8_lossy(&record.request.body).contains(ACTIVE_CANARY),
        "persisted probe request must contain injected JSON canary"
    );
    state.collaborator.shutdown().await.unwrap();
    origin.abort();
}

#[tokio::test]
async fn crawl_before_scan_discovers_subpaths_merges_findings_and_supports_cancellation() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let origin = tokio::spawn(json_and_crawl_origin(listener));

    let (events_tx, mut events_rx) = mpsc::channel(128);
    let mut state = State::new(events_tx, CancellationToken::new()).unwrap();
    let project = state.project;
    let task_id = Uuid::new_v4();

    state
        .audit_command(AuditCommand::StartScan {
            project,
            task_id,
            title: "Crawl + Active Audit".to_owned(),
            draft: Box::new(get_draft(addr, "/")),
            mode: AuditTaskMode::PassiveAndActive,
            crawl_same_origin: true,
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();

    loop {
        let sig = timeout(TEST_TIMEOUT, state.audit.next_signal())
            .await
            .unwrap()
            .unwrap();
        let done = matches!(sig.kind, AuditSignalKind::Finished { .. });
        state.on_audit_signal(sig).await;
        while events_rx.try_recv().is_ok() {}
        if done {
            break;
        }
    }
    origin.abort();

    let snapshot = state.store.read_audit_state().unwrap();
    let task = snapshot.tasks.iter().find(|t| t.id == task_id).unwrap();
    assert_eq!(task.state, AuditTaskState::Completed);
    assert!(task.requests_sent >= 10);
    assert!(
        snapshot.findings.iter().any(|f| {
            f.phase == AuditPhase::Passive
                && f.issue_id == "0x00500600"
                && f.url.contains("/api/json-echo")
        }),
        "crawl passive findings from discovered subpath must be merged"
    );
    assert!(
        snapshot.findings.iter().any(|f| {
            f.phase == AuditPhase::Active
                && f.issue_id == "0x00200300"
                && f.parameter.as_deref() == Some("msg")
        }),
        "discovered GET endpoint with query parameter must be actively audited"
    );
    assert!(
        snapshot.findings.iter().any(|f| {
            f.phase == AuditPhase::Active
                && f.issue_id == "0x00100200"
                && f.parameter.as_deref() == Some("sku")
        }),
        "discovered POST form endpoint must be actively audited"
    );

    let slow_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let slow_addr = slow_listener.local_addr().unwrap();
    let crawl_hangs = Arc::new(AtomicUsize::new(0));
    let hangs_srv = crawl_hangs.clone();
    let slow_origin = tokio::spawn(async move {
        while let Ok((mut stream, _)) = slow_listener.accept().await {
            let counter = hangs_srv.clone();
            tokio::spawn(async move {
                let Some((head, _)) = read_http_request(&mut stream).await else {
                    return;
                };
                let line = head.lines().next().unwrap_or_default();
                if line.starts_with("GET / ") {
                    let html = "<html><body><a href=\"/hang?k=1\">Hang</a></body></html>";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                        html.len()
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                } else {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let mut sink = [0_u8; 64];
                    let _ = stream.read(&mut sink).await;
                }
            });
        }
    });

    let cancel_task_id = Uuid::new_v4();
    state
        .audit_command(AuditCommand::StartScan {
            project,
            task_id: cancel_task_id,
            title: "Slow Crawl Scan".to_owned(),
            draft: Box::new(get_draft(slow_addr, "/")),
            mode: AuditTaskMode::PassiveAndActive,
            crawl_same_origin: true,
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();

    let wait_start = Instant::now();
    while crawl_hangs.load(Ordering::SeqCst) == 0 {
        assert!(wait_start.elapsed() < Duration::from_secs(2));
        while let Ok(Some(sig)) =
            timeout(Duration::from_millis(20), state.audit.next_signal()).await
        {
            state.on_audit_signal(sig).await;
        }
        sleep(Duration::from_millis(10)).await;
    }

    state
        .audit_command(AuditCommand::StopScan {
            project,
            task_id: cancel_task_id,
        })
        .await
        .unwrap();
    loop {
        let sig = timeout(Duration::from_secs(3), state.audit.next_signal())
            .await
            .expect("StopScan during crawl must complete within 3s")
            .unwrap();
        let done = matches!(sig.kind, AuditSignalKind::Finished { .. });
        state.on_audit_signal(sig).await;
        while events_rx.try_recv().is_ok() {}
        if done {
            break;
        }
    }
    slow_origin.abort();

    assert!(!state.audit.has_resources());
    let snapshot_after = state.store.read_audit_state().unwrap();
    let cancelled_task = snapshot_after
        .tasks
        .iter()
        .find(|t| t.id == cancel_task_id)
        .unwrap();
    assert_eq!(cancelled_task.state, AuditTaskState::Cancelled);
    state.collaborator.shutdown().await.unwrap();
}
