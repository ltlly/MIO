use std::net::SocketAddr;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{
    Draft, ProjectKey, TaskId, Uuid,
    audit_engine::{AuditFinding, AuditPhase, EvidenceSpan, IssueConfidence},
    oast::{OastInteraction, OastPayloadToken, OastProtocol, SelfHostedOastConfig},
    target_issues::IssueSeverity,
};

pub use crate::audit_engine::{
    ACTIVE_CANARY, ACTIVE_REDIRECT_TARGET, ACTIVE_SPLIT_HEADER, InsertionLocation, InsertionPoint,
    inspect_flow_passive, render_audit_report_html, render_audit_report_json,
};
pub use crate::oast::LocalOastServer;

pub const MAX_AUDIT_TASKS: usize = 512;
pub const MAX_AUDIT_FINDINGS: usize = 4_096;
pub const MAX_AUDIT_STATE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_COLLABORATOR_INTERACTIONS: usize = 2_048;
pub const MAX_COLLABORATOR_BATCH_PAYLOADS: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditTaskMode {
    #[default]
    PassiveAndActive,
    PassiveOnly,
}

impl AuditTaskMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::PassiveAndActive => "主动 + 被动审计",
            Self::PassiveOnly => "仅被动审计",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditTaskState {
    #[default]
    Running,
    Paused,
    Completed,
    Cancelled,
    Failed,
}

impl AuditTaskState {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Running | Self::Paused)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "扫描中",
            Self::Paused => "已暂停",
            Self::Completed => "已完成",
            Self::Cancelled => "已取消",
            Self::Failed => "失败",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditTaskSummary {
    pub id: Uuid,
    pub title: String,
    pub target_url: String,
    pub mode: AuditTaskMode,
    pub state: AuditTaskState,
    pub requests_sent: u64,
    pub total_planned: u64,
    pub issues_found: u64,
    pub started_unix_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditSeverity {
    Information,
    Low,
    #[default]
    Medium,
    High,
}

impl From<IssueSeverity> for AuditSeverity {
    fn from(value: IssueSeverity) -> Self {
        match value {
            IssueSeverity::Information => Self::Information,
            IssueSeverity::Low => Self::Low,
            IssueSeverity::Medium => Self::Medium,
            IssueSeverity::High => Self::High,
        }
    }
}

impl From<AuditSeverity> for IssueSeverity {
    fn from(value: AuditSeverity) -> Self {
        match value {
            AuditSeverity::Information => Self::Information,
            AuditSeverity::Low => Self::Low,
            AuditSeverity::Medium => Self::Medium,
            AuditSeverity::High => Self::High,
        }
    }
}

pub type AuditConfidence = IssueConfidence;

const fn default_custom_scan_check_enabled() -> bool {
    true
}

const fn default_custom_scan_check_confidence() -> AuditConfidence {
    AuditConfidence::Firm
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomScanCheck {
    pub id: Uuid,
    #[serde(default = "default_custom_scan_check_enabled")]
    pub enabled: bool,
    pub name: String,
    pub severity: AuditSeverity,
    #[serde(default = "default_custom_scan_check_confidence")]
    pub confidence: AuditConfidence,
    pub script: String,
    pub description: String,
}

impl Default for CustomScanCheck {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            enabled: true,
            name: String::new(),
            severity: AuditSeverity::Medium,
            confidence: AuditConfidence::Firm,
            script: String::new(),
            description: String::new(),
        }
    }
}

const fn default_live_passive() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditStateSnapshot {
    pub revision: u64,
    #[serde(default = "default_live_passive")]
    pub live_passive_enabled: bool,
    #[serde(default = "default_live_passive")]
    pub live_passive_crawl_enabled: bool,
    #[serde(default)]
    pub live_passive_suite_scope_only: bool,
    #[serde(default)]
    pub tasks: Vec<AuditTaskSummary>,
    #[serde(default)]
    pub findings: Vec<AuditFinding>,
}

impl Default for AuditStateSnapshot {
    fn default() -> Self {
        Self {
            revision: 0,
            live_passive_enabled: true,
            live_passive_crawl_enabled: true,
            live_passive_suite_scope_only: false,
            tasks: Vec::new(),
            findings: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum AuditCommand {
    QueryState {
        project: ProjectKey,
        query_revision: u64,
    },
    SetLivePassive {
        project: ProjectKey,
        enabled: bool,
    },
    SetLivePassiveCrawl {
        project: ProjectKey,
        enabled: bool,
    },
    SetLivePassiveScope {
        project: ProjectKey,
        suite_scope_only: bool,
    },
    StartScan {
        project: ProjectKey,
        task_id: Uuid,
        title: String,
        draft: Box<Draft>,
        mode: AuditTaskMode,
        crawl_same_origin: bool,
        cancel: CancellationToken,
    },
    StopScan {
        project: ProjectKey,
        task_id: TaskId,
    },
    PauseScan {
        project: ProjectKey,
        task_id: TaskId,
    },
    ResumeScan {
        project: ProjectKey,
        task_id: TaskId,
    },
    DeleteTask {
        project: ProjectKey,
        task_id: TaskId,
    },
    SetFindingSeverity {
        project: ProjectKey,
        finding_id: Uuid,
        severity: IssueSeverity,
        confidence: IssueConfidence,
    },
    DeleteFinding {
        project: ProjectKey,
        finding_id: Uuid,
    },
    ClearFindings {
        project: ProjectKey,
    },
    RecordManualFinding {
        project: ProjectKey,
        finding: Box<AuditFinding>,
    },
    AttachFlowToFinding {
        project: ProjectKey,
        finding_id: Uuid,
        flow_id: Option<crate::FlowId>,
        url: String,
        evidence: String,
    },
}

impl AuditCommand {
    pub fn project(&self) -> ProjectKey {
        match self {
            Self::QueryState { project, .. }
            | Self::SetLivePassive { project, .. }
            | Self::SetLivePassiveCrawl { project, .. }
            | Self::SetLivePassiveScope { project, .. }
            | Self::StartScan { project, .. }
            | Self::StopScan { project, .. }
            | Self::PauseScan { project, .. }
            | Self::ResumeScan { project, .. }
            | Self::DeleteTask { project, .. }
            | Self::SetFindingSeverity { project, .. }
            | Self::DeleteFinding { project, .. }
            | Self::ClearFindings { project }
            | Self::RecordManualFinding { project, .. }
            | Self::AttachFlowToFinding { project, .. } => *project,
        }
    }
}

#[derive(Clone, Debug)]
pub enum AuditEvent {
    State {
        project: ProjectKey,
        query_revision: u64,
        result: Result<Box<AuditStateSnapshot>, String>,
    },
    TaskUpdated {
        project: ProjectKey,
        revision: u64,
        task: AuditTaskSummary,
        new_findings: Vec<AuditFinding>,
    },
    PassiveFindings {
        project: ProjectKey,
        revision: u64,
        findings: Vec<AuditFinding>,
    },
}

impl AuditEvent {
    pub fn project(&self) -> ProjectKey {
        match self {
            Self::State { project, .. }
            | Self::TaskUpdated { project, .. }
            | Self::PassiveFindings { project, .. } => *project,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollaboratorStatus {
    pub running: bool,
    pub http_addr: Option<SocketAddr>,
    pub dns_addr: Option<SocketAddr>,
    pub config: SelfHostedOastConfig,
    pub issued_tokens: Vec<OastPayloadToken>,
    pub interactions: Vec<OastInteraction>,
}

#[derive(Clone, Debug)]
pub enum CollaboratorCommand {
    QueryStatus {
        project: ProjectKey,
    },
    Start {
        project: ProjectKey,
        config: SelfHostedOastConfig,
    },
    Stop {
        project: ProjectKey,
    },
    GeneratePayloads {
        project: ProjectKey,
        count: usize,
        label: String,
    },
    PollNow {
        project: ProjectKey,
    },
    ClearInteractions {
        project: ProjectKey,
    },
    DeleteInteraction {
        project: ProjectKey,
        interaction_id: Uuid,
    },
}

impl CollaboratorCommand {
    pub fn project(&self) -> ProjectKey {
        match self {
            Self::QueryStatus { project }
            | Self::Start { project, .. }
            | Self::Stop { project }
            | Self::GeneratePayloads { project, .. }
            | Self::PollNow { project }
            | Self::ClearInteractions { project }
            | Self::DeleteInteraction { project, .. } => *project,
        }
    }
}

#[derive(Clone, Debug)]
pub enum CollaboratorEvent {
    Status {
        project: ProjectKey,
        status: Box<CollaboratorStatus>,
    },
    PayloadsGenerated {
        project: ProjectKey,
        payloads: Vec<OastPayloadToken>,
    },
    InteractionObserved {
        project: ProjectKey,
        interaction: Box<OastInteraction>,
    },
    Error {
        project: ProjectKey,
        message: String,
    },
}

impl CollaboratorEvent {
    pub fn project(&self) -> ProjectKey {
        match self {
            Self::Status { project, .. }
            | Self::PayloadsGenerated { project, .. }
            | Self::InteractionObserved { project, .. }
            | Self::Error { project, .. } => *project,
        }
    }
}

pub fn render_collaborator_interactions_json(
    interactions: &[OastInteraction],
) -> anyhow::Result<String> {
    serde_json::to_string_pretty(interactions)
        .map_err(|err| anyhow::anyhow!("序列化 Collaborator 交互记录失败：{err}"))
}

pub type ReexportedAuditTypes = (
    AuditFinding,
    AuditPhase,
    EvidenceSpan,
    IssueConfidence,
    OastInteraction,
    OastPayloadToken,
    OastProtocol,
    SelfHostedOastConfig,
);
