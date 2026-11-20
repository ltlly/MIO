use std::time::{Duration, Instant};

use mio_proxy::SendConfig;
use tokio_util::sync::CancellationToken;

use super::*;

#[test]
fn session_rules_match_tools_scope_params_and_apply_ordered_actions_skipping_disabled() {
    let mut scope = TargetScope::default();
    scope.include.push(ScopeRule {
        id: Uuid::new_v4(),
        enabled: true,
        prefix: "https://app.example.com/api".into(),
        include_subdomains: false,
    });

    let rule_first = SessionRule {
        id: Uuid::new_v4(),
        name: "Inject v1".into(),
        enabled: true,
        tools: SessionToolScope {
            proxy: false,
            repeater: true,
            intruder: true,
            ..SessionToolScope::default()
        },
        condition: SessionRuleCondition {
            url_scope: SessionUrlScope::SuiteScope,
            param_condition: Some(SessionParamCondition::RequirePresent(vec![
                "account".into(),
            ])),
            header_contains: Some("host: app.example.com".into()),
        },
        actions: vec![
            SessionRuleAction::SetHeader {
                name: "X-Trace".into(),
                value: "step-1".into(),
                only_if_present: false,
            },
            SessionRuleAction::RewriteUrlParam {
                name: "sig".into(),
                value: "alpha".into(),
                remove: false,
                only_if_present: false,
            },
        ],
    };
    let rule_disabled = SessionRule {
        id: Uuid::new_v4(),
        name: "Disabled overwrite".into(),
        enabled: false,
        tools: SessionToolScope {
            proxy: true,
            repeater: true,
            intruder: true,
            ..SessionToolScope::default()
        },
        condition: SessionRuleCondition::default(),
        actions: vec![SessionRuleAction::SetHeader {
            name: "X-Trace".into(),
            value: "must-not-run".into(),
            only_if_present: false,
        }],
    };
    let rule_second = SessionRule {
        id: Uuid::new_v4(),
        name: "Override ordered".into(),
        enabled: true,
        tools: SessionToolScope {
            proxy: false,
            repeater: true,
            intruder: false,
            ..SessionToolScope::default()
        },
        condition: SessionRuleCondition {
            url_scope: SessionUrlScope::CustomPrefixes(vec!["https://app.example.com/api/".into()]),
            param_condition: None,
            header_contains: None,
        },
        actions: vec![
            SessionRuleAction::SetHeader {
                name: "X-Trace".into(),
                value: "step-2".into(),
                only_if_present: true,
            },
            SessionRuleAction::RewriteUrlParam {
                name: "obsolete".into(),
                value: String::new(),
                remove: true,
                only_if_present: true,
            },
        ],
    };

    let mut req = sample_request("https://app.example.com/api/orders?account=42&obsolete=1#top");
    assert!(!rule_matches_request(
        &rule_first,
        SessionTool::Proxy,
        &req,
        &scope
    ));
    assert!(!rule_matches_request(
        &rule_disabled,
        SessionTool::Repeater,
        &req,
        &scope
    ));
    assert!(rule_matches_request(
        &rule_first,
        SessionTool::Repeater,
        &req,
        &scope
    ));
    assert!(rule_matches_request(
        &rule_second,
        SessionTool::Repeater,
        &req,
        &scope
    ));

    for rule in [&rule_first, &rule_disabled, &rule_second] {
        if rule_matches_request(rule, SessionTool::Repeater, &req, &scope) {
            for action in &rule.actions {
                apply_rule_sync_action(action, &[], &mut req, 1_700_000_000_000).unwrap();
            }
        }
    }

    assert_eq!(
        req.url,
        "https://app.example.com/api/orders?account=42&sig=alpha#top"
    );
    let trace = req
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("x-trace"))
        .unwrap();
    assert_eq!(trace.value, b"step-2");

    let set_cookie_only_present = SessionRuleAction::SetCookie {
        name: "missing_ck".into(),
        value: "ignored".into(),
        only_if_present: true,
    };
    assert!(!apply_rule_sync_action(&set_cookie_only_present, &[], &mut req, 1_700_000_000_000).unwrap());

    let set_cookie_new = SessionRuleAction::SetCookie {
        name: "sid".into(),
        value: "v_100".into(),
        only_if_present: false,
    };
    assert!(apply_rule_sync_action(&set_cookie_new, &[], &mut req, 1_700_000_000_000).unwrap());
    let set_cookie_replace = SessionRuleAction::SetCookie {
        name: "sid".into(),
        value: "v_200".into(),
        only_if_present: true,
    };
    assert!(apply_rule_sync_action(&set_cookie_replace, &[], &mut req, 1_700_000_000_000).unwrap());
    let ck_hdr = req
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("cookie"))
        .unwrap();
    assert_eq!(ck_hdr.value, b"sid=v_200");

    req.body = b"user=legacy_user&role=guest".to_vec();
    req.headers.push(Header {
        name: "Content-Length".into(),
        value: req.body.len().to_string().into_bytes(),
    });
    let replace_regex = SessionRuleAction::ReplaceRequestText {
        match_pattern: r"role=([a-z]+)".into(),
        replace_with: "role=admin_$1".into(),
        regex: true,
    };
    assert!(apply_rule_sync_action(&replace_regex, &[], &mut req, 1_700_000_000_000).unwrap());
    assert_eq!(req.body, b"user=legacy_user&role=admin_guest");
    let cl = req
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-length"))
        .unwrap();
    assert_eq!(cl.value, req.body.len().to_string().into_bytes());
}

#[tokio::test]
async fn multi_step_macro_replays_in_order_extracts_tokens_updates_jar_and_honors_retry_and_cancel()
{
    let auth_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let auth_addr = auth_listener.local_addr().unwrap();
    let step1_hits = Arc::new(AtomicUsize::new(0));
    let step1_hits_srv = step1_hits.clone();

    let server = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = auth_listener.accept().await else {
                break;
            };
            let hits = step1_hits_srv.clone();
            tokio::spawn(async move {
                let raw = read_http_request(&mut stream).await;
                if raw.starts_with("GET /csrf ") {
                    let attempt = hits.fetch_add(1, Ordering::SeqCst);
                    if attempt == 0 {
                        write_http_response(&mut stream, "503 Service Unavailable", &[], "busy")
                            .await;
                    } else {
                        write_http_response(
                            &mut stream,
                            "200 OK",
                            &[("Set-Cookie", "pre_sid=pre99; Path=/; HttpOnly")],
                            "<input name=\"csrf\" value=\"tok_xyz_77\">",
                        )
                        .await;
                    }
                } else if raw.starts_with("POST /login ") {
                    assert!(
                        raw.to_ascii_lowercase().contains("cookie: pre_sid=pre99"),
                        "step 2 must carry Set-Cookie from step 1: {raw}"
                    );
                    assert!(
                        raw.contains("csrf=tok_xyz_77"),
                        "step 2 body must receive extracted csrf from step 1: {raw}"
                    );
                    write_http_response(
                        &mut stream,
                        "200 OK",
                        &[
                            ("Set-Cookie", "auth_sid=authed_888; Path=/; HttpOnly"),
                            ("X-Api-Ticket", "ticket_final_42"),
                        ],
                        "{\"nonce\":\"n_900\"}",
                    )
                    .await;
                }
            });
        }
    });

    let macro_id = Uuid::new_v4();
    let mut mac = SessionMacro {
        id: macro_id,
        name: "TwoStepLogin".into(),
        steps: vec![
            MacroStep {
                id: Uuid::new_v4(),
                method: "GET".into(),
                url: format!("http://{auth_addr}/csrf"),
                headers: Vec::new(),
                body: Vec::new(),
                use_cookie_jar: true,
                expected_status: Some(200),
                extractions: vec![MacroExtraction {
                    name: "csrf".into(),
                    source: MacroExtractSource::Delimiter {
                        start: "value=\"".into(),
                        end: "\"".into(),
                    },
                    target: MacroApplyTarget::UrlOrBodyParam,
                }],
            },
            MacroStep {
                id: Uuid::new_v4(),
                method: "POST".into(),
                url: format!("http://{auth_addr}/login"),
                headers: vec![Header {
                    name: "Content-Type".into(),
                    value: b"application/x-www-form-urlencoded".to_vec(),
                }],
                body: b"user=admin&csrf=placeholder".to_vec(),
                use_cookie_jar: true,
                expected_status: Some(200),
                extractions: vec![
                    MacroExtraction {
                        name: "api_ticket".into(),
                        source: MacroExtractSource::ResponseHeader {
                            header_name: "X-Api-Ticket".into(),
                        },
                        target: MacroApplyTarget::Header {
                            header_name: "X-Session-Ticket".into(),
                        },
                    },
                    MacroExtraction {
                        name: "nonce".into(),
                        source: MacroExtractSource::Regex {
                            pattern: "\"nonce\":\"([^\"]+)\"".into(),
                        },
                        target: MacroApplyTarget::UrlOrBodyParam,
                    },
                ],
            },
        ],
        failure_policy: MacroFailurePolicy::Abort,
        timeout_ms: 5000,
    };

    let mut jar = Vec::new();
    let cancel = CancellationToken::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    let err = execute_macro(&mac, &mut jar, &SendConfig::default(), &cancel, deadline)
        .await
        .unwrap_err();
    assert!(matches!(err, SessionExecError::Failed(_)));
    assert!(jar.is_empty());
    assert_eq!(step1_hits.load(Ordering::SeqCst), 1);

    step1_hits.store(0, Ordering::SeqCst);
    mac.failure_policy = MacroFailurePolicy::Retry { max_retries: 1 };
    let extracted = execute_macro(&mac, &mut jar, &SendConfig::default(), &cancel, deadline)
        .await
        .unwrap();
    assert_eq!(step1_hits.load(Ordering::SeqCst), 2);
    assert!(
        jar.iter()
            .any(|c| c.name == "auth_sid" && c.value == "authed_888")
    );

    let mut final_req = HttpRequest {
        method: "GET".into(),
        url: format!("http://{auth_addr}/profile?nonce=old"),
        version: HttpVersion::Http11,
        headers: vec![Header {
            name: "Host".into(),
            value: auth_addr.to_string().into_bytes(),
        }],
        body: Vec::new(),
        trailers: Vec::new(),
    };
    apply_cookie_jar_to_request(&jar, &mut final_req, true, true, current_unix_ms()).unwrap();
    macros::apply_extractions_to_final_request(&mut final_req, &extracted).unwrap();
    assert!(final_req.url.contains("nonce=n_900"));
    assert_eq!(
        final_req
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("x-session-ticket"))
            .unwrap()
            .value,
        b"ticket_final_42"
    );
    let cookie_val = String::from_utf8(
        final_req
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("cookie"))
            .unwrap()
            .value
            .clone(),
    )
    .unwrap();
    assert!(cookie_val.contains("auth_sid=authed_888"));

    let pre_cancelled = CancellationToken::new();
    pre_cancelled.cancel();
    let cancel_err = execute_macro(
        &mac,
        &mut jar,
        &SendConfig::default(),
        &pre_cancelled,
        deadline,
    )
    .await
    .unwrap_err();
    assert_eq!(cancel_err, SessionExecError::Cancelled);

    server.abort();
}
