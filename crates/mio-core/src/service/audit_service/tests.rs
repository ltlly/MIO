use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use mio_plugin_api::HttpVersion;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    time::{Instant, sleep, timeout},
};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::*;
use crate::{Command, IssueConfidence, target_issues::IssueSeverity};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

fn draft_for(addr: SocketAddr, path_and_query: &str) -> Draft {
    let raw = format!("GET {path_and_query} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    Draft {
        revision: 1,
        target: format!("http://{addr}"),
        raw: raw.into_bytes(),
        version: HttpVersion::Http11,
    }
}

async fn read_http_head(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 512];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&buf);
    text.lines().next().map(str::to_owned)
}

async fn vuln_origin(listener: TcpListener) {
    while let Ok((mut stream, _)) = listener.accept().await {
        tokio::spawn(async move {
            let Some(request_line) = read_http_head(&mut stream).await else {
                return;
            };
            let target = request_line.split_whitespace().nth(1).unwrap_or("/");
            let parsed = Url::parse(&format!("http://127.0.0.1{target}"))
                .unwrap_or_else(|_| Url::parse("http://127.0.0.1/").unwrap());
            let mut q = String::new();
            let mut id = String::new();
            let mut next = String::new();
            for (k, v) in parsed.query_pairs() {
                match k.as_ref() {
                    "q" => q = v.into_owned(),
                    "id" => id = v.into_owned(),
                    "next" => next = v.into_owned(),
                    _ => {}
                }
            }
            let response = if next.starts_with(ACTIVE_REDIRECT_TARGET) {
                let body = b"redirecting";
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: {next}\r\nContent-Length: {}\r\nConnection: close\r\n\r\nredirecting",
                    body.len()
                )
            } else if id.contains('\'') {
                let body = "You have an error in your SQL syntax near '''";
                format!(
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            } else {
                let body = format!("<html><body>search:{q}</body></html>");
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nSet-Cookie: sid=abc123; Path=/\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            };
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
    }
}

#[tokio::test]
async fn active_and_passive_scan_detects_vulns_and_persists_readable_probe_flows() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let origin = tokio::spawn(vuln_origin(listener));

    let (events_tx, mut events_rx) = mpsc::channel(128);
    let mut state = State::new(events_tx, CancellationToken::new()).unwrap();
    let project = state.project;
    let task_id = Uuid::new_v4();

    state
        .audit_command(AuditCommand::StartScan {
            project,
            task_id,
            title: "Vuln Endpoint Audit".to_owned(),
            draft: Box::new(draft_for(addr, "/search?q=hello&id=1&next=/home")),
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
        let is_done = matches!(sig.kind, AuditSignalKind::Finished { .. });
        state.on_audit_signal(sig).await;
        while events_rx.try_recv().is_ok() {}
        if is_done {
            break;
        }
    }

    let snapshot = state.store.read_audit_state().unwrap();
    let task = snapshot.tasks.iter().find(|t| t.id == task_id).unwrap();
    assert_eq!(task.state, AuditTaskState::Completed);
    assert_eq!(task.requests_sent, 16);
    assert!(task.issues_found >= 4);

    for expected_issue in ["0x00200300", "0x00100200", "0x00500100", "0x00500600"] {
        let finding = snapshot
            .findings
            .iter()
            .find(|f| f.issue_id == expected_issue)
            .unwrap_or_else(|| panic!("missing issue {expected_issue}"));
        let flow_id = finding.flow_id.expect("finding bound to persisted flow_id");
        state
            .command(Command::ReadFlow {
                project,
                selection_revision: 1,
                flow_id,
            })
            .await
            .unwrap();
        let mut found_detail = None;
        while let Ok(ev) = events_rx.try_recv() {
            if let Event::FlowDetail { record, .. } = ev {
                found_detail = Some(record);
            }
        }
        let detail = found_detail.expect("Event::FlowDetail emitted for probe flow");
        assert_eq!(detail.summary.id, flow_id);
        assert_eq!(detail.summary.source, TrafficSource::Scanner);
        assert!(detail.response.is_some());
    }
    let history_query = crate::HttpHistoryQuery {
        project,
        query_revision: 1,
        offset: 0,
        limit: 50,
        url_filter: String::new(),
        filter: crate::HttpHistoryFilter::default(),
        sort: crate::HttpHistorySort::default(),
        selected_ids: Vec::new(),
    };
    let proxy_page = state
        .store
        .query_http_history(&history_query, &CancellationToken::new())
        .expect("Scanner flows must not corrupt Proxy HTTP history source validation");
    assert_eq!(proxy_page.total, 0);
    let (logger_total, logger_rows) = state
        .store
        .logger_history_page(
            0,
            50,
            "",
            Some(&TrafficSource::Scanner),
            &CancellationToken::new(),
        )
        .expect("Logger history must accept Scanner source");
    assert!(logger_total >= 4 && !logger_rows.is_empty());
    state.collaborator.shutdown().await.unwrap();
    origin.abort();
}

#[tokio::test]
async fn stop_scan_cancels_in_flight_probes_within_bounded_deadline() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let active_probes = Arc::new(AtomicUsize::new(0));
    let probes_counter = active_probes.clone();
    let origin = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let counter = probes_counter.clone();
            tokio::spawn(async move {
                let Some(line) = read_http_head(&mut stream).await else {
                    return;
                };
                let is_baseline = line.starts_with("GET /slow?a=1&b=2&c=3 ");
                if is_baseline {
                    let resp =
                        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
                    let _ = stream.write_all(resp.as_bytes()).await;
                } else {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let mut sink = [0_u8; 64];
                    let _ = stream.read(&mut sink).await;
                }
            });
        }
    });

    let (events_tx, mut events_rx) = mpsc::channel(64);
    let mut state = State::new(events_tx, CancellationToken::new()).unwrap();
    let project = state.project;
    let task_id = Uuid::new_v4();

    state
        .audit_command(AuditCommand::StartScan {
            project,
            task_id,
            title: "Slow Scan".to_owned(),
            draft: Box::new(draft_for(addr, "/slow?a=1&b=2&c=3")),
            mode: AuditTaskMode::PassiveAndActive,
            crawl_same_origin: false,
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();

    let wait_start = Instant::now();
    while active_probes.load(Ordering::SeqCst) == 0 {
        assert!(wait_start.elapsed() < Duration::from_secs(2));
        while let Ok(Some(sig)) =
            timeout(Duration::from_millis(20), state.audit.next_signal()).await
        {
            state.on_audit_signal(sig).await;
        }
        sleep(Duration::from_millis(10)).await;
    }

    let stop_started = Instant::now();
    state
        .audit_command(AuditCommand::StopScan { project, task_id })
        .await
        .unwrap();
    loop {
        let sig = timeout(Duration::from_secs(3), state.audit.next_signal())
            .await
            .expect("StopScan must finish within 3s")
            .unwrap();
        let done = matches!(sig.kind, AuditSignalKind::Finished { .. });
        state.on_audit_signal(sig).await;
        while events_rx.try_recv().is_ok() {}
        if done {
            break;
        }
    }
    assert!(stop_started.elapsed() < Duration::from_secs(3));
    assert!(!state.audit.has_resources());
    assert!(!state.audit.has_in_flight_jobs());
    let snapshot = state.store.read_audit_state().unwrap();
    assert_eq!(snapshot.tasks[0].state, AuditTaskState::Cancelled);
    state.collaborator.shutdown().await.unwrap();
    origin.abort();
}

#[tokio::test]
async fn pause_and_resume_scan_gates_active_probes_and_triage_updates_findings() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let probe_hits = Arc::new(AtomicUsize::new(0));
    let hits_srv = probe_hits.clone();
    let origin = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let hits = hits_srv.clone();
            tokio::spawn(async move {
                let Some(line) = read_http_head(&mut stream).await else {
                    return;
                };
                let is_base = line.starts_with("GET /gate?a=1&b=2&c=3&d=4&e=5 ");
                if !is_base {
                    hits.fetch_add(1, Ordering::SeqCst);
                    sleep(Duration::from_millis(25)).await;
                }
                let body = format!("<html><body>echo:{line}</body></html>");
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nSet-Cookie: sid=1; Path=/\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });

    let (events_tx, mut events_rx) = mpsc::channel(128);
    let mut state = State::new(events_tx, CancellationToken::new()).unwrap();
    let project = state.project;
    let task_id = Uuid::new_v4();

    state
        .audit_command(AuditCommand::StartScan {
            project,
            task_id,
            title: "Pauseable Scan".to_owned(),
            draft: Box::new(draft_for(addr, "/gate?a=1&b=2&c=3&d=4&e=5")),
            mode: AuditTaskMode::PassiveAndActive,
            crawl_same_origin: false,
            cancel: CancellationToken::new(),
        })
        .await
        .unwrap();

    let wait_start = Instant::now();
    while probe_hits.load(Ordering::SeqCst) == 0 {
        assert!(wait_start.elapsed() < Duration::from_secs(2));
        while let Ok(Some(sig)) =
            timeout(Duration::from_millis(10), state.audit.next_signal()).await
        {
            state.on_audit_signal(sig).await;
        }
        sleep(Duration::from_millis(5)).await;
    }

    state
        .audit_command(AuditCommand::PauseScan { project, task_id })
        .await
        .unwrap();
    assert_eq!(
        state.store.read_audit_state().unwrap().tasks[0].state,
        AuditTaskState::Paused
    );

    sleep(Duration::from_millis(60)).await;
    while let Ok(Some(sig)) = timeout(Duration::from_millis(10), state.audit.next_signal()).await {
        state.on_audit_signal(sig).await;
    }
    let paused_count = probe_hits.load(Ordering::SeqCst);
    assert!(
        paused_count < 20,
        "scan should pause before sending all probes"
    );
    sleep(Duration::from_millis(60)).await;
    assert_eq!(
        probe_hits.load(Ordering::SeqCst),
        paused_count,
        "no new probes should be sent while paused"
    );

    state
        .audit_command(AuditCommand::ResumeScan { project, task_id })
        .await
        .unwrap();
    assert_eq!(
        state.store.read_audit_state().unwrap().tasks[0].state,
        AuditTaskState::Running
    );

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

    let snap = state.store.read_audit_state().unwrap();
    assert_eq!(snap.tasks[0].state, AuditTaskState::Completed);
    assert!(probe_hits.load(Ordering::SeqCst) > paused_count);
    let first_id = snap.findings[0].id;

    state
        .audit_command(AuditCommand::SetFindingSeverity {
            project,
            finding_id: first_id,
            severity: IssueSeverity::Information,
            confidence: IssueConfidence::Tentative,
        })
        .await
        .unwrap();
    let updated = state
        .store
        .read_audit_state()
        .unwrap()
        .findings
        .into_iter()
        .find(|f| f.id == first_id)
        .unwrap();
    assert_eq!(updated.severity(), IssueSeverity::Information);
    assert_eq!(updated.confidence, IssueConfidence::Tentative);

    state
        .audit_command(AuditCommand::DeleteFinding {
            project,
            finding_id: first_id,
        })
        .await
        .unwrap();
    assert!(
        !state
            .store
            .read_audit_state()
            .unwrap()
            .findings
            .iter()
            .any(|f| f.id == first_id)
    );
    state.collaborator.shutdown().await.unwrap();
    origin.abort();
}
