use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::Duration,
};

use anyhow::{Result, bail, ensure};
use mio_plugin_api::{Header, HttpRequest, HttpVersion};
use mio_proxy::{SendConfig, send};
use tokio::{
    sync::mpsc,
    task::{JoinError, JoinHandle, JoinSet},
    time::{Instant as TokioInstant, timeout_at},
};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::State;
use crate::{
    CollaboratorCommand, CollaboratorEvent, CollaboratorStatus, Event,
    MAX_COLLABORATOR_BATCH_PAYLOADS, MAX_COLLABORATOR_INTERACTIONS, ProjectKey, Uuid,
    oast::{LocalOastServer, OastInteraction, OastPayloadToken, SelfHostedOastConfig},
};

const INTERACTION_CHANNEL_CAPACITY: usize = 256;
const MAX_INTERACTIONS_RAW_BYTES: usize = 16 * 1024 * 1024;
const MAX_POLL_BODY_BYTES: usize = 4 * 1024 * 1024;
const POLL_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

struct PollJobOutput {
    job_id: Uuid,
    project: ProjectKey,
    result: std::result::Result<Vec<OastInteraction>, String>,
}

#[derive(Default)]
pub(super) struct CollaboratorService {
    project: Option<ProjectKey>,
    server: Option<LocalOastServer>,
    rx: Option<mpsc::Receiver<OastInteraction>>,
    config: SelfHostedOastConfig,
    interactions: VecDeque<OastInteraction>,
    interaction_ids: HashSet<Uuid>,
    total_raw_bytes: usize,
    pending_events: VecDeque<CollaboratorEvent>,
    poll_jobs: JoinSet<PollJobOutput>,
    active_polls: HashMap<Uuid, (ProjectKey, CancellationToken)>,
    auto_poll: Option<(CancellationToken, JoinHandle<()>)>,
}

mod listener;
mod poll;
mod state;
#[cfg(test)]
mod tests;

use poll::build_poll_request;
