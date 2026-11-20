use std::{
    net::SocketAddr,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use mio_core::{
    Client, Command, Draft, Event, FlowState, HttpVersion, IntruderResultOutcome,
    IntruderResultPage, IntruderRunState, IntruderSpec, IntruderTab, ProjectKey, Uuid, Workspace,
    intruder::{AttackType, Position},
};
use tokio_util::sync::CancellationToken;

use super::{
    BenchConfig,
    metrics::{
        IntruderReport, IntruderStopReport, IntruderThroughputReport, LatencySummary, TreeSampler,
        ensure_fresh_mio_path,
    },
    origin::fetch_origin_stats,
    proxy_load::{recv_matching, save_project_cmd, send_cmd, wait_project_opened},
};

fn build_sniper_spec(
    origin: SocketAddr,
    method_and_prefix: &str,
    suffix: &str,
    body: &[u8],
    payload_count: usize,
    concurrency: u16,
) -> IntruderSpec {
    let marker = b"0";
    let head = format!(
        "{method_and_prefix}{}{suffix} HTTP/1.1\r\nHost: {origin}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
        std::str::from_utf8(marker).expect("ascii"),
        body.len()
    );
    let start = method_and_prefix.len();
    let end = start + marker.len();
    let mut raw = head.into_bytes();
    raw.extend_from_slice(body);
    let payloads: Vec<Vec<u8>> = (0..payload_count)
        .map(|idx| idx.to_string().into_bytes())
        .collect();
    IntruderSpec {
        revision: 1,
        draft: Draft {
            revision: 1,
            target: format!("http://{origin}"),
            raw,
            version: HttpVersion::Http11,
        },
        attack_type: AttackType::Sniper,
        positions: vec![Position { start, end }],
        payload_sets: vec![payloads],
        max_concurrent_requests: concurrency,
        request_timeout_ms: 30_000,
        update_host_header: true,
        save_to_project: true,
        baseline_request: false,
        resource_pool: None,
    }
}

async fn bind_intruder_tab(
    client: &mut Client,
    project: ProjectKey,
    workspace: &mut Workspace,
    title: &str,
    spec: &IntruderSpec,
) -> Result<Uuid> {
    let tab_id = Uuid::new_v4();
    workspace.intruder.tabs.push(IntruderTab {
        id: tab_id,
        title: title.into(),
        group: None,
        spec: spec.clone(),
        last_run: None,
    });
    workspace.intruder.active = Some(tab_id);
    workspace.revision += 1;
    send_cmd(
        client,
        Command::SaveWorkspace {
            project,
            workspace: workspace.clone(),
        },
    )
    .await?;
    let ev = recv_matching(client, |e| {
        matches!(
            e,
            Event::WorkspaceChanged { project: p, .. } | Event::WorkspaceRejected { project: p, .. }
                if *p == project
        )
    })
    .await?;
    match ev {
        Event::WorkspaceChanged { .. } => Ok(tab_id),
        Event::WorkspaceRejected { message, .. } => {
            bail!("Intruder workspace save rejected: {message}")
        }
        _ => unreachable!(),
    }
}

async fn query_intruder_page(
    client: &mut Client,
    project: ProjectKey,
    run_id: Uuid,
    rev: &mut u64,
    after: Option<u64>,
    limit: usize,
) -> Result<IntruderResultPage> {
    *rev += 1;
    let qrev = *rev;
    send_cmd(
        client,
        Command::QueryIntruderResults {
            project,
            run_id,
            query_revision: qrev,
            after,
            limit,
        },
    )
    .await?;
    let ev = recv_matching(client, |e| {
        matches!(
            e,
            Event::IntruderResults { project: p, run_id: r, query_revision: q, .. }
                if *p == project && *r == run_id && *q == qrev
        )
    })
    .await?;
    let Event::IntruderResults { result, .. } = ev else {
        unreachable!()
    };
    result.map_err(|err| anyhow::anyhow!("QueryIntruderResults failed: {err}"))
}

async fn wait_intruder_terminal(
    client: &mut Client,
    project: ProjectKey,
    run_id: Uuid,
    rev: &mut u64,
) -> Result<IntruderResultPage> {
    loop {
        let page = query_intruder_page(client, project, run_id, rev, None, 1).await?;
        if page.run.state.is_terminal() {
            return Ok(page);
        }
        recv_matching(
            client,
            |e| matches!(e, Event::IntruderChanged { project: p } if *p == project),
        )
        .await?;
    }
}

async fn run_throughput(
    client: &mut Client,
    project: ProjectKey,
    workspace: &mut Workspace,
    origin: SocketAddr,
    cfg: &BenchConfig,
) -> Result<IntruderThroughputReport> {
    let spec = build_sniper_spec(
        origin,
        "GET /bench?id=",
        "&tag=intruder",
        b"",
        cfg.intruder_count,
        cfg.intruder_concurrency,
    );
    let tab_id = bind_intruder_tab(client, project, workspace, "r5-throughput", &spec).await?;
    let run_id = Uuid::new_v4();
    let before = fetch_origin_stats(origin).await?;
    let started = Instant::now();
    send_cmd(
        client,
        Command::StartIntruder {
            project,
            run_id,
            tab_id,
            spec: Box::new(spec),
            cancel: CancellationToken::new(),
        },
    )
    .await?;
    let started_ev = recv_matching(client, |e| {
        matches!(e, Event::IntruderStarted { project: p, run_id: r, .. } if *p == project && *r == run_id)
    })
    .await?;
    if let Event::IntruderStarted { result: Err(e), .. } = started_ev {
        bail!("StartIntruder rejected: {e}");
    }
    let mut qrev = 0_u64;
    let terminal = wait_intruder_terminal(client, project, run_id, &mut qrev).await?;
    let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
    ensure!(
        terminal.run.state == IntruderRunState::Completed,
        "Intruder throughput run ended in {:?}",
        terminal.run.state
    );
    ensure!(
        terminal.run.completed == cfg.intruder_count as u64,
        "Intruder completed {} != {}",
        terminal.run.completed,
        cfg.intruder_count
    );
    let after = fetch_origin_stats(origin).await?;
    let origin_observed = after.bench_requests.saturating_sub(before.bench_requests);
    ensure!(
        origin_observed == cfg.intruder_count,
        "origin observed {origin_observed} != {}",
        cfg.intruder_count
    );

    let mut cursor = None;
    let mut elapsed_samples = Vec::with_capacity(cfg.intruder_count);
    loop {
        let page = query_intruder_page(client, project, run_id, &mut qrev, cursor, 200).await?;
        for row in &page.rows {
            let IntruderResultOutcome::Flow(flow) = &row.outcome else {
                bail!(
                    "unexpected rejected Intruder row at ordinal {}",
                    row.ordinal
                );
            };
            ensure!(
                flow.state == FlowState::Complete && flow.status == Some(200),
                "Intruder flow incomplete: {:?}",
                flow.state
            );
            elapsed_samples.push(flow.elapsed_ms);
        }
        cursor = page.next_after;
        if cursor.is_none() {
            break;
        }
    }
    ensure!(
        elapsed_samples.len() == cfg.intruder_count,
        "paginated Intruder rows {} != {}",
        elapsed_samples.len(),
        cfg.intruder_count
    );
    let throughput_rps = (cfg.intruder_count as f64) / (wall_ms / 1000.0).max(1e-9);
    Ok(IntruderThroughputReport {
        requests: cfg.intruder_count,
        origin_observed,
        concurrency: cfg.intruder_concurrency,
        wall_ms,
        throughput_rps,
        flow_elapsed_ms: LatencySummary::from_samples(&mut elapsed_samples),
    })
}

async fn run_stop_reclaim(
    client: &mut Client,
    project: ProjectKey,
    workspace: &mut Workspace,
    origin: SocketAddr,
    cfg: &BenchConfig,
) -> Result<IntruderStopReport> {
    let total_planned = cfg
        .hold_total
        .max(usize::from(cfg.intruder_concurrency) * 2);
    let target_held = usize::from(cfg.intruder_concurrency).min(total_planned);
    let spec = build_sniper_spec(
        origin,
        "POST /hold?slot=",
        "",
        b"ping",
        total_planned,
        cfg.intruder_concurrency,
    );
    let tab_id = bind_intruder_tab(client, project, workspace, "r5-stop", &spec).await?;
    let run_id = Uuid::new_v4();
    let before = fetch_origin_stats(origin).await?;
    send_cmd(
        client,
        Command::StartIntruder {
            project,
            run_id,
            tab_id,
            spec: Box::new(spec),
            cancel: CancellationToken::new(),
        },
    )
    .await?;
    let started_ev = recv_matching(client, |e| {
        matches!(e, Event::IntruderStarted { project: p, run_id: r, .. } if *p == project && *r == run_id)
    })
    .await?;
    if let Event::IntruderStarted { result: Err(e), .. } = started_ev {
        bail!("StartIntruder (hold) rejected: {e}");
    }

    let held_before_stop = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let st = fetch_origin_stats(origin).await?;
            if st.hold_active >= target_held {
                return Ok::<usize, anyhow::Error>(st.hold_active);
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("Intruder did not fill concurrency slots on /hold before deadline")??;

    let stop_started = Instant::now();
    send_cmd(client, Command::StopIntruder { project, run_id }).await?;
    let mut qrev = 0_u64;
    let terminal = wait_intruder_terminal(client, project, run_id, &mut qrev).await?;
    ensure!(
        terminal.run.state == IntruderRunState::Stopped,
        "expected Stopped state after StopIntruder, got {:?}",
        terminal.run.state
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if fetch_origin_stats(origin).await?.hold_active == 0 {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    let stop_reclaim_ms = stop_started.elapsed().as_secs_f64() * 1000.0;
    let after_stop = fetch_origin_stats(origin).await?;
    let post_check = fetch_origin_stats(origin).await?;
    let replayed_after_stop = post_check
        .hold_requests
        .saturating_sub(after_stop.hold_requests);
    ensure!(
        after_stop
            .hold_requests
            .saturating_sub(before.hold_requests)
            <= terminal.run.dispatched as usize,
        "origin observed more held requests than Intruder dispatched"
    );
    ensure!(
        replayed_after_stop == 0,
        "Intruder replayed requests after Stop"
    );
    Ok(IntruderStopReport {
        total_planned,
        held_before_stop,
        dispatched_at_stop: terminal.run.dispatched,
        completed_at_stop: terminal.run.completed,
        stop_reclaim_ms,
        origin_active_after_stop: after_stop.hold_active,
        replayed_after_stop,
    })
}

pub(crate) async fn run_intruder_benchmarks(
    client: &mut Client,
    cfg: &BenchConfig,
    work_dir: &Path,
    origin: SocketAddr,
    sampler: &TreeSampler,
) -> Result<IntruderReport> {
    let intruder_path = work_dir.join("intruder-bench.mio");
    ensure_fresh_mio_path(&intruder_path)?;
    send_cmd(
        client,
        Command::NewProject {
            path: Some(intruder_path.clone()),
        },
    )
    .await?;
    let (project, mut workspace) = wait_project_opened(client, Some(&intruder_path)).await?;

    let throughput = run_throughput(client, project, &mut workspace, origin, cfg).await?;
    sampler.snapshot_now();
    let stop_reclaim = run_stop_reclaim(client, project, &mut workspace, origin, cfg).await?;

    let _ = save_project_cmd(client, project, None).await?;
    sampler.snapshot_now();

    Ok(IntruderReport {
        throughput,
        stop_reclaim,
    })
}
