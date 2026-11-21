use super::*;

#[test]
fn proxy_intercept_body_preserves_raw_request_and_reports_actual_control_and_unavailable_stream() {
    use crate::ProxyInterceptOperator::And;
    use crate::ProxyInterceptRelation::{Matches, RequestMatches, ResponseDoesNotMatch};
    use crate::ProxyInterceptType::Body;
    let cancel = CancellationToken::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let limits = (deadline, MAX_BODY);
    let mut settings = crate::ProxyInterceptionSettings::default();
    settings.request.rules = vec![interception_rule(1, And, Body, Matches, r"^\x00\xFFraw$")];
    settings.response.enabled = true;
    settings.response.rules = vec![interception_rule(
        2,
        And,
        Body,
        RequestMatches,
        r"^\x00\xFFraw$",
    )];
    let filter = build_intercept_filter(settings.clone(), crate::TargetScope::default()).unwrap();
    let mut record = interception_record();
    record.request.headers = vec![Header {
        name: "Content-Encoding".into(),
        value: b"gzip".to_vec(),
    }];
    record.request.body = b"\0\xffraw".to_vec();
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
    assert!(
        filter(
            &record,
            false,
            false,
            false,
            false,
            eval_port(18080),
            &cancel,
            (deadline, 1)
        )
        .is_err()
    );
    assert!(matches!(
        filter(
            &record,
            false,
            false,
            false,
            false,
            eval_port(18080),
            &cancel,
            (std::time::Instant::now(), MAX_BODY)
        ),
        Err(mio_proxy::ProxyError::Timeout)
    ));
    cancel.cancel();
    assert!(matches!(
        filter(
            &record,
            true,
            true,
            false,
            false,
            eval_port(18080),
            &cancel,
            limits
        ),
        Err(mio_proxy::ProxyError::Cancelled)
    ));
    let live_cancel = CancellationToken::new();
    settings.response.rules[0].relationship = ResponseDoesNotMatch;
    let streaming = build_intercept_filter(settings, crate::TargetScope::default()).unwrap();
    assert!(
        streaming(
            &record,
            true,
            true,
            false,
            false,
            eval_port(18080),
            &live_cancel,
            limits
        )
        .unwrap_err()
        .to_string()
        .contains("尚未完整接收")
    );
}

#[test]
fn proxy_message_unpack_settings_transform_real_bodies_before_matching_and_never_hide_decode_errors()
 {
    use crate::ProxyInterceptOperator::And;
    use crate::ProxyInterceptRelation::{Matches, ResponseMatches};
    use crate::ProxyInterceptType::Body;
    use std::io::Write;
    let cancel = CancellationToken::new();
    let limits = (std::time::Instant::now() + Duration::from_secs(3), MAX_BODY);
    let payload = b"MIO-RESPONSE-0123456789\0\xff";
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(payload).unwrap();
    let gzip = encoder.finish().unwrap();
    let mut record = interception_record();
    record.request.body = gzip.clone();
    record.request.headers = vec![Header {
        name: "Content-Encoding".into(),
        value: b"gzip".to_vec(),
    }];
    record.summary.request_bytes = gzip.len() as u64;
    record.response.as_mut().unwrap().body = gzip.clone();
    record.response.as_mut().unwrap().headers.push(Header {
        name: "Content-Encoding".into(),
        value: b"gzip".to_vec(),
    });
    record.summary.response_bytes = gzip.len() as u64;
    let original = record.clone();
    let mut settings = crate::ProxyInterceptionSettings::default();
    settings.request.rules = vec![interception_rule(
        1,
        And,
        Body,
        Matches,
        r"MIO-RESPONSE-0123456789\x00\xFF",
    )];
    settings.response.enabled = true;
    settings.response.rules = vec![interception_rule(
        2,
        And,
        Body,
        ResponseMatches,
        r"MIO-RESPONSE-0123456789\x00\xFF",
    )];
    let matcher = build_intercept_filter(settings, crate::TargetScope::default()).unwrap();
    assert!(
        !matcher(
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
        !matcher(
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
    let off = crate::ProxyMessageSettings {
        unpack_compressed_requests: false,
        unpack_compressed_responses: false,
        ..Default::default()
    };
    assert!(build_flow_transform(Vec::new(), off).unwrap().is_none());
    let default_transform =
        build_flow_transform(Vec::new(), crate::ProxyMessageSettings::default())
            .unwrap()
            .unwrap();
    assert!(!default_transform(&mut record, false, false, &cancel, limits).unwrap());
    assert_eq!(record, original);
    let mut streamed = original.clone();
    assert!(!default_transform(&mut streamed, true, true, &cancel, limits).unwrap());
    assert_eq!(streamed, original);
    assert!(default_transform(&mut record, true, false, &cancel, limits).unwrap());
    assert_eq!(record.request, original.request);
    assert_eq!(record.response.as_ref().unwrap().body, payload);
    assert_eq!(record.summary.response_bytes, payload.len() as u64);
    assert!(
        matcher(
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
    let request_transform = build_flow_transform(
        Vec::new(),
        crate::ProxyMessageSettings {
            unpack_compressed_requests: true,
            unpack_compressed_responses: false,
            ..Default::default()
        },
    )
    .unwrap()
    .unwrap();
    assert!(request_transform(&mut record, false, false, &cancel, limits).unwrap());
    assert_eq!(record.request.body, payload);
    assert_eq!(record.summary.request_bytes, payload.len() as u64);
    assert_eq!(record.request.url, original.request.url);
    assert!(
        matcher(
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
    let mut bad = original.clone();
    bad.response.as_mut().unwrap().body = b"notgzip".to_vec();
    let unchanged_bad = bad.clone();
    assert!(default_transform(&mut bad, true, false, &cancel, limits).is_err());
    assert_eq!(bad, unchanged_bad);
    let mut cancelled = original.clone();
    cancel.cancel();
    assert!(matches!(
        default_transform(&mut cancelled, true, false, &cancel, limits),
        Err(mio_proxy::ProxyError::Cancelled)
    ));
    assert_eq!(cancelled, original);
    assert!(
        build_flow_transform(
            vec![crate::MatchAndReplaceRule {
                id: crate::Uuid::new_v4(),
                enabled: true,
                item_type: crate::MatchReplaceTarget::ResponseBody,
                match_pattern: "(".into(),
                replace_with: "value".into(),
                regex: true,
                comment: String::new(),
                ..Default::default()
            }],
            off
        )
        .is_err()
    );
}

#[test]
fn proxy_intercept_negative_relations_keep_control_limits_and_missing_response_as_errors() {
    use crate::ProxyInterceptOperator::And;
    use crate::ProxyInterceptRelation::{DoesNotMatch, RequestDoesNotMatch, ResponseDoesNotMatch};
    use crate::ProxyInterceptType::Body;
    let cancel = CancellationToken::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut record = interception_record();
    record.request.body = b"\0\xffraw".to_vec();
    record.response.as_mut().unwrap().body = b"\0\xffraw".to_vec();
    let original = record.clone();
    for (response_stage, relationship) in [
        (false, DoesNotMatch),
        (true, RequestDoesNotMatch),
        (true, ResponseDoesNotMatch),
    ] {
        let mut settings = crate::ProxyInterceptionSettings::default();
        let rules = if response_stage {
            settings.response.enabled = true;
            &mut settings.response.rules
        } else {
            &mut settings.request.rules
        };
        *rules = vec![interception_rule(20, And, Body, relationship, "^missing$")];
        let filter = build_intercept_filter(settings, crate::TargetScope::default()).unwrap();
        assert!(
            filter(
                &record,
                response_stage,
                false,
                false,
                false,
                eval_port(18080),
                &cancel,
                (deadline, record.request.body.len()),
            )
            .unwrap()
            .should_intercept()
        );
        assert!(matches!(
            filter(
                &record,
                response_stage,
                false,
                false,
                false,
                eval_port(18080),
                &cancel,
                (deadline, record.request.body.len() - 1),
            ),
            Err(mio_proxy::ProxyError::InvalidMessage(_))
        ));
        assert!(matches!(
            filter(
                &record,
                response_stage,
                false,
                false,
                false,
                eval_port(18080),
                &cancel,
                (std::time::Instant::now(), MAX_BODY),
            ),
            Err(mio_proxy::ProxyError::Timeout)
        ));
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(matches!(
            filter(
                &record,
                response_stage,
                false,
                false,
                false,
                eval_port(18080),
                &cancelled,
                (deadline, MAX_BODY),
            ),
            Err(mio_proxy::ProxyError::Cancelled)
        ));
        assert_eq!(record, original);
        if response_stage {
            record.response = None;
            assert!(matches!(
                filter(
                    &record,
                    true,
                    false,
                    false,
                    false,
                    eval_port(18080),
                    &cancel,
                    (deadline, MAX_BODY),
                ),
                Err(mio_proxy::ProxyError::InvalidMessage(_))
            ));
            record = original.clone();
        }
    }
}
