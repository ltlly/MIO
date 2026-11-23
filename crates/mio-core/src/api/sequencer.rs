use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{
    Draft, ProjectKey, Uuid,
    sequencer::{SequencerAnalysis, SequencerTokenLocation},
};

pub const DEFAULT_SEQUENCER_CONCURRENCY: usize = 4;
pub const MAX_SEQUENCER_CONCURRENCY: usize = 64;
pub const DEFAULT_SEQUENCER_TARGET_SAMPLES: usize = 200;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SequencerCaptureConfig {
    pub draft: Draft,
    pub location: SequencerTokenLocation,
    pub target_samples: usize,
    pub concurrency: usize,
}

#[derive(Clone, Debug)]
pub enum SequencerCommand {
    StartLiveCapture {
        project: ProjectKey,
        run_id: Uuid,
        config: Box<SequencerCaptureConfig>,
        cancel: CancellationToken,
    },
    StopLiveCapture {
        project: ProjectKey,
        run_id: Uuid,
    },
    AnalyzeSamples {
        project: ProjectKey,
        operation_id: Uuid,
        samples: Vec<String>,
        cancel: CancellationToken,
    },
}

impl SequencerCommand {
    pub fn project(&self) -> ProjectKey {
        match self {
            Self::StartLiveCapture { project, .. }
            | Self::StopLiveCapture { project, .. }
            | Self::AnalyzeSamples { project, .. } => *project,
        }
    }
}

#[derive(Clone, Debug)]
pub enum SequencerEvent {
    CaptureStarted {
        project: ProjectKey,
        run_id: Uuid,
        target_samples: usize,
    },
    CaptureProgress {
        project: ProjectKey,
        run_id: Uuid,
        requests_sent: usize,
        tokens_collected: usize,
        extraction_misses: usize,
        request_errors: usize,
        latest_samples: Vec<String>,
    },
    CaptureFinished {
        project: ProjectKey,
        run_id: Uuid,
        requests_sent: usize,
        tokens_collected: usize,
        extraction_misses: usize,
        request_errors: usize,
        samples: Vec<String>,
        stopped_early: bool,
        error: Option<String>,
    },
    AnalysisReady {
        project: ProjectKey,
        operation_id: Uuid,
        result: Result<Box<SequencerAnalysis>, String>,
    },
}

impl SequencerEvent {
    pub fn project(&self) -> ProjectKey {
        match self {
            Self::CaptureStarted { project, .. }
            | Self::CaptureProgress { project, .. }
            | Self::CaptureFinished { project, .. }
            | Self::AnalysisReady { project, .. } => *project,
        }
    }
}
