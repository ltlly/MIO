use std::{net::SocketAddr, path::Path, time::Instant};

use anyhow::{Result, ensure};
use mio_core::{
    Client, Command, Event, HttpHistoryFilter, HttpHistoryQuery, HttpHistoryResult,
    HttpHistorySort, HttpHistorySortColumn, ProjectKey,
};

use super::{
    BenchConfig,
    metrics::{
        LatencySummary, ProxyReport, QueryCaseReport, SaveAndReopenReport, TreeSampler,
        ensure_fresh_mio_path,
    },
    origin::fetch_origin_stats,
    proxy_load::{
        drive_proxy_http_load, measure_proxy_active_stop, recv_matching, save_project_cmd,
        send_cmd, start_proxy_cmd, stop_proxy_cmd, wait_project_opened,
    },
};

fn make_history_query(
    project: ProjectKey,
    query_revision: u64,
    url_filter: &str,
    filter: HttpHistoryFilter,
    sort: HttpHistorySort,
) -> HttpHistoryQuery {
    HttpHistoryQuery {
        project,
        query_revision,
        offset: 0,
        limit: 200,
        url_filter: url_filter.to_owned(),
        filter,
        sort,
        selected_ids: Vec::new(),
    }
}

async fn query_history_once(
    client: &mut Client,
    query: HttpHistoryQuery,
) -> Result<(u64, HttpHistoryResult)> {
    let project = query.project;
    let rev = query.query_revision;
    let started = Instant::now();
    send_cmd(client, Command::QueryHttpHistory { query }).await?;
    let ev = recv_matching(client, |e| {
        matches!(
            e,
            Event::HttpHistoryPage { query: q, .. } if q.project == project && q.query_revision == rev
        )
    })
    .await?;
    let elapsed_us = started.elapsed().as_micros().min(u64::MAX as u128) as u64;
    let Event::HttpHistoryPage { result, .. } = ev else {
        unreachable!()
    };
    let page = result.map_err(|err| anyhow::anyhow!("QueryHttpHistory failed: {err}"))?;
    Ok((elapsed_us, page))
}

async fn benchmark_history_queries(
    client: &mut Client,
    project: ProjectKey,
    iters: usize,
    rev: &mut u64,
) -> Result<Vec<QueryCaseReport>> {
    let mut script_mime = [false; 8];
    script_mime[1] = true;
    let sort = |column, ascending| HttpHistorySort { column, ascending };
    let def_sort = HttpHistorySort::default();
    let def_filter = HttpHistoryFilter::default;
    let cases: Vec<(&str, String, HttpHistoryFilter, HttpHistorySort)> = vec![
        (
            "sort_number_desc",
            String::new(),
            def_filter(),
            sort(HttpHistorySortColumn::Number, false),
        ),
        (
            "sort_length_asc",
            String::new(),
            def_filter(),
            sort(HttpHistorySortColumn::Length, true),
        ),
        (
            "sort_status_url_filter",
            "kind=script".into(),
            def_filter(),
            sort(HttpHistorySortColumn::Status, true),
        ),
        (
            "filter_status_4xx",
            String::new(),
            HttpHistoryFilter {
                status_classes: [false, false, true, false],
                ..def_filter()
            },
            def_sort,
        ),
        (
            "filter_parameterized_only",
            String::new(),
            HttpHistoryFilter {
                parameterized_only: true,
                ..def_filter()
            },
            def_sort,
        ),
        (
            "filter_mime_script",
            String::new(),
            HttpHistoryFilter {
                mime_types: script_mime,
                ..def_filter()
            },
            def_sort,
        ),
        (
            "search_literal_body",
            String::new(),
            HttpHistoryFilter {
                search_term: "alpha-script".into(),
                ..def_filter()
            },
            def_sort,
        ),
        (
            "search_regex_body",
            String::new(),
            HttpHistoryFilter {
                search_term: "seq=(1|2) ".into(),
                regex: true,
                ..def_filter()
            },
            def_sort,
        ),
    ];
    let mut reports = Vec::with_capacity(cases.len());
    for (name, url_filter, filter, sort) in cases {
        *rev += 1;
        let (_, warmup) = query_history_once(
            client,
            make_history_query(project, *rev, &url_filter, filter.clone(), sort),
        )
        .await?;
        let expected_total = warmup.total;
        let mut samples = Vec::with_capacity(iters);
        for _ in 0..iters {
            *rev += 1;
            let (us, res) = query_history_once(
                client,
                make_history_query(project, *rev, &url_filter, filter.clone(), sort),
            )
            .await?;
            ensure!(res.total == expected_total, "query {name} total unstable");
            samples.push(us);
        }
        reports.push(QueryCaseReport {
            name: name.into(),
            total_matched: expected_total,
            iterations: iters,
            latency_us: LatencySummary::from_samples(&mut samples),
        });
    }
    Ok(reports)
}

pub(crate) async fn run_proxy_and_queries(
    client: &mut Client,
    cfg: &BenchConfig,
    work_dir: &Path,
    origin: SocketAddr,
    sampler: &TreeSampler,
) -> Result<(ProxyReport, Vec<QueryCaseReport>, SaveAndReopenReport)> {
    let proxy_path = work_dir.join("proxy-bench.mio");
    let save_as_path = work_dir.join("proxy-save-as.mio");
    ensure_fresh_mio_path(&proxy_path)?;
    ensure_fresh_mio_path(&save_as_path)?;

    send_cmd(
        client,
        Command::NewProject {
            path: Some(proxy_path.clone()),
        },
    )
    .await?;
    let (project, _) = wait_project_opened(client, Some(&proxy_path)).await?;
    let proxy_addr = start_proxy_cmd(client, project).await?;

    let before_stats = fetch_origin_stats(origin).await?;
    let (wall_ms, latency_us) =
        drive_proxy_http_load(proxy_addr, origin, cfg.proxy_count, cfg.proxy_concurrency).await?;
    stop_proxy_cmd(client, project).await?;
    let after_stats = fetch_origin_stats(origin).await?;
    let origin_observed = after_stats
        .bench_requests
        .saturating_sub(before_stats.bench_requests);
    ensure!(
        origin_observed == cfg.proxy_count,
        "origin observed {origin_observed} != {}",
        cfg.proxy_count
    );
    sampler.snapshot_now();

    let mut query_rev = 1_u64;
    let (_, initial_page) = query_history_once(
        client,
        make_history_query(
            project,
            query_rev,
            "",
            HttpHistoryFilter::default(),
            HttpHistorySort::default(),
        ),
    )
    .await?;
    ensure!(
        initial_page.total == cfg.proxy_count,
        "persisted proxy history {} != {}",
        initial_page.total,
        cfg.proxy_count
    );

    let queries =
        benchmark_history_queries(client, project, cfg.query_iters, &mut query_rev).await?;
    let save_ms = save_project_cmd(client, project, None).await?;
    let save_as_ms = save_project_cmd(client, project, Some(save_as_path.clone())).await?;
    let save_as_peak_tree_bytes = sampler.snapshot_now();

    send_cmd(client, Command::NewProject { path: None }).await?;
    wait_project_opened(client, None).await?;

    let reopen_started = Instant::now();
    send_cmd(
        client,
        Command::OpenProject {
            path: save_as_path.clone(),
        },
    )
    .await?;
    let (reopened_project, _) = wait_project_opened(client, Some(&save_as_path)).await?;
    let open_ack_ms = reopen_started.elapsed().as_secs_f64() * 1000.0;
    let first_page_started = Instant::now();
    let (_, reopened_page) = query_history_once(
        client,
        make_history_query(
            reopened_project,
            1,
            "",
            HttpHistoryFilter::default(),
            HttpHistorySort::default(),
        ),
    )
    .await?;
    let first_page_ready_ms = first_page_started.elapsed().as_secs_f64() * 1000.0;
    let warm_reopen_total_ms = reopen_started.elapsed().as_secs_f64() * 1000.0;
    ensure!(
        reopened_page.total == cfg.proxy_count,
        "warm reopened total {} != {}",
        reopened_page.total,
        cfg.proxy_count
    );

    let held_conns = cfg.proxy_concurrency.clamp(2, 8);
    let stop_reclaim = measure_proxy_active_stop(client, origin, held_conns).await?;
    let throughput_rps = (cfg.proxy_count as f64) / (wall_ms / 1000.0).max(1e-9);

    Ok((
        ProxyReport {
            requests: cfg.proxy_count,
            origin_observed,
            concurrency: cfg.proxy_concurrency,
            wall_ms,
            throughput_rps,
            latency_us,
            stop_reclaim,
        },
        queries,
        SaveAndReopenReport {
            save_ms,
            save_as_ms,
            save_as_peak_tree_bytes,
            open_ack_ms,
            first_page_ready_ms,
            warm_reopen_total_ms,
            reopened_total_rows: reopened_page.total,
        },
    ))
}
