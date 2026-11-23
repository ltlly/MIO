//! Bounded R5 loopback benchmark over `mio_core::Runtime` (Proxy, HTTP history
//! queries, Save/SaveAs/Warm-reopen, Intruder 16-way dispatch & Stop reclaim).

use std::{
    ffi::OsString,
    fs,
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    process::ExitCode,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use mio_core::Runtime;

#[path = "r5_bench/intruder.rs"]
mod intruder;
#[path = "r5_bench/metrics.rs"]
mod metrics;
#[path = "r5_bench/origin.rs"]
mod origin;
#[path = "r5_bench/proxy_load.rs"]
mod proxy_load;
#[path = "r5_bench/proxy_query.rs"]
mod proxy_query;

use intruder::run_intruder_benchmarks;
use metrics::{
    BenchConfigSummary, BenchReport, DiskReport, ProcessReport, TreeSampler, max_rss_bytes,
    measure_sqlite_files,
};
use origin::BenchOrigin;
use proxy_load::wait_project_opened;
use proxy_query::run_proxy_and_queries;

const TOTAL_BUDGET: Duration = Duration::from_secs(600);
const WORK_BUDGET: Duration = Duration::from_secs(570);
const USAGE: &str = "usage: r5_bench --out <report.json> [--work-dir <dir>] [--proxy-count <n>] [--proxy-concurrency <n>] [--query-iters <n>] [--intruder-count <n>] [--intruder-concurrency <n>] [--hold-total <n>] [--origin 127.0.0.1:PORT]";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BenchConfig {
    pub out: PathBuf,
    pub work_dir: Option<PathBuf>,
    pub proxy_count: usize,
    pub proxy_concurrency: usize,
    pub query_iters: usize,
    pub intruder_count: usize,
    pub intruder_concurrency: u16,
    pub hold_total: usize,
    pub origin: Option<SocketAddr>,
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<BenchConfig> {
    let raw: Vec<String> = args
        .into_iter()
        .map(|arg| {
            arg.into_string()
                .map_err(|_| anyhow::anyhow!("non-UTF-8 CLI argument"))
        })
        .collect::<Result<_>>()?;
    let mut out: Option<PathBuf> = None;
    let mut work_dir: Option<PathBuf> = None;
    let mut proxy_count: usize = 5_000;
    let mut proxy_concurrency: usize = 16;
    let mut query_iters: usize = 25;
    let mut intruder_count: usize = 1_000;
    let mut intruder_concurrency: u16 = 16;
    let mut hold_total: usize = 200;
    let mut origin: Option<SocketAddr> = None;

    let mut idx = 0;
    while idx < raw.len() {
        let flag = raw[idx].as_str();
        let value = raw
            .get(idx + 1)
            .with_context(|| format!("missing value for {flag}; {USAGE}"))?;
        match flag {
            "--out" => {
                ensure!(
                    out.is_none() && !value.is_empty(),
                    "duplicate or empty --out"
                );
                out = Some(PathBuf::from(value));
            }
            "--work-dir" => {
                ensure!(
                    work_dir.is_none() && !value.is_empty(),
                    "duplicate or empty --work-dir"
                );
                work_dir = Some(PathBuf::from(value));
            }
            "--proxy-count" => {
                proxy_count = value.parse().context("invalid --proxy-count")?;
                ensure!(
                    (1..=50_000).contains(&proxy_count),
                    "--proxy-count must be in 1..=50000"
                );
            }
            "--proxy-concurrency" => {
                proxy_concurrency = value.parse().context("invalid --proxy-concurrency")?;
                ensure!(
                    (1..=64).contains(&proxy_concurrency),
                    "--proxy-concurrency must be in 1..=64"
                );
            }
            "--query-iters" => {
                query_iters = value.parse().context("invalid --query-iters")?;
                ensure!(
                    (1..=200).contains(&query_iters),
                    "--query-iters must be in 1..=200"
                );
            }
            "--intruder-count" => {
                intruder_count = value.parse().context("invalid --intruder-count")?;
                ensure!(
                    (1..=50_000).contains(&intruder_count),
                    "--intruder-count must be in 1..=50000"
                );
            }
            "--intruder-concurrency" => {
                intruder_concurrency = value.parse().context("invalid --intruder-concurrency")?;
                ensure!(
                    (1..=256).contains(&intruder_concurrency),
                    "--intruder-concurrency must be in 1..=256"
                );
            }
            "--hold-total" => {
                hold_total = value.parse().context("invalid --hold-total")?;
                ensure!(
                    (2..=5_000).contains(&hold_total),
                    "--hold-total must be in 2..=5000"
                );
            }
            "--origin" => {
                ensure!(origin.is_none(), "duplicate --origin");
                let addr: SocketAddr = value.parse().context("invalid --origin address")?;
                ensure!(
                    addr.ip() == Ipv4Addr::LOCALHOST && addr.port() != 0,
                    "--origin must be an explicit 127.0.0.1 nonzero port"
                );
                origin = Some(addr);
            }
            other => bail!("unknown flag {other}; {USAGE}"),
        }
        idx += 2;
    }
    let out = out.with_context(|| format!("--out is required; {USAGE}"))?;
    Ok(BenchConfig {
        out,
        work_dir,
        proxy_count,
        proxy_concurrency,
        query_iters,
        intruder_count,
        intruder_concurrency,
        hold_total,
        origin,
    })
}

pub(crate) fn run_benchmark(cfg: &BenchConfig) -> Result<BenchReport> {
    let started = Instant::now();
    if let Some(parent) = cfg.out.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let owned_temp = if cfg.work_dir.is_none() {
        Some(tempfile::tempdir()?)
    } else {
        None
    };
    let work_dir = match &cfg.work_dir {
        Some(dir) => {
            fs::create_dir_all(dir)?;
            dir.canonicalize()?
        }
        None => owned_temp
            .as_ref()
            .expect("tempdir")
            .path()
            .canonicalize()?,
    };
    let sampler = TreeSampler::start(&work_dir)?;
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()?;
    let (runtime, mut client) = Runtime::start()?;

    let outcome = executor.block_on(async {
        let local_origin = if cfg.origin.is_none() {
            Some(BenchOrigin::start().await?)
        } else {
            None
        };
        let origin_addr = cfg
            .origin
            .unwrap_or_else(|| local_origin.as_ref().expect("local origin").address);
        let run_res = tokio::time::timeout(WORK_BUDGET, async {
            wait_project_opened(&mut client, None).await?;
            let (proxy, queries, save_reopen) =
                run_proxy_and_queries(&mut client, cfg, &work_dir, origin_addr, &sampler).await?;
            let intruder =
                run_intruder_benchmarks(&mut client, cfg, &work_dir, origin_addr, &sampler).await?;
            Ok::<_, anyhow::Error>((origin_addr, proxy, queries, save_reopen, intruder))
        })
        .await
        .context("R5 benchmark exceeded work budget");
        let origin_stop = match local_origin {
            Some(origin) => origin.stop().await,
            None => Ok(()),
        };
        let inner = run_res??;
        origin_stop?;
        Ok::<_, anyhow::Error>(inner)
    });

    client.shutdown.cancel();
    drop(client);
    let shutdown_res = runtime.shutdown();
    let runtime_join_confirmed = shutdown_res.is_ok();
    let (peak_tree_bytes, peak_wal_bytes, peak_shm_bytes, peak_temp_bytes, samples_taken) =
        sampler.finish();
    shutdown_res?;
    let (origin_addr, proxy, http_history_queries, save_and_warm_reopen, intruder) = outcome?;

    let proxy_project_steady = measure_sqlite_files(&work_dir.join("proxy-bench.mio"))?;
    let save_as_project_steady = measure_sqlite_files(&work_dir.join("proxy-save-as.mio"))?;
    let intruder_project_steady = measure_sqlite_files(&work_dir.join("intruder-bench.mio"))?;
    let total_wall_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    ensure!(
        started.elapsed() <= TOTAL_BUDGET,
        "R5 benchmark exceeded 10-minute wall budget ({total_wall_ms} ms)"
    );

    let report = BenchReport {
        config: BenchConfigSummary {
            work_dir: work_dir.display().to_string(),
            origin: origin_addr.to_string(),
            proxy_count: cfg.proxy_count,
            proxy_concurrency: cfg.proxy_concurrency,
            query_iters: cfg.query_iters,
            intruder_count: cfg.intruder_count,
            intruder_concurrency: cfg.intruder_concurrency,
            hold_total: cfg.hold_total,
        },
        proxy,
        http_history_queries,
        save_and_warm_reopen: save_and_warm_reopen.clone(),
        intruder,
        disk: DiskReport {
            peak_tree_bytes,
            peak_wal_bytes,
            peak_shm_bytes,
            peak_temp_bytes,
            save_as_peak_tree_bytes: save_and_warm_reopen.save_as_peak_tree_bytes,
            samples_taken,
            proxy_project_steady,
            save_as_project_steady,
            intruder_project_steady,
        },
        process: ProcessReport {
            ru_maxrss_bytes: max_rss_bytes(),
        },
        runtime_join_confirmed,
        total_wall_ms,
    };
    let bytes = serde_json::to_vec_pretty(&report)?;
    fs::write(&cfg.out, bytes)?;
    Ok(report)
}

fn main() -> ExitCode {
    match parse_args(std::env::args_os().skip(1)).and_then(|cfg| run_benchmark(&cfg)) {
        Ok(report) => {
            println!(
                "R5_BENCH_OK total_wall_ms={} proxy_rps={:.1} intruder_rps={:.1} peak_tree_bytes={} join={}",
                report.total_wall_ms,
                report.proxy.throughput_rps,
                report.intruder.throughput.throughput_rps,
                report.disk.peak_tree_bytes,
                report.runtime_join_confirmed
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("r5_bench failed: {err:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_defaults_match_r5_contract_and_reject_invalid_options() {
        let cfg = parse_args([OsString::from("--out"), OsString::from("/tmp/r5.json")]).unwrap();
        assert_eq!(cfg.proxy_count, 5_000);
        assert_eq!(cfg.proxy_concurrency, 16);
        assert_eq!(cfg.query_iters, 25);
        assert_eq!(cfg.intruder_count, 1_000);
        assert_eq!(cfg.intruder_concurrency, 16);
        assert_eq!(cfg.hold_total, 200);
        assert_eq!(cfg.origin, None);

        let custom = parse_args([
            OsString::from("--out"),
            OsString::from("/tmp/r5-custom.json"),
            OsString::from("--proxy-count"),
            OsString::from("250"),
            OsString::from("--intruder-count"),
            OsString::from("120"),
            OsString::from("--origin"),
            OsString::from("127.0.0.1:43210"),
        ])
        .unwrap();
        assert_eq!(custom.proxy_count, 250);
        assert_eq!(custom.intruder_count, 120);
        assert_eq!(
            custom.origin,
            Some("127.0.0.1:43210".parse::<SocketAddr>().unwrap())
        );

        assert!(parse_args(std::iter::empty::<OsString>()).is_err());
        assert!(
            parse_args([
                OsString::from("--out"),
                OsString::from("/tmp/r5.json"),
                OsString::from("--intruder-concurrency"),
                OsString::from("257"),
            ])
            .is_err()
        );
        assert!(
            parse_args([
                OsString::from("--out"),
                OsString::from("/tmp/r5.json"),
                OsString::from("--origin"),
                OsString::from("0.0.0.0:8080"),
            ])
            .is_err()
        );
    }

    #[test]
    fn micro_smoke_exercises_proxy_queries_intruder_and_json_report() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("report.json");
        let work = dir.path().join("work");
        let cfg = BenchConfig {
            out: out.clone(),
            work_dir: Some(work),
            proxy_count: 5,
            proxy_concurrency: 2,
            query_iters: 1,
            intruder_count: 4,
            intruder_concurrency: 2,
            hold_total: 4,
            origin: None,
        };
        let report = run_benchmark(&cfg).unwrap();
        assert!(report.runtime_join_confirmed);
        assert_eq!(report.proxy.requests, 5);
        assert_eq!(report.proxy.origin_observed, 5);
        assert!(report.proxy.stop_reclaim.listener_closed);
        assert_eq!(report.http_history_queries.len(), 8);
        assert_eq!(report.save_and_warm_reopen.reopened_total_rows, 5);
        assert_eq!(report.intruder.throughput.requests, 4);
        assert_eq!(report.intruder.stop_reclaim.replayed_after_stop, 0);
        assert!(report.disk.peak_tree_bytes >= report.disk.proxy_project_steady.total_bytes);
        assert!(out.is_file());
    }
}
