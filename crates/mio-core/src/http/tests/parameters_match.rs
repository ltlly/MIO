use super::*;

#[test]
fn proxy_intercept_param_name_and_value_match_query_form_multipart_and_exclude_json_xml_cookie() {
    use crate::ProxyInterceptOperator::And;
    use crate::ProxyInterceptRelation::{
        DoesNotMatch, Matches, RequestDoesNotMatch, RequestMatches, ResponseDoesNotMatch,
        ResponseMatches,
    };
    use crate::ProxyInterceptType::{ParamName, ParamValue};
    let cancel = CancellationToken::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let limits = (deadline, MAX_BODY);
    for match_type in [ParamName, ParamValue] {
        for relation in [
            RequestMatches,
            RequestDoesNotMatch,
            ResponseMatches,
            ResponseDoesNotMatch,
        ] {
            for response_stage in [false, true] {
                let mut invalid = crate::ProxyInterceptionSettings::default();
                let rules = if response_stage {
                    invalid.response.enabled = true;
                    &mut invalid.response.rules
                } else {
                    &mut invalid.request.rules
                };
                *rules = vec![interception_rule(1, And, match_type, relation, "^a$")];
                assert!(validate_proxy_interception_shape(&invalid).is_err());
            }
        }
    }

    let mut record = interception_record();
    record.request.url =
        "http://example.com/fixture?&&q%20k=v+1%202&bare&empty=&dup=1&dup=2#ignored=3".into();
    record.request.headers = vec![
        Header {
            name: "Cookie".into(),
            value: b"cookie_name=cookie_val".to_vec(),
        },
        Header {
            name: "Content-Type".into(),
            value: b"application/x-www-form-urlencoded".to_vec(),
        },
    ];
    record.request.body = b"&&f_key=f%2Bval&f_bare&f_empty=&bin_k=\x00\xff".to_vec();
    let assert_matches = |record: &FlowRecord, match_type, relation, condition, expected: bool| {
        let mut settings = crate::ProxyInterceptionSettings::default();
        settings.request.rules = vec![interception_rule(1, And, match_type, relation, condition)];
        settings.response.enabled = true;
        settings.response.rules = vec![interception_rule(2, And, match_type, relation, condition)];
        let filter = build_intercept_filter(settings, crate::TargetScope::default()).unwrap();
        assert_eq!(
            filter(
                record,
                false,
                false,
                false,
                false,
                eval_port(18080),
                &cancel,
                limits
            )
            .unwrap(),
            expected,
            "request {match_type:?} {relation:?} {condition:?}"
        );
        assert_eq!(
            filter(
                record,
                true,
                true,
                false,
                false,
                eval_port(18080),
                &cancel,
                limits
            )
            .unwrap(),
            expected,
            "response {match_type:?} {relation:?} {condition:?}"
        );
    };
    for (match_type, condition, expected) in [
        (ParamName, r"^q%20k$", true),
        (ParamName, r"^q k$", false),
        (ParamValue, r"^v\+1%202$", true),
        (ParamValue, r"^v 1 2$", false),
        (ParamName, r"^bare$", true),
        (ParamName, r"^empty$", true),
        (ParamValue, r"^$", true),
        (ParamValue, r"^2$", true),
        (ParamName, r"^ignored$", false),
        (ParamName, r"^cookie_name$", false),
        (ParamValue, r"^cookie_val$", false),
        (ParamName, r"^f_key$", true),
        (ParamValue, r"^f%2Bval$", true),
        (ParamName, r"^f_bare$", true),
        (ParamName, r"^f_empty$", true),
        (ParamName, r"^bin_k$", true),
        (ParamValue, r"^\x00\xFF$", true),
    ] {
        assert_matches(&record, match_type, Matches, condition, expected);
        assert_matches(&record, match_type, DoesNotMatch, condition, !expected);
    }

    for content_type in [
        Some(b"text/plain".as_slice()),
        Some(b"application/octet-stream"),
        None,
    ] {
        let mut generic = interception_record();
        generic.request.headers = content_type
            .into_iter()
            .map(|value| Header {
                name: "Content-Type".into(),
                value: value.to_vec(),
            })
            .collect();
        generic.request.body = b"plain_name=plain_val&raw_seg".to_vec();
        assert_matches(&generic, ParamName, Matches, r"^plain_name$", true);
        assert_matches(&generic, ParamValue, Matches, r"^plain_val$", true);
        assert_matches(&generic, ParamName, Matches, r"^raw_seg$", true);
        assert_matches(&generic, ParamValue, Matches, r"^$", true);
    }

    for (content_type, body) in [
        (
            b"application/json".as_slice(),
            b"{\"json_k\":\"json_v\",\"a\":\"1\"}".as_slice(),
        ),
        (b"application/problem+json; charset=utf-8", b"a=1&b=2"),
        (b"application/xml", b"<root attr=\"val\"><k>v</k></root>"),
        (b"text/xml", b"a=1&b=2"),
        (b"application/atom+xml", b"a=1&b=2"),
    ] {
        let mut structured = interception_record();
        structured.request.url = "http://example.com/fixture?url_q=url_v".into();
        structured.request.headers = vec![Header {
            name: "Content-Type".into(),
            value: content_type.to_vec(),
        }];
        structured.request.body = body.to_vec();
        assert_matches(&structured, ParamName, Matches, r"^url_q$", true);
        assert_matches(&structured, ParamValue, Matches, r"^url_v$", true);
        for excluded in [
            "^json_k$", "^json_v$", "^a$", "^1$", "^attr$", "^val$", "^k$", "^v$",
        ] {
            assert_matches(&structured, ParamName, Matches, excluded, false);
            assert_matches(&structured, ParamValue, Matches, excluded, false);
        }
    }

    let mut multipart = interception_record();
    multipart.request.headers = vec![Header {
        name: "Content-Type".into(),
        value: b"multipart/form-data; boundary=MioBound".to_vec(),
    }];
    multipart.request.body = b"--MioBound\r\nContent-Disposition: form-data; name=\"text_field\"\r\n\r\ntext_body\r\n--MioBound\r\nContent-Disposition: form-data; name=\"upload_field\"; filename=\" leak_name.txt \"\r\nContent-Type: application/octet-stream\r\n\r\n\x00\xfffile_body\r\n--MioBound--\r\n".to_vec();
    assert_matches(&multipart, ParamName, Matches, r"^text_field$", true);
    assert_matches(&multipart, ParamValue, Matches, r"^text_body$", true);
    assert_matches(&multipart, ParamName, Matches, r"^upload_field$", true);
    assert_matches(
        &multipart,
        ParamValue,
        Matches,
        r"^\x00\xFFfile_body$",
        true,
    );
    assert_matches(&multipart, ParamValue, Matches, r"leak_name", false);
}

#[test]
fn proxy_intercept_param_rules_propagate_malformed_limits_and_collaborate_with_request_unpack() {
    use crate::ProxyInterceptOperator::And;
    use crate::ProxyInterceptRelation::{DoesNotMatch, Matches};
    use crate::ProxyInterceptType::{ParamName, ParamValue};
    use std::io::Write;
    let cancel = CancellationToken::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let limits = (deadline, MAX_BODY);
    let mut settings = crate::ProxyInterceptionSettings::default();
    settings.request.rules = vec![interception_rule(
        1,
        And,
        ParamName,
        DoesNotMatch,
        "^never$",
    )];
    settings.response.enabled = true;
    settings.response.rules = vec![interception_rule(
        2,
        And,
        ParamValue,
        DoesNotMatch,
        "^never$",
    )];
    let negative = build_intercept_filter(settings, crate::TargetScope::default()).unwrap();

    for (headers, body) in [
        (
            vec![Header {
                name: "Content-Type".into(),
                value: b"multipart/form-data".to_vec(),
            }],
            b"--MioBound--\r\n".to_vec(),
        ),
        (
            vec![Header {
                name: "Content-Type".into(),
                value: b"multipart/form-data; boundary=MioBound".to_vec(),
            }],
            b"--MioBound\r\nContent-Disposition: form-data; name=\"x\"\r\n\r\nunterminated"
                .to_vec(),
        ),
        (
            vec![
                Header {
                    name: "Content-Type".into(),
                    value: b"multipart/form-data; boundary=MioBound".to_vec(),
                },
                Header {
                    name: "Content-Encoding".into(),
                    value: b"gzip".to_vec(),
                },
            ],
            b"compressed".to_vec(),
        ),
        (
            vec![Header {
                name: "Content-Type".into(),
                value: b"application/x-www-form-urlencoded".to_vec(),
            }],
            (0..257)
                .map(|index| format!("k{index}=v{index}"))
                .collect::<Vec<_>>()
                .join("&")
                .into_bytes(),
        ),
    ] {
        let mut bad = interception_record();
        bad.request.headers = headers;
        bad.request.body = body;
        for response_stage in [false, true] {
            assert!(
                matches!(
                    negative(
                        &bad,
                        response_stage,
                        false,
                        false,
                        false,
                        eval_port(18080),
                        &cancel,
                        limits
                    ),
                    Err(mio_proxy::ProxyError::InvalidMessage(_))
                ),
                "DoesNotMatch must not invert malformed multipart or parameter limit errors"
            );
        }
    }

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(b"packed_k=packed_v").unwrap();
    let gzip_body = encoder.finish().unwrap();
    let mut compressed = interception_record();
    compressed.request.headers = vec![
        Header {
            name: "Content-Type".into(),
            value: b"application/x-www-form-urlencoded".to_vec(),
        },
        Header {
            name: "Content-Encoding".into(),
            value: b"gzip".to_vec(),
        },
    ];
    compressed.request.body = gzip_body;
    let mut match_settings = crate::ProxyInterceptionSettings::default();
    match_settings.request.rules =
        vec![interception_rule(1, And, ParamName, Matches, "^packed_k$")];
    let matcher = build_intercept_filter(match_settings, crate::TargetScope::default()).unwrap();
    assert!(
        !matcher(
            &compressed,
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
    let unpack = build_flow_transform(
        Vec::new(),
        crate::ProxyMessageSettings {
            unpack_compressed_requests: true,
            unpack_compressed_responses: false,
            ..Default::default()
        },
    )
    .unwrap()
    .unwrap();
    assert!(unpack(&mut compressed, false, false, &cancel, limits).unwrap());
    assert!(
        matcher(
            &compressed,
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
    let mut oversized_url = interception_record();
    oversized_url.request.url = format!("http://example.com/fixture?a={}", "x".repeat(MAX_HEAD));
    let Err(error) = intercept_request_parameters(&oversized_url.request, &cancel, limits) else {
        panic!("Proxy 参数条件没有拒绝超过 128 KiB 的请求 URL");
    };
    assert!(error.is::<crate::intruder::AutomaticParameterLimit>());
    for response_stage in [false, true] {
        assert!(matches!(
            negative(
                &oversized_url,
                response_stage,
                false,
                false,
                false,
                eval_port(18080),
                &cancel,
                limits
            ),
            Err(mio_proxy::ProxyError::InvalidMessage(_))
        ));
    }

    use std::cell::Cell;
    let part = b"--MioBound\r\nX-One: 1\r\nContent-Disposition: form-data; name=\"field\"\r\nBad Header\r\n\r\nbody\r\n--MioBound--\r\n";
    let matcher = regex::bytes::Regex::new("^last$").unwrap();
    for expected in [UnpackError::Cancelled, UnpackError::Deadline] {
        let calls = Cell::new(0);
        let mut pairs = Vec::new();
        let error =
            crate::intruder::intercept_multipart_parameters(part, b"MioBound", &mut pairs, &|| {
                calls.set(calls.get() + 1);
                if calls.get() == 5 {
                    Err(expected.clone().into())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert_eq!(error.downcast_ref::<UnpackError>(), Some(&expected));
        assert!(pairs.is_empty());
        assert_eq!(calls.get(), 5);
        let calls = Cell::new(0);
        let error = intercept_pairs_match(
            [b"first".as_slice(), b"second", b"last"].into_iter(),
            &matcher,
            &|| {
                calls.set(calls.get() + 1);
                if calls.get() == 4 {
                    Err(expected.clone().into())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert_eq!(error.downcast_ref::<UnpackError>(), Some(&expected));
        assert_eq!(calls.get(), 4);
    }
}
