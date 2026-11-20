use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::Uuid;

mod models;

pub const MAX_INTRUDER_RESOURCE_POOLS: usize = 64;
pub const MAX_INTRUDER_RESOURCE_POOLS_BYTES: usize = 512 * 1024;

/// Counts retained tasks, including finished tasks, by their current pool binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntruderPoolUsage {
    pub catalog_revision: u64,
    pub task_counts: BTreeMap<Uuid, u64>,
    /// Current tasks, configuration tabs, or in-flight requests need explicit reassignment.
    pub requires_confirmation: BTreeSet<Uuid>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntruderPoolDeleteReceipt {
    pub catalog: IntruderResourcePools,
    pub workspace: crate::Workspace,
    pub reassigned_runs: u64,
    pub reassigned_tabs: u64,
}

/// Session databases commit separately. Failure never conceals completed reassignments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntruderPoolDeleteError {
    pub message: String,
    pub reassigned_sessions: u64,
    pub default_pool_created: bool,
    pub catalog: Option<IntruderResourcePools>,
    pub workspace: Option<Box<crate::Workspace>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntruderResourcePool {
    pub id: Uuid,
    pub name: String,
    /// None adds no pool limit; the runtime's global resource limit still applies.
    pub max_concurrent_requests: Option<u16>,
    #[serde(default)]
    pub delay: Option<IntruderPoolDelay>,
    #[serde(default)]
    pub automatic_throttling: IntruderAutoThrottle,
    /// Retains disabled controls without changing the effective cap or delay policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_inputs: Option<IntruderPoolInputs>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntruderPoolInputs {
    pub cap: String,
    pub delay: String,
    pub increment: String,
    pub delay_mode: IntruderPoolDelayMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntruderPoolDelayMode {
    Fixed,
    Random,
    Increment,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntruderAutoThrottle {
    pub enabled: bool,
    pub status_429: bool,
    pub status_503: bool,
    pub other_enabled: bool,
    pub other_codes: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", deny_unknown_fields)]
pub enum IntruderPoolDelay {
    Fixed { milliseconds: u64 },
    Random { milliseconds: u64 },
    Increment { initial_ms: u64, increment_ms: u64 },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntruderResourcePools {
    pub revision: u64,
    pub pools: Vec<IntruderResourcePool>,
    #[serde(default)]
    pub default_pool_id: Option<Uuid>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntruderPoolSelection {
    Existing(Uuid),
    /// A saved form draft. The pool is only created when Start succeeds.
    Create(IntruderResourcePool),
}

#[cfg(test)]
mod tests;
