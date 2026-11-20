use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Result, bail, ensure};
use mio_plugin_api::{HttpRequest, HttpResponse, HttpVersion};
use mio_proxy::{ProxyError, SendConfig, prepare_send, send};
use tokio::{
    sync::mpsc,
    task::{AbortHandle, Id, JoinError, JoinSet},
    time::{Instant as TokioInstant, timeout_at},
};
use tokio_util::sync::CancellationToken;

use super::State;
use crate::{
    Event, MAX_SEQUENCER_CONCURRENCY, ProjectKey, SequencerCaptureConfig, SequencerCommand,
    SequencerEvent, Uuid, http,
    sequencer::{
        MAX_SAMPLE_COUNT, MAX_TOTAL_SAMPLE_BYTES, MIN_SAMPLE_COUNT, SequencerTokenLocation,
        analyze_tokens, extract_token,
    },
};

const PROGRESS_BATCH_REQUESTS: usize = 10;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);
const PROGRESS_PREVIEW: usize = 32;
const PROGRESS_QUEUE: usize = 32;
const STOP_DEADLINE: Duration = Duration::from_secs(3);
const JOB_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

struct CapturePlan {
    project: ProjectKey,
    run_id: Uuid,
    request: HttpRequest,
    config: SendConfig,
    location: SequencerTokenLocation,
    target_samples: usize,
    concurrency: usize,
    unpack_compressed_responses: bool,
    sessions: crate::sessions::SessionRuntime,
}

struct Attempt {
    result: std::result::Result<HttpResponse, ProxyError>,
}

struct ActiveCapture {
    job_id: Uuid,
    project: ProjectKey,
    run_id: Uuid,
    task_id: Id,
    cancel: CancellationToken,
    abort: AbortHandle,
}

struct AnalysisJob {
    project: ProjectKey,
    operation_id: Uuid,
    task_id: Id,
    cancel: CancellationToken,
    abort: AbortHandle,
}

enum JobOutput {
    Capture { job_id: Uuid, event: SequencerEvent },
    Analysis { job_id: Uuid, event: SequencerEvent },
}

pub(super) struct SequencerService {
    active_capture: Option<ActiveCapture>,
    analyses: HashMap<Uuid, AnalysisJob>,
    jobs: JoinSet<JobOutput>,
    progress_tx: mpsc::Sender<SequencerEvent>,
    progress_rx: mpsc::Receiver<SequencerEvent>,
    pending_events: VecDeque<SequencerEvent>,
}

impl Default for SequencerService {
    fn default() -> Self {
        let (progress_tx, progress_rx) = mpsc::channel(PROGRESS_QUEUE);
        Self {
            active_capture: None,
            analyses: HashMap::new(),
            jobs: JoinSet::new(),
            progress_tx,
            progress_rx,
            pending_events: VecDeque::new(),
        }
    }
}

mod capture;
mod command;
mod state;
#[cfg(test)]
mod tests;

use capture::{fallback_capture_finished, run_capture};
