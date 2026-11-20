use crate::{
    ExtCategory, ExtCollectionReport, ExtEmbeddedConfigStatus, ExtImportReport,
    RepeaterRedirectMode, Workspace, extract_embedded_ext_project_config,
};

#[test]
fn embedded_project_config_extracts_safe_sections_skips_unsafe_and_applies_to_workspace() {
    let raw = br#"{
        "target": {
            "scope": {
                "advanced_mode": false,
                "include": [{"enabled": true, "prefix": "https://app.example.com/api"}],
                "exclude": [{"enabled": true, "prefix": "https://app.example.com/api/logout"}]
            },
            "crawl": {"max_link_depth": 5}
        },
        "proxy": {
            "request_listeners": [{"listener_port": 8080, "running": true}],
            "match_replace_rules": [{
                "enabled": true,
                "rule_type": "request_header",
                "string_match": "",
                "string_replace": "X-Debug: 1",
                "is_simple_match": true,
                "comment": "inject header"
            }],
            "unknown_proxy_flag": true
        },
        "repeater": {
            "unpack_compressed_responses": false,
            "follow_redirects": "on_site_only",
            "request_timeout_secs": 45
        },
        "extender": {"extensions": [{"loaded": true}]},
        "scanner": {"audit_optimization": "fast"}
    }"#;

    let report = extract_embedded_ext_project_config(raw);
    assert_eq!(report.status, ExtEmbeddedConfigStatus::Extracted);
    assert!(!report.skipped_unsafe_sections.is_empty());
    assert!(
        report
            .unmapped_keys
            .iter()
            .any(|k| k == "proxy.unknown_proxy_flag")
    );

    let mut ws = Workspace::default();
    assert!(report.config.apply_to_workspace(&mut ws).unwrap());
    assert_eq!(ws.scope.include.len(), 1);
    assert_eq!(ws.scope.exclude.len(), 1);
    assert_eq!(ws.match_and_replace_rules.len(), 1);
    assert!(!ws.repeater_settings.unpack_compressed_responses);
    assert_eq!(
        ws.repeater_settings.follow_redirects,
        RepeaterRedirectMode::OnSiteOnly
    );
    assert_eq!(ws.repeater_settings.request_timeout_secs, 45);
    assert!(
        ws.proxy_listeners.is_empty(),
        "embedded config import must never bind or overwrite proxy listeners"
    );
}

#[test]
fn coverage_report_classifies_http_websocket_repeater_config_and_unmapped_buckets() {
    let report = ExtImportReport {
        schema_floor: 229,
        schema_revision: 229,
        source_bytes: 4096,
        indexed_items: 14,
        indexed_messages: 22,
        collections: vec![
            ExtCollectionReport {
                path: "root.f1.f0".into(),
                logical_items: Some(5),
                indexed_items: 5,
                message_count: 10,
                state: "available".into(),
            },
            ExtCollectionReport {
                path: "root.f1.f2".into(),
                logical_items: Some(4),
                indexed_items: 4,
                message_count: 4,
                state: "available".into(),
            },
            ExtCollectionReport {
                path: "root.f2.f1".into(),
                logical_items: Some(3),
                indexed_items: 3,
                message_count: 6,
                state: "available".into(),
            },
            ExtCollectionReport {
                path: "root.f7.f0".into(),
                logical_items: Some(2),
                indexed_items: 2,
                message_count: 2,
                state: "unmapped".into(),
            },
        ],
        repeater_active_tab: Some(0),
        gaps: vec![
            "root.f4.f0 (Embedded Project Config): 已提取 Scope 规则、Repeater 设置。".into(),
        ],
        complete_archive: true,
    };

    let coverage = report.coverage_report();
    assert_eq!(coverage.categories.len(), ExtCategory::ALL.len());
    assert_eq!(
        coverage
            .category(ExtCategory::ProxyHttpHistory)
            .unwrap()
            .recovered_items,
        5
    );
    assert_eq!(
        coverage
            .category(ExtCategory::ProxyWebSocketHistory)
            .unwrap()
            .recovered_items,
        4
    );
    assert_eq!(
        coverage
            .category(ExtCategory::RepeaterWorkspaceAndHistory)
            .unwrap()
            .recovered_items,
        3
    );
    assert_eq!(
        coverage
            .category(ExtCategory::EmbeddedProjectConfig)
            .unwrap()
            .state,
        "extracted"
    );
    assert!(
        coverage
            .category(ExtCategory::UnmappedSlots)
            .unwrap()
            .collection_paths
            .iter()
            .any(|p| p == "root.f7.f0")
    );
}
