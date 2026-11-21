use super::*;
use crate::{
    Draft, HttpVersion, IntruderSpec, IntruderTab, Orientation, TabGroup, WorkTab,
    intruder::AttackType,
};

fn workspace() -> Workspace {
    let mut workspace = Workspace::default();
    workspace
        .open_request(Some(&super::tests::record().request))
        .unwrap();
    let group = TabGroup {
        id: Uuid::new_v4(),
        title: "草稿组".into(),
        collapsed: true,
        color: None,
    };
    let tab = IntruderTab {
        id: Uuid::new_v4(),
        title: "未发送 中文🙂".into(),
        group: Some(group.id),
        spec: IntruderSpec {
            resource_pool: None,
            baseline_request: false,
            revision: 1,
            draft: Draft {
                revision: 1,
                target: String::new(),
                raw: vec![0, 255, 10],
                version: HttpVersion::Http2,
            },
            attack_type: AttackType::ClusterBomb,
            positions: vec![],
            payload_sets: vec![vec![vec![], vec![0, 255], vec![0, 255]], vec![]],
            max_concurrent_requests: 0,
            request_timeout_ms: 0,
            update_host_header: true,
            save_to_project: true,
        },
        last_run: None,
    };
    workspace.intruder.orientation = Orientation::Vertical;
    workspace.intruder.tab_sidebar_width = 187;
    workspace.intruder.groups.push(group);
    workspace.intruder.active = Some(tab.id);
    let mut closed = tab.clone();
    closed.id = Uuid::new_v4();
    closed.title = "已关闭".into();
    workspace.intruder.tabs.push(tab);
    workspace.intruder.closed.push(closed);
    workspace
}

#[test]
fn repeater_settings_default_for_absent_fields_and_preserve_explicit_false() {
    let defaults = Workspace::default();
    assert!(defaults.repeater_settings.unpack_compressed_responses);
    assert!(defaults.repeater_settings.update_content_length);
    let mut legacy = serde_json::to_value(&defaults).unwrap();
    legacy.as_object_mut().unwrap().remove("repeater_settings");
    assert_eq!(
        serde_json::from_value::<Workspace>(legacy.clone()).unwrap(),
        defaults
    );
    legacy["repeater_settings"] = serde_json::json!({});
    assert_eq!(
        serde_json::from_value::<Workspace>(legacy.clone()).unwrap(),
        defaults
    );
    legacy["repeater_settings"] = serde_json::json!({
        "unpack_compressed_responses": false
    });
    let without_update_cl: Workspace = serde_json::from_value(legacy.clone()).unwrap();
    assert!(
        !without_update_cl
            .repeater_settings
            .unpack_compressed_responses
    );
    assert!(without_update_cl.repeater_settings.update_content_length);
    legacy["repeater_settings"]["unpack_compressed_responses"] = false.into();
    legacy["repeater_settings"]["update_content_length"] = false.into();
    let disabled: Workspace = serde_json::from_value(legacy).unwrap();
    assert!(!disabled.repeater_settings.unpack_compressed_responses);
    assert!(!disabled.repeater_settings.update_content_length);
    assert_eq!(
        serde_json::from_slice::<Workspace>(&serde_json::to_vec(&disabled).unwrap()).unwrap(),
        disabled
    );
    assert!(!defaults.proxy_messages.unpack_compressed_requests);
    assert!(defaults.proxy_messages.unpack_compressed_responses);
    let mut custom_messages = defaults.clone();
    custom_messages.proxy_messages = crate::ProxyMessageSettings {
        unpack_compressed_requests: true,
        unpack_compressed_responses: false,
        ..Default::default()
    };
    let encoded = serde_json::to_value(&custom_messages).unwrap();
    assert_eq!(
        serde_json::from_value::<Workspace>(encoded.clone()).unwrap(),
        custom_messages
    );
    for invalid in [
        serde_json::json!({"unpack_compressed_requests": true}),
        serde_json::json!({"unpack_compressed_requests": false, "unpack_compressed_responses": true, "unmapped": true}),
    ] {
        let mut corrupted = encoded.clone();
        corrupted["proxy_messages"] = invalid;
        assert!(serde_json::from_value::<Workspace>(corrupted).is_err());
    }
}

#[test]
fn upstream_tls_invalid_root_save_cannot_replace_the_previous_workspace() {
    let mut store = ProjectStore::create(None).unwrap();
    let original = workspace();
    store.save_workspace(&original).unwrap();
    let mut candidate = original.clone();
    candidate.revision += 1;
    candidate.upstream_tls.custom_ca_ders = vec![vec![0x30, 0]];
    assert!(store.save_workspace(&candidate).is_err());
    assert_eq!(store.workspace().unwrap(), original);
    let directory = tempfile::tempdir().unwrap();
    let copy = directory.path().join("invalid-tls.mio");
    assert!(
        store
            .save_with(
                Some(&copy),
                Some(&candidate),
                &[],
                &[],
                &[],
                &CancellationToken::new(),
            )
            .is_err()
    );
    assert!(!copy.exists());
    assert_eq!(store.workspace().unwrap(), original);
    store.close().unwrap();
}

#[test]
fn proxy_interception_structure_checks_do_not_weaken_saved_or_loaded_workspace_trust() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("interception-trust.mio");
    let mut store = ProjectStore::create(Some(&source)).unwrap();
    let original = workspace();
    let binary = super::tests::record();
    store.put(&binary).unwrap();
    store.save_workspace(&original).unwrap();
    store.save(None).unwrap();
    let source_bytes = fs::read(&source).unwrap();
    let mut candidate = original.clone();
    candidate.revision += 1;
    candidate.proxy_interception.request.rules[0].enabled = false;
    candidate.proxy_interception.request.rules[0].condition = "(".into();
    candidate.validate_structure().unwrap();
    assert!(
        candidate
            .validate()
            .unwrap_err()
            .to_string()
            .contains("正则")
    );
    assert!(store.save_workspace(&candidate).is_err());
    assert_eq!(store.workspace().unwrap(), original);
    assert_eq!(store.read(binary.summary.id).unwrap(), binary);
    assert_eq!(fs::read(&source).unwrap(), source_bytes);

    let mut invalid_id = original.clone();
    invalid_id.proxy_interception.request.rules[0].id = Uuid::nil();
    assert!(invalid_id.validate_structure().is_err());
    invalid_id.proxy_interception.request.rules[0].id =
        invalid_id.proxy_interception.response.rules[0].id;
    assert!(invalid_id.validate_structure().is_err());
    let mut oversized = original.clone();
    oversized.proxy_interception.request.rules = (0..32)
        .map(|index| crate::ProxyInterceptRule {
            id: Uuid::from_u128(1000 + index),
            enabled: false,
            operator: crate::ProxyInterceptOperator::And,
            match_type: crate::ProxyInterceptType::HttpMethod,
            relationship: crate::ProxyInterceptRelation::Matches,
            condition: "a".repeat(4096),
        })
        .collect();
    oversized.proxy_interception.response.rules.clear();
    oversized.validate_structure().unwrap();
    oversized
        .proxy_interception
        .response
        .rules
        .push(crate::ProxyInterceptRule {
            id: Uuid::from_u128(2000),
            enabled: false,
            operator: crate::ProxyInterceptOperator::Or,
            match_type: crate::ProxyInterceptType::StatusCode,
            relationship: crate::ProxyInterceptRelation::Matches,
            condition: "a".into(),
        });
    assert!(
        oversized
            .validate_structure()
            .unwrap_err()
            .to_string()
            .contains("128 KiB")
    );
    assert_eq!(store.workspace().unwrap(), original);
    store.close().unwrap();
    assert_eq!(fs::read(&source).unwrap(), source_bytes);

    let writer = Connection::open(&source).unwrap();
    let corrupt_workspace = serde_json::to_vec(&candidate).unwrap();
    writer
        .execute(
            "UPDATE metadata SET value=?1 WHERE key='workspace'",
            [&corrupt_workspace],
        )
        .unwrap();
    let stored_body: Vec<u8> = writer
        .query_row(
            "SELECT body FROM messages WHERE flow=?1 AND side=0",
            [binary.summary.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_body, binary.request.body);
    writer
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    writer.close().unwrap();
    let damaged_source_bytes = fs::read(&source).unwrap();
    let failed_reopen = match ProjectStore::open(&source) {
        Err(error) => error,
        Ok(store) => {
            store.close().unwrap();
            panic!("bad disabled regex must be rejected at read-only preflight")
        }
    };
    assert!(failed_reopen.to_string().contains("正则"));
    assert_eq!(
        fs::read(&source).unwrap(),
        damaged_source_bytes,
        "failed open cannot migrate or rewrite the damaged source"
    );
    let reader = Connection::open_with_flags(&source, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let unchanged_workspace: Vec<u8> = reader
        .query_row(
            "SELECT value FROM metadata WHERE key='workspace'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unchanged_workspace, corrupt_workspace);
    let unchanged_body: Vec<u8> = reader
        .query_row(
            "SELECT body FROM messages WHERE flow=?1 AND side=0",
            [binary.summary.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unchanged_body, binary.request.body);
    reader.close().unwrap();
}

#[test]
fn repeater_settings_use_workspace_cas_and_failed_save_can_retry_with_save_as() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("repeater-settings.mio");
    let copy = directory.path().join("repeater-settings-copy.mio");
    let mut store = ProjectStore::create(Some(&source)).unwrap();
    let mut disabled = store.workspace().unwrap();
    disabled.repeater_settings.unpack_compressed_responses = false;
    assert!(store.save_workspace(&disabled).is_err());
    assert!(
        store
            .workspace()
            .unwrap()
            .repeater_settings
            .unpack_compressed_responses
    );
    disabled.revision += 1;
    store.save_workspace(&disabled).unwrap();
    store.save(None).unwrap();

    let mut enabled = disabled.clone();
    enabled.repeater_settings.unpack_compressed_responses = true;
    assert!(store.save_workspace(&enabled).is_err());
    assert_eq!(store.workspace().unwrap(), disabled);
    enabled.revision += 1;
    store.reject_writes(true);
    assert!(store.save_workspace(&enabled).is_err());
    assert_eq!(store.workspace().unwrap(), disabled);
    store
        .save_with(
            Some(&copy),
            Some(&enabled),
            &[],
            &[],
            &[],
            &CancellationToken::new(),
        )
        .unwrap();
    store.close().unwrap();

    for (path, expected) in [(&source, disabled), (&copy, enabled)] {
        let reopened = ProjectStore::open(path).unwrap();
        assert_eq!(reopened.workspace().unwrap(), expected);
        assert_eq!(reopened.page(0, 1, "").unwrap().0, 0);
        reopened.close().unwrap();
    }
}

#[test]
fn unsent_incomplete_intruder_and_repeater_drafts_survive_save_save_as_and_both_reopens() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("drafts.mio");
    let copy = directory.path().join("drafts-copy.mio");
    let mut store = ProjectStore::create(None).unwrap();
    let workspace = workspace();
    store
        .save_with(
            Some(&source),
            Some(&workspace),
            &[],
            &[],
            &[],
            &CancellationToken::new(),
        )
        .unwrap();
    store.save(None).unwrap();
    store.save(Some(&copy)).unwrap();
    store.close().unwrap();

    for path in [&source, &copy] {
        let store = ProjectStore::open(path).unwrap();
        assert_eq!(store.workspace().unwrap(), workspace);
        assert!(store.intruder_runs(None, 10).unwrap().rows.is_empty());
        assert_eq!(
            store
                .connection
                .pragma_query_value::<i32, _>(None, "user_version", |row| row.get(0))
                .unwrap(),
            super::SCHEMA
        );
        store.close().unwrap();
    }
}

#[test]
fn intruder_config_and_draft_revisions_cover_open_and_closed_tabs_without_tool_reuse() {
    let mut store = ProjectStore::create(None).unwrap();
    let original = workspace();
    store.save_workspace(&original).unwrap();
    let reject = |store: &mut ProjectStore, candidate: &Workspace, expected: &Workspace| {
        assert!(store.save_workspace(candidate).is_err());
        assert_eq!(store.workspace().unwrap(), *expected);
    };

    let mut changed = original.clone();
    changed.revision += 1;
    changed.intruder.tabs[0].spec.update_host_header = false;
    reject(&mut store, &changed, &original);
    changed.intruder.tabs[0].spec.revision += 1;
    store.save_workspace(&changed).unwrap();
    let saved = changed.clone();
    changed.revision += 1;
    changed.intruder.tabs[0].spec.draft.raw.push(7);
    changed.intruder.tabs[0].spec.revision += 1;
    reject(&mut store, &changed, &saved);
    changed.intruder.tabs[0].spec.draft.revision += 1;
    store.save_workspace(&changed).unwrap();
    let latest = changed.clone();
    reject(&mut store, &changed, &latest);

    changed.revision += 1;
    changed.intruder.tabs.clear();
    changed.intruder.active = None;
    changed
        .intruder
        .closed
        .push(original.intruder.tabs[0].clone());
    reject(&mut store, &changed, &latest);
    changed.intruder.closed.pop();
    changed
        .intruder
        .closed
        .push(latest.intruder.tabs[0].clone());
    store.save_workspace(&changed).unwrap();
    let closed = changed.clone();

    changed.revision += 1;
    let intruder = changed.intruder.closed.pop().unwrap();
    let mut repeater = WorkTab::from_request(None, "not a conversion".into());
    repeater.id = intruder.id;
    changed.tabs.push(repeater);
    assert!(
        store
            .save_workspace(&changed)
            .unwrap_err()
            .to_string()
            .contains("所属工具")
    );
    assert_eq!(store.workspace().unwrap(), closed);

    changed = closed.clone();
    changed.revision += 1;
    let repeater = changed.tabs.remove(0);
    changed.active = None;
    let mut intruder = original.intruder.tabs[0].clone();
    intruder.id = repeater.id;
    changed.intruder.tabs.push(intruder);
    assert!(
        store
            .save_workspace(&changed)
            .unwrap_err()
            .to_string()
            .contains("所属工具")
    );
    assert_eq!(store.workspace().unwrap(), closed);

    changed = closed;
    changed.revision += 1;
    changed
        .intruder
        .tabs
        .push(changed.intruder.closed.pop().unwrap());
    changed.intruder.active = Some(changed.intruder.tabs[0].id);
    changed.intruder.tabs[0].title = "重开后重命名".into();
    store.save_workspace(&changed).unwrap();
    assert_eq!(store.workspace().unwrap(), changed);
}

#[test]
fn locked_workspace_save_keeps_latest_intruder_input_for_save_as() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("locked.mio");
    let copy = directory.path().join("recovered.mio");
    let mut store = ProjectStore::create(Some(&source)).unwrap();
    let original = workspace();
    store.save_workspace(&original).unwrap();
    let mut latest = original.clone();
    latest.revision += 1;
    latest.intruder.tabs[0].spec.draft.raw.push(42);
    latest.intruder.tabs[0].spec.draft.revision += 1;
    latest.intruder.tabs[0].spec.revision += 1;
    let writer = rusqlite::Connection::open(&source).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE;").unwrap();
    assert!(store.save_workspace(&latest).is_err());
    assert_eq!(store.workspace().unwrap(), original);
    store
        .save_with(
            Some(&copy),
            Some(&latest),
            &[],
            &[],
            &[],
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(store.workspace().unwrap(), latest);
    writer.execute_batch("ROLLBACK;").unwrap();
    writer.close().unwrap();
    store.close().unwrap();
    assert_eq!(
        ProjectStore::open(&source).unwrap().workspace().unwrap(),
        original
    );
    let restored = ProjectStore::open(&copy).unwrap();
    assert_eq!(restored.workspace().unwrap(), latest);
    assert!(restored.intruder_runs(None, 10).unwrap().rows.is_empty());
}

#[test]
fn phase2_workspace_fields_persist_across_reopen_and_default_from_legacy_json() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("phase2-settings.mio");
    let mut store = ProjectStore::create(Some(&path)).unwrap();
    let mut ws = store.workspace().unwrap();
    assert!(ws.tasks_auto_start);
    assert!(ws.live_passive_crawl_enabled);

    ws.revision += 1;
    ws.tasks_auto_start = false;
    ws.live_passive_crawl_enabled = false;
    ws.scope.include.push(crate::ScopeRule {
        id: Uuid::new_v4(),
        enabled: true,
        prefix: "https://example.com".into(),
        include_subdomains: true,
    });
    ws.scope.drop_out_of_scope = true;
    ws.scope.out_of_scope_use_custom = true;
    ws.scope.out_of_scope_custom_rules.push(crate::ScopeRule {
        id: Uuid::new_v4(),
        enabled: true,
        prefix: "https://allow.example.com".into(),
        include_subdomains: true,
    });
    ws.network_http.redirect_3xx = false;
    ws.network_http.redirect_javascript = true;
    ws.network_http.redirect_any_location = true;
    ws.network_http.streaming_advanced_scope = true;
    ws.network_http.streaming_scope_rules.push(crate::ScopeRule {
        id: Uuid::new_v4(),
        enabled: true,
        prefix: "https://stream.example.com".into(),
        include_subdomains: false,
    });
    ws.repeater_settings.http1_connection_reuse = true;
    ws.repeater_settings.http2_connection_reuse = false;
    ws.repeater_settings.allow_http2_alpn_override = true;
    ws.repeater_settings.process_cookies_in_redirects = true;
    ws.repeater_settings.cross_domain_protocol_redirects = true;
    ws.repeater_settings.streaming_response_timeout_secs = 0;
    ws.upstream_proxy.override_project_only = true;
    ws.upstream_proxy.rules.push(crate::UpstreamProxyRule {
        id: Uuid::new_v4(),
        enabled: true,
        destination_host: "*.corp.local".into(),
        protocol: crate::UpstreamProxyProtocol::Http,
        host: "127.0.0.1".into(),
        port: 8888,
        auth_type: crate::UpstreamAuthType::NtlmV2,
        username: "u1".into(),
        password: "p1".into(),
        domain: "CORP".into(),
        workstation: "WS01".into(),
        description: "Corp gateway".into(),
    });
    ws.upstream_proxy.platform_auth = crate::PlatformAuthSettings {
        enabled: true,
        prompt_on_failure: true,
        override_project_only: true,
        rules: vec![crate::PlatformAuthRule {
            id: Uuid::new_v4(),
            enabled: true,
            destination_host: "app.example.invalid".into(),
            auth_type: crate::PlatformAuthType::Digest,
            username: "svc".into(),
            password: "pw".into(),
            domain: "LAB".into(),
            hostname: "HOST1".into(),
        }],
    };
    ws.ssl_pass_through_rules.push(crate::SslPassThroughRule {
        id: Uuid::new_v4(),
        enabled: true,
        host_pattern: "*.pin.example.invalid".into(),
        port: Some(8443),
        is_regex: false,
    });
    ws.ws_match_and_replace_rules
        .push(crate::WsMatchAndReplaceRule {
            id: Uuid::new_v4(),
            enabled: true,
            direction: crate::WsReplaceDirection::Both,
            match_pattern: "ping".into(),
            replace_with: "pong".into(),
            is_regex: false,
            comment: "ws rule".into(),
        });
    ws.upstream_proxy.socks.bypass_hosts = vec!["localhost".into(), "*.internal".into()];
    ws.upstream_proxy.socks.override_project_only = true;
    ws.upstream_tls.protocol_mode = crate::TlsProtocolMode::Default;
    ws.upstream_tls.disable_session_resume = true;
    ws.upstream_tls.client_certs_override_project_only = true;
    ws.proxy_listeners.push(crate::ProxyListenerConfig {
        id: Uuid::new_v4(),
        enabled: true,
        bind_address: "127.0.0.1:8081".into(),
        custom_cert_pem: Some("-----BEGIN CERTIFICATE-----\nTEST\n-----END CERTIFICATE-----".into()),
        custom_key_pem: Some("-----BEGIN PRIVATE KEY-----\nTEST\n-----END PRIVATE KEY-----".into()),
        ..crate::ProxyListenerConfig::default()
    });
    ws.scheduled_tasks.push(crate::ScheduledTask {
        id: Uuid::new_v4(),
        enabled: true,
        action: crate::ScheduledTaskAction::RunScan,
        trigger_spec: "0 2 * * *".into(),
        description: "Nightly scan".into(),
    });
    ws.organiser_collections.push(crate::OrganiserCollection {
        id: Uuid::new_v4(),
        name: "Auth flows".into(),
        item_ids: vec![Uuid::new_v4()],
    });
    ws.custom_scan_checks.push(crate::CustomScanCheck {
        id: Uuid::new_v4(),
        enabled: true,
        name: "Custom Header Leak".into(),
        severity: crate::AuditSeverity::High,
        confidence: crate::AuditConfidence::Certain,
        script: "response.has_header(\"X-Debug\")".into(),
        description: "Flags debug response headers".into(),
    });

    store.save_workspace(&ws).unwrap();
    store.save(None).unwrap();
    store.close().unwrap();

    let reopened = ProjectStore::open(&path).unwrap();
    assert_eq!(reopened.workspace().unwrap(), ws);
    reopened.close().unwrap();

    let legacy_ws: Workspace = serde_json::from_str(
        r#"{"revision":0,"orientation":"Horizontal","groups":[],"tabs":[],"active":null,"closed":[]}"#,
    )
    .unwrap();
    assert_eq!(legacy_ws, Workspace::default());
    assert!(legacy_ws.tasks_auto_start);
    assert!(legacy_ws.live_passive_crawl_enabled);
    assert!(legacy_ws.ssl_pass_through_rules.is_empty());
    assert!(legacy_ws.ws_match_and_replace_rules.is_empty());
    assert!(legacy_ws.upstream_proxy.platform_auth.is_default());
}
