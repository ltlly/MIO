use std::{io, time::Duration};

use prost::Message;

use crate::{Header, HttpRequest, HttpVersion, ProjectKey, Uuid, panel::Empty};

use super::*;

#[test]
fn oversized_prefix_fails_before_reading_or_allocating_payload() {
    for length in [0, MAX_FRAME_BYTES as u32 + 1, u32::MAX] {
        let mut reader = io::Cursor::new(length.to_be_bytes());
        assert_eq!(
            read_frame(&mut reader).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(reader.position(), 4);
    }
    assert!(decode_frame(&[0xff]).is_err());
}

#[test]
fn binary_http_dto_round_trip_keeps_duplicates_and_trailers() {
    let request = HttpRequest {
        method: "POST".into(),
        url: "https://localhost/".into(),
        version: HttpVersion::Http2,
        headers: vec![
            Header {
                name: "x-test".into(),
                value: b"first".to_vec(),
            },
            Header {
                name: "x-test".into(),
                value: vec![0xff],
            },
        ],
        body: vec![0, 0xff],
        trailers: vec![Header {
            name: "x-trailer".into(),
            value: vec![0xfe],
        }],
    };
    let wire = RequestDto::from(request.clone()).encode_to_vec();
    let decoded: HttpRequest = RequestDto::decode(wire.as_slice())
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(decoded, request);
}

#[test]
fn outgoing_http_payload_limits_are_checked_before_writing_a_frame() {
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
    };
    let request = RequestDto {
        method: "POST".into(),
        url: "http://127.0.0.1/".into(),
        version: HttpVersionDto::Http11 as i32,
        body: vec![0; MAX_INLINE_BODY + 1],
        ..Default::default()
    };
    let bodies = [
        envelope::Body::OpenRequest(OpenRequest {
            resource_id: Uuid::new_v4().as_bytes().to_vec(),
            message_revision: 1,
            read_only: false,
            request: Some(request),
        }),
        envelope::Body::HostResult(HostResult {
            outcome: Some(host_result::Outcome::Response(ResponseDto {
                status: 200,
                version: HttpVersionDto::Http11 as i32,
                body: vec![0; MAX_INLINE_BODY + 1],
                ..Default::default()
            })),
        }),
    ];
    for body in bodies {
        let envelope = Envelope::new(identity, 1, 1, body, Duration::from_secs(5));
        assert!(envelope.encoded_len() < MAX_FRAME_BYTES);
        assert!(
            envelope.validate().is_err(),
            "frame length alone does not enforce inline payload limits"
        );
    }
}

#[test]
fn envelope_checks_forgery_generation_and_replay() {
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: ProjectKey {
            id: Uuid::new_v4(),
            generation: 4,
        },
    };
    let e = Envelope::new(
        identity,
        1,
        0,
        envelope::Body::Shutdown(Empty {}),
        Duration::from_secs(1),
    );
    let frame = encode_frame(&e).unwrap();
    let decoded = read_frame(&mut frame.as_slice()).unwrap();
    decoded.check_channel(identity, 0).unwrap();
    assert!(decoded.check_channel(identity, 1).is_err());
    let mut forged = decoded;
    forged.project_generation += 1;
    assert!(forged.check_channel(identity, 0).is_err());
    forged.instance_id = Uuid::new_v4().as_bytes().to_vec();
    assert!(forged.check_channel(identity, 0).is_err());
}

#[test]
fn v13_preferences_response_editor_and_context_menu_frames_validate_limits() {
    assert_eq!(PROTOCOL_MINOR, 4);
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: ProjectKey {
            id: Uuid::new_v4(),
            generation: 2,
        },
    };
    let pref_set = HostCall {
        operation: Some(host_call::Operation::PreferencesSet(StorageEntry {
            key: "theme".into(),
            value: b"dark".to_vec(),
        })),
    };
    let decoded_call = HostCall::decode_bytes(&pref_set.encode_to_vec()).unwrap();
    assert_eq!(decoded_call, pref_set);

    let req = RequestDto {
        method: "GET".into(),
        url: "http://127.0.0.1/ctx".into(),
        version: HttpVersionDto::Http11 as i32,
        ..Default::default()
    };
    let resp = ResponseDto {
        status: 200,
        version: HttpVersionDto::Http11 as i32,
        body: b"ok".to_vec(),
        ..Default::default()
    };
    let open_resp = Envelope::new(
        identity,
        1,
        1,
        envelope::Body::OpenResponse(OpenResponse {
            resource_id: Uuid::new_v4().as_bytes().to_vec(),
            message_revision: 1,
            read_only: true,
            response: Some(resp.clone()),
        }),
        Duration::from_secs(1),
    );
    open_resp.validate().unwrap();

    let mut bad_ctx = Envelope::new(
        identity,
        2,
        1,
        envelope::Body::ContextMenuCall(ContextMenuCall {
            source: SourceDto::Repeater as i32,
            action_id: "default".into(),
            selection_start: 10,
            selection_end: 5,
            selected_bytes: vec![],
            request: Some(req),
            response: Some(resp),
        }),
        Duration::from_secs(1),
    );
    assert!(bad_ctx.validate().is_err());
    if let Some(envelope::Body::ContextMenuCall(call)) = &mut bad_ctx.body {
        call.selection_end = 10;
    }
    bad_ctx.validate().unwrap();
    bad_ctx.minor = 2;
    bad_ctx.validate().unwrap();
}

#[test]
fn passive_check_frames_validate_fields_and_round_trip() {
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
    };
    let flow_id = Uuid::new_v4().to_string();
    let req = HttpRequestDto {
        method: "GET".into(),
        url: "http://127.0.0.1/check".into(),
        version: HttpVersionDto::Http11 as i32,
        ..Default::default()
    };
    let resp = HttpResponseDto {
        status: 200,
        version: HttpVersionDto::Http11 as i32,
        body: b"PLUGIN_VULN_MARKER".to_vec(),
        ..Default::default()
    };
    let call_env = Envelope::new(
        identity,
        1,
        1,
        envelope::Body::PassiveCheckCall(PassiveCheckCall {
            flow_id,
            url: req.url.clone(),
            request: Some(req),
            response: Some(resp),
        }),
        Duration::from_secs(1),
    );
    let raw_call = encode_frame(&call_env).unwrap();
    assert_eq!(read_frame(&mut raw_call.as_slice()).unwrap(), call_env);

    let mut res_env = Envelope::new(
        identity,
        2,
        1,
        envelope::Body::PassiveCheckResult(PassiveCheckResult {
            findings: vec![PluginFindingDto {
                title: "插件被动检查告警".into(),
                detail: "检测到测试标记".into(),
                remediation: "移除调试标记".into(),
                severity: "Medium".into(),
                confidence: "Firm".into(),
                parameter: "body".into(),
                evidence: "PLUGIN_VULN_MARKER".into(),
            }],
            log_line: "passive hit".into(),
        }),
        Duration::from_secs(1),
    );
    res_env.validate().unwrap();
    if let Some(envelope::Body::PassiveCheckResult(res)) = &mut res_env.body {
        res.findings[0].title.clear();
    }
    assert!(res_env.validate().is_err());
}

#[test]
fn context_menu_result_replace_request_validates_bounds_and_round_trips() {
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
    };
    let mut env = Envelope::new(
        identity,
        3,
        1,
        envelope::Body::ContextMenuResult(ContextMenuResult {
            status_message: "已原地替换请求".into(),
            log_line: "context_menu replace_request".into(),
            open_repeater: None,
            replace_request: Some(b"GET /replaced HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n".to_vec()),
        }),
        Duration::from_secs(1),
    );
    let raw = encode_frame(&env).unwrap();
    assert_eq!(read_frame(&mut raw.as_slice()).unwrap(), env);
    if let Some(envelope::Body::ContextMenuResult(res)) = &mut env.body {
        res.replace_request = Some(vec![b'x'; MAX_INLINE_BODY + 1]);
    }
    assert!(env.validate().is_err());
}

#[test]
fn active_check_frames_validate_fields_and_round_trip() {
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
    };
    let req = HttpRequestDto {
        method: "GET".into(),
        url: "http://127.0.0.1/active?q=test".into(),
        version: HttpVersionDto::Http11 as i32,
        ..Default::default()
    };
    let resp = HttpResponseDto {
        status: 200,
        version: HttpVersionDto::Http11 as i32,
        body: b"ok".to_vec(),
        ..Default::default()
    };
    let mut call_env = Envelope::new(
        identity,
        1,
        1,
        envelope::Body::ActiveCheckCall(ActiveCheckCall {
            call_id: 7,
            flow_id: Uuid::new_v4().to_string(),
            base_request: Some(req),
            base_response: Some(resp),
            parameter_name: "q".into(),
            parameter_value: "PLUGIN_ACTIVE_PROBE".into(),
        }),
        Duration::from_secs(1),
    );
    let raw_call = encode_frame(&call_env).unwrap();
    assert_eq!(read_frame(&mut raw_call.as_slice()).unwrap(), call_env);
    if let Some(envelope::Body::ActiveCheckCall(call)) = &mut call_env.body {
        call.call_id = 0;
    }
    assert!(call_env.validate().is_err());

    let mut res_env = Envelope::new(
        identity,
        2,
        1,
        envelope::Body::ActiveCheckResult(ActiveCheckResult {
            call_id: 7,
            findings: vec![PluginFindingDto {
                title: "插件主动检查告警".into(),
                detail: "主动探测参数注入命中".into(),
                remediation: "对输入参数做严格校验".into(),
                severity: "High".into(),
                confidence: "Firm".into(),
                parameter: "q".into(),
                evidence: "PLUGIN_ACTIVE_PROBE".into(),
            }],
        }),
        Duration::from_secs(1),
    );
    let raw_res = encode_frame(&res_env).unwrap();
    assert_eq!(read_frame(&mut raw_res.as_slice()).unwrap(), res_env);
    if let Some(envelope::Body::ActiveCheckResult(res)) = &mut res_env.body {
        res.call_id = 0;
    }
    assert!(res_env.validate().is_err());
}

#[test]
fn v14_plugin_host_calls_and_results_roundtrip_and_validate() {
    let identity = ChannelIdentity {
        instance_id: Uuid::new_v4(),
        project: ProjectKey {
            id: Uuid::new_v4(),
            generation: 1,
        },
    };
    let flow_id = Uuid::new_v4();
    let req = RequestDto {
        method: "POST".into(),
        url: "https://example.com/api".into(),
        version: HttpVersionDto::Http11 as i32,
        ..Default::default()
    };
    let ops = [
        host_call::Operation::ScopeCheck("https://example.com/api".into()),
        host_call::Operation::ScopeUpdate(ScopeUpdateDto {
            url_prefix: "https://example.com/".into(),
            exclude: false,
            include_subdomains: true,
        }),
        host_call::Operation::QuerySiteMap(SiteMapQueryDto {
            prefix: "https://example.com".into(),
            in_scope_only: true,
            offset: 0,
            limit: 10,
        }),
        host_call::Operation::ReadAnnotations(flow_id.as_bytes().to_vec()),
        host_call::Operation::UpdateAnnotations(UpdateAnnotationsDto {
            flow_id: flow_id.as_bytes().to_vec(),
            notes: Some("verified".into()),
            highlight_color: Some(1),
        }),
        host_call::Operation::OpenOrganiser(OpenOrganiserDto {
            request: Some(req.clone()),
            response: None,
            notes: "triage".into(),
            highlight_color: Some(2),
        }),
        host_call::Operation::SubmitAuditIssue(SubmitAuditIssueDto {
            flow_id: flow_id.as_bytes().to_vec(),
            url: req.url.clone(),
            finding: Some(PluginFindingDto {
                title: "Issue".into(),
                detail: "Detail".into(),
                remediation: "Fix".into(),
                severity: "High".into(),
                confidence: "Certain".into(),
                parameter: "id".into(),
                evidence: "poc".into(),
            }),
        }),
        host_call::Operation::CollaboratorGenerate(CollaboratorGenerateDto {
            label: "ssrf".into(),
        }),
        host_call::Operation::CollaboratorPoll(CollaboratorPollDto {
            token_filter: "tok".into(),
            limit: 16,
        }),
    ];
    for op in ops {
        let call = HostCall {
            operation: Some(op.clone()),
        };
        let decoded = HostCall::decode_bytes(&call.encode_to_vec()).unwrap();
        assert_eq!(decoded, call);
    }

    let outcomes = [
        host_result::Outcome::ScopeStatus(ScopeStatusDto { in_scope: true }),
        host_result::Outcome::SiteMap(SiteMapPageDto {
            offset: 0,
            total: 1,
            rows: vec![SiteMapRowDto {
                id: flow_id.as_bytes().to_vec(),
                method: "POST".into(),
                url: "https://example.com/api".into(),
                status: Some(200),
                response_bytes: 12,
                in_scope: true,
                notes_preview: "note".into(),
                highlight_color: 1,
            }],
        }),
        host_result::Outcome::Annotations(AnnotationsDto {
            flow_id: flow_id.as_bytes().to_vec(),
            notes: "note".into(),
            highlight_color: 1,
            notes_revision: 1,
            highlight_revision: 1,
        }),
        host_result::Outcome::CollaboratorPayload(CollaboratorPayloadDto {
            token: "tok123".into(),
            label: "ssrf".into(),
            http_callback_url: "http://127.0.0.1:9000/tok123".into(),
            dns_qname: "tok123.oast.local".into(),
            created_unix_ms: 1_700_000_000_000,
        }),
        host_result::Outcome::CollaboratorInteractions(CollaboratorInteractionsDto {
            interactions: vec![CollaboratorInteractionDto {
                id: Uuid::new_v4().as_bytes().to_vec(),
                token: "tok123".into(),
                protocol: "http".into(),
                peer_addr: "127.0.0.1:12345".into(),
                summary: "GET /tok123".into(),
                raw_request: b"GET /tok123 HTTP/1.1\r\n\r\n".to_vec(),
                observed_unix_ms: 1_700_000_001_000,
            }],
        }),
    ];
    for (idx, outcome) in outcomes.into_iter().enumerate() {
        let res = HostResult {
            outcome: Some(outcome),
        };
        res.validate().unwrap();
        let env = Envelope::new(
            identity,
            (idx + 1) as u64,
            1,
            envelope::Body::HostResult(res),
            Duration::from_secs(1),
        );
        let raw = encode_frame(&env).unwrap();
        assert_eq!(read_frame(&mut raw.as_slice()).unwrap(), env);
    }

    let bad_color = HostResult {
        outcome: Some(host_result::Outcome::Annotations(AnnotationsDto {
            flow_id: flow_id.as_bytes().to_vec(),
            notes: String::new(),
            highlight_color: 10,
            notes_revision: 0,
            highlight_revision: 0,
        })),
    };
    assert!(bad_color.validate().is_err());
}
