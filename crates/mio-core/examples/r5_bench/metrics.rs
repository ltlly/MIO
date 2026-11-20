use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle as ThreadJoinHandle},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LatencySummary {
    pub min: u64,
    pub p50: u64,
    pub p95: u64,
    pub p99: u64,
    pub max: u64,
    pub mean: u64,
}

impl LatencySummary {
    pub(crate) fn from_samples(samples: &mut [u64]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        samples.sort_unstable();
        let sum: u128 = samples.iter().map(|&v| u128::from(v)).sum();
        let mean = (sum / (samples.len() as u128)) as u64;
        Self {
            min: samples[0],
            p50: percentile(samples, 50, 100),
            p95: percentile(samples, 95, 100),
            p99: percentile(samples, 99, 100),
            max: *samples.last().expect("non-empty"),
            mean,
        }
    }
}

fn percentile(sorted: &[u64], numerator: usize, denominator: usize) -> u64 {
    let rank = (sorted.len() * numerator)
        .div_ceil(denominator)
        .saturating_sub(1);
    sorted[rank.min(sorted.len() - 1)]
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProjectFilesBytes {
    pub file_bytes: u64,
    pub wal_bytes: u64,
    pub shm_bytes: u64,
    pub journal_bytes: u64,
    pub total_bytes: u64,
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut candidate = path.as_os_str().to_os_string();
    candidate.push(suffix);
    PathBuf::from(candidate)
}

pub(crate) fn ensure_fresh_mio_path(path: &Path) -> Result<()> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        match fs::symlink_metadata(sidecar_path(path, suffix)) {
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Ok(_) => bail!("target already exists: {}", path.display()),
            Err(err) => return Err(err).context("cannot inspect benchmark target path"),
        }
    }
    Ok(())
}

pub(crate) fn measure_sqlite_files(path: &Path) -> Result<ProjectFilesBytes> {
    let mut sizes = [0_u64; 4];
    for (idx, suffix) in ["", "-wal", "-shm", "-journal"].iter().enumerate() {
        match fs::symlink_metadata(sidecar_path(path, suffix)) {
            Ok(meta) => {
                ensure!(meta.is_file(), "SQLite output replaced by non-regular file");
                sizes[idx] = meta.len();
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err).context("cannot stat SQLite output"),
        }
    }
    Ok(ProjectFilesBytes {
        file_bytes: sizes[0],
        wal_bytes: sizes[1],
        shm_bytes: sizes[2],
        journal_bytes: sizes[3],
        total_bytes: sizes.iter().sum(),
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct DiskReport {
    pub peak_tree_bytes: u64,
    pub peak_wal_bytes: u64,
    pub peak_shm_bytes: u64,
    pub peak_temp_bytes: u64,
    pub save_as_peak_tree_bytes: u64,
    pub samples_taken: u64,
    pub proxy_project_steady: ProjectFilesBytes,
    pub save_as_project_steady: ProjectFilesBytes,
    pub intruder_project_steady: ProjectFilesBytes,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ProxyStopReport {
    pub held_connections: usize,
    pub stop_reclaim_ms: f64,
    pub listener_closed: bool,
    pub origin_active_after_stop: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ProxyReport {
    pub requests: usize,
    pub origin_observed: usize,
    pub concurrency: usize,
    pub wall_ms: f64,
    pub throughput_rps: f64,
    pub latency_us: LatencySummary,
    pub stop_reclaim: ProxyStopReport,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct QueryCaseReport {
    pub name: String,
    pub total_matched: usize,
    pub iterations: usize,
    pub latency_us: LatencySummary,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SaveAndReopenReport {
    pub save_ms: f64,
    pub save_as_ms: f64,
    pub save_as_peak_tree_bytes: u64,
    pub open_ack_ms: f64,
    pub first_page_ready_ms: f64,
    pub warm_reopen_total_ms: f64,
    pub reopened_total_rows: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct IntruderThroughputReport {
    pub requests: usize,
    pub origin_observed: usize,
    pub concurrency: u16,
    pub wall_ms: f64,
    pub throughput_rps: f64,
    pub flow_elapsed_ms: LatencySummary,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct IntruderStopReport {
    pub total_planned: usize,
    pub held_before_stop: usize,
    pub dispatched_at_stop: u64,
    pub completed_at_stop: u64,
    pub stop_reclaim_ms: f64,
    pub origin_active_after_stop: usize,
    pub replayed_after_stop: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct IntruderReport {
    pub throughput: IntruderThroughputReport,
    pub stop_reclaim: IntruderStopReport,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct BenchConfigSummary {
    pub work_dir: String,
    pub origin: String,
    pub proxy_count: usize,
    pub proxy_concurrency: usize,
    pub query_iters: usize,
    pub intruder_count: usize,
    pub intruder_concurrency: u16,
    pub hold_total: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ProcessReport {
    pub ru_maxrss_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct BenchReport {
    pub config: BenchConfigSummary,
    pub proxy: ProxyReport,
    pub http_history_queries: Vec<QueryCaseReport>,
    pub save_and_warm_reopen: SaveAndReopenReport,
    pub intruder: IntruderReport,
    pub disk: DiskReport,
    pub process: ProcessReport,
    pub runtime_join_confirmed: bool,
    pub total_wall_ms: u64,
}

pub(crate) fn max_rss_bytes() -> u64 {
    #[cfg(unix)]
    {
        let mut usage = unsafe { std::mem::zeroed::<libc::rusage>() };
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } == 0 && usage.ru_maxrss > 0 {
            #[cfg(target_os = "macos")]
            {
                usage.ru_maxrss as u64
            }
            #[cfg(not(target_os = "macos"))]
            {
                (usage.ru_maxrss as u64).saturating_mul(1024)
            }
        } else {
            0
        }
    }
    #[cfg(not(unix))]
    {
        mio_core::current_process_memory_bytes()
    }
}

#[derive(Default)]
struct SamplerShared {
    stop: AtomicBool,
    peak_tree: AtomicU64,
    peak_wal: AtomicU64,
    peak_shm: AtomicU64,
    peak_temp: AtomicU64,
    samples: AtomicU64,
}

pub(crate) struct TreeSampler {
    root: PathBuf,
    shared: Arc<SamplerShared>,
    worker: Option<ThreadJoinHandle<()>>,
}

impl TreeSampler {
    pub(crate) fn start(root: &Path) -> Result<Self> {
        let root = root.to_path_buf();
        let shared = Arc::new(SamplerShared::default());
        sample_once(&root, &shared);
        let thread_root = root.clone();
        let thread_shared = shared.clone();
        let worker = thread::Builder::new()
            .name("r5-tree-sampler".into())
            .spawn(move || {
                while !thread_shared.stop.load(Ordering::Relaxed) {
                    sample_once(&thread_root, &thread_shared);
                    thread::sleep(Duration::from_millis(5));
                }
                sample_once(&thread_root, &thread_shared);
            })?;
        Ok(Self {
            root,
            shared,
            worker: Some(worker),
        })
    }

    pub(crate) fn snapshot_now(&self) -> u64 {
        sample_once(&self.root, &self.shared);
        self.shared.peak_tree.load(Ordering::SeqCst)
    }

    pub(crate) fn finish(mut self) -> (u64, u64, u64, u64, u64) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        (
            self.shared.peak_tree.load(Ordering::SeqCst),
            self.shared.peak_wal.load(Ordering::SeqCst),
            self.shared.peak_shm.load(Ordering::SeqCst),
            self.shared.peak_temp.load(Ordering::SeqCst),
            self.shared.samples.load(Ordering::SeqCst),
        )
    }
}

fn sample_once(root: &Path, shared: &SamplerShared) {
    let (mut total, mut wal, mut shm, mut temp) = (0_u64, 0_u64, 0_u64, 0_u64);
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() {
                let len = meta.len();
                total = total.saturating_add(len);
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.ends_with("-wal") {
                    wal = wal.saturating_add(len);
                } else if name.ends_with("-shm") {
                    shm = shm.saturating_add(len);
                } else if name.starts_with(".tmp") || name.ends_with("-journal") {
                    temp = temp.saturating_add(len);
                }
            }
        }
    }
    shared.peak_tree.fetch_max(total, Ordering::SeqCst);
    shared.peak_wal.fetch_max(wal, Ordering::SeqCst);
    shared.peak_shm.fetch_max(shm, Ordering::SeqCst);
    shared.peak_temp.fetch_max(temp, Ordering::SeqCst);
    shared.samples.fetch_add(1, Ordering::Relaxed);
}
