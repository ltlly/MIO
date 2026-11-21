use super::*;

#[test]
fn proxy_intercept_response_parameter_relationships_use_the_real_request_even_for_sse_headers() {
    let cancel = CancellationToken::new();
    let limits = (std::time::Instant::now() + Duration::from_secs(3), MAX_BODY);
    use crate::ProxyInterceptOperator::{And, Or};
    use crate::ProxyInterceptRelation::{ContainsParameters, DoesNotContainParameters, Matches};
    use crate::ProxyInterceptType::{Request, StatusCode};

    let mut settings = crate::ProxyInterceptionSettings::default();
    settings.response.enabled = true;
    settings.response.rules = vec![interception_rule(100, And, Request, ContainsParameters, "")];
    let filter = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    let mut record = interception_record();
    record.request.headers = vec![Header {
        name: "Content-Type".into(),
        value: b"application/json".to_vec(),
    }];
    record.request.body = br#"{"request":"value"}"#.to_vec();
    record.response.as_mut().unwrap().body.clear();
    let unchanged = record.clone();
    assert!(
        filter(
            &record,
            true,
            true,
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
    record.request.body.clear();
    record.response.as_mut().unwrap().body = b"response=value".to_vec();
    assert!(
        !filter(
            &record,
            true,
            false,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept(),
        "response payload cannot substitute for request parameters"
    );
    settings.response.rules[0].relationship = DoesNotContainParameters;
    let inverted = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        inverted(
            &record,
            true,
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
    record
        .request
        .body
        .resize(crate::intruder::MAX_AUTO_BYTES + 1, b'x');
    assert!(
        inverted(
            &record,
            true,
            true,
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
    settings
        .response
        .rules
        .insert(0, interception_rule(101, And, StatusCode, Matches, "^200$"));
    settings.response.rules[1].operator = Or;
    let short_circuited =
        build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        short_circuited(
            &record,
            true,
            true,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept()
    );
    settings.response.enabled = false;
    let off = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        !off(
            &record,
            true,
            true,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept()
    );
    settings.response.enabled = true;
    settings.response.rules.remove(0);
    record.request.body.clear();
    record.request.url = "http://example.com/?request=value".into();
    record.response = None;
    settings.response.rules[0].relationship = ContainsParameters;
    let no_response = build_intercept_filter(settings, crate::TargetScope::default()).unwrap();
    assert!(
        no_response(
            &record,
            true,
            true,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .is_err(),
        "request parameters alone cannot fabricate a response interception stage"
    );
}

#[test]
fn proxy_intercept_header_regex_keeps_duplicate_binary_values_and_response_fields_without_body() {
    let cancel = CancellationToken::new();
    let limits = (std::time::Instant::now() + Duration::from_secs(3), MAX_BODY);
    use crate::ProxyInterceptOperator::{And, Or};
    use crate::ProxyInterceptRelation::{DoesNotMatch, Matches, WasIntercepted, WasModified};
    use crate::ProxyInterceptType::{ContentTypeHeader, Header as HeaderType, Request, StatusCode};
    let mut settings = crate::ProxyInterceptionSettings::default();
    settings.request.rules = vec![interception_rule(
        1,
        And,
        HeaderType,
        Matches,
        r"^X-Test: two$",
    )];
    let filter = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    let mut record = interception_record();
    record.request.headers = vec![
        Header {
            name: "X-Test".into(),
            value: b"one".to_vec(),
        },
        Header {
            name: "X-Test".into(),
            value: b"two".to_vec(),
        },
        Header {
            name: "X-Binary".into(),
            value: vec![255],
        },
        Header {
            name: "X-Binary".into(),
            value: b"kept".to_vec(),
        },
    ];
    record.response.as_mut().unwrap().headers = record.request.headers.clone();
    record.response.as_mut().unwrap().headers.push(Header {
        name: "Content-Type".into(),
        value: b"text/plain".to_vec(),
    });
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
    settings.response.enabled = true;
    settings.response.rules = vec![interception_rule(
        2,
        And,
        HeaderType,
        Matches,
        "^X-Test: two$",
    )];
    let both = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        both(
            &record,
            true,
            true,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept()
    );
    for pattern in [r"^x-binary: \xFF$", "^X-Binary: kept$"] {
        settings.request.rules[0].condition = pattern.into();
        settings.response.rules[0].condition = pattern.into();
        let bytes =
            build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
        assert!(
            bytes(
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
        assert!(
            bytes(
                &record,
                true,
                true,
                false,
                false,
                eval_port(18080),
                &cancel,
                limits
            )
            .unwrap()
            .should_intercept()
        );
    }
    assert_eq!(record, unchanged);
    settings.request.rules[0].condition = "^X-Test: two$".into();
    settings.response.rules[0].condition = "^X-Test: two$".into();
    settings.request.rules[0].relationship = DoesNotMatch;
    settings.response.rules[0].relationship = DoesNotMatch;
    let negative = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        !negative(
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
    assert!(
        !negative(
            &record,
            true,
            true,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept()
    );
    settings.request.rules[0].condition = "^X-Test: missing$".into();
    settings.response.rules[0].condition = "^X-Test: missing$".into();
    let absent = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        absent(
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
    assert!(
        absent(
            &record,
            true,
            true,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept()
    );
    for malformed_tail in [
        Header {
            name: "Invalid Header".into(),
            value: b"bad\r\nvalue".to_vec(),
        },
        Header {
            name: "X-Bad".into(),
            value: b"bad\x7fvalue".to_vec(),
        },
    ] {
        record = unchanged.clone();
        record.request.headers.push(malformed_tail.clone());
        record
            .response
            .as_mut()
            .unwrap()
            .headers
            .push(malformed_tail);
        for matcher in [&both, &negative, &absent] {
            assert!(
                matches!(
                    matcher(
                        &record,
                        false,
                        false,
                        false,
                        false,
                        eval_port(18080),
                        &cancel,
                        limits
                    ),
                    Err(mio_proxy::ProxyError::InvalidMessage(_))
                ),
                "matching or negating an earlier header cannot conceal a malformed trailing header"
            );
            assert!(matches!(
                matcher(
                    &record,
                    true,
                    true,
                    false,
                    false,
                    eval_port(18080),
                    &cancel,
                    limits
                ),
                Err(mio_proxy::ProxyError::InvalidMessage(_))
            ));
        }
    }
    record = unchanged;
    settings.request.rules[0].relationship = Matches;
    settings.response.rules[0].relationship = Matches;
    settings.request.rules[0].condition = "x-label: 中文".into();
    record.request.headers.push(Header {
        name: "X-Label".into(),
        value: "中文".as_bytes().to_vec(),
    });
    let chinese = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        chinese(
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
    settings.response.enabled = true;
    settings.response.rules = vec![
        interception_rule(2, And, ContentTypeHeader, Matches, "^text/plain$"),
        interception_rule(3, And, StatusCode, Matches, "^200$"),
    ];
    let filter = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        filter(
            &record,
            true,
            true,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept(),
        "SSE/header-only matching uses real header/status without pretending its body is available"
    );
    record.response = None;
    assert!(
        filter(
            &record,
            true,
            true,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .is_err()
    );
    settings.response.rules = vec![
        interception_rule(4, And, Request, WasModified, ""),
        interception_rule(5, Or, Request, WasIntercepted, ""),
    ];
    let filter = build_intercept_filter(settings, crate::TargetScope::default()).unwrap();
    record.response = interception_record().response;
    record.summary.state = crate::FlowState::Intercepted;
    assert!(
        !filter(
            &record,
            true,
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
    assert!(
        filter(
            &record,
            true,
            false,
            true,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept()
    );
    assert!(
        filter(
            &record,
            true,
            false,
            false,
            true,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap()
        .should_intercept()
    );
}
