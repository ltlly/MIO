mod catalog;
mod query;

#[cfg(test)]
mod tests;

pub use query::{filter_issue_definitions, issue_definitions};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IssueSeverity {
    Information,
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IssueDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub typical_severity: IssueSeverity,
    pub cwe: &'static str,
    pub description: &'static str,
    pub remediation: &'static str,
    pub references: &'static [&'static str],
}
