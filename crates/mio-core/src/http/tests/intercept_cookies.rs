use super::*;

#[test]
fn proxy_intercept_protocol_uses_target_scheme_and_listener_rule_uses_bound_port_only() {
    let cancel = CancellationToken::new();
    let limits = (std::time::Instant::now() + Duration::from_secs(3), MAX_BODY);
    use crate::ProxyInterceptOperator::{And, Or};
    use crate::ProxyInterceptRelation::{DoesNotMatch, IsHttp, IsHttps, Matches};
    use crate::ProxyInterceptType::{HttpMethod, ListenerPort, Protocol};

    let mut settings = crate::ProxyInterceptionSettings::default();
    settings.request.rules = vec![interception_rule(1, And, Protocol, IsHttps, "")];
    settings.response.enabled = true;
    settings.response.rules = vec![interception_rule(2, And, Protocol, IsHttp, "")];
    let protocol = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    let mut record = interception_record();
    for (url, version, secure) in [
        (
            "http://example.com:443/a/../path?q=%2f",
            HttpVersion::Http2,
            false,
        ),
        (
            "https://example.com:80/a/../path?q=%2F",
            HttpVersion::Http11,
            true,
        ),
        ("HTTP://example.com/fixture", HttpVersion::Http10, false),
        ("HTTPS://example.com/fixture", HttpVersion::Http2, true),
        ("ws://example.com/upgrade", HttpVersion::Http11, false),
        ("wss://example.com/upgrade", HttpVersion::Http11, true),
    ] {
        record.request.url = url.into();
        record.request.version = version;
        record.request.body = vec![0, 255];
        let unchanged = record.clone();
        assert_eq!(
            protocol(
                &record,
                false,
                false,
                false,
                false,
                eval_port(49153),
                &cancel,
                limits
            )
            .unwrap(),
            secure
        );
        assert_eq!(
            protocol(
                &record,
                true,
                true,
                false,
                false,
                eval_port(49153),
                &cancel,
                limits
            )
            .unwrap(),
            !secure
        );
        assert_eq!(record, unchanged);
    }
    for invalid in ["/no-scheme", "ftp://example.com/fixture"] {
        record.request.url = invalid.into();
        assert!(
            protocol(
                &record,
                false,
                false,
                false,
                false,
                eval_port(49153),
                &cancel,
                limits
            )
            .is_err()
        );
        assert!(
            protocol(
                &record,
                true,
                true,
                false,
                false,
                eval_port(49153),
                &cancel,
                limits
            )
            .is_err()
        );
    }
    settings.request.rules[0].relationship = Matches;
    assert!(validate_proxy_interception_shape(&settings).is_err());
    settings.request.rules[0].match_type = HttpMethod;
    settings.request.rules[0].relationship = IsHttp;
    assert!(validate_proxy_interception_shape(&settings).is_err());
    settings.request.rules = vec![interception_rule(1, And, ListenerPort, Matches, "^49153$")];
    settings.response.rules = vec![interception_rule(
        2,
        And,
        ListenerPort,
        DoesNotMatch,
        "^49153$",
    )];
    record.request.url = "https://example.com:18080/target".into();
    record.request.headers = vec![Header {
        name: "Host".into(),
        value: b"host.invalid:49154".to_vec(),
    }];
    let port = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        port(
            &record,
            false,
            false,
            false,
            false,
            eval_port(49153),
            &cancel,
            limits
        )
        .unwrap()
    );
    assert!(
        !port(
            &record,
            true,
            true,
            false,
            false,
            eval_port(49153),
            &cancel,
            limits
        )
        .unwrap()
    );
    assert!(
        !port(
            &record,
            false,
            false,
            false,
            false,
            eval_port(49154),
            &cancel,
            limits
        )
        .unwrap()
    );
    assert!(
        port(
            &record,
            true,
            true,
            false,
            false,
            eval_port(49154),
            &cancel,
            limits
        )
        .unwrap()
    );
    assert!(
        port(
            &record,
            false,
            false,
            false,
            false,
            eval_port(0),
            &cancel,
            limits
        )
        .unwrap_err()
        .to_string()
        .contains("尚未可用")
    );
    assert!(
        port(
            &record,
            true,
            true,
            false,
            false,
            eval_port(0),
            &cancel,
            limits
        )
        .is_err()
    );
    settings
        .request
        .rules
        .insert(0, interception_rule(3, And, HttpMethod, Matches, "get"));
    settings.request.rules[1].operator = Or;
    let short_circuit =
        build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        short_circuit(
            &record,
            false,
            false,
            false,
            false,
            eval_port(0),
            &cancel,
            limits
        )
        .unwrap()
    );
    settings.request.enabled = false;
    settings.response.enabled = false;
    let off = build_intercept_filter(settings, crate::TargetScope::default()).unwrap();
    assert!(
        !off(
            &record,
            false,
            false,
            false,
            false,
            eval_port(0),
            &cancel,
            limits
        )
        .unwrap()
    );
    assert!(
        !off(
            &record,
            true,
            true,
            false,
            false,
            eval_port(0),
            &cancel,
            limits
        )
        .unwrap()
    );
}

#[test]
fn proxy_intercept_cookie_sides_preserve_quoted_binary_duplicates_and_validate_all_pairs() {
    use crate::ProxyInterceptOperator::And;
    use crate::ProxyInterceptRelation::{
        Matches, RequestDoesNotMatch, RequestMatches, ResponseDoesNotMatch, ResponseMatches,
    };
    use crate::ProxyInterceptType::{CookieName, CookieValue};
    let cancel = CancellationToken::new();
    let limits = (std::time::Instant::now() + Duration::from_secs(3), MAX_BODY);
    let mut record = interception_record();
    record.request.headers = vec![
        Header {
            name: "Cookie".into(),
            value: b"x=one; x=\"two;three\"; empty=; bare".to_vec(),
        },
        Header {
            name: "Cookie".into(),
            value: b"binary=\xff; encoded=%ZZ".to_vec(),
        },
    ];
    record.response.as_mut().unwrap().headers = vec![
        Header {
            name: "Set-Cookie".into(),
            value: b"x=first; Path=/; SameSite=Lax".to_vec(),
        },
        Header {
            name: "Set-Cookie".into(),
            value: b"x=second; Secure; HttpOnly".to_vec(),
        },
    ];
    let unchanged = record.clone();
    for (match_type, condition) in [
        (CookieName, "^x$"),
        (CookieValue, r#"^"two$"#),
        (CookieValue, r"^\xFF$"),
        (CookieValue, "^%ZZ$"),
    ] {
        let mut settings = crate::ProxyInterceptionSettings::default();
        settings.request.rules = vec![interception_rule(1, And, match_type, Matches, condition)];
        settings.response.enabled = true;
        settings.response.rules = vec![interception_rule(
            2,
            And,
            match_type,
            RequestMatches,
            condition,
        )];
        let filter = build_intercept_filter(settings, crate::TargetScope::default()).unwrap();
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
        );
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
        );
    }
    let mut settings = crate::ProxyInterceptionSettings::default();
    settings.response.enabled = true;
    settings.response.rules = vec![interception_rule(
        2,
        And,
        CookieValue,
        ResponseMatches,
        "^second$",
    )];
    let response = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        response(
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
    );
    settings.response.rules[0].relationship = ResponseDoesNotMatch;
    let negated = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        !negated(
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
    );
    settings.response.rules[0].match_type = CookieName;
    settings.response.rules[0].relationship = ResponseMatches;
    settings.response.rules[0].condition = "^Path$".into();
    let attributes =
        build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        !attributes(
            &record,
            true,
            true,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        )
        .unwrap(),
        "Set-Cookie attributes are not additional response cookies"
    );
    settings.response.rules[0].relationship = RequestDoesNotMatch;
    settings.response.rules[0].condition = "^x$".into();
    let no_request =
        build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    assert!(
        !no_request(
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
    );
    assert_eq!(record, unchanged);
    record.request.headers[0].value = (0..257)
        .map(|index| format!("x={index}"))
        .collect::<Vec<_>>()
        .join(";")
        .into_bytes();
    assert!(
        no_request(
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
        .contains("256")
    );
    for malformed_tail in [
        Header {
            name: "Bad Header".into(),
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
        for matcher in [&response, &negated, &no_request] {
            assert!(
                matches!(
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
                ),
                "a matching or negated earlier cookie cannot conceal a bad trailing header"
            );
        }
    }
    settings.response.rules[0].relationship = Matches;
    assert!(validate_proxy_interception_shape(&settings).is_err());
}
