use super::*;
use crate::store::ProjectStore;

#[test]
fn cookie_jar_enforces_domain_path_secure_httponly_and_expiry_rules() {
    let now = 1_700_000_000_000i64;
    let entries = vec![
        CookieJarEntry {
            id: Uuid::new_v4(),
            domain: "example.com".into(),
            host_only: false,
            path: "/".into(),
            name: "root_sid".into(),
            value: "v_root".into(),
            secure: false,
            http_only: true,
            expires_unix_ms: None,
        },
        CookieJarEntry {
            id: Uuid::new_v4(),
            domain: "api.example.com".into(),
            host_only: true,
            path: "/admin".into(),
            name: "admin_tok".into(),
            value: "sec_1".into(),
            secure: true,
            http_only: true,
            expires_unix_ms: Some(now + 60_000),
        },
        CookieJarEntry {
            id: Uuid::new_v4(),
            domain: "api.example.com".into(),
            host_only: true,
            path: "/".into(),
            name: "expired_tok".into(),
            value: "old".into(),
            secure: false,
            http_only: false,
            expires_unix_ms: Some(now),
        },
    ];

    let mut http_req = sample_request("http://api.example.com/admin/users");
    assert!(apply_cookie_jar_to_request(&entries, &mut http_req, true, true, now).unwrap());
    let cookie_hdr = http_req
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("cookie"))
        .unwrap();
    assert_eq!(
        std::str::from_utf8(&cookie_hdr.value).unwrap(),
        "root_sid=v_root"
    );

    let mut https_req = sample_request("https://api.example.com/admin/users");
    assert!(apply_cookie_jar_to_request(&entries, &mut https_req, true, true, now).unwrap());
    let cookie_hdr = https_req
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("cookie"))
        .unwrap();
    assert_eq!(
        std::str::from_utf8(&cookie_hdr.value).unwrap(),
        "admin_tok=sec_1; root_sid=v_root"
    );

    let mut boundary_req = sample_request("https://api.example.com/administrator");
    assert!(apply_cookie_jar_to_request(&entries, &mut boundary_req, true, true, now).unwrap());
    let cookie_hdr = boundary_req
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("cookie"))
        .unwrap();
    assert_eq!(
        std::str::from_utf8(&cookie_hdr.value).unwrap(),
        "root_sid=v_root"
    );

    let mut sub_req = sample_request("https://sub.api.example.com/admin");
    assert!(apply_cookie_jar_to_request(&entries, &mut sub_req, true, true, now).unwrap());
    let cookie_hdr = sub_req
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("cookie"))
        .unwrap();
    assert_eq!(
        std::str::from_utf8(&cookie_hdr.value).unwrap(),
        "root_sid=v_root"
    );
}

#[test]
fn set_cookie_ingestion_parses_flags_max_age_expires_and_rejects_foreign_domains() {
    let now = 1_700_000_000_000i64;
    let mut jar = Vec::new();
    let resp = HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![
            Header {
                name: "Set-Cookie".into(),
                value:
                    b"sid=live123; Domain=.example.com; Path=/app; Secure; HttpOnly; Max-Age=120"
                        .to_vec(),
            },
            Header {
                name: "Set-Cookie".into(),
                value: b"evil=1; Domain=attacker.org; Path=/".to_vec(),
            },
            Header {
                name: "Set-Cookie".into(),
                value: b"legacy=ok; Expires=Wed, 21 Oct 2099 07:28:00 GMT; Path=/".to_vec(),
            },
        ],
        body: Vec::new(),
        trailers: Vec::new(),
    };
    assert!(ingest_set_cookie_headers(
        &mut jar,
        "https://portal.example.com/app/login",
        &resp,
        now
    ));
    assert_eq!(jar.len(), 2);
    let sid = jar.iter().find(|c| c.name == "sid").unwrap();
    assert_eq!(sid.domain, "example.com");
    assert!(!sid.host_only);
    assert_eq!(sid.path, "/app");
    assert!(sid.secure);
    assert!(sid.http_only);
    assert_eq!(sid.expires_unix_ms, Some(now + 120_000));

    let legacy = jar.iter().find(|c| c.name == "legacy").unwrap();
    assert_eq!(legacy.domain, "portal.example.com");
    assert!(legacy.host_only);
    assert!(legacy.expires_unix_ms.unwrap() > now);

    let delete_resp = HttpResponse {
        status: 200,
        version: HttpVersion::Http11,
        reason_phrase: None,
        headers: vec![Header {
            name: "Set-Cookie".into(),
            value: b"sid=deleted; Domain=example.com; Path=/app; Max-Age=0".to_vec(),
        }],
        body: Vec::new(),
        trailers: Vec::new(),
    };
    assert!(ingest_set_cookie_headers(
        &mut jar,
        "https://portal.example.com/app/logout",
        &delete_resp,
        now + 1000
    ));
    assert!(!jar.iter().any(|c| c.name == "sid"));
}

#[test]
fn workspace_sessions_persist_across_create_save_reopen_and_enforce_cas() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("sessions-roundtrip.mio");

    let macro_id = Uuid::new_v4();
    let sessions = SessionSettings {
        cookie_jar: CookieJarSettings {
            monitor_proxy: true,
            monitor_repeater: true,
            monitor_intruder: false,
            entries: vec![CookieJarEntry {
                id: Uuid::new_v4(),
                domain: "corp.example".into(),
                host_only: false,
                path: "/v1".into(),
                name: "jwt".into(),
                value: "tok_abc".into(),
                secure: true,
                http_only: true,
                expires_unix_ms: Some(1_999_999_000_000),
            }],
                ..CookieJarSettings::default()
        },
        macros: vec![SessionMacro {
            id: macro_id,
            name: "Refresh CSRF".into(),
            steps: vec![MacroStep {
                id: Uuid::new_v4(),
                method: "GET".into(),
                url: "https://corp.example/csrf".into(),
                headers: Vec::new(),
                body: Vec::new(),
                use_cookie_jar: true,
                expected_status: Some(200),
                extractions: vec![MacroExtraction {
                    name: "csrf".into(),
                    source: MacroExtractSource::Delimiter {
                        start: "token=\"".into(),
                        end: "\"".into(),
                    },
                    target: MacroApplyTarget::Header {
                        header_name: "X-CSRF-Token".into(),
                    },
                }],
            }],
            failure_policy: MacroFailurePolicy::Retry { max_retries: 2 },
            timeout_ms: 5000,
        }],
        rules: vec![SessionRule {
            id: Uuid::new_v4(),
            name: "Corp Auth".into(),
            enabled: true,
            tools: SessionToolScope {
                proxy: true,
                repeater: true,
                intruder: true,
                    ..SessionToolScope::default()
            },
            condition: SessionRuleCondition {
                url_scope: SessionUrlScope::CustomPrefixes(vec!["https://corp.example/".into()]),
                param_condition: None,
                header_contains: None,
            },
            actions: vec![
                SessionRuleAction::UseCookieJar {
                    add_new_cookies: true,
                    update_existing_cookies: true,
                },
                SessionRuleAction::RunMacro {
                    macro_id,
                    update_cookies_from_jar: true,
                    apply_extractions: true,
                },
            ],
        }],
    };

    let mut store = ProjectStore::create(None).unwrap();
    let mut ws = store.workspace().unwrap();
    let stale = ws.clone();
    ws.revision += 1;
    ws.sessions = sessions.clone();
    store.save_workspace(&ws).unwrap();

    let mut conflict = stale;
    conflict.revision += 1;
    conflict.sessions = SessionSettings::default();
    assert!(store.save_workspace(&conflict).is_err());

    store.save(Some(&project_path)).unwrap();
    store.close().unwrap();

    let reopened = ProjectStore::open(&project_path).unwrap();
    let loaded = reopened.workspace().unwrap();
    assert_eq!(loaded.sessions, sessions);
    reopened.close().unwrap();
}
