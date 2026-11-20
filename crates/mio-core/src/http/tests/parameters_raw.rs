use super::*;

#[test]
fn proxy_intercept_param_rules_sniff_valid_json_xml_without_counting_their_body_as_generic_pairs() {
    use crate::ProxyInterceptOperator::And;
    use crate::ProxyInterceptRelation::Matches;
    use crate::ProxyInterceptType::{ParamName, ParamValue};
    let cancel = CancellationToken::new();
    let limits = (std::time::Instant::now() + Duration::from_secs(3), MAX_BODY);
    for content_type in [
        None,
        Some(b"text/plain".as_slice()),
        Some(b"application/octet-stream".as_slice()),
        Some(b"application/x-www-form-urlencoded".as_slice()),
    ] {
        for body in [
            b"{\"key\":\"value\"}".as_slice(),
            b"<root><key>value</key></root>",
        ] {
            let mut record = interception_record();
            record.request.headers = content_type
                .into_iter()
                .map(|value| Header {
                    name: "Content-Type".into(),
                    value: value.to_vec(),
                })
                .collect();
            record.request.body = body.to_vec();
            let original = record.clone();
            for match_type in [ParamName, ParamValue] {
                let mut settings = crate::ProxyInterceptionSettings::default();
                settings.request.rules = vec![interception_rule(1, And, match_type, Matches, ".*")];
                settings.response.enabled = true;
                settings.response.rules =
                    vec![interception_rule(2, And, match_type, Matches, ".*")];
                let filter =
                    build_intercept_filter(settings, crate::TargetScope::default()).unwrap();
                for response_stage in [false, true] {
                    assert!(
                        !filter(
                            &record,
                            response_stage,
                            response_stage,
                            false,
                            false,
                            eval_port(18080),
                            &cancel,
                            limits,
                        )
                        .unwrap(),
                        "{content_type:?} valid structure must not become generic parameters"
                    );
                }
            }
            assert_eq!(record, original);
            record.request.url = "http://example.com/fixture?raw%20name=value+%20".into();
            let pairs = intercept_request_parameters(&record.request, &cancel, limits).unwrap();
            assert_eq!(pairs.len(), 1);
            assert_eq!(pairs[0].0.as_ref(), b"raw%20name");
            assert_eq!(pairs[0].1, b"value+%20");
            assert_eq!(record.request.body, original.request.body);
        }
    }
}

#[test]
fn proxy_intercept_param_structure_sniff_keeps_malformed_raw_pairs_and_propagates_guard_errors() {
    use std::cell::Cell;
    let cancel = CancellationToken::new();
    let limits = (std::time::Instant::now() + Duration::from_secs(3), MAX_BODY);
    for body in [
        b"name=value".as_slice(),
        b"bare-fragment",
        b"{broken",
        b"[broken",
        b"<broken",
        b"{broken\xFF=value\x00",
        b"",
    ] {
        let mut record = interception_record();
        record.request.url = "http://example.com/fixture?query%20=q+%20".into();
        record.request.body = body.to_vec();
        let original = record.clone();
        let pairs = intercept_request_parameters(&record.request, &cancel, limits).unwrap();
        assert_eq!(pairs.len(), 1 + usize::from(!body.is_empty()));
        assert_eq!(pairs[0].0.as_ref(), b"query%20");
        assert_eq!(pairs[0].1, b"q+%20");
        if !body.is_empty() {
            let (name, value) = body
                .iter()
                .position(|byte| *byte == b'=')
                .map_or((body, b"".as_slice()), |equal| {
                    (&body[..equal], &body[equal + 1..])
                });
            assert_eq!(pairs[1].0.as_ref(), name);
            assert_eq!(pairs[1].1, value);
        }
        assert_eq!(record, original);
    }
    for raw in [
        format!("{}0{}", "[".repeat(65), "]".repeat(65)),
        format!("[{}]", vec!["0"; 257].join(",")),
        format!("{}{}", "<n>".repeat(65), "</n>".repeat(65)),
    ] {
        let Err(error) = crate::intruder::intercept_body_is_structured(raw.as_bytes(), &|| Ok(()))
        else {
            panic!("参数结构嗅探不得把解析限额错误作为通用参数继续处理");
        };
        assert!(error.is::<crate::intruder::AutomaticParameterLimit>());
    }
    for raw in [
        b"{\"key\":\"value\"}".as_slice(),
        b"<root><key>value</key></root>",
    ] {
        for expected in [UnpackError::Cancelled, UnpackError::Deadline] {
            let calls = Cell::new(0);
            let error = crate::intruder::intercept_body_is_structured(raw, &|| {
                calls.set(calls.get() + 1);
                if calls.get() == 3 {
                    Err(expected.clone().into())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            assert_eq!(error.downcast_ref::<UnpackError>(), Some(&expected));
            assert_eq!(calls.get(), 3);
        }
    }
    let escaped = format!(r#"{{"kk":"{}\q"}}"#, "\\\\".repeat(8 * 1024));
    for expected in [UnpackError::Cancelled, UnpackError::Deadline] {
        let calls = Cell::new(0);
        let error = crate::intruder::intercept_body_is_structured(escaped.as_bytes(), &|| {
            calls.set(calls.get() + 1);
            if calls.get() >= 9 {
                Err(expected.clone().into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(error.downcast_ref::<UnpackError>(), Some(&expected));
        assert_eq!(calls.get(), 9);
    }
}

#[test]
fn raw_header_validation_rejects_del_but_preserves_opaque_bytes_and_tabs() {
    let mut request = b"GET /fixture HTTP/1.1\r\nX-Test: before\x7fafter\r\n\r\n".to_vec();
    let original = request.clone();
    assert!(parse_request(&request, "http://example.com", HttpVersion::Http11).is_err());
    assert_eq!(request, original);
    let response = b"HTTP/1.1 200 OK\r\nX-Test: before\x7fafter\r\n\r\n";
    assert!(parse_response(response, HttpVersion::Http11).is_err());
    request = b"GET /fixture HTTP/1.1\r\nX-Test: before\t\xffafter\r\n\r\n".to_vec();
    assert_eq!(
        parse_request(&request, "http://example.com", HttpVersion::Http11)
            .unwrap()
            .headers[0]
            .value,
        b"before\t\xffafter"
    );
    assert!(valid_header_value_bytes(b"before\t\xffafter"));
    assert!(!valid_header_value_bytes(b"before\x7fafter"));
}

#[test]
fn imported_origin_requires_a_host_and_explicit_port_without_url_components() {
    for target in [
        "http://127.0.0.1:49171",
        "https://localhost:443",
        "http://[::1]:80",
        "https://例子.invalid:8443",
    ] {
        validate_target_origin(target).unwrap();
    }
    for target in [
        "http://localhost",
        "http://localhost:0",
        "http://localhost:65536",
        "http://localhost:+80",
        "http://localhost:80/",
        "http://localhost:80?query",
        "http://localhost:80#fragment",
        "http://user@localhost:80",
        "http://:80",
        "http://::1:80",
        "http://localhost:\n80",
        "http://local host:80",
        "http://local\\host:80",
        "http://local%68ost:80",
        "ftp://localhost:80",
    ] {
        assert!(validate_target_origin(target).is_err(), "{target:?}");
    }
}

#[test]
fn imported_origin_rejects_numeric_hosts_that_the_sender_would_reinterpret() {
    for host in ["0177.0.0.1", "0x7f.0.0.1", "127.1", "2130706433"] {
        let target = format!("http://{host}:80");
        assert!(validate_target_origin(&target).is_err(), "{target}");
    }
}

#[test]
fn parse_preserves_origin_and_absolute_request_targets() {
    for (target, absolute_authority, authority) in [
        (
            "http://localhost:8912/base/ignored?also=ignored#fragment",
            "http://LOCALHOST:8912",
            "http://localhost:8912",
        ),
        (
            "https://localhost/base/ignored",
            "https://LOCALHOST:443",
            "https://localhost",
        ),
        (
            "http://[::1]:8912/base/ignored",
            "http://[0:0:0:0:0:0:0:1]:8912",
            "http://[::1]:8912",
        ),
    ] {
        for path in PATHS.iter().chain(DRAFT_ONLY_PATHS) {
            for request_target in [path.to_string(), format!("{absolute_authority}{path}")] {
                let raw = format!("GET {request_target} HTTP/1.1\r\n\r\n");
                let request = parse_request(raw.as_bytes(), target, HttpVersion::Http11).unwrap();
                assert_eq!(request.url, format!("{authority}{path}"));
            }
        }
    }
}

#[test]
fn raw_view_preserves_paths_queries_and_percent_encoding() {
    for path in PATHS.iter().chain(DRAFT_ONLY_PATHS) {
        let request = HttpRequest {
            method: "GET".into(),
            url: format!("http://localhost:8912{path}"),
            version: HttpVersion::Http11,
            headers: Vec::new(),
            body: Vec::new(),
            trailers: Vec::new(),
        };
        assert_eq!(
            request_to_raw(&request),
            format!("GET {path} HTTP/1.1\r\n\r\n").as_bytes(),
        );
    }
}

#[test]
fn absolute_target_without_path_retains_query_and_uses_selected_authority() {
    for (request_target, path) in [
        ("http://LOCALHOST:80", "/"),
        ("http://LOCALHOST:80?", "/?"),
        ("http://LOCALHOST:80?x=%2e%2E&x=1", "/?x=%2e%2E&x=1"),
    ] {
        let raw = format!("GET {request_target} HTTP/1.1\r\n\r\n");
        let request =
            parse_request(raw.as_bytes(), "http://localhost", HttpVersion::Http11).unwrap();
        assert_eq!(request.url, format!("http://localhost{path}"));
        assert_eq!(
            request_to_raw(&request),
            format!("GET {path} HTTP/1.1\r\n\r\n").as_bytes(),
        );
    }
}
