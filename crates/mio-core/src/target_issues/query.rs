use super::catalog::ISSUE_DEFINITIONS;
use super::*;

impl IssueSeverity {
    pub const ALL: [Self; 4] = [Self::High, Self::Medium, Self::Low, Self::Information];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Information => "信息",
            Self::Low => "低危",
            Self::Medium => "中危",
            Self::High => "高危",
        }
    }

    pub const fn en_label(self) -> &'static str {
        match self {
            Self::Information => "Information",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }

    pub const fn bilingual_label(self) -> &'static str {
        match self {
            Self::Information => "信息 (Information)",
            Self::Low => "低危 (Low)",
            Self::Medium => "中危 (Medium)",
            Self::High => "高危 (High)",
        }
    }

    pub const fn rank(self) -> u8 {
        match self {
            Self::Information => 0,
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 3,
        }
    }

    pub const fn weight(self) -> u8 {
        self.rank()
    }
}

pub fn issue_definitions() -> &'static [IssueDefinition] {
    ISSUE_DEFINITIONS
}

pub fn filter_issue_definitions(
    query: &str,
    severity: Option<IssueSeverity>,
) -> Vec<&'static IssueDefinition> {
    let query = query.trim().to_lowercase();
    issue_definitions()
        .iter()
        .filter(|definition| {
            severity.is_none_or(|expected| definition.typical_severity == expected)
        })
        .filter(|definition| {
            query.is_empty()
                || [
                    definition.id,
                    definition.name,
                    definition.cwe,
                    definition.description,
                    definition.remediation,
                ]
                .into_iter()
                .chain(definition.references.iter().copied())
                .any(|field| field.to_lowercase().contains(&query))
        })
        .collect()
}
