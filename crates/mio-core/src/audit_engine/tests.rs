use super::*;
use crate::HttpVersion::Http11;
use crate::target_issues::IssueSeverity;
use crate::{
    AuditStateSnapshot, AuditTaskMode, AuditTaskState, AuditTaskSummary, FlowRecord, FlowState,
    FlowSummary, Header, HttpRequest, HttpResponse, TrafficSource, response_to_raw,
};

fn rq(u: &str, h: &[(&str, &str)], b: &str) -> HttpRequest {
    let headers = h
        .iter()
        .map(|(k, v)| Header {
            name: (*k).into(),
            value: (*v).into(),
        })
        .collect();
    HttpRequest {
        method: if b.is_empty() { "GET" } else { "POST" }.into(),
        url: u.into(),
        version: Http11,
        headers,
        body: b.into(),
        trailers: vec![],
    }
}
fn rp(status: u16, h: &[(&str, &str)], b: &str) -> HttpResponse {
    let headers = h
        .iter()
        .map(|(k, v)| Header {
            name: (*k).into(),
            value: (*v).into(),
        })
        .collect();
    HttpResponse {
        status,
        version: Http11,
        reason_phrase: None,
        headers,
        body: b.into(),
        trailers: vec![],
    }
}

#[test]
fn passive_and_active_dedup_checks() {
    let h0 = [
        ("Content-Type", "text/html"),
        ("Set-Cookie", "s=1"),
        ("Access-Control-Allow-Origin", "*"),
        ("Access-Control-Allow-Credentials", "true"),
    ];
    let hits = inspect_passive(
        None,
        &rq("https://a.test/d", &[], ""),
        &rp(200, &h0, "<html>Index of / SQLSTATE[42]</html>"),
        1,
    );
    for id in
        "0x01000300|0x005009a0|0x00500600|0x00500200|0x00200601|0x00600100|0x00600080".split('|')
    {
        assert!(
            hits.iter()
                .any(|f| f.issue_id == id && f.severity().rank() == f.severity_rank)
        );
    }
    let ct = [
        ("Content-Type", "application/x-www-form-urlencoded"),
        ("Content-Length", "7"),
    ];
    assert!(
        inspect_passive(
            None,
            &rq("http://a.test/l", &ct[..1], "password=1"),
            &rp(200, &[], "ok"),
            2
        )
        .iter()
        .any(|f| f.issue_id == "0x00300100")
    );
    let base = rq("https://a.test/s?q=1&next=/h", &ct, "id=1000");
    let (pts, pay) = (
        extract_insertion_points(&base),
        format!("x<{ACTIVE_CANARY}>"),
    );
    let pr = build_probe_request(&base, &pts[2], &pay).unwrap();
    let body = format!("<{ACTIVE_CANARY}> SQLSTATE[42]");
    let (same, clean) = (rp(200, &[], &body), rp(200, &[], "ok"));
    assert!(evaluate_active_check(None, &pr, &pts[2], &pay, &same, &same, 3).is_empty());
    assert_eq!(
        evaluate_active_check(
            None,
            &pr,
            &pts[2],
            &pay,
            &clean,
            &rp(200, &[(ACTIVE_SPLIT_HEADER, "1")], &body),
            4
        )
        .len(),
        3
    );
    let rr = build_probe_request(&base, &pts[1], ACTIVE_REDIRECT_TARGET).unwrap();
    let (r0, r1) = (
        rp(302, &[("Location", "/h")], ""),
        rp(302, &[("Location", ACTIVE_REDIRECT_TARGET)], ""),
    );
    assert_eq!(
        evaluate_active_check(None, &rr, &pts[1], ACTIVE_REDIRECT_TARGET, &r0, &r1, 5)[0].issue_id,
        "0x00500100"
    );
}

#[test]
fn passive_cookie_samesite_and_cache_control_rules() {
    let missing_samesite = inspect_passive(
        None,
        &rq("https://app.test/profile", &[], ""),
        &rp(
            200,
            &[
                ("Set-Cookie", "sid=1; HttpOnly; Secure"),
                ("Strict-Transport-Security", "max-age=31536000"),
                ("Referrer-Policy", "no-referrer"),
                ("X-Content-Type-Options", "nosniff"),
            ],
            "ok",
        ),
        10,
    );
    assert!(missing_samesite.iter().any(|f| f.issue_id == "0x00500601"));
    assert!(missing_samesite.iter().any(|f| f.issue_id == "0x00700100"));

    let none_without_secure = inspect_passive(
        None,
        &rq("https://app.test/sso", &[], ""),
        &rp(
            200,
            &[
                ("Set-Cookie", "sid=1; SameSite=None; HttpOnly"),
                ("Cache-Control", "private, no-store"),
                ("Strict-Transport-Security", "max-age=31536000"),
                ("Referrer-Policy", "same-origin"),
                ("X-Content-Type-Options", "nosniff"),
            ],
            "ok",
        ),
        11,
    );
    let hit_none = none_without_secure
        .iter()
        .find(|f| f.issue_id == "0x00500601")
        .expect("SameSite=None without Secure must be flagged");
    assert!(hit_none.summary.contains("SameSite=None"));
    assert!(
        !none_without_secure
            .iter()
            .any(|f| f.issue_id == "0x00700100")
    );

    let cookie_named_samesite = inspect_passive(
        None,
        &rq("https://app.test/trick", &[], ""),
        &rp(
            200,
            &[
                ("Set-Cookie", "samesite=none; HttpOnly; Secure"),
                ("Cache-Control", "no-store"),
                ("Strict-Transport-Security", "max-age=31536000"),
                ("Referrer-Policy", "no-referrer"),
                ("X-Content-Type-Options", "nosniff"),
            ],
            "ok",
        ),
        12,
    );
    assert!(
        cookie_named_samesite
            .iter()
            .any(|f| f.issue_id == "0x00500601")
    );

    let auth_no_store_missing = inspect_passive(
        None,
        &rq(
            "https://api.test/me",
            &[("authorization", "Bearer tok_abc")],
            "",
        ),
        &rp(
            200,
            &[
                ("Cache-Control", "public, max-age=60"),
                ("Strict-Transport-Security", "max-age=31536000"),
                ("Referrer-Policy", "no-referrer"),
                ("X-Content-Type-Options", "nosniff"),
            ],
            "{}",
        ),
        13,
    );
    let auth_hit = auth_no_store_missing
        .iter()
        .find(|f| f.issue_id == "0x00700100")
        .expect("Authorization response without no-store must be flagged");
    assert!(!auth_hit.request_highlights.is_empty());
    assert!(!auth_hit.response_highlights.is_empty());
}

#[test]
fn passive_mixed_content_security_headers_and_flow_record_inspection() {
    let html_script = "<html><head><!--<script src=\"http://ignored.test/x.js\"></script>--><script defer src=\"http://cdn.plain.test/app.js\"></script></head><body>ok</body></html>";
    let resp_script = rp(
        200,
        &[
            ("Content-Type", "text/html; charset=utf-8"),
            ("Strict-Transport-Security", "max-age=31536000"),
            ("Content-Security-Policy", "frame-ancestors 'none'"),
        ],
        html_script,
    );
    let raw = response_to_raw(&resp_script);
    let hits = inspect_passive(
        None,
        &rq("https://portal.test/index", &[], ""),
        &resp_script,
        20,
    );
    let mixed = hits
        .iter()
        .find(|f| f.issue_id == "0x01000400")
        .expect("HTTPS HTML loading http:// script must be flagged");
    let span = mixed.response_highlights[0];
    assert_eq!(
        &raw[span.offset..span.offset + span.length],
        b"<script defer src=\"http://cdn.plain.test/app.js\">"
    );
    assert!(hits.iter().any(|f| f.issue_id == "0x005009b0"));
    assert!(hits.iter().any(|f| f.issue_id == "0x005009c0"));

    let form_hits = inspect_passive(
        None,
        &rq("https://portal.test/login", &[], ""),
        &rp(
            200,
            &[
                ("Content-Type", "text/html"),
                ("Strict-Transport-Security", "max-age=31536000"),
                ("X-Frame-Options", "DENY"),
                ("Referrer-Policy", "strict-origin-when-cross-origin"),
                ("X-Content-Type-Options", "nosniff"),
            ],
            "<html><body><form onsubmit=\"if(2>1)return true\" action='http://plain.test/post'></form></body></html>",
        ),
        21,
    );
    assert_eq!(form_hits.len(), 1);
    assert_eq!(form_hits[0].issue_id, "0x01000400");

    let compliant_flow = FlowRecord {
        summary: FlowSummary {
            id: Uuid::new_v4(),
            revision: 1,
            source: TrafficSource::Proxy,
            method: "GET".into(),
            url: "https://safe.test/doc".into(),
            status: Some(200),
            state: FlowState::Complete,
            request_bytes: 0,
            response_bytes: 64,
            started_unix_ms: 30,
            elapsed_ms: 5,
        },
        request: rq("https://safe.test/doc", &[], ""),
        response: Some(rp(
            200,
            &[
                ("Content-Type", "text/html"),
                ("Strict-Transport-Security", "max-age=31536000"),
                ("Content-Security-Policy", "frame-ancestors 'none'"),
                ("Set-Cookie", "sid=ok; Secure; HttpOnly; SameSite=Strict"),
                ("Cache-Control", "no-store"),
                ("Referrer-Policy", "no-referrer"),
                ("X-Content-Type-Options", "nosniff"),
            ],
            "<html><a href=\"http://plain-link.test/\">link</a><script src=\"https://safe.test/a.js\"></script></html>",
        )),
    };
    assert!(inspect_flow_passive(&compliant_flow, 31).is_empty());

    let mut pending_flow = compliant_flow;
    pending_flow.summary.state = FlowState::Pending;
    pending_flow.response = None;
    assert!(inspect_flow_passive(&pending_flow, 32).is_empty());

    let no_content = inspect_passive(
        None,
        &rq("https://safe.test/ping", &[], ""),
        &rp(
            204,
            &[("Strict-Transport-Security", "max-age=31536000")],
            "",
        ),
        33,
    );
    assert!(no_content.is_empty());
}

#[test]
fn report_renderers_escape_xss_and_summarize_all_severities() {
    let mk = |sev: IssueSeverity, id: &str, phase: AuditPhase| AuditFinding {
        id: Uuid::new_v4(),
        issue_id: id.into(),
        title: format!("Title <script>alert('{id}')</script>"),
        severity_label: sev.label().into(),
        severity_rank: sev.rank(),
        confidence: IssueConfidence::Certain,
        phase,
        host: "evil.test\" onload=\"x()".into(),
        url: "javascript:alert(1)//<img src=x onerror=alert('u')>".into(),
        parameter: Some("q\"><svg onload=alert(2)>".into()),
        flow_id: None,
        summary: "Summary & <b>bold</b>".into(),
        evidence: "</pre><script>evil_ev()</script>".into(),
        request_highlights: vec![EvidenceSpan {
            offset: 4,
            length: 8,
        }],
        response_highlights: vec![],
        remediation: "Use 'nosniff' & \"no-store\" <safe>".into(),
        detected_unix_ms: 1_700_000_000_000,
    };
    let snapshot = AuditStateSnapshot {
        revision: 7,
        live_passive_enabled: true,
        live_passive_crawl_enabled: true,
        live_passive_suite_scope_only: false,
        tasks: vec![AuditTaskSummary {
            id: Uuid::new_v4(),
            title: "Task <img src=1 onerror=alert(9)>".into(),
            target_url: "https://t.test/?x=<script>1</script>".into(),
            mode: AuditTaskMode::PassiveOnly,
            state: AuditTaskState::Completed,
            requests_sent: 1,
            total_planned: 1,
            issues_found: 4,
            started_unix_ms: 100,
            error: Some("err <svg/onload=alert(3)>".into()),
        }],
        findings: vec![
            mk(
                IssueSeverity::Information,
                "0x005009b0",
                AuditPhase::Passive,
            ),
            mk(IssueSeverity::High, "0x00200300", AuditPhase::Active),
            mk(IssueSeverity::Low, "0x00500601", AuditPhase::Passive),
            mk(IssueSeverity::Medium, "0x01000400", AuditPhase::Passive),
        ],
    };

    let html = render_audit_report_html(&snapshot);
    assert!(!html.contains("<script>"));
    assert!(!html.contains("</script>"));
    assert!(!html.contains("<img src="));
    assert!(!html.contains("<svg"));
    assert!(!html.contains("href=\"javascript:"));
    assert!(html.contains("&lt;script&gt;evil_ev()&lt;/script&gt;"));
    assert!(html.contains("&quot;&gt;&lt;svg onload=alert(2)&gt;"));
    assert!(html.contains("Use &#39;nosniff&#39; &amp; &quot;no-store&quot; &lt;safe&gt;"));
    for row in [
        "<tr><td>High (高危)</td><td>1</td></tr>",
        "<tr><td>Medium (中危)</td><td>1</td></tr>",
        "<tr><td>Low (低危)</td><td>1</td></tr>",
        "<tr><td>Information (信息)</td><td>1</td></tr>",
        "<tr><td>Total (总计)</td><td>4</td></tr>",
    ] {
        assert!(html.contains(row), "missing summary row: {row}");
    }

    let json_bytes = render_audit_report_json(&snapshot).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&json_bytes).unwrap();
    assert_eq!(parsed["format"], "mio.audit_report.v1");
    assert_eq!(parsed["summary"]["total"], 4);
    assert_eq!(parsed["summary"]["high"], 1);
    assert_eq!(parsed["summary"]["medium"], 1);
    assert_eq!(parsed["summary"]["low"], 1);
    assert_eq!(parsed["summary"]["information"], 1);
    assert_eq!(parsed["summary"]["passive"], 3);
    assert_eq!(parsed["summary"]["active"], 1);
    assert_eq!(parsed["findings"][0]["severity"], "High");
    assert_eq!(parsed["findings"][1]["severity"], "Medium");
    assert_eq!(parsed["findings"][2]["severity"], "Low");
    assert_eq!(parsed["findings"][3]["severity"], "Information");
}

#[test]
fn active_checks_cover_structured_points_and_passive_cors_null_and_jwt_alg_none() {
    let req = rq(
        "https://api.example.com/v1/echo?q=1",
        &[
            ("Content-Type", "application/json"),
            ("Cookie", "sid=tok1; theme=dark"),
            ("X-Custom-Tag", "v1"),
        ],
        r#"{"user":{"name":"alice"}}"#,
    );
    let pts = extract_insertion_points(&req);
    assert!(
        pts.iter()
            .any(|p| p.location == InsertionLocation::Json && p.name == "user.name")
    );
    assert!(
        pts.iter()
            .any(|p| p.location == InsertionLocation::Cookie && p.name == "theme")
    );
    assert!(
        pts.iter()
            .any(|p| p.location == InsertionLocation::Header && p.name == "X-Custom-Tag")
    );
    let json_pt = pts
        .iter()
        .find(|p| p.location == InsertionLocation::Json && p.name == "user.name")
        .unwrap();
    let payload = format!("alice<{ACTIVE_CANARY}>");
    let probe_req = build_probe_request(&req, json_pt, &payload).unwrap();
    let hits = evaluate_active_check(
        None,
        &probe_req,
        json_pt,
        &payload,
        &rp(200, &[], r#"{"echo":"alice"}"#),
        &rp(200, &[], &format!(r#"{{"echo":"{payload}"}}"#)),
        100,
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].issue_id, "0x00200300");
    assert_eq!(hits[0].parameter.as_deref(), Some("user.name"));
    assert!(!hits[0].request_highlights.is_empty());
    assert!(!hits[0].response_highlights.is_empty());

    let unsigned_jwt = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJzdWIiOiJhZG1pbiJ9.";
    let passive_hits = inspect_passive(
        None,
        &rq(
            "https://api.example.com/v1/me",
            &[("Authorization", &format!("Bearer {unsigned_jwt}"))],
            "",
        ),
        &rp(
            200,
            &[
                ("Strict-Transport-Security", "max-age=31536000"),
                ("Cache-Control", "no-store"),
                ("X-Content-Type-Options", "nosniff"),
                ("Referrer-Policy", "no-referrer"),
                ("Access-Control-Allow-Origin", "null"),
                ("Access-Control-Allow-Credentials", "true"),
            ],
            r#"{"ok":true}"#,
        ),
        101,
    );
    let cors_null = passive_hits
        .iter()
        .find(|f| f.issue_id == "0x00200602")
        .expect("CORS null origin + credentials must be flagged");
    assert_eq!(cors_null.severity(), IssueSeverity::Medium);
    assert!(!cors_null.response_highlights.is_empty());

    let jwt_hit = passive_hits
        .iter()
        .find(|f| f.issue_id == "0x00200903")
        .expect("unsigned alg=none JWT must be flagged");
    assert_eq!(jwt_hit.severity(), IssueSeverity::High);
    assert!(!jwt_hit.request_highlights.is_empty());

    let hs256_jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyIn0.c2ln";
    let signed_hits = inspect_passive(
        None,
        &rq(
            "https://api.example.com/v1/me",
            &[("Authorization", &format!("Bearer {hs256_jwt}"))],
            "",
        ),
        &rp(
            200,
            &[
                ("Strict-Transport-Security", "max-age=31536000"),
                ("Cache-Control", "no-store"),
                ("X-Content-Type-Options", "nosniff"),
                ("Referrer-Policy", "no-referrer"),
            ],
            r#"{"ok":true}"#,
        ),
        102,
    );
    assert!(!signed_hits.iter().any(|f| f.issue_id == "0x00200903"));
}

#[test]
fn scanner_inspects_bodies_past_1kb_and_avoids_double_url_encoding() {
    let padding = "A".repeat(4096);
    let req = rq(
        "https://app.test/search?q=hello%20world",
        &[("Content-Type", "application/x-www-form-urlencoded")],
        "note=foo+bar%21",
    );
    let pts = extract_insertion_points(&req);
    assert_eq!(pts[0].value, "hello world");
    assert_eq!(pts[1].value, "foo bar!");

    let payload = format!("{}<{ACTIVE_CANARY}>", pts[0].value);
    let probe_req = build_probe_request(&req, &pts[0], &payload).unwrap();
    assert!(
        probe_req.url.contains("q=hello%20world%3Cmio") && !probe_req.url.contains("%2520"),
        "query probe must not double-encode %20: {}",
        probe_req.url
    );

    let base_resp = rp(200, &[("Content-Type", "text/html")], &padding);
    let probe_body = format!("{padding}<div><{ACTIVE_CANARY}></div>");
    let probe_resp = rp(200, &[("Content-Type", "text/html")], &probe_body);
    let active_hits = evaluate_active_check(
        None,
        &probe_req,
        &pts[0],
        &payload,
        &base_resp,
        &probe_resp,
        200,
    );
    assert!(
        active_hits.iter().any(|f| f.issue_id == "0x00200300"),
        "reflection past 1024 chars must be detected"
    );
}
