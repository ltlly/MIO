use super::*;

#[test]
fn proxy_message_transform_keeps_head_response_separate_from_the_head_request_body() {
    use std::io::Write;
    let cancel = CancellationToken::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let payload = vec![b'X'; 128];
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&payload).unwrap();
    let gzip = encoder.finish().unwrap();
    let mut record = interception_record();
    record.request.method = "HEAD".into();
    record.summary.method = "HEAD".into();
    record.request.body = gzip;
    record.request.headers = vec![Header {
        name: "Content-Encoding".into(),
        value: b"gzip".to_vec(),
    }];
    record.summary.request_bytes = record.request.body.len() as u64;
    record.response.as_mut().unwrap().headers.push(Header {
        name: "Content-Encoding".into(),
        value: b"gzip".to_vec(),
    });
    let original = record.clone();
    let transform = build_flow_transform(
        Vec::new(),
        crate::ProxyMessageSettings {
            unpack_compressed_requests: true,
            unpack_compressed_responses: true,
            ..Default::default()
        },
    )
    .unwrap()
    .unwrap();
    for response_head_only in [false, true] {
        assert!(
            !transform(
                &mut record,
                true,
                response_head_only,
                &cancel,
                (deadline, payload.len()),
            )
            .unwrap()
        );
        assert_eq!(record, original);
    }
    assert!(matches!(
        transform(
            &mut record,
            false,
            false,
            &cancel,
            (deadline, payload.len() - 1)
        ),
        Err(mio_proxy::ProxyError::InvalidMessage(_))
    ));
    assert_eq!(record, original);
    assert!(
        transform(
            &mut record,
            false,
            false,
            &cancel,
            (deadline, payload.len())
        )
        .unwrap()
    );
    assert_eq!(record.request.body, payload);
    assert_eq!(record.summary.request_bytes, payload.len() as u64);
    assert_eq!(record.request.method, original.request.method);
    assert_eq!(record.request.url, original.request.url);
    assert_eq!(record.response, original.response);
    let decoded = record.clone();
    assert!(!transform(&mut record, true, false, &cancel, (deadline, payload.len())).unwrap());
    assert_eq!(record, decoded);
    for response_stage in [false, true] {
        assert!(matches!(
            transform(
                &mut record,
                response_stage,
                false,
                &cancel,
                (std::time::Instant::now(), payload.len()),
            ),
            Err(mio_proxy::ProxyError::Timeout)
        ));
        assert_eq!(record, decoded);
    }
}

#[test]
fn proxy_message_transform_rejects_missing_actual_response_without_mutation() {
    let cancel = CancellationToken::new();
    let limits = (std::time::Instant::now() + Duration::from_secs(3), MAX_BODY);
    let rule = crate::MatchAndReplaceRule {
        id: crate::Uuid::new_v4(),
        enabled: true,
        item_type: crate::MatchReplaceTarget::ResponseHeader,
        match_pattern: String::new(),
        replace_with: "X-MIO-Test: value".into(),
        regex: false,
        comment: String::new(),
        ..Default::default()
    };
    let mut record = interception_record();
    record.response = None;
    let original = record.clone();
    for (rules, messages) in [
        (Vec::new(), crate::ProxyMessageSettings::default()),
        (
            vec![rule],
            crate::ProxyMessageSettings {
                unpack_compressed_requests: false,
                unpack_compressed_responses: false,
                ..Default::default()
            },
        ),
    ] {
        let transform = build_flow_transform(rules, messages).unwrap().unwrap();
        for response_head_only in [false, true] {
            assert!(matches!(
                transform(&mut record, true, response_head_only, &cancel, limits),
                Err(mio_proxy::ProxyError::InvalidMessage(_))
            ));
            assert_eq!(record, original);
        }
    }
}

#[test]
fn proxy_message_transform_uses_the_actual_body_budget_on_both_legs() {
    let cancel = CancellationToken::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut original = interception_record();
    original.request.body = b"original".to_vec();
    original.request.headers = vec![Header {
        name: "Content-Length".into(),
        value: b"8".to_vec(),
    }];
    original.summary.request_bytes = 8;
    let response = original.response.as_mut().unwrap();
    response.body = b"original".to_vec();
    response.headers.push(Header {
        name: "Content-Length".into(),
        value: b"8".to_vec(),
    });
    original.summary.response_bytes = 8;
    for (response_stage, item_type) in [
        (false, crate::MatchReplaceTarget::RequestBody),
        (true, crate::MatchReplaceTarget::ResponseBody),
    ] {
        for regex in [false, true] {
            let transform = build_flow_transform(
                vec![crate::MatchAndReplaceRule {
                    id: crate::Uuid::new_v4(),
                    enabled: true,
                    item_type,
                    match_pattern: "original".into(),
                    replace_with: "replacement".into(),
                    regex,
                    comment: String::new(),
                    ..Default::default()
                }],
                crate::ProxyMessageSettings {
                    unpack_compressed_requests: false,
                    unpack_compressed_responses: false,
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
            let mut record = original.clone();
            assert!(matches!(
                transform(&mut record, response_stage, false, &cancel, (deadline, 10)),
                Err(mio_proxy::ProxyError::InvalidMessage(_))
            ));
            assert_eq!(record, original);
            record = original.clone();
            assert!(
                transform(&mut record, response_stage, false, &cancel, (deadline, 11)).unwrap()
            );
            let (body, headers, summary_bytes) = if response_stage {
                assert_eq!(record.request, original.request);
                let response = record.response.as_ref().unwrap();
                (
                    &response.body,
                    &response.headers,
                    record.summary.response_bytes,
                )
            } else {
                assert_eq!(record.response, original.response);
                (
                    &record.request.body,
                    &record.request.headers,
                    record.summary.request_bytes,
                )
            };
            assert_eq!(body, b"replacement");
            assert_eq!(summary_bytes, 11);
            assert_eq!(
                headers
                    .iter()
                    .find(|header| header.name.eq_ignore_ascii_case("content-length"))
                    .unwrap()
                    .value,
                b"11"
            );
            assert_eq!(record.summary.id, original.summary.id);
            assert_eq!(record.summary.revision, original.summary.revision);
        }
    }
}

#[test]
fn bounded_match_replace_rejects_small_literal_regex_and_capture_expansion_before_mutation() {
    for (regex, pattern, replacement, source) in [
        (false, "a", "0123456789", "aaaa"),
        (true, "a", "0123456789", "aaaa"),
        (true, "(a+)", "$1$1$1$1", "aaaa"),
        (true, "(?:)", "$0+", "中文"),
    ] {
        let rule = crate::MatchAndReplaceRule {
            id: crate::Uuid::new_v4(),
            enabled: true,
            item_type: crate::MatchReplaceTarget::RequestBody,
            match_pattern: pattern.into(),
            replace_with: replacement.into(),
            regex,
            comment: String::new(),
            ..Default::default()
        };
        let compiled = regex.then(|| compile_rule_regex(pattern).unwrap());
        let expected = if let Some(regex) = &compiled {
            regex.replace_all(source, replacement).into_owned()
        } else {
            source.replace(pattern, replacement)
        };
        assert!(expected.len() > source.len());
        assert!(
            replace_text_with_rule(
                source,
                &rule,
                compiled.as_ref(),
                expected.len() - 1,
                &|| Ok(()),
            )
            .is_err(),
            "{pattern:?} / {replacement:?}"
        );
        for limit in [expected.len(), expected.len() + 1] {
            assert_eq!(
                replace_text_with_rule(source, &rule, compiled.as_ref(), limit, &|| Ok(()))
                    .unwrap(),
                Some(expected.clone())
            );
        }
    }
}

#[test]
fn bounded_match_replace_preserves_current_capture_grammar_and_utf8_boundaries() {
    for (pattern, source) in [
        (r"(?P<name>ab)(c)?", "ababc"),
        (r"(?P<名字>中)(文)?", "中中文"),
        (r"(a)(?:)", "a中a"),
        (r"(?:)", "中文"),
        (r"a*", "ba中"),
        (r"^|a?", "中a"),
    ] {
        let regex = compile_rule_regex(pattern).unwrap();
        for replacement in [
            "$0",
            "$1${2}",
            "$0001",
            "${+1}",
            "$name/${name}",
            "${名字}",
            "$$",
            "$$foo",
            "$$$1",
            "$$$$",
            "$",
            "$-",
            "$1a",
            "${1}a",
            "${}",
            "${none}",
            "${no close",
            "${no close $name end",
            "${a{b}",
            "${☃}",
            "$中文",
            "$9999999999999999999999999999999999",
            "$0$0${1}$1$$$missing",
            "中文${0}\0末尾",
        ] {
            let rule = crate::MatchAndReplaceRule {
                id: crate::Uuid::new_v4(),
                enabled: true,
                item_type: crate::MatchReplaceTarget::RequestBody,
                match_pattern: pattern.into(),
                replace_with: replacement.into(),
                regex: true,
                comment: String::new(),
                ..Default::default()
            };
            let expected = regex.replace_all(source, replacement).into_owned();
            let expected = (expected != source).then_some(expected);
            assert_eq!(
                replace_text_with_rule(source, &rule, Some(&regex), 256, &|| Ok(())).unwrap(),
                expected,
                "{pattern:?} / {replacement:?}"
            );
        }
    }
    for (pattern, replacement, source) in [
        ("中", "中文", "中中"),
        ("中", "", "中中"),
        ("中", "$1$$", "中中"),
        ("unused", "large-unused-replacement", "中文"),
    ] {
        let rule = crate::MatchAndReplaceRule {
            id: crate::Uuid::new_v4(),
            enabled: true,
            item_type: crate::MatchReplaceTarget::RequestBody,
            match_pattern: pattern.into(),
            replace_with: replacement.into(),
            regex: false,
            comment: String::new(),
            ..Default::default()
        };
        let expected = source.replace(pattern, replacement);
        let expected = (expected != source).then_some(expected);
        assert_eq!(
            replace_text_with_rule(source, &rule, None, 64, &|| Ok(())).unwrap(),
            expected
        );
    }
}

#[test]
fn bounded_match_replace_polls_control_between_small_matches_and_capture_fragments() {
    use std::cell::Cell;
    for (regex, pattern, replacement) in [
        (false, "a", "replacement"),
        (true, "a", "replacement"),
        (true, "(a)", "$1$1$1$1$1"),
    ] {
        let rule = crate::MatchAndReplaceRule {
            id: crate::Uuid::new_v4(),
            enabled: true,
            item_type: crate::MatchReplaceTarget::RequestBody,
            match_pattern: pattern.into(),
            replace_with: replacement.into(),
            regex,
            comment: String::new(),
            ..Default::default()
        };
        let compiled = regex.then(|| compile_rule_regex(pattern).unwrap());
        for expected in [UnpackError::Cancelled, UnpackError::Deadline] {
            let calls = Cell::new(0);
            let check = || {
                calls.set(calls.get() + 1);
                if calls.get() >= 3 {
                    Err(expected.clone().into())
                } else {
                    Ok(())
                }
            };
            let error = replace_text_with_rule("aaaaaaaa", &rule, compiled.as_ref(), 128, &check)
                .unwrap_err();
            assert_eq!(error.downcast_ref::<UnpackError>(), Some(&expected));
            assert_eq!(calls.get(), 3);
        }
    }
}

#[test]
fn bounded_match_replace_checks_storage_before_append_and_keeps_header_failures_distinct() {
    use std::cell::Cell;
    let mut output = String::new();
    append_replacement(&mut output, "1234", 8, &|| Ok(())).unwrap();
    append_replacement(&mut output, "5678", 8, &|| Ok(())).unwrap();
    assert_eq!(output, "12345678");
    assert!(output.capacity() <= 8);
    let original_capacity = output.capacity();
    assert!(append_replacement(&mut output, "9", 8, &|| Ok(())).is_err());
    assert_eq!(output, "12345678");
    assert_eq!(output.capacity(), original_capacity);
    let mut headers = vec![Header {
        name: "X-Test".into(),
        value: b"value".to_vec(),
    }];
    let original = headers.clone();
    let mut rule = crate::MatchAndReplaceRule {
        id: crate::Uuid::new_v4(),
        enabled: true,
        item_type: crate::MatchReplaceTarget::RequestHeader,
        match_pattern: "value".into(),
        replace_with: "x".repeat(MAX_HEAD + 1),
        regex: false,
        comment: String::new(),
        ..Default::default()
    };
    assert!(!apply_header_rule(&mut headers, &rule, None, &|| Ok(())).unwrap());
    assert_eq!(headers, original);
    rule.replace_with = "replacement".into();
    for expected in [UnpackError::Cancelled, UnpackError::Deadline] {
        let calls = Cell::new(0);
        let check = || {
            calls.set(calls.get() + 1);
            if calls.get() >= 4 {
                Err(expected.clone().into())
            } else {
                Ok(())
            }
        };
        let error = apply_header_rule(&mut headers, &rule, None, &check).unwrap_err();
        assert_eq!(error.downcast_ref::<UnpackError>(), Some(&expected));
        assert_eq!(headers, original);
        assert_eq!(calls.get(), 4);
    }
}
