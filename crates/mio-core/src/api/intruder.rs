use serde::{Deserialize, Serialize};

pub use crate::intruder::evaluate_grep_config;
use crate::{
    Draft, FlowSummary, IntruderPoolSelection, Orientation, RepeaterRedirectMode, TabGroup, TabId,
    Uuid, Workspace,
    intruder::{AttackType, IntruderPayloadType, PayloadProcessingRule, Position},
};

mod spec;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntruderGrepExtractRule {
    pub start_delimiter: String,
    pub end_delimiter: String,
}

impl IntruderGrepExtractRule {
    pub fn delimited(start: impl Into<String>, end: impl Into<String>) -> Self {
        Self {
            start_delimiter: start.into(),
            end_delimiter: end.into(),
        }
    }

    pub fn regex(pattern: impl Into<String>) -> Self {
        let pat = pattern.into();
        let prefixed = if pat.starts_with("re:") {
            pat
        } else {
            format!("re:{pat}")
        };
        Self {
            start_delimiter: prefixed,
            end_delimiter: "$1".into(),
        }
    }

    pub fn regex_pattern(&self) -> Option<&str> {
        self.start_delimiter
            .strip_prefix("re:")
            .filter(|pat| !pat.is_empty())
    }

    pub fn key_label(&self) -> String {
        if let Some(pat) = self.regex_pattern() {
            format!("re:{pat}")
        } else {
            format!("{}..{}", self.start_delimiter, self.end_delimiter)
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntruderGrepConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub match_expressions: Vec<String>,
    #[serde(default)]
    pub match_case_sensitive: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extract_rules: Vec<IntruderGrepExtractRule>,
    #[serde(default)]
    pub grep_payloads: bool,
    #[serde(default)]
    pub grep_payloads_case_sensitive: bool,
}

impl IntruderGrepConfig {
    pub fn is_empty(&self) -> bool {
        self.match_expressions.is_empty()
            && !self.match_case_sensitive
            && self.extract_rules.is_empty()
            && !self.grep_payloads
            && !self.grep_payloads_case_sensitive
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntruderAttackSettings {
    #[serde(default = "default_update_content_length")]
    pub update_content_length: bool,
    #[serde(default)]
    pub set_connection_close: bool,
    #[serde(default)]
    pub follow_redirects: RepeaterRedirectMode,
    #[serde(default)]
    pub process_cookies_in_redirections: bool,
    #[serde(default)]
    pub network_failure_retries: u8,
    #[serde(default = "default_retry_pause_ms")]
    pub retry_pause_ms: u64,
}

const fn default_update_content_length() -> bool {
    true
}

const fn default_retry_pause_ms() -> u64 {
    2000
}

impl Default for IntruderAttackSettings {
    fn default() -> Self {
        Self {
            update_content_length: true,
            set_connection_close: false,
            follow_redirects: RepeaterRedirectMode::Never,
            process_cookies_in_redirections: false,
            network_failure_retries: 0,
            retry_pause_ms: 2000,
        }
    }
}

impl IntruderAttackSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntruderPayloadProcessingEntry {
    #[serde(default = "default_update_content_length")]
    pub enabled: bool,
    pub rule: PayloadProcessingRule,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntruderPayloadSetConfig {
    #[serde(default)]
    pub payload_type: IntruderPayloadType,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub processing_rules: Vec<IntruderPayloadProcessingEntry>,
}

impl IntruderPayloadSetConfig {
    pub fn is_default(&self) -> bool {
        self.payload_type == IntruderPayloadType::SimpleList && self.processing_rules.is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntruderWorkspace {
    pub orientation: Orientation,
    #[serde(default)]
    pub tab_sidebar_width: u16,
    pub groups: Vec<TabGroup>,
    pub tabs: Vec<IntruderTab>,
    pub active: Option<TabId>,
    pub closed: Vec<IntruderTab>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub grep_configs: std::collections::BTreeMap<TabId, IntruderGrepConfig>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub attack_settings: std::collections::BTreeMap<TabId, IntruderAttackSettings>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub payload_configs: std::collections::BTreeMap<TabId, Vec<IntruderPayloadSetConfig>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntruderTab {
    pub id: TabId,
    pub title: String,
    pub group: Option<Uuid>,
    pub spec: IntruderSpec,
    pub last_run: Option<Uuid>,
}

/// Editable configuration, frozen when admitted. Settings are not External defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntruderSpec {
    pub revision: u64,
    pub draft: Draft,
    pub attack_type: AttackType,
    pub positions: Vec<Position>,
    pub payload_sets: Vec<Vec<Vec<u8>>>,
    pub max_concurrent_requests: u16,
    pub request_timeout_ms: u64,
    pub update_host_header: bool,
    /// Frozen start policy. Later retention changes do not rewrite this configuration.
    #[serde(default = "legacy_intruder_save_to_project")]
    pub save_to_project: bool,
    /// Adds the original template once when starting. Older configurations did not send it.
    #[serde(default)]
    pub baseline_request: bool,
    /// Missing selection preserves the original independent per-run concurrency policy.
    #[serde(default)]
    pub resource_pool: Option<IntruderPoolSelection>,
}

// Older MIO configurations always started Project-backed; retain that meaning when loading them.
pub(crate) fn legacy_intruder_save_to_project() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntruderRunState {
    Running,
    /// New dispatch is suspended; requests already in flight may still finish.
    Paused,
    Stopping,
    Stopped,
    Completed,
    Interrupted,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntruderRunSummary {
    pub id: Uuid,
    pub tab_id: TabId,
    pub revision: u64,
    pub spec_revision: u64,
    pub total: u64,
    pub dispatched: u64,
    pub completed: u64,
    pub state: IntruderRunState,
    pub started_unix_ms: i64,
    /// Derived from backing ownership, independently of pending write recovery.
    #[serde(default)]
    pub retention: IntruderRetention,
    /// Empty selects the generated task title; independent of the configuration tab.
    #[serde(default)]
    pub title: String,
    /// Renames do not advance the lifecycle revision or a Session retirement ticket.
    #[serde(default)]
    pub title_revision: u64,
    /// The single baseline slot, including an explicit request added after the attack started.
    #[serde(default)]
    pub baseline: Option<IntruderBaseline>,
    /// Current binding, independently of the frozen Start selection and lifecycle revision.
    #[serde(default)]
    pub resource_pool: Option<Uuid>,
    #[serde(default)]
    pub resource_pool_revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntruderBaseline {
    pub ordinal: u64,
    /// Terminal includes a failed or cancelled attempt, which is never automatically replayed.
    pub completed: bool,
}

#[derive(Clone, Debug)]
pub struct IntruderBaselineReceipt {
    pub run: IntruderRunSummary,
    /// The run's cancellation token; admission cancellation cannot stop an accepted request.
    pub cancel: tokio_util::sync::CancellationToken,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntruderRetention {
    #[default]
    Project,
    /// Runtime-only RAM data. Ordinary Save/SaveAs and shutdown never promote it.
    Session,
}

/// A committed retention change; saving preserves an active run and its in-flight requests.
/// Moving to Session still requires a terminal run and does not erase earlier disk copies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntruderRetentionReceipt {
    /// Only the run revision advances; frozen inputs, flow revisions and IDs are preserved.
    pub run: IntruderRunSummary,
    pub retention: IntruderRetention,
    /// Moving to Session clears this run's persisted last_run links, not other edits.
    pub workspace: Workspace,
    /// Encoded SQL data and ownership accounting, not a process RSS measurement.
    pub session_logical_bytes: u64,
}

/// A committed logical deletion, not secure erasure or disk compaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntruderDiscardReceipt {
    /// The final run revision plus one, reserved by its retirement record.
    pub revision: u64,
    /// The proposal with only this run's last_run references cleared.
    pub workspace: Workspace,
    pub deleted_flows: u64,
    pub retained_flows: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntruderResultOutcome {
    Flow(FlowSummary),
    Rejected(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntruderResultSummary {
    pub ordinal: u64,
    pub outcome: IntruderResultOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntruderResultPage {
    pub run: IntruderRunSummary,
    pub rows: Vec<IntruderResultSummary>,
    pub next_after: Option<u64>,
    /// False for Session storage or pending Project recovery; inspect run.retention.
    /// Save repairs Project recovery only; Session promotion must be explicit.
    pub persisted: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IntruderResultSortColumn {
    #[default]
    Request,
    Payload,
    Status,
    ResponseReceived,
    Length,
    GrepMatch(usize),
    GrepExtract(usize),
    GrepPayload(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntruderResultSort {
    pub column: IntruderResultSortColumn,
    pub ascending: bool,
}

impl Default for IntruderResultSort {
    fn default() -> Self {
        Self {
            column: IntruderResultSortColumn::Request,
            ascending: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntruderResultFilter {
    pub status_classes: [bool; 4],
    pub search_term: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub negative_search: bool,
    pub only_with_notes: bool,
    pub only_highlighted: bool,
    pub only_grep_matched: bool,
}

impl Default for IntruderResultFilter {
    fn default() -> Self {
        Self {
            status_classes: [true; 4],
            search_term: String::new(),
            regex: false,
            case_sensitive: false,
            negative_search: false,
            only_with_notes: false,
            only_highlighted: false,
            only_grep_matched: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntruderResultViewQuery {
    pub project: super::ProjectKey,
    pub run_id: Uuid,
    pub view_id: Uuid,
    pub query_revision: u64,
    pub offset: usize,
    pub limit: usize,
    pub filter: IntruderResultFilter,
    pub sort: IntruderResultSort,
    pub selected_ordinal: Option<u64>,
    pub grep_config: IntruderGrepConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntruderPayloadPreview {
    pub set_index: usize,
    pub payload_index: usize,
    pub raw_bytes: u64,
    pub prefix: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntruderResultViewRow {
    pub ordinal: u64,
    /// None only for an admitted baseline that has not been dispatched yet.
    pub result: Option<IntruderResultSummary>,
    pub payloads: Vec<IntruderPayloadPreview>,
    pub response_length: Option<u64>,
    pub notes_revision: u64,
    pub notes_preview: String,
    pub highlight: Option<u8>,
    pub highlight_revision: u64,
    pub grep_matches: Vec<String>,
    pub grep_extracts: Vec<(String, String)>,
    pub grep_payload_hits: Vec<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntruderResultViewPage {
    pub run: IntruderRunSummary,
    pub offset: usize,
    pub total: usize,
    pub rows: Vec<IntruderResultViewRow>,
    pub next_offset: Option<usize>,
    /// Rank in all matching results, including when the selected row is on this page.
    pub selected_rank: Option<usize>,
    /// Present only when the selection still matches and is outside `rows`.
    pub selected_row: Option<IntruderResultViewRow>,
    pub persisted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntruderRunPage {
    pub rows: Vec<IntruderRunSummary>,
    pub next_after: Option<Uuid>,
}
