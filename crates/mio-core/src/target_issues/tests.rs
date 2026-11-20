use super::*;
use std::collections::{HashMap, HashSet};

#[test]
fn definitions_are_unique_and_complete() {
    let definitions = issue_definitions();
    assert!(definitions.len() >= 37);
    let mut ids = HashSet::new();
    let mut names = HashMap::new();
    for definition in definitions {
        assert!(!definition.id.is_empty(), "空 ID: {definition:?}");
        assert!(!definition.name.is_empty(), "空名称: {definition:?}");
        assert!(!definition.cwe.is_empty(), "空 CWE: {definition:?}");
        assert!(!definition.description.is_empty(), "空描述: {definition:?}");
        assert!(
            !definition.remediation.is_empty(),
            "空修复建议: {definition:?}"
        );
        assert!(!definition.references.is_empty(), "空参考: {definition:?}");
        assert!(ids.insert(definition.id), "重复 ID: {}", definition.id);
        assert!(
            names.insert(definition.name, ()).is_none(),
            "重复名称: {}",
            definition.name
        );
    }
}

#[test]
fn filter_matches_fields_case_insensitively() {
    assert_eq!(
        filter_issue_definitions("SQL injection", None)[0].id,
        "0x00100200"
    );
    assert_eq!(
        filter_issue_definitions("0x00200903", None)[0].cwe,
        "CWE-326"
    );
    assert_eq!(
        filter_issue_definitions("cwe-113", None)[0].id,
        "0x00200200"
    );
    assert_eq!(
        filter_issue_definitions("prepared statement", None)[0].id,
        "0x00100200"
    );
    assert_eq!(
        filter_issue_definitions("frame-ancestors", None)[0].id,
        "0x005009a0"
    );
    for expected_id in [
        "0x00500601",
        "0x00700100",
        "0x01000400",
        "0x005009b0",
        "0x005009c0",
    ] {
        assert_eq!(
            filter_issue_definitions(expected_id, None)[0].id,
            expected_id
        );
    }
}

#[test]
fn filter_requires_exact_severity() {
    let medium = filter_issue_definitions("", Some(IssueSeverity::Medium));
    assert!(!medium.is_empty());
    assert!(
        medium
            .iter()
            .all(|item| item.typical_severity == IssueSeverity::Medium)
    );
    let total_by_severity: usize = IssueSeverity::ALL
        .into_iter()
        .map(|sev| filter_issue_definitions("", Some(sev)).len())
        .sum();
    assert_eq!(total_by_severity, issue_definitions().len());
    assert!(filter_issue_definitions("SQL injection", Some(IssueSeverity::Low)).is_empty());
    assert_eq!(
        filter_issue_definitions("sql injection", Some(IssueSeverity::High)).len(),
        1
    );
}

#[test]
fn severity_orders_from_information_to_high() {
    assert!(IssueSeverity::Information < IssueSeverity::Low);
    assert!(IssueSeverity::Low < IssueSeverity::Medium);
    assert!(IssueSeverity::Medium < IssueSeverity::High);
    assert!(IssueSeverity::High.weight() > IssueSeverity::Medium.weight());
    assert_eq!(IssueSeverity::High.label(), "高危");
    assert_eq!(IssueSeverity::Information.en_label(), "Information");
}
