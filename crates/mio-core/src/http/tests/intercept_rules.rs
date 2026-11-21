use super::*;

#[test]
fn proxy_intercept_default_rules_preserve_static_extension_and_scope_semantics() {
    let cancel = CancellationToken::new();
    let limits = (std::time::Instant::now() + Duration::from_secs(3), MAX_BODY);
    let defaults = crate::ProxyInterceptionSettings::default();
    assert_eq!(defaults.request.rules.len(), 4);
    assert_eq!(defaults.response.rules.len(), 5);
    assert!(defaults.request.enabled);
    assert!(!defaults.response.enabled);
    let filter = build_intercept_filter(defaults.clone(), crate::TargetScope::default()).unwrap();
    let mut record = interception_record();
    for (url, expected) in [
        ("http://example.com/image.gif?next=/keep", false),
        ("http://example.com/image.GIF#fragment", false),
        ("http://example.com/dir.gif/", true),
        ("http://example.com/dir.gif/index", true),
        ("http://example.com/path?file=image.gif", true),
        ("http://example.gif", true),
        ("http://example.com/a/../file.gif", false),
        ("http://example.com/%66ile.%67if", true),
    ] {
        record.request.url = url.into();
        assert_eq!(
            filter(
                &record,
                false,
                false,
                false,
                false,
                eval_port(18080),
                &cancel,
                limits
            )
            .unwrap()
            .should_intercept(),
            expected,
            "{url}"
        );
    }
    assert!(
        !filter(
            &record,
            true,
            true,
            true,
            true,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept()
    );
    let mut scoped = defaults;
    scoped.request.rules[3].enabled = true;
    let scope = crate::TargetScope {
        include: vec![crate::ScopeRule {
            id: crate::Uuid::new_v4(),
            enabled: true,
            prefix: "http://example.com".into(),
            include_subdomains: false,
        }],
        exclude: vec![crate::ScopeRule {
            id: crate::Uuid::new_v4(),
            enabled: true,
            prefix: "http://example.com/private".into(),
            include_subdomains: false,
        }],
        ..Default::default()
    };
    let filter = build_intercept_filter(scoped, scope).unwrap();
    for (url, expected) in [
        ("http://example.com/api", true),
        ("http://other.invalid/api", false),
        ("http://example.com/private/item", false),
        ("http://example.com/image.gif", false),
    ] {
        record.request.url = url.into();
        assert_eq!(
            filter(
                &record,
                false,
                false,
                false,
                false,
                eval_port(18080),
                &cancel,
                limits
            )
            .unwrap()
            .should_intercept(),
            expected
        );
    }
}

#[test]
fn proxy_intercept_rules_left_fold_disabled_and_short_circuit_without_skipping_later_rows() {
    let cancel = CancellationToken::new();
    let limits = (std::time::Instant::now() + Duration::from_secs(3), MAX_BODY);
    use crate::ProxyInterceptOperator::{And, Or};
    use crate::ProxyInterceptRelation::{ContainsParameters, Matches};
    use crate::ProxyInterceptType::{HttpMethod, Request};
    let mut settings = crate::ProxyInterceptionSettings::default();
    settings.request.rules = vec![
        interception_rule(1, Or, HttpMethod, Matches, "get"),
        interception_rule(2, Or, HttpMethod, Matches, "post"),
        interception_rule(3, And, HttpMethod, Matches, "put"),
    ];
    let record = interception_record();
    let filter = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        !filter(
            &record,
            false,
            false,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept(),
        "left fold is (true OR false) AND false, not true OR (false AND false)"
    );
    settings.request.rules[2].enabled = false;
    let filter = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        filter(
            &record,
            false,
            false,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept()
    );
    for rule in &mut settings.request.rules {
        rule.enabled = false;
    }
    let filter = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        filter(
            &record,
            false,
            false,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept(),
        "enabled group with no enabled rows intercepts all"
    );
    settings.request.rules.clear();
    let filter = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        filter(
            &record,
            false,
            false,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept()
    );
    settings.request.enabled = false;
    let filter = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        !filter(
            &record,
            false,
            false,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept()
    );
    settings.request.enabled = true;
    settings.request.rules = vec![
        interception_rule(1, And, HttpMethod, Matches, "get"),
        interception_rule(2, Or, Request, ContainsParameters, ""),
        interception_rule(3, And, HttpMethod, Matches, "post"),
    ];
    let mut oversized = record;
    oversized
        .request
        .body
        .resize(crate::intruder::MAX_AUTO_BYTES + 1, b'x');
    let filter = build_intercept_filter(settings, crate::TargetScope::default()).unwrap();
    assert!(
        !filter(
            &oversized,
            false,
            false,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept(),
        "skipping unnecessary parameter parsing must still evaluate the final AND"
    );
}

#[test]
fn proxy_intercept_validation_rejects_bad_disabled_rules_and_wrong_stage_without_dropping_them() {
    let defaults = crate::ProxyInterceptionSettings::default();
    let original = defaults.clone();
    for invalid in ["(", "(?<=a)b", r"(a)\1"] {
        let mut settings = defaults.clone();
        settings.request.rules[0].condition = invalid.into();
        settings.request.rules[0].enabled = false;
        validate_proxy_interception_shape(&settings).unwrap();
        assert!(
            validate_proxy_interception(&settings)
                .unwrap_err()
                .to_string()
                .contains("Java lookaround/backreference")
        );
    }
    let mut settings = defaults.clone();
    settings.request.rules[0].id = crate::Uuid::nil();
    assert!(validate_proxy_interception(&settings).is_err());
    settings = defaults.clone();
    settings.request.rules[0].id = settings.response.rules[0].id;
    assert!(validate_proxy_interception(&settings).is_err());
    settings = defaults.clone();
    settings.request.rules[0].condition = "a".repeat(4097);
    assert!(validate_proxy_interception(&settings).is_err());
    settings = defaults.clone();
    settings.request.rules[0].match_type = crate::ProxyInterceptType::StatusCode;
    assert!(validate_proxy_interception(&settings).is_err());
    settings = defaults.clone();
    settings.response.rules[1].match_type = crate::ProxyInterceptType::Header;
    settings.response.rules[1].relationship = crate::ProxyInterceptRelation::ContainsParameters;
    assert!(validate_proxy_interception(&settings).is_err());
    settings = defaults.clone();
    settings.request.rules[1].relationship = crate::ProxyInterceptRelation::WasModified;
    assert!(validate_proxy_interception(&settings).is_err());
    settings = defaults.clone();
    settings.request.rules = (0..257)
        .map(|id| {
            interception_rule(
                100 + id,
                crate::ProxyInterceptOperator::And,
                crate::ProxyInterceptType::HttpMethod,
                crate::ProxyInterceptRelation::Matches,
                "get",
            )
        })
        .collect();
    assert!(validate_proxy_interception(&settings).is_err());
    settings = defaults.clone();
    settings.request.rules = (0..32)
        .map(|id| {
            interception_rule(
                100 + id,
                crate::ProxyInterceptOperator::And,
                crate::ProxyInterceptType::HttpMethod,
                crate::ProxyInterceptRelation::Matches,
                &"a".repeat(4096),
            )
        })
        .collect();
    settings.response.rules.clear();
    validate_proxy_interception_shape(&settings).unwrap();
    settings.response.rules.push(interception_rule(
        500,
        crate::ProxyInterceptOperator::Or,
        crate::ProxyInterceptType::StatusCode,
        crate::ProxyInterceptRelation::Matches,
        "a",
    ));
    settings.response.rules[0].enabled = false;
    assert!(
        validate_proxy_interception_shape(&settings)
            .unwrap_err()
            .to_string()
            .contains("128 KiB")
    );
    assert!(
        validate_proxy_interception(&settings)
            .unwrap_err()
            .to_string()
            .contains("128 KiB")
    );
    assert_eq!(defaults, original);
}

#[test]
fn proxy_intercept_parameter_rules_use_original_data_not_cookies_or_transfer_framing() {
    let cancel = CancellationToken::new();
    let limits = (std::time::Instant::now() + Duration::from_secs(3), MAX_BODY);
    use crate::ProxyInterceptOperator::And;
    use crate::ProxyInterceptRelation::{ContainsParameters, DoesNotContainParameters};
    use crate::ProxyInterceptType::Request;
    let mut settings = crate::ProxyInterceptionSettings::default();
    settings.request.rules = vec![interception_rule(1, And, Request, ContainsParameters, "")];
    let filter = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    let mut record = interception_record();
    record.request.headers = vec![Header {
        name: "Cookie".into(),
        value: b"name=value".to_vec(),
    }];
    assert!(
        !filter(
            &record,
            false,
            false,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept()
    );
    record.request.url = "http://example.com/?name=%ZZ".into();
    assert!(
        filter(
            &record,
            false,
            false,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept()
    );
    record.request.url = "http://example.com/".into();
    record.request.headers = vec![
        Header {
            name: "Content-Type".into(),
            value: b"application/json".to_vec(),
        },
        Header {
            name: "Transfer-Encoding".into(),
            value: b"chunked".to_vec(),
        },
    ];
    record.request.body = br#"{"name":"value"}"#.to_vec();
    let unchanged = record.clone();
    assert!(
        filter(
            &record,
            false,
            false,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept()
    );
    assert_eq!(record, unchanged);
    record.request.body = br#"{"name":}"#.to_vec();
    assert!(
        !filter(
            &record,
            false,
            false,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept()
    );
    record.request.body = (0..=crate::intruder::MAX_POSITIONS)
        .map(|index| format!("{index}=value"))
        .collect::<Vec<_>>()
        .join("&")
        .into_bytes();
    record.request.headers.clear();
    assert!(
        filter(
            &record,
            false,
            false,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap_err()
        .to_string()
        .contains("256")
    );
    settings.request.rules[0].relationship = DoesNotContainParameters;
    let inverted = build_intercept_filter(settings, crate::TargetScope::default()).unwrap();
    assert!(
        inverted(
            &record,
            false,
            false,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .is_err(),
        "negation cannot convert a parser limit into a matching condition"
    );
    record
        .request
        .body
        .resize(crate::intruder::MAX_AUTO_BYTES + 1, b'x');
    assert!(
        inverted(
            &record,
            false,
            false,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap_err()
        .to_string()
        .contains("4 MiB")
    );
}
