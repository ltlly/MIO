use crate::Orientation;

use super::*;

#[test]
fn upstream_tls_rejects_fake_der_in_parser_and_workspace() {
    let fake_der = vec![0x30, 0];
    assert!(crate::UpstreamTlsSettings::parse_pem_or_der_cert(&fake_der).is_err());
    let mut workspace = Workspace::default();
    workspace.upstream_tls.custom_ca_ders = vec![fake_der];
    assert!(workspace.validate().is_err());
}

#[test]
fn upstream_tls_der_preserves_signature_whitespace_and_rejects_extra_bytes() {
    let mut der = mio_proxy::MitmAuthority::generate()
        .unwrap()
        .certificate_der();
    // The trust anchor's opaque signature can contain ASCII whitespace bytes.
    *der.last_mut().unwrap() = b' ';
    assert_eq!(
        crate::UpstreamTlsSettings::parse_pem_or_der_cert(&der).unwrap(),
        der
    );
    let mut suffixed = der.clone();
    suffixed.extend_from_slice(b"\r\n");
    assert!(crate::UpstreamTlsSettings::parse_pem_or_der_cert(&suffixed).is_err());
    let mut prefixed = b" ".to_vec();
    prefixed.extend_from_slice(&der);
    assert!(crate::UpstreamTlsSettings::parse_pem_or_der_cert(&prefixed).is_err());
}

#[test]
fn upstream_tls_pem_requires_one_whole_certificate_block() {
    let der = mio_proxy::MitmAuthority::generate()
        .unwrap()
        .certificate_der();
    let encoded = crate::intruder::apply_payload_processing_rule(
        std::slice::from_ref(&der),
        &crate::intruder::PayloadProcessingRule::Base64Encode,
    )
    .unwrap();
    let encoded = std::str::from_utf8(&encoded[0]).unwrap();
    let pem = format!("-----BEGIN CERTIFICATE-----\n{encoded}\n-----END CERTIFICATE-----");
    assert_eq!(
        crate::UpstreamTlsSettings::parse_pem_or_der_cert(format!(" \r\n{pem}\r\n ").as_bytes())
            .unwrap(),
        der
    );
    for invalid in [
        format!("discarded prefix\n{pem}"),
        format!("{pem}\ndiscarded suffix"),
        format!("{pem}\n{pem}"),
        format!("{pem}\n-----BEGIN PRIVATE KEY-----\nAQID\n-----END PRIVATE KEY-----"),
        "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----".into(),
        "-----BEGIN CERTIFICATE-----\nMAA=\n-----END CERTIFICATE-----".into(),
        "-----BEGIN CERTIFICATE-----\n-----END CERTIFICATE-----".into(),
    ] {
        assert!(
            crate::UpstreamTlsSettings::parse_pem_or_der_cert(invalid.as_bytes()).is_err(),
            "ambiguous or malformed PEM must not be accepted"
        );
    }
}

#[test]
fn upstream_tls_empty_disabled_roots_do_not_fall_back_to_system() {
    let settings = crate::UpstreamTlsSettings {
        custom_ca_ders: Vec::new(),
        include_system_roots: false,
        insecure_skip_verify: false,
        ..crate::UpstreamTlsSettings::default()
    };
    assert!(matches!(
        settings.to_proxy_tls(),
        mio_proxy::UpstreamTls::CustomRoots {
            certificates_der,
            include_system: false,
        } if certificates_der.is_empty()
    ));
    let workspace = Workspace {
        upstream_tls: settings,
        ..Workspace::default()
    };
    assert!(workspace.validate().is_err());
}

#[test]
fn upstream_tls_model_preserves_defaults_and_explicit_system_setting() {
    assert_eq!(
        serde_json::from_str::<crate::UpstreamTlsSettings>("{}").unwrap(),
        crate::UpstreamTlsSettings::default()
    );
    let insecure = crate::UpstreamTlsSettings {
        custom_ca_ders: Vec::new(),
        include_system_roots: false,
        insecure_skip_verify: true,
        ..crate::UpstreamTlsSettings::default()
    };
    insecure.validate().unwrap();
    assert!(matches!(
        insecure.to_proxy_tls(),
        mio_proxy::UpstreamTls::Insecure
    ));
    assert!(
        !serde_json::from_str::<crate::UpstreamTlsSettings>(r#"{"include_system_roots":false}"#)
            .unwrap()
            .include_system_roots
    );
    let protocol_only = crate::UpstreamTlsSettings {
        protocol_mode: crate::TlsProtocolMode::Custom,
        allow_unsafe_renegotiation: true,
        disable_session_resume: true,
        client_certs_override_project_only: true,
        ..crate::UpstreamTlsSettings::default()
    };
    assert!(!protocol_only.is_default());
    assert!(!protocol_only.has_custom_root_override());
    protocol_only.validate().unwrap();
    assert!(matches!(
        protocol_only.to_proxy_tls(),
        mio_proxy::UpstreamTls::System
    ));
}

#[test]
fn new_repeater_draft_has_a_bindable_revision() {
    for request in [
        None,
        Some(
            crate::parse_request(
                b"GET / HTTP/1.1\r\n\r\n",
                "http://localhost/",
                HttpVersion::Http11,
            )
            .unwrap(),
        ),
    ] {
        let tab = WorkTab::from_request(request.as_ref(), "1".into());
        assert!(
            tab.draft.revision > 0,
            "a new persisted draft must be bindable without an artificial edit"
        );
    }
}

#[test]
fn orientation_close_restore_do_not_change_draft_identity() {
    let mut workspace = Workspace::default();
    let id = workspace.open_request(None).unwrap();
    workspace.tabs[0]
        .draft
        .raw
        .extend_from_slice("中文🙂".as_bytes());
    let original = workspace.tabs[0].clone();
    workspace.orientation = Orientation::Vertical;
    workspace.closed.push(workspace.tabs.remove(0));
    workspace.active = None;
    workspace.validate().unwrap();
    workspace.tabs.push(workspace.closed.pop().unwrap());
    workspace.active = Some(id);
    workspace.validate().unwrap();
    assert_eq!(workspace.tabs[0], original);
}

#[test]
fn stale_or_foreign_workspaces_are_rejected() {
    let mut workspace = Workspace::default();
    workspace.open_request(None).unwrap();
    workspace.tabs.push(workspace.tabs[0].clone());
    assert!(workspace.validate().is_err());
    workspace.tabs.pop();
    workspace.tabs[0].group = Some(Uuid::new_v4());
    assert!(workspace.validate().is_err());
}

#[test]
fn proxy_listeners_enforce_count_identity_loopback_and_enabled_port_uniqueness() {
    let listener = |id: u128, enabled: bool, addr: &str| crate::ProxyListenerConfig {
        id: Uuid::from_u128(id),
        enabled,
        bind_address: addr.into(),
        ..crate::ProxyListenerConfig::default()
    };
    let mut workspace = Workspace {
        proxy_listeners: vec![
            listener(1, true, "127.0.0.1:8080"),
            listener(2, true, "[::1]:8080"),
            listener(3, false, "127.0.0.1:8080"),
            listener(4, true, "127.0.0.1:0"),
            listener(5, true, "127.0.0.1:0"),
        ],
        ..Workspace::default()
    };
    workspace.validate_structure().unwrap();
    workspace.validate().unwrap();

    for invalid in [
        vec![listener(0, true, "127.0.0.1:8080")],
        vec![
            listener(1, true, "127.0.0.1:8080"),
            listener(1, true, "127.0.0.1:8081"),
        ],
        vec![listener(1, true, "")],
        vec![listener(1, true, "localhost:8080")],
        vec![listener(1, false, "0.0.0.0:8080")],
        vec![listener(1, true, "192.168.1.10:8080")],
        vec![
            listener(1, true, "127.0.0.1:8080"),
            listener(2, true, "127.0.0.1:8080"),
        ],
        vec![
            listener(1, true, "[::1]:8443"),
            listener(2, true, "[0:0:0:0:0:0:0:1]:8443"),
        ],
        (1..=33).map(|i| listener(i, true, "127.0.0.1:0")).collect(),
    ] {
        workspace.proxy_listeners = invalid;
        assert!(workspace.validate_structure().is_err());
        assert!(workspace.validate().is_err());
    }
}

#[test]
fn client_certificates_validate_bounds_and_pem_identity_without_blocking_pkcs12() {
    let pem_block = |label: &str, der: &[u8]| -> Vec<u8> {
        let b64 = crate::intruder::apply_payload_processing_rule(
            &[der.to_vec()],
            &crate::intruder::PayloadProcessingRule::Base64Encode,
        )
        .unwrap();
        format!(
            "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
            std::str::from_utf8(&b64[0]).unwrap()
        )
        .into_bytes()
    };
    let auth_a = mio_proxy::MitmAuthority::generate().unwrap();
    let auth_b = mio_proxy::MitmAuthority::generate().unwrap();
    let cert_pem = pem_block("CERTIFICATE", &auth_a.certificate_der());
    let key_pem = pem_block("PRIVATE KEY", &auth_a.private_key_der());
    let wrong_key_pem = pem_block("PRIVATE KEY", &auth_b.private_key_der());
    let mut combined_pem = cert_pem.clone();
    combined_pem.extend_from_slice(&key_pem);

    let pem_rule = |host: &str, cert: Vec<u8>, key: Vec<u8>| mio_proxy::ClientCertificateRule {
        enabled: true,
        host_pattern: host.into(),
        format: mio_proxy::ClientCertificateFormat::Pem,
        certificate_bytes: cert,
        private_key_pem: key,
        password: String::new(),
    };
    let p12_rule = mio_proxy::ClientCertificateRule {
        enabled: true,
        host_pattern: "*.example.com".into(),
        format: mio_proxy::ClientCertificateFormat::Pkcs12,
        certificate_bytes: b"unparsed-pkcs12-in-sync-validate".to_vec(),
        private_key_pem: Vec::new(),
        password: "secret".into(),
    };

    let mut workspace = Workspace {
        client_certificates: vec![
            pem_rule("api.example.com", cert_pem.clone(), key_pem.clone()),
            pem_rule("127.0.0.1", combined_pem, Vec::new()),
            p12_rule.clone(),
        ],
        ..Workspace::default()
    };
    workspace.validate_structure().unwrap();
    workspace.validate().unwrap();

    for bad_pem in [
        pem_rule("api.example.com", cert_pem.clone(), wrong_key_pem),
        pem_rule("api.example.com", b"not-pem".to_vec(), Vec::new()),
    ] {
        workspace.client_certificates = vec![bad_pem];
        workspace.validate_structure().unwrap();
        assert!(workspace.validate().is_err());
    }

    let mut bad_pw_newline = p12_rule.clone();
    bad_pw_newline.password = "sec\nret".into();
    let mut bad_pw_nul = p12_rule.clone();
    bad_pw_nul.password = "sec\0ret".into();
    let mut bad_pw_len = p12_rule.clone();
    bad_pw_len.password = "a".repeat(mio_proxy::MAX_CLIENT_CERT_PASSWORD_BYTES + 1);

    for invalid in [
        vec![pem_rule("", cert_pem.clone(), key_pem.clone())],
        vec![pem_rule(
            "https://example.com",
            cert_pem.clone(),
            key_pem.clone(),
        )],
        vec![pem_rule(
            "example.com:443",
            cert_pem.clone(),
            key_pem.clone(),
        )],
        vec![pem_rule("api.example.com", Vec::new(), key_pem.clone())],
        vec![pem_rule(
            "api.example.com",
            vec![b'x'; mio_proxy::MAX_CLIENT_CERT_BYTES + 1],
            Vec::new(),
        )],
        vec![pem_rule(
            "api.example.com",
            cert_pem.clone(),
            vec![b'x'; mio_proxy::MAX_CLIENT_CERT_BYTES + 1],
        )],
        vec![bad_pw_newline],
        vec![bad_pw_nul],
        vec![bad_pw_len],
        (0..65).map(|_| p12_rule.clone()).collect(),
    ] {
        workspace.client_certificates = invalid;
        assert!(workspace.validate_structure().is_err());
        assert!(workspace.validate().is_err());
    }
}

#[test]
fn scope_rule_include_subdomains_and_should_drop_url_behave_accurately() {
    let exact_rule = ScopeRule {
        id: Uuid::new_v4(),
        enabled: true,
        prefix: "https://example.com".into(),
        include_subdomains: false,
    };
    assert!(exact_rule.matches_url("https://example.com/a"));
    assert!(!exact_rule.matches_url("https://sub.example.com/a"));

    let sub_rule = ScopeRule {
        id: Uuid::new_v4(),
        enabled: true,
        prefix: "https://example.com/api".into(),
        include_subdomains: true,
    };
    assert!(sub_rule.matches_url("https://example.com/api/v1"));
    assert!(sub_rule.matches_url("https://sub.example.com/api/v1"));
    assert!(sub_rule.matches_url("https://a.b.example.com:443/api/v1"));
    assert!(!sub_rule.matches_url("https://notexample.com/api/v1"));
    assert!(!sub_rule.matches_url("http://sub.example.com/api/v1"));
    assert!(!sub_rule.matches_url("https://sub.example.com:8443/api/v1"));
    assert!(!sub_rule.matches_url("https://sub.example.com/other"));

    let mut scope = TargetScope {
        include: vec![ScopeRule {
            id: Uuid::new_v4(),
            enabled: true,
            prefix: "https://example.com".into(),
            include_subdomains: true,
        }],
        exclude: vec![ScopeRule {
            id: Uuid::new_v4(),
            enabled: true,
            prefix: "https://admin.example.com".into(),
            include_subdomains: false,
        }],
        ..TargetScope::default()
    };
    assert!(scope.is_url_in_scope("https://app.example.com/home"));
    assert!(!scope.is_url_in_scope("https://admin.example.com/home"));
    assert!(!scope.should_drop_url("https://other.org/"));

    scope.drop_out_of_scope = true;
    assert!(!scope.should_drop_url("https://app.example.com/home"));
    assert!(scope.should_drop_url("https://admin.example.com/home"));
    assert!(scope.should_drop_url("https://other.org/"));

    scope.out_of_scope_use_custom = true;
    scope.out_of_scope_custom_rules = vec![ScopeRule {
        id: Uuid::new_v4(),
        enabled: true,
        prefix: "https://trusted.org".into(),
        include_subdomains: true,
    }];
    scope.validate().unwrap();
    assert!(!scope.should_drop_url("https://cdn.trusted.org/a.js"));
    assert!(scope.should_drop_url("https://app.example.com/home"));
}

#[test]
fn socks_bypass_hosts_routes_matching_hosts_directly() {
    let settings = UpstreamProxySettings {
        socks: SocksProxySettings {
            use_proxy: true,
            host: "127.0.0.1".into(),
            port: 1080,
            bypass_hosts: vec!["localhost".into(), "*.internal.local".into()],
            override_project_only: true,
            ..SocksProxySettings::default()
        },
        ..UpstreamProxySettings::default()
    };
    settings.validate().unwrap();
    assert!(matches!(
        settings
            .route_for_host_with_tls(Some("localhost"), mio_proxy::UpstreamTls::System)
            .unwrap(),
        mio_proxy::UpstreamRoute::Direct
    ));
    assert!(matches!(
        settings
            .route_for_host_with_tls(Some("api.internal.local"), mio_proxy::UpstreamTls::System)
            .unwrap(),
        mio_proxy::UpstreamRoute::Direct
    ));
    assert!(matches!(
        settings
            .route_for_host_with_tls(Some("example.com"), mio_proxy::UpstreamTls::System)
            .unwrap(),
        mio_proxy::UpstreamRoute::Socks5(_)
            | mio_proxy::UpstreamRoute::Socks5WithBypass {
                local_dns: false,
                ..
            }
    ));
    let resolver = settings
        .build_route_resolver(mio_proxy::UpstreamTls::System)
        .unwrap();
    assert!(matches!(
        resolver("api.internal.local"),
        mio_proxy::UpstreamRoute::Direct
    ));
    assert!(matches!(
        resolver("example.com"),
        mio_proxy::UpstreamRoute::Socks5(_)
            | mio_proxy::UpstreamRoute::Socks5WithBypass {
                local_dns: false,
                ..
            }
    ));
}
