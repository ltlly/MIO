use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    future::Future,
    task::Poll,
};

use tokio::{
    task::{Id, JoinError},
    time::Instant as PoolInstant,
};

use super::*;
use crate::{intruder::AttackPlan, store::IntruderMutation};

mod admission;
mod auto_throttle;
mod control;
mod dispatch;
mod persistence;
mod pool_timing;
mod pools;
mod pre_send;
mod scheduler;
use pool_timing::PoolTiming;

// MIO admission limits, not External defaults. Terminal datasets belong to storage, not ActiveRun.
const MAX_RUNS: usize = 8;
const MAX_REQUESTS_IN_FLIGHT: usize = crate::MAX_INTRUDER_CONCURRENT_REQUESTS as usize;
const RESULT_PAGE_BYTES: usize = 1024 * 1024;
// Pending notifications suspend admission: one reply, one History change, and at most
// one error per in-flight completion plus two lifecycle errors per admitted run.
const MAX_NOTIFICATIONS: usize = 2 + MAX_REQUESTS_IN_FLIGHT + 2 * MAX_RUNS;

struct ActiveRun {
    summary: IntruderRunSummary,
    plan: AttackPlan,
    target: String,
    version: HttpVersion,
    max_concurrent_requests: u16,
    timeout: Duration,
    update_host_header: bool,
    attack_settings: IntruderAttackSettings,
    cancel: CancellationToken,
    active: usize,
    preparing: bool,
    waiting_for_capacity: bool,
    failure: Option<String>,
}

struct ActiveRequest {
    run_id: Uuid,
    ordinal: u64,
    flow_id: FlowId,
    // A later run rebind must not release or relabel this request's original pool slot.
    resource_pool: Option<Uuid>,
}

pub(super) struct Completion {
    project: ProjectKey,
    record: FlowRecord,
    reason_phrase: Option<Vec<u8>>,
}

pub(super) struct PrepCompletion {
    project: ProjectKey,
    run_id: Uuid,
    ordinal: u64,
    record: FlowRecord,
    config: SendConfig,
    response_codes: Vec<u16>,
}

pub(super) enum Signal {
    Completed(Box<std::result::Result<(Id, Completion), JoinError>>),
    Prepared(Box<std::result::Result<(Id, PrepCompletion), JoinError>>),
    Advance(Uuid),
    Notify(mpsc::OwnedPermit<Event>),
    Notification(mpsc::OwnedPermit<Event>),
}

#[derive(Default)]
pub(super) struct IntruderState {
    pub(super) resource_pools: IntruderResourcePools,
    pool_timings: BTreeMap<Uuid, PoolTiming>,
    runs: BTreeMap<Uuid, ActiveRun>,
    jobs: JoinSet<Completion>,
    prep_jobs: JoinSet<PrepCompletion>,
    requests: HashMap<Id, ActiveRequest>,
    prep_requests: HashMap<Id, (Uuid, u64)>,
    pub(super) recovery_reason_phrases: BTreeMap<(Uuid, u64), Vec<u8>>,
    last_dispatched: Option<Uuid>,
    dirty: bool,
    notified: bool,
    notifications: VecDeque<Event>,
    pub(super) recovery: Vec<IntruderMutation>,
}

impl IntruderState {
    // State is deliberately declared before implementation modules so private child modules
    // and tests can access these core types without public ABI expansion.
    pub(super) fn active_run_ids(&self, _project: ProjectKey) -> Vec<Uuid> {
        self.runs.keys().copied().collect()
    }
}

impl State {
    // Methods are split across admission, control, pools, dispatch, and persistence modules.
}

fn mutation_run(mutation: &IntruderMutation) -> &IntruderRunSummary {
    match mutation {
        IntruderMutation::Result { run, .. } | IntruderMutation::State { run } => run,
    }
}

fn next_revision(revision: u64) -> u64 {
    revision + 1
}

#[cfg(test)]
mod tests;
