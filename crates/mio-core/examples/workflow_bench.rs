//! Multi-scenario end-to-end workflow benchmark covering:
//! 1) Cold runtime + IPC socket startup & warm `.mio` reopen,
//! 2) Proxy multi-format capture (JSON, HTML, PNG static assets) with Match & Replace provenance,
//! 3) Repeater HTTP + SSE streaming replay,
//! 4) Scanner active + passive audit workflow,
//! 5) AI IPC (`status`, `flows_list`, `flows_get`, `repeater_open`) roundtrip latency,
//! 6) Full `SaveProject` vs `SlimSaveProjectAs` timing, disk compression, and runtime join.

use std::{
    fs,
    net::{Ipv4Addr, SocketAddr},
    process::ExitCode,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use mio_core::{
    AiIpcPayload, AiIpcRequest, AuditCommand, AuditEvent, AuditTaskMode, AuditTaskState, Command,
    Draft, Event, FlowState, HistoryRow, HttpVersion, MatchAndReplaceRule, MatchReplaceTarget,
    RepeaterSettings, Runtime, SlimSaveOptions, Uuid, WorkTab, call_ai_ipc,
    current_process_memory_bytes, measure_project_disk_bytes,
};
use tokio_util::sync::CancellationToken;

#[path = "workflow_bench/helpers.rs"]
mod helpers;
use helpers::{
    WorkflowConfig, WorkflowReport, parse_args, recv_ev, send_via_proxy, spawn_workflow_origin,
};

fn run_workflow(cfg: &WorkflowConfig) -> Result<WorkflowReport> {
    let wall_start = Instant::now();
    let dir = tempfile::Builder::new()
        .prefix("mio-wf-")
        .tempdir_in("/tmp")?;
    let sock_path = dir.path().join("wf.sock");
    let full_path = dir.path().join("full.mio");
    let slim_path = dir.path().join("slim.mio");

    let cold_start = Instant::now();
    let (runtime, mut client) = Runtime::start_with_ipc_socket(sock_path.clone())?;
    let tokio_rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()?;

    let inner = tokio_rt.block_on(async {
        let (project, mut workspace) = recv_ev(&mut client, |ev| match ev {
            Event::ProjectOpened {
                project, workspace, ..
            } => Some((project, workspace)),
            _ => None,
        })
        .await?;
        let cold_startup_ms = cold_start.elapsed().as_secs_f64() * 1000.0;

        let origin_cancel = CancellationToken::new();
        let origin = spawn_workflow_origin(origin_cancel.clone()).await?;

        let http_tab_id = Uuid::new_v4();
        let http_draft = Draft {
            revision: 1,
            target: format!("http://{origin}"),
            raw: format!("GET /api/auth HTTP/1.1\r\nHost: {origin}\r\n\r\n").into_bytes(),
            version: HttpVersion::Http11,
        };
        let sse_tab_id = Uuid::new_v4();
        let sse_draft = Draft {
            revision: 1,
            target: format!("http://{origin}"),
            raw: format!(
                "GET /sse HTTP/1.1\r\nHost: {origin}\r\nAccept: text/event-stream\r\n\r\n"
            )
            .into_bytes(),
            version: HttpVersion::Http11,
        };
        workspace.tabs = vec![
            WorkTab {
                id: http_tab_id,
                title: "1: /api/auth".into(),
                group: None,
                draft: http_draft.clone(),
                last_flow: None,
                archive_response: None,
                history: None,
            },
            WorkTab {
                id: sse_tab_id,
                title: "2: /sse".into(),
                group: None,
                draft: sse_draft.clone(),
                last_flow: None,
                archive_response: None,
                history: None,
            },
        ];
        workspace.active = Some(http_tab_id);
        workspace.revision += 1;
        workspace.match_and_replace_rules.push(MatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            item_type: MatchReplaceTarget::RequestHeader,
            match_pattern: String::new(),
            replace_with: "X-Bench-Trace: enabled".into(),
            regex: false,
            comment: "bench trace header".into(),
            ..Default::default()
        });
        client
            .commands
            .send(Command::SaveWorkspace {
                project,
                workspace: workspace.clone(),
            })
            .await?;
        recv_ev(&mut client, |ev| match ev {
            Event::WorkspaceChanged { .. } => Some(()),
            _ => None,
        })
        .await?;

        client
            .commands
            .send(Command::StartProxy {
                project,
                listen: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            })
            .await?;
        let proxy_addr = recv_ev(&mut client, |ev| match ev {
            Event::ProxyState {
                address: Some(address),
                ..
            } => Some(address),
            _ => None,
        })
        .await?;

        let capture_start = Instant::now();
        send_via_proxy(proxy_addr, origin, "/api/auth").await?;
        send_via_proxy(proxy_addr, origin, "/vuln?q=1").await?;
        for idx in 0..cfg.static_assets {
            send_via_proxy(proxy_addr, origin, &format!("/static/logo-{idx}.png")).await?;
        }
        let expected_captured = cfg.static_assets + 2;
        let mut qrev = 1u64;
        let api_flow_id = loop {
            client
                .commands
                .send(Command::QueryHistory {
                    project,
                    query_revision: qrev,
                    offset: 0,
                    limit: 500,
                    filter: String::new(),
                })
                .await?;
            qrev += 1;
            let rows = recv_ev(&mut client, |ev| match ev {
                Event::HistoryPage { rows, .. } => Some(rows),
                _ => None,
            })
            .await?;
            let completed = rows
                .iter()
                .filter(|r| matches!(r, HistoryRow::Live(s) if s.state == FlowState::Complete))
                .count();
            if completed >= expected_captured {
                break rows.iter().find_map(|r| match r {
                    HistoryRow::Live(s) if s.url.contains("/api/auth") => Some(s.id),
                    _ => None,
                });
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        let proxy_capture_ms = capture_start.elapsed().as_secs_f64() * 1000.0;
        let api_flow = api_flow_id.context("missing /api/auth flow")?;

        let rep_start = Instant::now();
        client
            .commands
            .send(Command::Send {
                project,
                tab_id: http_tab_id,
                run_id: Uuid::new_v4(),
                draft: http_draft,
                settings: RepeaterSettings::default(),
                cancel: CancellationToken::new(),
            })
            .await?;
        recv_ev(&mut client, |ev| match ev {
            Event::SendFinished { .. } => Some(()),
            _ => None,
        })
        .await?;
        let repeater_send_ms = rep_start.elapsed().as_secs_f64() * 1000.0;

        let sse_start = Instant::now();
        client
            .commands
            .send(Command::Send {
                project,
                tab_id: sse_tab_id,
                run_id: Uuid::new_v4(),
                draft: sse_draft,
                settings: RepeaterSettings::default(),
                cancel: CancellationToken::new(),
            })
            .await?;
        recv_ev(&mut client, |ev| match ev {
            Event::SendFinished { .. } => Some(()),
            _ => None,
        })
        .await?;
        let sse_stream_ms = sse_start.elapsed().as_secs_f64() * 1000.0;

        let scan_start = Instant::now();
        let task_id = Uuid::new_v4();
        client
            .commands
            .send(Command::Audit(AuditCommand::StartScan {
                project,
                task_id,
                title: "Workflow Bench Scan".into(),
                draft: Box::new(Draft {
                    revision: 1,
                    target: format!("http://{origin}"),
                    raw: format!("GET /vuln?id=1&q=hello HTTP/1.1\r\nHost: {origin}\r\n\r\n")
                        .into_bytes(),
                    version: HttpVersion::Http11,
                }),
                mode: AuditTaskMode::PassiveAndActive,
                crawl_same_origin: false,
                cancel: CancellationToken::new(),
            }))
            .await?;
        recv_ev(&mut client, |ev| match ev {
            Event::Audit(AuditEvent::TaskUpdated { task, .. })
                if task.id == task_id && task.state == AuditTaskState::Completed =>
            {
                Some(())
            }
            _ => None,
        })
        .await?;
        client
            .commands
            .send(Command::Audit(AuditCommand::QueryState {
                project,
                query_revision: 1,
            }))
            .await?;
        let scanner_findings = recv_ev(&mut client, |ev| match ev {
            Event::Audit(AuditEvent::State {
                result: Ok(snap), ..
            }) => Some(snap.findings.len()),
            _ => None,
        })
        .await?;
        let scanner_audit_ms = scan_start.elapsed().as_secs_f64() * 1000.0;

        let sock_bg = sock_path.clone();
        let iters = cfg.ipc_iters;
        let (mut samples_us, prov_steps) = tokio::task::spawn_blocking(move || -> Result<_> {
            let mut samples = Vec::with_capacity(iters);
            let mut steps = 0usize;
            for i in 0..iters {
                let t0 = Instant::now();
                let req = match i % 3 {
                    0 => AiIpcRequest::Status,
                    1 => AiIpcRequest::FlowsList {
                        offset: 0,
                        limit: 20,
                        filter: String::new(),
                        method: None,
                        status: None,
                    },
                    _ => AiIpcRequest::FlowsGet { flow_id: api_flow },
                };
                let (env, _) = call_ai_ipc(&sock_bg, &req, Duration::from_secs(5))?;
                ensure!(env.ok, "IPC call failed: {:?}", env.error);
                if let Some(AiIpcPayload::FlowsGet(detail)) = env.payload {
                    steps = steps.max(detail.provenance.request_steps.len());
                }
                samples.push(t0.elapsed().as_micros() as u64);
            }
            let (open_env, _) = call_ai_ipc(
                &sock_bg,
                &AiIpcRequest::RepeaterOpen {
                    flow_id: Some(api_flow),
                    use_original: false,
                    target: None,
                    raw_request: None,
                    title: None,
                },
                Duration::from_secs(5),
            )?;
            ensure!(open_env.ok, "IPC repeater_open failed");
            Ok((samples, steps))
        })
        .await??;
        samples_us.sort_unstable();
        let ipc_p50 = samples_us[samples_us.len() / 2];
        let ipc_p95 = samples_us[(samples_us.len() * 95 / 100).min(samples_us.len() - 1)];

        client.commands.send(Command::StopProxy { project }).await?;
        recv_ev(&mut client, |ev| match ev {
            Event::ProxyState { address: None, .. } => Some(()),
            _ => None,
        })
        .await?;
        origin_cancel.cancel();

        let save_t0 = Instant::now();
        client
            .commands
            .send(Command::SaveProject {
                project,
                path: Some(full_path.clone()),
                workspace: None,
                websocket_notes: Vec::new(),
                ext_http_notes: Vec::new(),
                ext_http_highlights: Vec::new(),
            })
            .await?;
        let full_project_bytes = recv_ev(&mut client, |ev| match ev {
            Event::ProjectSaved { total_bytes, .. } => Some(total_bytes),
            _ => None,
        })
        .await?;
        let full_save_ms = save_t0.elapsed().as_secs_f64() * 1000.0;

        let slim_t0 = Instant::now();
        client
            .commands
            .send(Command::SlimSaveProjectAs {
                project,
                path: slim_path.clone(),
                options: SlimSaveOptions::default(),
                cancel: CancellationToken::new(),
            })
            .await?;
        let slim_report = recv_ev(&mut client, |ev| match ev {
            Event::ProjectSlimSaved { result, .. } => Some(result),
            _ => None,
        })
        .await?
        .map_err(|e| anyhow::anyhow!(e))?;
        let slim_save_ms = slim_t0.elapsed().as_secs_f64() * 1000.0;
        let slim_project_bytes = measure_project_disk_bytes(&slim_path)?;

        let reopen_t0 = Instant::now();
        client
            .commands
            .send(Command::OpenProject {
                path: slim_path.clone(),
            })
            .await?;
        recv_ev(&mut client, |ev| match ev {
            Event::ProjectOpened { path: Some(p), .. } if p == slim_path => Some(()),
            _ => None,
        })
        .await?;
        let warm_reopen_ms = reopen_t0.elapsed().as_secs_f64() * 1000.0;

        Ok::<_, anyhow::Error>((
            cold_startup_ms,
            proxy_capture_ms,
            prov_steps,
            repeater_send_ms,
            sse_stream_ms,
            scanner_audit_ms,
            scanner_findings,
            ipc_p50,
            ipc_p95,
            full_save_ms,
            slim_save_ms,
            full_project_bytes,
            slim_project_bytes,
            slim_report.removed_flows,
            warm_reopen_ms,
        ))
    })?;

    client.shutdown.cancel();
    drop(client);
    runtime.shutdown()?;

    let (
        cold_startup_ms,
        proxy_capture_ms,
        provenance_steps_verified,
        repeater_send_ms,
        sse_stream_ms,
        scanner_audit_ms,
        scanner_findings,
        ipc_roundtrip_p50_us,
        ipc_roundtrip_p95_us,
        full_save_ms,
        slim_save_ms,
        full_project_bytes,
        slim_project_bytes,
        slim_removed_flows,
        warm_reopen_ms,
    ) = inner;
    let slim_reduction_percent = if full_project_bytes > 0 {
        (1.0 - (slim_project_bytes as f64 / full_project_bytes as f64)) * 100.0
    } else {
        0.0
    };
    let report = WorkflowReport {
        cold_startup_ms,
        proxy_capture_ms,
        provenance_steps_verified,
        repeater_send_ms,
        sse_stream_ms,
        scanner_audit_ms,
        scanner_findings,
        ipc_roundtrip_p50_us,
        ipc_roundtrip_p95_us,
        full_save_ms,
        slim_save_ms,
        full_project_bytes,
        slim_project_bytes,
        slim_reduction_percent,
        slim_removed_flows,
        warm_reopen_ms,
        process_memory_bytes: current_process_memory_bytes(),
        runtime_join_confirmed: true,
        total_wall_ms: wall_start.elapsed().as_secs_f64() * 1000.0,
    };
    fs::write(&cfg.out, serde_json::to_vec_pretty(&report)?)?;
    Ok(report)
}

fn main() -> ExitCode {
    match parse_args(std::env::args_os().skip(1)).and_then(|cfg| run_workflow(&cfg)) {
        Ok(r) => {
            println!(
                "WORKFLOW_BENCH_OK cold_ms={:.1} ipc_p50_us={} scan_ms={:.1} slim_pct={:.1}% ({} -> {} B)",
                r.cold_startup_ms,
                r.ipc_roundtrip_p50_us,
                r.scanner_audit_ms,
                r.slim_reduction_percent,
                r.full_project_bytes,
                r.slim_project_bytes
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("workflow_bench failed: {err:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_smoke_covers_provenance_ipc_scan_and_slim_save() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("wf.json");
        let report = run_workflow(&WorkflowConfig {
            out: out.clone(),
            static_assets: 4,
            ipc_iters: 6,
        })
        .unwrap();
        assert!(report.runtime_join_confirmed);
        assert!(report.provenance_steps_verified >= 2);
        assert!(report.scanner_findings >= 1);
        assert!(report.slim_removed_flows >= 4);
        assert!(report.slim_project_bytes < report.full_project_bytes);
        assert!(out.is_file());
    }
}
