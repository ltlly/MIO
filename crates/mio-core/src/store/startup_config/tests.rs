use tokio_util::sync::CancellationToken;

use super::*;
use crate::{StartupDefaultConfig, Uuid, store::ProjectStore};

#[test]
fn startup_configuration_cancelled_during_workspace_update_rolls_back_before_commit() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let configuration = parse_config_bytes(
        br#"{"proxy_interception":{"request":{"enabled":false,"rules":[]},"response":{"enabled":true,"rules":[]}},"proxy_messages":{"unpack_compressed_requests":false,"unpack_compressed_responses":true},"scope":{},"upstream_tls":{"custom_ca_ders":[],"include_system_roots":true},"match_and_replace_rules":[],"repeater":{"request_timeout_secs":17}}"#,
    )
    .unwrap();
    for (index, mode) in [
        StartupDefaultConfig::Defaults,
        StartupDefaultConfig::ConfigFile {
            path: directory.path().join("options.json"),
        },
    ]
    .into_iter()
    .enumerate()
    {
        let path = directory.path().join(format!("cancelled-{index}.mio"));
        let mut store = ProjectStore::create(Some(&path)).unwrap();
        let mut original = store.workspace().unwrap();
        original.open_request(None).unwrap();
        original.tabs[0].draft.raw.extend_from_slice(&[0, 255]);
        original.project_name = Some("原始项目".into());
        original.proxy_interception.request.rules[3].enabled = true;
        original.proxy_interception.response.enabled = true;
        original.proxy_messages = crate::ProxyMessageSettings {
            unpack_compressed_requests: true,
            unpack_compressed_responses: false,
            ..Default::default()
        };
        original.scope.include.push(crate::ScopeRule {
            id: Uuid::new_v4(),
            enabled: true,
            prefix: "http://127.0.0.1:1/".into(),
            include_subdomains: false,
        });
        original.upstream_tls = crate::UpstreamTlsSettings {
            custom_ca_ders: vec![
                mio_proxy::MitmAuthority::generate()
                    .unwrap()
                    .certificate_der(),
            ],
            include_system_roots: false,
            insecure_skip_verify: false,
            ..crate::UpstreamTlsSettings::default()
        };
        original
            .match_and_replace_rules
            .push(crate::MatchAndReplaceRule {
                id: Uuid::new_v4(),
                enabled: true,
                item_type: crate::MatchReplaceTarget::RequestHeader,
                match_pattern: "X-Original".into(),
                replace_with: "X-Preserved".into(),
                regex: false,
                comment: String::new(),
                ..Default::default()
            });
        store.save_workspace(&original).unwrap();
        store.checkpoint().unwrap();
        let source_bytes = std::fs::read(&path).unwrap();
        let encoded_before: Vec<u8> = store
            .connection
            .query_row(
                "SELECT value FROM main.metadata WHERE key='workspace'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let changes_before: u64 = store
            .connection
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .unwrap();
        let cancel = CancellationToken::new();
        let during_update = cancel.clone();
        store
            .connection
            .authorizer(Some(move |context: AuthContext<'_>| {
                if matches!(
                    context.action,
                    AuthAction::Update {
                        table_name: "metadata",
                        column_name: "value",
                    }
                ) && context.database_name == Some("main")
                {
                    during_update.cancel();
                }
                Authorization::Allow
            }));
        let outcome = store.apply_startup_options(
            Some("不应保存的名称".into()),
            &mode,
            Some(&configuration),
            &cancel,
        );
        store
            .connection
            .authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        let changes_after: u64 = store
            .connection
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .unwrap();
        let encoded_after: Vec<u8> = store
            .connection
            .query_row(
                "SELECT value FROM main.metadata WHERE key='workspace'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let restored = store.workspace().unwrap();
        assert!(cancel.is_cancelled());
        assert_eq!(
            changes_after,
            changes_before + 1,
            "the workspace UPDATE must execute before rollback"
        );
        assert!(
            outcome.is_err() && encoded_after == encoded_before && restored == original,
            "cancelled workspace UPDATE must reject and roll back: rejected={}, encoded_preserved={}, workspace_preserved={}",
            outcome.is_err(),
            encoded_after == encoded_before,
            restored == original
        );
        assert!(outcome.unwrap_err().to_string().contains("取消"));
        store.close().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), source_bytes);
        let reopened = ProjectStore::open(&path).unwrap();
        assert_eq!(reopened.workspace().unwrap(), original);
        reopened.close().unwrap();
    }
}

#[test]
fn interception_config_preserves_order_disabled_rules_and_defaults_through_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conditional.mio");
    let copy = directory.path().join("conditional-copy.mio");
    let mut interception = crate::ProxyInterceptionSettings::default();
    interception.request.rules[0].enabled = false;
    interception.request.rules[3].enabled = true;
    interception.request.rules.swap(1, 3);
    interception.response.enabled = true;
    interception.response.rules[2].enabled = true;
    let messages = crate::ProxyMessageSettings {
        unpack_compressed_requests: true,
        unpack_compressed_responses: false,
        ..Default::default()
    };
    let configuration = parse_config_bytes(
        &serde_json::to_vec(
            &serde_json::json!({"proxy_interception":interception,"proxy_messages":messages}),
        )
        .unwrap(),
    )
    .unwrap();
    let cancel = CancellationToken::new();
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let mut workspace = store.workspace().unwrap();
    workspace.open_request(None).unwrap();
    workspace.tabs[0].draft.raw.extend_from_slice(&[0, 255]);
    store.save_workspace(&workspace).unwrap();
    store
        .apply_startup_options(
            None,
            &StartupDefaultConfig::ConfigFile {
                path: directory.path().join("options.json"),
            },
            Some(&configuration),
            &cancel,
        )
        .unwrap();
    workspace.revision += 1;
    workspace.proxy_interception = interception;
    workspace.proxy_messages = messages;
    assert_eq!(store.workspace().unwrap(), workspace);
    store.save(None).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();
    let mut reopened = ProjectStore::open(&copy).unwrap();
    assert_eq!(reopened.workspace().unwrap(), workspace);
    reopened
        .apply_startup_options(None, &StartupDefaultConfig::Project, None, &cancel)
        .unwrap();
    assert_eq!(reopened.workspace().unwrap(), workspace);
    reopened
        .apply_startup_options(None, &StartupDefaultConfig::Defaults, None, &cancel)
        .unwrap();
    workspace.revision += 1;
    workspace.proxy_interception = crate::ProxyInterceptionSettings::default();
    workspace.proxy_messages = crate::ProxyMessageSettings::default();
    assert_eq!(reopened.workspace().unwrap(), workspace);
    reopened.close().unwrap();
    for invalid in [
        br#"{"proxy_interception":null}"#.as_slice(),
        br#"{"proxy_interception":{},"proxy_interception":{}}"#,
        br#"{"proxy_interception":{"request":{"enabled":true,"rules":[]},"response":{"enabled":false,"rules":[]},"unmapped":true}}"#,
        br#"{"proxy_messages":null}"#,
        br#"{"proxy_messages":{"unpack_compressed_requests":false}}"#,
        br#"{"proxy_messages":{"unpack_compressed_requests":false,"unpack_compressed_responses":true},"proxy_messages":{"unpack_compressed_requests":true,"unpack_compressed_responses":false}}"#,
        br#"{"proxy_messages":{"unpack_compressed_requests":false,"unpack_compressed_responses":true,"unknown":false}}"#,
    ] {
        assert!(parse_config_bytes(invalid).is_err());
    }
    let original = ProjectStore::open(&path).unwrap();
    assert!(
        !original
            .workspace()
            .unwrap()
            .proxy_interception
            .is_default()
    );
    original.close().unwrap();
}

#[test]
fn upstream_tls_config_import_save_as_reopen_and_defaults_preserve_explicit_trust() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("tls-project.mio");
    let copy = directory.path().join("tls-copy.mio");
    let tls = crate::UpstreamTlsSettings {
        custom_ca_ders: vec![
            mio_proxy::MitmAuthority::generate()
                .unwrap()
                .certificate_der(),
        ],
        include_system_roots: false,
        insecure_skip_verify: false,
        ..crate::UpstreamTlsSettings::default()
    };
    let config =
        parse_config_bytes(&serde_json::to_vec(&serde_json::json!({"upstream_tls": tls})).unwrap())
            .unwrap();
    let mode = StartupDefaultConfig::ConfigFile {
        path: directory.path().join("tls.json"),
    };
    let cancel = CancellationToken::new();
    let mut store = ProjectStore::create(Some(&source)).unwrap();
    let mut original = store.workspace().unwrap();
    original.open_request(None).unwrap();
    original.tabs[0].draft.raw.extend_from_slice(&[0, 255]);
    store.save_workspace(&original).unwrap();
    store
        .apply_startup_options(None, &mode, Some(&config), &cancel)
        .unwrap();
    let mut expected = original.clone();
    expected.revision += 1;
    expected.upstream_tls = tls;
    assert_eq!(store.workspace().unwrap(), expected);
    let empty_config = parse_config_bytes(b"{}").unwrap();
    store
        .apply_startup_options(None, &mode, Some(&empty_config), &cancel)
        .unwrap();
    assert_eq!(store.workspace().unwrap(), expected);
    store.save(None).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();

    let source_store = ProjectStore::open(&source).unwrap();
    assert_eq!(source_store.workspace().unwrap(), expected);
    source_store.close().unwrap();
    let mut copy_store = ProjectStore::open(&copy).unwrap();
    assert_eq!(copy_store.workspace().unwrap(), expected);
    copy_store
        .apply_startup_options(None, &StartupDefaultConfig::Project, None, &cancel)
        .unwrap();
    assert_eq!(copy_store.workspace().unwrap(), expected);
    copy_store
        .apply_startup_options(None, &StartupDefaultConfig::Defaults, None, &cancel)
        .unwrap();
    expected.revision += 1;
    expected.upstream_tls = crate::UpstreamTlsSettings::default();
    assert_eq!(copy_store.workspace().unwrap(), expected);
    copy_store.save(None).unwrap();
    copy_store.close().unwrap();
    let reopened = ProjectStore::open(&copy).unwrap();
    assert_eq!(reopened.workspace().unwrap(), expected);
    reopened.close().unwrap();

    for invalid in [
        br#"{"upstream_tls":null}"#.as_slice(),
        br#"{"upstream_tls":{"custom_ca_ders":[[48,0]],"include_system_roots":true}}"#,
        br#"{"upstream_tls":{"custom_ca_ders":[],"include_system_roots":false}}"#,
        br#"{"upstream_tls":{"unknown_option":true}}"#,
    ] {
        assert!(parse_config_bytes(invalid).is_err());
    }
}

#[test]
fn startup_config_guard_rejects_duplicate_keys_nested_nulls_and_rolls_back_failed_apply() {
    let pool_id = Uuid::new_v4();
    for invalid in [
        br#"{"proxy":{"request_listeners":[{"listener_port":8080}]},"proxy":{"request_listeners":[]}}"#.as_slice(),
        br#"{"repeater":{"unpack_compressed_responses":false,"unpack_compressed_responses":true}}"#.as_slice(),
        br#"{"repeater":{"unpack_compressed_responses":null}}"#.as_slice(),
        br#"{"repeater":{"update_content_length":null}}"#.as_slice(),
        br#"{"repeater":{"normalize_http1_line_endings":null}}"#.as_slice(),
        br#"{"repeater":{"follow_redirects":null}}"#.as_slice(),
        br#"{"repeater_settings":{"unpack_compressed_responses":null}}"#.as_slice(),
        br#"{"upstream_tls":{"include_system_roots":null}}"#.as_slice(),
        br#"{"upstream_tls":{"custom_ca_ders":null}}"#.as_slice(),
        br#"{"upstream_tls":{"include_system_roots":true,"include_system_roots":false}}"#.as_slice(),
        br#"{"upstream_tls":{},"upstream_tls":{}}"#.as_slice(),
        format!(
            r#"{{"intruder_resource_pools":{{"default_pool_id":null,"pools":[{{"id":"{pool_id}","name":"pool","max_concurrent_requests":4}}]}}}}"#
        )
        .as_bytes(),
        format!(
            r#"{{"intruder_resource_pools":{{"pools":[{{"id":"{pool_id}","name":"a","name":"b","max_concurrent_requests":4}}]}}}}"#
        )
        .as_bytes(),
        format!(
            r#"{{"resource_pools":[{{"id":"{pool_id}","name":"a","name":"b","max_concurrent_requests":4}}]}}"#
        )
        .as_bytes(),
    ] {
        assert!(
            parse_config_bytes(invalid).is_err(),
            "must reject invalid config bytes: {}",
            String::from_utf8_lossy(invalid)
        );
    }

    let mut store = ProjectStore::create(None).unwrap();
    let existing_default = store
        .ensure_intruder_default_pool(
            crate::ProjectKey {
                id: store.id,
                generation: 1,
            },
            &CancellationToken::new(),
        )
        .unwrap();
    let workspace_before = store.workspace().unwrap();
    let tls = crate::UpstreamTlsSettings {
        custom_ca_ders: vec![
            mio_proxy::MitmAuthority::generate()
                .unwrap()
                .certificate_der(),
        ],
        include_system_roots: false,
        insecure_skip_verify: false,
        ..crate::UpstreamTlsSettings::default()
    };
    let conflicting = parse_config_bytes(
        format!(
            r#"{{"repeater":{{"unpack_compressed_responses":false}},"upstream_tls":{},"intruder_resource_pools":{{"default_pool_id":"{pool_id}","pools":[{{"id":"{pool_id}","name":"conflict","max_concurrent_requests":4}}]}}}}"#,
            serde_json::to_string(&tls).unwrap()
        )
        .as_bytes(),
    )
    .unwrap();
    assert!(
        store
            .apply_startup_options(
                Some("回滚名称".into()),
                &StartupDefaultConfig::ConfigFile {
                    path: "/tmp/conflict.json".into(),
                },
                Some(&conflicting),
                &CancellationToken::new(),
            )
            .is_err()
    );
    assert_eq!(store.workspace().unwrap(), workspace_before);
    assert_eq!(store.intruder_resource_pools().unwrap(), existing_default);
}

#[test]
fn ext_proxy_ssl_passthrough_ws_match_replace_and_response_mod_startup_import_roundtrip() {
    let directory = tempfile::tempdir().unwrap();
    let config_json = br#"{
        "proxy": {
            "request_listeners": [],
            "ssl_pass_through": {
                "automatically_add_entries_on_client_ssl_negotiation_failure": true,
                "rules": [
                    {"enabled": true, "host": "*.pinning.example.com", "port": "443", "is_simple_match": true},
                    {"enabled": false, "host_pattern": "^127\\.0\\.0\\.1$", "port": 8443, "is_regex": true}
                ]
            },
            "ws_match_replace_rules": [
                {
                    "enabled": true,
                    "rule_type": "client_to_server",
                    "string_match": "ping-token",
                    "string_replace": "pong-token",
                    "is_simple_match": true,
                    "comment": "c2s token"
                },
                {
                    "enabled": true,
                    "direction": "server_to_client",
                    "match_pattern": "secret=(\\d+)",
                    "replace_with": "secret=***$1***",
                    "is_regex": true,
                    "comment": "mask"
                }
            ],
            "response_modification": {
                "unhide_hidden_form_fields": true,
                "enable_disabled_form_fields": true,
                "highlight_unhidden_fields": true
            }
        }
    }"#;
    let parsed = parse_config_bytes(config_json).unwrap();
    assert_eq!(parsed.ssl_pass_through_rules.as_ref().unwrap().len(), 2);
    assert_eq!(parsed.ws_match_and_replace_rules.as_ref().unwrap().len(), 2);
    assert!(
        parsed
            .response_modification
            .unwrap()
            .unhide_hidden_form_fields
    );

    let path = directory.path().join("proxy-adv.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let cancel = CancellationToken::new();
    store
        .apply_startup_options(
            None,
            &StartupDefaultConfig::ConfigFile {
                path: directory.path().join("ext_archive.json"),
            },
            Some(&parsed),
            &cancel,
        )
        .unwrap();
    let ws = store.workspace().unwrap();
    assert!(ws.match_and_replace_rules.is_empty());
    let ssl_rules = &ws.ssl_pass_through_rules;
    let ws_rules = &ws.ws_match_and_replace_rules;
    assert_eq!(ssl_rules.len(), 2);
    assert_eq!(ssl_rules[0].host_pattern, "*.pinning.example.com");
    assert_eq!(ssl_rules[0].port, Some(443));
    assert!(!ssl_rules[0].is_regex);
    assert!(ssl_rules[1].is_regex);
    assert_eq!(ws_rules.len(), 2);
    assert_eq!(
        ws_rules[0].direction,
        crate::WsReplaceDirection::ClientToServer
    );
    assert_eq!(parsed.ssl_pass_through_auto_add_on_failure, Some(true));
    assert!(
        ws.proxy_messages
            .response_modification
            .enable_disabled_form_fields
    );
    assert!(ws.proxy_messages.ssl_pass_through_auto_add_on_failure);
    store
        .apply_startup_options(None, &StartupDefaultConfig::Defaults, None, &cancel)
        .unwrap();
    let reset_ws = store.workspace().unwrap();
    assert!(reset_ws.proxy_messages.is_default());
    assert!(reset_ws.ssl_pass_through_rules.is_empty());
    assert!(reset_ws.ws_match_and_replace_rules.is_empty());
    store.close().unwrap();

    for bad in [
        br#"{"proxy":{"ssl_pass_through":{"rules":[{"enabled":true,"host":"(","is_regex":true}]}}}"#.as_slice(),
        br#"{"proxy":{"ws_match_replace_rules":[{"enabled":true,"match_pattern":"(","is_regex":true}]}}"#.as_slice(),
        br#"{"proxy":{"response_modification":{"strip_secure_cookies":true}}}"#.as_slice(),
    ] {
        assert!(parse_config_bytes(bad).is_err());
    }
}
