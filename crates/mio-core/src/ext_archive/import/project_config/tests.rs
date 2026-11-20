use super::extract_embedded_ext_project_config;
use crate::{ExtEmbeddedConfigStatus, RepeaterRedirectMode, Workspace};

#[test]
fn section_level_resilience_preserves_scope_match_replace_and_repeater_when_sessions_has_unknown_ext_fields()
 {
    let raw = br#"{
        "target": {
            "scope": {
                "advanced_mode": false,
                "include": [
                    {"enabled": true, "prefix": "https://portal.example.com/v1"},
                    {"enabled": true}
                ],
                "exclude": [
                    {"enabled": true, "prefix": "https://portal.example.com/v1/health"}
                ]
            }
        },
        "proxy": {
            "match_replace_rules": [
                {
                    "enabled": true,
                    "rule_type": "request_header",
                    "string_match": "^User-Agent:.*$",
                    "string_replace": "User-Agent: MIO-Compat",
                    "regex_match": true,
                    "comment": "valid rule"
                },
                {
                    "enabled": "not-a-bool",
                    "rule_type": "request_header"
                }
            ]
        },
        "repeater": {
            "unpack_compressed_responses": false,
            "update_content_length": true,
            "follow_redirects": "same_host",
            "request_timeout_secs": 30
        },
        "sessions": {
            "session_handling_rules": {"rules": [{"description": "External native macro rule"}]},
            "macros": [{"description": "login macro"}]
        }
    }"#;

    let report = extract_embedded_ext_project_config(raw);
    assert_eq!(report.status, ExtEmbeddedConfigStatus::Extracted);
    assert!(
        report
            .unmapped_keys
            .iter()
            .any(|key| key == "sessions (unmapped_archive_format)"),
        "unmapped_keys must record unmapped sessions section: {:?}",
        report.unmapped_keys
    );
    assert!(
        report
            .unmapped_keys
            .iter()
            .any(|key| key == "target.scope.include[1] (unmapped_archive_format)")
    );
    assert!(
        report
            .unmapped_keys
            .iter()
            .any(|key| key == "proxy.match_replace_rules[1] (unmapped_archive_format)")
    );
    assert!(report.config.sessions.is_none());

    let mut workspace = Workspace::default();
    assert!(report.config.apply_to_workspace(&mut workspace).unwrap());
    assert_eq!(workspace.scope.include.len(), 1);
    assert_eq!(
        workspace.scope.include[0].prefix,
        "https://portal.example.com/v1"
    );
    assert_eq!(workspace.scope.exclude.len(), 1);
    assert_eq!(workspace.match_and_replace_rules.len(), 1);
    assert_eq!(workspace.match_and_replace_rules[0].comment, "valid rule");
    assert!(!workspace.repeater_settings.unpack_compressed_responses);
    assert!(workspace.repeater_settings.update_content_length);
    assert_eq!(
        workspace.repeater_settings.follow_redirects,
        RepeaterRedirectMode::OnSiteOnly
    );
    assert_eq!(workspace.repeater_settings.request_timeout_secs, 30);
}

#[test]
fn repeater_follow_redirects_accepts_ext_aliases_and_isolates_unknown_values() {
    let cases = [
        (r#""never""#, RepeaterRedirectMode::Never),
        (r#""none""#, RepeaterRedirectMode::Never),
        (r#""false""#, RepeaterRedirectMode::Never),
        ("false", RepeaterRedirectMode::Never),
        (r#""on_site""#, RepeaterRedirectMode::OnSiteOnly),
        (r#""on_site_only""#, RepeaterRedirectMode::OnSiteOnly),
        (r#""same_host""#, RepeaterRedirectMode::OnSiteOnly),
        (r#""in_scope""#, RepeaterRedirectMode::InScopeOnly),
        (r#""in_scope_only""#, RepeaterRedirectMode::InScopeOnly),
        (r#""always""#, RepeaterRedirectMode::Always),
        (r#""true""#, RepeaterRedirectMode::Always),
        ("true", RepeaterRedirectMode::Always),
    ];
    for (raw_mode, expected) in cases {
        let json = format!(r#"{{"repeater":{{"follow_redirects":{raw_mode}}}}}"#);
        let report = extract_embedded_ext_project_config(json.as_bytes());
        assert_eq!(report.status, ExtEmbeddedConfigStatus::Extracted);
        assert_eq!(
            report.config.repeater.as_ref().unwrap().follow_redirects,
            Some(expected),
            "failed alias {raw_mode}"
        );
    }

    let unknown = br#"{
        "repeater": {
            "unpack_compressed_responses": false,
            "follow_redirects": "custom_mio_mode",
            "request_timeout_secs": 99999
        }
    }"#;
    let report = extract_embedded_ext_project_config(unknown);
    assert_eq!(report.status, ExtEmbeddedConfigStatus::Extracted);
    assert_eq!(
        report
            .config
            .repeater
            .as_ref()
            .unwrap()
            .unpack_compressed_responses,
        Some(false)
    );
    assert_eq!(
        report.config.repeater.as_ref().unwrap().follow_redirects,
        None
    );
    assert!(
        report
            .unmapped_keys
            .iter()
            .any(|k| k == "repeater.follow_redirects (unmapped_archive_format)")
    );
    assert!(
        report
            .unmapped_keys
            .iter()
            .any(|k| k == "repeater.request_timeout_secs (unmapped_archive_format)")
    );
}
