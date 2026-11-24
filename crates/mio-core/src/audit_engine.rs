mod checks;
pub mod discovery;
pub mod insertion_points;
mod linter;
mod report;

#[cfg(test)]
mod discovery_tests;
#[cfg(test)]
mod insertion_points_tests;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

use crate::{FlowId, Uuid};

pub use checks::{
    build_authz_strip_probe, build_host_override_probe, build_probe_request,
    build_sql_boolean_pair, evaluate_active_check, evaluate_authz_strip_probe,
    evaluate_custom_scan_checks, evaluate_host_override_probe, evaluate_sql_boolean_pair,
    extract_insertion_points, inspect_passive,
};
pub use discovery::{
    CrawlConfig, CrawlReport, DiscoveredEndpoint, DiscoverySource, MAX_CRAWL_DEPTH,
    MAX_CRAWL_REQUESTS, MAX_DISCOVERED_PER_PAGE, crawl_in_scope_endpoints,
    crawl_in_scope_endpoints_with_headers, discover_same_origin_links_and_forms,
};
pub use insertion_points::{
    MAX_STRUCTURED_PARAMETERS, extract_structured_parameters, rebuild_structured_parameter,
};
pub use linter::inspect_flow_passive;
pub use report::{
    AuditSeverityCounts, escape_html, render_audit_report_html, render_audit_report_json,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueConfidence {
    Certain,
    Firm,
    Tentative,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditPhase {
    Passive,
    Active,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceSpan {
    pub offset: usize,
    pub length: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditFinding {
    pub id: Uuid,
    pub issue_id: String,
    pub title: String,
    pub severity_label: String,
    pub severity_rank: u8,
    pub confidence: IssueConfidence,
    pub phase: AuditPhase,
    pub host: String,
    pub url: String,
    pub parameter: Option<String>,
    pub flow_id: Option<FlowId>,
    pub summary: String,
    pub evidence: String,
    pub request_highlights: Vec<EvidenceSpan>,
    pub response_highlights: Vec<EvidenceSpan>,
    pub remediation: String,
    pub detected_unix_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsertionLocation {
    Query,
    Form,
    Json,
    Xml,
    Multipart,
    Cookie,
    Header,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsertionPoint {
    pub location: InsertionLocation,
    pub name: String,
    pub value: String,
    pub occurrence: usize,
}

pub const ACTIVE_CANARY: &str = "mio7f9c2a";
pub const ACTIVE_REDIRECT_TARGET: &str = "https://mio-audit.invalid/redirect";
pub const ACTIVE_SPLIT_HEADER: &str = "x-mio-split";
pub const ACTIVE_OVERRIDE_HOST: &str = "mio-override.invalid";
